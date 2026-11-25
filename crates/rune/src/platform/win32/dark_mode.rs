//! Dark native menus. Windows draws pop-up, context, window and tray menus light unless the
//! process asks for the dark theme through two functions uxtheme exports by ordinal only:
//! `SetPreferredAppMode` (135) and `FlushMenuThemes` (136). Both have kept their ordinals since
//! Windows 10 1903 (build 18362); earlier builds export something else there, so they're left
//! alone and menus stay light.

#![allow(unsafe_code)]

use std::sync::{OnceLock, atomic::{AtomicU8, Ordering}};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
#[cfg(feature = "windows-system-material")]
use windows_sys::Win32::UI::WindowsAndMessaging::{WM_SYSCOLORCHANGE, WM_THEMECHANGED};
use windows_sys::Win32::UI::WindowsAndMessaging::{WM_NCDESTROY, WM_SETTINGCHANGE};

/// `PreferredAppMode`: follow the system's app mode, or force one.
const ALLOW_DARK: i32 = 1;
const FORCE_DARK: i32 = 2;
const FORCE_LIGHT: i32 = 3;

/// First Windows 10 build (1809) with uxtheme ordinal 104 (`RefreshImmersiveColorPolicyState`).
const FIRST_COLOR_POLICY_BUILD: u32 = 17763;
/// First Windows 10 build (1903) with uxtheme ordinals 135 (`SetPreferredAppMode`) & 136 (`FlushMenuThemes`).
const FIRST_BUILD: u32 = 18362;
const THEME_SUBCLASS: usize = 0x5255_4e54;

struct Uxtheme {
    set_preferred_app_mode: unsafe extern "system" fn(i32) -> i32,
    flush_menu_themes: unsafe extern "system" fn(),
}

/// The mode last asked for, so a frame that doesn't change it costs nothing.
static APPLIED: AtomicU8 = AtomicU8::new(0);

/// Menus follow the system's dark or light app mode until a window says what it draws.
pub(crate) fn follow_system() { apply(ALLOW_DARK); }

/// Menus from now on are dark or light, whatever the system's app mode.
pub(crate) fn menus(dark: bool) { apply(if dark { FORCE_DARK } else { FORCE_LIGHT }); }

fn apply(mode: i32) {
    let Some(uxtheme) = uxtheme() else { return };
    if APPLIED.swap(mode as u8, Ordering::Relaxed) == mode as u8 { return; }
    // SAFETY: both are uxtheme's own exports on a build that has them with these signatures.
    unsafe {
        (uxtheme.set_preferred_app_mode)(mode);
        (uxtheme.flush_menu_themes)();
    }
}

fn uxtheme() -> Option<&'static Uxtheme> {
    static UXTHEME: OnceLock<Option<Uxtheme>> = OnceLock::new();
    UXTHEME.get_or_init(|| {
        if windows_build()? < FIRST_BUILD { return None; }
        // SAFETY: loads a system DLL from System32 and looks up two exports by ordinal (a
        // pointer value below 0x10000). The module stays loaded for the process.
        unsafe {
            let module = LoadLibraryExW(wide("uxtheme.dll").as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
            if module.is_null() { return None; }
            let set = GetProcAddress(module, 135 as *const u8)?;
            let flush = GetProcAddress(module, 136 as *const u8)?;
            Some(Uxtheme {
                set_preferred_app_mode: std::mem::transmute::<unsafe extern "system" fn() -> isize, unsafe extern "system" fn(i32) -> i32>(set),
                flush_menu_themes: std::mem::transmute::<unsafe extern "system" fn() -> isize, unsafe extern "system" fn()>(flush),
            })
        }
    }).as_ref()
}

/// `OSVERSIONINFOW`, as `RtlGetVersion` fills it.
#[repr(C)]
struct VersionInfo {
    size: u32,
    major: u32,
    minor: u32,
    build: u32,
    platform: u32,
    service_pack: [u16; 128],
}

/// The Windows build number, cached for the process. `RtlGetVersion` reports it whatever the
/// app's manifest says, which `GetVersionEx` doesn't.
pub(crate) fn windows_build() -> Option<u32> {
    static BUILD: OnceLock<Option<u32>> = OnceLock::new();
    *BUILD.get_or_init(query_build)
}

fn query_build() -> Option<u32> {
    // SAFETY: ntdll is loaded in every process; RtlGetVersion fills the struct whose size it is
    // given and returns an NTSTATUS.
    unsafe {
        let ntdll = GetModuleHandleW(wide("ntdll.dll").as_ptr());
        if ntdll.is_null() { return None; }
        let get = GetProcAddress(ntdll, c"RtlGetVersion".as_ptr().cast())?;
        let get = std::mem::transmute::<unsafe extern "system" fn() -> isize, unsafe extern "system" fn(*mut VersionInfo) -> i32>(get);
        let mut info = VersionInfo { size: size_of::<VersionInfo>() as u32, major: 0, minor: 0, build: 0, platform: 0, service_pack: [0; 128] };
        (get(&mut info) == 0).then_some(info.build)
    }
}

/// Whether a `WM_SETTINGCHANGE` area string (`lParam`) should trigger an uxtheme color-policy
/// refresh: matches `"ImmersiveColorSet"` (case-insensitive, broadcast by Windows Settings on
/// Light/Dark and accent switches) or `None` (`lParam == 0`, generic settings broadcast).
pub(crate) fn should_refresh_color_policy_area(area: Option<&[u16]>) -> bool {
    const TARGET: &[u8] = b"ImmersiveColorSet";
    match area {
        None => true,
        Some(units) => {
            units.len() == TARGET.len()
                && units
                    .iter()
                    .zip(TARGET)
                    .all(|(&u, &b)| u <= 0x7f && (u as u8).eq_ignore_ascii_case(&b))
        }
    }
}

/// Pure decision helper behind [`refresh_color_policy`] for off-screen tests.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn refresh_color_policy_with(
    build: Option<u32>,
    area: Option<&[u16]>,
    refresh: impl FnOnce(),
) -> bool {
    if build.is_none_or(|b| b < FIRST_COLOR_POLICY_BUILD) || !should_refresh_color_policy_area(area) {
        return false;
    }
    refresh();
    true
}

/// Inspects a `WM_SETTINGCHANGE` message's `lParam` (NULL or NUL-terminated UTF-16 area name).
pub(crate) fn should_refresh_color_policy(lparam: LPARAM) -> bool {
    if lparam == 0 {
        return should_refresh_color_policy_area(None);
    }
    let ptr = lparam as *const u16;
    let mut len = 0usize;
    // Bounded scan: "ImmersiveColorSet" is 17 code units; longer names cannot match.
    // SAFETY: on `WM_SETTINGCHANGE`, a non-zero `lParam` points to a NUL-terminated UTF-16
    // string supplied by Windows for the duration of the message dispatch.
    while len < 64 && unsafe { *ptr.add(len) } != 0 {
        len += 1;
    }
    if len == 64 {
        return false;
    }
    // SAFETY: `ptr` has `len` initialized UTF-16 code units before the NUL terminator.
    let area = unsafe { std::slice::from_raw_parts(ptr, len) };
    should_refresh_color_policy_area(Some(area))
}

/// Refreshes uxtheme's process-cached immersive color policy (`RefreshImmersiveColorPolicyState`,
/// ordinal 104, Windows 10 1809+) so winit's `ShouldAppsUseDarkMode` (ordinal 132) sees live OS
/// Light/Dark switches instead of its launch-time cache. Also flushes menu themes when menus are
/// following the system (`ALLOW_DARK`).
pub(crate) fn refresh_color_policy() {
    if let Some(refresh) = refresh_fn() {
        // SAFETY: uxtheme ordinal 104 takes no arguments on build 17763 and later.
        unsafe { refresh(); }
    }
    if APPLIED.load(Ordering::Relaxed) == ALLOW_DARK as u8
        && let Some(uxtheme) = uxtheme()
    {
        // SAFETY: uxtheme ordinal 136 takes no arguments on build 18362 and later.
        unsafe { (uxtheme.flush_menu_themes)(); }
    }
}

fn refresh_fn() -> Option<unsafe extern "system" fn()> {
    static REFRESH: OnceLock<Option<unsafe extern "system" fn()>> = OnceLock::new();
    *REFRESH.get_or_init(|| {
        if windows_build()? < FIRST_COLOR_POLICY_BUILD { return None; }
        // SAFETY: loads uxtheme.dll from System32 only and resolves ordinal 104
        // (`RefreshImmersiveColorPolicyState`), present since Windows 10 1809 (build 17763).
        unsafe {
            let module = LoadLibraryExW(wide("uxtheme.dll").as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
            if module.is_null() { return None; }
            let f = GetProcAddress(module, 104 as *const u8)?;
            Some(std::mem::transmute::<unsafe extern "system" fn() -> isize, unsafe extern "system" fn()>(f))
        }
    })
}

/// Subclasses `window` so `WM_SETTINGCHANGE` refreshes uxtheme's cached immersive color policy
/// before winit's window procedure checks `ShouldAppsUseDarkMode` and emits
/// `WindowEvent::ThemeChanged`.
pub(crate) fn install_theme_refresh(window: &winit::window::Window) {
    let Some(hwnd) = super::hwnd(window) else { return };
    // SAFETY: the subclass holds no state and removes itself on `WM_NCDESTROY`.
    unsafe { SetWindowSubclass(hwnd, Some(theme_subclass), THEME_SUBCLASS, 0); }
}

unsafe extern "system" fn theme_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    _: usize,
) -> LRESULT {
    if message == WM_SETTINGCHANGE && should_refresh_color_policy(lparam) {
        refresh_color_policy();
    } else if message == WM_NCDESTROY {
        // SAFETY: removes this subclass from the window being destroyed.
        unsafe { RemoveWindowSubclass(hwnd, Some(theme_subclass), THEME_SUBCLASS); }
    }
    #[cfg(feature = "windows-system-material")]
    if matches!(message, WM_SETTINGCHANGE | WM_SYSCOLORCHANGE | WM_THEMECHANGED) {
        crate::system_material::windows::notify_wm_setting_change();
    }
    // SAFETY: forwards the message to the next procedure in this window's subclass chain (winit).
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }

#[cfg(test)]
#[path = "dark_mode_tests.rs"]
mod tests;
