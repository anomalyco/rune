//! Windows native touch capability and pointer-source classification for `Cx::touch_controls`
//! and `Cx::hover_none`.
//!
//! Combines two Win32 signals:
//! 1. **System tablet posture** (`SM_MAXIMUMTOUCHES`, `SM_DIGITIZER`, `SM_CONVERTIBLESLATEMODE`,
//!    `SM_MOUSEPRESENT`): a ready touch digitizer in slate/tablet mode or without a mouse starts
//!    in touch-control mode, and `WM_SETTINGCHANGE` (`ConvertibleSlateMode` / `UserInteractionMode`)
//!    refreshes it without polling on ordinary redraws.
//! 2. **Live Win32 pointer messages** (`WM_TOUCH`, `WM_POINTERDOWN`, `WM_POINTERUPDATE`,
//!    `WM_POINTERUP`, and `GetMessageExtraInfo()` `MI_WP_SIGNATURE` bit 7): distinguishes finger
//!    touch (`WM_TOUCH`, `PT_TOUCH`) from pen/stylus (`PT_PEN`, pen-promoted mouse messages) and
//!    physical mouse/touchpad input, because winit delivers both `PT_TOUCH` and `PT_PEN` as
//!    `WindowEvent::Touch`.

#![allow(unsafe_code)]

use std::cell::Cell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetMessageExtraInfo, GetSystemMetrics, NID_EXTERNAL_TOUCH, NID_INTEGRATED_TOUCH, NID_READY,
    POINTER_INPUT_TYPE, PT_MOUSE, PT_PEN, PT_TOUCH, PT_TOUCHPAD, SM_CONVERTIBLESLATEMODE,
    SM_DIGITIZER, SM_MAXIMUMTOUCHES, SM_MOUSEPRESENT, WM_NCDESTROY, WM_POINTERDOWN, WM_POINTERUP,
    WM_POINTERUPDATE, WM_SETTINGCHANGE, WM_TOUCH,
};

use crate::runtime::PrimaryPointer;

const SUBCLASS: usize = 0x5255_4e54; // "RUNT"

/// Bumped whenever a window receives `WM_SETTINGCHANGE` for `ConvertibleSlateMode` or
/// `UserInteractionMode`, so redraws only re-query `GetSystemMetrics` after a real posture change.
static TABLET_SETTING_GEN: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// Pointer source of the Win32 touch/pointer message currently dispatching on this thread.
    static CURRENT_POINTER: Cell<Option<MessagePointerKind>> = const { Cell::new(None) };
}

/// Snapshot of Win32 system metrics relevant to primary touch posture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SystemTouchMetrics {
    /// `GetSystemMetrics(SM_MAXIMUMTOUCHES)` (0 when no touch digitizer is installed).
    pub max_touches: i32,
    /// `GetSystemMetrics(SM_DIGITIZER) as u32`.
    pub digitizer: u32,
    /// `GetSystemMetrics(SM_CONVERTIBLESLATEMODE)` (0 = slate/tablet mode, non-zero = clamshell/desktop).
    pub slate_mode: i32,
    /// `GetSystemMetrics(SM_MOUSEPRESENT)` (0 = no mouse present, non-zero = mouse present).
    pub mouse_present: i32,
}

/// Whether a ready touch digitizer (integrated or external) is available.
pub(crate) const fn has_touch_digitizer(m: SystemTouchMetrics) -> bool {
    const TOUCH_MASK: u32 = NID_INTEGRATED_TOUCH | NID_EXTERNAL_TOUCH;
    m.max_touches > 0 && (m.digitizer & NID_READY) != 0 && (m.digitizer & TOUCH_MASK) != 0
}

/// Whether Windows is in a touch-first posture (ready touch digitizer AND either convertible
/// slate/tablet mode or no mouse installed).
pub(crate) const fn is_tablet_posture(m: SystemTouchMetrics) -> bool {
    has_touch_digitizer(m) && (m.slate_mode == 0 || m.mouse_present == 0)
}

/// Queries live Win32 touch/posture system metrics.
pub(crate) fn read_system_metrics() -> SystemTouchMetrics {
    // SAFETY: `GetSystemMetrics` is a pure Win32 query taking documented metric constants.
    unsafe {
        SystemTouchMetrics {
            max_touches: GetSystemMetrics(SM_MAXIMUMTOUCHES),
            digitizer: GetSystemMetrics(SM_DIGITIZER) as u32,
            slate_mode: GetSystemMetrics(SM_CONVERTIBLESLATEMODE),
            mouse_present: GetSystemMetrics(SM_MOUSEPRESENT),
        }
    }
}

/// Whether the live Windows host is currently in a touch-first tablet posture.
pub(crate) fn system_tablet_mode() -> bool {
    is_tablet_posture(read_system_metrics())
}

/// Current `WM_SETTINGCHANGE` generation for tablet/slate-mode transitions.
#[inline]
pub(crate) fn setting_generation() -> u64 {
    TABLET_SETTING_GEN.load(Ordering::Acquire)
}

/// Source of a legacy Windows mouse message as encoded in `GetMessageExtraInfo()`.
///
/// Microsoft documents `MI_WP_SIGNATURE = 0xFF515700` (mask `0xFFFFFF00`) for pen/touch-promoted
/// mouse messages, with bit 7 (`0x80`) set for finger touch and clear for pen/stylus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PromotedSource {
    /// Genuine mouse or precision touchpad message (no `MI_WP_SIGNATURE`).
    Mouse,
    /// Synthesized from a pen/stylus (`0xFF51_5700..=0xFF51_577F`).
    Pen,
    /// Synthesized from finger touch (`0xFF51_5780..=0xFF51_57FF`).
    Touch,
}

/// Classifies a raw `GetMessageExtraInfo()` value into mouse, pen-promoted, or touch-promoted.
pub(crate) const fn classify_extra_info(info: usize) -> PromotedSource {
    const MI_WP_SIGNATURE: u32 = 0xFF51_5700;
    const SIGNATURE_MASK: u32 = 0xFFFF_FF00;
    const TOUCH_BIT: u32 = 0x0000_0080;
    let low = info as u32;
    if low & SIGNATURE_MASK != MI_WP_SIGNATURE {
        PromotedSource::Mouse
    } else if low & TOUCH_BIT != 0 {
        PromotedSource::Touch
    } else {
        PromotedSource::Pen
    }
}

/// Source of the legacy mouse message currently being handled on this thread.
pub(crate) fn current_promoted_source() -> PromotedSource {
    // SAFETY: reads the calling thread's current message extra info; no pointers involved.
    classify_extra_info(unsafe { GetMessageExtraInfo() } as usize)
}

/// Pointer source of a Win32 touch/pointer message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MessagePointerKind {
    Touch,
    Pen,
    Hover,
}

impl MessagePointerKind {
    pub(crate) const fn primary_pointer(self) -> PrimaryPointer {
        match self {
            Self::Touch => PrimaryPointer::Touch,
            Self::Pen | Self::Hover => PrimaryPointer::Hover,
        }
    }
}

/// Classifies a Win32 message into its pointer source, calling `query_pointer_type` with the
/// `WM_POINTER*` pointer ID (`LOWORD(wparam)`) when needed.
pub(crate) fn classify_win32_message(
    message: u32,
    wparam: usize,
    query_pointer_type: impl FnOnce(u32) -> Option<POINTER_INPUT_TYPE>,
) -> Option<MessagePointerKind> {
    match message {
        WM_TOUCH => Some(MessagePointerKind::Touch),
        WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP => {
            let pointer_id = (wparam & 0xFFFF) as u32;
            match query_pointer_type(pointer_id) {
                Some(PT_TOUCH) => Some(MessagePointerKind::Touch),
                Some(PT_PEN) => Some(MessagePointerKind::Pen),
                Some(PT_MOUSE) | Some(PT_TOUCHPAD) => Some(MessagePointerKind::Hover),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Whether a `WM_SETTINGCHANGE` area string (`lParam`) announces a tablet/slate posture change.
pub(crate) fn is_tablet_setting_change_area(area: Option<&str>) -> bool {
    matches!(
        area,
        Some(s) if s.eq_ignore_ascii_case("ConvertibleSlateMode")
            || s.eq_ignore_ascii_case("UserInteractionMode")
    )
}

fn is_tablet_setting_change(lparam: LPARAM) -> bool {
    if lparam == 0 {
        return false;
    }
    let ptr = lparam as *const u16;
    // SAFETY: on `WM_SETTINGCHANGE`, a non-zero `lParam` points to a NUL-terminated UTF-16
    // setting-area string owned by the caller for the duration of the message.
    let len = unsafe { (0..128usize).take_while(|&i| *ptr.add(i) != 0).count() };
    // SAFETY: `ptr` is valid for `len` `u16` elements (all non-NUL, bounded above).
    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
    let area = String::from_utf16_lossy(slice);
    is_tablet_setting_change_area(Some(&area))
}

fn query_pointer_type(pointer_id: u32) -> Option<POINTER_INPUT_TYPE> {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    type GetPointerTypeFn = unsafe extern "system" fn(u32, *mut POINTER_INPUT_TYPE) -> i32;
    static FN: OnceLock<Option<GetPointerTypeFn>> = OnceLock::new();
    let f = (*FN.get_or_init(|| {
        const USER32: [u16; 11] = [
            b'u' as u16, b's' as u16, b'e' as u16, b'r' as u16, b'3' as u16,
            b'2' as u16, b'.' as u16, b'd' as u16, b'l' as u16, b'l' as u16, 0,
        ];
        // SAFETY: `user32.dll` is already loaded in every Win32 GUI process; `GetPointerType`
        // has signature `BOOL GetPointerType(UINT32 pointerId, POINTER_INPUT_TYPE *pointerType)`.
        unsafe {
            let m = GetModuleHandleW(USER32.as_ptr());
            if m.is_null() { return None; }
            GetProcAddress(m, c"GetPointerType".as_ptr().cast())
                .map(|p| std::mem::transmute::<_, GetPointerTypeFn>(p))
        }
    }))?;
    let mut kind: POINTER_INPUT_TYPE = 0;
    // SAFETY: `kind` is a valid, aligned local out-parameter.
    (unsafe { f(pointer_id, &mut kind) } != 0).then_some(kind)
}

/// Resolves the [`PrimaryPointer`] for a `WindowEvent::Touch` currently being dispatched on Windows.
///
/// Inside `WM_TOUCH` or `WM_POINTER*(PT_TOUCH)` this returns `PrimaryPointer::Touch`; inside
/// `WM_POINTER*(PT_PEN | PT_MOUSE | PT_TOUCHPAD)` it returns `PrimaryPointer::Hover` so a stylus
/// or touchpad never activates coarse touch controls. Outside a subclassed Win32 message (for
/// example in synthetic tests) it defaults to `PrimaryPointer::Touch`.
pub(crate) fn current_primary_pointer() -> PrimaryPointer {
    CURRENT_POINTER.get().map_or(PrimaryPointer::Touch, MessagePointerKind::primary_pointer)
}

#[cfg(test)]
pub(crate) fn with_current_pointer_for_test<R>(kind: Option<MessagePointerKind>, f: impl FnOnce() -> R) -> R {
    let prev = CURRENT_POINTER.replace(kind);
    let out = f();
    CURRENT_POINTER.set(prev);
    out
}

/// Subclasses `window` to track the Win32 pointer source across `WM_TOUCH` / `WM_POINTER*`
/// and watch `WM_SETTINGCHANGE` for tablet/slate posture transitions.
pub(crate) fn install(window: &winit::window::Window) {
    let Some(hwnd) = super::hwnd(window) else { return };
    install_hwnd(hwnd);
}

/// Subclasses `hwnd` directly (for overlays that already hold an `HWND`).
pub(crate) fn install_hwnd(hwnd: HWND) {
    // SAFETY: the subclass holds no heap state and removes itself on `WM_NCDESTROY`.
    unsafe { SetWindowSubclass(hwnd, Some(subclass), SUBCLASS, 0); }
}

unsafe extern "system" fn subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    _: usize,
) -> LRESULT {
    if let Some(kind) = classify_win32_message(message, wparam, query_pointer_type) {
        let outer = CURRENT_POINTER.replace(Some(kind));
        // SAFETY: forwards to the next procedure in this window's subclass chain.
        let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
        CURRENT_POINTER.set(outer);
        return result;
    }
    if message == WM_SETTINGCHANGE && is_tablet_setting_change(lparam) {
        TABLET_SETTING_GEN.fetch_add(1, Ordering::Release);
        // SAFETY: `hwnd` is a live window on this thread; passing null invalidates the client area
        // so winit delivers a redraw to reconcile the updated tablet posture.
        unsafe { InvalidateRect(hwnd, std::ptr::null(), 0); }
    } else if message == WM_NCDESTROY {
        // SAFETY: removes this subclass from the window being destroyed.
        unsafe { RemoveWindowSubclass(hwnd, Some(subclass), SUBCLASS); }
    }
    // SAFETY: forwards to the next procedure in this window's subclass chain.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

#[cfg(test)]
#[path = "touch_input_tests.rs"]
mod tests;
