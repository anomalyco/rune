//! State-independent `El::on_layout_any`, `El::on_visible_any`, and `El::on_key_any` handlers:
//! mounting one non-generic component function across distinct app state types, plus layout/scale,
//! visibility, key propagation, `Tx::took`, click-to-focus, inert/disabled/modal guards, and
//! debug caller-site tracking.
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use super::*;
use crate::{
    Cx, El, Env, Key, KeyEvent, Live, Mods, ScrollSpec, TextEdit, TextStyle, col, el, text,
    text_input,
};

#[derive(Default)]
struct WidgetLog {
    layouts: RefCell<Vec<([f32; 4], f32)>>,
    visible: RefCell<Vec<bool>>,
    keys: RefCell<Vec<Key>>,
}

/// One state-agnostic reusable component (no `<S>` parameter) that uses all three `_any` handlers:
/// `on_layout_any`, `on_visible_any`, and `on_key_any`.
fn reusable_widget(log: &Rc<WidgetLog>, live_w: &Live<f32>, width: f32) -> El {
    let l_layout = log.clone();
    let l_vis = log.clone();
    let l_key = log.clone();
    let w_live = live_w.clone();
    col()
        .key("widget")
        .size(width, 60.0)
        .role("group")
        .label("Reusable timeline")
        .focusable(true)
        .on_layout_any(move |rect, tx| {
            l_layout.layouts.borrow_mut().push((rect, tx.scale()));
            w_live.set(rect[2]);
            tx.live_only();
        })
        .on_visible_any(move |on, tx| {
            l_vis.visible.borrow_mut().push(on);
            tx.live_only();
        })
        .on_key_any(move |ev, tx| {
            if matches!(ev.key, Key::Left | Key::Right) {
                l_key.keys.borrow_mut().push(ev.key.clone());
                tx.live_only();
                true
            } else {
                false
            }
        })
        .child(text("Timeline"))
}

fn send_key<S: 'static>(rt: &mut Runtime<S>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
}

fn focused_key<S: 'static>(rt: &Runtime<S>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key)
}

fn center_of<S: 'static>(rt: &Runtime<S>, key: &str) -> (f32, f32) {
    let r = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect;
    (r[0] + r[2] * 0.5, r[1] + r[3] * 0.5)
}

fn click<S: 'static>(rt: &mut Runtime<S>, (x, y): (f32, f32)) {
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

#[test]
fn single_state_agnostic_component_mounts_under_two_distinct_app_state_types() {
    struct HostA {
        log: Rc<WidgetLog>,
        live_w: Live<f32>,
        width: f32,
        outer_keys: Vec<Key>,
    }
    struct HostB {
        tag: &'static str,
        log: Rc<WidgetLog>,
        live_w: Live<f32>,
        outer_keys: Vec<Key>,
    }

    fn view_a(s: &HostA, _: &mut Cx<HostA>) -> El {
        col()
            .w_full()
            .on_key(|s: &mut HostA, ev: &KeyEvent, _| {
                s.outer_keys.push(ev.key.clone());
                true
            })
            .child(reusable_widget(&s.log, &s.live_w, s.width))
            .child(el().size(200.0, 400.0))
    }

    fn view_b(s: &HostB, _: &mut Cx<HostB>) -> El {
        let _ = s.tag;
        col()
            .w_full()
            .on_key(|s: &mut HostB, ev: &KeyEvent, _| {
                s.outer_keys.push(ev.key.clone());
                true
            })
            .child(el().size(200.0, 260.0))
            .child(reusable_widget(&s.log, &s.live_w, 180.0))
            .child(el().size(200.0, 300.0))
    }

    // Host A: widget starts visible on screen.
    let log_a = Rc::new(WidgetLog::default());
    let live_a = Live::new(0.0_f32);
    let mut rt_a = Runtime::new(
        HostA { log: log_a.clone(), live_w: live_a.clone(), width: 150.0, outer_keys: Vec::new() },
        view_a,
        (300.0, 200.0),
        Env::new(true, 1.0),
    );
    rt_a.page_mode();
    rt_a.frame(0.0);
    rt_a.frame(1.0 / 60.0);
    assert_eq!(live_a.get(), 150.0);
    assert_eq!(log_a.layouts.borrow().len(), 1);
    assert_eq!(log_a.layouts.borrow()[0].1, 1.0);
    assert_eq!(*log_a.visible.borrow(), vec![true]);

    // Scroll offscreen and back in Host A.
    rt_a.set_page_scroll(120.0);
    rt_a.frame(1.0 / 60.0);
    rt_a.frame(1.0 / 60.0);
    assert_eq!(*log_a.visible.borrow(), vec![true, false]);

    rt_a.set_page_scroll(0.0);
    rt_a.frame(1.0 / 60.0);
    rt_a.frame(1.0 / 60.0);
    assert_eq!(*log_a.visible.borrow(), vec![true, false, true]);

    // Focus widget and test consumed vs propagated keys in Host A.
    send_key(&mut rt_a, Key::Tab);
    assert_eq!(focused_key(&rt_a).as_deref(), Some("widget"));
    send_key(&mut rt_a, Key::Right);
    send_key(&mut rt_a, Key::Up);
    assert_eq!(*log_a.keys.borrow(), vec![Key::Right]);
    assert_eq!(rt_a.state.outer_keys, vec![Key::Up]);

    // Host B: completely different state type, widget starts offscreen.
    let log_b = Rc::new(WidgetLog::default());
    let live_b = Live::new(0.0_f32);
    let mut rt_b = Runtime::new(
        HostB { tag: "gallery", log: log_b.clone(), live_w: live_b.clone(), outer_keys: Vec::new() },
        view_b,
        (300.0, 200.0),
        Env::new(true, 2.0),
    );
    rt_b.page_mode();
    rt_b.frame(0.0);
    rt_b.frame(1.0 / 60.0);
    assert_eq!(live_b.get(), 180.0);
    assert_eq!(log_b.layouts.borrow().len(), 1);
    assert_eq!(log_b.layouts.borrow()[0].1, 2.0);
    assert!(log_b.visible.borrow().is_empty(), "starts offscreen in Host B");

    rt_b.set_page_scroll(160.0);
    rt_b.frame(1.0 / 60.0);
    rt_b.frame(1.0 / 60.0);
    assert_eq!(*log_b.visible.borrow(), vec![true]);

    send_key(&mut rt_b, Key::Tab);
    send_key(&mut rt_b, Key::Left);
    send_key(&mut rt_b, Key::Down);
    assert_eq!(*log_b.keys.borrow(), vec![Key::Left]);
    assert_eq!(rt_b.state.outer_keys, vec![Key::Down]);
}

type LayoutReports = Rc<RefCell<Vec<([f32; 4], f32)>>>;

#[test]
fn on_layout_any_reports_initial_resize_zero_size_scale_and_live_only_skips_commits() {
    let seen: LayoutReports = Rc::new(RefCell::new(Vec::new()));
    let zero_seen = Rc::new(RefCell::new(Vec::<[f32; 4]>::new()));

    struct S {
        w: f32,
        seen: LayoutReports,
        zero_seen: Rc<RefCell<Vec<[f32; 4]>>>,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let seen = s.seen.clone();
        let zero = s.zero_seen.clone();
        col()
            .w_full()
            .h_full()
            .padding(10.0)
            .child(
                el().key("card")
                    .size(s.w, 40.0)
                    .on_layout_any(move |rect, tx| {
                        seen.borrow_mut().push((rect, tx.scale()));
                        tx.live_only();
                    }),
            )
            .child(
                el().key("empty")
                    .size(0.0, 0.0)
                    .on_layout_any(move |rect, tx| {
                        zero.borrow_mut().push(rect);
                        tx.live_only();
                    }),
            )
    }

    let mut rt = Runtime::new(
        S { w: 100.0, seen: seen.clone(), zero_seen: zero_seen.clone() },
        view,
        (300.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    let commits_after_first = rt.stats.commits;
    rt.frame(1.0 / 60.0);
    assert_eq!(seen.borrow().len(), 1);
    assert_eq!(seen.borrow()[0], ([10.0, 10.0, 100.0, 40.0], 1.0));
    assert_eq!(zero_seen.borrow().len(), 1);
    assert_eq!(zero_seen.borrow()[0][2], 0.0);
    assert_eq!(zero_seen.borrow()[0][3], 0.0);
    assert_eq!(rt.stats.commits, commits_after_first, "live_only on_layout_any skips extra commit");

    // Idle frame after layout report does zero work.
    assert!(!rt.frame(1.0 / 60.0));
    assert_eq!(rt.stats.commits, commits_after_first);

    // Display scale change re-fires on_layout_any even when logical rect is unchanged.
    rt.set_viewport((300.0, 200.0), 2.0);
    rt.frame(1.0 / 60.0);
    let commits_after_scale = rt.stats.commits;
    rt.frame(1.0 / 60.0);
    assert_eq!(seen.borrow().len(), 2);
    assert_eq!(seen.borrow()[1], ([10.0, 10.0, 100.0, 40.0], 2.0));
    assert_eq!(rt.stats.commits, commits_after_scale, "live_only scale report skips extra commit");

    // Resizing the element re-fires on_layout_any with new width.
    rt.update(|s, _| s.w = 180.0);
    rt.frame(1.0 / 60.0);
    let commits_after_resize = rt.stats.commits;
    rt.frame(1.0 / 60.0);
    assert_eq!(seen.borrow().len(), 3);
    assert_eq!(seen.borrow()[2], ([10.0, 10.0, 180.0, 40.0], 2.0));
    assert_eq!(rt.stats.commits, commits_after_resize);
}

#[test]
fn on_layout_any_commits_when_not_live_only_and_records_debug_caller_site() {
    let calls = Rc::new(RefCell::new(0usize));
    let calls_ref = calls.clone();

    struct S {
        calls: Rc<RefCell<usize>>,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let c = s.calls.clone();
        col().size(200.0, 100.0).child(
            el().key("box").size(80.0, 40.0).on_layout_any(move |_, _| {
                *c.borrow_mut() += 1;
            }),
        )
    }

    let mut rt = Runtime::new(S { calls: calls_ref }, view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let commits_before = rt.stats.commits;
    rt.frame(1.0 / 60.0);
    assert_eq!(*calls.borrow(), 1);
    assert_eq!(rt.stats.commits, commits_before + 1, "without live_only, on_layout_any commits once");

    // Idle settles cleanly with no extra commits.
    let settled = rt.stats.commits;
    assert!(!rt.frame(1.0 / 60.0));
    assert_eq!(rt.stats.commits, settled);

    #[cfg(debug_assertions)]
    {
        let node = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("box")).unwrap();
        let site = node.handlers.get().and_then(|h| h.sites.layout);
        assert!(site.is_some(), "on_layout_any must record debug caller site like on_layout_tx");
        assert!(site.unwrap().file().ends_with("any_handlers_tests.rs"));
    }
}

#[test]
fn on_visible_any_tracks_scroll_viewport_live_only_and_wake_after() {
    let log = Rc::new(RefCell::new(Vec::<bool>::new()));
    let log_ref = log.clone();

    struct S {
        log: Rc<RefCell<Vec<bool>>>,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let l = s.log.clone();
        col()
            .size(300.0, 200.0)
            .child(
                col()
                    .key("scroller")
                    .size(300.0, 200.0)
                    .scroll(ScrollSpec { stick_bottom: false })
                    .child(el().size(300.0, 300.0))
                    .child(
                        el().key("target")
                            .size(300.0, 100.0)
                            .on_visible_any(move |on, tx| {
                                l.borrow_mut().push(on);
                                if !on {
                                    tx.wake_after(Duration::from_millis(50));
                                }
                                tx.live_only();
                            }),
                    )
                    .child(el().size(300.0, 400.0)),
            )
    }

    let mut rt = Runtime::new(S { log: log_ref }, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let commits_after_mount = rt.stats.commits;
    rt.frame(1.0 / 60.0);
    assert!(log.borrow().is_empty(), "target starts below the 200pt viewport");
    assert_eq!(rt.stats.commits, commits_after_mount);

    // Scroll 130pt: only 30 of 100pt visible (<50%), still not reported.
    rt.update(|_, tx| tx.scroll_to_y("scroller", 130.0));
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    assert!(log.borrow().is_empty());

    // Scroll 160pt: 60 of 100pt visible (>=50%), reports true once with no extra view commit.
    rt.update(|_, tx| tx.scroll_to_y("scroller", 160.0));
    rt.frame(1.0 / 60.0);
    let commits_on_scroll = rt.stats.commits;
    rt.frame(1.0 / 60.0);
    assert_eq!(*log.borrow(), vec![true]);
    assert_eq!(rt.stats.commits, commits_on_scroll, "live_only on_visible_any skips extra commit");

    // Scroll to 460pt: target leaves viewport -> reports false and schedules wake_after.
    rt.update(|_, tx| tx.scroll_to_y("scroller", 460.0));
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(*log.borrow(), vec![true, false]);
    assert!(rt.wake_deadline().is_some(), "wake_after from on_visible_any must be scheduled");
}

#[test]
fn on_key_any_preserves_nesting_click_focus_tx_took_text_field_and_modal_inert_guards() {
    let log = Rc::new(RefCell::new(Vec::<String>::new()));

    struct S {
        log: Rc<RefCell<Vec<String>>>,
        outer_count: usize,
        field: TextEdit,
        modal_open: bool,
        inert_control: bool,
        disabled_control: bool,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let inner_log = s.log.clone();
        let mid_log = s.log.clone();
        let preview_log = s.log.clone();

        let mut control = col()
            .key("control")
            .size(140.0, 50.0)
            .focusable(!s.disabled_control)
            .on_key_any(move |ev, tx| {
                // Intentionally mark live_only even when declining, to verify Tx::took clears it.
                tx.live_only();
                match ev.key {
                    Key::Right => {
                        inner_log.borrow_mut().push("inner Right".to_owned());
                        true
                    }
                    _ => false,
                }
            })
            .child(el().key("inner-pad").size(120.0, 30.0));
        if s.inert_control {
            control = control.inert();
        }

        let mid = col()
            .key("mid")
            .on_key_any(move |ev, tx| {
                if ev.key == Key::Left {
                    mid_log.borrow_mut().push("mid Left".to_owned());
                    tx.live_only();
                    true
                } else {
                    false
                }
            })
            .child(control)
            .child(
                text_input(&s.field, TextStyle::new(14.0, 400.0))
                    .key("field")
                    .size(160.0, 30.0)
                    .on_edit(|s: &mut S| &mut s.field),
            );

        let outer = col()
            .key("outer")
            .on_key_preview(move |_: &mut S, ev: &KeyEvent, _| {
                if ev.key == Key::Escape {
                    preview_log.borrow_mut().push("preview Escape".to_owned());
                    true
                } else {
                    false
                }
            })
            .on_key(|s: &mut S, ev: &KeyEvent, _| {
                if ev.key == Key::Up {
                    s.outer_count += 1;
                    s.log.borrow_mut().push(format!("outer Up #{}", s.outer_count));
                    true
                } else {
                    false
                }
            })
            .child(mid);

        col()
            .size(300.0, 200.0)
            .child(outer)
            .children(s.modal_open.then(|| {
                col()
                    .key("dialog")
                    .size(200.0, 100.0)
                    .modal(false, |s: &mut S| s.modal_open = false)
                    .child(el().key("dialog-btn").size(80.0, 30.0).focusable(true))
            }))
    }

    let mut rt = Runtime::new(
        S {
            log: log.clone(),
            outer_count: 0,
            field: TextEdit::default(),
            modal_open: false,
            inert_control: false,
            disabled_control: false,
        },
        view,
        (300.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    // Clicking an unfocusable child inside a focusable on_key_any control focuses the control.
    let pad = center_of(&rt, "inner-pad");
    click(&mut rt, pad);
    assert_eq!(focused_key(&rt).as_deref(), Some("control"));

    // Inner on_key_any consumes Right; mid on_key_any consumes Left; outer typed on_key consumes Up.
    let commits_before_right = rt.stats.commits;
    send_key(&mut rt, Key::Right);
    assert_eq!(rt.stats.commits, commits_before_right, "live_only consumed on_key_any skips commit");

    send_key(&mut rt, Key::Left);
    let commits_before_up = rt.stats.commits;
    send_key(&mut rt, Key::Up);
    assert_eq!(
        rt.stats.commits,
        commits_before_up + 1,
        "declining inner on_key_any must clear live_only via Tx::took so outer on_key commits",
    );

    // on_key_preview still runs before on_key_any.
    send_key(&mut rt, Key::Escape);
    assert_eq!(
        *log.borrow(),
        vec!["inner Right", "mid Left", "outer Up #1", "preview Escape"]
    );
    log.borrow_mut().clear();

    // Focused text field keeps its keys (ancestor on_key_any does not steal Left/Right).
    let field = center_of(&rt, "field");
    click(&mut rt, field);
    assert_eq!(focused_key(&rt).as_deref(), Some("field"));
    send_key(&mut rt, Key::Left);
    assert!(log.borrow().is_empty(), "ancestor on_key_any must not steal keys from focused text_input");

    // Modal blocks on_key_any behind it.
    rt.update(|s, _| s.modal_open = true);
    rt.frame(1.0 / 60.0);
    send_key(&mut rt, Key::Right);
    assert!(log.borrow().is_empty(), "active modal blocks on_key_any behind it");

    // Disabled (focusable(false)) and inert controls cannot take focus or receive on_key_any.
    rt.update(|s, _| {
        s.modal_open = false;
        s.disabled_control = true;
    });
    rt.frame(1.0 / 60.0);
    let pad = center_of(&rt, "inner-pad");
    click(&mut rt, pad);
    assert_ne!(focused_key(&rt).as_deref(), Some("control"));
    send_key(&mut rt, Key::Right);
    assert!(log.borrow().is_empty(), "disabled control receives no on_key_any");

    rt.update(|s, _| {
        s.disabled_control = false;
        s.inert_control = true;
    });
    rt.frame(1.0 / 60.0);
    let pad = center_of(&rt, "inner-pad");
    click(&mut rt, pad);
    assert_ne!(focused_key(&rt).as_deref(), Some("control"));
    send_key(&mut rt, Key::Right);
    assert!(log.borrow().is_empty(), "inert control receives no on_key_any");
}
