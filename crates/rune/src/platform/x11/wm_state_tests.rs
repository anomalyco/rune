use super::*;

fn sample_atoms() -> ActionAtoms {
    ActionAtoms {
        gtk_show_window_menu: 101,
        minimize: 201,
        maximize_horz: 202,
        maximize_vert: 203,
        fullscreen: 204,
        close: 205,
        state_maximized_horz: 301,
        state_maximized_vert: 302,
        state_fullscreen: 303,
    }
}

// Mutter's exact _GTK_EDGE_CONSTRAINTS bitmasks (GNOME/mutter src/x11/window-x11.c & src/core/window.c):
//   bit 0 = TOP_TILED,    bit 1 = TOP_RESIZABLE
//   bit 2 = RIGHT_TILED,  bit 3 = RIGHT_RESIZABLE
//   bit 4 = BOTTOM_TILED, bit 5 = BOTTOM_RESIZABLE
//   bit 6 = LEFT_TILED,   bit 7 = LEFT_RESIZABLE
const MUTTER_UNTILED: u32 = 0xAA; // all 4 RESIZABLE, 0 TILED
const MUTTER_MAXIMIZED: u32 = 0x55; // all 4 TILED, 0 RESIZABLE
const MUTTER_TILE_LEFT: u32 = 0x59; // TOP|BOTTOM|LEFT TILED, RIGHT RESIZABLE
const MUTTER_TILE_RIGHT: u32 = 0x95; // TOP|RIGHT|BOTTOM TILED, LEFT RESIZABLE
const MUTTER_VERT_MAX_ONLY: u32 = 0x99; // TOP|BOTTOM TILED, LEFT|RIGHT RESIZABLE (Ubuntu Tiling Assistant)

#[test]
fn frame_extents_decodes_server_client_borderless_fullscreen_absent_and_malformed() {
    let unk = DecorationMode::Unknown;
    let srv = DecorationMode::Server;
    let cli = DecorationMode::Client;

    // Malformed lengths always stay Unknown (outside fullscreen).
    assert_eq!(decode_frame_extents(Some(&[]), false, true, false, false, unk), unk);
    assert_eq!(decode_frame_extents(Some(&[0, 0, 0]), false, true, false, false, unk), unk);
    assert_eq!(decode_frame_extents(Some(&[0, 0, 0, 0, 0]), false, true, false, false, unk), unk);
    assert_eq!(decode_frame_extents(Some(&[1, 1, 24, 1, 9]), true, true, true, false, unk), unk);

    // Explicit four zeroes -> Client unless preserving a framed titled window's Server mode.
    assert_eq!(decode_frame_extents(Some(&[0, 0, 0, 0]), true, true, false, false, unk), cli);
    assert_eq!(decode_frame_extents(Some(&[0, 0, 0, 0]), false, true, false, false, unk), cli);
    assert_eq!(decode_frame_extents(Some(&[0, 0, 0, 0]), false, true, false, false, srv), cli);

    // Any non-zero extent -> Server even if borderless was requested.
    assert_eq!(decode_frame_extents(Some(&[1, 0, 0, 0]), false, true, true, false, unk), srv);
    assert_eq!(decode_frame_extents(Some(&[0, 1, 0, 0]), true, true, true, false, unk), srv);
    assert_eq!(decode_frame_extents(Some(&[0, 0, 37, 0]), true, true, true, false, unk), srv);
    assert_eq!(decode_frame_extents(Some(&[0, 0, 0, 1]), true, true, true, false, unk), srv);

    // Absent _NET_FRAME_EXTENTS:
    assert_eq!(decode_frame_extents(None, true, true, true, false, unk), unk);
    assert_eq!(decode_frame_extents(None, true, true, false, false, unk), unk);
    assert_eq!(decode_frame_extents(None, false, true, false, false, unk), cli);
    assert_eq!(decode_frame_extents(None, false, false, false, false, unk), unk);
    assert_eq!(decode_frame_extents(None, false, true, true, false, unk), unk);

    // Full screen: preserves prior confirmed mode when Mutter collapses frame borders to [0, 0, 0, 0].
    assert_eq!(decode_frame_extents(Some(&[0, 0, 0, 0]), true, true, true, true, srv), srv);
    assert_eq!(decode_frame_extents(None, false, true, false, true, cli), cli);
    // Framed titled window launching directly into fullscreen stays Unknown rather than Client.
    assert_eq!(decode_frame_extents(Some(&[0, 0, 0, 0]), true, true, true, true, unk), unk);
    // 1-frame window right after leaving fullscreen while mutter-x11-frames still reports [0, 0, 0, 0]:
    // still framed & titled with previous_mode == Server -> stays Server (no stale Client frame).
    assert_eq!(decode_frame_extents(Some(&[0, 0, 0, 0]), true, true, true, false, srv), srv);
}

#[test]
fn edge_constraints_decodes_each_tiled_bit_ignores_resizable_bits_and_handles_absence() {
    assert_eq!(decode_edge_constraints(None, false), None);
    assert_eq!(decode_edge_constraints(None, true), Some(TiledEdges::NONE));
    assert_eq!(decode_edge_constraints(Some(&[]), true), None);
    assert_eq!(decode_edge_constraints(Some(&[GTK_EDGE_TOP_TILED, 0]), true), None);

    assert_eq!(decode_edge_constraints(Some(&[MUTTER_UNTILED]), true), Some(TiledEdges::NONE));
    assert_eq!(decode_edge_constraints(Some(&[MUTTER_MAXIMIZED]), true), Some(TiledEdges::ALL));
    assert_eq!(decode_edge_constraints(Some(&[MUTTER_TILE_LEFT]), true), Some(TiledEdges::new(true, false, true, true)));
    assert_eq!(decode_edge_constraints(Some(&[MUTTER_TILE_RIGHT]), true), Some(TiledEdges::new(true, true, true, false)));

    assert_eq!(decode_edge_constraints(Some(&[GTK_EDGE_TOP_TILED]), false), Some(TiledEdges::new(true, false, false, false)));
    assert_eq!(decode_edge_constraints(Some(&[GTK_EDGE_RIGHT_TILED]), false), Some(TiledEdges::new(false, true, false, false)));
    assert_eq!(decode_edge_constraints(Some(&[GTK_EDGE_BOTTOM_TILED]), false), Some(TiledEdges::new(false, false, true, false)));
    assert_eq!(decode_edge_constraints(Some(&[GTK_EDGE_LEFT_TILED]), false), Some(TiledEdges::new(false, false, false, true)));

    let resizable_only = (1 << 1) | (1 << 3) | (1 << 5) | (1 << 7) | 0xFFFF_FF00;
    assert_eq!(decode_edge_constraints(Some(&[resizable_only]), true), Some(TiledEdges::NONE));
}

#[test]
fn mutter_gnome_table_titled_and_borderless_cells() {
    let atoms = sample_atoms();
    let edge_atom = 401;
    let supported = [atoms.gtk_show_window_menu, edge_atom];
    let normal_actions = [atoms.minimize, atoms.maximize_horz, atoms.maximize_vert, atoms.fullscreen, atoms.close];
    let monitor = PixelRect::new(0, 0, 1920, 1080);
    let net_workarea = [68u32, 32, 1852, 1048];
    let work_area = resolve_work_area(Some(&net_workarea), Some(0), Some(monitor));
    assert_eq!(work_area, Some(PixelRect::new(68, 32, 1852, 1048)));

    let left_half = Some(PixelRect::new(68, 32, 926, 1048));
    let right_half = Some(PixelRect::new(68 + 926, 32, 926, 1048));
    let centered_vert_max = Some(PixelRect::new(400, 32, 926, 1048));

    // 1. Titled window tiled left (stock Mutter writes 0x59 on frame, even if client holds stale 0xAA).
    let titled_tile_left_stock = decode_wm_state(RawX11WmState {
        frame_extents: Some(&[0, 0, 37, 0]),
        client_edge_constraints: Some(&[MUTTER_UNTILED]),
        frame_edge_constraints: Some(&[MUTTER_TILE_LEFT]),
        wm_state: Some(&[atoms.state_maximized_vert]),
        allowed_actions: Some(&normal_actions),
        net_supported: Some(&supported),
        gtk_edge_constraints_atom: edge_atom,
        action_atoms: atoms,
        requested_decorations: true,
        has_wm_frame: true,
        outer_rect: left_half,
        work_area,
        ..Default::default()
    });
    assert_eq!(titled_tile_left_stock.decoration_mode, DecorationMode::Server);
    assert_eq!(titled_tile_left_stock.tiled_edges, Some(TiledEdges::new(true, false, true, true)));

    // Titled window on Ubuntu 26.04 (ubuntu-tiling-assistant writes 0x99 on frame + MAXIMIZED_VERT).
    let titled_tile_right_ubuntu = decode_wm_state(RawX11WmState {
        frame_extents: Some(&[0, 0, 37, 0]),
        client_edge_constraints: None,
        frame_edge_constraints: Some(&[MUTTER_VERT_MAX_ONLY]),
        wm_state: Some(&[atoms.state_maximized_vert]),
        allowed_actions: Some(&normal_actions),
        net_supported: Some(&supported),
        gtk_edge_constraints_atom: edge_atom,
        action_atoms: atoms,
        requested_decorations: true,
        has_wm_frame: true,
        outer_rect: right_half,
        work_area,
        ..Default::default()
    });
    assert_eq!(titled_tile_right_ubuntu.decoration_mode, DecorationMode::Server);
    assert_eq!(titled_tile_right_ubuntu.tiled_edges, Some(TiledEdges::new(true, true, true, false)));

    // Titled window maximized: agrees with borderless X11 and Wayland (TiledEdges::ALL) even when
    // client holds a stale pre-frame 0xAA or frame _GTK_EDGE_CONSTRAINTS has not updated yet.
    let titled_max_stale_client = decode_wm_state(RawX11WmState {
        frame_extents: Some(&[0, 0, 37, 0]),
        client_edge_constraints: Some(&[MUTTER_UNTILED]),
        frame_edge_constraints: None,
        wm_state: Some(&[atoms.state_maximized_vert, atoms.state_maximized_horz]),
        allowed_actions: Some(&normal_actions),
        net_supported: Some(&supported),
        gtk_edge_constraints_atom: edge_atom,
        action_atoms: atoms,
        requested_decorations: true,
        has_wm_frame: true,
        outer_rect: work_area,
        work_area,
        ..Default::default()
    });
    assert_eq!(titled_max_stale_client.tiled_edges, Some(TiledEdges::ALL));

    let titled_max_frame = decode_wm_state(RawX11WmState {
        frame_extents: Some(&[0, 0, 37, 0]),
        client_edge_constraints: Some(&[MUTTER_UNTILED]),
        frame_edge_constraints: Some(&[MUTTER_MAXIMIZED]),
        wm_state: Some(&[atoms.state_maximized_vert, atoms.state_maximized_horz]),
        allowed_actions: Some(&normal_actions),
        net_supported: Some(&supported),
        gtk_edge_constraints_atom: edge_atom,
        action_atoms: atoms,
        requested_decorations: true,
        has_wm_frame: true,
        outer_rect: work_area,
        work_area,
        ..Default::default()
    });
    assert_eq!(titled_max_frame.tiled_edges, Some(TiledEdges::ALL));

    let titled_restored = decode_wm_state(RawX11WmState {
        frame_extents: Some(&[0, 0, 37, 0]),
        client_edge_constraints: None,
        frame_edge_constraints: Some(&[MUTTER_UNTILED]),
        wm_state: Some(&[]),
        allowed_actions: Some(&normal_actions),
        net_supported: Some(&supported),
        gtk_edge_constraints_atom: edge_atom,
        action_atoms: atoms,
        requested_decorations: true,
        has_wm_frame: true,
        outer_rect: Some(PixelRect::new(200, 150, 720, 420)),
        work_area,
        ..Default::default()
    });
    assert_eq!(titled_restored.tiled_edges, Some(TiledEdges::NONE));

    // 2. Borderless window tiled left/right on Ubuntu 26.04.
    let borderless_tile_left = decode_wm_state(RawX11WmState {
        frame_extents: None,
        client_edge_constraints: Some(&[MUTTER_VERT_MAX_ONLY]),
        frame_edge_constraints: None,
        wm_state: Some(&[atoms.state_maximized_vert]),
        allowed_actions: Some(&normal_actions),
        net_supported: Some(&supported),
        gtk_edge_constraints_atom: edge_atom,
        action_atoms: atoms,
        requested_decorations: false,
        has_wm_frame: false,
        outer_rect: left_half,
        work_area,
        ..Default::default()
    });
    assert_eq!(borderless_tile_left.decoration_mode, DecorationMode::Client);
    assert_eq!(borderless_tile_left.tiled_edges, Some(TiledEdges::new(true, false, true, true)));

    let borderless_tile_right = decode_wm_state(RawX11WmState {
        frame_extents: None,
        client_edge_constraints: Some(&[MUTTER_VERT_MAX_ONLY]),
        frame_edge_constraints: None,
        wm_state: Some(&[atoms.state_maximized_vert]),
        allowed_actions: Some(&normal_actions),
        net_supported: Some(&supported),
        gtk_edge_constraints_atom: edge_atom,
        action_atoms: atoms,
        requested_decorations: false,
        has_wm_frame: false,
        outer_rect: right_half,
        work_area,
        ..Default::default()
    });
    assert_eq!(borderless_tile_right.decoration_mode, DecorationMode::Client);
    assert_eq!(borderless_tile_right.tiled_edges, Some(TiledEdges::new(true, true, true, false)));

    // Floating vertically-maximized window touching neither side must not set left/right.
    let floating_vert_max = decode_wm_state(RawX11WmState {
        frame_extents: None,
        client_edge_constraints: Some(&[MUTTER_VERT_MAX_ONLY]),
        frame_edge_constraints: None,
        wm_state: Some(&[atoms.state_maximized_vert]),
        allowed_actions: Some(&normal_actions),
        net_supported: Some(&supported),
        gtk_edge_constraints_atom: edge_atom,
        action_atoms: atoms,
        requested_decorations: false,
        has_wm_frame: false,
        outer_rect: centered_vert_max,
        work_area,
        ..Default::default()
    });
    assert_eq!(floating_vert_max.tiled_edges, Some(TiledEdges::new(true, false, true, false)));
}

#[test]
fn titled_x11_window_keeps_server_decoration_mode_through_fullscreen_and_exit() {
    let atoms = sample_atoms();
    let edge_atom = 401;
    let supported = [atoms.gtk_show_window_menu, edge_atom];
    let normal_actions = [atoms.minimize, atoms.maximize_horz, atoms.maximize_vert, atoms.fullscreen, atoms.close];
    let fullscreen_actions = [atoms.minimize, atoms.fullscreen, atoms.close];

    // Before fullscreen: titled window has Server decorations.
    let normal = decode_wm_state(RawX11WmState {
        frame_extents: Some(&[0, 0, 37, 0]),
        frame_edge_constraints: Some(&[MUTTER_UNTILED]),
        wm_state: Some(&[]),
        allowed_actions: Some(&normal_actions),
        net_supported: Some(&supported),
        gtk_edge_constraints_atom: edge_atom,
        action_atoms: atoms,
        requested_decorations: true,
        has_wm_frame: true,
        ..Default::default()
    });
    assert_eq!(normal.decoration_mode, DecorationMode::Server);
    assert_eq!(normal.actions.maximize, Some(true));

    // In fullscreen: Mutter collapses _NET_FRAME_EXTENTS to [0, 0, 0, 0] and drops maximize action.
    // DecorationMode must stay Server while actions.maximize honestly reports Some(false).
    let in_fullscreen = decode_wm_state(RawX11WmState {
        frame_extents: Some(&[0, 0, 0, 0]),
        frame_edge_constraints: Some(&[MUTTER_UNTILED]),
        wm_state: Some(&[atoms.state_fullscreen]),
        allowed_actions: Some(&fullscreen_actions),
        net_supported: Some(&supported),
        gtk_edge_constraints_atom: edge_atom,
        action_atoms: atoms,
        requested_decorations: true,
        has_wm_frame: true,
        previous_decoration_mode: normal.decoration_mode,
        ..Default::default()
    });
    assert_eq!(in_fullscreen.decoration_mode, DecorationMode::Server);
    assert_eq!(in_fullscreen.actions.maximize, Some(false));

    // Frame 1 right after leaving fullscreen: _NET_WM_STATE_FULLSCREEN cleared before
    // mutter-x11-frames restores [0, 0, 37, 0] (extents still [0, 0, 0, 0]). Must NOT flash Client.
    let exit_frame_1 = decode_wm_state(RawX11WmState {
        frame_extents: Some(&[0, 0, 0, 0]),
        frame_edge_constraints: Some(&[MUTTER_UNTILED]),
        wm_state: Some(&[]),
        allowed_actions: Some(&normal_actions),
        net_supported: Some(&supported),
        gtk_edge_constraints_atom: edge_atom,
        action_atoms: atoms,
        requested_decorations: true,
        has_wm_frame: true,
        previous_decoration_mode: in_fullscreen.decoration_mode,
        ..Default::default()
    });
    assert_eq!(exit_frame_1.decoration_mode, DecorationMode::Server);
    assert_eq!(exit_frame_1.actions.maximize, Some(true));

    // Frame 2 after leaving fullscreen: _NET_FRAME_EXTENTS restored to [0, 0, 37, 0].
    let exit_frame_2 = decode_wm_state(RawX11WmState {
        frame_extents: Some(&[0, 0, 37, 0]),
        frame_edge_constraints: Some(&[MUTTER_UNTILED]),
        wm_state: Some(&[]),
        allowed_actions: Some(&normal_actions),
        net_supported: Some(&supported),
        gtk_edge_constraints_atom: edge_atom,
        action_atoms: atoms,
        requested_decorations: true,
        has_wm_frame: true,
        previous_decoration_mode: exit_frame_1.decoration_mode,
        ..Default::default()
    });
    assert_eq!(exit_frame_2.decoration_mode, DecorationMode::Server);
}

#[test]
fn allowed_actions_decodes_per_window_actions_partial_maximize_and_window_menu() {
    let atoms = sample_atoms();

    assert_eq!(decode_allowed_actions(None, None, atoms), WindowActions::default());

    let supported_no_menu = decode_allowed_actions(None, Some(&[999]), atoms);
    assert_eq!(supported_no_menu, WindowActions::default().with_window_menu(Some(false)));

    let supported_with_menu = decode_allowed_actions(None, Some(&[999, atoms.gtk_show_window_menu]), atoms);
    assert_eq!(supported_with_menu, WindowActions::default().with_window_menu(Some(true)));

    let empty_allowed = decode_allowed_actions(Some(&[]), None, atoms);
    assert_eq!(
        empty_allowed,
        WindowActions::default()
            .with_maximize(Some(false))
            .with_minimize(Some(false))
            .with_fullscreen(Some(false))
            .with_close(Some(false)),
    );

    let horz_only = decode_allowed_actions(Some(&[atoms.maximize_horz, atoms.close]), None, atoms);
    assert_eq!(horz_only.maximize, Some(false));
    assert_eq!(horz_only.close, Some(true));
    assert_eq!(horz_only.minimize, Some(false));
    assert_eq!(horz_only.fullscreen, Some(false));

    let vert_only = decode_allowed_actions(Some(&[atoms.maximize_vert]), None, atoms);
    assert_eq!(vert_only.maximize, Some(false));

    let full = decode_allowed_actions(
        Some(&[atoms.minimize, atoms.maximize_horz, atoms.maximize_vert, atoms.fullscreen, atoms.close]),
        Some(&[atoms.gtk_show_window_menu]),
        atoms,
    );
    assert_eq!(
        full,
        WindowActions::default()
            .with_window_menu(Some(true))
            .with_maximize(Some(true))
            .with_minimize(Some(true))
            .with_fullscreen(Some(true))
            .with_close(Some(true)),
    );

    let zero_atoms = ActionAtoms::default();
    let with_zero = decode_allowed_actions(Some(&[0]), Some(&[0]), zero_atoms);
    assert_eq!(
        with_zero,
        WindowActions::default()
            .with_window_menu(Some(false))
            .with_maximize(Some(false))
            .with_minimize(Some(false))
            .with_fullscreen(Some(false))
            .with_close(Some(false)),
    );
}

#[test]
fn decode_wm_state_never_infers_server_decoration_mode_from_actions_or_supported() {
    let atoms = sample_atoms();
    let edge_atom = 401;

    let state = decode_wm_state(RawX11WmState {
        frame_extents: None,
        client_edge_constraints: None,
        frame_edge_constraints: None,
        wm_state: Some(&[]),
        allowed_actions: Some(&[]),
        net_supported: Some(&[edge_atom]),
        gtk_edge_constraints_atom: edge_atom,
        action_atoms: atoms,
        requested_decorations: true,
        has_wm_frame: true,
        ..Default::default()
    });
    assert_eq!(state.decoration_mode, DecorationMode::Unknown);
    assert_eq!(state.tiled_edges, Some(TiledEdges::NONE));
    assert_eq!(state.actions.window_menu, Some(false));
    assert_eq!(state.actions.maximize, Some(false));
}

#[test]
fn x11_property_notify_reread_wakes_headless_runtime_from_pre_map_to_framed_and_tiled() {
    use std::cell::Cell;
    use std::rc::Rc;
    use crate::{Cx, El, Env, Runtime, el, memo, row};

    struct Counters {
        builds: Rc<Cell<usize>>,
        memo_builds: Rc<Cell<usize>>,
    }

    fn view(c: &Counters, cx: &mut Cx<Counters>) -> El {
        c.builds.set(c.builds.get() + 1);
        let mode = cx.decoration_mode();
        let tiled = cx.tiled_edges();
        let memo_builds = c.memo_builds.clone();
        let w = match mode {
            DecorationMode::Unknown => 10.0,
            DecorationMode::Server => 20.0,
            DecorationMode::Client => 30.0,
        } + if tiled.is_some_and(|e| e.left) { 5.0 } else { 0.0 };
        row().size(200.0, 40.0).child(memo("x11-bar", 0, move || {
            memo_builds.set(memo_builds.get() + 1);
            el().size(w, 20.0)
        }))
    }

    let atoms = sample_atoms();
    let edge_atom = 401;
    let supported = [atoms.gtk_show_window_menu, edge_atom];
    let work_area = Some(PixelRect::new(68, 32, 1852, 1048));
    let builds = Rc::new(Cell::new(0));
    let memo_builds = Rc::new(Cell::new(0));
    let mut rt = Runtime::new(
        Counters { builds: builds.clone(), memo_builds: memo_builds.clone() },
        view,
        (200.0, 40.0),
        Env::new(true, 1.0),
    );

    rt.frame(0.0);
    assert_eq!(rt.presented("x11-bar").unwrap().0[2], 10.0);
    assert_eq!((builds.get(), memo_builds.get()), (1, 1));

    let framed = decode_wm_state(RawX11WmState {
        frame_extents: Some(&[0, 0, 37, 0]),
        client_edge_constraints: None,
        frame_edge_constraints: Some(&[MUTTER_UNTILED]),
        wm_state: Some(&[]),
        allowed_actions: Some(&[atoms.minimize, atoms.maximize_horz, atoms.maximize_vert, atoms.close]),
        net_supported: Some(&supported),
        gtk_edge_constraints_atom: edge_atom,
        action_atoms: atoms,
        requested_decorations: true,
        has_wm_frame: true,
        outer_rect: Some(PixelRect::new(200, 150, 720, 420)),
        work_area,
        ..Default::default()
    });
    rt.set_window_manager_state(framed);
    assert!(rt.core().stale());
    rt.frame(1.0);
    rt.frame(6.0);
    assert_eq!(rt.presented("x11-bar").unwrap().0[2], 20.0);
    assert_eq!((builds.get(), memo_builds.get()), (2, 2));

    let tiled_left = decode_wm_state(RawX11WmState {
        frame_extents: Some(&[0, 0, 37, 0]),
        client_edge_constraints: None,
        frame_edge_constraints: Some(&[MUTTER_VERT_MAX_ONLY]),
        wm_state: Some(&[atoms.state_maximized_vert]),
        allowed_actions: Some(&[atoms.minimize, atoms.maximize_horz, atoms.maximize_vert, atoms.close]),
        net_supported: Some(&supported),
        gtk_edge_constraints_atom: edge_atom,
        action_atoms: atoms,
        requested_decorations: true,
        has_wm_frame: true,
        previous_decoration_mode: framed.decoration_mode,
        outer_rect: Some(PixelRect::new(68, 32, 926, 1048)),
        work_area,
        ..Default::default()
    });
    rt.set_window_manager_state(tiled_left);
    assert!(rt.core().stale());
    rt.frame(7.0);
    rt.frame(12.0);
    assert_eq!(rt.presented("x11-bar").unwrap().0[2], 25.0);
    assert_eq!((builds.get(), memo_builds.get()), (3, 3));
}
