//! `El::on_drop_files`: files dragged from another app reach the innermost element under the
//! drop that accepts them, in one call, with a drop hover while the drag is over it.
use super::*;
use crate::{Cx, El, Env, col, el};
use std::path::PathBuf;

#[derive(Default)]
struct Drops {
    /// Each `on_drop_files` call: the target and the files it got.
    dropped: Vec<(&'static str, Vec<String>)>,
    /// Each `on_drop_hover` call.
    hover: Vec<(&'static str, bool)>,
    root_takes: bool,
    /// The inner zone is gone, and a modal that takes drops is open.
    no_inner: bool,
    modal: bool,
}

fn zone(key: &'static str) -> El {
    el().key(key)
        .on_drop_files(move |s: &mut Drops, files, _| s.dropped.push((key, files.iter().map(|f| f.display().to_string()).collect())))
        .on_drop_hover(move |s: &mut Drops, over, _| s.hover.push((key, over)))
}

/// An outer zone that takes anything around an inner one that takes pictures, in a 400 by 300
/// window whose root takes drops when `root_takes`.
fn view(s: &Drops, _: &mut Cx<Drops>) -> El {
    let outer = zone("outer").size(200.0, 200.0);
    let outer = if s.no_inner { outer } else { outer.child(zone("inner").size(100.0, 100.0).accepts_extensions(&["png", ".JPG"])) };
    let mut root = col().w_full().h_full().child(outer);
    if s.modal {
        root = root.child(zone("modal").absolute(100.0, 50.0).size(200.0, 200.0).modal(false, |s: &mut Drops| s.modal = false));
    }
    if s.root_takes { root.on_drop_files(|s: &mut Drops, files, _| s.dropped.push(("root", files.iter().map(|f| f.display().to_string()).collect()))) } else { root }
}

fn run(root_takes: bool) -> Runtime<Drops> {
    let mut rt = Runtime::new(Drops { root_takes, ..Default::default() }, view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn files(names: &[&str]) -> Vec<PathBuf> { names.iter().map(PathBuf::from).collect() }

fn drop_at(rt: &mut Runtime<Drops>, names: &[&str], at: (f32, f32)) {
    rt.drag_files(files(names), Some(at));
    rt.drop_files(files(names), Some(at));
    rt.frame(1.0 / 60.0);
}

fn dropped(rt: &Runtime<Drops>) -> Vec<(&'static str, Vec<&str>)> {
    rt.state.dropped.iter().map(|(k, f)| (*k, f.iter().map(String::as_str).collect())).collect()
}

#[test]
fn the_innermost_accepting_target_takes_the_drop() {
    let mut rt = run(true);
    drop_at(&mut rt, &["a.png"], (50.0, 50.0));
    drop_at(&mut rt, &["b.png"], (150.0, 150.0));
    assert_eq!(dropped(&rt), [("inner", vec!["a.png"]), ("outer", vec!["b.png"])]);
}

#[test]
fn files_a_target_refuses_pass_to_the_next_that_accepts_them() {
    let mut rt = run(true);
    drop_at(&mut rt, &["notes.txt"], (50.0, 50.0));
    assert_eq!(dropped(&rt), [("outer", vec!["notes.txt"])], "the inner zone takes only pictures");
    drop_at(&mut rt, &["c.png"], (300.0, 250.0));
    assert_eq!(dropped(&rt)[1], ("root", vec!["c.png"]), "a drop over no zone falls back to the root");
    rt.drop_files(files(&["d.png"]), None);
    rt.frame(2.0 / 60.0);
    assert_eq!(dropped(&rt)[2], ("root", vec!["d.png"]), "as does a drop with no position");

    let mut rt = run(false);
    drop_at(&mut rt, &["c.png"], (300.0, 250.0));
    assert_eq!(dropped(&rt), [], "with no root handler a drop nothing takes is ignored");
    assert_eq!(rt.state.hover, [], "and shows no hover");
}

#[test]
fn the_drop_hover_follows_the_drag_and_ends_when_it_leaves_or_drops() {
    let mut rt = run(false);
    rt.drag_files(files(&["a.png"]), Some((50.0, 50.0)));
    assert!(rt.dragging_files());
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.hover, [("inner", true)]);
    assert_eq!(rt.drop_hovered().as_deref(), Some("inner"));
    rt.drag_files_to(Some((150.0, 150.0)));
    rt.frame(2.0 / 60.0);
    assert_eq!(rt.state.hover[1..], [("inner", false), ("outer", true)], "only the innermost accepting target is hovered");
    rt.drag_files_leave();
    assert!(!rt.dragging_files());
    rt.frame(3.0 / 60.0);
    assert_eq!(rt.state.hover[3..], [("outer", false)], "leaving the window ends the hover");
    assert_eq!(rt.drop_hovered(), None);

    rt.state.hover.clear();
    rt.drag_files(files(&["notes.txt"]), Some((50.0, 50.0)));
    rt.frame(4.0 / 60.0);
    assert_eq!(rt.state.hover, [("outer", true)], "a target that refuses the files isn't hovered");
    rt.drop_files(files(&["notes.txt"]), Some((50.0, 50.0)));
    rt.frame(5.0 / 60.0);
    assert_eq!(rt.state.hover, [("outer", true), ("outer", false)], "dropping ends the hover");
    assert_eq!(dropped(&rt), [("outer", vec!["notes.txt"])]);
}

#[test]
fn several_files_arrive_in_one_call() {
    let mut rt = run(false);
    drop_at(&mut rt, &["a.png", "b.JPG", "c.txt"], (50.0, 50.0));
    assert_eq!(dropped(&rt), [("inner", vec!["a.png", "b.JPG"])], "the target gets the files it accepts, together");
    drop_at(&mut rt, &["a.png", "c.txt"], (150.0, 150.0));
    assert_eq!(dropped(&rt)[1], ("outer", vec!["a.png", "c.txt"]));
}

#[test]
fn a_covered_window_takes_a_drop_on_the_tree_it_has_now() {
    let mut rt = run(true);
    rt.set_live_visible(false);
    // While covered, the window builds nothing: the inner zone unmounts only in state.
    rt.update(|s, _| s.no_inner = true);
    rt.drag_files(files(&["a.png"]), Some((50.0, 50.0)));
    rt.drop_files(files(&["a.png"]), Some((50.0, 50.0)));
    rt.hidden_wake();
    assert_eq!(dropped(&rt), [("outer", vec!["a.png"])], "not the inner zone, which has gone");
    assert_eq!(rt.state.hover, [("outer", true), ("outer", false)]);

    // Nor the zone behind a modal opened while covered (which, still fading in, takes none).
    rt.update(|s, _| s.modal = true);
    rt.drop_files(files(&["b.png"]), Some((150.0, 150.0)));
    rt.hidden_wake();
    assert_eq!(dropped(&rt).len(), 1);
}

#[test]
fn a_drop_outside_the_window_or_an_open_modal_reaches_nothing() {
    let mut rt = run(true);
    drop_at(&mut rt, &["a.png"], (450.0, 50.0));
    assert_eq!(dropped(&rt), [], "the root takes drops only inside the window");
    rt.update(|s, _| s.modal = true);
    rt.frame(2.0 / 60.0);
    rt.drag_files(files(&["a.png"]), Some((50.0, 50.0)));
    rt.frame(3.0 / 60.0);
    assert_eq!(rt.drop_hovered(), None, "the dimmed window behind a modal takes no drag");
    rt.drop_files(files(&["a.png"]), Some((50.0, 50.0)));
    rt.frame(4.0 / 60.0);
    assert_eq!(dropped(&rt), []);
    drop_at(&mut rt, &["b.png"], (150.0, 150.0));
    assert_eq!(dropped(&rt), [("modal", vec!["b.png"])]);
    assert_eq!(rt.state.hover, [("modal", true), ("modal", false)]);
}

#[test]
fn a_hovered_target_that_unmounts_hears_its_hover_end() {
    let mut rt = run(false);
    rt.drag_files(files(&["a.png"]), Some((50.0, 50.0)));
    rt.frame(1.0 / 60.0);
    rt.update(|s, _| s.no_inner = true);
    rt.frame(2.0 / 60.0);
    // The host samples the pointer while the drag lasts; the target under it has changed.
    assert!(rt.drag_files_to(Some((50.0, 50.0))));
    rt.frame(3.0 / 60.0);
    assert_eq!(rt.state.hover, [("inner", true), ("inner", false), ("outer", true)]);
    assert_eq!(rt.drop_hovered().as_deref(), Some("outer"));
}

#[test]
fn moving_over_one_target_queues_no_frames() {
    let mut rt = run(false);
    rt.drag_files(files(&["a.png"]), Some((50.0, 50.0)));
    rt.frame(1.0 / 60.0);
    for x in 51..90 {
        assert!(!rt.drag_files_to(Some((x as f32, 50.0))), "still over the inner zone");
    }
    assert!(!rt.has_pending_input(), "so the host draws nothing");
    assert!(rt.drag_files_to(Some((150.0, 150.0))), "onto the outer zone");
    assert!(rt.has_pending_input());
}

#[test]
fn a_drop_zone_over_other_elements_leaves_their_menus_and_help() {
    #[derive(Default)]
    struct S { menus: u32 }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        el().w_full().h_full()
            .child(el().key("row").size(100.0, 40.0).help("Row").on_secondary_press(|s: &mut S, _, _| s.menus += 1))
            .child(el().key("zone").absolute(0.0, 0.0).w_full().h_full().on_drop_files(|_: &mut S, _, _| {}))
    }
    let mut rt = Runtime::new(S::default(), view, (300.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.secondary_down(10.0, 10.0);
    rt.secondary_up(10.0, 10.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.menus, 1, "the row's context menu");
    rt.observe_pointer(10.0, 10.0);
    rt.frame(2.0 / 60.0);
    rt.frame(rt.help_delay);
    assert_eq!(rt.help_tooltip().map(|(t, _)| t), Some("Row"));
}

#[test]
fn a_covered_window_still_takes_a_drop() {
    let mut rt = run(false);
    // As the host does for `WindowEvent::Occluded(true)`, then for a drop (`covered_step`).
    rt.set_live_visible(false);
    rt.drag_files(files(&["a.png"]), Some((50.0, 50.0)));
    rt.drop_files(files(&["a.png"]), Some((50.0, 50.0)));
    assert!(rt.has_pending_input());
    rt.hidden_wake();
    assert_eq!(dropped(&rt), [("inner", vec!["a.png"])]);
    assert_eq!(rt.state.hover, [("inner", true), ("inner", false)]);
}

#[derive(Default)]
struct Picky {
    dropped: Vec<(&'static str, Vec<String>)>,
    rejected: Vec<Vec<String>>,
    hover: Vec<(&'static str, bool)>,
}

fn names(files: &[PathBuf]) -> Vec<String> { files.iter().map(|f| f.display().to_string()).collect() }

/// A zone that takes only exports, and says why it refuses anything else, inside one that takes
/// anything.
fn picky(_: &Picky, _: &mut Cx<Picky>) -> El {
    let exports = el().key("exports").size(100.0, 100.0)
        .on_drop_files(|s: &mut Picky, files, _| s.dropped.push(("exports", names(files))))
        .accepts_extensions(&["json"])
        .on_drop_hover(|s: &mut Picky, over, _| s.hover.push(("exports", over)))
        .on_drop_rejected(|s: &mut Picky, files, _| s.rejected.push(names(files)))
        .on_drop_rejected_hover(|s: &mut Picky, over, _| s.hover.push(("exports-rejected", over)));
    col().w_full().h_full().child(
        el().key("outer").size(200.0, 200.0)
            .on_drop_files(|s: &mut Picky, files, _| s.dropped.push(("outer", names(files))))
            .on_drop_hover(|s: &mut Picky, over, _| s.hover.push(("outer", over)))
            .child(exports),
    )
}

#[test]
fn a_target_that_hears_rejected_drops_stops_them_and_shows_its_own_hover() {
    let mut rt = Runtime::new(Picky::default(), picky, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.drag_files(files(&["notes.txt"]), Some((50.0, 50.0)));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.hover, [("exports-rejected", true)], "not the outer zone's hover");
    assert_eq!((rt.drop_rejected_hovered().as_deref(), rt.drop_hovered()), (Some("exports"), None));
    rt.drag_files_to(Some((150.0, 150.0)));
    rt.frame(2.0 / 60.0);
    assert_eq!(rt.state.hover[1..], [("exports-rejected", false), ("outer", true)]);
    assert_eq!((rt.drop_rejected_hovered(), rt.drop_hovered().as_deref()), (None, Some("outer")));
    rt.drag_files_to(Some((50.0, 50.0)));
    rt.drop_files(files(&["notes.txt"]), Some((50.0, 50.0)));
    rt.frame(3.0 / 60.0);
    assert_eq!(rt.state.hover[3..], [("outer", false), ("exports-rejected", true), ("exports-rejected", false)], "dropping ends it");
    assert_eq!((rt.state.rejected.as_slice(), rt.state.dropped.as_slice()), (&[vec!["notes.txt".to_owned()]][..], &[][..]));

    rt.state.hover.clear();
    rt.drag_files(files(&["a.json", "b.txt"]), Some((50.0, 50.0)));
    rt.frame(4.0 / 60.0);
    assert_eq!(rt.state.hover, [("exports", true)], "a drag carrying an export is accepted");
    rt.drop_files(files(&["a.json", "b.txt"]), Some((50.0, 50.0)));
    rt.frame(5.0 / 60.0);
    assert_eq!(rt.state.dropped, [("exports", vec!["a.json".to_owned()])]);
    assert_eq!(rt.state.rejected[1..], [vec!["b.txt".to_owned()]], "and the rest are rejected");

    rt.drag_files(files(&["a.json"]), Some((50.0, 50.0)));
    rt.frame(6.0 / 60.0);
    rt.drag_files_leave();
    rt.drag_files(files(&["c.txt"]), Some((50.0, 50.0)));
    rt.frame(7.0 / 60.0);
    assert_eq!(rt.state.hover[2..], [("exports", true), ("exports", false), ("exports-rejected", true)], "the same target swaps hovers");

    rt.drag_files_leave();
    rt.frame(8.0 / 60.0);
    rt.state.hover.clear();
    rt.drag_files(Vec::new(), Some((50.0, 50.0)));
    rt.drop_files(Vec::new(), Some((50.0, 50.0)));
    rt.frame(9.0 / 60.0);
    assert!(rt.state.hover.is_empty() && rt.drop_rejected_hovered().is_none(), "an empty drag claims no rejected hover");
}

#[test]
fn a_target_that_follows_the_drag_hears_its_point_on_the_element_and_where_it_dropped() {
    #[derive(Default)]
    struct Caret { seen: Vec<(&'static str, Option<(f32, f32)>)> }
    fn view(_: &Caret, _: &mut Cx<Caret>) -> El {
        let local = |tx: &crate::Tx| tx.drop_point().map(|p| (p.x, p.y));
        col().w_full().h_full().children([
            el().size(200.0, 20.0),
            el().key("doc").size(200.0, 100.0)
                .on_drop_files(move |s: &mut Caret, _, tx| s.seen.push(("drop", local(tx))))
                .on_drop_hover(move |s: &mut Caret, over, tx| s.seen.push((if over { "over" } else { "off" }, local(tx))))
                .on_drop_move(|s: &mut Caret, p, tx| { assert_eq!(tx.drop_point(), Some(p)); s.seen.push(("move", Some((p.x, p.y)))) }),
            el().key("plain").size(200.0, 100.0).on_drop_files(|_: &mut Caret, _, _| {}),
        ])
    }
    let mut rt = Runtime::new(Caret::default(), view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let take = |rt: &mut Runtime<Caret>| std::mem::take(&mut rt.state.seen);
    rt.drag_files(files(&["a.txt"]), Some((30.0, 40.0)));
    rt.frame(1.0 / 60.0);
    assert_eq!(take(&mut rt), [("over", Some((30.0, 20.0))), ("move", Some((30.0, 20.0)))], "points on the element, below its 20 pt neighbour");

    assert!(!rt.drag_files_to(Some((30.0, 40.0))), "a drag that didn't move asks for nothing");
    assert!(rt.drag_files_to(Some((50.0, 60.0))), "a move over a target that follows it asks for a frame");
    assert!(!rt.drag_files_to(Some((60.0, 70.0))), "once a frame");
    rt.frame(2.0 / 60.0);
    assert_eq!(take(&mut rt), [("move", Some((60.0, 50.0)))], "at the latest point");

    assert!(rt.drag_files_to(Some((60.0, 150.0))), "onto another target");
    rt.frame(3.0 / 60.0);
    assert_eq!(take(&mut rt), [("off", None)]);
    assert!(!rt.drag_files_to(Some((70.0, 160.0))), "a target that doesn't follow the drag asks for no frames as it moves");

    rt.drag_files(files(&["a.txt"]), Some((70.0, 80.0)));
    rt.drop_files(files(&["a.txt"]), Some((70.0, 80.0)));
    rt.frame(4.0 / 60.0);
    assert_eq!(take(&mut rt), [("over", Some((70.0, 60.0))), ("move", Some((70.0, 60.0))), ("off", None), ("drop", Some((70.0, 60.0)))]);
}

#[test]
fn a_drag_that_left_or_dropped_carries_nothing_into_the_next_sample() {
    let mut rt = run(false);
    rt.drag_files(files(&["a.png"]), Some((50.0, 50.0)));
    rt.drag_files_leave();
    assert!(!rt.drag_files_to(Some((150.0, 150.0))), "no drag: a sample queues nothing");
    rt.frame(1.0 / 60.0);
    drop_at(&mut rt, &["a.png"], (50.0, 50.0));
    assert!(!rt.dragging_files() && !rt.drag_files_to(Some((150.0, 150.0))));
    rt.state.hover.clear();
    rt.drag_files(files(&["notes.txt"]), Some((50.0, 50.0)));
    rt.frame(3.0 / 60.0);
    assert_eq!(rt.state.hover, [("outer", true)], "the next drag starts from its own files, not the last one's");
}

#[test]
fn held_file_steps_survive_headless_timer_substeps_below_60_fps() {
    use std::time::Duration;

    fn poll_1s(_: &mut Drops, _: f64, tx: &mut Tx) -> bool {
        tx.wake_after(Duration::from_secs(1));
        true
    }
    fn poll_16ms(_: &mut Drops, _: f64, tx: &mut Tx) -> bool {
        tx.wake_after(Duration::from_millis(16));
        true
    }

    for (label, poll) in [("1s", poll_1s as fn(&mut Drops, f64, &mut Tx) -> bool), ("16ms", poll_16ms)] {
        for fps in [30.0_f64, 45.0] {
            let dt = 1.0 / fps;
            let mut rt = Runtime::new(Drops { root_takes: true, ..Default::default() }, view, (400.0, 300.0), Env::new(true, 1.0));
            rt.poll = Some(poll);
            rt.frame(0.0);

            let early = (0.4 * fps).round() as usize;
            let late = (1.6 * fps).round() as usize;
            let total = (2.0 * fps).round() as usize;
            for k in 1..=total {
                if k == early {
                    rt.drag_files(files(&["early.png"]), Some((50.0, 50.0)));
                    rt.drop_files(files(&["early.png"]), Some((50.0, 50.0)));
                } else if k == late {
                    rt.drag_files(files(&["late.png"]), Some((50.0, 50.0)));
                    rt.drop_files(files(&["late.png"]), Some((50.0, 50.0)));
                }
                rt.frame(dt);
            }

            assert_eq!(
                dropped(&rt),
                [("inner", vec!["early.png"]), ("inner", vec!["late.png"])],
                "poll={label} fps={fps}: drops before and after 1s must survive timer sub-steps"
            );
            assert_eq!(
                rt.state.hover,
                [("inner", true), ("inner", false), ("inner", true), ("inner", false)],
                "poll={label} fps={fps}: hover must pair with each drop"
            );
            assert!(!rt.pointer.files_held(), "poll={label} fps={fps}: waited flag must clear once delivered");
        }
    }
}

#[test]
fn has_drop_targets_tracks_mount_unmount_exiting_modal_root_and_rejects() {
    #[derive(Default)]
    struct S {
        child_drop: bool,
        child_exit: bool,
        root_drop: bool,
        reject_only: bool,
        hover_only: bool,
        modal: bool,
        modal_drop: bool,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = el().size(300.0, 200.0);
        if s.root_drop {
            root = root.on_drop_files(|_: &mut S, _, _| {});
        }
        if s.child_drop {
            let mut child = el().key("drop").size(100.0, 100.0).on_drop_files(|_: &mut S, _, _| {});
            if s.child_exit {
                child = child.transition(crate::Transition::opacity());
            }
            root = root.child(child);
        }
        if s.reject_only {
            root = root.child(el().key("rej").size(50.0, 50.0).on_drop_rejected(|_: &mut S, _, _| {}));
        }
        if s.hover_only {
            root = root.child(el().key("hov").size(50.0, 50.0).on_drop_hover(|_: &mut S, _, _| {}));
        }
        if s.modal {
            let mut m = el().key("modal").size(120.0, 80.0).modal(false, |s: &mut S| s.modal = false);
            if s.modal_drop {
                m = m.on_drop_files(|_: &mut S, _, _| {});
            }
            root = root.child(m);
        }
        root
    }

    let mut rt = Runtime::new(S::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(!rt.core().has_drop_targets(), "empty tree has no drop targets");

    // Hover-only without on_drop_files or reject handlers does not count.
    rt.update(|s, _| s.hover_only = true);
    rt.frame(1.0 / 60.0);
    assert!(!rt.core().has_drop_targets());

    // Mount and unmount child on_drop_files.
    rt.update(|s, _| { s.hover_only = false; s.child_drop = true; s.child_exit = true; });
    rt.frame(1.0 / 60.0);
    assert!(rt.core().has_drop_targets());

    // Unmounting an exiting node keeps it in the tree while exiting, but it must not count.
    rt.update(|s, _| s.child_drop = false);
    rt.frame(1.0 / 60.0);
    assert!(rt.tree.nodes.iter().any(|n| n.key.as_deref() == Some("drop")), "exiting node still present");
    assert!(!rt.core().has_drop_targets(), "exiting drop target must not count");

    // Modal without drop targets hides a background drop target.
    rt.update(|s, _| { s.child_drop = true; s.child_exit = false; s.modal = true; });
    rt.frame(1.0 / 60.0);
    assert!(!rt.core().has_drop_targets(), "modal without drop targets hides background target");

    // Modal with its own drop target counts.
    rt.update(|s, _| s.modal_drop = true);
    rt.frame(1.0 / 60.0);
    assert!(rt.core().has_drop_targets());

    // Root drop target counts.
    rt.update(|s, _| { s.modal = false; s.modal_drop = false; s.child_drop = false; s.root_drop = true; });
    rt.frame(1.0 / 60.0);
    assert!(rt.core().has_drop_targets());

    // on_drop_rejected-only target counts.
    rt.update(|s, _| { s.root_drop = false; s.reject_only = true; });
    rt.frame(1.0 / 60.0);
    assert!(rt.core().has_drop_targets());
}

#[test]
fn accepts_drag_reads_state_and_whole_drag_slice() {
    struct S {
        read_only: bool,
        self_dir: PathBuf,
        hover: Vec<(&'static str, bool)>,
        dropped: Vec<(&'static str, Vec<String>)>,
        rejected: Vec<Vec<String>>,
        use_reject: bool,
        png_only: bool,
    }
    impl Default for S {
        fn default() -> Self {
            Self {
                read_only: true,
                self_dir: PathBuf::from("/dir/folder"),
                hover: Vec::new(),
                dropped: Vec::new(),
                rejected: Vec::new(),
                use_reject: false,
                png_only: false,
            }
        }
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut folder = el().key("folder").size(150.0, 150.0)
            .on_drop_files(|s: &mut S, files, _| s.dropped.push(("folder", names(files))))
            .on_drop_hover(|s: &mut S, over, _| s.hover.push(("folder", over)))
            .accepts_drag(|s: &S, paths| !s.read_only && paths.len() <= 2 && !paths.iter().any(|p| p == &s.self_dir));
        if s.png_only {
            folder = folder.accepts_extensions(&["png"]);
        }
        if s.use_reject {
            folder = folder
                .on_drop_rejected(|s: &mut S, files, _| s.rejected.push(names(files)))
                .on_drop_rejected_hover(|s: &mut S, over, _| s.hover.push(("folder-rejected", over)));
        }
        el().key("outer").size(300.0, 200.0)
            .on_drop_files(|s: &mut S, files, _| s.dropped.push(("outer", names(files))))
            .on_drop_hover(|s: &mut S, over, _| s.hover.push(("outer", over)))
            .child(folder)
    }

    let mut rt = Runtime::new(S::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // read_only == true: folder refuses the drag and passes it to outer.
    rt.drag_files(files(&["a.txt", "b.png"]), Some((50.0, 50.0)));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.hover, [("outer", true)]);
    assert!(!rt.drag_files_to(Some((50.0, 50.0))), "stationary drag without state change queues nothing");
    rt.drop_files(files(&["a.txt", "b.png"]), Some((50.0, 50.0)));
    rt.frame(2.0 / 60.0);
    assert_eq!(rt.state.dropped, [("outer", vec!["a.txt".into(), "b.png".into()])]);

    // read_only == false: folder takes up to 2 files, not containing self_dir.
    rt.update(|s, _| { s.read_only = false; s.hover.clear(); s.dropped.clear(); });
    rt.frame(3.0 / 60.0);
    rt.drag_files(files(&["a.txt", "b.png"]), Some((50.0, 50.0)));
    rt.frame(4.0 / 60.0);
    assert_eq!(rt.state.hover, [("folder", true)]);
    assert_eq!(rt.drop_hovered().as_deref(), Some("folder"));

    // State changes mid-drag while pointer stays stationary: next poll rechecks accepts_drag.
    rt.update(|s, _| s.read_only = true);
    rt.frame(5.0 / 60.0);
    assert!(rt.drag_files_to(Some((50.0, 50.0))), "commit under accepts_drag triggers recheck");
    rt.frame(6.0 / 60.0);
    assert_eq!(rt.state.hover[1..], [("folder", false), ("outer", true)]);
    assert!(!rt.drag_files_to(Some((50.0, 50.0))), "once rechecked, stationary drag queues nothing");
    rt.drag_files_leave();
    rt.frame(7.0 / 60.0);

    // Whole-drag checks: >2 files or dragging self_dir onto itself are refused.
    rt.update(|s, _| { s.read_only = false; s.hover.clear(); s.dropped.clear(); });
    rt.frame(8.0 / 60.0);
    rt.drag_files(files(&["a.txt", "b.txt", "c.txt"]), Some((50.0, 50.0)));
    rt.drop_files(files(&["a.txt", "b.txt", "c.txt"]), Some((50.0, 50.0)));
    rt.frame(9.0 / 60.0);
    assert_eq!(rt.state.dropped, [("outer", vec!["a.txt".into(), "b.txt".into(), "c.txt".into()])]);

    rt.drag_files(files(&["/dir/folder", "a.txt"]), Some((50.0, 50.0)));
    rt.drop_files(files(&["/dir/folder", "a.txt"]), Some((50.0, 50.0)));
    rt.frame(10.0 / 60.0);
    assert_eq!(rt.state.dropped[1], ("outer", vec!["/dir/folder".into(), "a.txt".into()]));

    // Composing accepts_drag with accepts_extensions + on_drop_rejected(_hover).
    rt.update(|s, _| { s.use_reject = true; s.png_only = true; s.read_only = true; s.hover.clear(); s.dropped.clear(); s.rejected.clear(); });
    rt.frame(11.0 / 60.0);
    rt.drag_files(files(&["a.png"]), Some((50.0, 50.0)));
    rt.frame(12.0 / 60.0);
    assert_eq!(rt.state.hover, [("folder-rejected", true)], "read_only rejects even a png");
    rt.drop_files(files(&["a.png"]), Some((50.0, 50.0)));
    rt.frame(13.0 / 60.0);
    assert_eq!(rt.state.dropped, []);
    assert_eq!(rt.state.rejected, [vec!["a.png".to_owned()]]);

    rt.update(|s, _| { s.read_only = false; s.hover.clear(); s.dropped.clear(); s.rejected.clear(); });
    rt.frame(14.0 / 60.0);
    rt.drag_files(files(&["a.png", "b.txt"]), Some((50.0, 50.0)));
    rt.drop_files(files(&["a.png", "b.txt"]), Some((50.0, 50.0)));
    rt.frame(15.0 / 60.0);
    assert_eq!(rt.state.dropped, [("folder", vec!["a.png".into()])]);
    assert_eq!(rt.state.rejected, [vec!["b.txt".to_owned()]]);
}

#[test]
fn drag_paths_is_populated_across_drop_callbacks_and_none_elsewhere() {
    assert_eq!(Tx::default().drag_paths(), None);

    #[derive(Default)]
    struct Seen {
        log: Vec<(&'static str, Option<Vec<String>>)>,
    }
    fn snap(tx: &Tx) -> Option<Vec<String>> { tx.drag_paths().map(names) }
    fn view(_: &Seen, _: &mut Cx<Seen>) -> El {
        el().key("wrap").size(300.0, 200.0)
            .on_drop_hover_within(|s: &mut Seen, over, tx| s.log.push((if over { "within-on" } else { "within-off" }, snap(tx))))
            .on_drop_move_within(|s: &mut Seen, _, tx| s.log.push(("within-move", snap(tx))))
            .child(
                el().key("zone").size(150.0, 150.0)
                    .on_press_tx(|s: &mut Seen, tx| s.log.push(("press", snap(tx))))
                    .on_drop_files(|s: &mut Seen, _, tx| s.log.push(("drop", snap(tx))))
                    .accepts_extensions(&["png"])
                    .on_drop_hover(|s: &mut Seen, over, tx| s.log.push((if over { "hover-on" } else { "hover-off" }, snap(tx))))
                    .on_drop_move(|s: &mut Seen, _, tx| s.log.push(("move", snap(tx))))
                    .on_drop_rejected(|s: &mut Seen, _, tx| s.log.push(("reject", snap(tx))))
                    .on_drop_rejected_hover(|s: &mut Seen, over, tx| s.log.push((if over { "reject-on" } else { "reject-off" }, snap(tx)))),
            )
    }

    let mut rt = Runtime::new(Seen::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Non-drop handler sees None.
    rt.pointer_down(20.0, 20.0);
    rt.pointer_up(20.0, 20.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(std::mem::take(&mut rt.state.log), [("press", None)]);

    // Mixed drag: hover(true), move, within-hover(true), within-move, drop and reject all see the full slice;
    // hover(false) and within-hover(false) see None.
    let mixed = vec!["a.png".to_owned(), "b.txt".to_owned()];
    rt.drag_files(files(&["a.png", "b.txt"]), Some((40.0, 40.0)));
    rt.frame(2.0 / 60.0);
    assert_eq!(
        std::mem::take(&mut rt.state.log),
        [
            ("hover-on", Some(mixed.clone())),
            ("within-on", Some(mixed.clone())),
            ("move", Some(mixed.clone())),
            ("within-move", Some(mixed.clone())),
        ]
    );
    rt.drop_files(files(&["a.png", "b.txt"]), Some((40.0, 40.0)));
    rt.frame(3.0 / 60.0);
    assert_eq!(
        std::mem::take(&mut rt.state.log),
        [
            ("hover-off", None),
            ("within-off", None),
            ("drop", Some(mixed.clone())),
            ("reject", Some(mixed)),
        ]
    );

    // Rejected-only drag populates drag_paths in on_drop_rejected_hover(true) and clears on leave.
    let txt = vec!["notes.txt".to_owned()];
    rt.drag_files(files(&["notes.txt"]), Some((40.0, 40.0)));
    rt.frame(4.0 / 60.0);
    assert_eq!(
        std::mem::take(&mut rt.state.log),
        [
            ("reject-on", Some(txt.clone())),
            ("within-on", Some(txt.clone())),
            ("within-move", Some(txt)),
        ]
    );
    rt.drag_files_leave();
    rt.frame(5.0 / 60.0);
    assert_eq!(std::mem::take(&mut rt.state.log), [("reject-off", None), ("within-off", None)]);
}

#[test]
fn within_observers_follow_drag_across_child_targets_without_stealing_drop() {
    #[derive(Default)]
    struct S {
        log: Vec<String>,
        hide_strip: bool,
        inert_strip: bool,
        modal: bool,
    }
    fn col_zone(key: &'static str) -> El {
        el().key(key).size(150.0, 150.0)
            .on_drop_hover(move |s: &mut S, over, _| s.log.push(format!("{key} hover {over}")))
            .on_drop_files(move |s: &mut S, _, _| s.log.push(format!("{key} drop")))
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut strip = crate::row().key("strip").size(360.0, 150.0)
            .on_drop_hover(|s: &mut S, over, _| s.log.push(format!("strip hover {over}")))
            .on_drop_files(|s: &mut S, _, _| s.log.push("strip drop".into()))
            .on_drop_hover_within(|s: &mut S, over, tx| {
                let pt = tx.drop_point().map(|p| (p.x, p.y));
                s.log.push(format!("strip within {over} {pt:?}"));
            })
            .on_drop_move_within(|s: &mut S, p, tx| {
                assert_eq!(tx.drop_point(), Some(p));
                s.log.push(format!("strip move ({},{})", p.x, p.y));
            })
            .child(col_zone("column-a"))
            .child(col_zone("column-b"));
        if s.inert_strip {
            strip = strip.inert();
        }
        let mut outer = crate::col().key("outer").px(20.0).py(10.0).w_full().h_full()
            .on_drop_hover_within(|s: &mut S, over, _| s.log.push(format!("outer within {over}")))
            .on_drop_move_within(|s: &mut S, p, _| s.log.push(format!("outer move ({},{})", p.x, p.y)));
        if !s.hide_strip {
            outer = outer.child(strip);
        }
        if s.modal {
            outer = outer.child(el().key("modal").absolute(0.0, 0.0).size(80.0, 80.0).modal(false, |s: &mut S| s.modal = false));
        }
        outer
    }

    let mut rt = Runtime::new(S::default(), view, (420.0, 220.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Drag crosses column-a (x=50 -> strip local x=30), column-b (x=200 -> strip local x=180),
    // then strip's own space (x=350 -> strip local x=330).
    let p = files(&["a.txt"]);
    rt.drag_files(p.clone(), Some((50.0, 50.0)));
    rt.frame(1.0 / 60.0);
    assert_eq!(
        std::mem::take(&mut rt.state.log),
        [
            "column-a hover true",
            "strip within true Some((30.0, 40.0))",
            "outer within true",
            "strip move (30,40)",
            "outer move (50,50)",
        ],
        "innermost ancestor fires first, with each ancestor's own local points"
    );

    assert!(rt.drag_files_to(Some((200.0, 50.0))));
    rt.frame(2.0 / 60.0);
    assert_eq!(
        std::mem::take(&mut rt.state.log),
        [
            "column-a hover false",
            "column-b hover true",
            "strip move (180,40)",
            "outer move (200,50)",
        ],
        "strip stays within-hovered across sibling columns without flickering"
    );

    assert!(rt.drag_files_to(Some((350.0, 50.0))));
    rt.frame(3.0 / 60.0);
    assert_eq!(
        std::mem::take(&mut rt.state.log),
        [
            "column-b hover false",
            "strip hover true",
            "strip move (330,40)",
            "outer move (350,50)",
        ]
    );

    // Move back onto column-a and drop: column-a takes the drop, not strip.
    assert!(rt.drag_files_to(Some((50.0, 50.0))));
    rt.frame(4.0 / 60.0);
    rt.state.log.clear();
    rt.drop_files(p.clone(), Some((50.0, 50.0)));
    rt.frame(5.0 / 60.0);
    assert_eq!(
        std::mem::take(&mut rt.state.log),
        [
            "column-a hover false",
            "strip within false None",
            "outer within false",
            "column-a drop",
        ]
    );

    // Empty drag does not trigger within observers.
    rt.drag_files(Vec::new(), Some((50.0, 50.0)));
    rt.frame(6.0 / 60.0);
    assert!(rt.state.log.is_empty());

    // Unmounting a within-hovered ancestor delivers false on the next sample.
    rt.drag_files(p.clone(), Some((50.0, 50.0)));
    rt.frame(7.0 / 60.0);
    rt.state.log.clear();
    rt.update(|s, _| s.hide_strip = true);
    rt.frame(8.0 / 60.0);
    assert!(rt.drag_files_to(Some((50.0, 50.0))));
    rt.frame(9.0 / 60.0);
    assert!(rt.state.log.iter().any(|l| l == "strip within false None"), "unmounted ancestor hears false: {:?}", rt.state.log);
    rt.drag_files_leave();
    rt.frame(10.0 / 60.0);

    // Inert ancestor and modal-blocked ancestor hear nothing.
    rt.update(|s, _| { s.hide_strip = false; s.inert_strip = true; s.log.clear(); });
    rt.frame(11.0 / 60.0);
    rt.drag_files(p.clone(), Some((50.0, 50.0)));
    rt.frame(12.0 / 60.0);
    assert!(!rt.state.log.iter().any(|l| l.starts_with("strip")), "inert strip hears nothing: {:?}", rt.state.log);
    rt.drag_files_leave();
    rt.frame(13.0 / 60.0);

    rt.update(|s, _| { s.inert_strip = false; s.modal = true; s.log.clear(); });
    rt.frame(14.0 / 60.0);
    rt.drag_files(p, Some((200.0, 50.0)));
    rt.frame(15.0 / 60.0);
    assert!(rt.state.log.is_empty(), "drag outside modal reaches no background within observer");
}

#[test]
fn within_observers_wake_frames_only_when_needed_and_count_as_drop_targets() {
    #[derive(Default)]
    struct S {
        with_move: bool,
        within_only: bool,
        dropped: usize,
        moves: usize,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        if s.within_only {
            return el().key("obs").size(200.0, 200.0)
                .on_drop_hover_within(|_: &mut S, _, _| {});
        }
        let mut box_a = el().key("box-a").size(150.0, 150.0)
            .on_drop_hover_within(|_: &mut S, _, _| {});
        if s.with_move {
            box_a = box_a.on_drop_move_within(|s: &mut S, _, _| s.moves += 1);
        }
        el().key("root").size(300.0, 200.0)
            .on_drop_files(|s: &mut S, files, _| s.dropped += files.len())
            .child(box_a)
    }

    let mut rt = Runtime::new(S::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Passive child observer with on_drop_hover_within only: moving inside queues no frames,
    // and dropping inside still reaches parent's on_drop_files.
    rt.drag_files(files(&["a.txt"]), Some((20.0, 20.0)));
    rt.frame(1.0 / 60.0);
    for x in 25..100 {
        assert!(!rt.drag_files_to(Some((x as f32, 20.0))), "hover_within without move_within wakes no frames");
    }
    assert!(rt.drag_files_to(Some((200.0, 20.0))), "leaving box-a changes within_hovered set");
    rt.frame(2.0 / 60.0);
    rt.drop_files(files(&["a.txt"]), Some((20.0, 20.0)));
    rt.frame(3.0 / 60.0);
    assert_eq!(rt.state.dropped, 1, "passive child observer does not steal parent's drop");

    // With on_drop_move_within: stationary queues nothing, moves coalesce to once a frame, outside queues nothing.
    rt.update(|s, _| s.with_move = true);
    rt.frame(4.0 / 60.0);
    rt.drag_files(files(&["a.txt"]), Some((20.0, 20.0)));
    rt.frame(5.0 / 60.0);
    assert_eq!(rt.state.moves, 1);
    assert!(!rt.drag_files_to(Some((20.0, 20.0))), "stationary move_within queues nothing");
    assert!(rt.drag_files_to(Some((30.0, 20.0))), "first move wakes one frame");
    assert!(!rt.drag_files_to(Some((40.0, 20.0))), "coalesced to once a frame");
    rt.frame(6.0 / 60.0);
    assert_eq!(rt.state.moves, 2);
    assert!(rt.drag_files_to(Some((200.0, 20.0))), "leaving box-a");
    rt.frame(7.0 / 60.0);
    assert!(!rt.drag_files_to(Some((220.0, 20.0))), "outside box-a wakes no frames");
    rt.drag_files_leave();
    rt.frame(8.0 / 60.0);

    // A tree with only on_drop_hover_within still counts for has_drop_targets (Wayland lazy binding).
    rt.update(|s, _| s.within_only = true);
    rt.frame(9.0 / 60.0);
    assert!(rt.core().has_drop_targets());
}
