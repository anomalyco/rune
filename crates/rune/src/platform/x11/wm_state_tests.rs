use super::*;

fn sample_atoms() -> ActionAtoms {
    ActionAtoms {
        gtk_show_window_menu: 101,
        minimize: 201,
        maximize_horz: 202,
        maximize_vert: 203,
        fullscreen: 204,
        close: 205,
    }
}

#[test]
fn frame_extents_decodes_server_client_absent_and_malformed() {
    assert_eq!(decode_frame_extents(None), DecorationMode::Unknown);
    assert_eq!(decode_frame_extents(Some(&[])), DecorationMode::Unknown);
    assert_eq!(decode_frame_extents(Some(&[0, 0, 0])), DecorationMode::Unknown);
    assert_eq!(decode_frame_extents(Some(&[0, 0, 0, 0, 0])), DecorationMode::Unknown);
    assert_eq!(decode_frame_extents(Some(&[1, 1, 24, 1, 9])), DecorationMode::Unknown);

    assert_eq!(decode_frame_extents(Some(&[0, 0, 0, 0])), DecorationMode::Client);

    assert_eq!(decode_frame_extents(Some(&[1, 0, 0, 0])), DecorationMode::Server);
    assert_eq!(decode_frame_extents(Some(&[0, 1, 0, 0])), DecorationMode::Server);
    assert_eq!(decode_frame_extents(Some(&[0, 0, 32, 0])), DecorationMode::Server);
    assert_eq!(decode_frame_extents(Some(&[0, 0, 0, 1])), DecorationMode::Server);
    assert_eq!(decode_frame_extents(Some(&[2, 2, 34, 2])), DecorationMode::Server);
}

#[test]
fn edge_constraints_decodes_each_tiled_bit_ignores_resizable_bits_and_handles_absence() {
    // Absent and not advertised in _NET_SUPPORTED -> Unknown (None).
    assert_eq!(decode_edge_constraints(None, false), None);

    // Absent while advertised in _NET_SUPPORTED -> known untiled (Some(NONE)).
    assert_eq!(decode_edge_constraints(None, true), Some(TiledEdges::NONE));

    // Malformed lengths (0 or >1 cardinals) -> Unknown (None), even when advertised.
    assert_eq!(decode_edge_constraints(Some(&[]), true), None);
    assert_eq!(decode_edge_constraints(Some(&[GTK_EDGE_TOP_TILED, 0]), true), None);

    // Individual tiled bits: bit 0=top, bit 2=right, bit 4=bottom, bit 6=left.
    assert_eq!(decode_edge_constraints(Some(&[0]), false), Some(TiledEdges::NONE));
    assert_eq!(decode_edge_constraints(Some(&[GTK_EDGE_TOP_TILED]), false), Some(TiledEdges::new(true, false, false, false)));
    assert_eq!(decode_edge_constraints(Some(&[GTK_EDGE_RIGHT_TILED]), false), Some(TiledEdges::new(false, true, false, false)));
    assert_eq!(decode_edge_constraints(Some(&[GTK_EDGE_BOTTOM_TILED]), false), Some(TiledEdges::new(false, false, true, false)));
    assert_eq!(decode_edge_constraints(Some(&[GTK_EDGE_LEFT_TILED]), false), Some(TiledEdges::new(false, false, false, true)));

    // GNOME left-half tile (top + bottom + left) and right-half tile (top + bottom + right).
    let left_half = GTK_EDGE_TOP_TILED | GTK_EDGE_BOTTOM_TILED | GTK_EDGE_LEFT_TILED;
    let right_half = GTK_EDGE_TOP_TILED | GTK_EDGE_BOTTOM_TILED | GTK_EDGE_RIGHT_TILED;
    assert_eq!(decode_edge_constraints(Some(&[left_half]), true), Some(TiledEdges::new(true, false, true, true)));
    assert_eq!(decode_edge_constraints(Some(&[right_half]), true), Some(TiledEdges::new(true, true, true, false)));

    // Resizable bits (1, 3, 5, 7) and higher bits do not count as tiled.
    let resizable_only = (1 << 1) | (1 << 3) | (1 << 5) | (1 << 7) | 0xFFFF_FF00;
    assert_eq!(decode_edge_constraints(Some(&[resizable_only]), true), Some(TiledEdges::NONE));

    // All four tiled bits plus resizable bits.
    let all_bits = GTK_EDGE_TOP_TILED | GTK_EDGE_RIGHT_TILED | GTK_EDGE_BOTTOM_TILED | GTK_EDGE_LEFT_TILED | resizable_only;
    assert_eq!(decode_edge_constraints(Some(&[all_bits]), false), Some(TiledEdges::ALL));
}

#[test]
fn allowed_actions_decodes_per_window_actions_partial_maximize_and_window_menu() {
    let atoms = sample_atoms();

    // Both properties absent -> all Unknown (None).
    assert_eq!(decode_allowed_actions(None, None, atoms), WindowActions::default());

    // _NET_SUPPORTED present without _GTK_SHOW_WINDOW_MENU -> window_menu is Some(false);
    // per-window actions remain None when _NET_WM_ALLOWED_ACTIONS is absent.
    let supported_no_menu = decode_allowed_actions(None, Some(&[999]), atoms);
    assert_eq!(supported_no_menu, WindowActions::default().with_window_menu(Some(false)));

    // _NET_SUPPORTED present with _GTK_SHOW_WINDOW_MENU -> window_menu is Some(true).
    let supported_with_menu = decode_allowed_actions(None, Some(&[999, atoms.gtk_show_window_menu]), atoms);
    assert_eq!(supported_with_menu, WindowActions::default().with_window_menu(Some(true)));

    // Empty _NET_WM_ALLOWED_ACTIONS means WM explicitly allows none of these actions.
    let empty_allowed = decode_allowed_actions(Some(&[]), None, atoms);
    assert_eq!(
        empty_allowed,
        WindowActions::default()
            .with_maximize(Some(false))
            .with_minimize(Some(false))
            .with_fullscreen(Some(false))
            .with_close(Some(false)),
    );

    // Partial maximize (only HORZ or only VERT) is not full maximize.
    let horz_only = decode_allowed_actions(Some(&[atoms.maximize_horz, atoms.close]), None, atoms);
    assert_eq!(horz_only.maximize, Some(false));
    assert_eq!(horz_only.close, Some(true));
    assert_eq!(horz_only.minimize, Some(false));
    assert_eq!(horz_only.fullscreen, Some(false));

    let vert_only = decode_allowed_actions(Some(&[atoms.maximize_vert]), None, atoms);
    assert_eq!(vert_only.maximize, Some(false));

    // Full set allowed.
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

    // Uninterned (0) atoms never match even if 0 appears in a property list.
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
fn decode_wm_state_never_infers_decoration_mode_from_actions_or_supported() {
    let atoms = sample_atoms();
    let edge_atom = 301;

    // Missing _NET_FRAME_EXTENTS stays Unknown even when _NET_SUPPORTED and actions are empty.
    let state = decode_wm_state(None, None, Some(&[]), Some(&[edge_atom]), edge_atom, atoms);
    assert_eq!(state.decoration_mode, DecorationMode::Unknown);
    assert_eq!(state.tiled_edges, Some(TiledEdges::NONE));
    assert_eq!(state.actions.window_menu, Some(false));
    assert_eq!(state.actions.maximize, Some(false));

    // Missing _NET_WM_ALLOWED_ACTIONS and _NET_SUPPORTED do not erase Server/Client extents.
    let server = decode_wm_state(Some(&[0, 0, 28, 0]), None, None, None, edge_atom, atoms);
    assert_eq!(server.decoration_mode, DecorationMode::Server);
    assert_eq!(server.tiled_edges, None);
    assert_eq!(server.actions, WindowActions::default());

    let client = decode_wm_state(Some(&[0, 0, 0, 0]), Some(&[GTK_EDGE_LEFT_TILED]), None, None, edge_atom, atoms);
    assert_eq!(client.decoration_mode, DecorationMode::Client);
    assert_eq!(client.tiled_edges, Some(TiledEdges::new(false, false, false, true)));
    assert_eq!(client.actions, WindowActions::default());
}
