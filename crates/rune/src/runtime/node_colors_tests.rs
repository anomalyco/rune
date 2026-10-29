use super::*;
use crate::{col, el, text};

struct State { dark: bool }

fn view(s: &State, _: &mut Cx<State>) -> El {
    let (bg, fg) = if s.dark { (Color::BLACK, Color::WHITE) } else { (Color::WHITE, Color::BLACK) };
    col().w_full().h_full().fill(bg).children((0..12).map(|i| {
        el().key(format!("row-{i}")).w_full().h(20.0).fill(bg).radius(4.0).opacity(0.9)
            .child(text(format!("row {i}")).color(fg))
    }))
}

fn settled(dark: bool) -> Runtime<State> {
    let mut rt = Runtime::new(State { dark }, view, (200.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let mut n = 0;
    while n < 240 && rt.frame(1.0 / 120.0) { n += 1; }
    rt.scene.frame();
    rt
}

fn row(rt: &Runtime<State>) -> &Node {
    rt.nodes.iter().find(|n| n.key.as_deref() == Some("row-3")).unwrap()
}

fn label(rt: &Runtime<State>) -> &Node {
    let row = rt.nodes.iter().position(|n| n.key.as_deref() == Some("row-3")).unwrap();
    rt.nodes.iter().find(|n| n.parent == Some(row) && n.text.is_some()).unwrap()
}

#[test]
fn a_static_tree_keeps_no_motion_tracks() {
    let mut rt = settled(false);
    assert_eq!(rt.scene.store().track_len(), 0, "nothing here has moved");
    rt.invalidate();
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.scene.store().track_len(), 0, "an unchanged rebuild makes none either");
}

#[test]
fn an_unchanged_commit_leaves_the_scene_clean() {
    let mut rt = settled(false);
    let tx = rt.transaction();
    rt.commit(&tx);
    assert!(rt.scene.frame().changed.is_empty(), "no node presents again");
}

#[test]
fn a_first_colour_change_still_crossfades_from_the_drawn_colour() {
    let mut rt = settled(false);
    rt.update(|s, _| s.dark = true);
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);
    let n = row(&rt);
    let fill = rt.color(n.id, P_FILL, n.vis.fill);
    assert!(fill.r < 1.0 && fill.r > 0.0, "the fill is between white and black: {fill:?}");
    let l = label(&rt);
    let text = rt.color(l.id, P_TEXT, l.text.as_ref().unwrap().color);
    assert!(text.r > 0.0 && text.r < 1.0, "the text is between black and white: {text:?}");
    let mut k = 0;
    while k < 240 && rt.frame(1.0 / 120.0) { k += 1; }
    let n = row(&rt);
    assert_eq!(rt.color(n.id, P_FILL, n.vis.fill), Color::BLACK);
}

#[test]
fn an_instant_colour_change_lands_without_a_track() {
    let mut rt = settled(false);
    rt.update(|s, tx| { s.dark = true; tx.instant(); });
    rt.frame(1.0 / 120.0);
    let n = row(&rt);
    assert_eq!(rt.color(n.id, P_FILL, n.vis.fill), Color::BLACK);
    assert_eq!(rt.scene.store().track_len(), 0);
}
