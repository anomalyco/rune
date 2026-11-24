//! What handlers and updates ask of the window itself, carried out by `Host::shell`:
//! minimizing it and showing the title they set.

use super::Host;

fn minimize_ignored(what: &str) {
    if cfg!(debug_assertions) { eprintln!("rune: minimize ignored: {what} can't minimize"); }
}

/// Whether a shown window is allowed to carry out a minimize request.
///
/// Panels, launchers, and click-through overlays never minimize. Decorated windows minimize
/// whenever `WindowButtons::MINIMIZE` is enabled. Borderless windows minimize on Windows and
/// Linux when resizable and `WindowButtons::MINIMIZE` is enabled, so apps drawing their own
/// frame can minimize; macOS AppKit cannot miniaturize borderless `NSWindow`s, and
/// non-resizable borderless windows (HUDs, pop-ups, launchers) remain rejected.
pub(in crate::window) fn can_minimize(
    is_macos: bool,
    is_panel: bool,
    click_through: bool,
    borderless: bool,
    resizable: bool,
    has_minimize_button: bool,
) -> Result<(), &'static str> {
    if is_panel { return Err("a panel"); }
    if click_through { return Err("an overlay"); }
    if !has_minimize_button { return Err("a window without a minimize button"); }
    if borderless && (is_macos || !resizable) {
        return Err("a window without a minimize button");
    }
    Ok(())
}

impl Host {
    /// `Tx::minimize` and `AppHandle::minimize`: miniaturize on macOS, `set_minimized`
    /// elsewhere. A hidden window, a panel or launcher, a click-through overlay, a non-resizable
    /// borderless window, a macOS borderless window, and a window without a minimize button
    /// ignore it.
    pub(in crate::window) fn minimize(&mut self) {
        let Some(w) = self.win.as_mut().filter(|_| self.life.shown()) else { return minimize_ignored("a hidden window") };
        let is_panel = w.is_panel() || self.macos_window.launcher;
        let has_minimize_button = w.window.enabled_buttons().contains(winit::window::WindowButtons::MINIMIZE);
        if let Err(reason) = can_minimize(
            cfg!(target_os = "macos"),
            is_panel,
            self.options.click_through,
            self.options.borderless,
            self.options.resizable,
            has_minimize_button,
        ) {
            return minimize_ignored(reason);
        }
        #[cfg(target_os = "macos")]
        { w.native.prime.stop(); }
        w.window.set_minimized(true);
    }

    /// `Tx::set_title`: shows a title set since the window last showed one, hidden or covered
    /// as it may be. A window created later opens with the current title (`try_create`).
    pub(in crate::window) fn show_title(&mut self) {
        if let Some(title) = self.app.rt_mut().host.window_title.take_change() && let Some(w) = &self.win { w.window.set_title(title); }
    }

    /// `Tx::set_color_scheme`: applies a per-window color scheme set since the shell last
    /// checked, retaining it on `Host` across native window recreation.
    pub(in crate::window) fn apply_tx_color_scheme(&mut self) {
        if let Some(scheme) = self.app.rt_mut().host.window_color_scheme.take() {
            self.set_color_scheme(scheme);
        }
    }

    /// `Tx::set_represented_file` and `Tx::set_document_edited`: applies macOS document state
    /// changes set since the shell last checked. Retained in `HostState` across native window
    /// recreation; no-op off macOS.
    pub(in crate::window) fn apply_tx_macos_document(&mut self) {
        if self.win.is_none() { return; }
        let represented_change = self.app.rt_mut().host.represented_file.take_change().cloned();
        let edited_change = self.app.rt_mut().host.document_edited.take_change().copied();
        #[cfg(target_os = "macos")]
        if let Some(w) = &self.win {
            if let Some(path) = represented_change {
                let fallback_title = self.app.rt().host.window_title.current().unwrap_or(self.geometry.title);
                crate::platform::macos::document::set_represented_file(
                    &w.window,
                    path.as_deref(),
                    self.macos_window.title_from_file,
                    Some(fallback_title),
                );
            }
            if let Some(edited) = edited_change {
                crate::platform::macos::document::set_document_edited(&w.window, edited);
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (represented_change, edited_change);
    }
}

#[cfg(test)]
#[path = "shell_requests_tests.rs"]
mod tests;
