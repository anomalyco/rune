use super::*;
use crate::el::{ScrollSpec, col, el};
use crate::live::Live;

/// A 100 pt scroll over `rows` items of one dot each, plus an optional visible badge outside
/// the scroll so tests can remove one visible item while hundreds of rows sit off-screen.
struct ListState {
    rows: usize,
    show_badge: bool,
}

fn list_view(s: &ListState, _cx: &mut Cx<ListState>) -> El {
    let rows = (0..s.rows).map(|i| {
        col()
            .key(format!("r{i}"))
            .w_full()
            .h(40.0)
            .child(el().key(format!("d{i}")).size(10.0, 10.0).fill(Color::WHITE))
    });
    let mut root = col().w_full().h_full().child(
        col()
            .key("scroll")
            .w_full()
            .h(100.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(col().key("content").children(rows)),
    );
    if s.show_badge {
        root = root.child(
            el()
                .key("badge")
                .size(20.0, 20.0)
                .fill(Color::WHITE)
                .transition(Transition::opacity().springs(rune_motion::SpringPair { enter: Spring::new(0.6, 0.0), exit: Spring::new(0.6, 0.0) })),
        );
    }
    root
}

#[test]
fn motion_frame_inspects_no_unseen_nodes_when_nothing_is_exiting() {
    let mut rt = Runtime::new(
        ListState { rows: 400, show_badge: true },
        list_view,
        (300.0, 300.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);

    // With hundreds of off-screen rows and nothing exiting, motion frames inspect zero nodes.
    let before = rt.motion.unseen_exit_checks;
    for _ in 0..10 {
        rt.frame(1.0 / 120.0);
    }
    assert_eq!(
        rt.motion.unseen_exit_checks - before,
        0,
        "frames with nothing exiting must not inspect unseen rows"
    );

    // Removing one visible badge inspects only that one exiting node per frame, not the 400 rows.
    rt.update(|s, tx| {
        s.show_badge = false;
        tx.animated();
    });
    rt.frame(1.0 / 120.0);
    let badge_id = rt.tree.nodes.iter().find(|n| n.named("badge")).expect("badge is exiting").id;
    assert!(rt.is_exiting(badge_id));

    let before_exit_frame = rt.motion.unseen_exit_checks;
    rt.frame(1.0 / 120.0);
    assert_eq!(
        rt.motion.unseen_exit_checks - before_exit_frame,
        1,
        "only the single exiting node is inspected, regardless of list length"
    );

    // Once the badge finishes exiting and unmounts, inspection drops back to zero.
    for _ in 0..180 {
        if !rt.scene.contains(badge_id) {
            break;
        }
        rt.frame(1.0 / 120.0);
    }
    assert!(!rt.scene.contains(badge_id), "visible exit still finishes");
    assert!(!rt.tree.nodes.iter().any(|n| n.id == badge_id), "finished exit is pruned from the tree");

    let after_done = rt.motion.unseen_exit_checks;
    for _ in 0..5 {
        rt.frame(1.0 / 120.0);
    }
    assert_eq!(rt.motion.unseen_exit_checks - after_done, 0);
}

#[test]
fn unseen_exit_under_hidden_group_ends_at_once_while_visible_exit_plays_on() {
    struct State {
        show_rows: bool,
        group_opacity: Live<f32>,
    }

    fn view(s: &State, _cx: &mut Cx<State>) -> El {
        let slow_exit = Transition::opacity().springs(rune_motion::SpringPair { enter: Spring::new(1.5, 0.0), exit: Spring::new(1.5, 0.0) });
        let mut group = col().key("group").size(100.0, 40.0).live_opacity(&s.group_opacity);
        if s.show_rows {
            group = group.child(
                col()
                    .key("hidden_row")
                    .size(80.0, 30.0)
                    .fill(Color::WHITE)
                    .transition(slow_exit)
                    .child(el().key("hidden_leaf").size(20.0, 10.0).fill(Color::WHITE)),
            );
        }
        let mut root = col().w_full().h_full().child(group);
        if s.show_rows {
            root = root.child(
                el()
                    .key("visible_row")
                    .size(80.0, 30.0)
                    .fill(Color::WHITE)
                    .transition(slow_exit),
            );
        }
        root
    }

    let mut rt = Runtime::new(
        State { show_rows: true, group_opacity: Live::new(1.0) },
        view,
        (300.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);

    // Start both rows exiting in sight.
    rt.update(|s, tx| {
        s.show_rows = false;
        tx.animated();
    });
    rt.frame(1.0 / 120.0);

    let hidden_id = rt.tree.nodes.iter().find(|n| n.named("hidden_row")).expect("hidden_row exiting").id;
    let leaf_id = rt.tree.nodes.iter().find(|n| n.named("hidden_leaf")).expect("hidden_leaf still mounted").id;
    let visible_id = rt.tree.nodes.iter().find(|n| n.named("visible_row")).expect("visible_row exiting").id;
    assert!(rt.is_exiting(hidden_id));
    assert!(rt.is_exiting(visible_id));

    // Hide the group without a new commit: the next draw records inherited opacity 0 for the
    // exiting subtree, and the following motion frame ends the unseen exit at once.
    rt.state.group_opacity.set(0.0);
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);

    assert!(!rt.scene.contains(hidden_id), "unseen exit unmounts from the scene");
    assert!(!rt.scene.contains(leaf_id), "unseen exit's child unmounts with it");
    assert!(!rt.tree.nodes.iter().any(|n| n.id == hidden_id || n.id == leaf_id), "unseen exit is pruned from the tree");
    assert!(rt.is_exiting(visible_id), "the visible exit keeps playing");

    // The visible exit still finishes on its own schedule.
    for _ in 0..300 {
        if !rt.scene.contains(visible_id) {
            break;
        }
        rt.frame(1.0 / 120.0);
    }
    assert!(!rt.scene.contains(visible_id), "visible exit finishes cleanly");
    assert!(!rt.tree.nodes.iter().any(|n| n.id == visible_id));
}

#[test]
fn exit_scrolled_out_of_clip_ends_at_once_and_unmounts() {
    struct State {
        show_first: bool,
    }

    fn view(s: &State, _cx: &mut Cx<State>) -> El {
        let slow_exit = Transition::opacity().springs(rune_motion::SpringPair { enter: Spring::new(1.5, 0.0), exit: Spring::new(1.5, 0.0) });
        let mut items = Vec::new();
        if s.show_first {
            items.push(
                col()
                    .key("leaving")
                    .w_full()
                    .h(40.0)
                    .fill(Color::WHITE)
                    .transition(slow_exit)
                    .child(el().key("leaving_child").size(10.0, 10.0).fill(Color::WHITE)),
            );
        }
        for i in 0..20 {
            items.push(col().key(format!("r{i}")).w_full().h(40.0).fill(Color::WHITE));
        }
        col().w_full().h_full().child(
            col()
                .key("scroll")
                .w_full()
                .h(100.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(col().key("content").children(items)),
        )
    }

    let mut rt = Runtime::new(State { show_first: true }, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);

    // Start the top row exiting while it is visible in the viewport.
    rt.update(|s, tx| {
        s.show_first = false;
        tx.animated();
    });
    rt.frame(1.0 / 120.0);

    let leaving_id = rt.tree.nodes.iter().find(|n| n.named("leaving")).expect("leaving row is exiting").id;
    let child_id = rt.tree.nodes.iter().find(|n| n.named("leaving_child")).expect("child stays mounted").id;
    assert!(rt.is_exiting(leaving_id));

    // Scroll the exiting row well above the 100 pt viewport clip.
    let scroll_id = rt.tree.nodes.iter().find(|n| n.named("scroll")).unwrap().id;
    rt.scroll.scrolls.get_mut(&scroll_id).unwrap().offset = 300.0;
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);

    assert!(!rt.scene.contains(leaving_id), "clipped-out exit ends and unmounts from the scene");
    assert!(!rt.scene.contains(child_id), "clipped-out exit's child unmounts with it");
    assert!(!rt.tree.nodes.iter().any(|n| n.id == leaving_id || n.id == child_id), "clipped-out exit is pruned from the tree");
}
