use super::*;

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

#[test]
fn color_policy_refresh_matches_immersive_color_set_and_generic_broadcast() {
    // Generic WM_SETTINGCHANGE with lParam == 0 (NULL).
    assert!(should_refresh_color_policy_area(None));
    // Windows Settings broadcast ("ImmersiveColorSet"), any ASCII case.
    assert!(should_refresh_color_policy_area(Some(&utf16("ImmersiveColorSet"))));
    assert!(should_refresh_color_policy_area(Some(&utf16("immersivecolorset"))));
    assert!(should_refresh_color_policy_area(Some(&utf16("IMMERSIVECOLORSET"))));

    // Unrelated WM_SETTINGCHANGE areas are ignored.
    for other in ["", "Environment", "Policy", "intl", "ImmersiveColor", "ImmersiveColorSet2"] {
        assert!(
            !should_refresh_color_policy_area(Some(&utf16(other))),
            "unexpected refresh for WM_SETTINGCHANGE area {other:?}",
        );
    }
}

#[test]
fn setting_change_seam_refreshes_uxtheme_before_winit_reads_theme() {
    // Models uxtheme's process-cached ShouldAppsUseDarkMode: returns cached_dark until
    // RefreshImmersiveColorPolicyState copies registry_dark into cached_dark.
    struct FakeUxtheme {
        registry_dark: bool,
        cached_dark: bool,
        refresh_calls: usize,
    }

    let mut fake = FakeUxtheme { registry_dark: true, cached_dark: true, refresh_calls: 0 };

    // Registry flips to light (AppsUseLightTheme = 1).
    fake.registry_dark = false;
    assert!(fake.cached_dark, "without refresh uxtheme keeps returning stale cached dark");

    // Pre-1809 build (< 17763): never invokes ordinal 104.
    let area = utf16("ImmersiveColorSet");
    assert!(!refresh_color_policy_with(Some(17134), Some(&area), || {
        fake.refresh_calls += 1;
        fake.cached_dark = fake.registry_dark;
    }));
    assert_eq!(fake.refresh_calls, 0);
    assert!(fake.cached_dark);

    // Unrelated area on Win11: zero refresh calls.
    let env = utf16("Environment");
    assert!(!refresh_color_policy_with(Some(26300), Some(&env), || {
        fake.refresh_calls += 1;
        fake.cached_dark = fake.registry_dark;
    }));
    assert_eq!(fake.refresh_calls, 0);
    assert!(fake.cached_dark);

    // Real Settings broadcast ("ImmersiveColorSet") on Win11: refreshes cache BEFORE winit queries.
    assert!(refresh_color_policy_with(Some(26300), Some(&area), || {
        fake.refresh_calls += 1;
        fake.cached_dark = fake.registry_dark;
    }));
    assert_eq!(fake.refresh_calls, 1);
    assert!(!fake.cached_dark, "winit's ShouldAppsUseDarkMode now sees light theme");

    // Flip back to dark and send generic WM_SETTINGCHANGE (lParam == 0): also refreshes.
    fake.registry_dark = true;
    assert!(refresh_color_policy_with(Some(26300), None, || {
        fake.refresh_calls += 1;
        fake.cached_dark = fake.registry_dark;
    }));
    assert_eq!(fake.refresh_calls, 2);
    assert!(fake.cached_dark, "winit's ShouldAppsUseDarkMode now sees dark theme again");
}

struct FakeUxtheme {
    registry_dark: bool,
    cached_dark: bool,
    preferred_app_mode: i32,
    refresh_calls: usize,
    set_mode_calls: Vec<i32>,
    flush_calls: usize,
}

impl FakeUxtheme {
    fn new(registry_dark: bool) -> Self {
        Self {
            registry_dark,
            cached_dark: registry_dark,
            preferred_app_mode: 0,
            refresh_calls: 0,
            set_mode_calls: Vec::new(),
            flush_calls: 0,
        }
    }

    /// Models uxtheme ordinal 132 (`ShouldAppsUseDarkMode`): returns the forced process mode
    /// while `FORCE_DARK` / `FORCE_LIGHT` is active, even after `RefreshImmersiveColorPolicyState`.
    fn should_apps_use_dark_mode(&self) -> bool {
        match self.preferred_app_mode {
            FORCE_DARK => true,
            FORCE_LIGHT => false,
            _ => self.cached_dark,
        }
    }
}

impl UxthemeSeam for FakeUxtheme {
    fn set_preferred_app_mode(&mut self, mode: i32) {
        self.preferred_app_mode = mode;
        self.set_mode_calls.push(mode);
    }

    fn refresh_color_policy(&mut self) {
        self.refresh_calls += 1;
        self.cached_dark = self.registry_dark;
    }

    fn flush_menu_themes(&mut self) {
        self.flush_calls += 1;
    }
}

#[test]
fn menu_theme_for_window_keeps_allow_dark_when_system_window_matches_os() {
    use crate::ColorScheme;

    // ColorScheme::System with transparent or matching surface keeps ALLOW_DARK.
    assert_eq!(menu_theme_for_window(ColorScheme::System, None, true), MenuTheme::System);
    assert_eq!(menu_theme_for_window(ColorScheme::System, None, false), MenuTheme::System);
    assert_eq!(menu_theme_for_window(ColorScheme::System, Some(true), true), MenuTheme::System);
    assert_eq!(menu_theme_for_window(ColorScheme::System, Some(false), false), MenuTheme::System);

    // ColorScheme::System with an opposite-polarity custom canvas forces matching menus.
    assert_eq!(menu_theme_for_window(ColorScheme::System, Some(true), false), MenuTheme::Dark);
    assert_eq!(menu_theme_for_window(ColorScheme::System, Some(false), true), MenuTheme::Light);

    // Explicit ColorScheme overrides force the window's rendered theme.
    assert_eq!(menu_theme_for_window(ColorScheme::Dark, None, false), MenuTheme::Dark);
    assert_eq!(menu_theme_for_window(ColorScheme::Dark, Some(true), false), MenuTheme::Dark);
    assert_eq!(menu_theme_for_window(ColorScheme::Dark, Some(false), true), MenuTheme::Light);
    assert_eq!(menu_theme_for_window(ColorScheme::Light, None, true), MenuTheme::Light);
    assert_eq!(menu_theme_for_window(ColorScheme::Light, Some(false), true), MenuTheme::Light);
    assert_eq!(menu_theme_for_window(ColorScheme::Light, Some(true), false), MenuTheme::Dark);
}

#[test]
fn focused_system_window_continues_following_os_light_dark_switches() {
    use crate::ColorScheme;

    let mut ctrl = ModeController::new();
    let mut ux = FakeUxtheme::new(true);

    // Startup puts menus in ALLOW_DARK.
    ctrl.set_base(MenuTheme::System).run(&mut ux);
    assert_eq!(ux.preferred_app_mode, ALLOW_DARK);
    assert!(ux.should_apps_use_dark_mode());

    // Focusing a ColorScheme::System window whose canvas follows system_dark stays in ALLOW_DARK.
    let mode = menu_theme_for_window(ColorScheme::System, Some(true), true);
    ctrl.set_base(mode).run(&mut ux);
    assert_eq!(ux.preferred_app_mode, ALLOW_DARK);
    assert_eq!(ux.set_mode_calls, vec![ALLOW_DARK]);

    // Windows Settings switches to Light; WM_SETTINGCHANGE refreshes policy and flushes menus.
    ux.registry_dark = false;
    ctrl.enter_query(true).run(&mut ux);
    assert!(!ux.should_apps_use_dark_mode(), "winit must observe Light after focus");
    ctrl.exit_query().run(&mut ux);
    assert_eq!(ux.preferred_app_mode, ALLOW_DARK);
    assert_eq!(ux.refresh_calls, 1);
    assert_eq!(ux.flush_calls, 2);

    // Window focuses again in Light mode, then Windows Settings switches back to Dark.
    let mode = menu_theme_for_window(ColorScheme::System, Some(false), false);
    ctrl.set_base(mode).run(&mut ux);
    assert_eq!(ux.preferred_app_mode, ALLOW_DARK);

    ux.registry_dark = true;
    ctrl.enter_query(true).run(&mut ux);
    assert!(ux.should_apps_use_dark_mode(), "winit must observe Dark again after focus");
    ctrl.exit_query().run(&mut ux);
    assert_eq!(ux.preferred_app_mode, ALLOW_DARK);
}

#[test]
fn forced_menu_override_does_not_block_system_theme_queries_or_wm_settingchange() {
    let mut ctrl = ModeController::new();
    let mut ux = FakeUxtheme::new(true);

    // A focused ColorScheme::Dark window (or dark custom canvas) forces FORCE_DARK.
    ctrl.set_base(MenuTheme::Dark).run(&mut ux);
    assert_eq!(ux.preferred_app_mode, FORCE_DARK);

    // Windows Settings switches to Light: refreshing alone cannot bypass FORCE_DARK.
    ux.registry_dark = false;
    ux.refresh_color_policy();
    assert!(ux.should_apps_use_dark_mode(), "FORCE_DARK pins uxtheme ordinal 132 to true");

    // WM_SETTINGCHANGE query scope temporarily uses ALLOW_DARK so winit's query on sibling
    // ColorScheme::System windows and overlays sees Light, then restores FORCE_DARK.
    ctrl.enter_query(true).run(&mut ux);
    assert_eq!(ux.preferred_app_mode, ALLOW_DARK);
    assert!(!ux.should_apps_use_dark_mode(), "winit must observe Light inside WM_SETTINGCHANGE");
    ctrl.exit_query().run(&mut ux);
    assert_eq!(ux.preferred_app_mode, FORCE_DARK, "focused dark window keeps its forced menus");

    // Creating a new ColorScheme::System window/overlay or calling set_theme(None) also reads
    // the real OS setting without disturbing the forced menu theme afterwards.
    ctrl.enter_query(false).run(&mut ux);
    assert!(!ux.should_apps_use_dark_mode());
    ctrl.exit_query().run(&mut ux);
    assert_eq!(ux.preferred_app_mode, FORCE_DARK);
}

#[test]
fn scoped_popup_menus_restores_previous_theme_and_handles_nested_reentrant_queries() {
    let mut ctrl = ModeController::new();
    let mut ux = FakeUxtheme::new(true);

    ctrl.set_base(MenuTheme::System).run(&mut ux);
    assert_eq!(ux.preferred_app_mode, ALLOW_DARK);

    // Opening a forced-light popup menu switches to FORCE_LIGHT only while tracking.
    let (prev, actions) = ctrl.push_popup(false);
    actions.run(&mut ux);
    assert_eq!(ux.preferred_app_mode, FORCE_LIGHT);
    assert!(!ux.should_apps_use_dark_mode());

    // Nested WM_SETTINGCHANGE / theme query mid-popup still sees real OS Dark mode.
    ctrl.enter_query(true).run(&mut ux);
    ctrl.enter_query(false).run(&mut ux);
    assert_eq!(ux.preferred_app_mode, ALLOW_DARK);
    assert!(ux.should_apps_use_dark_mode());

    // Re-entrant base change during the query stays in ALLOW_DARK until the outer query exits,
    // then flushes once the query ends.
    ctrl.set_base(MenuTheme::Dark).run(&mut ux);
    assert_eq!(ux.preferred_app_mode, ALLOW_DARK);
    ctrl.exit_query().run(&mut ux);
    assert_eq!(ux.preferred_app_mode, ALLOW_DARK, "inner exit must keep outer query in ALLOW_DARK");
    let flushes_before_outer_exit = ux.flush_calls;
    ctrl.exit_query().run(&mut ux);
    assert_eq!(ux.preferred_app_mode, FORCE_LIGHT, "active popup override stays active until dismissed");
    assert_eq!(ux.flush_calls, flushes_before_outer_exit + 1);

    // Dismissing the popup reveals the updated base theme (MenuTheme::Dark -> FORCE_DARK).
    ctrl.pop_popup(prev).run(&mut ux);
    assert_eq!(ux.preferred_app_mode, FORCE_DARK);

    // Returning base to System restores ALLOW_DARK.
    ctrl.set_base(MenuTheme::System).run(&mut ux);
    assert_eq!(ux.preferred_app_mode, ALLOW_DARK);
}

#[test]
fn real_uxtheme_system_theme_query_bypasses_forced_menu_mode() {
    if windows_build().is_none_or(|b| b < FIRST_BUILD) {
        return;
    }
    let Some(should_apps_use_dark_mode) = ({
        // SAFETY: loads uxtheme.dll from System32 only and resolves ordinal 132
        // (`ShouldAppsUseDarkMode`), present since Windows 10 1809 (build 17763).
        unsafe {
            let module = LoadLibraryExW(
                wide("uxtheme.dll").as_ptr(),
                std::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            );
            if module.is_null() {
                None
            } else {
                GetProcAddress(module, 132 as *const u8).map(|f| {
                    std::mem::transmute::<
                        unsafe extern "system" fn() -> isize,
                        unsafe extern "system" fn() -> u8,
                    >(f)
                })
            }
        }
    }) else {
        return;
    };
    let should_apps_use_dark_mode = || unsafe { should_apps_use_dark_mode() != 0 };

    struct RestoreSystem;
    impl Drop for RestoreSystem {
        fn drop(&mut self) {
            follow_system();
        }
    }
    let _restore = RestoreSystem;

    // Establish baseline after refreshing uxtheme's immersive color policy (a bare console test
    // process has not initialized uxtheme's policy cache yet before the first refresh).
    let os_dark = with_system_theme_query(should_apps_use_dark_mode);

    // Forcing the opposite menu mode overrides uxtheme's ShouldAppsUseDarkMode.
    set_menu_theme(if os_dark { MenuTheme::Light } else { MenuTheme::Dark });
    let forced = should_apps_use_dark_mode();
    assert_eq!(forced, !os_dark, "forcing opposite menu mode should override ShouldAppsUseDarkMode");

    // with_system_theme_query temporarily restores ALLOW_DARK so winit sees the real OS mode,
    // then restores the forced menu mode on return.
    let queried = with_system_theme_query(should_apps_use_dark_mode);
    assert_eq!(queried, os_dark, "with_system_theme_query must reveal real OS dark mode");
    let after = should_apps_use_dark_mode();
    assert_eq!(after, !os_dark, "forced menu mode must be restored after query");

    // scoped_popup_menus restores ALLOW_DARK on drop.
    follow_system();
    {
        let _popup = scoped_popup_menus(!os_dark);
        assert_eq!(should_apps_use_dark_mode(), !os_dark);
        let during_popup = with_system_theme_query_and_flush(should_apps_use_dark_mode);
        assert_eq!(during_popup, os_dark);
    }
    assert_eq!(should_apps_use_dark_mode(), os_dark);
}

