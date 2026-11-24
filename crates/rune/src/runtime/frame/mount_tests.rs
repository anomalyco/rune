use std::time::Duration;

use super::*;
use crate::color::Color;
use crate::{Live, Pick, col, el, text};

fn element_text<S: 'static>(rt: &Runtime<S>, key: &str) -> Option<String> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).and_then(|e| e.text)
}

fn element_fill<S: 'static>(rt: &Runtime<S>, key: &str) -> Option<Color> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).map(|e| e.fill)
}

#[test]
fn first_frame_mount_renders_state_change_in_same_frame() {
    #[derive(Default)]
    struct S {
        shown: bool,
        mounts: usize,
    }
    let view = |s: &S, _: &mut Cx<S>| {
        col().size(200.0, 80.0).child(
            el().key("tip")
                .size(120.0, 40.0)
                .fill(if s.shown { Color::WHITE } else { Color::BLACK })
                .on_mount(|s: &mut S, _| {
                    s.shown = true;
                    s.mounts += 1;
                })
                .child(text(if s.shown { "shown" } else { "pending" }).key("tip-label")),
        )
    };
    let mut rt = Runtime::new(S::default(), view, (200.0, 80.0), Env::default());
    rt.frame(0.0);

    assert_eq!(rt.state.mounts, 1, "on_mount fired on the first frame");
    assert!(rt.state.shown);
    assert_eq!(element_text(&rt, "tip-label").as_deref(), Some("shown"), "text reflects mount update in frame 0");
    assert_eq!(element_fill(&rt, "tip"), Some(Color::WHITE), "fill reflects mount update in frame 0 without spring lag");
    assert!(rt.accessibility_json().to_string().contains("shown"), "a11y tree reflects mount update in frame 0");

    let commits_after_first = rt.stats.commits;
    let animating = rt.frame(1.0 / 60.0);
    assert!(!animating, "idle after first-frame mount");
    assert_eq!(rt.state.mounts, 1, "no re-fire on idle frame");
    assert_eq!(rt.stats.commits, commits_after_first, "no extra commit on idle frame");
    assert_eq!(rt.wake_deadline(), None, "no wake scheduled while idle");
}

#[test]
fn click_triggered_mount_updates_drawn_tree_in_same_frame() {
    #[derive(Default)]
    struct S {
        show_tip: bool,
        log: Vec<&'static str>,
    }
    let view = |s: &S, _: &mut Cx<S>| {
        col().size(240.0, 120.0)
            .child(
                el().key("toggle").size(100.0, 40.0)
                    .on_press(|s: &mut S| s.show_tip = true),
            )
            .children(s.show_tip.then(|| {
                let label = if s.log.is_empty() { "lagging" } else { "shown" };
                col().key("tip").size(120.0, 40.0)
                    .fill(if s.log.is_empty() { Color::BLACK } else { Color::WHITE })
                    .on_mount(|s: &mut S, _| s.log.push("shown"))
                    .child(text(label).key("tip-text"))
            }))
    };
    let mut rt = Runtime::new(S::default(), view, (240.0, 120.0), Env::default());
    rt.frame(0.0);
    assert!(rt.state.log.is_empty());

    let (x, y) = rt.find("toggle").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);

    assert_eq!(rt.state.log, vec!["shown"], "mount handler ran in the click frame");
    assert_eq!(element_text(&rt, "tip-text").as_deref(), Some("shown"), "same-frame drawn tree shows post-mount text");
    assert_eq!(element_fill(&rt, "tip"), Some(Color::WHITE), "same-frame drawn tree shows post-mount fill");
    assert!(rt.accessibility_json().to_string().contains("shown"), "same-frame a11y tree shows post-mount text");

    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.log, vec!["shown"], "no second fire on next frame");
}

#[test]
fn rebuild_resize_and_move_do_not_refire_on_mount() {
    struct S {
        counter: usize,
        offset_x: f32,
        mounts: usize,
    }
    let view = |s: &S, _: &mut Cx<S>| {
        col().size(300.0, 200.0).pl(s.offset_x).child(
            col().key("card").size(120.0, 60.0)
                .on_mount(|s: &mut S, _| s.mounts += 1)
                .child(text(format!("c={}", s.counter)).key("lbl")),
        )
    };
    let mut rt = Runtime::new(S { counter: 0, offset_x: 0.0, mounts: 0 }, view, (300.0, 200.0), Env::default());
    rt.frame(0.0);
    assert_eq!(rt.state.mounts, 1);

    // Rebuild with changed state.
    rt.update(|s, _| s.counter += 1);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.mounts, 1, "rebuild must not re-fire on_mount");

    // Move element in layout.
    rt.update(|s, _| s.offset_x = 40.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.mounts, 1, "layout move must not re-fire on_mount");

    // Resize viewport.
    rt.set_viewport((480.0, 320.0), 1.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.mounts, 1, "viewport resize must not re-fire on_mount");

    // Display scale change.
    rt.set_viewport((480.0, 320.0), 2.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.mounts, 1, "scale change must not re-fire on_mount");
}

#[test]
fn unmount_and_remount_refires_both_instant_and_unkeyed() {
    #[derive(Default)]
    struct S {
        show: bool,
        keyed_mounts: usize,
        unkeyed_mounts: usize,
    }
    let view = |s: &S, _: &mut Cx<S>| {
        col().size(200.0, 120.0).children(s.show.then(|| {
            col().key("wrap").size(160.0, 80.0)
                .on_mount(|s: &mut S, _| s.keyed_mounts += 1)
                .child(el().size(40.0, 20.0).on_mount(|s: &mut S, _| s.unkeyed_mounts += 1))
        }))
    };
    let mut rt = Runtime::new(S { show: true, ..S::default() }, view, (200.0, 120.0), Env::default());
    rt.frame(0.0);
    assert_eq!((rt.state.keyed_mounts, rt.state.unkeyed_mounts), (1, 1));

    // Instant unmount clears mounted_seen immediately.
    rt.update(|s, tx| {
        s.show = false;
        tx.instant();
    });
    rt.frame(1.0 / 60.0);
    assert!(rt.commit.mounted_seen_empty(), "instant unmount clears mounted_seen");
    assert_eq!((rt.state.keyed_mounts, rt.state.unkeyed_mounts), (1, 1));

    // Remount re-fires both keyed and positional elements.
    rt.update(|s, tx| {
        s.show = true;
        tx.instant();
    });
    rt.frame(1.0 / 60.0);
    assert_eq!((rt.state.keyed_mounts, rt.state.unkeyed_mounts), (2, 2));
}

#[test]
fn remount_during_exit_transition_refires_for_element_and_descendant() {
    struct S {
        show: bool,
        parent_mounts: usize,
        child_mounts: usize,
    }
    let view = |s: &S, _: &mut Cx<S>| {
        col().size(240.0, 120.0)
            .child(
                el().key("btn").size(60.0, 30.0)
                    .on_press(|s: &mut S| s.show = !s.show),
            )
            .children(s.show.then(|| {
                col().key("panel").size(140.0, 60.0)
                    .transition(Transition::opacity())
                    .on_mount(|s: &mut S, _| s.parent_mounts += 1)
                    .child(
                        el().key("inner").size(80.0, 30.0)
                            .on_mount(|s: &mut S, _| s.child_mounts += 1),
                    )
            }))
    };
    let mut rt = Runtime::new(S { show: true, parent_mounts: 0, child_mounts: 0 }, view, (240.0, 120.0), Env::default());
    rt.frame(0.0);
    assert_eq!((rt.state.parent_mounts, rt.state.child_mounts), (1, 1));

    // Trigger an animated exit (via user click so motion_needs_cause allows exit transition).
    let (x, y) = rt.find("btn").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);

    // Panel must still be in tree.nodes as Exiting, yet cleared from mounted_seen.
    let panel_idx = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("panel")).expect("exiting panel stays in nodes");
    assert!(rt.core().subtree_exiting(panel_idx), "panel is mid-exit transition");
    assert!(rt.commit.mounted_seen_empty(), "exiting element and its descendant are cleared from mounted_seen");

    // Remount while the exit transition is still in flight (16ms later).
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(
        (rt.state.parent_mounts, rt.state.child_mounts),
        (2, 2),
        "remount during active exit transition re-fires both parent and descendant",
    );
}

#[test]
fn fires_in_preorder_tree_order() {
    #[derive(Default)]
    struct S {
        order: Vec<&'static str>,
    }
    let view = |_: &S, _: &mut Cx<S>| {
        col().key("root").size(200.0, 200.0)
            .on_mount(|s: &mut S, _| s.order.push("root"))
            .child(
                col().key("a").size(80.0, 40.0)
                    .on_mount(|s: &mut S, _| s.order.push("a"))
                    .child(el().key("a1").size(20.0, 20.0).on_mount(|s: &mut S, _| s.order.push("a1")))
                    .child(el().key("a2").size(20.0, 20.0).on_mount(|s: &mut S, _| s.order.push("a2"))),
            )
            .child(el().key("b").size(80.0, 40.0).on_mount(|s: &mut S, _| s.order.push("b")))
    };
    let mut rt = Runtime::new(S::default(), view, (200.0, 200.0), Env::default());
    rt.frame(0.0);
    assert_eq!(rt.state.order, vec!["root", "a", "a1", "a2", "b"]);
}

#[test]
fn chained_mounts_run_same_frame_and_cap_at_four_rounds() {
    #[derive(Default)]
    struct S {
        depth: usize,
        log: Vec<usize>,
    }
    // Depth 0 mounts step-0; step-k's on_mount bumps depth to k+1, mounting step-(k+1), up to 6 steps.
    let view = |s: &S, _: &mut Cx<S>| {
        let mut root = col().size(200.0, 200.0);
        for step in 0..=s.depth.min(5) {
            root = root.child(
                el().key(format!("step-{step}")).size(20.0, 20.0).on_mount(move |s: &mut S, _| {
                    s.log.push(step);
                    if step < 5 {
                        s.depth = s.depth.max(step + 1);
                    }
                }),
            );
        }
        root
    };
    let mut rt = Runtime::new(S::default(), view, (200.0, 200.0), Env::default());

    // Frame 1: at most 4 rounds (steps 0, 1, 2, 3 fire; step 4 is mounted by round 3's commit but waits).
    rt.frame(0.0);
    assert_eq!(rt.state.log, vec![0, 1, 2, 3], "chained mounts capped at 4 rounds in one frame");
    assert!(rt.core().mounts_pending(), "step 4 remains pending after hitting the 4-round cap");
    assert!(rt.core().layout_settle_pending(), "settle sees remaining pending mount");

    // Frame 2: remaining steps 4 and 5 complete in frame 2.
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.log, vec![0, 1, 2, 3, 4, 5], "remaining chain finishes on next frame");
    assert!(!rt.core().mounts_pending());
    assert!(!rt.core().layout_settle_pending());
}

#[test]
fn runtime_update_and_out_of_frame_commit_fire_on_next_frame() {
    #[derive(Default)]
    struct S {
        show: bool,
        fired: usize,
    }
    let view = |s: &S, _: &mut Cx<S>| {
        col().size(200.0, 100.0).children(s.show.then(|| {
            text(if s.fired > 0 { "ready" } else { "init" })
                .key("box")
                .on_mount(|s: &mut S, _| s.fired += 1)
        }))
    };
    let mut rt = Runtime::new(S::default(), view, (200.0, 100.0), Env::default());
    rt.frame(0.0);
    assert_eq!(rt.state.fired, 0);

    // 1. Runtime::update followed by next frame.
    rt.update(|s, _| s.show = true);
    assert_eq!(rt.state.fired, 0, "update alone does not run on_mount before frame");
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.fired, 1);
    assert_eq!(element_text(&rt, "box").as_deref(), Some("ready"));

    // Unmount.
    rt.update(|s, tx| {
        s.show = false;
        tx.instant();
    });
    rt.frame(1.0 / 60.0);

    // 2. Direct commit outside frame_step still fires on the next frame.
    rt.state.show = true;
    rt.commit(&Tx::default());
    assert_eq!(rt.state.fired, 1, "raw commit outside frame defers on_mount until next frame");
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.fired, 2, "next frame fires on_mount for already-committed node");
}

#[test]
fn tx_focus_wake_scroll_and_live_only_work_from_on_mount() {
    struct S {
        show: bool,
        live_only: bool,
        alpha: Live<f32>,
        mounts: usize,
    }
    let view = |s: &S, _: &mut Cx<S>| {
        let alpha = s.alpha.clone();
        let live_only = s.live_only;
        col().size(240.0, 160.0)
            .child(
                col().key("scroller").size(200.0, 80.0).scroll(ScrollSpec::default())
                    .child(el().size(200.0, 400.0)),
            )
            .children(s.show.then(|| {
                col().key("panel").size(160.0, 40.0)
                    .focusable(true)
                    .live_opacity(&s.alpha)
                    .on_mount(move |s: &mut S, tx: &mut Tx| {
                        s.mounts += 1;
                        alpha.set(0.5);
                        tx.wake_after(Duration::from_millis(50));
                        tx.scroll_to_y("scroller", 120.0);
                        if live_only {
                            tx.live_only();
                        } else {
                            tx.focus("panel", false);
                        }
                    })
            }))
    };

    // Non-live-only: focuses panel, schedules wake, scrolls viewport in same frame.
    let mut rt = Runtime::new(
        S { show: true, live_only: false, alpha: Live::new(1.0), mounts: 0 },
        view,
        (240.0, 160.0),
        Env::default(),
    );
    rt.frame(0.0);
    assert_eq!(rt.state.mounts, 1);
    assert_eq!(rt.presented_focus_key(), Some("panel"), "tx.focus in on_mount takes effect in same frame");
    assert!(rt.wake_deadline().is_some(), "tx.wake_after in on_mount is scheduled");
    let scroll_id = rt.tree.id_of_path("#0/scroller").unwrap();
    let offset = rt.scroll.scrolls.get(&scroll_id).map(|s| s.offset).unwrap_or(0.0);
    assert!((offset - 120.0).abs() < 1.0, "tx.scroll_to_y in on_mount takes effect in same frame (got {offset})");

    // Live-only: runs handler, updates Live, schedules wake & scroll, skips follow-up commit!
    let mut rt2 = Runtime::new(
        S { show: true, live_only: true, alpha: Live::new(1.0), mounts: 0 },
        view,
        (240.0, 160.0),
        Env::default(),
    );
    rt2.frame(0.0);
    assert_eq!(rt2.state.mounts, 1);
    assert_eq!(rt2.stats.commits, 1, "tx.live_only skips follow-up commit");
    assert!((rt2.state.alpha.get() - 0.5).abs() < 1e-4, "Live value updated in same frame");
    assert!(rt2.wake_deadline().is_some(), "wake still scheduled under live_only");
    let scroll_id2 = rt2.tree.id_of_path("#0/scroller").unwrap();
    let offset2 = rt2.scroll.scrolls.get(&scroll_id2).map(|s| s.offset).unwrap_or(0.0);
    assert!((offset2 - 120.0).abs() < 1.0, "scroll still executed under live_only (got {offset2})");
}

#[test]
fn memo_reused_element_fires_once_and_refires_after_unmount() {
    #[derive(Default)]
    struct S {
        show: bool,
        other: usize,
        mounts: usize,
    }
    let view = |s: &S, _: &mut Cx<S>| {
        col().size(200.0, 100.0)
            .child(text(format!("other={}", s.other)))
            .children(s.show.then(|| {
                crate::memo("memo-card", 1u32, || {
                    el().key("card").size(100.0, 40.0).on_mount(|s: &mut S, _| s.mounts += 1)
                })
            }))
    };
    let mut rt = Runtime::new(S { show: true, other: 0, mounts: 0 }, view, (200.0, 100.0), Env::default());
    rt.frame(0.0);
    assert_eq!(rt.state.mounts, 1);

    // Sibling change reuses memo without re-firing on_mount.
    rt.update(|s, _| s.other += 1);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.mounts, 1, "memo reuse does not re-fire on_mount");

    // Unmount and remount re-fires even if memo key returns.
    rt.update(|s, tx| {
        s.show = false;
        tx.instant();
    });
    rt.frame(1.0 / 60.0);
    rt.update(|s, tx| {
        s.show = true;
        tx.instant();
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.mounts, 2, "remounting memo subtree re-fires on_mount");
}

#[test]
fn inert_reduced_motion_non_interactive_and_self_unmount_edges() {
    #[derive(Default)]
    struct S {
        show_self_unmount: bool,
        inert_mounts: usize,
        self_unmounts: usize,
    }
    let view = |s: &S, _: &mut Cx<S>| {
        col().size(200.0, 120.0)
            .child(
                el().key("inert-box").size(100.0, 40.0).inert()
                    .on_mount(|s: &mut S, _| s.inert_mounts += 1),
            )
            .children(s.show_self_unmount.then(|| {
                el().key("ephemeral").size(80.0, 40.0)
                    .on_mount(|s: &mut S, tx| {
                        s.self_unmounts += 1;
                        s.show_self_unmount = false;
                        tx.instant();
                    })
            }))
    };
    let mut rt = Runtime::new(S { show_self_unmount: true, ..S::default() }, view, (200.0, 120.0), Env::default());
    rt.set_system_reduced_motion(true);
    rt.frame(0.0);

    assert_eq!(rt.state.inert_mounts, 1, "inert elements still mount and fire on_mount");
    assert_eq!(rt.state.self_unmounts, 1, "self-unmounting element fires once");
    assert!(!rt.state.show_self_unmount);
    assert!(rt.find("ephemeral").is_none(), "self-unmounted element is gone in the same frame");

    // on_mount does not turn an element into an interactive pointer hit target.
    assert!(
        rt.element_at_with(50.0, 20.0, Pick::new().interactive(true)).is_none(),
        "on_mount must not make an element an interactive hit target",
    );

    // Remounting the self-unmounting element re-fires because mounted_seen dropped it.
    rt.update(|s, _| s.show_self_unmount = true);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.self_unmounts, 2, "re-showing self-unmounted element fires again");
}

#[test]
fn unused_views_pay_zero_extra_commits_or_idle_wakes() {
    let view = |_: &(), _: &mut Cx<()>| {
        col().size(200.0, 100.0).child(text("plain").key("plain"))
    };
    let mut rt = Runtime::new((), view, (200.0, 100.0), Env::default());
    let anim0 = rt.frame(0.0);
    assert!(!anim0);
    assert_eq!(rt.stats.commits, 1, "single commit on first frame when on_mount is unused");
    assert!(!rt.tree.has_mount);
    assert!(rt.commit.mounted_seen_empty());
    assert!(!rt.stale());
    assert!(!rt.needs_commit());
    assert_eq!(rt.wake_deadline(), None);

    let anim1 = rt.frame(1.0 / 60.0);
    assert!(!anim1);
    assert_eq!(rt.stats.commits, 1, "zero commits on idle frames");
    assert_eq!(rt.wake_deadline(), None);
}
