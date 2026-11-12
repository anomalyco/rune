//! Telling the platform's input method that Rune ended a composition itself (a blur, a click,
//! the window going inactive): the field already wrote what it showed, so the input method
//! drops its own copy rather than carry it into the next field. winit only switches the input
//! method off and on, which leaves the composition in Windows' input context and in AppKit's.

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
