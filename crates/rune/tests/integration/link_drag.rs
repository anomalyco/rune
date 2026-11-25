//! Issue #2389: a drag that starts and ends inside a link in static text must not fire
//! `on_link`; once pointer travel crosses the click/touch drag threshold (`CLICK_SLOP = 4.0` pt
//! per axis for mouse, `TOUCH_SLOP = 8.0` pt Euclidean for touch), the press becomes a drag —
//! dropping the link's pressed look, suppressing `on_link` on release (even if the pointer
//! returns to the start), and starting text selection when `selectable(true)`.

use rune::{
    Color, Cursor, Cx, El, Env, Key, KeyEvent, Link, LinkPaint, Mark, MarkRange, Mods,
    PillStyle, PointerButton, Runtime, ScrollSpec, TextEdit, TextStyle, TouchPhase, col, text,
    text_input,
};
use crate::support::{centre, click, find, tick, wait};

const BODY: &str = "see open the full report now";
const RANGE: std::ops::Range<usize> = 4..24;

fn pressed_bg() -> Color { rune::color::hex("#00ff55").unwrap() }
fn pill_bg() -> Color { rune::color::hex("#223344").unwrap() }

fn painted(rt: &Runtime<S>, c: Color) -> bool {
    let want = c.premul(1.0);
    rt.instances.iter().any(|i| i.fill.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3))
}

struct S {
    selectable: bool,
    with_pill: bool,
    edit: TextEdit,
    clicks: Vec<(String, PointerButton)>,
    ancestor_presses: usize,
}

impl S {
    fn new(selectable: bool, with_pill: bool) -> Self {
        let link = MarkRange::new(RANGE.clone(), Mark::Link("field-target".into()));
        let edit = TextEdit::from_parts(BODY, vec![link], vec![]);
        Self { selectable, with_pill, edit, clicks: Vec::new(), ancestor_presses: 0 }
    }
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let mut t = text(BODY)
        .key("t")
        .font(style)
        .w(260.0)
        .selectable(s.selectable)
        .link(
            Link::new(RANGE.clone(), "https://example.com/report")
                .pressed(LinkPaint::new().bg(pressed_bg())),
        )
        .on_link(|s: &mut S, id, tx| s.clicks.push((id.to_owned(), tx.button())));
    if s.with_pill {
        t = t.pill(RANGE.clone(), PillStyle::new().bg(pill_bg()));
    }
    col()
        .w_full()
        .h_full()
        .padding(10.0)
        .gap(12.0)
        .on_press(|s: &mut S| s.ancestor_presses += 1)
        .child(t)
        .child(
            text_input(&s.edit, style)
                .key("field")
                .label("Field")
                .w(260.0)
                .on_edit(|s: &mut S| &mut s.edit)
                .on_link(|s: &mut S, id, tx| s.clicks.push((id.to_owned(), tx.button()))),
        )
}

fn rig(selectable: bool, with_pill: bool) -> Runtime<S> {
    let mut rt = Runtime::new(S::new(selectable, with_pill), view, (340.0, 200.0), Env::new(true, 2.0));
    rt.set_srgb_blend(false);
    rt.frame(0.0);
    rt
}

fn drag_steps(rt: &mut Runtime<S>, from: (f32, f32), to: (f32, f32)) {
    rt.pointer_move(from.0, from.1);
    rt.pointer_down(from.0, from.1);
    tick(rt);
    for k in 1..=4 {
        let f = k as f32 / 4.0;
        rt.pointer_move(from.0 + (to.0 - from.0) * f, from.1 + (to.1 - from.1) * f);
        tick(rt);
    }
    rt.pointer_up(to.0, to.1);
    tick(rt);
}

#[test]
fn reporter_table_plain_click_drags_pill_outside_and_field() {
    // 1. Plain click on the link fires on_link and selects nothing.
    let mut rt = rig(true, false);
    let link = find(&rt, "https://example.com/report");
    let y = centre(&link).1;
    let p20 = (link.rect[0] + link.rect[2] * 0.2, y);
    let p70 = (link.rect[0] + link.rect[2] * 0.7, y);
    click(&mut rt, p20);
    assert_eq!(rt.state.clicks, [("https://example.com/report".into(), PointerButton::Primary)]);
    assert_eq!(rt.selected_text(), None);

    // 2. Drag inside the link, selectable(true): does NOT fire on_link, DOES select text.
    wait(&mut rt);
    rt.state.clicks.clear();
    drag_steps(&mut rt, p20, p70);
    assert!(rt.state.clicks.is_empty(), "drag inside selectable link must not fire on_link: {:?}", rt.state.clicks);
    assert!(rt.selected_text().is_some_and(|s| !s.is_empty()), "drag inside selectable link must select text");
    assert_eq!(rt.state.ancestor_presses, 0, "ancestor on_press must not fire on link drag");

    // 3. Drag inside the link, selectable(false): does NOT fire on_link, selects nothing.
    let mut rt = rig(false, false);
    let link = find(&rt, "https://example.com/report");
    let y = centre(&link).1;
    let p20 = (link.rect[0] + link.rect[2] * 0.2, y);
    let p70 = (link.rect[0] + link.rect[2] * 0.7, y);
    drag_steps(&mut rt, p20, p70);
    assert!(rt.state.clicks.is_empty(), "drag inside non-selectable link must not fire on_link");
    assert_eq!(rt.selected_text(), None);
    // Plain click with selectable(false) still fires.
    wait(&mut rt);
    click(&mut rt, p20);
    assert_eq!(rt.state.clicks.len(), 1);

    // 4. Drag inside the link with .pill(4..24, ..), selectable(true): does NOT fire, DOES select.
    let mut rt = rig(true, true);
    let link = find(&rt, "https://example.com/report");
    let y = centre(&link).1;
    let p20 = (link.rect[0] + link.rect[2] * 0.2, y);
    let p70 = (link.rect[0] + link.rect[2] * 0.7, y);
    drag_steps(&mut rt, p20, p70);
    assert!(rt.state.clicks.is_empty(), "drag inside pill link must not fire on_link");
    assert!(rt.selected_text().is_some_and(|s| !s.is_empty()), "drag inside pill link must select text");
    wait(&mut rt);
    click(&mut rt, p20);
    assert_eq!(rt.state.clicks.len(), 1, "plain click on pill link still fires");

    // 5. Press in the link, release just outside it: does NOT fire.
    wait(&mut rt);
    rt.state.clicks.clear();
    let edge_in = (link.rect[0] + link.rect[2] - 1.5, y);
    let just_out = (link.rect[0] + link.rect[2] + 2.0, y);
    rt.pointer_move(edge_in.0, edge_in.1);
    rt.pointer_down(edge_in.0, edge_in.1);
    tick(&mut rt);
    rt.pointer_up(just_out.0, just_out.1);
    tick(&mut rt);
    assert!(rt.state.clicks.is_empty(), "release just outside link must not fire");

    // 6. Drag inside a Mark::Link in a text_input: does NOT fire, selects text.
    let mut rt = rig(true, false);
    let fl = find(&rt, "field-target");
    let fy = centre(&fl).1;
    drag_steps(&mut rt, (fl.rect[0] + fl.rect[2] * 0.2, fy), (fl.rect[0] + fl.rect[2] * 0.7, fy));
    assert!(rt.state.clicks.is_empty());
    assert_ne!(rt.state.edit.anchor, rt.state.edit.focus);
}

#[test]
fn trembling_click_round_trip_drag_pressed_look_double_click_middle_right_and_touch() {
    // 1. Trembling click within CLICK_SLOP (4 pt per axis) still fires and keeps pressed look;
    //    crossing CLICK_SLOP drops pressed look and suppresses on_link even if pointer returns.
    for selectable in [true, false] {
        let mut rt = rig(selectable, false);
        let link = find(&rt, "https://example.com/report");
        let (x, y) = centre(&link);

        // Tremble by (3.0, 2.0) <= 4.0 pt: still pressed and still a click.
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        tick(&mut rt);
        assert!(painted(&rt, pressed_bg()) && find(&rt, "https://example.com/report").press == 1.0);
        rt.pointer_move(x + 3.0, y + 2.0);
        tick(&mut rt);
        assert!(painted(&rt, pressed_bg()) && find(&rt, "https://example.com/report").press == 1.0, "still pressed within CLICK_SLOP");
        rt.pointer_up(x + 3.0, y + 2.0);
        tick(&mut rt);
        assert_eq!(rt.state.clicks.len(), 1, "trembling click (selectable={selectable}) must fire on_link");
        assert_eq!(rt.selected_text(), None);

        // Move past CLICK_SLOP (+6 pt) then BACK to start and release: drag, no click, pressed look ends on cross.
        wait(&mut rt);
        rt.state.clicks.clear();
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        tick(&mut rt);
        assert_eq!(find(&rt, "https://example.com/report").press, 1.0);
        rt.pointer_move(x + 6.0, y);
        tick(&mut rt);
        assert!(!painted(&rt, pressed_bg()) && find(&rt, "https://example.com/report").press == 0.0, "pressed look ends when drag starts (selectable={selectable})");
        if selectable {
            assert_eq!(rt.cursor(), Cursor::Text, "cursor switches to Text while drag-selecting inside link");
        }
        rt.pointer_move(x, y);
        rt.pointer_up(x, y);
        tick(&mut rt);
        assert!(rt.state.clicks.is_empty(), "out-and-back past CLICK_SLOP must not fire on_link (selectable={selectable})");
    }

    // 2. Middle click opens link; middle drag past slop inside link does not.
    let mut rt = rig(true, false);
    let link = find(&rt, "https://example.com/report");
    let (x, y) = centre(&link);
    rt.pointer_move(x, y);
    rt.button_down(x, y, PointerButton::Middle);
    rt.pointer_move(x + 2.0, y);
    rt.button_up(x + 2.0, y, PointerButton::Middle);
    tick(&mut rt);
    assert_eq!(rt.state.clicks, [("https://example.com/report".into(), PointerButton::Middle)]);
    rt.state.clicks.clear();
    rt.pointer_move(x, y);
    rt.button_down(x, y, PointerButton::Middle);
    rt.pointer_move(x + 12.0, y);
    rt.button_up(x + 12.0, y, PointerButton::Middle);
    tick(&mut rt);
    assert!(rt.state.clicks.is_empty(), "middle-button drag inside link must not fire on_link");

    // 3. Double-click and triple-click on selectable link: click 1 opens, click 2 selects word without opening again.
    let mut rt = rig(true, false);
    let link = find(&rt, "https://example.com/report");
    let (x, y) = centre(&link);
    click(&mut rt, (x, y));
    assert_eq!(rt.state.clicks.len(), 1);
    click(&mut rt, (x, y));
    assert_eq!(rt.state.clicks.len(), 1, "double-click selects word without firing on_link twice");
    assert!(rt.selected_text().is_some_and(|s| !s.is_empty()));
    click(&mut rt, (x, y));
    assert_eq!(rt.state.clicks.len(), 1, "triple-click selects paragraph without firing on_link again");
    assert_eq!(rt.selected_text().as_deref(), Some(BODY));

    // 4. Right-click opens link menu; keyboard Enter still opens link.
    let mut rt = rig(true, false);
    let link = find(&rt, "https://example.com/report");
    let (x, y) = centre(&link);
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    tick(&mut rt);
    assert!(rt.popup_menu().and_then(|m| m.find("open-link")).is_some(), "right-click opens link menu");
    rt.resolve_popup(None).unwrap();
    tick(&mut rt);
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    tick(&mut rt);
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    tick(&mut rt);
    assert_eq!(rt.state.clicks.len(), 1, "Enter on focused link opens it");

    // 5. Drag from outside into link selects without opening; drag from link to another opens neither.
    let mut rt = rig(true, false);
    let t = find(&rt, "t");
    let link = find(&rt, "https://example.com/report");
    let y = centre(&link).1;
    drag_steps(&mut rt, (t.rect[0] + 2.0, y), centre(&link));
    assert!(rt.state.clicks.is_empty(), "drag from outside into link must not fire");
    assert!(rt.selected_text().is_some_and(|s| s.starts_with("see")));

    // 6. Touch: tap with 6 pt jitter (<= TOUCH_SLOP = 8 pt) fires on_link; drag > 8 pt does not; long press opens menu.
    let mut rt = rig(true, false);
    let link = find(&rt, "https://example.com/report");
    let (x, y) = centre(&link);
    rt.touch(1, TouchPhase::Started, x, y, 0.0);
    rt.touch(1, TouchPhase::Moved, x + 6.0, y, 0.02);
    rt.touch(1, TouchPhase::Ended, x + 6.0, y, 0.05);
    tick(&mut rt);
    assert_eq!(rt.state.clicks.len(), 1, "6 pt touch tap (<= 8 pt TOUCH_SLOP) must fire on_link");
    assert_eq!(rt.selected_text(), None);

    wait(&mut rt);
    rt.state.clicks.clear();
    let left = link.rect[0] + 4.0;
    rt.touch(2, TouchPhase::Started, left, y, 1.0);
    rt.touch(2, TouchPhase::Moved, left + 20.0, y, 1.02);
    rt.touch(2, TouchPhase::Ended, left + 20.0, y, 1.05);
    tick(&mut rt);
    assert!(rt.state.clicks.is_empty(), "touch drag > 8 pt must not fire on_link");
    assert!(rt.selected_text().is_some_and(|s| !s.is_empty()));

    wait(&mut rt);
    rt.state.clicks.clear();
    rt.touch(3, TouchPhase::Started, x, y, 2.0);
    rt.frame(0.0);
    rt.frame(0.6);
    assert!(rt.popup_menu().is_some(), "touch long press opens link menu");
    rt.resolve_popup(None).unwrap();
    rt.touch(3, TouchPhase::Ended, x, y, 2.6);
    tick(&mut rt);
    assert!(rt.state.clicks.is_empty(), "touch long press must not fire on_link");
}

#[test]
fn touch_scroll_starting_on_a_link_in_a_scroll_view_scrolls_without_firing_on_link() {
    #[derive(Default)]
    struct Sc { clicks: Vec<String> }
    fn sc_view(_: &Sc, _: &mut Cx<Sc>) -> El {
        let style = TextStyle::new(14.0, 400.0).lh(20.0);
        let mut inner = col().w_full().gap(8.0);
        for k in 0..20 {
            inner = inner.child(
                text(BODY)
                    .key(format!("row-{k}"))
                    .font(style)
                    .w(260.0)
                    .selectable(true)
                    .link(Link::new(RANGE.clone(), format!("https://example.com/{k}")))
                    .on_link(|s: &mut Sc, id, _| s.clicks.push(id.to_owned())),
            );
        }
        col().key("sv").w(300.0).h(120.0).scroll(ScrollSpec { stick_bottom: false }).child(inner)
    }
    let mut rt = Runtime::new(Sc::default(), sc_view, (300.0, 120.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let link = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("https://example.com/1")).unwrap();
    let (x, y) = centre(&link);
    rt.touch(1, TouchPhase::Started, x, y, 0.0);
    rt.touch(1, TouchPhase::Moved, x, y - 40.0, 0.02);
    rt.touch(1, TouchPhase::Ended, x, y - 40.0, 0.05);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.clicks.is_empty(), "touch scroll starting on a link must not fire on_link");
    assert_eq!(rt.selected_text(), None, "touch scroll starting on a link must not select text");
}

#[test]
fn link_to_link_drag_and_shift_click_on_selectable_link() {
    #[derive(Default)]
    struct Two { clicks: Vec<String> }
    fn two_view(_: &Two, _: &mut Cx<Two>) -> El {
        let style = TextStyle::new(14.0, 400.0).lh(20.0);
        col().w_full().h_full().padding(10.0).child(
            text("lead first-link and second-link tail")
                .key("t")
                .font(style)
                .w(320.0)
                .selectable(true)
                .link(Link::new(5..15, "first"))
                .link(Link::new(20..31, "second"))
                .on_link(|s: &mut Two, id, _| s.clicks.push(id.to_owned())),
        )
    }
    let mut rt = Runtime::new(Two::default(), two_view, (360.0, 120.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let t = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("t")).unwrap();
    let first = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("first")).unwrap();
    let second = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("second")).unwrap();

    // Drag from first link into second link opens neither and selects across both.
    let (ax, ay) = centre(&first);
    let (bx, by) = centre(&second);
    rt.pointer_move(ax, ay);
    rt.pointer_down(ax, ay);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(bx, by);
    rt.frame(1.0 / 60.0);
    rt.pointer_up(bx, by);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.clicks.is_empty(), "drag from one link to another must open neither");
    assert!(rt.selected_text().is_some_and(|s| s.contains("and")), "drag across two links selects text: {:?}", rt.selected_text());

    // Shift+click on a link extends selection from plain text without firing on_link.
    rt.frame(1.0);
    let plain = (t.rect[0] + 0.2, ay);
    rt.pointer_move(plain.0, plain.1);
    rt.pointer_down(plain.0, plain.1);
    rt.pointer_up(plain.0, plain.1);
    rt.frame(1.0 / 60.0);
    rt.physical_input(rune::PhysicalInput::Modifiers(Mods { shift: true, ..Mods::default() }));
    rt.pointer_move(ax, ay);
    rt.pointer_down(ax, ay);
    rt.pointer_up(ax, ay);
    rt.frame(1.0 / 60.0);
    rt.physical_input(rune::PhysicalInput::Modifiers(Mods::default()));
    assert!(rt.state.clicks.is_empty(), "Shift+click extending selection into link must not open it");
    assert!(rt.selected_text().is_some_and(|s| s.starts_with("lead")), "Shift+click extends selection: {:?}", rt.selected_text());
}

#[test]
fn threshold_boundaries_touch_out_and_back_and_text_input_touch_tap() {
    // 1. Mouse exact boundary around CLICK_SLOP = 4.0 pt: no dead band and no click+select overlap.
    let mut rt = rig(true, false);
    let link = find(&rt, "https://example.com/report");
    let y = centre(&link).1;
    let start_x = link.rect[0] + 10.0;

    // Exactly 4.0 pt (<= CLICK_SLOP): clicks link, selects nothing.
    rt.pointer_move(start_x, y);
    rt.pointer_down(start_x, y);
    rt.pointer_move(start_x + 4.0, y);
    rt.pointer_up(start_x + 4.0, y);
    tick(&mut rt);
    assert_eq!(rt.state.clicks.len(), 1, "at 4.0 pt boundary must still click");
    assert_eq!(rt.selected_text(), None, "at 4.0 pt boundary must not select");

    // 4.5 pt (> CLICK_SLOP, enough to cross a glyph): selects text, does not click.
    wait(&mut rt);
    rt.state.clicks.clear();
    rt.pointer_move(start_x, y);
    rt.pointer_down(start_x, y);
    rt.pointer_move(start_x + 4.5, y);
    rt.pointer_up(start_x + 4.5, y);
    tick(&mut rt);
    assert!(rt.state.clicks.is_empty(), "past 4.0 pt must not click");
    assert!(rt.selected_text().is_some_and(|s| !s.is_empty()), "past 4.0 pt must select text (no dead band)");

    // 2. Touch exact boundary around TOUCH_SLOP = 8.0 pt: 8.0 taps, 8.5 drags/selects.
    wait(&mut rt);
    rt.state.clicks.clear();
    rt.touch(1, TouchPhase::Started, start_x, y, 10.0);
    rt.touch(1, TouchPhase::Moved, start_x + 8.0, y, 10.02);
    rt.touch(1, TouchPhase::Ended, start_x + 8.0, y, 10.04);
    tick(&mut rt);
    assert_eq!(rt.state.clicks.len(), 1, "at 8.0 pt touch boundary must still tap");
    assert_eq!(rt.selected_text(), None, "at 8.0 pt touch boundary must not select");

    wait(&mut rt);
    rt.state.clicks.clear();
    rt.touch(2, TouchPhase::Started, start_x, y, 20.0);
    rt.touch(2, TouchPhase::Moved, start_x + 8.5, y, 20.02);
    rt.touch(2, TouchPhase::Ended, start_x + 8.5, y, 20.04);
    tick(&mut rt);
    assert!(rt.state.clicks.is_empty(), "past 8.0 pt touch must not tap");
    assert!(rt.selected_text().is_some_and(|s| !s.is_empty()), "past 8.0 pt touch must select (no dead band)");

    // 3. Touch out-and-back drag (25 pt out past TOUCH_SLOP, then back within 2 pt of start):
    //    must NOT fire on_link (repairs #2390's release-only touch distance check).
    wait(&mut rt);
    rt.state.clicks.clear();
    rt.touch(3, TouchPhase::Started, start_x, y, 30.0);
    rt.touch(3, TouchPhase::Moved, start_x + 25.0, y, 30.02);
    rt.touch(3, TouchPhase::Moved, start_x + 2.0, y, 30.04);
    rt.touch(3, TouchPhase::Ended, start_x + 2.0, y, 30.06);
    tick(&mut rt);
    assert!(rt.state.clicks.is_empty(), "touch drag out past 8 pt and back within 2 pt must not fire on_link");

    // 4. Touch tap with 6 pt wobble on a Mark::Link inside text_input still opens the link.
    wait(&mut rt);
    rt.state.clicks.clear();
    let fl = find(&rt, "field-target");
    let fy = centre(&fl).1;
    let fx = fl.rect[0] + fl.rect[2] * 0.4;
    rt.touch(4, TouchPhase::Started, fx, fy, 40.0);
    rt.touch(4, TouchPhase::Moved, fx + 6.0, fy, 40.02);
    rt.touch(4, TouchPhase::Ended, fx + 6.0, fy, 40.04);
    tick(&mut rt);
    assert_eq!(rt.state.clicks, [("field-target".into(), PointerButton::Primary)], "6 pt touch tap on text_input link must open it");
}
