//! State-independent `El::on_layout_any`, `El::on_visible_any`, `El::on_key_any`,
//! `El::on_press_tx_any`, `El::on_wheel_any`, and `El::on_magnify_any` handlers:
//! mounting one non-generic component function across distinct app state types, plus layout/scale,
//! visibility, key propagation, button/keyboard/touch press activation, wheel pass-through/bubbling
//! and momentum, pinch/Ctrl+wheel/touch magnify, `Tx::took`, `Tx::live_only`, inert/disabled/modal
//! guards, and debug caller-site tracking.
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use super::*;
use crate::{
    Cx, El, Env, GesturePhase, Key, KeyEvent, Live, MagnifyEvent, Mods, PointerButton, ScrollSpec,
    TextEdit, TextStyle, TouchPhase, WheelEvent, col, el, row, text, text_input,
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

#[derive(Default)]
struct SurfaceLog {
    presses: RefCell<Vec<(PointerButton, bool)>>,
    wheels: RefCell<Vec<(f32, f32, f32, f32, bool)>>,
    magnifies: RefCell<Vec<(f32, f32, f32, GesturePhase, bool)>>,
}

/// One state-agnostic reusable component (no `<S>` parameter) exercising all three handlers:
/// `on_press_tx_any`, `on_wheel_any`, and `on_magnify_any`.
fn reusable_surface(log: &Rc<SurfaceLog>, zoom: &Live<f32>) -> El {
    let (l_press, l_wheel, l_mag, z_live) = (log.clone(), log.clone(), log.clone(), zoom.clone());
    col()
        .key("surface")
        .size(200.0, 140.0)
        .on_wheel_any(move |ev, tx| {
            l_wheel.wheels.borrow_mut().push((ev.dx, ev.dy, ev.x, ev.y, ev.touch));
            tx.live_only();
            false
        })
        .on_magnify_any(move |ev, tx| {
            l_mag.magnifies.borrow_mut().push((ev.delta, ev.x, ev.y, ev.phase, ev.wheel));
            z_live.set(z_live.get() * (1.0 + ev.delta));
            tx.live_only();
            true
        })
        .child(
            el().key("close-btn").size(60.0, 32.0).label("Dismiss").on_press_tx_any(move |tx| {
                l_press.presses.borrow_mut().push((tx.button(), tx.press_point().is_some()));
                tx.live_only();
            }),
        )
}

#[test]
fn single_component_with_press_wheel_magnify_any_mounts_under_distinct_hosts() {
    struct HostA { log: Rc<SurfaceLog>, zoom: Live<f32> }
    struct HostB { log: Rc<SurfaceLog>, zoom: Live<f32>, _extra: String }

    fn exercise<S: 'static>(rt: &mut Runtime<S>, log: &Rc<SurfaceLog>, zoom: &Live<f32>) {
        rt.frame(0.0);
        let btn = center_of(rt, "close-btn");
        click(rt, btn);
        assert_eq!(log.presses.borrow().as_slice(), &[(PointerButton::Primary, true)]);

        let surface = center_of(rt, "surface");
        rt.wheel_event(WheelEvent::new(-18.0, 0.0).at(surface.0, surface.1).precise(true));
        rt.frame(1.0 / 60.0);
        assert_eq!(log.wheels.borrow().len(), 1);
        assert_eq!(log.wheels.borrow()[0].0, -18.0);

        rt.magnify_event(MagnifyEvent::new(0.25).at(surface.0, surface.1).phase(GesturePhase::Began));
        rt.magnify_event(MagnifyEvent::new(0.0).at(surface.0, surface.1).phase(GesturePhase::Ended));
        rt.frame(1.0 / 60.0);
        assert_eq!(log.magnifies.borrow().len(), 2);
        assert!((zoom.get() - 1.25).abs() < 1e-4, "zoom updated via Live: {}", zoom.get());
    }

    let (log_a, zoom_a) = (Rc::new(SurfaceLog::default()), Live::new(1.0_f32));
    let mut rt_a = Runtime::new(
        HostA { log: log_a.clone(), zoom: zoom_a.clone() },
        |s, _| el().size(240.0, 180.0).child(reusable_surface(&s.log, &s.zoom)),
        (240.0, 180.0),
        Env::new(true, 1.0),
    );
    exercise(&mut rt_a, &log_a, &zoom_a);

    let (log_b, zoom_b) = (Rc::new(SurfaceLog::default()), Live::new(1.0_f32));
    let mut rt_b = Runtime::new(
        HostB { log: log_b.clone(), zoom: zoom_b.clone(), _extra: "other-host".into() },
        |s, _| el().size(240.0, 180.0).child(reusable_surface(&s.log, &s.zoom)),
        (240.0, 180.0),
        Env::new(true, 1.0),
    );
    exercise(&mut rt_b, &log_b, &zoom_b);
}

#[test]
fn on_press_tx_any_preserves_button_role_keyboard_touch_live_only_and_guards() {
    struct App {
        modal_open: bool,
        disabled_btn: bool,
        inert_btn: bool,
        live_only: bool,
        log: Rc<RefCell<Vec<(&'static str, bool)>>>,
    }

    let log = Rc::new(RefCell::new(Vec::new()));
    let mut rt = Runtime::new(
        App { modal_open: false, disabled_btn: false, inert_btn: false, live_only: true, log: log.clone() },
        |s, _| {
            let (l1, l2, live_only) = (s.log.clone(), s.log.clone(), s.live_only);
            let mut btn = el()
                .key("dismiss")
                .size(80.0, 32.0)
                .label("Dismiss")
                .focusable(!s.disabled_btn)
                .press_on_return()
                .disabled(s.disabled_btn)
                .on_press_tx_any(move |tx| {
                    l1.borrow_mut().push(("dismiss", tx.press_point().is_some()));
                    if live_only { tx.live_only(); }
                });
            if s.inert_btn { btn = btn.inert(); }
            col()
                .size(240.0, 160.0)
                .child(btn)
                .child(
                    el().key("custom-role").size(80.0, 32.0).role("tab").label("Tab").on_press_tx_any(move |tx| {
                        l2.borrow_mut().push(("tab", tx.press_point().is_some()));
                        tx.live_only();
                    }),
                )
                .children(s.modal_open.then(|| {
                    el().key("modal").absolute(0.0, 0.0).size(240.0, 160.0).modal(false, |_: &mut App| {})
                }))
        },
        (240.0, 160.0),
        Env::new(true, 1.0),
    );
    rt.check_live_only(false);
    rt.frame(0.0);

    // Default role promotes from "group" to "button"; explicit "tab" role stays intact.
    let dismiss_info = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("dismiss")).unwrap();
    let tab_info = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("custom-role")).unwrap();
    assert_eq!(dismiss_info.role, "button", "on_press_tx_any promotes default role to button");
    assert_eq!(tab_info.role, "tab", "on_press_tx_any preserves explicit role");

    // Pointer click delivers press_point; a live_only on_press_tx_any skips commits when focus doesn't change.
    let tab_pt = center_of(&rt, "custom-role");
    let commits_before = rt.stats.commits;
    click(&mut rt, tab_pt);
    assert_eq!(rt.stats.commits, commits_before, "live_only on_press_tx_any skips commit");
    assert_eq!(*log.borrow(), vec![("tab", true)]);
    log.borrow_mut().clear();

    let pt = center_of(&rt, "dismiss");
    click(&mut rt, pt);
    assert_eq!(*log.borrow(), vec![("dismiss", true)]);

    // Touch tap activates on_press_tx_any with press_point.
    rt.touch(1, TouchPhase::Started, pt.0, pt.1, 0.0);
    rt.touch(1, TouchPhase::Ended, pt.0, pt.1, 0.02);
    rt.frame(1.0 / 60.0);
    assert_eq!(log.borrow().last(), Some(&("dismiss", true)));

    // Keyboard activation (Tab to focus, then Space and Return) activates with no press_point,
    // skipping commit when live_only and committing otherwise.
    log.borrow_mut().clear();
    send_key(&mut rt, Key::Tab);
    assert_eq!(focused_key(&rt).as_deref(), Some("dismiss"));
    let commits_before_live_key = rt.stats.commits;
    send_key(&mut rt, Key::Text(" ".into()));
    assert_eq!(rt.stats.commits, commits_before_live_key, "live_only keyboard on_press_tx_any skips commit");
    assert_eq!(*log.borrow(), vec![("dismiss", false)]);

    rt.update(|s, _| s.live_only = false);
    rt.frame(1.0 / 60.0);
    log.borrow_mut().clear();
    let commits_before_enter = rt.stats.commits;
    send_key(&mut rt, Key::Enter);
    assert_eq!(rt.stats.commits, commits_before_enter + 1, "non-live_only on_press_tx_any commits");
    assert_eq!(*log.borrow(), vec![("dismiss", false)]);
    log.borrow_mut().clear();

    // Active modal, disabled, and inert guards block on_press_tx_any.
    rt.update(|s, _| s.modal_open = true);
    rt.frame(1.0 / 60.0);
    click(&mut rt, pt);
    assert!(log.borrow().is_empty(), "modal blocks on_press_tx_any behind it");

    // Disabled blocks keyboard focus/activation; inert blocks pointer activation too.
    rt.update(|s, _| { s.modal_open = false; s.disabled_btn = true; });
    rt.frame(1.0 / 60.0);
    send_key(&mut rt, Key::Tab);
    assert_ne!(focused_key(&rt).as_deref(), Some("dismiss"));
    send_key(&mut rt, Key::Text(" ".into()));
    send_key(&mut rt, Key::Enter);
    assert!(log.borrow().is_empty(), "disabled on_press_tx_any does not take focus or keyboard activation");

    rt.update(|s, _| { s.disabled_btn = false; s.inert_btn = true; });
    rt.frame(1.0 / 60.0);
    click(&mut rt, pt);
    assert!(log.borrow().is_empty(), "inert on_press_tx_any does not fire");
}

#[test]
fn on_wheel_any_declines_to_native_scroll_and_momentum_or_consumes_and_bubbles() {
    struct App {
        consume: bool,
        outer_consume: bool,
        modal_open: bool,
        inert_strip: bool,
        outer_wheels: Vec<(f32, f32)>,
        inner_log: Rc<RefCell<Vec<(f32, f32, f32, f32, bool, bool)>>>,
    }

    let inner_log = Rc::new(RefCell::new(Vec::new()));
    let mut rt = Runtime::new(
        App {
            consume: false,
            outer_consume: false,
            modal_open: false,
            inert_strip: false,
            outer_wheels: Vec::new(),
            inner_log: inner_log.clone(),
        },
        |s, _| {
            let (log, consume) = (s.inner_log.clone(), s.consume);
            let mut strip = row()
                .key("strip")
                .size(200.0, 80.0)
                .scroll(ScrollSpec::default())
                .scroll_x()
                .on_wheel_any(move |ev, tx| {
                    log.borrow_mut().push((ev.dx, ev.dy, ev.x, ev.y, ev.momentum, ev.touch));
                    tx.live_only();
                    consume
                })
                .child(el().key("wide").size(800.0, 80.0));
            if s.inert_strip { strip = strip.inert(); }
            el().key("outer")
                .size(300.0, 120.0)
                .on_wheel(|s: &mut App, ev, _| { s.outer_wheels.push((ev.dx, ev.dy)); s.outer_consume })
                .child(strip)
                .children(s.modal_open.then(|| {
                    el().key("modal").absolute(0.0, 0.0).size(300.0, 120.0).modal(false, |_: &mut App| {})
                }))
        },
        (300.0, 120.0),
        Env::new(true, 1.0),
    );
    rt.check_live_only(false);
    rt.frame(0.0);

    // 1. Declining on_wheel_any observes horizontal wheel, bubbles to outer on_wheel, and lets
    //    native horizontal scrolling + momentum run when outer also declines.
    let strip_id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("strip")).unwrap().id;
    rt.wheel_event(WheelEvent::new(-30.0, 0.0).at(50.0, 40.0).phase(GesturePhase::Began).precise(true));
    rt.frame(1.0 / 60.0);
    rt.wheel_event(WheelEvent::new(-25.0, 0.0).at(50.0, 40.0).phase(GesturePhase::Moved).precise(true));
    rt.frame(1.0 / 60.0);
    rt.wheel_event(WheelEvent::new(0.0, 0.0).at(50.0, 40.0).phase(GesturePhase::Ended).precise(true));
    rt.frame(1.0 / 60.0);
    rt.wheel_event(
        WheelEvent::new(-20.0, 0.0).at(50.0, 40.0).phase(GesturePhase::Began).momentum(true).precise(true),
    );
    rt.wheel_event(
        WheelEvent::new(0.0, 0.0).at(50.0, 40.0).phase(GesturePhase::Ended).momentum(true).precise(true),
    );
    rt.frame(1.0 / 60.0);

    assert_eq!(inner_log.borrow().len(), 1, "once scroll owns the phased gesture, native scroll + momentum run");
    assert_eq!(rt.state.outer_wheels.len(), 1, "declined delta bubbled to outer on_wheel");
    assert!(
        rt.scroll.scrolls[&strip_id].x.offset >= 70.0,
        "declining on_wheel_any preserves native horizontal scroll + momentum: {}",
        rt.scroll.scrolls[&strip_id].x.offset,
    );

    // 1b. When inner declines after calling live_only and outer consumes, Tx::took clears live_only
    //     so outer commits. Advance past the momentum window first so this starts a fresh gesture.
    rt.update(|s, _| { s.outer_consume = true; s.outer_wheels.clear(); });
    rt.frame(0.5);
    let commits_before = rt.stats.commits;
    rt.wheel_event(WheelEvent::new(-10.0, 0.0).at(50.0, 40.0).phase(GesturePhase::Began).precise(true));
    rt.wheel_event(WheelEvent::new(0.0, 0.0).at(50.0, 40.0).phase(GesturePhase::Ended).precise(true));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.stats.commits, commits_before + 1, "declining inner live_only lets consuming outer commit");
    assert_eq!(rt.state.outer_wheels.len(), 2);

    // 2. Consuming on_wheel_any stops bubbling, skips native scroll, respects live_only, and locks
    //    the phased gesture owner even when the pointer drifts outside.
    rt.update(|s, _| { s.consume = true; s.outer_wheels.clear(); });
    rt.frame(0.5);
    inner_log.borrow_mut().clear();

    let scroll_before = rt.scroll.scrolls[&strip_id].x.offset;
    let commits_before = rt.stats.commits;
    rt.wheel_event(WheelEvent::new(-15.0, 0.0).at(60.0, 40.0).phase(GesturePhase::Began).precise(true));
    rt.wheel_event(WheelEvent::new(-10.0, 0.0).at(260.0, 100.0).phase(GesturePhase::Moved).precise(true));
    rt.wheel_event(WheelEvent::new(0.0, 0.0).at(260.0, 100.0).phase(GesturePhase::Ended).precise(true));
    rt.frame(1.0 / 60.0);

    assert_eq!(rt.stats.commits, commits_before, "live_only consuming on_wheel_any skips commit");
    assert_eq!(inner_log.borrow().len(), 3, "locked phased owner hears Began/Moved/Ended outside bounds");
    assert!(rt.state.outer_wheels.is_empty(), "consumed wheel does not bubble to outer handler");
    assert_eq!(rt.scroll.scrolls[&strip_id].x.offset, scroll_before, "consumed wheel does not scroll");

    // 3. Two-finger touchscreen pan reaches on_wheel_any with WheelEvent::touch == true.
    inner_log.borrow_mut().clear();
    rt.touch(1, TouchPhase::Started, 60.0, 40.0, 0.0);
    rt.touch(2, TouchPhase::Started, 120.0, 40.0, 0.01);
    rt.touch(1, TouchPhase::Moved, 80.0, 40.0, 0.02);
    rt.touch(2, TouchPhase::Moved, 140.0, 40.0, 0.02);
    rt.touch(2, TouchPhase::Ended, 140.0, 40.0, 0.03);
    rt.touch(1, TouchPhase::Ended, 80.0, 40.0, 0.04);
    rt.frame(1.0 / 60.0);
    assert!(
        inner_log.borrow().iter().any(|e| e.5 && e.0.abs() > 0.0),
        "two-finger touch pan reaches on_wheel_any with touch == true: {:?}",
        *inner_log.borrow(),
    );

    // 4. Modal and inert guards block on_wheel_any.
    inner_log.borrow_mut().clear();
    rt.update(|s, _| s.modal_open = true);
    rt.frame(1.0 / 60.0);
    rt.wheel_event(WheelEvent::new(-12.0, 0.0).at(50.0, 40.0));
    rt.frame(1.0 / 60.0);
    assert!(inner_log.borrow().is_empty(), "modal blocks on_wheel_any behind it");

    rt.update(|s, _| { s.modal_open = false; s.inert_strip = true; });
    rt.frame(1.0 / 60.0);
    rt.wheel_event(WheelEvent::new(-12.0, 0.0).at(50.0, 40.0));
    rt.frame(1.0 / 60.0);
    assert!(inner_log.borrow().is_empty(), "inert element receives no on_wheel_any");
}

#[test]
fn on_magnify_any_consumes_pinch_ctrl_wheel_and_touch_pinch_or_bubbles() {
    struct App {
        consume: bool,
        modal_open: bool,
        inert_canvas: bool,
        outer_magnifies: Vec<(f32, bool)>,
        inner_log: Rc<RefCell<Vec<(f32, f32, f32, GesturePhase, bool)>>>,
        zoom: Live<f32>,
    }

    let (inner_log, zoom) = (Rc::new(RefCell::new(Vec::new())), Live::new(1.0_f32));
    let mut rt = Runtime::new(
        App {
            consume: true,
            modal_open: false,
            inert_canvas: false,
            outer_magnifies: Vec::new(),
            inner_log: inner_log.clone(),
            zoom: zoom.clone(),
        },
        |s, _| {
            let (log, z, consume) = (s.inner_log.clone(), s.zoom.clone(), s.consume);
            let mut canvas = el()
                .key("canvas")
                .absolute(40.0, 20.0)
                .size(160.0, 140.0)
                .on_magnify_any(move |ev, tx| {
                    log.borrow_mut().push((ev.delta, ev.x, ev.y, ev.phase, ev.wheel));
                    z.set(z.get() * (1.0 + ev.delta));
                    tx.live_only();
                    consume
                });
            if s.inert_canvas { canvas = canvas.inert(); }
            el().key("outer")
                .size(300.0, 200.0)
                .on_magnify(|s: &mut App, ev, _| { s.outer_magnifies.push((ev.delta, ev.wheel)); true })
                .child(canvas)
                .children(s.modal_open.then(|| {
                    el().key("modal").absolute(0.0, 0.0).size(300.0, 200.0).modal(false, |_: &mut App| {})
                }))
        },
        (300.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.check_live_only(false);
    rt.frame(0.0);

    // 1. Phased trackpad pinch delivers local coordinates, consumes via live_only, and locks owner
    //    even after the pointer drifts outside the canvas.
    let commits_before = rt.stats.commits;
    rt.magnify_event(MagnifyEvent::new(0.2).at(100.0, 70.0).phase(GesturePhase::Began));
    rt.magnify_event(MagnifyEvent::new(0.1).at(260.0, 180.0).phase(GesturePhase::Moved));
    rt.magnify_event(MagnifyEvent::new(0.0).at(260.0, 180.0).phase(GesturePhase::Ended));
    rt.frame(1.0 / 60.0);

    assert_eq!(rt.stats.commits, commits_before, "live_only consuming on_magnify_any skips commit");
    assert_eq!(inner_log.borrow().len(), 3, "phased pinch stays locked to on_magnify_any owner");
    assert_eq!((inner_log.borrow()[0].1, inner_log.borrow()[0].2), (60.0, 50.0), "local canvas coords");
    assert!((zoom.get() - 1.32).abs() < 1e-4, "camera zoom updated: {}", zoom.get());
    assert!(rt.state.outer_magnifies.is_empty(), "consumed pinch does not bubble to outer handler");

    // 2. Ctrl + wheel reaches on_magnify_any with MagnifyEvent::wheel == true.
    inner_log.borrow_mut().clear();
    assert!(rt.pinch_wheel(100.0, 70.0, 40.0, None));
    rt.frame(1.0 / 60.0);
    assert_eq!(inner_log.borrow().len(), 1);
    assert!(inner_log.borrow()[0].4, "Ctrl+wheel arrives with wheel == true");
    assert!(inner_log.borrow()[0].0 > 0.0);

    // 3. Touchscreen two-finger pinch reaches on_magnify_any with wheel == false.
    inner_log.borrow_mut().clear();
    rt.touch(1, TouchPhase::Started, 80.0, 90.0, 0.0);
    rt.touch(2, TouchPhase::Started, 140.0, 90.0, 0.01);
    rt.touch(2, TouchPhase::Moved, 180.0, 90.0, 0.02);
    rt.touch(2, TouchPhase::Ended, 180.0, 90.0, 0.03);
    rt.touch(1, TouchPhase::Ended, 80.0, 90.0, 0.04);
    rt.frame(1.0 / 60.0);
    assert!(
        inner_log.borrow().iter().any(|e| !e.4 && e.0 > 0.0),
        "touchscreen two-finger pinch reaches on_magnify_any: {:?}",
        *inner_log.borrow(),
    );

    // 4. Declining on_magnify_any bubbles to outer typed on_magnify and commits (Tx::took clears live_only).
    rt.update(|s, _| s.consume = false);
    rt.frame(1.0 / 60.0);
    inner_log.borrow_mut().clear();
    let commits_before = rt.stats.commits;
    rt.magnify_event(MagnifyEvent::new(0.15).at(100.0, 70.0).phase(GesturePhase::Began));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.stats.commits, commits_before + 1, "declining inner live_only lets outer commit");
    assert_eq!(inner_log.borrow().len(), 1);
    assert_eq!(rt.state.outer_magnifies, vec![(0.15, false)]);

    // 5. Modal and inert guards block on_magnify_any.
    rt.update(|s, _| { s.consume = true; s.modal_open = true; });
    rt.frame(1.0 / 60.0);
    inner_log.borrow_mut().clear();
    rt.magnify_event(MagnifyEvent::new(0.2).at(100.0, 70.0).phase(GesturePhase::Began));
    rt.frame(1.0 / 60.0);
    assert!(inner_log.borrow().is_empty(), "modal blocks on_magnify_any behind it");

    rt.update(|s, _| { s.modal_open = false; s.inert_canvas = true; });
    rt.frame(1.0 / 60.0);
    rt.magnify_event(MagnifyEvent::new(0.2).at(100.0, 70.0).phase(GesturePhase::Began));
    rt.frame(1.0 / 60.0);
    assert!(inner_log.borrow().is_empty(), "inert canvas receives no on_magnify_any");
}
