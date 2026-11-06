//! What handlers and updates ask of the window itself, carried out by `Host::shell`:
//! minimizing it and showing the title they set.
use super::*;

fn minimize_ignored(what: &str) {
    if cfg!(debug_assertions) { eprintln!("rune: minimize ignored: {what} can't minimize"); }
}

pub(super) fn toggle_maximize(window: &Window) {
    window.set_maximized(!window.is_maximized());
}

pub(super) fn titlebar_double_click(window: &Window) {
    #[cfg(target_os = "macos")]
    mac::titlebar_double_click(window);
    #[cfg(not(target_os = "macos"))]
    toggle_maximize(window);
}

impl<S: 'static> Host<S> {
    /// Windows with app-drawn chrome, excluding the click-through overlay and panel presets.
    pub(super) fn borderless_controls(&self) -> bool {
        cfg!(any(target_os = "windows", target_os = "linux")) && self.options.borderless
            && !self.options.click_through && !self.macos_window.nonactivating_panel
    }

    /// `Tx::minimize` and `AppHandle::minimize`: miniaturize on macOS, `set_minimized`
    /// elsewhere. A hidden window, a panel or launcher, and a window that can't minimize
    /// ignore it.
    pub(super) fn minimize(&mut self) {
        let borderless = self.options.borderless && !self.borderless_controls();
        #[cfg(target_os = "macos")]
        let launcher = self.launcher();
        let Some(w) = self.win.as_mut().filter(|w| w.visible) else { return minimize_ignored("a hidden window") };
        #[cfg(target_os = "macos")]
        if w.native.panel.is_some() || launcher { return minimize_ignored("a panel"); }
        if borderless || !w.window.enabled_buttons().contains(winit::window::WindowButtons::MINIMIZE) {
            return minimize_ignored("a window without a minimize button");
        }
        #[cfg(target_os = "macos")]
        { w.native.prime = 0; }
        w.window.set_minimized(true);
    }

    /// `Tx::set_title`: shows a title set since the window last showed one, hidden or covered
    /// as it may be. A window created later opens with the current title (`try_create`).
    pub(super) fn show_title(&mut self) {
        if let Some(title) = self.rt.window_title.take_change() && let Some(w) = &self.win { w.window.set_title(title); }
    }
}
