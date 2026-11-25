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
    // v1 compositor: decoration_mode known, tiled edges and capabilities remain Unknown (None)
    // even when sctk's default capabilities are WindowManagerCapabilities::all().
    let v1 = decode_wayland_wm_state(Some(gate_wayland_configure(
        false,
        [true, false, true, true],
        ALL_WAYLAND_CAPS,
        1,
    )));
    assert_eq!(v1.decoration_mode, DecorationMode::Client);
    assert_eq!(v1.tiled_edges, None);
    assert_eq!(v1.actions, WindowActions::default());

    // v2..=v4 compositor: tiled edges known, wm_capabilities still Unknown (None) so sctk's
    // default all() never leaks as confirmed capabilities.
    for version in 2..=4 {
        let state = decode_wayland_wm_state(Some(gate_wayland_configure(
            true,
            [true, false, true, true],
            ALL_WAYLAND_CAPS,
            version,
        )));
        assert_eq!(state.decoration_mode, DecorationMode::Server);
        assert_eq!(state.tiled_edges, Some(TiledEdges::new(true, false, true, true)));
        assert_eq!(state.actions, WindowActions::default(), "v{version} must not report capabilities");
    }

    // v5+ compositor: all three are reported; close remains None (absent from xdg-shell).
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
            .with_close(None),
    );
}

#[test]
fn decodes_every_tiled_edge_and_capability_bit_independently() {
    // Every tiled edge individually.
    assert_eq!(
        decode_wayland_wm_state(Some(WaylandConfigureState {
            server_decorations: false,
            tiled_edges: Some([true, false, false, false]),
            capabilities: None,
        }))
        .tiled_edges,
        Some(TiledEdges::new(true, false, false, false)),
    );
    assert_eq!(
        decode_wayland_wm_state(Some(WaylandConfigureState {
            server_decorations: false,
            tiled_edges: Some([false, true, false, false]),
            capabilities: None,
        }))
        .tiled_edges,
        Some(TiledEdges::new(false, true, false, false)),
    );
    assert_eq!(
        decode_wayland_wm_state(Some(WaylandConfigureState {
            server_decorations: false,
            tiled_edges: Some([false, false, true, false]),
            capabilities: None,
        }))
        .tiled_edges,
        Some(TiledEdges::new(false, false, true, false)),
    );
    assert_eq!(
        decode_wayland_wm_state(Some(WaylandConfigureState {
            server_decorations: false,
            tiled_edges: Some([false, false, false, true]),
            capabilities: None,
        }))
        .tiled_edges,
        Some(TiledEdges::new(false, false, false, true)),
    );

    // Every capability bit individually, plus empty (0) and higher unknown bits ignored.
    let empty_caps = decode_wayland_wm_state(Some(WaylandConfigureState {
        server_decorations: true,
        tiled_edges: Some([false; 4]),
        capabilities: Some(0xFFF0),
    }));
    // Empty capabilities must not affect server decoration_mode.
    assert_eq!(empty_caps.decoration_mode, DecorationMode::Server);
    assert_eq!(
        empty_caps.actions,
        WindowActions::default()
            .with_window_menu(Some(false))
            .with_maximize(Some(false))
            .with_minimize(Some(false))
            .with_fullscreen(Some(false))
            .with_close(None),
    );

    let only_menu = decode_wayland_wm_state(Some(WaylandConfigureState {
        server_decorations: false,
        tiled_edges: None,
        capabilities: Some(WAYLAND_CAP_WINDOW_MENU),
    }));
    assert_eq!(only_menu.actions.window_menu, Some(true));
    assert_eq!(only_menu.actions.maximize, Some(false));
    assert_eq!(only_menu.actions.minimize, Some(false));
    assert_eq!(only_menu.actions.fullscreen, Some(false));
    assert_eq!(only_menu.actions.close, None);

    let only_max = decode_wayland_wm_state(Some(WaylandConfigureState {
        server_decorations: false,
        tiled_edges: None,
        capabilities: Some(WAYLAND_CAP_MAXIMIZE),
    }));
    assert_eq!(only_max.actions.maximize, Some(true));
    assert_eq!(only_max.actions.window_menu, Some(false));

    let only_full = decode_wayland_wm_state(Some(WaylandConfigureState {
        server_decorations: false,
        tiled_edges: None,
        capabilities: Some(WAYLAND_CAP_FULLSCREEN),
    }));
    assert_eq!(only_full.actions.fullscreen, Some(true));
    assert_eq!(only_full.actions.minimize, Some(false));

    let only_min = decode_wayland_wm_state(Some(WaylandConfigureState {
        server_decorations: false,
        tiled_edges: None,
        capabilities: Some(WAYLAND_CAP_MINIMIZE),
    }));
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

    // Initial configure on a Wayland v5 CSD compositor (untiled, all capabilities).
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

    // Compositor tiles left and drops MAXIMIZE capability without changing size.
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

    // Compositor switches to server-side decorations without resizing.
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
