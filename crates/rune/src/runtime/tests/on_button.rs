use std::cell::Cell;
use std::rc::Rc;

use super::*;
use crate::PointerButton;
use crate::el::{Cursor, ScrollSpec, text, text_input};

#[derive(Default)]
struct BubbleState {
    log: Vec<(&'static str, PointerButton, PointerButton)>,
    leaf_takes: bool,
    mid_takes: bool,
    root_takes: bool,
    secondary_presses: u32,
}

fn bubble_view(s: &BubbleState, _: &mut Cx<BubbleState>) -> El {
    let leaf_takes = s.leaf_takes;
    let mid_takes = s.mid_takes;
    let root_takes = s.root_takes;
    el().key("root")
        .size(300.0, 200.0)
        .on_button(move |s: &mut BubbleState, btn, tx| {
            s.log.push(("root", btn, tx.button()));
            root_takes
        })
        .on_secondary_press(|s: &mut BubbleState, _, _| s.secondary_presses += 1)
        .child(
            el().key("mid")
                .size(200.0, 150.0)
                .on_button(move |s: &mut BubbleState, btn, tx| {
                    s.log.push(("mid", btn, tx.button()));
                    mid_takes
                })
                .child(
                    el().key("leaf")
                        .size(100.0, 80.0)
                        .on_button(move |s: &mut BubbleState, btn, tx| {
                            s.log.push(("leaf", btn, tx.button()));
                            leaf_takes
                        }),
                ),
        )
}

#[test]
fn on_button_bubbles_innermost_first_and_stops_when_consumed() {
    let mut rt = Runtime::new(
        BubbleState { mid_takes: true, root_takes: true, ..Default::default() },
        bubble_view,
        (300.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    // Every extra button reaches leaf -> mid (returns true, stops before root).
    for btn in [PointerButton::Middle, PointerButton::Back, PointerButton::Forward, PointerButton::Other(9)] {
        rt.state.log.clear();
        rt.button_down(50.0, 40.0, btn);
        rt.button_up(50.0, 40.0, btn);
        rt.frame(0.0);
        assert_eq!(rt.state.log, [("leaf", btn, btn), ("mid", btn, btn)], "for {btn:?}");
    }

    // When leaf consumes, neither mid nor root hears it.
    rt.update(|s, _| {
        s.leaf_takes = true;
        s.log.clear();
    });
    rt.frame(0.0);
    rt.button_down(50.0, 40.0, PointerButton::Back);
    rt.button_up(50.0, 40.0, PointerButton::Back);
    rt.frame(0.0);
    assert_eq!(rt.state.log, [("leaf", PointerButton::Back, PointerButton::Back)]);

    // When none consume, all three hear it in innermost-first order.
    rt.update(|s, _| {
        s.leaf_takes = false;
        s.mid_takes = false;
        s.root_takes = false;
        s.log.clear();
    });
    rt.frame(0.0);
    rt.button_down(50.0, 40.0, PointerButton::Forward);
    rt.button_up(50.0, 40.0, PointerButton::Forward);
    rt.frame(0.0);
    assert_eq!(
        rt.state.log,
        [
            ("leaf", PointerButton::Forward, PointerButton::Forward),
            ("mid", PointerButton::Forward, PointerButton::Forward),
            ("root", PointerButton::Forward, PointerButton::Forward),
        ],
    );

    // Primary and Secondary never reach on_button.
    rt.state.log.clear();
    rt.pointer_down(50.0, 40.0);
    rt.pointer_up(50.0, 40.0);
    rt.secondary_down(50.0, 40.0);
    rt.secondary_up(50.0, 40.0);
    rt.frame(0.0);
    assert!(rt.state.log.is_empty(), "primary and secondary must never reach on_button");
    assert_eq!(rt.state.secondary_presses, 1);
}

#[test]
fn on_button_yields_to_pointer_buttons_on_middle_press_and_middle_links() {
    #[derive(Default)]
    struct PriorityState {
        log: Vec<&'static str>,
    }
    fn view(_: &PriorityState, _: &mut Cx<PriorityState>) -> El {
        col().key("root")
            .size(300.0, 240.0)
            .on_button(|s: &mut PriorityState, btn, _| {
                s.log.push(match btn {
                    PointerButton::Back => "root:back",
                    PointerButton::Middle => "root:middle",
                    _ => "root:other",
                });
                true
            })
            .child(
                el().key("opt-back")
                    .size(200.0, 50.0)
                    .pointer_buttons([PointerButton::Back])
                    .on_press_tx(|s: &mut PriorityState, _| s.log.push("press:back")),
            )
            .child(
                el().key("mid-press")
                    .size(200.0, 50.0)
                    .on_middle_press(|s: &mut PriorityState, _| s.log.push("middle-press")),
            )
            .child(
                text("open link")
                    .key("link-text")
                    .size(200.0, 20.0)
                    .link(crate::Link::new(0..9, "https://example.com"))
                    .on_link(|s: &mut PriorityState, _, tx| {
                        if tx.button() == PointerButton::Middle { s.log.push("link:middle"); }
                    }),
            )
            .child(el().key("plain").size(200.0, 50.0))
    }

    let mut rt = Runtime::new(PriorityState::default(), view, (300.0, 240.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // pointer_buttons([Back]) + on_press wins over root's on_button.
    let (x, y) = rt.find("opt-back").unwrap();
    rt.button_down(x, y, PointerButton::Back);
    rt.button_up(x, y, PointerButton::Back);
    rt.frame(0.0);
    assert_eq!(rt.state.log, ["press:back"]);

    // Unlisted button (Forward) over opt-back falls through to root's on_button.
    rt.state.log.clear();
    rt.button_down(x, y, PointerButton::Forward);
    rt.button_up(x, y, PointerButton::Forward);
    rt.frame(0.0);
    assert_eq!(rt.state.log, ["root:other"]);

    // on_middle_press wins over root's on_button.
    rt.state.log.clear();
    let (x, y) = rt.find("mid-press").unwrap();
    rt.button_down(x, y, PointerButton::Middle);
    rt.button_up(x, y, PointerButton::Middle);
    rt.frame(0.0);
    assert_eq!(rt.state.log, ["middle-press"]);

    // Middle click on link wins over root's on_button.
    rt.state.log.clear();
    let (_, y) = rt.find("link-text").unwrap();
    rt.button_down(12.0, y, PointerButton::Middle);
    rt.button_up(12.0, y, PointerButton::Middle);
    rt.frame(0.0);
    assert_eq!(rt.state.log, ["link:middle"]);

    // Plain area falls through to root's on_button.
    rt.state.log.clear();
    let (x, y) = rt.find("plain").unwrap();
    rt.button_down(x, y, PointerButton::Middle);
    rt.button_up(x, y, PointerButton::Middle);
    rt.frame(0.0);
    assert_eq!(rt.state.log, ["root:middle"]);
}

#[test]
fn on_button_middle_starts_autoscroll_only_when_declined() {
    #[derive(Default)]
    struct ScrollState {
        consume_middle: bool,
        heard: u32,
    }
    fn view(s: &ScrollState, _: &mut Cx<ScrollState>) -> El {
        let consume = s.consume_middle;
        el().key("root")
            .size(200.0, 200.0)
            .on_button(move |s: &mut ScrollState, btn, _| {
                if btn == PointerButton::Middle {
                    s.heard += 1;
                    return consume;
                }
                false
            })
            .child(
                col().key("list")
                    .size(200.0, 200.0)
                    .scroll(ScrollSpec { stick_bottom: false })
                    .child(el().w_full().h(1000.0)),
            )
    }

    // Consumed: autoscroll never starts.
    let mut rt = Runtime::new(ScrollState { consume_middle: true, heard: 0 }, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.button_down(100.0, 100.0, PointerButton::Middle);
    rt.frame(0.0);
    assert_eq!(rt.state.heard, 1);
    assert!(rt.core().autoscroll_cursor().is_none(), "consumed Middle must not start autoscroll");

    // Declined (down -> frame): falls back to autoscroll on platforms that enable it.
    let mut rt2 = Runtime::new(ScrollState { consume_middle: false, heard: 0 }, view, (200.0, 200.0), Env::new(true, 1.0));
    rt2.frame(0.0);
    rt2.button_down(100.0, 100.0, PointerButton::Middle);
    rt2.frame(0.0);
    assert_eq!(rt2.state.heard, 1);
    assert_eq!(rt2.core().autoscroll_cursor().is_some(), crate::runtime::autoscroll::ENABLED);

    // Declined (same-frame down + drag-away + up -> frame): release stops autoscroll rather than sticking.
    let mut rt3 = Runtime::new(ScrollState { consume_middle: false, heard: 0 }, view, (200.0, 200.0), Env::new(true, 1.0));
    rt3.frame(0.0);
    rt3.button_down(100.0, 100.0, PointerButton::Middle);
    rt3.button_up(100.0, 160.0, PointerButton::Middle);
    rt3.frame(0.0);
    assert_eq!(rt3.state.heard, 1);
    assert!(rt3.core().autoscroll_cursor().is_none(), "same-frame release past autoscroll radius ends autoscroll");
}

#[test]
fn on_button_respects_modal_isolation_and_inert() {
    #[derive(Default)]
    struct ScopeState {
        log: Vec<&'static str>,
        modal_open: bool,
    }
    fn view(s: &ScopeState, _: &mut Cx<ScopeState>) -> El {
        let mut root = el().key("root")
            .size(300.0, 200.0)
            .on_button(|s: &mut ScopeState, _, _| { s.log.push("root"); true })
            .child(
                el().key("inert-box")
                    .size(100.0, 100.0)
                    .inert()
                    .on_button(|s: &mut ScopeState, _, _| { s.log.push("inert"); true }),
            );
        if s.modal_open {
            root = root.child(
                el().key("sheet")
                    .absolute(150.0, 20.0)
                    .size(120.0, 120.0)
                    .modal(true, |s: &mut ScopeState| s.modal_open = false)
                    .on_button(|s: &mut ScopeState, _, _| { s.log.push("sheet"); false })
                    .child(
                        el().key("sheet-inner")
                            .size(80.0, 80.0)
                            .on_button(|s: &mut ScopeState, _, _| { s.log.push("sheet-inner"); false }),
                    ),
            );
        }
        root
    }

    let mut rt = Runtime::new(ScopeState { log: Vec::new(), modal_open: false }, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Inert child is skipped; root hears the button instead.
    rt.button_down(50.0, 50.0, PointerButton::Back);
    rt.button_up(50.0, 50.0, PointerButton::Back);
    rt.frame(0.0);
    assert_eq!(rt.state.log, ["root"]);

    // With modal open: outside click does not reach root; inside click bubbles to modal boundary only.
    rt.update(|s, tx| { s.modal_open = true; s.log.clear(); tx.instant(); });
    rt.frame(0.0);
    rt.button_down(50.0, 50.0, PointerButton::Back);
    rt.button_up(50.0, 50.0, PointerButton::Back);
    rt.frame(0.0);
    assert!(rt.state.log.is_empty(), "modal blocks on_button outside it");

    rt.button_down(170.0, 40.0, PointerButton::Forward);
    rt.button_up(170.0, 40.0, PointerButton::Forward);
    rt.frame(0.0);
    assert_eq!(rt.state.log, ["sheet-inner", "sheet"], "bubbling inside modal stops at modal boundary");
}

#[test]
fn on_button_on_root_preserves_blur_press_cursor_text_selection_and_a11y() {
    struct RootState {
        field: crate::edit::TextEdit,
        nav: i32,
        btn_clicks: u32,
    }
    fn view(s: &RootState, cx: &mut Cx<RootState>) -> El {
        cx.press_cursor(Cursor::Pointer);
        col().key("root")
            .size(300.0, 260.0)
            .on_button(|s: &mut RootState, btn, _| match btn {
                PointerButton::Back => { s.nav -= 1; true }
                PointerButton::Forward => { s.nav += 1; true }
                _ => false,
            })
            .child(
                text_input(&s.field, crate::tok::TextStyle::new(14.0, 400.0))
                    .key("field")
                    .label("Input")
                    .size(200.0, 32.0)
                    .on_edit(|s: &mut RootState| &mut s.field),
            )
            .child(
                el().key("btn")
                    .size(100.0, 32.0)
                    .on_press(|s: &mut RootState| s.btn_clicks += 1),
            )
            .child(text("selectable copy").key("sel").selectable(true))
            .child(text("plain page text").key("plain"))
    }

    let mut rt = Runtime::new(
        RootState { field: crate::edit::TextEdit::new("hello"), nav: 0, btn_clicks: 0 },
        view,
        (300.0, 260.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    // 1. Focus field, then primary-click empty space on root -> blurs field (not swallowed by root).
    let (fx, fy) = rt.find("field").unwrap();
    rt.pointer_down(fx, fy);
    rt.pointer_up(fx, fy);
    rt.frame(0.0);
    assert_eq!(rt.focused_key().as_deref(), Some("field"));

    rt.pointer_move(260.0, 220.0);
    assert_eq!(rt.cursor(), Cursor::Default, "press_cursor must NOT turn on_button root into Cursor::Pointer");
    rt.pointer_down(260.0, 220.0);
    rt.pointer_up(260.0, 220.0);
    rt.frame(0.0);
    assert_eq!(rt.focused_key(), None, "clicking empty space on an on_button root still blurs focused field");

    // 2. Child button keeps Cursor::Pointer and primary press.
    let (bx, by) = rt.find("btn").unwrap();
    rt.pointer_move(bx, by);
    assert_eq!(rt.cursor(), Cursor::Pointer);
    rt.pointer_down(bx, by);
    rt.pointer_up(bx, by);
    rt.frame(0.0);
    assert_eq!(rt.state.btn_clicks, 1);

    // 3. Selectable text inside root still receives click/drag selection.
    let (_, sy) = rt.find("sel").unwrap();
    rt.pointer_move(10.0, sy);
    assert_eq!(rt.cursor(), Cursor::Text);
    rt.pointer_down(2.0, sy);
    rt.pointer_move(120.0, sy);
    rt.pointer_up(120.0, sy);
    rt.frame(0.0);
    assert!(rt.core().has_nonempty_selection(), "selectable text inside an on_button root still selects");

    // 4. Accessibility & web mirror: root keeps default role ("group"), not focusable, and web page text stays selectable.
    let root_node = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("root")).unwrap();
    assert_eq!(root_node.role, "group");
    assert!(!root_node.focus.enabled);
    rt.core_mut().page_mode();
    let plain_i = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("plain")).unwrap();
    assert!(rt.core().page_text_selects()[plain_i], "on_button on root must not disable web page text selection");

    // 5. Back/Forward side buttons still work anywhere on root.
    rt.button_down(260.0, 220.0, PointerButton::Back);
    rt.button_up(260.0, 220.0, PointerButton::Back);
    rt.button_down(260.0, 220.0, PointerButton::Forward);
    rt.button_up(260.0, 220.0, PointerButton::Forward);
    rt.button_down(260.0, 220.0, PointerButton::Forward);
    rt.button_up(260.0, 220.0, PointerButton::Forward);
    rt.frame(0.0);
    assert_eq!(rt.state.nav, 1);
}

#[test]
fn on_button_live_only_skips_view_rebuild() {
    struct LiveState {
        live: crate::Live<f32>,
        builds: Rc<Cell<u32>>,
    }
    fn view(s: &LiveState, _: &mut Cx<LiveState>) -> El {
        s.builds.set(s.builds.get() + 1);
        el().key("root")
            .size(200.0, 200.0)
            .live_opacity(&s.live)
            .on_button(|s: &mut LiveState, btn, tx| {
                if btn == PointerButton::Back {
                    s.live.set(0.5);
                    tx.live_only();
                    return true;
                }
                false
            })
    }
    let builds = Rc::new(Cell::new(0));
    let mut rt = Runtime::new(
        LiveState { live: crate::Live::new(1.0), builds: builds.clone() },
        view,
        (200.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    let before = builds.get();
    rt.button_down(50.0, 50.0, PointerButton::Back);
    rt.button_up(50.0, 50.0, PointerButton::Back);
    rt.frame(0.0);
    assert_eq!(rt.state.live.get(), 0.5);
    assert_eq!(builds.get(), before, "live_only on_button skips view rebuild");
}
