//! Windows: how many buffers a window's swap chain has. It holds one frame the compositor hasn't
//! shown (`FRAME_LATENCY`), so two buffers: the one on screen and the one being drawn. The GPU
//! can't start a frame until DWM gives back the buffer it draws into, which it does when it
//! composes the frame before; while another window keeps the GPU busy, that round trip takes two
//! refreshes, and a moving window shows a new image every other refresh (a 240 Hz list scroll:
//! 223-324 of about 717 refreshes missed with the OpenCode desktop drawing beside it). A third
//! buffer lets the GPU start at once: 1-16 missed. The frame latency stays 1, so no frame of input
//! waits behind a queued one.
//!
//! The window the user works in keeps a third buffer: the focused window, and for [`UNFOCUSED`]
//! after it loses focus. Read a long reply for minutes, then roll the wheel: the scroll has it. A
//! window unfocused that long, or hidden, minimised or covered whole, goes back to two.
//!
//! Making it (`ResizeBuffers`, 13-16 ms on blade's Iris) waits for the GPU to finish, and a new
//! buffer is slow the first time it's drawn into, so it's never made on a frame the user waits for:
//! only on a free turn (no input waiting, no frame on its way, the swap chain's room free), and
//! not within [`HOLD`] of a wheel, key, press, touch or resize step, so a click that focuses the
//! window has its frame first and a scroll that comes before the grow runs on two until it pauses.
//! Then each buffer is drawn into once. A resize or a swap chain made again starts at two and
//! grows again on the next free turn; if the third can't be made, the window keeps two for its life.

use std::time::Duration;

use crate::time::Instant;

/// How long after losing focus a window keeps its third buffer.
pub(super) const UNFOCUSED: Duration = Duration::from_secs(60);
/// How long after a wheel, key, press, touch or resize step no third buffer is made: a free turn
/// between two steps of a scroll or a click isn't free for the 13-16 ms `ResizeBuffers` takes.
pub(super) const HOLD: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Depth {
    /// Two buffers; no third is made before `hold`.
    Two { hold: Instant },
    /// Three buffers, `warm` of them drawn into since they were made.
    Three { warm: u8 },
    /// Two for the window's life: a third couldn't be made, or the window draws with alpha.
    Off,
}

impl Default for Depth {
    fn default() -> Self { Depth::Two { hold: Instant::now() } }
}

/// Whether the user works in the window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Focus {
    /// Not focused since it opened.
    #[default]
    Never,
    Focused,
    /// Lost focus at the instant.
    Since(Instant),
}

impl Focus {
    /// Whether the window keeps a third buffer at `now`.
    pub(super) fn keeps(self, now: Instant) -> bool {
        match self { Focus::Focused => true, Focus::Since(at) => now < at + UNFOCUSED, Focus::Never => false }
    }

    /// When it stops keeping one.
    fn ends(self) -> Option<Instant> {
        match self { Focus::Since(at) => Some(at + UNFOCUSED), _ => None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DepthEvent {
    /// A wheel, key, press, touch or resize step: its frames come now.
    Active { now: Instant },
    /// A frame was presented.
    Presented,
    /// A turn with no input waiting, no frame on its way and the swap chain's room free. `keep`:
    /// the window is focused (or was, within `UNFOCUSED`) and can be seen.
    Free { now: Instant, keep: bool },
    /// The swap chain was configured to another size or made again: it has two.
    Remade { now: Instant },
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
            (_, E::Remade { now }) => (D::Two { hold: now + HOLD }, F::None),
            (D::Two { hold }, E::Active { now }) => (D::Two { hold: hold.max(now + HOLD) }, F::None),
            (D::Three { .. }, E::Active { .. }) | (D::Two { .. }, E::Presented) => (self, F::None),
            (D::Two { hold }, E::Free { now, keep: true }) if now >= hold => (D::Three { warm: 0 }, F::Grow),
            (D::Two { .. }, E::Free { .. }) => (self, F::None),
            (D::Three { warm }, E::Presented) => (D::Three { warm: (warm + 1).min(3) }, F::None),
            (D::Three { .. }, E::Free { now, keep: false }) => (D::Two { hold: now }, F::Shrink),
            (D::Three { warm }, E::Free { keep: true, .. }) if warm < 3 => (self, F::Warm),
            (D::Three { .. }, E::Free { .. }) => (self, F::None),
        }
    }

    /// When a turn must come: to grow once the hold ends while the window keeps a third buffer,
    /// or to give it back when focus has been gone long enough. Once past, the next turn that
    /// comes for anything else does it: a swap chain whose room isn't free (a covered window's
    /// frame DWM hasn't taken) mustn't spin the loop.
    pub(super) fn deadline(self, focus: Focus, now: Instant) -> Option<Instant> {
        let at = match self {
            Depth::Two { hold } if focus.keeps(now) => Some(hold),
            Depth::Three { .. } => focus.ends(),
            _ => None,
        };
        at.filter(|at| now < *at)
    }

    pub(super) fn extra(self) -> u32 { matches!(self, Depth::Three { .. }) as u32 }
}

impl<S: 'static> super::Host<S> {
    /// One step of the window's buffer count, and what it asks for.
    pub(super) fn depth_step(&mut self, event: DepthEvent) {
        let Some(w) = self.win.as_mut() else { return };
        // Measuring only: RUNE_DEPTH=0 keeps two buffers, so both arms are one binary.
        if std::env::var_os("RUNE_DEPTH").is_some_and(|v| v == "0") { w.native.depth = Depth::Off; return; }
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

    /// The window gained or lost focus.
    pub(super) fn depth_focus(&mut self, focused: bool) {
        let Some(w) = self.win.as_mut() else { return };
        w.native.focus = if focused { Focus::Focused } else { Focus::Since(Instant::now()) };
        #[cfg(feature = "frame-profile")]
        crate::frame::live_log::mark(if focused { "focus.in" } else { "focus.out" });
    }

    /// At the end of a turn: a free turn grows or gives back the third buffer, or draws into a
    /// buffer not yet drawn into.
    pub(super) fn depth_turn(&mut self) {
        let seen = self.shown_win().is_some_and(|w| !matches!(w.native.look, super::look::Look::Covered { .. }));
        let Some(w) = self.win.as_mut() else { return };
        if w.native.depth == Depth::Off { return; }
        let now = Instant::now();
        let keep = seen && w.native.focus.keeps(now);
        // Nothing to do: two and not kept, or three, warm and kept.
        if matches!(w.native.depth, Depth::Two { .. }) && !keep || w.native.depth == (Depth::Three { warm: 3 }) && keep { return; }
        if w.alpha.is_some() { return self.depth_step(DepthEvent::Failed); }
        let gpu_draws = matches!(w.native.mode, super::cpu_frame::SurfaceMode::Gpu | super::cpu_frame::SurfaceMode::Handed { .. });
        if !gpu_draws || !w.surface_ok || w.redraw.get() != super::redraw::Redraw::Idle || super::post_frame::input_waiting() { return; }
        // SAFETY: the object is the live swap chain's; a zero timeout only polls it.
        let waitable = w.surface.as_ref().and_then(|s| unsafe { s.as_hal::<wgpu::hal::api::Dx12>().and_then(|h| h.waitable_handle()) });
        if !w.native.slot.room(waitable.map(|h| h.0)) { return; }
        self.depth_step(DepthEvent::Free { now, keep });
    }

    /// Configures the window's swap chain with the buffers its depth says, at its size. If the
    /// third can't be made, two, for the window's life.
    fn configure_depth(&mut self) {
        let shown = self.life.shown();
        let (Some(w), Some((gpu, _))) = (self.win.as_mut(), self.gpu.as_ref()) else { return };
        let Some(surface) = w.surface.as_ref() else { return };
        let extra = w.native.depth.extra();
        // SAFETY: only sets a count the next configure reads.
        let set = |extra| unsafe { surface.as_hal::<wgpu::hal::api::Dx12>().map(|h| h.set_extra_buffers(extra)).is_some() };
        if !set(extra) { w.native.depth = Depth::Off; return; }
        #[cfg(feature = "frame-profile")]
        let start = crate::frame::live_log::now();
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
        #[cfg(feature = "frame-profile")]
        crate::frame::live_log::mark(&format!("depth\",\"extra\":{extra},\"refused\":{refused},\"start\":{start},\"ns\":{},\"x\":\"", crate::frame::live_log::now() - start));
        w.native.slot.reset();
        w.native.damage.reset();
        w.presented = None;
        // What DWM shows next comes from the new buffers: a frame fills them. A window out of
        // sight draws when it's shown.
        if shown { w.request_redraw(); }
    }
}
