use crate::input::host_events;
use crate::runtime::RuntimeCore;
use crate::window::surface::Win;

/// What a built frame hands the window besides its pixels: secure input, the IME, the cursor,
/// help tags and the clipboard.
pub(in crate::window) fn sync_platform_core(l: &mut Win, rt: &mut RuntimeCore, secure_input: &mut crate::input::secure_input::SecureInput,
    ime: &mut bool, clipboard: &crate::clipboard::Client) {
    let focused = l.window.has_focus();
    secure_input.set(rt.has_secure_focus() && focused);
    l.apply_cursor_core(rt, focused);
    #[cfg(target_os = "macos")]
    if rt.zoom() == 1.0 {
        l.native.help.sync(&l.window, rt.help_regions());
    } else {
        // Window points: the view's times its zoom.
        let regions: Vec<([f32; 4], String)> = rt.help_regions().iter().map(|(r, t)| (rt.to_window(*r), t.clone())).collect();
        l.native.help.sync(&l.window, &regions);
    }
    // The process has one menu theme: the active window's, so its window menu matches it.
    #[cfg(target_os = "windows")]
    if focused { crate::platform::win32::dark_mode::menus(rt.backdrop_dark().unwrap_or(rt.env.system_dark)); }
    clipboard.flush_core(rt);
    host_events::sync_ime(&l.window, rt, ime, focused);
}

impl Win {
    /// The runtime's cursor while the pointer is in the window, the arrow once it leaves.
    /// `Hidden` needs focus, so it can't outlive the window being active.
    pub(in crate::window) fn apply_cursor_core(&mut self, rt: &RuntimeCore, focused: bool) {
        let want = match rt.cursor() { crate::el::Cursor::Hidden if !focused => crate::el::Cursor::Default, c => c };
        self.cursor.apply(&self.window, want, rt.pointer().is_some());
    }
}
