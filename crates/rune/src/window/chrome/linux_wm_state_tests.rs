use std::cell::Cell;
use std::rc::Rc;

use super::*;
use crate::{Cx, El, Env, Runtime, el, memo, row};

const ALL_WAYLAND_CAPS: u16 = WAYLAND_CAP_WINDOW_MENU
    | WAYLAND_CAP_MAXIMIZE
    | WAYLAND_CAP_FULLSCREEN
    | WAYLAND_CAP_MINIMIZE;

#[test]
fn pre_configure_returns_all_unknown() {
    let state = decode_wayland_wm_state(None);
    assert_eq!(state, WindowManagerState::default());
    assert_eq!(state.decoration_mode, DecorationMode::Unknown);
    assert_eq!(state.tiled_edges, None);
    assert_eq!(state.actions, WindowActions::default());
}

#[test]
fn protocol_version_gating_keeps_older_compositors_unknown() {
    // v1 compositor: decoration_mode and client-controlled close are known; tiled edges and
    // compositor-gated capabilities stay Unknown (None) even when sctk defaults capabilities to all().
    let v1 = decode_wayland_wm_state(Some(gate_wayland_configure(
        false,
        [true, false, true, true],
        ALL_WAYLAND_CAPS,
        1,
    )));
    assert_eq!(v1.decoration_mode, DecorationMode::Client);
    assert_eq!(v1.tiled_edges, None);
    assert_eq!(v1.actions, WindowActions::default().with_close(Some(true)));

    // v2..=v4 compositor: tiled edges known, compositor wm_capabilities still Unknown (None) so
    // sctk's default all() never leaks as confirmed capabilities.
    for version in 2..=4 {
        let state = decode_wayland_wm_state(Some(gate_wayland_configure(
            true,
            [true, false, true, true],
            ALL_WAYLAND_CAPS,
            version,
        )));
        assert_eq!(state.decoration_mode, DecorationMode::Server);
        assert_eq!(state.tiled_edges, Some(TiledEdges::new(true, false, true, true)));
        assert_eq!(
            state.actions,
            WindowActions::default().with_close(Some(true)),
            "v{version} must not report compositor wm_capabilities",
        );
    }

    // v5+ compositor: all five actions are known (close follows window closeable policy).
    let v5 = decode_wayland_wm_state(Some(gate_wayland_configure(
        false,
        [false, false, false, false],
        ALL_WAYLAND_CAPS,
        5,
    )));
    assert_eq!(v5.decoration_mode, DecorationMode::Client);
    assert_eq!(v5.tiled_edges, Some(TiledEdges::NONE));
    assert_eq!(
        v5.actions,
        WindowActions::default()
            .with_window_menu(Some(true))
            .with_maximize(Some(true))
            .with_minimize(Some(true))
            .with_fullscreen(Some(true))
            .with_close(Some(true)),
    );
}

#[test]
fn non_resizable_wayland_window_reports_maximize_false_while_keeping_fullscreen_and_close() {
    // Live GNOME Shell 50.1 (Mutter) sends all four wm_capabilities unconditionally, even for a
    // non-resizable window whose Super+Up maximize is refused while fullscreen is still allowed.
    let mut cfg = gate_wayland_configure(false, [false; 4], ALL_WAYLAND_CAPS, 5);
    cfg.resizable = false;
    let state = decode_wayland_wm_state(Some(cfg));
    assert_eq!(state.decoration_mode, DecorationMode::Client);
    assert_eq!(state.actions.maximize, Some(false));
    assert_eq!(state.actions.fullscreen, Some(true));
    assert_eq!(state.actions.minimize, Some(true));
    assert_eq!(state.actions.window_menu, Some(true));
    assert_eq!(state.actions.close, Some(true));

    // Even below v5, a non-resizable window knows maximize is disallowed while other compositor
    // capabilities remain unknown.
    let mut cfg_v4 = gate_wayland_configure(false, [false; 4], ALL_WAYLAND_CAPS, 4);
    cfg_v4.resizable = false;
    let state_v4 = decode_wayland_wm_state(Some(cfg_v4));
    assert_eq!(state_v4.actions.maximize, Some(false));
    assert_eq!(state_v4.actions.fullscreen, None);
    assert_eq!(state_v4.actions.window_menu, None);
    assert_eq!(state_v4.actions.minimize, None);
    assert_eq!(state_v4.actions.close, Some(true));

    // Non-closeable window reflects close: Some(false).
    let mut no_close = gate_wayland_configure(false, [false; 4], ALL_WAYLAND_CAPS, 5);
    no_close.closeable = false;
    assert_eq!(decode_wayland_wm_state(Some(no_close)).actions.close, Some(false));
}

#[test]
fn gnome_ubuntu_wayland_live_fixtures_pass_through_protocol_states_honestly() {
    // Live GNOME 50.1 maximized window sends all four TILED_* states.
    let maximized = decode_wayland_wm_state(Some(gate_wayland_configure(
        false,
        [true, true, true, true],
        ALL_WAYLAND_CAPS,
        5,
    )));
    assert_eq!(maximized.tiled_edges, Some(TiledEdges::ALL));

    // Ubuntu 26.04 Tiling Assistant half-tiles via vertical maximize, so Mutter only sends
    // TILED_TOP | TILED_BOTTOM on Wayland; Rune reports the compositor's exact states with no guess.
    let ubuntu_vert_half = decode_wayland_wm_state(Some(gate_wayland_configure(
        false,
        [true, false, true, false],
        ALL_WAYLAND_CAPS,
        5,
    )));
    assert_eq!(ubuntu_vert_half.tiled_edges, Some(TiledEdges::new(true, false, true, false)));

    // Stock Mutter left-half tile sends TILED_TOP | TILED_BOTTOM | TILED_LEFT.
    let stock_left = decode_wayland_wm_state(Some(gate_wayland_configure(
        false,
        [true, false, true, true],
        ALL_WAYLAND_CAPS,
        5,
    )));
    assert_eq!(stock_left.tiled_edges, Some(TiledEdges::new(true, false, true, true)));
}

#[test]
fn decodes_every_tiled_edge_and_capability_bit_independently() {
    let mk = |server_decorations, tiled_edges, capabilities| WaylandConfigureState {
        server_decorations,
        tiled_edges,
        capabilities,
        resizable: true,
        closeable: true,
    };

    assert_eq!(decode_wayland_wm_state(Some(mk(false, Some([true, false, false, false]), None))).tiled_edges, Some(TiledEdges::new(true, false, false, false)));
    assert_eq!(decode_wayland_wm_state(Some(mk(false, Some([false, true, false, false]), None))).tiled_edges, Some(TiledEdges::new(false, true, false, false)));
    assert_eq!(decode_wayland_wm_state(Some(mk(false, Some([false, false, true, false]), None))).tiled_edges, Some(TiledEdges::new(false, false, true, false)));
    assert_eq!(decode_wayland_wm_state(Some(mk(false, Some([false, false, false, true]), None))).tiled_edges, Some(TiledEdges::new(false, false, false, true)));

    // Empty capabilities (0) plus higher unknown bits must not affect server decoration_mode.
    let empty_caps = decode_wayland_wm_state(Some(mk(true, Some([false; 4]), Some(0xFFF0))));
    assert_eq!(empty_caps.decoration_mode, DecorationMode::Server);
    assert_eq!(
        empty_caps.actions,
        WindowActions::default()
            .with_window_menu(Some(false))
            .with_maximize(Some(false))
            .with_minimize(Some(false))
            .with_fullscreen(Some(false))
            .with_close(Some(true)),
    );

    let only_menu = decode_wayland_wm_state(Some(mk(false, None, Some(WAYLAND_CAP_WINDOW_MENU))));
    assert_eq!(only_menu.actions.window_menu, Some(true));
    assert_eq!(only_menu.actions.maximize, Some(false));
    assert_eq!(only_menu.actions.minimize, Some(false));
    assert_eq!(only_menu.actions.fullscreen, Some(false));
    assert_eq!(only_menu.actions.close, Some(true));

    let only_max = decode_wayland_wm_state(Some(mk(false, None, Some(WAYLAND_CAP_MAXIMIZE))));
    assert_eq!(only_max.actions.maximize, Some(true));
    assert_eq!(only_max.actions.window_menu, Some(false));

    let only_full = decode_wayland_wm_state(Some(mk(false, None, Some(WAYLAND_CAP_FULLSCREEN))));
    assert_eq!(only_full.actions.fullscreen, Some(true));
    assert_eq!(only_full.actions.minimize, Some(false));

    let only_min = decode_wayland_wm_state(Some(mk(false, None, Some(WAYLAND_CAP_MINIMIZE))));
    assert_eq!(only_min.actions.minimize, Some(true));
    assert_eq!(only_min.actions.fullscreen, Some(false));
}

#[test]
fn decoded_wayland_transitions_drive_headless_runtime_reactivity() {
    struct State {
        builds: Rc<Cell<usize>>,
        memo_builds: Rc<Cell<usize>>,
    }

    fn view(s: &State, cx: &mut Cx<State>) -> El {
        s.builds.set(s.builds.get() + 1);
        let mode = cx.decoration_mode();
        let tiled = cx.tiled_edges();
        let actions = cx.window_actions();
        let memo_builds = s.memo_builds.clone();
        let w = match mode {
            DecorationMode::Unknown => 10.0,
            DecorationMode::Client => 20.0,
            DecorationMode::Server => 30.0,
        } + if tiled.is_some_and(|e| e.left) { 5.0 } else { 0.0 }
            + if actions.maximize == Some(false) { 2.0 } else { 0.0 };
        row().size(200.0, 40.0).child(memo("wl-bar", 0, move || {
            memo_builds.set(memo_builds.get() + 1);
            el().size(w, 20.0)
        }))
    }

    let builds = Rc::new(Cell::new(0));
    let memo_builds = Rc::new(Cell::new(0));
    let mut rt = Runtime::new(
        State { builds: builds.clone(), memo_builds: memo_builds.clone() },
        view,
        (200.0, 40.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    assert_eq!(rt.presented("wl-bar").unwrap().0[2], 10.0);
    assert_eq!((builds.get(), memo_builds.get()), (1, 1));

    let initial = decode_wayland_wm_state(Some(gate_wayland_configure(
        false,
        [false; 4],
        ALL_WAYLAND_CAPS,
        5,
    )));
    rt.set_window_manager_state(initial);
    assert!(rt.core().stale());
    rt.frame(1.0);
    rt.frame(6.0);
    assert_eq!(rt.presented("wl-bar").unwrap().0[2], 20.0);
    assert_eq!((builds.get(), memo_builds.get()), (2, 2));

    let tiled_no_max = decode_wayland_wm_state(Some(gate_wayland_configure(
        false,
        [true, false, true, true],
        WAYLAND_CAP_WINDOW_MENU | WAYLAND_CAP_MINIMIZE | WAYLAND_CAP_FULLSCREEN,
        5,
    )));
    rt.set_window_manager_state(tiled_no_max);
    assert!(rt.core().stale());
    rt.frame(7.0);
    rt.frame(12.0);
    assert_eq!(rt.presented("wl-bar").unwrap().0[2], 27.0);
    assert_eq!((builds.get(), memo_builds.get()), (3, 3));

    let ssd = decode_wayland_wm_state(Some(gate_wayland_configure(
        true,
        [true, false, true, true],
        WAYLAND_CAP_WINDOW_MENU | WAYLAND_CAP_MINIMIZE | WAYLAND_CAP_FULLSCREEN,
        5,
    )));
    rt.set_window_manager_state(ssd);
    assert!(rt.core().stale());
    rt.frame(13.0);
    rt.frame(18.0);
    assert_eq!(rt.presented("wl-bar").unwrap().0[2], 37.0);
    assert_eq!((builds.get(), memo_builds.get()), (4, 4));
}
