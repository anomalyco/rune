//! An overlay's XWayland backing, for a Wayland compositor without layer-shell.

use super::*;

impl<T: 'static> Overlay<T> {
    /// The compositor has no layer-shell (GNOME): an overlay that takes no input shows on
    /// XWayland when there is one.
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) fn xwayland(&mut self) -> Option<Live> {
        if self.options.input.resolved(self.interactive) != OverlayInput::None || self.options.keys == KeyPolicy::OnDemand { return None; }
        let surface = crate::platform::x11::xwayland::XwaylandSurface::new(self.name, self.at, &self.options)?;
        let scale = surface.scale();
        self.shared.native.lock().unwrap().1 = crate::OverlayCapabilities {
            capture_exclusion: OverlaySupport::Unsupported, global_pointer: OverlaySupport::Unsupported, nonactivating_keys: OverlaySupport::Unsupported,
        };
        self.attach(Backing::Xwayland(surface), scale)
    }

    #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
    pub(super) fn xwayland(&mut self) -> Option<Live> {
        None
    }
}
