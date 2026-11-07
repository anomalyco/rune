//! The top row: scenes, the thermostat and the lights.

use motion_bento_chaos::Action;
use motion_bento_chaos::look::{color, grid};
use motion_bento_chaos::model::{LIGHTS, SCENE_NOTES, SCENES};
use rune::{El, LineCap, Shape, canvas, col, el, row, spacer, text};
use rune_kit::{Kit, RollingText, Switch};

use crate::State;
use crate::style::{c, caption, font, inner_h, inner_w, nums, spring, tile, well_button};

const SEG_H: f32 = 44.0;
const SEG_PAD: f32 = 4.0;

pub fn scenes(s: &State, k: &Kit) -> El {
    let m = &s.m;
    let mood = color::SCENE[m.scene];
    let w = inner_w(2.0);
    let seg_w = (w - 2.0 * SEG_PAD) / SCENES.len() as f32;
    let snap = spring(s.springs.snap);
    let morph = spring(s.springs.morph);

    let mut segments = row()
        .w(w)
        .h(SEG_H)
        .radius(12.0)
        .fill(c(color::WELL))
        .border(1.0, c(color::EDGE))
        .padding(SEG_PAD)
        .child(
            el().size(seg_w, SEG_H - 2.0 * SEG_PAD)
                .absolute(SEG_PAD + seg_w * m.scene as f32, SEG_PAD)
                .radius(9.0)
                .fill(c(mood))
                .shadow(4.0, 14.0, c(mood.alpha(0.45)))
                .motion(snap),
        );
    for (i, name) in SCENES.iter().enumerate() {
        let on = i == m.scene;
        segments = segments.child(
            row()
                .key(format!("seg{i}"))
                .grow(1.0)
                .h_full()
                .center()
                .on_press(move |s: &mut State| s.act(Action::Scene(i as u8)))
                .child(text(*name).font(font(14.0, 560.0)).color(c(if on { color::ON_ACCENT } else { color::TEXT_2 })).motion(snap)),
        );
    }

    let lights_on = m.lights.iter().filter(|l| **l).count();
    let picked = m.chips.iter().filter(|c| **c).count();
    let stat = |label: &str, value: String| {
        col()
            .gap(2.0)
            .padding(10.0)
            .w(118.0)
            .radius(12.0)
            .fill(c(color::WELL))
            .child(caption(label))
            .child(RollingText::new(format!("stat-{label}"), value).font(nums(16.0, 600.0)).color(c(color::TEXT)).build(k))
    };
    let summary = row()
        .h(inner_h(grid::ROW1) - SEG_H - 16.0)
        .w(w)
        .center_y()
        .gap(10.0)
        .child(
            // The mood's glow, sliding under the summary toward the picked segment.
            el().size(260.0, 90.0)
                .absolute(-30.0 + seg_w * m.scene as f32 * 0.55, 6.0)
                .radius(45.0)
                .fill(c(mood.alpha(0.22)))
                .blur(36.0)
                .motion(morph),
        )
        .child(
            col()
                .gap(4.0)
                .grow(1.0)
                .child(RollingText::new("scene-name", SCENES[m.scene]).value(m.scene as f64).font(font(30.0, 650.0)).color(c(color::TEXT)).build(k))
                .child(text(SCENE_NOTES[m.scene]).font(font(13.0, 460.0)).color(c(color::TEXT_2))),
        )
        .child(stat("Lights", format!("{lights_on} of 4")))
        .child(stat("Target", m.temp_label()))
        .child(stat("Genres", picked.to_string()));

    let swatch = row()
        .gap(8.0)
        .center_y()
        .child(caption("Mood"))
        .child(el().size(14.0, 14.0).radius(7.0).fill(c(mood)).shadow(0.0, 8.0, c(mood.alpha(0.6))).motion(morph));
    tile("scene", "Scenes", swatch, col().gap(16.0).child(segments).child(summary)).w(inner_w(2.0) + 2.0 * grid::TILE_PAD)
}

/// The thermostat ring's arc: 270° from the lower left, clockwise, as points.
fn arc(size: f32, inset: f32) -> Vec<[f32; 2]> {
    let r = size / 2.0 - inset;
    let (a0, sweep) = (135f32.to_radians(), 270f32.to_radians());
    (0..=96)
        .map(|i| {
            let a = a0 + sweep * i as f32 / 96.0;
            [size / 2.0 + r * a.cos(), size / 2.0 + r * a.sin()]
        })
        .collect()
}

pub const RING: f32 = 150.0;

pub fn thermostat(s: &State, k: &Kit) -> El {
    let m = &s.m;
    let points = arc(RING, 9.0);
    let track = Shape::new().stroke_with(points.clone(), false, 10.0, LineCap::Round, c(color::WELL_HI));
    let fill = Shape::new().stroke_with(points, false, 10.0, LineCap::Round, c(color::ACCENT));
    let ring = el()
        .size(RING, RING)
        .child(canvas(track).size(RING, RING).absolute(0.0, 0.0))
        .child(canvas(fill).size(RING, RING).absolute(0.0, 0.0).live_trim(s.ring.smooth(spring(s.springs.travel))))
        .child(
            col()
                .size(RING, RING)
                .absolute(0.0, 0.0)
                .center()
                .gap(0.0)
                .child(RollingText::new("temp", m.temp_label()).value(m.temp as f64).font(nums(34.0, 640.0)).color(c(color::TEXT)).build(k))
                .child(caption("Target")),
        );
    let heating = m.temp > 21.0;
    let aside = row()
        .gap(6.0)
        .center_y()
        .child(el().size(8.0, 8.0).radius(4.0).fill(c(if heating { color::WARM } else { color::ACCENT })).motion(spring(s.springs.morph)))
        .child(RollingText::new("mode", if heating { "Heating" } else { "Cooling" }).font(font(12.0, 460.0)).color(c(color::TEXT_3)).build(k));
    let buttons = col()
        .gap(10.0)
        .child(well_button("up", "+", 52.0, 44.0).on_press(|s: &mut State| s.act(Action::TempUp)))
        .child(well_button("down", "−", 52.0, 44.0).on_press(|s: &mut State| s.act(Action::TempDown)));
    let body = row().h(inner_h(grid::ROW1)).center_y().gap(14.0).child(ring).child(spacer()).child(buttons);
    tile("thermo", "Thermostat", aside, body).w(inner_w(1.0) + 2.0 * grid::TILE_PAD)
}

pub fn lights(s: &State, k: &Kit) -> El {
    let m = &s.m;
    let on = m.lights.iter().filter(|l| **l).count();
    let mut rows = col().gap(4.0);
    for (i, name) in LIGHTS.iter().enumerate() {
        let lit = m.lights[i];
        rows = rows.child(
            row()
                .h(34.0)
                .center_y()
                .gap(12.0)
                .child(
                    el().size(10.0, 10.0)
                        .radius(5.0)
                        .fill(c(if lit { color::WARM } else { color::WELL_HI }))
                        .shadow(0.0, 10.0, c(if lit { color::WARM.alpha(0.7) } else { color::WARM.alpha(0.0) }))
                        .motion(spring(s.springs.snap)),
                )
                .child(text(*name).font(font(14.0, 500.0)).color(c(if lit { color::TEXT } else { color::TEXT_2 })))
                .child(spacer())
                .child(
                    Switch::new(lit)
                        .label(*name)
                        .hide_label()
                        .on_toggle(move |s: &mut State| s.act(Action::Light(i as u8)))
                        .build(k)
                        .key(format!("light{i}")),
                ),
        );
    }
    let aside = RollingText::new("lights-on", format!("{on} on")).value(on as f64).font(font(12.0, 460.0)).color(c(color::TEXT_3)).build(k);
    tile("lights", "Lights", aside, rows).w(inner_w(1.0) + 2.0 * grid::TILE_PAD)
}
