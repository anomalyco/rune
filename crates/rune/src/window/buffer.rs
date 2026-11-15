//! Windows: a window resized inside its swap chain's buffers. `ResizeBuffers` waits until the GPU
//! has finished every frame in flight and then makes new buffers (19,000-23,000 µs a step on
//! an Iris Xe), so a window being resized keeps buffers as big as its display, which the window
//! clips: each step costs nothing but its frame, drawn at the window's size into the renderer's
//! kept frame and copied into the top-left, the rest the window's background (`Renderer::copy_kept`).
//! Where the window grows before its next frame, the system shows that background, not black.
//! The first step still resizes the buffers exactly and draws, as DWM then shows the new size
//! with its frame, and only after that frame do they grow (`Growing`): new buffers of another
//! size would show nothing where the window grew until a frame filled them. Once the resizing
//! stops (`SETTLE`, outside the system's size loop), the buffers go back to the window's size,
//! when no frames are in flight to wait for. (`IDXGISwapChain2::SetSourceSize` would show only
//! the window's part, but a composition swap chain stretches it to the buffers.)

use std::time::Duration;

use crate::time::Instant;

/// How long after its last resize a window's buffers go back to its size.
pub(super) const SETTLE: Duration = Duration::from_millis(1000);

/// The tested single-output envelope, in physical pixels. Larger and multi-display setups use
/// exact buffers. Compare native output identity too: a hotplug can replace the sole display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Display { id: usize, size: (u32, u32), scale: u64 }

impl Display {
    pub(super) fn measured(window: &winit::window::Window, (w, h): (u32, u32)) -> Option<Self> {
        use winit::platform::windows::MonitorHandleExtWindows;
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CMONITORS};
        // SAFETY: reads only the number of active monitors on the virtual desktop.
        if unsafe { GetSystemMetrics(SM_CMONITORS) } != 1 || window.scale_factor() != 1.5 { return None; }
        let m = window.current_monitor()?;
        let size = m.size();
        if size.width > 2560 || size.height > 1440 || w > size.width || h > size.height { return None; }
        // The largest grow step and the maximised client that were checked for unfilled pixels.
        if !((w <= 2180 && h <= 1420) || (w <= 2488 && h <= 1406)) { return None; }
        Some(Self { id: m.hmonitor() as usize, size: (size.width, size.height), scale: window.scale_factor().to_bits() })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Buffer {
    /// The swap chain's buffers are the window's size.
    #[default]
    Exact,
    /// The window's size, after a resize's first step: they grow once its frame is in.
    Growing(Display),
    /// The exact-size frame was actually presented. A draw deferred for a slot, occlusion or
    /// surface recovery must not expose an empty, larger swap chain instead of that frame.
    Filled(Display),
    /// Buffers of `size`, the window in their top-left; it last resized at `at`.
    Larger { display: Display, at: Instant },
    /// No more headroom for this native window. A scale/output change, unknown topology,
    /// unmeasured size or refused allocation returns to ordinary exact-size configuration.
    Fallback,
}

impl Buffer {
    pub(super) fn display(self) -> Option<Display> {
        match self { Self::Growing(d) | Self::Filled(d) | Self::Larger { display: d, .. } => Some(d), _ => None }
    }

    /// When the buffers go back to the window's size.
    pub(super) fn settle_at(self) -> Option<Instant> {
        match self {
            Buffer::Exact | Buffer::Growing(_) | Buffer::Filled(_) | Buffer::Fallback => None,
            Buffer::Larger { at, .. } => Some(at + SETTLE),
        }
    }

    pub(super) fn larger(self) -> bool { matches!(self, Buffer::Larger { .. }) }

    pub(super) fn accepts_drawable(self, size: (u32, u32)) -> bool {
        !matches!(self, Buffer::Larger { display, .. } if display.size != size)
    }
}

impl super::Win {
    /// A moved or rescaled window can keep the same client size. Leave the larger surface now,
    /// not on some later `Resized`. Never publish dirty rectangles from the old buffers.
    pub(super) fn fallback_buffer(&mut self, gpu: Option<&super::Gpu>) {
        let was = std::mem::replace(&mut self.native.buffer, Buffer::Fallback);
        if !was.larger() { return; }
        if let (Some(surface), Some(gpu)) = (&self.surface, gpu) {
            self.surface_ok = super::configure(surface, gpu, &self.config).is_ok();
        }
        self.native.slot.reset();
        self.native.damage.reset();
        self.request_redraw();
    }
}

impl<S: 'static> super::Host<S> {
    /// After a resize's first step drew at the window's new size: the buffers grow to the
    /// display's size, and a frame fills them before the next step, so none shows them empty.
    pub(super) fn grow_buffer(&mut self) -> bool {
        let Some(l) = self.win.as_mut() else { return false };
        let Buffer::Filled(display) = l.native.buffer else { return false };
        l.native.buffer = Buffer::Exact;
        let (Some(surface), Some((gpu, _))) = (l.surface.as_ref(), self.gpu.as_ref()) else { return false };
        let (width, height) = (l.config.width, l.config.height);
        if Display::measured(&l.window, (width, height)) != Some(display) {
            l.native.buffer = Buffer::Fallback;
            return false;
        }
        let mut config = l.config.clone();
        (config.width, config.height) = display.size;
        let oom = gpu.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let configured = super::configure(surface, gpu, &config);
        if pollster::block_on(oom.pop()).is_some() || configured.is_err() {
            // The window's own size still holds: configure it back.
            l.native.buffer = Buffer::Fallback;
            l.surface_ok = super::configure(surface, gpu, &l.config).is_ok();
            l.native.slot.reset();
            l.native.damage.reset();
            l.request_redraw();
            return false;
        }
        l.native.buffer = Buffer::Larger { display, at: Instant::now() };
        l.native.slot.reset();
        l.native.damage.reset();
        true
    }

    /// The window stopped resizing (`SETTLE`, and the user has let go of its frame): its
    /// buffers go back to its size, and a frame fills them.
    pub(super) fn settle_buffer(&mut self) {
        let Some(l) = self.win.as_mut() else { return };
        if l.native.buffer.settle_at().is_none_or(|at| Instant::now() < at) || crate::platform::win32::in_size_move(&l.window) { return; }
        l.native.buffer = Buffer::Exact;
        let (Some(surface), Some((gpu, renderer))) = (l.surface.as_ref(), self.gpu.as_ref()) else { return };
        if let Err(error) = super::configure(surface, gpu, &l.config) {
            eprintln!("rune: a window's surface refused its size after a resize: {error}");
            l.surface_ok = false;
        }
        l.native.slot.reset();
        l.native.damage.reset();
        renderer.drop_kept();
        // The display-sized buffers and the kept frame are gone once the device's frees run, which
        // a window at rest wouldn't ask for; the configure waited for the GPU, so they run now.
        let _ = gpu.device.poll(wgpu::PollType::Poll);
        l.request_redraw();
    }
}
