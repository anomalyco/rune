use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;

use super::*;
use crate::{Cx, El, Env, Runtime, el, memo, row};

#[test]
fn bits_stays_within_17_bits_and_distinguishes_every_state_transition() {
    let default = WindowManagerState::default();
    assert_eq!(default.bits(), 0);
    assert!(default.bits() < (1 << 17));

    // All three decoration modes produce distinct bit patterns.
    let modes = [DecorationMode::Unknown, DecorationMode::Server, DecorationMode::Client];
    let mode_bits: HashSet<u32> = modes
        .iter()
        .map(|&m| WindowManagerState::default().with_decoration_mode(m).bits())
        .collect();
    assert_eq!(mode_bits.len(), 3);

    // None (unknown) vs Some(TiledEdges::NONE) (known untiled) vs every 4-edge bitmask (17 total).
    let mut edge_bits = HashSet::new();
    assert!(edge_bits.insert(WindowManagerState::default().with_tiled_edges(None).bits()));
    for mask in 0u8..16 {
        let edges = TiledEdges::new(
            mask & 1 != 0,
            mask & 2 != 0,
            mask & 4 != 0,
            mask & 8 != 0,
        );
        let bits = WindowManagerState::default().with_tiled_edges(Some(edges)).bits();
        assert!(bits < (1 << 17));
        assert!(edge_bits.insert(bits), "duplicate bit pattern for mask {mask:#x}");
    }
    assert_eq!(edge_bits.len(), 17);

    // Every single tri-state action field produces 3 distinct values without colliding with others.
    let tri = [None, Some(false), Some(true)];
    let mut action_bits = HashSet::new();
    assert!(action_bits.insert(WindowManagerState::default().bits()));
    for v in [Some(false), Some(true)] {
        for actions in [
            WindowActions::default().with_window_menu(v),
            WindowActions::default().with_maximize(v),
            WindowActions::default().with_minimize(v),
            WindowActions::default().with_fullscreen(v),
            WindowActions::default().with_close(v),
        ] {
            let bits = WindowManagerState::default().with_actions(actions).bits();
            assert!(bits < (1 << 17));
            assert!(action_bits.insert(bits), "duplicate bit pattern for {actions:?}");
        }
    }
    assert_eq!(action_bits.len(), 1 + 5 * 2);

    // Maximum possible value stays strictly below 1 << 17 (leaving room for << 3 in chrome_bits[4]).
    let all_on = WindowManagerState::default()
        .with_decoration_mode(DecorationMode::Client)
        .with_tiled_edges(Some(TiledEdges::ALL))
        .with_actions(
            WindowActions::default()
                .with_window_menu(tri[2])
                .with_maximize(tri[2])
                .with_minimize(tri[2])
                .with_fullscreen(tri[2])
                .with_close(tri[2]),
        );
    assert!(all_on.bits() < (1 << 17));
}

#[test]
fn tiled_edges_helpers_report_any_and_empty() {
    assert!(!TiledEdges::NONE.any());
    assert!(TiledEdges::NONE.is_empty());
    assert!(TiledEdges::ALL.any());
    assert!(!TiledEdges::ALL.is_empty());
    assert!(TiledEdges::new(true, false, false, false).any());
    assert!(TiledEdges::new(false, true, false, false).any());
    assert!(TiledEdges::new(false, false, true, false).any());
    assert!(TiledEdges::new(false, false, false, true).any());
}

#[test]
fn wm_state_rebuilds_reading_view_and_memos_only_on_change_and_clears_for_staged_subviews() {
    struct Counters {
        view_builds: Rc<Cell<usize>>,
        memo_builds: Rc<Cell<usize>>,
        cleared_mode: Rc<Cell<DecorationMode>>,
        cleared_edges: Rc<Cell<Option<TiledEdges>>>,
        cleared_actions: Rc<Cell<WindowActions>>,
    }

    fn view(c: &Counters, cx: &mut Cx<Counters>) -> El {
        c.view_builds.set(c.view_builds.get() + 1);
        let mode = cx.decoration_mode();
        let edges = cx.tiled_edges();
        let actions = cx.window_actions();

        let mut sub = cx.retype::<()>();
        sub.clear_chrome();
        c.cleared_mode.set(sub.decoration_mode());
        c.cleared_edges.set(sub.tiled_edges());
        c.cleared_actions.set(sub.window_actions());

        let mode_w = match mode {
            DecorationMode::Unknown => 10.0,
            DecorationMode::Server => 20.0,
            DecorationMode::Client => 30.0,
        };
        let edges_w = match edges {
            None => 10.0,
            Some(e) if e.is_empty() => 20.0,
            Some(e) => 30.0 + if e.left { 5.0 } else { 0.0 } + if e.right { 7.0 } else { 0.0 },
        };
        let action_w = match actions.maximize {
            None => 10.0,
            Some(false) => 20.0,
            Some(true) => 30.0,
        };
        let memo_builds = c.memo_builds.clone();
        row().size(300.0, 40.0)
            .child(el().key("mode").size(mode_w, 10.0))
            .child(el().key("edges").size(edges_w, 10.0))
            .child(el().key("action").size(action_w, 10.0))
            .child(memo("wm-memo", 0, move || {
                memo_builds.set(memo_builds.get() + 1);
                el().size(mode_w + edges_w + action_w, 10.0)
            }))
    }

    let view_builds = Rc::new(Cell::new(0));
    let memo_builds = Rc::new(Cell::new(0));
    let cleared_mode = Rc::new(Cell::new(DecorationMode::Client));
    let cleared_edges = Rc::new(Cell::new(Some(TiledEdges::ALL)));
    let cleared_actions = Rc::new(Cell::new(WindowActions::default().with_maximize(Some(true))));
    let mut rt = Runtime::new(
        Counters {
            view_builds: view_builds.clone(),
            memo_builds: memo_builds.clone(),
            cleared_mode: cleared_mode.clone(),
            cleared_edges: cleared_edges.clone(),
            cleared_actions: cleared_actions.clone(),
        },
        view,
        (300.0, 40.0),
        Env::new(true, 1.0),
    );
    let w = |rt: &Runtime<Counters>, key: &str| rt.presented(key).unwrap().0[2];

    // Defaults to Unknown / None / all None.
    rt.frame(0.0);
    assert_eq!((w(&rt, "mode"), w(&rt, "edges"), w(&rt, "action"), w(&rt, "wm-memo")), (10.0, 10.0, 10.0, 30.0));
    assert_eq!((view_builds.get(), memo_builds.get()), (1, 1));

    // Setting the already-current state is a no-op (no dirty, no rebuild).
    rt.set_window_manager_state(WindowManagerState::default());
    assert!(!rt.core().stale());
    rt.frame(1.0);
    assert_eq!((view_builds.get(), memo_builds.get()), (1, 1));

    // Transitioning tiled_edges from None (unknown) to Some(NONE) (known untiled) wakes view & memo.
    let untiled_client = WindowManagerState::default()
        .with_decoration_mode(DecorationMode::Client)
        .with_tiled_edges(Some(TiledEdges::NONE))
        .with_actions(WindowActions::default().with_maximize(Some(true)));
    rt.set_window_manager_state(untiled_client);
    assert!(rt.core().stale());
    rt.frame(2.0);
    rt.frame(7.0);
    assert_eq!((w(&rt, "mode"), w(&rt, "edges"), w(&rt, "action"), w(&rt, "wm-memo")), (30.0, 20.0, 30.0, 80.0));
    assert_eq!((view_builds.get(), memo_builds.get()), (2, 2));
    assert_eq!(cleared_mode.get(), DecorationMode::Unknown);
    assert_eq!(cleared_edges.get(), None);
    assert_eq!(cleared_actions.get(), WindowActions::default());

    // Repeating the same state does not dirty or rebuild.
    rt.set_window_manager_state(untiled_client);
    assert!(!rt.core().stale());
    rt.frame(8.0);
    assert_eq!((view_builds.get(), memo_builds.get()), (2, 2));

    // Tiling left and forbidding maximize rebuilds both view and memo.
    let tiled_left_no_max = WindowManagerState::default()
        .with_decoration_mode(DecorationMode::Server)
        .with_tiled_edges(Some(TiledEdges::new(true, false, true, true)))
        .with_actions(WindowActions::default().with_maximize(Some(false)));
    rt.set_window_manager_state(tiled_left_no_max);
    assert!(rt.core().stale());
    rt.frame(9.0);
    rt.frame(14.0);
    assert_eq!((w(&rt, "mode"), w(&rt, "edges"), w(&rt, "action"), w(&rt, "wm-memo")), (20.0, 35.0, 20.0, 75.0));
    assert_eq!((view_builds.get(), memo_builds.get()), (3, 3));
}

#[test]
fn app_that_never_reads_wm_state_does_no_extra_work_when_wm_state_changes() {
    struct State {
        view_builds: Rc<Cell<usize>>,
        memo_builds: Rc<Cell<usize>>,
        read_later: bool,
        last_mode: Rc<Cell<DecorationMode>>,
    }

    fn view(s: &State, cx: &mut Cx<State>) -> El {
        s.view_builds.set(s.view_builds.get() + 1);
        if s.read_later {
            s.last_mode.set(cx.decoration_mode());
        }
        let memo_builds = s.memo_builds.clone();
        row().size(200.0, 40.0).child(memo("plain", 0, move || {
            memo_builds.set(memo_builds.get() + 1);
            el().size(50.0, 20.0)
        }))
    }

    let view_builds = Rc::new(Cell::new(0));
    let memo_builds = Rc::new(Cell::new(0));
    let last_mode = Rc::new(Cell::new(DecorationMode::Unknown));
    let mut rt = Runtime::new(
        State {
            view_builds: view_builds.clone(),
            memo_builds: memo_builds.clone(),
            read_later: false,
            last_mode: last_mode.clone(),
        },
        view,
        (200.0, 40.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    assert_eq!((view_builds.get(), memo_builds.get()), (1, 1));

    // Changing WM state while the view never reads it must not mark dirty or rebuild anything.
    let tiled = WindowManagerState::default()
        .with_decoration_mode(DecorationMode::Server)
        .with_tiled_edges(Some(TiledEdges::new(true, true, true, false)));
    rt.set_window_manager_state(tiled);
    assert!(!rt.core().stale());
    rt.frame(1.0);
    assert_eq!((view_builds.get(), memo_builds.get()), (1, 1));

    // Once the view starts reading it on an app update, it sees the latest stored value
    // immediately and subsequent WM changes wake it.
    rt.update(|s, _| s.read_later = true);
    rt.frame(2.0);
    assert_eq!(last_mode.get(), DecorationMode::Server);
    assert_eq!(view_builds.get(), 2);

    rt.set_window_manager_state(tiled.with_decoration_mode(DecorationMode::Client));
    assert!(rt.core().stale());
    rt.frame(3.0);
    assert_eq!(last_mode.get(), DecorationMode::Client);
    assert_eq!(view_builds.get(), 3);
}
