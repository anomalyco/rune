//! Dark native menus. Windows draws pop-up, context, window and tray menus light unless the
//! process asks for the dark theme through two functions uxtheme exports by ordinal only:
//! `SetPreferredAppMode` (135) and `FlushMenuThemes` (136). Both have kept their ordinals since
//! Windows 10 1903 (build 18362); earlier builds export something else there, so they're left
//! alone and menus stay light.

#![allow(unsafe_code)]

use std::sync::{Mutex, OnceLock};

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

/// Desired process-wide Win32 native menu theme (`PreferredAppMode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuTheme {
    /// Follow Windows' app mode (`ALLOW_DARK`).
    System,
    /// Force light native menus (`FORCE_LIGHT`).
    Light,
    /// Force dark native menus (`FORCE_DARK`).
    Dark,
}

impl MenuTheme {
    const fn preferred_app_mode(self) -> i32 {
        match self {
            MenuTheme::System => ALLOW_DARK,
            MenuTheme::Light => FORCE_LIGHT,
            MenuTheme::Dark => FORCE_DARK,
        }
    }
}

/// Chooses the Win32 menu theme for a focused window.
///
/// Stays in `MenuTheme::System` (`ALLOW_DARK`) whenever the window follows the OS scheme and
/// its rendered surface matches `system_dark` (or is transparent), so uxtheme's
/// `ShouldAppsUseDarkMode` continues to reflect live OS Settings switches and tray/unfocused
/// menus follow the OS too. Forces `Dark` or `Light` only when the window diverges from the OS.
pub(crate) const fn menu_theme_for_window(
    effective_scheme: crate::ColorScheme,
    backdrop_dark: Option<bool>,
    system_dark: bool,
) -> MenuTheme {
    match effective_scheme {
        crate::ColorScheme::System => match backdrop_dark {
            Some(dark) if dark != system_dark => {
                if dark { MenuTheme::Dark } else { MenuTheme::Light }
            }
            _ => MenuTheme::System,
        },
        crate::ColorScheme::Dark => match backdrop_dark {
            Some(false) => MenuTheme::Light,
            _ => MenuTheme::Dark,
        },
        crate::ColorScheme::Light => match backdrop_dark {
            Some(true) => MenuTheme::Dark,
            _ => MenuTheme::Light,
        },
    }
}

struct Uxtheme {
    set_preferred_app_mode: unsafe extern "system" fn(i32) -> i32,
    flush_menu_themes: unsafe extern "system" fn(),
}

/// Seam over uxtheme's ordinal calls so the mode state machine is unit-testable off-screen.
trait UxthemeSeam {
    fn set_preferred_app_mode(&mut self, mode: i32);
    fn refresh_color_policy(&mut self);
    fn flush_menu_themes(&mut self);
}

struct RealUxthemeSeam;

impl UxthemeSeam for RealUxthemeSeam {
    fn set_preferred_app_mode(&mut self, mode: i32) {
        if let Some(ux) = uxtheme() {
            // SAFETY: uxtheme ordinal 135 takes `PreferredAppMode` on build 18362 and later.
            unsafe { (ux.set_preferred_app_mode)(mode); }
        }
    }

    fn refresh_color_policy(&mut self) {
        if let Some(refresh) = refresh_fn() {
            // SAFETY: uxtheme ordinal 104 takes no arguments on build 17763 and later.
            unsafe { refresh(); }
        }
    }

    fn flush_menu_themes(&mut self) {
        if let Some(ux) = uxtheme() {
            // SAFETY: uxtheme ordinal 136 takes no arguments on build 18362 and later.
            unsafe { (ux.flush_menu_themes)(); }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ModeActions {
    set_mode: Option<i32>,
    refresh: bool,
    flush: bool,
}

impl ModeActions {
    const NONE: Self = Self { set_mode: None, refresh: false, flush: false };

    fn run(self, seam: &mut impl UxthemeSeam) {
        if let Some(mode) = self.set_mode {
            seam.set_preferred_app_mode(mode);
        }
        if self.refresh {
            seam.refresh_color_policy();
        }
        if self.flush {
            seam.flush_menu_themes();
        }
    }
}

/// Three-layer state for process-wide `PreferredAppMode`:
/// 1. Active OS theme queries (`query_depth > 0`) always hold `ALLOW_DARK` so winit's
///    `ShouldAppsUseDarkMode` (ordinal 132) sees the real OS app mode rather than a forced menu
///    override.
/// 2. An active native pop-up menu (`popup`) forces dark/light only while `TrackPopupMenuEx`
///    tracks.
/// 3. The focused window's `base` theme (`MenuTheme::System` by default).
struct ModeController {
    base: MenuTheme,
    popup: Option<bool>,
    query_depth: u32,
    applied: i32,
    pending_flush: bool,
}

impl ModeController {
    const fn new() -> Self {
        Self {
            base: MenuTheme::System,
            popup: None,
            query_depth: 0,
            applied: 0,
            pending_flush: false,
        }
    }

    const fn resolve(&self) -> i32 {
        if self.query_depth > 0 {
            ALLOW_DARK
        } else if let Some(dark) = self.popup {
            if dark { FORCE_DARK } else { FORCE_LIGHT }
        } else {
            self.base.preferred_app_mode()
        }
    }

    fn sync_after_change(&mut self) -> ModeActions {
        let target = self.resolve();
        if self.applied != target {
            self.applied = target;
            ModeActions { set_mode: Some(target), refresh: false, flush: true }
        } else {
            if self.query_depth > 0 {
                self.pending_flush = true;
            }
            ModeActions::NONE
        }
    }

    fn set_base(&mut self, theme: MenuTheme) -> ModeActions {
        self.base = theme;
        self.sync_after_change()
    }

    fn push_popup(&mut self, dark: bool) -> (Option<bool>, ModeActions) {
        let prev = self.popup.replace(dark);
        (prev, self.sync_after_change())
    }

    fn pop_popup(&mut self, prev: Option<bool>) -> ModeActions {
        self.popup = prev;
        self.sync_after_change()
    }

    fn enter_query(&mut self, flush_if_system: bool) -> ModeActions {
        self.query_depth = self.query_depth.saturating_add(1);
        let target = self.resolve();
        let set_mode = (self.applied != target).then(|| {
            self.applied = target;
            target
        });
        let flush = flush_if_system && self.base == MenuTheme::System && self.popup.is_none();
        ModeActions { set_mode, refresh: true, flush }
    }

    fn exit_query(&mut self) -> ModeActions {
        self.query_depth = self.query_depth.saturating_sub(1);
        let target = self.resolve();
        let set_mode = (self.applied != target).then(|| {
            self.applied = target;
            target
        });
        let flush = self.query_depth == 0 && std::mem::take(&mut self.pending_flush);
        ModeActions { set_mode, refresh: false, flush }
    }
}

static MODE: Mutex<ModeController> = Mutex::new(ModeController::new());

/// Menus follow the system's dark or light app mode until a window diverges from it.
pub(crate) fn follow_system() { set_menu_theme(MenuTheme::System); }

/// Updates the process-wide menu theme for the focused window.
pub(crate) fn set_menu_theme(theme: MenuTheme) {
    let actions = MODE.lock().unwrap_or_else(|e| e.into_inner()).set_base(theme);
    actions.run(&mut RealUxthemeSeam);
}

/// RAII guard forcing dark or light native menus only while a Win32 modal pop-up menu tracks.
pub(crate) struct PopupMenuThemeGuard {
    prev: Option<bool>,
}

impl Drop for PopupMenuThemeGuard {
    fn drop(&mut self) {
        let actions = MODE.lock().unwrap_or_else(|e| e.into_inner()).pop_popup(self.prev);
        actions.run(&mut RealUxthemeSeam);
    }
}

/// Temporarily forces dark or light native menus for the lifetime of the returned guard.
pub(crate) fn scoped_popup_menus(dark: bool) -> PopupMenuThemeGuard {
    let (prev, actions) = MODE.lock().unwrap_or_else(|e| e.into_inner()).push_popup(dark);
    actions.run(&mut RealUxthemeSeam);
    PopupMenuThemeGuard { prev }
}

struct QueryScopeGuard;

impl Drop for QueryScopeGuard {
    fn drop(&mut self) {
        let actions = MODE.lock().unwrap_or_else(|e| e.into_inner()).exit_query();
        actions.run(&mut RealUxthemeSeam);
    }
}

fn with_system_theme_query_inner<R>(flush_if_system: bool, f: impl FnOnce() -> R) -> R {
    let actions = MODE.lock().unwrap_or_else(|e| e.into_inner()).enter_query(flush_if_system);
    actions.run(&mut RealUxthemeSeam);
    let _guard = QueryScopeGuard;
    f()
}

/// Runs `f` with uxtheme temporarily in `ALLOW_DARK` and its immersive color policy refreshed so
/// winit's `ShouldAppsUseDarkMode` (ordinal 132) reads the real OS setting instead of a forced
/// menu override, then restores any previously forced menu mode.
pub(crate) fn with_system_theme_query<R>(f: impl FnOnce() -> R) -> R {
    with_system_theme_query_inner(false, f)
}

/// Like [`with_system_theme_query`], and flushes cached menu themes when menus follow the system
/// (for `WM_SETTINGCHANGE` broadcasts from Windows Settings).
pub(crate) fn with_system_theme_query_and_flush<R>(f: impl FnOnce() -> R) -> R {
    with_system_theme_query_inner(true, f)
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
    let refresh_theme = message == WM_SETTINGCHANGE && should_refresh_color_policy(lparam);
    if message == WM_NCDESTROY {
        // SAFETY: removes this subclass from the window being destroyed.
        unsafe { RemoveWindowSubclass(hwnd, Some(theme_subclass), THEME_SUBCLASS); }
    }
    #[cfg(feature = "windows-system-material")]
    if matches!(message, WM_SETTINGCHANGE | WM_SYSCOLORCHANGE | WM_THEMECHANGED) {
        crate::system_material::windows::notify_wm_setting_change();
    }
    // SAFETY: forwards the message to the next procedure in this window's subclass chain (winit).
    if refresh_theme {
        with_system_theme_query_and_flush(|| unsafe { DefSubclassProc(hwnd, message, wparam, lparam) })
    } else {
        unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
    }
}

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }

#[cfg(test)]
#[path = "dark_mode_tests.rs"]
mod tests;
