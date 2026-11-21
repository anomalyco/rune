//! A Wayland client-side frame's rounded corners: whether this window has any, at what radius,
//! and the pass that clears the content outside them (`render::csd_corners`).
//!
//! Under `sctk-adwaita` a decorated window's frame rounds the window's four corners while it
//! floats, and squares them when the window is maximized or tiled. The frame's own subsurfaces
//! carry the headerbar above the content and the shadow and one-point border around it, so the
//! content only has to leave the bottom two corners: outside the arc the frame's border covers,
//! the surface must composite nothing rather than its background.

use super::Win;
use crate::window::host::Host;

impl Host {
    /// Whether a client-side frame draws this window's decorations, whatever state the frame is in
    /// (a maximized, tiled or fullscreen window keeps its frame), asked of winit after the window's
    /// first configure: a compositor that draws the decorations itself (server side, the default on
    /// KDE, sway and Hyprland) never has one, and its windows keep exactly the surface they had.
    ///
    /// A frame's window keeps its surface's pixels' alpha for the corner arcs the content clears
    /// (`window/host/create.rs`), and is still opaque to Rune: its clear colour, its root fill and its
    /// device are an opaque window's, and only those corner pixels are transparent (`winit` keeps
    /// the rest of the window an opaque region).
    #[cfg_attr(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))), allow(unused_variables))]
    pub(in crate::window) fn client_frame(&self, window: &winit::window::Window) -> bool {
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if self.wayland {
            return winit::platform::wayland::WindowExtWayland::has_client_side_frame(window);
        }
        false
    }

    /// Whether this window is decorated at all on Wayland, and so may have a client-side frame:
    /// what the surface is asked for, without asking winit which decorations the compositor
    /// settled on (`client_frame`).
    pub(in crate::window) fn rounds_corners(&self) -> bool {
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        return self.wayland && !self.options.borderless;
        #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
        false
    }

    /// The radius, in device pixels, the window's client-side frame leaves the content's bottom
    /// corners at. Zero when it leaves them square (the window is maximized or tiled), when no
    /// such frame draws it, and when its surface can't composite translucent pixels: the content
    /// then paints as it always did, square to its edges.
    #[cfg_attr(
        not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))),
        allow(unused_variables)
    )]
    pub(in crate::window) fn corner_radius(&self, window: &Win) -> f32 {
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if self.rounds_corners() && window.config.alpha_mode != wgpu::CompositeAlphaMode::Opaque {
            let radius = winit::platform::wayland::WindowExtWayland::content_corner_radius(&*window.window);
            return radius * self.app.rt().env.scale;
        }
        0.
    }
}

/// Prepares `pass` to clear `size` at `radius` device pixels, making it (or another for a surface
/// that changed its format) when it must.
pub(in crate::window) fn prepare(
    pass: &mut Option<crate::render::csd_corners::CsdCorners>,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: (u32, u32),
    format: wgpu::TextureFormat,
    radius: f32,
) {
    let stale = pass.as_ref().is_some_and(|pass| pass.format() != format);
    if stale {
        *pass = None;
    }
    let pass = pass.get_or_insert_with(|| crate::render::csd_corners::CsdCorners::new(device, format));
    pass.prepare(queue, size, radius);
}

/// Clears outside a window's rounded bottom corners, after everything else that drew into
/// `target`.
pub(in crate::window) fn draw(
    pass: Option<&crate::render::csd_corners::CsdCorners>,
    encoder: &mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    radius: f32,
) {
    if radius <= 0. {
        return;
    }
    if let Some(pass) = pass {
        pass.draw(encoder, target);
    }
}
