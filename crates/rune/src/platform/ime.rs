//! Telling the platform's input method that Rune ended a composition itself (a blur, a click,
//! the window going inactive): the field already wrote what it showed, so the input method
//! drops its own copy rather than carry it into the next field. winit only switches the input
//! method off and on, which leaves the composition in Windows' input context and in AppKit's.
//! And what winit's input method handling leaves out: clauses and reconversion (`windows`,
//! `macos`).

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub(crate) use windows::{sync, take_clauses, take_select};
#[cfg(target_os = "macos")]
pub(crate) use macos::{sync, take_clauses, take_select};

/// macOS: through the vendored winit's `NSTextInputClient` (`platform::macos::set_ime_text`
/// and the rest), which reads `setMarkedText:`'s clause attributes and answers `selectedRange`
/// and `attributedSubstringForProposedRange:` from the text published here.
#[cfg(target_os = "macos")]
mod macos {
    use std::cell::Cell;
    use std::ops::Range;

    use crate::edit::{Clause, ImeText};

    thread_local! {
        /// Where the published text starts in its field, in bytes.
        static START: Cell<usize> = const { Cell::new(0) };
    }

    pub(crate) fn sync(_window: &winit::window::Window, text: Option<ImeText>) {
        START.set(text.as_ref().map_or(0, |t| t.start));
        winit::platform::macos::set_ime_text(text.map(|t| (t.text.clone(), t.selection.clone())));
    }

    pub(crate) fn take_clauses(_window: &winit::window::Window, preedit: &str) -> Vec<Clause> {
        winit::platform::macos::take_ime_clauses(preedit).into_iter().map(|(range, target)| {
            let clause = Clause::new(range);
            if target { clause.target() } else { clause }
        }).collect()
    }

    pub(crate) fn take_select(_window: &winit::window::Window) -> Option<Range<usize>> {
        let start = START.get();
        winit::platform::macos::take_ime_replacement().map(|r| start + r.start..start + r.end)
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
pub(crate) fn cancel(window: &winit::window::Window) {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else { return };
    // SAFETY: `ns_view` is the window's live content view on the main thread; `inputContext`
    // returns it or nil, and `discardMarkedText` takes no arguments.
    unsafe {
        let view = h.ns_view.as_ptr().cast::<objc2::runtime::AnyObject>();
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
