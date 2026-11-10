use super::*;
use crate::{Color, Key, Mods, ScrollSpec};

#[derive(Default)]
struct Viewer {
    open: bool,
    glide: u32,
}

/// A tile in a list scrolled out of its first rows, or open over it, outside the scroll.
fn view(s: &Viewer, _: &mut Cx<Viewer>) -> El {
    let tile = |size: f32| el().key("tile").matched("tile").size(size, size).fill(Color::WHITE);
    let slot = if s.open { el().key("held").size(40.0, 40.0) } else { tile(40.0) };
    let rows = col().key("rows").w_full().child(el().w_full().h(200.0)).child(slot).child(el().w_full().h(400.0));
    let list = col().key("list").size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).child(rows);
    let root = col().size(300.0, 200.0).child(list);
    if s.open { root.child(tile(160.0).absolute(100.0, 0.0)) } else { root }
}

fn close(s: &mut Viewer, _: &KeyEvent, _: &mut Tx) -> bool {
    s.open = false;
    true
}

#[test]
fn a_flight_home_after_an_earlier_commit_in_its_frame_starts_where_it_was_seen() {
    let mut rt = Runtime::new(Viewer::default(), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.on_key = Some(close);
    rt.frame(0.0);
    rt.wheel(150.0, 100.0, -150.0);
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    assert_eq!(rt.presented("tile").expect("tile").0, [0.0, 50.0, 40.0, 40.0]);
    rt.update(|s, _| s.open = true);
    for _ in 0..120 { rt.frame(1.0 / 60.0); }
    assert_eq!(rt.presented("tile").expect("tile").0, [100.0, 0.0, 160.0, 160.0]);
    // A glide changes state each frame, so the frame commits it before routing the key that
    // closes the viewer: the flight home starts from the second commit.
    rt.update(|s, _| s.glide += 1);
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(1.0 / 120.0);
    let first = rt.presented("tile").expect("tile").0;
    assert!(first[0] > 90.0 && first[1] > -10.0 && first[1] < 10.0, "starts where it showed open: {first:?}");
}
