use super::*;
use crate::{Key, KeyEvent, Mods};
use rune_motion::{Curve, Easing};

const DT: f64 = 1.0 / 120.0;

/// A row with a toggle and a menu button, both hidden at rest. The toggle holds a chevron that
/// shows for the row's hover or for focus on the toggle, on a 0.15 s fade of its own; the menu
/// button shows for the row's hover or its own focus, on the row's instant track. `pointer`:
/// only the pointer lights the row.
fn view(pointer: &bool, _: &mut Cx<bool>) -> El {
    let fade = Curve::tween(0.15, Easing::LINEAR);
    let chevron = el().key("chevron").size(10.0, 10.0).fill(Color::WHITE).group_opacity(0.0, 1.0).group_focus_within("toggle").group_motion(fade, fade);
    let toggle = crate::row().key("toggle").focusable(true).size(100.0, 40.0).on_press(|_: &mut bool| {}).child(chevron);
    let menu = el().key("menu").focusable(true).size(20.0, 20.0).fill(Color::WHITE).on_press(|_: &mut bool| {}).group_opacity(0.0, 1.0);
    let row = crate::row().key("row").size(200.0, 40.0).hover_motion(Anim::INSTANT, Anim::INSTANT);
    let row = if *pointer { row.hover_group_pointer() } else { row.hover_group() };
    col().size(200.0, 100.0).child(row.child(toggle).child(menu))
}

fn alpha(rt: &Runtime<bool>, key: &str) -> f32 {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).map_or(-1.0, |e| e.opacity)
}

fn tab(rt: &mut Runtime<bool>) {
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
}

/// `key`'s opacity over the next `frames` frames.
fn run(rt: &mut Runtime<bool>, key: &str, frames: usize) -> Vec<f32> {
    (0..frames).map(|_| { rt.frame(DT); alpha(rt, key) }).collect()
}

#[test]
fn a_member_with_its_own_curves_fades_while_the_group_cuts() {
    let mut rt = Runtime::new(true, view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!((alpha(&rt, "chevron"), alpha(&rt, "menu")), (0.0, 0.0));
    rt.pointer_move(150.0, 20.0);
    rt.frame(DT);
    rt.frame(DT);
    assert_eq!(alpha(&rt, "menu"), 1.0, "the row's own track is instant");
    let part = alpha(&rt, "chevron");
    assert!(part > 0.0 && part < 0.5, "the chevron is on its way after two frames of a 0.15 s fade: {part}");
    let seen = run(&mut rt, "chevron", 30);
    assert!(seen.windows(2).all(|w| w[1] >= w[0] - 1e-4), "it rises: {seen:?}");
    assert!((seen[29] - 1.0).abs() < 1e-3, "and lands: {seen:?}");
    rt.pointer_leave();
    rt.frame(DT);
    rt.frame(DT);
    assert_eq!(alpha(&rt, "menu"), 0.0, "the row lets go at once");
    let part = alpha(&rt, "chevron");
    assert!(part < 1.0 && part > 0.5, "the chevron fades out on its own curve: {part}");
    let seen = run(&mut rt, "chevron", 30);
    assert!(seen.windows(2).all(|w| w[1] <= w[0] + 1e-4), "it falls: {seen:?}");
    assert!(seen[29].abs() < 1e-3, "and lands: {seen:?}");
    assert!(!rt.animating);
}

#[test]
fn a_pointer_group_leaves_focus_to_what_names_it() {
    let mut rt = Runtime::new(true, view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    tab(&mut rt);
    let seen = run(&mut rt, "chevron", 40);
    assert_eq!(rt.focused_key().as_deref(), Some("toggle"));
    assert!((seen[39] - 1.0).abs() < 1e-3, "focus on the toggle shows its chevron: {seen:?}");
    assert!(seen[0] < 0.5, "on the chevron's own fade: {seen:?}");
    assert_eq!(alpha(&rt, "menu"), 0.0, "focus inside a pointer group doesn't light it");
    tab(&mut rt);
    let seen = run(&mut rt, "chevron", 40);
    assert_eq!(rt.focused_key().as_deref(), Some("menu"));
    assert_eq!(alpha(&rt, "menu"), 1.0, "a control shows for its own focus");
    assert!(seen[39].abs() < 1e-3, "and the chevron goes with the toggle's focus: {seen:?}");
}

#[test]
fn a_hover_group_still_lights_for_focus_inside_it() {
    let mut rt = Runtime::new(false, view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    tab(&mut rt);
    let seen = run(&mut rt, "chevron", 40);
    assert_eq!(rt.focused_key().as_deref(), Some("toggle"));
    assert_eq!(alpha(&rt, "menu"), 1.0, "focus in the row lights the row");
    assert!((seen[39] - 1.0).abs() < 1e-3, "and the chevron with it: {seen:?}");
}

#[test]
fn a_named_ancestor_is_the_nearest_one_with_that_key() {
    fn nested(_: &bool, _: &mut Cx<bool>) -> El {
        let mark = el().key("mark").size(10.0, 10.0).fill(Color::WHITE).group_opacity(0.0, 1.0).group_focus_within("outer");
        let inner = crate::row().key("inner").size(50.0, 20.0).child(mark);
        let outer = crate::row().key("outer").focusable(true).size(100.0, 20.0).on_press(|_: &mut bool| {}).child(inner);
        let other = crate::row().key("other").focusable(true).size(100.0, 20.0).on_press(|_: &mut bool| {});
        col().key("list").size(200.0, 100.0).hover_group_pointer().child(outer).child(other)
    }
    let mut rt = Runtime::new(true, nested, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(alpha(&rt, "mark"), 0.0);
    tab(&mut rt);
    for _ in 0..4 { rt.frame(DT); }
    assert_eq!(rt.focused_key().as_deref(), Some("outer"));
    assert_eq!(alpha(&rt, "mark"), 1.0, "focus on the ancestor two levels up shows it, at once without curves of its own");
    tab(&mut rt);
    for _ in 0..4 { rt.frame(DT); }
    assert_eq!(rt.focused_key().as_deref(), Some("other"));
    assert_eq!(alpha(&rt, "mark"), 0.0, "focus elsewhere in the pointer group doesn't");
}
