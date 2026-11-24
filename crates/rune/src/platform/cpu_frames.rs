//! Linux: where a window's CPU-drawn frames (`render::cpu_boot`) go before any GPU exists, and
//! how they reach the screen: `wl_shm` buffers attached to winit's `wl_surface` on Wayland
//! (`wayland`), an image put to the window on X11 (`x11`). The window's Vulkan swap chain takes
//! the same surface or window over later (`window::linux::handover`).
//!
//! One side at a time sends a window's pixels to the display: these frames until the window goes
//! to the GPU, the driver's swap chain after. The switch is `Frames::retire`, made before the
//! window's Vulkan surface exists: it takes the frames and gives back a `Retired`, which keeps
//! their buffers alive (the display shows the last one until the GPU's first frame) and can send
//! nothing. That order is a rule of the display's on Wayland, not a preference: a compositor with
//! explicit synchronization (`wp_linux_drm_syncobj_manager_v1`: GNOME 47, KDE 6.1) ends a client
//! that commits a `wl_shm` buffer to a surface while a driver's synchronization object for it
//! exists (`unsupported_buffer`), and Mesa makes that object with the surface's first swap chain,
//! not with its first present (`wsi_common_wayland.c`: `wsi_wl_surface_init` in 24.1 to 25.2,
//! kept until the Vulkan surface is destroyed; `wsi_wl_surface_create_swapchain` in 26, kept
//! until the swap chain is).

#![allow(unsafe_code)]

use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use winit::window::Window;

#[cfg(test)]
pub(crate) mod fake;
mod wayland;
mod x11;

/// The buffer a frame is painted into: B, G, R, A bytes, `stride` from one row to the next,
/// the top row first. `kept`: it holds the frame painted last, so only what changed needs
/// painting; otherwise its pixels are unknown and every one is painted.
pub(crate) struct Canvas<'a> {
    pub(crate) bytes: &'a mut [u8],
    pub(crate) stride: usize,
    pub(crate) kept: bool,
}

/// What of a painted frame goes to the screen.
#[derive(Clone, Copy)]
pub(crate) enum Put<'a> {
    /// Only these rectangles changed, `[x0, y0, x1, y1)`; none: nothing goes.
    Rects(&'a [[u32; 4]]),
    /// The whole frame.
    All,
}

/// A window's CPU frames on their way to the screen.
pub(crate) enum Frames {
    Wayland(wayland::Frames),
    X11(x11::Frames),
    /// Frames that go nowhere and count what a display would have had.
    #[cfg(test)]
    Fake(fake::Frames),
}

/// A window's CPU frames after the window went to the GPU (`Frames::retire`): their buffers
/// live on, and nothing here can send the display a frame.
pub(crate) struct Retired(Frames);

impl Retired {
    /// The frame the window showed last, as `Frames::last`.
    pub(crate) fn last(&self) -> Option<(Vec<u8>, (u32, u32))> { self.0.last() }

    /// The frames back, for a window whose GPU surface couldn't be made and is gone again, where
    /// a surface that came and went is known to leave nothing behind on the window: X11, and a
    /// Wayland compositor without explicit synchronization. With it, whether the driver's
    /// synchronization object went with its surface is the driver's own business (Mesa's does;
    /// other drivers' can't be read), and one CPU frame committed beside it would end the app:
    /// these stay retired (`Err`).
    pub(crate) fn revive(self) -> Result<Frames, Retired> {
        if self.0.explicit_sync() { Err(self) } else { Ok(self.0) }
    }
}

impl Frames {
    /// For `window`, whose frames are `size` device pixels at first. `alpha`: the frame's alpha
    /// is composited, premultiplied (a Wayland client-side frame's corner arcs); otherwise it's
    /// ignored. None when the display has no way to show CPU frames that is known to be exact
    /// (no `wl_shm`; an X11 visual that isn't 24-bit true colour in B, G, R, X byte order): the
    /// window then opens on the GPU, as it did.
    pub(crate) fn new(window: &Window, size: (u32, u32), alpha: bool) -> Option<Frames> {
        match (window.display_handle().ok()?.as_raw(), window.window_handle().ok()?.as_raw()) {
            (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(handle)) => {
                wayland::Frames::new(display.display.as_ptr().cast(), handle.surface.as_ptr().cast(), size, alpha).map(Frames::Wayland)
            }
            (_, RawWindowHandle::Xlib(handle)) if !alpha => {
                x11::Frames::new(u32::try_from(handle.window).ok()?, u32::try_from(handle.visual_id).ok()?).map(Frames::X11)
            }
            (_, RawWindowHandle::Xcb(handle)) if !alpha => x11::Frames::new(handle.window.get(), handle.visual_id?.get()).map(Frames::X11),
            _ => None,
        }
    }

    /// The buffer for a frame of `size`. None when there's none to paint into now (a Wayland
    /// compositor still holds every buffer; no memory): the frame is tried again later.
    pub(crate) fn begin(&mut self, size: (u32, u32)) -> Option<Canvas<'_>> {
        match self {
            Frames::Wayland(frames) => frames.begin(size),
            Frames::X11(frames) => frames.begin(size),
            #[cfg(test)]
            Frames::Fake(frames) => frames.begin(size),
        }
    }

    /// Sends what `put` says of the frame `begin`'s buffer now holds. True when something went.
    pub(crate) fn present(&mut self, put: Put<'_>) -> bool {
        match self {
            Frames::Wayland(frames) => frames.present(put),
            Frames::X11(frames) => frames.present(put),
            #[cfg(test)]
            Frames::Fake(frames) => frames.present(put),
        }
    }

    /// Whether the display asked for the window's pixels again since the last call (X11: an
    /// Expose; the server keeps nothing of a window that was covered or not yet mapped).
    /// `wait`: every request the display has had so far is answered first (a round trip).
    pub(crate) fn exposed(&mut self, wait: bool) -> bool {
        match self {
            Frames::Wayland(_) => false,
            Frames::X11(frames) => frames.exposed(wait),
            #[cfg(test)]
            Frames::Fake(_) => false,
        }
    }

    /// The buffers' pixels no longer count as any frame's: the next `begin` isn't `kept`.
    pub(crate) fn forget(&mut self) {
        match self {
            Frames::Wayland(frames) => frames.forget(),
            Frames::X11(frames) => frames.forget(),
            #[cfg(test)]
            Frames::Fake(frames) => frames.forget(),
        }
    }

    /// The window goes to the GPU: from here these frames can send the display nothing (the
    /// module's rule). Called before the window's Vulkan surface is made. X11: returns once the
    /// server has every frame sent so far, so what the driver's connection draws lands after
    /// them. Wayland: one connection, nothing to wait for.
    pub(crate) fn retire(mut self) -> Retired {
        if let Frames::X11(frames) = &mut self { frames.exposed(true); }
        Retired(self)
    }

    /// Whether the display ends a client that sends it a CPU frame while a GPU driver's
    /// synchronization object for the window exists (`Retired::revive`).
    fn explicit_sync(&self) -> bool {
        match self {
            Frames::Wayland(frames) => frames.explicit_sync(),
            Frames::X11(_) => false,
            #[cfg(test)]
            Frames::Fake(frames) => frames.explicit_sync,
        }
    }

    /// Whether the compositor paces frames itself (a Wayland frame callback, through winit's
    /// `pre_present_notify`); otherwise the window does, by the clock.
    pub(crate) fn paced(&self) -> bool { matches!(self, Frames::Wayland(_)) }

    /// The frame the window shows, tightly packed B, G, R, A rows, for comparing with the GPU's.
    pub(crate) fn last(&self) -> Option<(Vec<u8>, (u32, u32))> {
        match self {
            Frames::Wayland(frames) => frames.last(),
            Frames::X11(frames) => frames.last(),
            #[cfg(test)]
            Frames::Fake(frames) => frames.last(),
        }
    }
}

/// The smallest rectangle around `rects`, cut to `size`; None when it's empty.
fn bounds(rects: &[[u32; 4]], size: (u32, u32)) -> Option<[u32; 4]> {
    let all = rects.iter().fold([u32::MAX, u32::MAX, 0, 0], |u, r| [u[0].min(r[0]), u[1].min(r[1]), u[2].max(r[2]), u[3].max(r[3])]);
    let cut = [all[0].min(size.0), all[1].min(size.1), all[2].min(size.0), all[3].min(size.1)];
    (cut[2] > cut[0] && cut[3] > cut[1]).then_some(cut)
}

#[cfg(test)]
#[path = "cpu_frames_tests.rs"]
mod tests;
