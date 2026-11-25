use std::cell::Cell;
use std::rc::Rc;

use super::*;
use crate::el::{Cursor, text_input};
use crate::{Live, PointerButton, PointerEvent, TextEdit, TextStyle, Transition};

#[derive(Default)]
struct ButtonLogState {
    seen: Vec<(PointerButton, PointerButton, PointerEvent)>,
    zoom: f32,
}

fn button_log_view(s: &ButtonLogState, _: &mut Cx<ButtonLogState>) -> El {
    let z = if s.zoom > 0.0 { s.zoom } else { 1.0 };
    col().key("root").size(300.0, 200.0).child(
        el().key("observer")
            .absolute(40.0, 25.0)
            .size(100.0, 60.0)
            .origin(0.0, 0.0)
            .scale(z)
            .on_any_press(|s: &mut ButtonLogState, btn, p, tx| {
                s.seen.push((btn, tx.button(), p));
            }),
    )
}

#[test]
fn on_any_press_observes_all_buttons_and_reports_local_and_window_coords() {
    let mut rt = Runtime::new(
        ButtonLogState { zoom: 1.0, ..Default::default() },
        button_log_view,
        (300.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    assert!(rt.core().tree.has_any_press);

    let cases = [
        (PointerButton::Primary, (60.0, 45.0), (20.0, 20.0)),
        (PointerButton::Secondary, (10.0, 15.0), (-30.0, -10.0)),
        (PointerButton::Middle, (200.0, 120.0), (160.0, 95.0)),
        (PointerButton::Back, (40.0, 25.0), (0.0, 0.0)),
        (PointerButton::Forward, (140.0, 85.0), (100.0, 60.0)),
        (PointerButton::Other(9), (250.0, 180.0), (210.0, 155.0)),
    ];

    for (btn, (wx, wy), (lx, ly)) in cases {
        rt.state.seen.clear();
        rt.button_down(wx, wy, btn);
        rt.button_up(wx, wy, btn);
        rt.frame(0.0);
        assert_eq!(rt.state.seen.len(), 1, "expected 1 observation for {btn:?}");
        let (got_btn, tx_btn, p) = rt.state.seen[0];
        assert_eq!(got_btn, btn);
        assert_eq!(tx_btn, btn);
        assert!(p.pressing, "on_any_press must report pressing == true for {btn:?}");
        assert!((p.window_x - wx).abs() < 0.01 && (p.window_y - wy).abs() < 0.01);
        assert!((p.x - lx).abs() < 0.01 && (p.y - ly).abs() < 0.01);
        assert_eq!(p.window(), PointerPress { x: wx, y: wy });
    }

    // Under 2x element scale, local coordinates divide by scale while window coordinates stay in window points.
    rt.update(|s, tx| {
        s.zoom = 2.0;
        s.seen.clear();
        tx.instant();
    });
    rt.frame(0.0);
    rt.pointer_down(120.0, 85.0);
    rt.pointer_up(120.0, 85.0);
    rt.frame(0.0);
    assert_eq!(rt.state.seen.len(), 1);
    let (_, _, p) = rt.state.seen[0];
    assert!((p.window_x - 120.0).abs() < 0.01 && (p.window_y - 85.0).abs() < 0.01);
    assert!((p.x - 40.0).abs() < 0.01 && (p.y - 30.0).abs() < 0.01, "got local ({}, {})", p.x, p.y);
}

#[derive(Default)]
struct OrderState {
    events: Vec<&'static str>,
}

fn order_view(_: &OrderState, _: &mut Cx<OrderState>) -> El {
    col().key("root")
        .size(300.0, 200.0)
        .on_any_press(|s: &mut OrderState, _, _, _| s.events.push("obs:root"))
        .child(
            col().key("panel")
                .size(200.0, 120.0)
                .on_any_press(|s: &mut OrderState, _, _, _| s.events.push("obs:panel"))
                .child(
                    el().key("btn")
                        .size(100.0, 40.0)
                        .press_on_down()
                        .on_drag(|s: &mut OrderState, d, _| {
                            if d.phase == Phase::Down { s.events.push("drag:down"); }
                            if d.phase == Phase::Up { s.events.push("drag:up"); }
                        })
                        .on_press(|s: &mut OrderState| s.events.push("target:press"))
                        .on_button(|s: &mut OrderState, _, _| {
                            s.events.push("target:button");
                            true
                        }),
                ),
        )
        .child(
            el().key("popup")
                .absolute(180.0, 10.0)
                .size(100.0, 60.0)
                .on_any_press(|s: &mut OrderState, _, _, _| s.events.push("obs:popup")),
        )
}

#[test]
fn on_any_press_runs_topmost_first_and_before_target_actions() {
    let mut rt = Runtime::new(OrderState::default(), order_view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let (bx, by) = rt.find("btn").unwrap();
    rt.pointer_down(bx, by);
    rt.pointer_up(bx, by);
    rt.frame(0.0);
    assert_eq!(
        rt.state.events,
        ["obs:popup", "obs:panel", "obs:root", "drag:down", "target:press", "drag:up"],
        "observers run in reverse tree order before the press's drag/press actions",
    );

    rt.state.events.clear();
    rt.button_down(bx, by, PointerButton::Back);
    rt.button_up(bx, by, PointerButton::Back);
    rt.frame(0.0);
    assert_eq!(
        rt.state.events,
        ["obs:popup", "obs:panel", "obs:root", "target:button"],
        "observers run in reverse tree order before El::on_button",
    );
}

struct PopupState {
    popup_open: bool,
    combo_open: bool,
    action_clicks: u32,
    keep_focus_clicks: u32,
    field: TextEdit,
    combo: TextEdit,
}

impl Default for PopupState {
    fn default() -> Self {
        Self {
            popup_open: true,
            combo_open: false,
            action_clicks: 0,
            keep_focus_clicks: 0,
            field: TextEdit::new("hello"),
            combo: TextEdit::new("apple"),
        }
    }
}

fn popup_view(s: &PopupState, _: &mut Cx<PopupState>) -> El {
    let style = TextStyle::new(14.0, 400.0);
    col().key("root").size(400.0, 240.0).gap(8.0)
        .child(
            el().key("action-btn")
                .size(120.0, 32.0)
                .on_press(|s: &mut PopupState| s.action_clicks += 1),
        )
        .child(
            el().key("keep-focus-btn")
                .size(120.0, 32.0)
                .preserve_focus_on_press(true)
                .on_press(|s: &mut PopupState| s.keep_focus_clicks += 1),
        )
        .child(
            text_input(&s.field, style)
                .key("field")
                .size(160.0, 28.0)
                .on_edit(|s: &mut PopupState| &mut s.field),
        )
        .child(
            text_input(&s.combo, style)
                .key("combo")
                .size(160.0, 28.0)
                .on_edit(|s: &mut PopupState| &mut s.combo)
                .on_any_press(|s: &mut PopupState, btn, p, _| {
                    if btn == PointerButton::Primary
                        && p.x >= 0.0 && p.y >= 0.0 && p.x <= 160.0 && p.y <= 28.0
                    {
                        s.combo_open = true;
                    }
                }),
        )
        .children(s.popup_open.then(|| {
            el().key("popup")
                .absolute(220.0, 10.0)
                .size(150.0, 80.0)
                .on_any_press(|s: &mut PopupState, _, p, _| {
                    if p.x < 0.0 || p.y < 0.0 || p.x > 150.0 || p.y > 80.0 {
                        s.popup_open = false;
                    }
                })
        }))
}

#[test]
fn on_any_press_dismisses_popup_without_consuming_click_or_stealing_focus() {
    let mut rt = Runtime::new(PopupState::default(), popup_view, (400.0, 240.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Clicking inside the popup keeps it open.
    rt.pointer_down(260.0, 40.0);
    rt.pointer_up(260.0, 40.0);
    rt.frame(0.0);
    assert!(rt.state.popup_open);

    // Clicking an underlying button dismisses the popup AND runs on_press in one click.
    let (ax, ay) = rt.find("action-btn").unwrap();
    rt.pointer_down(ax, ay);
    rt.pointer_up(ax, ay);
    rt.frame(0.0);
    assert!(!rt.state.popup_open);
    assert_eq!(rt.state.action_clicks, 1);

    // Focus the text field and reopen the popup.
    let (fx, fy) = rt.find("field").unwrap();
    rt.pointer_down(fx, fy);
    rt.pointer_up(fx, fy);
    rt.update(|s, _| s.popup_open = true);
    rt.frame(0.0);
    assert_eq!(rt.focused_key().as_deref(), Some("field"));
    assert!(rt.state.popup_open);

    // Pressing a preserve_focus_on_press button dismisses the popup, clicks the button,
    // and leaves focus in the text field.
    let (kx, ky) = rt.find("keep-focus-btn").unwrap();
    rt.pointer_down(kx, ky);
    rt.pointer_up(kx, ky);
    rt.frame(0.0);
    assert!(!rt.state.popup_open);
    assert_eq!(rt.state.keep_focus_clicks, 1);
    assert_eq!(rt.focused_key().as_deref(), Some("field"));

    // Secondary and middle presses on empty space also dismiss the popup without moving focus.
    for btn in [PointerButton::Secondary, PointerButton::Middle] {
        rt.update(|s, _| s.popup_open = true);
        rt.frame(0.0);
        rt.button_down(380.0, 220.0, btn);
        rt.button_up(380.0, 220.0, btn);
        rt.frame(0.0);
        assert!(!rt.state.popup_open, "{btn:?} on empty space should dismiss popup");
        assert_eq!(rt.focused_key().as_deref(), Some("field"));
    }

    // Clicking near the start of an editable combobox's text_input opens its list via
    // on_any_press while the editor still takes focus and places the caret where the click landed.
    assert_eq!(rt.state.combo.focus, "apple".len());
    let combo_rect = rt.core().node("root/combo").unwrap().target;
    let click_x = combo_rect[0] + 1.0;
    let click_y = (combo_rect[1] + combo_rect[3]) * 0.5;
    rt.pointer_down(click_x, click_y);
    rt.pointer_up(click_x, click_y);
    rt.frame(0.0);
    assert!(rt.state.combo_open);
    assert_eq!(rt.focused_key().as_deref(), Some("combo"));
    assert_eq!(rt.state.combo.focus, 0, "editor must still place the caret at the click position");
    assert_eq!(rt.state.combo.anchor, 0);
}

#[derive(Default)]
struct ChordState {
    seen: Vec<PointerButton>,
}

fn chord_view(_: &ChordState, _: &mut Cx<ChordState>) -> El {
    el().key("root")
        .size(200.0, 200.0)
        .on_drag(|_: &mut ChordState, _, _| {})
        .on_any_press(|s: &mut ChordState, btn, _, _| s.seen.push(btn))
}

#[test]
fn on_any_press_suppresses_chorded_button_downs_until_all_released_or_cancelled() {
    let mut rt = Runtime::new(ChordState::default(), chord_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Secondary held down -> chorded Primary and Middle must NOT fire observers.
    rt.secondary_down(50.0, 50.0);
    rt.pointer_down(50.0, 50.0);
    rt.button_down(50.0, 50.0, PointerButton::Middle);
    rt.frame(0.0);
    assert_eq!(rt.state.seen, [PointerButton::Secondary]);

    // Releasing Primary and Middle while Secondary is still held -> Back still suppressed.
    rt.pointer_up(50.0, 50.0);
    rt.button_up(50.0, 50.0, PointerButton::Middle);
    rt.button_down(50.0, 50.0, PointerButton::Back);
    rt.button_up(50.0, 50.0, PointerButton::Back);
    rt.frame(0.0);
    assert_eq!(rt.state.seen, [PointerButton::Secondary]);

    // Releasing Secondary ends the gesture -> next Primary fires normally.
    rt.secondary_up(50.0, 50.0);
    rt.pointer_down(50.0, 50.0);
    rt.frame(0.0);
    assert_eq!(rt.state.seen, [PointerButton::Secondary, PointerButton::Primary]);

    // While Primary holds drag capture, Secondary and Forward are suppressed.
    rt.secondary_down(50.0, 50.0);
    rt.button_down(50.0, 50.0, PointerButton::Forward);
    rt.frame(0.0);
    assert_eq!(rt.state.seen, [PointerButton::Secondary, PointerButton::Primary]);

    // cancel_pointer clears all held buttons -> next Forward fires.
    rt.cancel_pointer();
    rt.button_down(50.0, 50.0, PointerButton::Forward);
    rt.button_up(50.0, 50.0, PointerButton::Forward);
    rt.frame(0.0);
    assert_eq!(rt.state.seen, [PointerButton::Secondary, PointerButton::Primary, PointerButton::Forward]);

    // Chording two distinct Other(n) buttons keeps gesture held until both release.
    rt.state.seen.clear();
    rt.button_down(50.0, 50.0, PointerButton::Other(6));
    rt.button_down(50.0, 50.0, PointerButton::Other(7));
    rt.button_up(50.0, 50.0, PointerButton::Other(6));
    rt.pointer_down(50.0, 50.0);
    rt.pointer_up(50.0, 50.0);
    rt.frame(0.0);
    assert_eq!(rt.state.seen, [PointerButton::Other(6)]);

    rt.button_up(50.0, 50.0, PointerButton::Other(7));
    rt.pointer_down(50.0, 50.0);
    rt.pointer_up(50.0, 50.0);
    rt.frame(0.0);
    assert_eq!(rt.state.seen, [PointerButton::Other(6), PointerButton::Primary]);
}

#[derive(Default)]
struct ScopeState {
    show_exiting: bool,
    show_modal: bool,
    events: Vec<&'static str>,
}

fn scope_view(s: &ScopeState, _: &mut Cx<ScopeState>) -> El {
    col().key("root")
        .size(300.0, 200.0)
        .on_any_press(|s: &mut ScopeState, _, _, _| s.events.push("obs:behind"))
        .child(
            el().key("inert-branch")
                .size(80.0, 40.0)
                .inert()
                .on_any_press(|s: &mut ScopeState, _, _, _| s.events.push("obs:inert")),
        )
        .children(s.show_exiting.then(|| {
            el().key("exiting-obs")
                .size(80.0, 40.0)
                .transition(Transition::opacity())
                .on_any_press(|s: &mut ScopeState, _, _, _| s.events.push("obs:exiting"))
        }))
        .children(s.show_modal.then(|| {
            col().key("modal")
                .absolute(80.0, 40.0)
                .size(140.0, 100.0)
                .modal(true, |s: &mut ScopeState| {
                    s.events.push("modal:dismiss");
                    s.show_modal = false;
                })
                .on_any_press(|s: &mut ScopeState, _, _, _| s.events.push("obs:modal-root"))
                .child(
                    el().key("modal-child")
                        .size(80.0, 40.0)
                        .on_any_press(|s: &mut ScopeState, _, _, _| s.events.push("obs:modal-child")),
                )
        }))
}

#[test]
fn on_any_press_excludes_inert_exiting_and_behind_modal_but_notifies_inside_modal() {
    let mut rt = Runtime::new(
        ScopeState { show_exiting: true, show_modal: false, events: Vec::new() },
        scope_view,
        (300.0, 200.0),
        Env::new(false, 1.0),
    );
    rt.frame(0.0);

    // Inert observer never fires; live root and live exiting-obs both fire.
    rt.pointer_down(10.0, 10.0);
    rt.pointer_up(10.0, 10.0);
    rt.frame(0.0);
    assert_eq!(rt.state.events, ["obs:exiting", "obs:behind"]);

    // Unmount exiting-obs so it enters its exit transition: while exiting it must NOT fire.
    rt.state.events.clear();
    rt.update(|s, _| s.show_exiting = false);
    rt.frame(0.016);
    rt.pointer_down(10.0, 10.0);
    rt.pointer_up(10.0, 10.0);
    rt.frame(0.016);
    assert_eq!(rt.state.events, ["obs:behind"]);

    // Open a modal: behind-modal observers are excluded; inside-modal observers run topmost-first
    // BEFORE modal outside-click dismissal (for both primary and secondary outside presses).
    rt.update(|s, _| {
        s.show_modal = true;
        s.events.clear();
    });
    rt.frame(0.0);
    rt.pointer_down(10.0, 10.0);
    rt.pointer_up(10.0, 10.0);
    rt.frame(0.0);
    assert_eq!(rt.state.events, ["obs:modal-child", "obs:modal-root", "modal:dismiss"]);
    assert!(!rt.state.show_modal);

    // Reopen modal and test inside-modal press (no dismiss) and secondary outside press.
    rt.update(|s, _| {
        s.show_modal = true;
        s.events.clear();
    });
    rt.frame(0.0);
    rt.pointer_down(100.0, 60.0);
    rt.pointer_up(100.0, 60.0);
    rt.frame(0.0);
    assert_eq!(rt.state.events, ["obs:modal-child", "obs:modal-root"]);
    assert!(rt.state.show_modal);

    rt.state.events.clear();
    rt.secondary_down(10.0, 10.0);
    rt.secondary_up(10.0, 10.0);
    rt.frame(0.0);
    assert_eq!(rt.state.events, ["obs:modal-child", "obs:modal-root", "modal:dismiss"]);
}

struct PassiveState {
    mount_observer: bool,
    under_clicks: u32,
    live_counter: Live<f32>,
    views: Rc<Cell<u32>>,
}

fn passive_view(s: &PassiveState, cx: &mut Cx<PassiveState>) -> El {
    cx.press_cursor(Cursor::Pointer);
    s.views.set(s.views.get() + 1);
    let live = s.live_counter.clone();
    col().key("root").size(200.0, 120.0)
        .child(
            el().key("under")
                .absolute(10.0, 10.0)
                .size(80.0, 40.0)
                .on_press(|s: &mut PassiveState| s.under_clicks += 1),
        )
        .children(s.mount_observer.then(move || {
            let live = live.clone();
            el().key("passive-cover")
                .absolute(0.0, 0.0)
                .size(200.0, 120.0)
                .on_any_press(move |_: &mut PassiveState, _, _, tx| {
                    live.set(live.get() + 1.0);
                    tx.live_only();
                })
        }))
}

#[test]
fn on_any_press_is_passive_tracks_has_any_press_flag_and_supports_live_only() {
    let views = Rc::new(Cell::new(0));
    let live = Live::new(0.0);
    let mut rt = Runtime::new(
        PassiveState { mount_observer: false, under_clicks: 0, live_counter: live.clone(), views: views.clone() },
        passive_view,
        (200.0, 120.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    assert!(!rt.core().tree.has_any_press, "flag stays false when no observer is mounted");

    rt.update(|s, _| s.mount_observer = true);
    rt.frame(0.0);
    assert!(rt.core().tree.has_any_press, "flag turns true when observer mounts");

    // Passive observer does not change cursor, does not become a button, and does not block hits
    // to the element underneath it.
    rt.pointer_move(150.0, 80.0);
    assert_eq!(rt.cursor(), Cursor::Default, "observer alone must not trigger press_cursor");
    rt.pointer_move(30.0, 25.0);
    assert_eq!(rt.cursor(), Cursor::Pointer, "element underneath still receives hover and press_cursor");

    let node = rt.core().node("root/passive-cover").unwrap();
    assert_ne!(node.role, "button");
    assert!(!crate::runtime::web_a11y::takes_pointer(node));

    // Clicking empty space fires live_only observer without rebuilding view.
    let views_before = views.get();
    rt.pointer_down(150.0, 80.0);
    rt.pointer_up(150.0, 80.0);
    rt.frame(0.0);
    assert_eq!(live.get(), 1.0);
    assert_eq!(views.get(), views_before, "live_only on_any_press skips view rebuild");

    // Clicking over the element underneath reaches it through the covering observer.
    rt.pointer_down(30.0, 25.0);
    rt.pointer_up(30.0, 25.0);
    rt.frame(0.0);
    assert_eq!(rt.state.under_clicks, 1);
    assert_eq!(live.get(), 2.0);

    // Unmounting observer resets has_any_press back to false.
    rt.update(|s, _| s.mount_observer = false);
    rt.frame(0.0);
    assert!(!rt.core().tree.has_any_press, "flag resets to false after observer unmounts");
}

struct PressingState {
    show_field: bool,
    field: TextEdit,
    move_pressing: Vec<bool>,
    secondary_pressing: Vec<bool>,
}

impl Default for PressingState {
    fn default() -> Self {
        Self {
            show_field: true,
            field: TextEdit::new("edit me"),
            move_pressing: Vec::new(),
            secondary_pressing: Vec::new(),
        }
    }
}

fn pressing_view(s: &PressingState, _: &mut Cx<PressingState>) -> El {
    col().key("area")
        .size(300.0, 200.0)
        .on_pointer_move_local(|s: &mut PressingState, p, _| s.move_pressing.push(p.pressing))
        .on_secondary_press_local(|s: &mut PressingState, p, _| s.secondary_pressing.push(p.pressing))
        .children(s.show_field.then(|| {
            text_input(&s.field, TextStyle::new(14.0, 400.0))
                .key("field")
                .size(160.0, 32.0)
                .on_edit(|s: &mut PressingState| &mut s.field)
        }))
}

#[test]
fn pointer_event_pressing_tracks_held_buttons_across_text_inputs_unmount_and_cancel() {
    assert!(!PointerEvent::default().pressing);

    let mut rt = Runtime::new(PressingState::default(), pressing_view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Resting move reports pressing == false.
    rt.pointer_move(20.0, 15.0);
    rt.frame(0.0);
    assert_eq!(rt.state.move_pressing, [false]);

    // Primary press inside a text_input sets pressing == true during moves, and false after release.
    rt.state.move_pressing.clear();
    rt.pointer_down(20.0, 15.0);
    rt.pointer_move(30.0, 15.0);
    rt.frame(0.0);
    assert_eq!(rt.state.move_pressing, [true]);

    rt.state.move_pressing.clear();
    rt.pointer_up(30.0, 15.0);
    rt.pointer_move(40.0, 15.0);
    rt.frame(0.0);
    assert!(rt.state.move_pressing.iter().all(|&p| !p) && !rt.state.move_pressing.is_empty());

    // Unmounting the text_input while Primary is held: pressing stays true while held,
    // and returns to false after release even though the field is gone.
    rt.state.move_pressing.clear();
    rt.pointer_down(20.0, 15.0);
    rt.update(|s, _| s.show_field = false);
    rt.frame(0.0);
    assert!(rt.find("field").is_none(), "field must be unmounted");
    rt.pointer_move(25.0, 15.0);
    rt.frame(0.0);
    assert_eq!(rt.state.move_pressing, [true], "pressing stays true while button held after target unmounts");

    rt.state.move_pressing.clear();
    rt.pointer_up(25.0, 15.0);
    rt.pointer_move(35.0, 15.0);
    rt.frame(0.0);
    assert!(rt.state.move_pressing.iter().all(|&p| !p) && !rt.state.move_pressing.is_empty(),
        "pressing clears to false on release even after target unmounted");

    // Secondary, Middle, Back, Forward, and Other buttons set pressing == true while held
    // and false after release.
    for btn in [
        PointerButton::Secondary,
        PointerButton::Middle,
        PointerButton::Back,
        PointerButton::Forward,
        PointerButton::Other(12),
    ] {
        rt.state.move_pressing.clear();
        rt.button_down(180.0, 120.0, btn);
        rt.pointer_move(185.0, 120.0);
        rt.frame(0.0);
        assert_eq!(rt.state.move_pressing, [true], "pressing should be true while {btn:?} held");

        rt.state.move_pressing.clear();
        rt.button_up(185.0, 120.0, btn);
        rt.pointer_move(190.0, 120.0);
        rt.frame(0.0);
        assert!(rt.state.move_pressing.iter().all(|&p| !p) && !rt.state.move_pressing.is_empty(),
            "pressing should be false after {btn:?} released");
    }
    assert_eq!(rt.state.secondary_pressing, [false], "secondary release sees pressing == false");

    // Chording Primary + Secondary keeps pressing == true until BOTH are released.
    rt.state.move_pressing.clear();
    rt.pointer_down(180.0, 120.0);
    rt.secondary_down(180.0, 120.0);
    rt.pointer_up(180.0, 120.0);
    rt.pointer_move(184.0, 120.0);
    rt.frame(0.0);
    assert!(rt.state.move_pressing.iter().all(|&p| p) && !rt.state.move_pressing.is_empty());

    rt.state.move_pressing.clear();
    rt.secondary_up(184.0, 120.0);
    rt.pointer_move(188.0, 120.0);
    rt.frame(0.0);
    assert!(rt.state.move_pressing.iter().all(|&p| !p) && !rt.state.move_pressing.is_empty());

    // cancel_pointer clears pressing back to false immediately.
    rt.state.move_pressing.clear();
    rt.pointer_down(180.0, 120.0);
    rt.cancel_pointer();
    rt.pointer_move(192.0, 120.0);
    rt.frame(0.0);
    assert_eq!(rt.state.move_pressing, [false]);
}

#[test]
fn on_any_press_and_pressing_clean_up_across_touch_tap_and_hold() {
    #[derive(Default)]
    struct TouchState {
        observed: Vec<PointerButton>,
        moves: Vec<bool>,
    }
    fn view(_: &TouchState, _: &mut Cx<TouchState>) -> El {
        el().key("root")
            .size(200.0, 200.0)
            .on_pointer_move_local(|s: &mut TouchState, p, _| s.moves.push(p.pressing))
            .on_secondary_press(|_: &mut TouchState, _, _| {})
            .on_any_press(|s: &mut TouchState, btn, _, _| s.observed.push(btn))
    }

    let mut rt = Runtime::new(TouchState::default(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Touch tap fires on_any_press(Primary) and clears pressing on lift.
    rt.touch(1, crate::TouchPhase::Started, 50.0, 50.0, 0.0);
    rt.touch(1, crate::TouchPhase::Ended, 50.0, 50.0, 0.05);
    rt.frame(0.0);
    assert_eq!(rt.state.observed, [PointerButton::Primary]);

    // Touch long-press transitions into Mode::Menu and lifts without pointer_up; pressing must
    // still clear so subsequent pointer moves see false and the next press fires on_any_press.
    rt.state.observed.clear();
    rt.touch(2, crate::TouchPhase::Started, 60.0, 60.0, 0.1);
    rt.frame(0.0);
    rt.frame(0.6);
    rt.touch(2, crate::TouchPhase::Ended, 60.0, 60.0, 0.75);
    rt.state.moves.clear();
    rt.pointer_move(70.0, 70.0);
    rt.frame(0.0);
    assert_eq!(rt.state.moves, [false], "touch long-press lift must clear Primary button bit");
    assert_eq!(rt.state.observed, [PointerButton::Primary]);

    rt.pointer_down(70.0, 70.0);
    rt.pointer_up(70.0, 70.0);
    rt.frame(0.0);
    assert_eq!(rt.state.observed, [PointerButton::Primary, PointerButton::Primary]);
}
