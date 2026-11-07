//! The middle row: the mixer, the XY pad, the play queue and the activity feed.

use motion_bento_chaos::Action;
use motion_bento_chaos::look::{color, grid, lch};
use motion_bento_chaos::model::{FADERS, FEED_KINDS, TRACKS};
use rune::{Edge, El, Shape, Transition, canvas, col, el, row, spacer, text};
use rune_kit::{Kit, RollingText};

use crate::State;
use crate::style::{c, caption, font, inner_h, inner_w, nums, spring, tile};

const TRACK_H: f32 = 132.0;

fn tile_w() -> f32 {
    inner_w(1.0) + 2.0 * grid::TILE_PAD
}

pub fn mixer(s: &State, k: &Kit) -> El {
    let travel = spring(s.springs.travel);
    let mut faders = row().w_full().h(inner_h(grid::ROW2)).justify_between();
    for (i, name) in FADERS.iter().enumerate() {
        let v = s.m.faders[i];
        let h = 12.0 + v * (TRACK_H - 12.0);
        let track = el()
            .size(30.0, TRACK_H)
            .child(el().size(8.0, TRACK_H).absolute(11.0, 0.0).radius(4.0).fill(c(color::WELL)))
            .child(
                el().size(8.0, h)
                    .absolute(11.0, TRACK_H - h)
                    .radius(4.0)
                    .gradient(c(lch(0.74, 0.14, 240.0)), c(color::ACCENT))
                    .motion(travel),
            )
            .child(
                el().size(30.0, 12.0)
                    .absolute(0.0, TRACK_H - h - 6.0)
                    .radius(6.0)
                    .fill(c(color::TEXT))
                    .shadow(2.0, 6.0, c(lch(0.0, 0.0, 0.0).alpha(0.5)))
                    .motion(travel),
            );
        faders = faders.child(
            col()
                .key(format!("fader{i}"))
                .items_center()
                .gap(8.0)
                .on_press(move |s: &mut State| s.act(Action::Fader(i as u8)))
                .child(RollingText::new(format!("level{i}"), format!("{:.0}", v * 100.0)).value(v as f64).font(nums(12.0, 560.0)).color(c(color::TEXT_2)).build(k))
                .child(track)
                .child(caption(*name)),
        );
    }
    let live = row()
        .gap(6.0)
        .center_y()
        .child(el().size(6.0, 6.0).radius(3.0).fill(c(color::GOOD)))
        .child(caption("Live"));
    tile("mixer", "Mixer", live, faders).w(tile_w())
}

fn grid_lines(w: f32, h: f32) -> Shape {
    let line = c(lch(1.0, 0.0, 0.0).alpha(0.06));
    let mut s = Shape::new();
    for i in 1..4 {
        let x = w * i as f32 / 4.0;
        let y = h * i as f32 / 4.0;
        s = s.line([x, 0.0], [x, h], 1.0, rune::LineCap::Butt, line).line([0.0, y], [w, y], 1.0, rune::LineCap::Butt, line);
    }
    s
}

pub fn xy(s: &State, k: &Kit) -> El {
    let travel = spring(s.springs.travel);
    let (w, h) = (inner_w(1.0), inner_h(grid::ROW2));
    let [px, py] = s.m.puck;
    let (x, y) = (px * w, py * h);
    let pad = el()
        .key("pad")
        .size(w, h)
        .radius(14.0)
        .fill(c(color::WELL))
        .border(1.0, c(color::EDGE))
        .clip_rounded(14.0)
        .on_press(|s: &mut State| s.act(Action::Puck))
        .child(canvas(grid_lines(w, h)).size(w, h).absolute(0.0, 0.0))
        .child(el().size(1.0, h).absolute(x, 0.0).fill(c(color::ACCENT.alpha(0.35))).motion(travel))
        .child(el().size(w, 1.0).absolute(0.0, y).fill(c(color::ACCENT.alpha(0.35))).motion(travel))
        .child(el().size(96.0, 96.0).absolute(x - 48.0, y - 48.0).radius(48.0).fill(c(color::ACCENT.alpha(0.28))).blur(18.0).motion(travel))
        .child(
            el().size(24.0, 24.0)
                .absolute(x - 12.0, y - 12.0)
                .radius(12.0)
                .fill(c(color::ACCENT))
                .border(3.0, c(color::TEXT))
                .shadow(3.0, 10.0, c(lch(0.0, 0.0, 0.0).alpha(0.45)))
                .motion(travel),
        );
    let readout = row()
        .gap(10.0)
        .child(RollingText::new("cutoff", format!("{:.0} Hz", 200.0 + px * 7800.0)).value(px as f64).font(nums(12.0, 500.0)).color(c(color::TEXT_3)).build(k))
        .child(RollingText::new("res", format!("Q {:.1}", 0.5 + (1.0 - py) * 9.5)).value((1.0 - py) as f64).font(nums(12.0, 500.0)).color(c(color::TEXT_3)).build(k));
    tile("xy", "Filter", readout, pad).w(tile_w())
}

/// The queue's card art: a two-tone gradient per track.
fn art(track: usize) -> El {
    let hue = (track as f32 * 47.0 + 200.0) % 360.0;
    el().size(64.0, 64.0)
        .radius(12.0)
        .linear_gradient(135f32.to_radians(), [(0.0, c(lch(0.72, 0.15, hue))), (1.0, c(lch(0.42, 0.14, (hue + 50.0) % 360.0)))])
}

pub const CARD_H: f32 = 140.0;
/// How far each card behind the front one sits lower, and how much smaller it is.
pub const PEEK: f32 = 14.0;
pub const SHRINK: f32 = 0.05;

pub fn queue(s: &State, _k: &Kit) -> El {
    let layout = spring(s.springs.layout);
    let w = inner_w(1.0);
    let h = inner_h(grid::ROW2);
    let mut deck = el().w(w).h(h);
    // Back to front, so the front card paints last.
    for (depth, card) in s.m.queue.iter().enumerate().rev() {
        let d = depth as f32;
        let front = depth == 0;
        let (title, artist) = TRACKS[card.track];
        let shade = lch(0.29 - 0.025 * d, 0.018, 258.0);
        let progress = 0.18 + 0.09 * (card.id % 7) as f32;
        let info = col()
            .gap(14.0)
            .w_full()
            .opacity(if front { 1.0 } else { 0.0 })
            .child(
                row()
                    .gap(14.0)
                    .center_y()
                    .child(art(card.track))
                    .child(
                        col()
                            .gap(3.0)
                            .child(text(title).font(font(16.0, 620.0)).color(c(color::TEXT)))
                            .child(text(artist).font(font(13.0, 460.0)).color(c(color::TEXT_2))),
                    ),
            )
            .child(
                col()
                    .gap(6.0)
                    .child(el().w_full().h(4.0).radius(2.0).fill(c(color::WELL_HI)).child(el().w_frac(progress).h(4.0).radius(2.0).fill(c(color::TEXT_2))))
                    .child(
                        row()
                            .child(text(format!("{}:{:02}", (progress * 220.0) as u32 / 60, (progress * 220.0) as u32 % 60)).font(nums(11.0, 500.0)).color(c(color::TEXT_3)))
                            .child(spacer())
                            .child(text("3:40").font(nums(11.0, 500.0)).color(c(color::TEXT_3))),
                    ),
            );
        deck = deck.child(
            col()
                .key(format!("card{}", card.id))
                .size(w, CARD_H)
                .absolute(0.0, d * PEEK)
                .scale(1.0 - d * SHRINK)
                .radius(16.0)
                .fill(c(shade))
                .border(1.0, c(color::EDGE))
                .shadow(6.0, 18.0, c(lch(0.0, 0.0, 0.0).alpha(0.35)))
                .padding(16.0)
                .motion(layout)
                .transition(Transition::asymmetric(
                    Transition::scale(0.9),
                    Transition::move_edge(Edge::Trailing).combined(Transition::opacity()),
                ))
                .child(info),
        );
    }
    let skip = row()
        .key("skip")
        .h(24.0)
        .px(10.0)
        .radius(12.0)
        .center()
        .fill(c(color::ACCENT_DIM))
        .hover(c(color::ACCENT.alpha(0.28)))
        .press_scale(0.94)
        .on_press(|s: &mut State| s.act(Action::Queue))
        .child(text("Skip").font(font(12.0, 600.0)).color(c(color::ACCENT)));
    tile("queue", "Up next", skip, deck).w(tile_w())
}

const KIND_HUES: [f32; 8] = [245.0, 30.0, 155.0, 70.0, 200.0, 265.0, 330.0, 175.0];

pub fn feed(s: &State, _k: &Kit) -> El {
    let mut rows = col().w_full().gap(6.0);
    for item in &s.m.feed {
        let hue = KIND_HUES[item.kind % KIND_HUES.len()];
        rows = rows.child(
            row()
                .key(format!("item{}", item.id))
                .h(40.0)
                .w_full()
                .px(12.0)
                .gap(10.0)
                .radius(12.0)
                .center_y()
                .fill(c(color::WELL))
                .transition(Transition::asymmetric(
                    Transition::offset(0.0, -12.0).combined(Transition::opacity()),
                    Transition::scale(0.96),
                ))
                .child(el().size(8.0, 8.0).radius(4.0).fill(c(lch(0.72, 0.15, hue))))
                .child(text(FEED_KINDS[item.kind]).font(font(13.0, 500.0)).color(c(color::TEXT)).truncate())
                .child(spacer())
                .child(text(format!("9:{:02}", item.minute)).font(nums(12.0, 460.0)).color(c(color::TEXT_3))),
        );
    }
    let test = row()
        .key("test")
        .h(24.0)
        .px(10.0)
        .radius(12.0)
        .center()
        .fill(c(color::WELL))
        .hover(c(color::WELL_HI))
        .press_scale(0.94)
        .on_press(|s: &mut State| s.act(Action::Feed))
        .child(text("Test alert").font(font(12.0, 560.0)).color(c(color::TEXT_2)));
    tile("feed", "Activity", test, rows.motion(spring(s.springs.layout))).w(tile_w())
}
