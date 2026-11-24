//! Telling the platform's input method that Rune ended a composition itself (a blur, a click,
//! the window going inactive): the field already wrote what it showed, so the input method
//! drops its own copy rather than carry it into the next field. winit only switches the input
//! method off and on, which leaves the composition in Windows' input context and in AppKit's.
//! And what winit's input method handling leaves out: clauses and reconversion (`windows`,
//! `macos`).

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "macos")]
pub(crate) use macos::{sync, take_clauses, take_direct_commit, take_select};
#[cfg(target_os = "windows")]
pub(crate) use windows::{sync, take_clauses, take_select};

/// Text an input method inserted directly without marked text during the current key press,
/// when the platform drops it and emits the raw key instead (macOS).
#[cfg(not(target_os = "macos"))]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn take_direct_commit(_window: &winit::window::Window) -> Option<String> { None }

/// macOS device-dependent modifier masks from IOKit's `IOLLEvent.h` (`NX_DEVICELALTKEYMASK` and
/// `NX_DEVICERALTKEYMASK`), matching winit's `lalt_pressed` / `ralt_pressed`.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(crate) const NX_DEVICELALTKEYMASK: usize = 0x0000_0020;
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(crate) const NX_DEVICERALTKEYMASK: usize = 0x0000_0040;

/// Whether the active macOS `OptionAsAlt` mode treats the pressed Option key(s) in `flags_bits`
/// (from `NSEvent::modifierFlags().bits()`) as Alt rather than layout-composed text.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(crate) fn option_as_alt_matches(
    mode: crate::OptionAsAlt,
    alt_pressed: bool,
    flags_bits: usize,
) -> bool {
    if !alt_pressed {
        return false;
    }
    match mode {
        crate::OptionAsAlt::None => false,
        crate::OptionAsAlt::Left => flags_bits & NX_DEVICELALTKEYMASK != 0,
        crate::OptionAsAlt::Right => flags_bits & NX_DEVICERALTKEYMASK != 0,
        crate::OptionAsAlt::Both => true,
    }
}

/// Key-down context captured while macOS `WinitView::keyDown:` runs `interpretKeyEvents:`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(crate) struct KeyDown {
    pub(crate) view: usize,
    pub(crate) chars: Option<String>,
    pub(crate) chars_ignoring_mods: Option<String>,
    pub(crate) ctrl: bool,
    pub(crate) cmd: bool,
    pub(crate) alt: bool,
    pub(crate) option_as_alt: bool,
}

/// Decides whether an `insertText:replacementRange:` call without marked text during `key_down`
/// is text the input method substituted for the key (such as full-width CJK punctuation `，`
/// for `,`) and should be committed as `Ime::Commit` instead of falling through as the raw
/// `KeyboardInput`.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(crate) fn should_direct_commit(
    key_down: &KeyDown,
    has_marked_text: bool,
    inserted: &str,
) -> bool {
    if has_marked_text || inserted.is_empty() || inserted.chars().any(char::is_control) {
        return false;
    }
    if key_down.ctrl || key_down.cmd || (key_down.alt && key_down.option_as_alt) {
        return false;
    }
    if key_down.chars.as_deref() == Some(inserted) {
        return false;
    }
    if key_down.alt && key_down.chars_ignoring_mods.as_deref() == Some(inserted) {
        return false;
    }
    true
}

#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
struct ActiveKeyDown {
    key_down: KeyDown,
    commit: Option<String>,
}

/// Tracks active `keyDown:` calls on macOS so an `insertText:replacementRange:` without marked
/// text that substitutes the key's characters is handed to the matching `KeyboardInput` press
/// as `Ime::Commit` and never leaks across views or keypresses.
#[derive(Default)]
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(crate) struct DirectCommit {
    stack: Vec<ActiveKeyDown>,
}

#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
impl DirectCommit {
    pub(crate) const fn new() -> Self {
        Self { stack: Vec::new() }
    }

    pub(crate) fn begin(&mut self, key_down: KeyDown) {
        self.stack.push(ActiveKeyDown { key_down, commit: None });
    }

    pub(crate) fn record_insert(&mut self, view: usize, has_marked_text: bool, inserted: &str) {
        let Some(active) = self.stack.last_mut().filter(|a| a.key_down.view == view) else { return };
        if !should_direct_commit(&active.key_down, has_marked_text, inserted) { return; }
        match &mut active.commit {
            Some(existing) => existing.push_str(inserted),
            None => active.commit = Some(inserted.to_owned()),
        }
    }

    pub(crate) fn take(&mut self, view: usize) -> Option<String> {
        self.stack.last_mut().filter(|a| a.key_down.view == view)?.commit.take()
    }

    pub(crate) fn end(&mut self) {
        self.stack.pop();
    }
}

/// Publishes the focused field's text for the input method (reconversion). On Wayland and X11
/// winit gives the input method no text to read (text-input-v3's surrounding text, XIM's
/// string conversion), and the web's field holds its own.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub(crate) fn sync(_window: &winit::window::Window, _text: Option<crate::edit::ImeText>) {}

/// The clauses the platform gave composition `preedit`: none where winit doesn't pass them.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub(crate) fn take_clauses(_window: &winit::window::Window, _preedit: &str) -> Vec<crate::edit::Clause> { Vec::new() }

/// A reconversion's range of the field's text.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub(crate) fn take_select(_window: &winit::window::Window) -> Option<std::ops::Range<usize>> { None }

/// Drops the window's open composition, if any.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
pub(crate) fn cancel(window: &winit::window::Window) {
    use windows_sys::Win32::UI::Input::Ime::{CPS_CANCEL, ImmGetContext, ImmNotifyIME, ImmReleaseContext, NI_COMPOSITIONSTR};
    let Some(hwnd) = super::win32::hwnd(window) else { return };
    // SAFETY: `hwnd` is this thread's live window; a context it hands out is released here.
    unsafe {
        let himc = ImmGetContext(hwnd);
        if himc.is_null() { return; }
        ImmNotifyIME(himc, NI_COMPOSITIONSTR, CPS_CANCEL, 0);
        ImmReleaseContext(hwnd, himc);
    }
}

/// Drops the window's open composition, if any.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub(crate) fn cancel(window: &winit::window::Window) {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else { return };
    // SAFETY: `ns_view` is the window's live content view on the main thread; `inputContext`
    // returns it or nil, and `discardMarkedText` takes no arguments.
    unsafe {
        let view = h.ns_view.as_ptr().cast::<objc2::runtime::AnyObject>();
        // Nothing is marked in a view no input method is composing in. Asking the input context
        // to drop it anyway starts AppKit's text input services for a window of the app in
        // front: about 15,000 µs, in a new window's first frame.
        let responds: objc2::runtime::Bool = objc2::msg_send![view, respondsToSelector: objc2::sel!(hasMarkedText)];
        if responds.as_bool() {
            let marked: objc2::runtime::Bool = objc2::msg_send![view, hasMarkedText];
            if !marked.as_bool() { return; }
        }
        let context: *mut objc2::runtime::AnyObject = objc2::msg_send![view, inputContext];
        if !context.is_null() {
            let () = objc2::msg_send![context, discardMarkedText];
        }
    }
}

/// Wayland's and X11's input methods end the composition when winit turns them off, as the
/// web's text field does when it blurs.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub(crate) fn cancel(_window: &winit::window::Window) {}

/// Whether a Windows primary language ID (`PRIMARYLANGID(LOWORD(hkl))`) belongs to a
/// right-to-left keyboard layout, matching Chrome's `IsRTLPrimaryLangID`
/// (`ui/base/ime/win/input_method_win_base.cc`).
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub(crate) fn is_rtl_primary_lang_id(lang: u16) -> bool {
    const LANG_ARABIC: u16 = 0x01;
    const LANG_HEBREW: u16 = 0x0d;
    const LANG_URDU: u16 = 0x20;
    const LANG_PERSIAN: u16 = 0x29;
    const LANG_SYRIAC: u16 = 0x5a;
    const LANG_UIGHUR: u16 = 0x80;
    matches!(lang, LANG_ARABIC | LANG_HEBREW | LANG_URDU | LANG_PERSIAN | LANG_SYRIAC | LANG_UIGHUR)
}

/// Whether the system has a right-to-left keyboard layout installed (Windows only, matching
/// Chrome's `IsRTLKeyboardLayoutInstalled`). Always false on other platforms.
pub(crate) fn has_rtl_keyboard_layout() -> bool {
    #[cfg(target_os = "windows")]
    { windows::has_rtl_keyboard_layout() }
    #[cfg(not(target_os = "windows"))]
    { false }
}

#[cfg(test)]
#[path = "ime_tests.rs"]
mod tests;
