use rune::{Cx, Env, Runtime};

use super::*;
use crate::Knob;
use crate::rows::{Shape, shape};

struct T {
    knobs: Knobs,
}
const L: Knob = Knob::new("Left size", 0.0, 10.0, 5.0).label("Left column size");
const C: Knob = Knob::choice("Style", &["One", "Two", "Three"], 0).hints(&["The first", "The second", ""]);
const R: Knob = Knob::new("Right pad", 0.0, 40.0, 20.0);
const GROUPS: &[Group] = &[Group::new("Left", "left", &[L, C]), Group::new("Right", "right", &[R])];

fn view(s: &T, _: &mut Cx<T>) -> El {
    let page = row().key("cols").w_full().h_full().child(col().key("left").w(300.0).h_full()).child(col().key("right").grow(1.0).h_full());
    side(page, &s.knobs, GROUPS, Style::dark(), |s: &mut T| &mut s.knobs)
}

fn settle(rt: &mut Runtime<T>) {
    for _ in 0..40 {
        rt.frame(1.0 / 60.0);
    }
}

fn click(rt: &mut Runtime<T>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no {key}"));
    rt.pointer_move(x, y);
    settle(rt);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

fn page_w(rt: &mut Runtime<T>) -> f32 {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some("knobs-page")).expect("page").target[2]
}

#[test]
fn opening_pushes_the_page_over_and_closing_gives_it_back() {
    let mut rt = Runtime::new(T { knobs: Knobs::default() }, view, (1000.0, 700.0), Env::new(true, 1.0));
    settle(&mut rt);
    assert_eq!(page_w(&mut rt), 1000.0);
    rt.state.knobs.show();
    rt.invalidate();
    settle(&mut rt);
    assert_eq!(page_w(&mut rt), 1000.0 - crate::SIDE_W);
    assert_eq!(rt.state.knobs.side_room(), crate::SIDE_W);
    assert!(rt.find("knobs-sec-0").is_some() && rt.find("knobs-sec-1").is_some(), "every group is a section");
    rt.state.knobs.close();
    rt.invalidate();
    settle(&mut rt);
    assert_eq!(page_w(&mut rt), 1000.0);
}

fn body(rt: &Runtime<T>, sec: &str) -> f32 {
    rt.elements().into_iter().find(|e| e.path.contains(sec) && e.key.as_deref() == Some("body")).map(|e| e.rect[3]).unwrap()
}

#[test]
fn a_header_opens_its_group_and_picks_it() {
    let mut s = T { knobs: Knobs::default() };
    s.knobs.show();
    let mut rt = Runtime::new(s, view, (1000.0, 700.0), Env::new(true, 1.0));
    settle(&mut rt);
    assert_eq!(body(&rt, "knobs-sec-1"), 0.0, "folded up front, though on screen");
    click(&mut rt, "knobs-sec-1/head");
    assert!(rt.state.knobs.unfolded.contains("Right"));
    assert_eq!(rt.state.knobs.group.as_deref(), Some("Right"));
    assert!(body(&rt, "knobs-sec-1") > 20.0);
    click(&mut rt, "knobs-sec-1/head");
    assert!(rt.state.knobs.folded.contains("Right"));
    assert_eq!(body(&rt, "knobs-sec-1"), 0.0);
}

#[test]
fn the_panel_reopens_folded_but_for_the_group_last_opened() {
    let mut s = T { knobs: Knobs::default() };
    s.knobs.show();
    let mut rt = Runtime::new(s, view, (1000.0, 700.0), Env::new(true, 1.0));
    settle(&mut rt);
    click(&mut rt, "knobs-sec-0/head");
    click(&mut rt, "knobs-sec-1/head");
    rt.state.knobs.close();
    rt.state.knobs.show();
    rt.invalidate();
    settle(&mut rt);
    assert_eq!(body(&rt, "knobs-sec-0"), 0.0);
    assert!(body(&rt, "knobs-sec-1") > 20.0);
}

#[test]
fn using_a_knob_makes_its_group_the_panels() {
    let mut s = T { knobs: Knobs::default() };
    s.knobs.show();
    let mut rt = Runtime::new(s, view, (1000.0, 700.0), Env::new(true, 1.0));
    settle(&mut rt);
    click(&mut rt, "knobs-sec-1/head");
    click(&mut rt, "knobs-sec-1/body/knobs-card/knobs-knob-0");
    assert_eq!(rt.state.knobs.group.as_deref(), Some("Right"));
    click(&mut rt, "knobs-random");
    assert!(rt.state.knobs.values.contains_key("Right pad"), "Random acts on the group in use");
    assert!(!rt.state.knobs.values.contains_key("Left size"));
}

#[test]
fn the_edge_drags_to_resize() {
    let mut s = T { knobs: Knobs::default() };
    s.knobs.show();
    let mut rt = Runtime::new(s, view, (1000.0, 700.0), Env::new(true, 1.0));
    settle(&mut rt);
    let (x, y) = rt.find("knobs-edge").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_move(x - 60.0, y);
    rt.pointer_up(x - 60.0, y);
    settle(&mut rt);
    assert_eq!(rt.state.knobs.side_w(), crate::SIDE_W + 60.0);
    assert_eq!(page_w(&mut rt), 1000.0 - crate::SIDE_W - 60.0);
}

#[test]
fn choices_fit_the_width() {
    const FEW: Knob = Knob::choice("Edge", &["Fade", "Line", "Tint"], 0);
    const SIX: Knob = Knob::choice("Question glyph", &["Disc", "? disc", "? mark", "? square", "? ring", "Bang"], 0);
    const MANY: Knob = Knob::choice("Theme picker style", &["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"], 0);
    assert_eq!(shape(&FEW, 316.0, false), Shape::Inline);
    // Six options come out as two even rows of three, not four and two.
    assert_eq!(shape(&SIX, 316.0, false), Shape::Grid { cols: 3 });
    assert_eq!(shape(&SIX, 600.0, false), Shape::Inline);
    assert!(matches!(shape(&MANY, 316.0, false), Shape::Long { .. }));
}

#[test]
fn folding_rolls_the_rows_up_on_the_fold_spring() {
    let mut s = T { knobs: Knobs::default() };
    s.knobs.show();
    s.knobs.set_folded("Left", false);
    let mut rt = Runtime::new(s, view, (1000.0, 700.0), Env::new(true, 1.0));
    settle(&mut rt);
    let seen = |rt: &Runtime<T>| rt.elements().into_iter().find(|e| e.path.contains("knobs-sec-0") && e.key.as_deref() == Some("body")).map(|e| e.rect[3]).unwrap();
    let open = seen(&rt);
    let (x, y) = rt.find("knobs-sec-0/head").unwrap();
    rt.pointer_move(x, y);
    rt.frame(1.0 / 60.0);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    for _ in 0..4 {
        rt.frame(1.0 / 60.0);
    }
    let mid = seen(&rt);
    assert!(mid > 1.0 && mid < open - 1.0, "partway: {mid} of {open}");
    settle(&mut rt);
    assert!(seen(&rt) < 0.5);
}

#[test]
fn hovering_an_option_says_what_it_does_under_the_list() {
    let mut s = T { knobs: Knobs::default() };
    s.knobs.show();
    s.knobs.set_folded("Left", false);
    let mut rt = Runtime::new(s, view, (1000.0, 700.0), Env::new(true, 1.0));
    settle(&mut rt);
    assert!(rt.find("knobs-about-option").is_none(), "nothing in hand, so the keys show");
    let (x, y) = rt.find("opt-1").expect("Style's second option");
    rt.pointer_move(x, y);
    settle(&mut rt);
    assert!(rt.find("knobs-about-option").is_some(), "the hovered option's line shows");
    assert_eq!(C.hint(1), "The second");
    assert_eq!(C.hint(2), "", "an option without a line says nothing");
}

fn open_left() -> Runtime<T> {
    let mut s = T { knobs: Knobs::default() };
    s.knobs.show();
    s.knobs.set_folded("Left", false);
    let mut rt = Runtime::new(s, view, (1000.0, 700.0), Env::new(true, 1.0));
    settle(&mut rt);
    rt
}

fn rect(rt: &Runtime<T>, suffix: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.path.ends_with(suffix) && e.opacity > 0.0).unwrap_or_else(|| panic!("no {suffix}")).rect
}

fn right_click(rt: &mut Runtime<T>, x: f32, y: f32) {
    rt.pointer_move(x, y);
    settle(rt);
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    settle(rt);
}

fn press(rt: &mut Runtime<T>, key: rune::Key) {
    rt.key(rune::KeyEvent { key, mods: Default::default() });
    settle(rt);
}

#[test]
fn right_clicking_a_knob_puts_only_it_back() {
    let mut rt = open_left();
    rt.state.knobs.set(&L, 8.0);
    rt.state.knobs.set(&C, 2.0);
    let r = rect(&rt, "knobs-sec-0/body/knobs-card/knobs-knob-0");
    right_click(&mut rt, r[0] + 40.0, r[1] + r[3] / 2.0);
    assert_eq!(rt.state.knobs.get(&L), 5.0, "back to its default");
    assert_eq!(rt.state.knobs.get(&C), 2.0, "its neighbour keeps its value");
    rt.state.knobs.undo();
    assert_eq!(rt.state.knobs.get(&L), 8.0, "undoable");
}

#[test]
fn a_hovered_group_header_offers_random_and_reset() {
    let mut rt = open_left();
    rt.state.knobs.set(&L, 8.0);
    rt.state.knobs.set(&R, 30.0);
    assert!(rt.find("knobs-sec-0/head/reset").is_none(), "the count shows until hovered");
    let h = rect(&rt, "knobs-sec-0/head");
    rt.pointer_move(h[0] + 40.0, h[1] + h[3] / 2.0);
    settle(&mut rt);
    click(&mut rt, "knobs-sec-0/head/reset");
    assert_eq!(rt.state.knobs.get(&L), 5.0, "Reset puts the group back");
    assert_eq!(rt.state.knobs.get(&R), 30.0, "and leaves other groups alone");
    assert!(rt.find("knobs-sec-0/body/knobs-card/knobs-knob-0").is_some(), "without folding the group");
    click(&mut rt, "knobs-sec-0/head/random");
    assert!(rt.state.knobs.values.contains_key("Left size"), "Random sets the group's knobs");
    right_click(&mut rt, h[0] + 40.0, h[1] + h[3] / 2.0);
    assert!(!rt.state.knobs.values.contains_key("Left size"), "right-clicking the header resets it too");
}

#[test]
fn clicking_a_choice_row_puts_the_arrows_on_its_options() {
    let mut rt = open_left();
    // Put the keyboard elsewhere first, on the slider above.
    let r = rect(&rt, "knobs-sec-0/head");
    click(&mut rt, "knobs-sec-0/head");
    click(&mut rt, "knobs-sec-0/head");
    assert_eq!(rt.state.knobs.at, Some(Node::Group(0)), "at {r:?}");
    let row = rect(&rt, "knobs-sec-0/body/knobs-card/knobs-knob-1");
    let (x, y) = (row[0] + 20.0, row[1] + row[3] / 2.0);
    rt.pointer_move(x, y);
    settle(&mut rt);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(&mut rt);
    press(&mut rt, rune::Key::Right);
    assert_eq!(rt.state.knobs.get(&C), 1.0, "→ steps the clicked choice, not the group");
    press(&mut rt, rune::Key::Right);
    assert_eq!(rt.state.knobs.get(&C), 2.0);
}

#[test]
fn moving_the_pointer_takes_the_highlight_from_the_keyboard() {
    let mut rt = open_left();
    click(&mut rt, "knobs-sec-0/head");
    click(&mut rt, "knobs-sec-0/head");
    press(&mut rt, rune::Key::Down);
    assert!(rt.state.knobs.kbd, "the keyboard's row is lit");
    let row = rect(&rt, "knobs-sec-0/body/knobs-card/knobs-knob-1");
    rt.pointer_move(row[0] + 30.0, row[1] + row[3] / 2.0);
    settle(&mut rt);
    assert!(!rt.state.knobs.kbd, "only the row under the pointer stays lit");
    press(&mut rt, rune::Key::Down);
    assert!(rt.state.knobs.kbd, "a key lights the keyboard's row again");
}

#[test]
fn closing_panel_keeps_its_full_width_while_side_box_slides_shut() {
    let mut s = T { knobs: Knobs::default() };
    s.knobs.show();
    let mut rt = Runtime::new(s, view, (1000.0, 700.0), Env::new(true, 1.0));
    settle(&mut rt);
    let open_w = rt.presented("knobs-panel").expect("panel open").0[2];
    assert!((open_w - crate::SIDE_W).abs() < 0.5);

    rt.state.knobs.close();
    rt.invalidate();
    let mut saw_sliding = false;
    for f in 0..20 {
        rt.frame(1.0 / 60.0);
        let (side_box, _) = rt.presented("knobs-side").expect("knobs-side");
        if let Some((panel, _)) = rt.presented("knobs-panel") {
            // knobs-panel is absolute-positioned inside knobs-side: it keeps its full width
            // while knobs-side clips and slides shut.
            assert!((panel[2] - crate::SIDE_W).abs() < 0.5, "f={f}: knobs-panel width changed to {}", panel[2]);
            if side_box[2] < crate::SIDE_W - 10.0 && side_box[2] > 10.0 {
                saw_sliding = true;
            }
        }
    }
    assert!(saw_sliding, "knobs-side should slide shut smoothly");
    settle(&mut rt);
    assert!(rt.presented("knobs-panel").is_none(), "knobs-panel unmounts after close");
}
