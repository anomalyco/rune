//! A feed scrolling under a floating liquid-glass tab bar. The selection is a drop of the same
//! glass that slides between tabs and bulges out of the bar as it goes.

use rune::{Cx, El, col, el, row};

use crate::gfx::{LENS, MESH};
use crate::kit::*;
use crate::State;

const STORIES: [(&str, &str, u32); 6] = [
    ("Kyoto after rain", "Photo essay · 12 min", 1),
    ("The quiet studio", "Design · 6 min", 7),
    ("Night markets", "Travel · 9 min", 5),
    ("Ferry to Naoshima", "Photo essay · 8 min", 4),
    ("Paper and light", "Craft · 5 min", 2),
    ("Cold coast", "Travel · 11 min", 6),
];

fn story(title: &str, meta: &str, pal: u32, w: f32, h: f32, seed: f32) -> El {
    col().w(w).gap(10.0)
        .child(fx(MESH, [pal as f32, 0.5, seed, 0.25]).w(w).h(h).radius(18.0))
        .child(t(title, 17.0, 600.0, TEXT))
        .child(t(meta, 13.0, 400.0, MUTED))
}

pub fn view(s: &State, _: &mut Cx<State>) -> El {
    let t_ = s.t;
    // The feed scrolls on its own, with a flick a second in.
    let scroll = 50.0 * t_ as f32 + track(t_, 0.0, &[(0.9, 260.0, sp(1.1, 0.0))]);
    let mut feed = col().absolute(40.0, 30.0 - scroll).w(W - 80.0).gap(28.0)
        .child(col().gap(4.0).child(t("Today", 30.0, 600.0, TEXT)).child(t("Friday, November 6", 14.0, 500.0, MUTED)))
        .child(story("Glass, light and the long weekend", "Featured · 14 min", 0, W - 80.0, 300.0, 1.0));
    for pair in STORIES.chunks(3) {
        let mut r = row().gap(20.0);
        for (i, (title, meta, pal)) in pair.iter().enumerate() {
            r = r.child(story(title, meta, *pal, (W - 80.0 - 40.0) / 3.0, 180.0, 10.0 + i as f32 + *pal as f32));
        }
        feed = feed.child(r);
    }

    // The bar and its selection drop.
    let (bw, bh) = (400.0, 64.0);
    let (bx, by) = ((W - bw) / 2.0, H - bh - 26.0);
    let tabs = ["Today", "Search", "Saved", "Profile"];
    let slot = bw / 4.0;
    let sel = track(t_, 0.0, &[(0.55, 2.0, sp(0.7, 0.28)), (1.75, 1.0, sp(0.6, 0.3))]);
    let sx = bx + slot * (sel + 0.5);
    let mut root = el().w(W).h(H).fill(c(0x0b0b0d)).child(feed)
        .child(fx(LENS, [sx, by + bh / 2.0, 42.0, 0.2]).effect_backdrop(true).absolute(bx, by).size(bw, bh).radius(bh / 2.0))
        .child(fx(LENS, [0.0, 0.0, 0.0, 0.25]).effect_backdrop(true).absolute(bx + bw + 16.0, by).size(bh, bh).radius(bh / 2.0));
    for (i, name) in tabs.iter().enumerate() {
        let on = (sel - i as f32).abs() < 0.5;
        root = root.child(row().absolute(bx + slot * i as f32, by).size(slot, bh).center().child(t(*name, 13.5, 600.0, if on { 0xffffff } else { 0xd8d8de })));
    }
    root.child(row().absolute(bx + bw + 16.0, by).size(bh, bh).center().child(t("+", 26.0, 400.0, 0xffffff)))
}
