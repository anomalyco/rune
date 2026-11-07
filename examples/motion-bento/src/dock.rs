//! The bottom row: five tiles that reflow so the focused one is wide.

use motion_bento_chaos::Action;
use motion_bento_chaos::look::{color, grid, lch};
use motion_bento_chaos::model::{CHIPS, DOCK, DOCK_KEYS, ENERGY_BARS, FILES, VISIT_MAX};
use rune::{El, Transition, col, el, memo, row, spacer, text};
use rune_kit::{Kit, RollingText};

use crate::State;
use crate::style::{c, caption, font, nums, spring, tile};

const DAYS: [&str; ENERGY_BARS] = ["M", "T", "W", "T", "F", "S", "S"];

pub fn dock(s: &State, k: &Kit) -> El {
    let body_h = dock_h() - 2.0 * grid::TILE_PAD - 34.0;
    let mut dock = row().key("dock").w_full().h(dock_h()).gap(grid::GAP);
    for i in 0..DOCK.len() {
        let focused = s.m.focus == i;
        dock = dock.child(memo(DOCK_KEYS[i], (focused, version(s, i)), || {
            let (aside, body) = match i {
                0 => energy(s, k, body_h),
                1 => chips(s, k),
                2 => orb(s, k, body_h),
                3 => transfers(s, k),
                _ => visitors(s, k, body_h),
            };
            tile(DOCK_KEYS[i], DOCK[i], aside, body)
                .grow(if focused { grid::DOCK_FOCUS } else { 1.0 })
                .w(0.0)
                .h_full()
                .fill(c(if focused { color::TILE_FOCUS } else { color::TILE }))
                .border(1.0, c(if focused { color::ACCENT.alpha(0.45) } else { color::EDGE }))
                .clip_rounded(grid::RADIUS)
                .on_press(move |s: &mut State| s.act(Action::Focus(i as u8)))
        }));
    }
    dock
}

/// What dock tile `i` shows of the model, as a memo version: f32s by their bits.
fn version(s: &State, i: usize) -> Vec<u32> {
    let m = &s.m;
    match i {
        0 => m.energy.iter().map(|v| v.to_bits()).chain([m.kwh_tenths]).collect(),
        1 => m.chips.iter().map(|&c| c as u32).chain([m.chips_open as u32]).collect(),
        2 => [m.orb.size, m.orb.hue, m.orb.round, m.orb.turn].map(f32::to_bits).to_vec(),
        3 => m.transfers.map(f32::to_bits).to_vec(),
        _ => m.visits.iter().flat_map(|v| [v.id, v.count]).chain([m.visitors]).collect(),
    }
}

pub fn dock_h() -> f32 {
    grid::HEIGHT - 2.0 * grid::PAD - grid::HEADER - grid::ROW1 - grid::ROW2 - 3.0 * grid::GAP
}

fn energy(s: &State, k: &Kit, h: f32) -> (El, El) {
    let bars_h = h - 22.0;
    let mut bars = row().key("bars").w_full().h(bars_h).items_end().gap(6.0).on_press(|s: &mut State| s.act(Action::Energy));
    let mut days = row().w_full().gap(6.0);
    for (i, v) in s.m.energy.iter().enumerate() {
        let peak = s.m.energy.iter().cloned().fold(0.0, f32::max) == *v;
        bars = bars.child(
            el().grow(1.0)
                .w(0.0)
                .h(10.0 + v * (bars_h - 10.0))
                .radius(6.0)
                .gradient(c(if peak { lch(0.8, 0.13, 240.0) } else { lch(0.5, 0.1, 250.0) }), c(if peak { color::ACCENT } else { lch(0.36, 0.07, 255.0) }))
                .motion(spring(s.springs.travel)),
        );
        days = days.child(row().grow(1.0).w(0.0).center().child(caption(DAYS[i])));
    }
    let aside = RollingText::new("kwh", format!("{} kWh", s.m.energy_kwh())).value(s.m.kwh_tenths as f64).font(nums(12.0, 560.0)).color(c(color::TEXT_2)).build(k);
    (aside, col().w_full().gap(6.0).child(bars).child(days))
}

fn chips(s: &State, _k: &Kit) -> (El, El) {
    let snap = spring(s.springs.snap);
    let mut wrap = row().w_full().flex_wrap().gap(6.0).motion(spring(s.springs.layout));
    for i in 0..s.m.chips_shown() {
        let on = s.m.chips[i];
        wrap = wrap.child(
            row()
                .key(format!("chip{i}"))
                .h(28.0)
                .px(12.0)
                .radius(14.0)
                .center()
                .fill(c(if on { color::ACCENT } else { color::WELL }))
                .border(1.0, c(if on { color::ACCENT.alpha(0.0) } else { color::EDGE }))
                .press_scale(0.94)
                .transition(Transition::scale(0.8))
                .on_press(move |s: &mut State| s.act(Action::Chip(i as u8)))
                .child(text(CHIPS[i]).font(font(13.0, 540.0)).color(c(if on { color::ON_ACCENT } else { color::TEXT_2 })).motion(snap))
                .motion(snap),
        );
    }
    let more = row()
        .key("more")
        .h(22.0)
        .px(8.0)
        .radius(11.0)
        .center()
        .hover(c(color::WELL))
        .on_press(|s: &mut State| s.act(Action::ChipsMore))
        .child(RollingText::new("more-label", if s.m.chips_open { "Less" } else { "More" }).font(font(12.0, 560.0)).color(c(color::ACCENT)).build(_k));
    (more, wrap)
}

fn orb(s: &State, k: &Kit, h: f32) -> (El, El) {
    let o = s.m.orb;
    let side = 40.0 + o.size * (h - 50.0);
    let tint = lch(0.7, 0.16, o.hue);
    let shape = el()
        .key("orb")
        .size(side, side)
        .radius(o.round * side)
        .gradient(c(lch(0.84, 0.12, o.hue - 20.0)), c(lch(0.52, 0.19, o.hue + 25.0)))
        .shadow(10.0, 44.0, c(tint.alpha(0.55)))
        .rotate(o.turn.to_radians())
        .motion(spring(s.springs.morph))
        .on_press(|s: &mut State| s.act(Action::Orb))
        .child(el().size(side * 0.34, side * 0.34).absolute(side * 0.14, side * 0.14).radius(side * 0.17).fill(c(lch(1.0, 0.0, 0.0).alpha(0.28))));
    let aside = RollingText::new("hue", format!("{:.0}°", o.hue)).value(o.hue as f64).font(nums(12.0, 560.0)).color(c(color::TEXT_2)).build(k);
    (aside, row().w_full().h(h).center().child(shape))
}

fn transfers(s: &State, k: &Kit) -> (El, El) {
    let travel = spring(s.springs.travel);
    let mut rows = col().w_full().gap(12.0);
    for (i, name) in FILES.iter().enumerate() {
        let p = s.m.transfers[i];
        let done = p >= 1.0;
        let label = if done { "Done".to_string() } else { format!("{:.0}%", p * 100.0) };
        rows = rows.child(
            col()
                .key(format!("row{i}"))
                .w_full()
                .gap(6.0)
                .on_press(move |s: &mut State| s.act(Action::Transfer(i as u8)))
                .child(
                    row()
                        .w_full()
                        .center_y()
                        .child(text(*name).font(font(13.0, 500.0)).color(c(color::TEXT)).truncate())
                        .child(spacer())
                        .child(RollingText::new(format!("pct{i}"), label).value(p as f64).font(nums(12.0, 560.0)).color(c(if done { color::GOOD } else { color::TEXT_2 })).trailing().build(k)),
                )
                .child(
                    el().w_full().h(6.0).radius(3.0).fill(c(color::WELL)).child(
                        el().w_frac(p.max(0.02)).h(6.0).radius(3.0).fill(c(if done { color::GOOD } else { color::ACCENT })).motion(travel),
                    ),
                ),
        );
    }
    let active = s.m.transfers.iter().filter(|p| **p < 1.0).count();
    let aside = RollingText::new("active", format!("{active} active")).value(active as f64).font(font(12.0, 460.0)).color(c(color::TEXT_3)).build(k);
    (aside, rows)
}

fn thousands(n: u32) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn visitors(s: &State, k: &Kit, h: f32) -> (El, El) {
    let count = row()
        .key("count")
        .on_press(|s: &mut State| s.act(Action::Visitors))
        .child(RollingText::new("visitors", thousands(s.m.visitors)).value(s.m.visitors as f64).font(nums(40.0, 660.0)).color(c(color::TEXT)).build(k));
    let chart_h = h - 52.0 - 18.0 - 14.0;
    let newest = s.m.visits.last().map(|v| v.id);
    let mut chart = row().w_full().h(chart_h).items_end().gap(4.0).motion(spring(s.springs.layout));
    for v in &s.m.visits {
        let fresh = Some(v.id) == newest;
        chart = chart.child(
            el().key(format!("visit{}", v.id))
                .grow(1.0)
                .w(0.0)
                .h(4.0 + (chart_h - 4.0) * v.count as f32 / VISIT_MAX as f32)
                .radius(3.0)
                .fill(c(if fresh { color::GOOD } else { lch(0.4, 0.06, 160.0) }))
                .transition(Transition::scale_anchor(0.05, rune::Vec2::new(0.5, 1.0))),
        );
    }
    let body = col().gap(4.0).child(count).child(caption("Unique visitors today")).child(el().h(10.0)).child(chart);
    let live = row().gap(6.0).center_y().child(el().size(6.0, 6.0).radius(3.0).fill(c(color::GOOD))).child(caption("Live"));
    (live, body)
}

#[cfg(test)]
mod tests {
    #[test]
    fn thousands_groups_digits() {
        assert_eq!(super::thousands(1284), "1,284");
        assert_eq!(super::thousands(999), "999");
        assert_eq!(super::thousands(1_000_000), "1,000,000");
    }
}
