//! Windows: how many buffers a window's swap chain has. It holds one frame the compositor hasn't
//! shown (`FRAME_LATENCY`), so two buffers: the one on screen and the one being drawn. The GPU
//! can't start a frame until DWM gives back the buffer it draws into, which it does when it
//! composes the frame before; while another window keeps the GPU busy, that round trip takes two
//! refreshes, and a moving window shows a new image every other refresh (a 240 Hz list scroll:
//! 223-302 of about 717 refreshes missed with the OpenCode desktop drawing beside it). A third
//! buffer lets the GPU start at once: 1-5 missed. The frame latency stays 1, so no frame of input
//! waits behind a queued one.
//!
//! A third buffer costs a window-sized buffer, so a window has it only while it's in use. Making
//! it (`ResizeBuffers`) waits for the GPU to finish and the new buffer is slow the first time it's
//! drawn into, so it's never made on a frame the user waits for: input that makes it likely (the
//! pointer reaching the window, focus, a show) asks for it, and the first turn with nothing
//! waiting (no input, no frame owed, the swap chain's room free) makes it and draws into each
//! buffer once. [`IDLE`] after the window's last input, or as it goes out of sight, it
//! goes back to two, on a turn when nothing is in flight. A resize takes two (each step configures
//! anyway); a swap chain made again starts at two. If the third can't be made, the window keeps
//! two for its life.

use std::time::Duration;

use crate::time::Instant;

/// How long after its last input a window keeps its third buffer.
pub(super) const IDLE: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Depth {
    /// Two buffers; `wanted`: input says the window is in use, so the next free turn makes a third.
    #[default]
    Two,
    Wanted,
    /// Three buffers, `warm` of them drawn into since they were made; last input at `last`.
    Three { warm: u8, last: Instant },
    /// Two for the window's life: a third couldn't be made, or the window draws with alpha.
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DepthEvent {
    /// Input reached the window (the pointer, a key, a wheel), it gained focus, or it was shown.
    Likely { now: Instant },
    /// A frame was presented.
    Presented { now: Instant },
    /// A turn with no input waiting, no frame owed and the swap chain's room free; `seen` is
    /// whether the window can be seen.
    Free { now: Instant, seen: bool },
    /// The swap chain was configured to another size or made again: it has two.
    Remade,
    /// Making the third failed.
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DepthEffect {
    None,
    /// Configure with three buffers, then draw.
    Grow,
    /// Draw a frame into the next buffer not yet drawn into.
    Warm,
    /// Configure with two buffers, then draw.
    Shrink,
}

impl Depth {
    pub(super) fn step(self, event: DepthEvent) -> (Depth, DepthEffect) {
        use {Depth as D, DepthEffect as F, DepthEvent as E};
        match (self, event) {
            (D::Off, _) | (_, E::Failed) => (D::Off, F::None),
            (_, E::Remade) => (D::Two, F::None),
            (D::Two | D::Wanted, E::Likely { .. }) => (D::Wanted, F::None),
            (D::Two | D::Wanted, E::Presented { .. }) | (D::Two, E::Free { .. }) => (self, F::None),
            (D::Wanted, E::Free { now, seen: true }) => (D::Three { warm: 0, last: now }, F::Grow),
            (D::Wanted, E::Free { seen: false, .. }) => (D::Two, F::None),
            (D::Three { warm, .. }, E::Likely { now }) => (D::Three { warm, last: now }, F::None),
            // A frame doesn't keep the third buffer: an animation's frames leave no free turn.
            (D::Three { warm, last }, E::Presented { .. }) => (D::Three { warm: (warm + 1).min(3), last }, F::None),
            (D::Three { last, .. }, E::Free { now, seen }) if !seen || now >= last + IDLE => (D::Two, F::Shrink),
            (D::Three { warm, last }, E::Free { .. }) if warm < 3 => (D::Three { warm, last }, F::Warm),
            (D::Three { .. }, E::Free { .. }) => (self, F::None),
        }
    }

    /// When a turn must come to give the third buffer back. Once past, the next turn that comes
    /// for anything else does it: a swap chain whose room isn't free (a covered window's frame
    /// DWM hasn't taken) mustn't spin the loop.
    pub(super) fn deadline(self, now: Instant) -> Option<Instant> {
        match self { Depth::Three { last, .. } if now < last + IDLE => Some(last + IDLE), _ => None }
    }

    pub(super) fn extra(self) -> u32 { matches!(self, Depth::Three { .. }) as u32 }
}

impl<S: 'static> super::Host<S> {
    /// One step of the window's buffer count, and what it asks for.
    pub(super) fn depth_step(&mut self, event: DepthEvent) {
        let Some(w) = self.win.as_mut() else { return };
        let (next, effect) = w.native.depth.step(event);
        w.native.depth = next;
        match effect {
            DepthEffect::None => {}
            DepthEffect::Grow | DepthEffect::Shrink => self.configure_depth(),
            DepthEffect::Warm => {
                w.presented = None;
                w.request_redraw();
            }
        }
    }

    /// At the end of a turn: a free turn grows or gives back the third buffer, or draws into a
    /// buffer not yet drawn into. Free: no input waiting, no frame on its way, the swap chain's
    /// room free, the GPU drawing it.
    pub(super) fn depth_turn(&mut self) {
        let seen = self.shown_win().is_some();
        let Some(w) = self.win.as_mut() else { return };
        if matches!(w.native.depth, Depth::Two | Depth::Off) { return; }
        if w.alpha.is_some() { return self.depth_step(DepthEvent::Failed); }
        let gpu_draws = matches!(w.native.mode, super::cpu_frame::SurfaceMode::Gpu | super::cpu_frame::SurfaceMode::Handed { .. });
        if !gpu_draws || !w.surface_ok || w.redraw.get() != super::redraw::Redraw::Idle || super::post_frame::input_waiting() { return; }
        // SAFETY: the object is the live swap chain's; a zero timeout only polls it.
        let waitable = w.surface.as_ref().and_then(|s| unsafe { s.as_hal::<wgpu::hal::api::Dx12>().and_then(|h| h.waitable_handle()) });
        if !w.native.slot.room(waitable.map(|h| h.0)) { return; }
        self.depth_step(DepthEvent::Free { now: Instant::now(), seen });
    }

    /// Configures the window's swap chain with the buffers its depth says, at its size. If the
    /// third can't be made, two, for the window's life.
    fn configure_depth(&mut self) {
        let (Some(w), Some((gpu, _))) = (self.win.as_mut(), self.gpu.as_ref()) else { return };
        let Some(surface) = w.surface.as_ref() else { return };
        let extra = w.native.depth.extra();
        // SAFETY: only sets a count the next configure reads.
        let set = |extra| unsafe { surface.as_hal::<wgpu::hal::api::Dx12>().map(|h| h.set_extra_buffers(extra)).is_some() };
        if !set(extra) { w.native.depth = Depth::Off; return; }
        let oom = gpu.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let configured = super::configure(surface, gpu, &w.config);
        let refused = pollster::block_on(oom.pop()).is_some() || configured.is_err();
        if refused && extra > 0 {
            set(0);
            w.native.depth = Depth::Off;
            w.surface_ok = super::configure(surface, gpu, &w.config).is_ok();
        } else if configured.is_err() {
            w.surface_ok = false;
        }
        w.native.slot.reset();
        w.native.damage.reset();
        w.presented = None;
        // What DWM shows next comes from the new buffers: a frame fills them. A window out of
        // sight draws when it's shown.
        if self.life.shown() { w.request_redraw(); }
    }
}
