//! An overlay's last backing on a Wayland compositor with neither layer-shell nor an XWayland it
//! may use (GNOME without XWayland, or an overlay that takes keys or a pointer its host kept off
//! XWayland): an undecorated, transparent xdg toplevel. The protocol gives a toplevel no position,
//! stacking or way to keep off the dock, so it reports `placement`, `always_on_top` and (under
//! `OverlayInput::Regions`) `click_through` as `Unsupported`. It carries the app's id, so the dock
//! groups it with the app rather than listing it alone. Wayland can't hide a toplevel: hidden
//! means destroyed, as for a layer surface.

use super::*;

impl OverlayCore {
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) fn toplevel(&mut self, app: &mut dyn OverlayApp, event_loop: &ActiveEventLoop) -> Option<Live> {
        let mut attrs = Window::default_attributes()
            .with_title(self.name)
            .with_inner_size(LogicalSize::new(self.at.size.0 as f64, self.at.size.1 as f64))
            .with_decorations(false)
            .with_resizable(false)
            .with_transparent(true)
            .with_active(false)
            .with_window_level(WindowLevel::AlwaysOnTop);
        if let Some(id) = crate::tooling::app_id() {
            attrs = winit::platform::wayland::WindowAttributesExtWayland::with_name(attrs, id, id);
        }
        let window = match event_loop.create_window(attrs) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                self.fail(app, format!("could not create its window: {error}"));
                return None;
            }
        };
        self.dark = crate::platform::preferences::window_dark(&window, None);
        app.rt_mut().set_system_dark(self.dark);
        if self.cursor.is_none() { self.cursor = Some(crate::cursor::os::OsCursor::new()); }
        let input = self.options.input.resolved(self.interactive);
        window.set_cursor_hittest(input != OverlayInput::None).ok();
        self.shared.native.lock().unwrap().1 = toplevel_capabilities(input);
        let scale = crate::window::backing_scale(&window);
        self.attach(app, Backing::Window(window), scale)
    }

    #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
    pub(super) fn toplevel(&mut self, _app: &mut dyn OverlayApp, _event_loop: &ActiveEventLoop) -> Option<Live> {
        None
    }
}

/// Whether `window` is a Wayland toplevel, which is only ever an overlay's fallback backing.
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(super) fn is_toplevel(window: &Window) -> bool {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    matches!(window.window_handle().map(|h| h.as_raw()), Ok(RawWindowHandle::Wayland(_)))
}

/// What a toplevel fallback with `input` can do: no capture exclusion, global pointer,
/// placement or stacking, keys only as the compositor focuses it, and clicks passed through only
/// when it takes none (`set_cursor_hittest` empties or clears the whole input region).
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
pub(super) fn toplevel_capabilities(input: OverlayInput) -> crate::OverlayCapabilities {
    let unsupported = OverlaySupport::Unsupported;
    crate::OverlayCapabilities {
        placement: unsupported,
        always_on_top: unsupported,
        click_through: if input == OverlayInput::Regions { unsupported } else { OverlaySupport::Supported },
        ..crate::OverlayCapabilities::placed(unsupported, unsupported, unsupported)
    }
}

#[cfg(test)]
#[path = "toplevel_tests.rs"]
mod tests;
