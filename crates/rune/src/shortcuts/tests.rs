use super::{LinuxEnv, ShortcutError, open_linux_backend};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Choice {
    Portal,
    X11,
}

struct Case {
    name: &'static str,
    env: LinuxEnv<'static>,
    portal: Result<Choice, ShortcutError>,
    x11: Result<Choice, ShortcutError>,
    expect: Result<Choice, ShortcutError>,
    tried_portal: bool,
    tried_x11: bool,
}

#[test]
fn linux_backend_selection_uses_portal_under_xwayland_and_keeps_x11_fallback() {
    let portal_err = || Err(ShortcutError::Backend("portal refused".into()));
    let x11_err = || Err(ShortcutError::Backend("x11 refused".into()));
    let cases = [
        Case {
            name: "native Wayland without DISPLAY uses portal",
            env: LinuxEnv { wayland: true, session_type: Some("wayland"), wayland_display: Some("wayland-0"), display: None, dbus: true },
            portal: Ok(Choice::Portal),
            x11: Ok(Choice::X11),
            expect: Ok(Choice::Portal),
            tried_portal: true,
            tried_x11: false,
        },
        Case {
            name: "native Wayland with XWayland DISPLAY still never falls back to X11 on portal failure",
            env: LinuxEnv { wayland: true, session_type: Some("wayland"), wayland_display: Some("wayland-0"), display: Some(":0"), dbus: true },
            portal: portal_err(),
            x11: Ok(Choice::X11),
            expect: portal_err(),
            tried_portal: true,
            tried_x11: false,
        },
        Case {
            name: "native Wayland without dbus is unsupported and skips X11",
            env: LinuxEnv { wayland: true, session_type: Some("wayland"), wayland_display: Some("wayland-0"), display: Some(":0"), dbus: false },
            portal: Ok(Choice::Portal),
            x11: Ok(Choice::X11),
            expect: Err(ShortcutError::Unsupported),
            tried_portal: false,
            tried_x11: false,
        },
        Case {
            name: "native Wayland nested inside an X11 session still uses portal",
            env: LinuxEnv { wayland: true, session_type: Some("x11"), wayland_display: Some("wayland-0"), display: Some(":0"), dbus: true },
            portal: Ok(Choice::Portal),
            x11: Ok(Choice::X11),
            expect: Ok(Choice::Portal),
            tried_portal: true,
            tried_x11: false,
        },
        Case {
            name: "XWayland via unset WAYLAND_DISPLAY in a Wayland session uses portal",
            env: LinuxEnv { wayland: false, session_type: Some("wayland"), wayland_display: None, display: Some(":0"), dbus: true },
            portal: Ok(Choice::Portal),
            x11: Ok(Choice::X11),
            expect: Ok(Choice::Portal),
            tried_portal: true,
            tried_x11: false,
        },
        Case {
            name: "XWayland via empty WAYLAND_DISPLAY in a Wayland session uses portal",
            env: LinuxEnv { wayland: false, session_type: Some("wayland"), wayland_display: Some(""), display: Some(":0"), dbus: true },
            portal: Ok(Choice::Portal),
            x11: Ok(Choice::X11),
            expect: Ok(Choice::Portal),
            tried_portal: true,
            tried_x11: false,
        },
        Case {
            name: "XWayland via WINIT_UNIX_BACKEND=x11 in a Wayland session uses portal",
            env: LinuxEnv { wayland: false, session_type: Some("wayland"), wayland_display: Some("wayland-0"), display: Some(":0"), dbus: true },
            portal: Ok(Choice::Portal),
            x11: Ok(Choice::X11),
            expect: Ok(Choice::Portal),
            tried_portal: true,
            tried_x11: false,
        },
        Case {
            name: "XWayland with mixed-case XDG_SESSION_TYPE uses portal",
            env: LinuxEnv { wayland: false, session_type: Some("Wayland"), wayland_display: None, display: Some(":0"), dbus: true },
            portal: Ok(Choice::Portal),
            x11: Ok(Choice::X11),
            expect: Ok(Choice::Portal),
            tried_portal: true,
            tried_x11: false,
        },
        Case {
            name: "XWayland in a tty-started Wayland compositor (WAYLAND_DISPLAY set, no XDG_SESSION_TYPE) uses portal",
            env: LinuxEnv { wayland: false, session_type: None, wayland_display: Some("wayland-0"), display: Some(":0"), dbus: true },
            portal: Ok(Choice::Portal),
            x11: Ok(Choice::X11),
            expect: Ok(Choice::Portal),
            tried_portal: true,
            tried_x11: false,
        },
        Case {
            name: "XWayland falls back to X11 when portal is absent",
            env: LinuxEnv { wayland: false, session_type: Some("wayland"), wayland_display: None, display: Some(":0"), dbus: true },
            portal: Err(ShortcutError::Unsupported),
            x11: Ok(Choice::X11),
            expect: Ok(Choice::X11),
            tried_portal: true,
            tried_x11: true,
        },
        Case {
            name: "XWayland falls back to X11 when portal refuses",
            env: LinuxEnv { wayland: false, session_type: Some("wayland"), wayland_display: Some("wayland-0"), display: Some(":0"), dbus: true },
            portal: portal_err(),
            x11: Ok(Choice::X11),
            expect: Ok(Choice::X11),
            tried_portal: true,
            tried_x11: true,
        },
        Case {
            name: "XWayland returns X11 error when both portal and X11 fail",
            env: LinuxEnv { wayland: false, session_type: Some("wayland"), wayland_display: None, display: Some(":0"), dbus: true },
            portal: Err(ShortcutError::Unsupported),
            x11: x11_err(),
            expect: x11_err(),
            tried_portal: true,
            tried_x11: true,
        },
        Case {
            name: "XWayland without dbus feature keeps X11 behaviour",
            env: LinuxEnv { wayland: false, session_type: Some("wayland"), wayland_display: None, display: Some(":0"), dbus: false },
            portal: Ok(Choice::Portal),
            x11: Ok(Choice::X11),
            expect: Ok(Choice::X11),
            tried_portal: false,
            tried_x11: true,
        },
        Case {
            name: "XWayland without DISPLAY keeps portal error when portal fails",
            env: LinuxEnv { wayland: false, session_type: Some("wayland"), wayland_display: None, display: None, dbus: true },
            portal: portal_err(),
            x11: Ok(Choice::X11),
            expect: portal_err(),
            tried_portal: true,
            tried_x11: false,
        },
        Case {
            name: "real X11 session opens X11 directly without touching the portal",
            env: LinuxEnv { wayland: false, session_type: Some("x11"), wayland_display: None, display: Some(":0"), dbus: true },
            portal: Ok(Choice::Portal),
            x11: Ok(Choice::X11),
            expect: Ok(Choice::X11),
            tried_portal: false,
            tried_x11: true,
        },
        Case {
            name: "explicit X11 session ignores a stale WAYLAND_DISPLAY",
            env: LinuxEnv { wayland: false, session_type: Some("x11"), wayland_display: Some("wayland-0"), display: Some(":0"), dbus: true },
            portal: Ok(Choice::Portal),
            x11: Ok(Choice::X11),
            expect: Ok(Choice::X11),
            tried_portal: false,
            tried_x11: true,
        },
        Case {
            name: "X11 session without XDG_SESSION_TYPE (startx) opens X11 directly",
            env: LinuxEnv { wayland: false, session_type: None, wayland_display: None, display: Some(":0"), dbus: true },
            portal: Ok(Choice::Portal),
            x11: Ok(Choice::X11),
            expect: Ok(Choice::X11),
            tried_portal: false,
            tried_x11: true,
        },
        Case {
            name: "X11 session with empty WAYLAND_DISPLAY opens X11 directly",
            env: LinuxEnv { wayland: false, session_type: None, wayland_display: Some(""), display: Some(":0"), dbus: true },
            portal: Ok(Choice::Portal),
            x11: Ok(Choice::X11),
            expect: Ok(Choice::X11),
            tried_portal: false,
            tried_x11: true,
        },
        Case {
            name: "no Wayland session and no DISPLAY is unsupported without calling X11",
            env: LinuxEnv { wayland: false, session_type: Some("x11"), wayland_display: None, display: None, dbus: true },
            portal: Ok(Choice::Portal),
            x11: Ok(Choice::X11),
            expect: Err(ShortcutError::Unsupported),
            tried_portal: false,
            tried_x11: false,
        },
        Case {
            name: "empty DISPLAY with no Wayland session is unsupported",
            env: LinuxEnv { wayland: false, session_type: None, wayland_display: None, display: Some(""), dbus: true },
            portal: Ok(Choice::Portal),
            x11: Ok(Choice::X11),
            expect: Err(ShortcutError::Unsupported),
            tried_portal: false,
            tried_x11: false,
        },
    ];

    for case in cases {
        let mut tried_portal = false;
        let mut tried_x11 = false;
        let got = open_linux_backend(
            case.env,
            || { tried_portal = true; case.portal },
            || { tried_x11 = true; case.x11 },
        );
        assert_eq!(got, case.expect, "{}: backend result", case.name);
        assert_eq!(tried_portal, case.tried_portal, "{}: tried_portal", case.name);
        assert_eq!(tried_x11, case.tried_x11, "{}: tried_x11", case.name);
    }
}
