//! Shared-element motion with `El::matched`: a 2×2 grid of picture tiles. 1–4 picks a tile, ↩
//! (or a click) zooms it to the whole stage and Esc zooms it back. Both places are plain flex
//! layout; the tile flies between them because both carry the same `matched` id.
//!
//! ```sh
//! cargo run -p matched
//! cargo run -p matched -- --script examples/matched/scripts/zoom.json --frames 240 --out frames
//! ```

#![forbid(unsafe_code)]

use rune::tok::{self, type_};
use rune::{AppSpec, Color, Cx, El, Key, KeyEvent, Pixels, SurfaceFit, TextStyle, Transition, Tx, col, el, row, text};

struct State {
    picked: usize,
    zoomed: bool,
    pictures: [Pixels; 4],
}

/// Sky, ground and sun colours of each tile's picture, as sRGB bytes.
const SCENES: [([u8; 3], [u8; 3], [u8; 3]); 4] = [
    ([255, 166, 120], [58, 40, 92], [255, 236, 170]),
    ([120, 200, 255], [30, 90, 70], [255, 255, 235]),
    ([196, 150, 255], [40, 30, 70], [255, 200, 240]),
    ([255, 214, 120], [120, 60, 30], [255, 250, 220]),
];

const NAMES: [&str; 4] = ["Dusk", "Lake", "Haze", "Dune"];

fn picture(i: usize) -> Pixels {
    let (w, h) = (320u32, 240u32);
    let (sky, ground, sun) = SCENES[i];
    let mix = |a: [u8; 3], b: [u8; 3], t: f32| a.map(|v| v as f32).iter().zip(b).map(|(a, b)| a + (b as f32 - a) * t).collect::<Vec<_>>();
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            let horizon = 0.62 + 0.04 * (u * 6.0 + i as f32).sin();
            // Ground coverage of this pixel, anti-aliased over one pixel row.
            let land = ((v - horizon) * h as f32 + 0.5).clamp(0.0, 1.0);
            let air = mix(sky, ground, (v / horizon).min(1.0).powf(2.2) * 0.5);
            let d = ((u - 0.3 - 0.12 * i as f32).powi(2) + ((v - 0.42) * 0.75).powi(2)).sqrt();
            let glow = (1.0 - (d / 0.16)).clamp(0.0, 1.0).powf(0.6);
            let air: Vec<f32> = air.iter().zip(sun).map(|(a, b)| a + (b as f32 - a) * glow).collect();
            let soil = mix(ground, [10, 10, 16], (v - horizon).max(0.0) * 1.6);
            rgba.extend(air.iter().zip(soil).map(|(a, s)| (a + (s - a) * land).clamp(0.0, 255.0) as u8));
            rgba.push(255);
        }
    }
    let p = Pixels::new();
    p.set(w, h, rgba);
    p
}

fn init() -> State {
    State { picked: 0, zoomed: false, pictures: std::array::from_fn(picture) }
}

fn tile(s: &State, i: usize) -> El {
    let id = format!("tile-{}", i + 1);
    let mut t = el().key(id.as_str()).matched(id.as_str()).grow(1.0).radius(20.0).clip_rounded(20.0)
        .fill(Color::BLACK)
        .on_press(move |s: &mut State| {
            if s.zoomed { s.zoomed = false } else { (s.picked, s.zoomed) = (i, true) }
        })
        .child(el().w_full().h_full().pixels_fit(&s.pictures[i], SurfaceFit::Fill))
        .child(text(format!("{}  {}", i + 1, NAMES[i])).font(TextStyle::new(22.0, 600.0)).color(Color::WHITE).absolute(18.0, 14.0));
    if s.picked == i {
        t = t.child(el().absolute(0.0, 0.0).w_full().h_full().radius(20.0).border(3.0, Color::WHITE));
    }
    t
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let theme = tok::theme(true);
    let hint = text("1–4 pick    ↩ zoom    esc back").font(type_::LABEL).color(theme.text_secondary);
    let stage = col().key("stage").w_full().grow(1.0);
    let stage = if s.zoomed {
        stage.child(tile(s, s.picked))
    } else {
        // The grid's own entrance and exit; the matched tile rides above it.
        let pair = |a: usize| row().key(format!("row-{a}")).grow(1.0).gap(12.0).child(tile(s, a)).child(tile(s, a + 1));
        stage.child(col().key("grid").grow(1.0).gap(12.0).transition(Transition::scale(0.96))
            .child(pair(0)).child(pair(2)))
    };
    col().w_full().h_full().padding(24.0).gap(14.0).fill(theme.bg).child(hint).child(stage)
}

fn on_key(s: &mut State, e: &KeyEvent, _: &mut Tx) -> bool {
    match &e.key {
        Key::Text(t) if !s.zoomed && let Some(n @ 1..=4) = t.parse::<usize>().ok() => s.picked = n - 1,
        Key::Enter => s.zoomed = true,
        Key::Escape => s.zoomed = false,
        _ => return false,
    }
    true
}

fn main() {
    rune::run(AppSpec::<State> {
        scenes: &[("grid", |_| {}), ("one", |s| s.zoomed = true)],
        on_key: Some(on_key),
        ..AppSpec::new("Matched", (720.0, 480.0), init, view)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn enter_zooms_the_picked_tile_from_its_grid_cell() {
        let mut rt = Runtime::new(init(), view, (720.0, 480.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.update(|s, _| s.picked = 3);
        rt.frame(1.0 / 60.0);
        let cell = rt.presented("tile-4").unwrap().0;
        rt.update(|s, _| s.zoomed = true);
        rt.frame(1.0 / 120.0);
        let first = rt.presented("tile-4").unwrap().0;
        assert!((first[0] - cell[0]).abs() < 8.0 && (first[2] - cell[2]).abs() < 8.0, "{cell:?} -> {first:?}");
        for _ in 0..120 {
            rt.frame(1.0 / 60.0);
        }
        let one = rt.presented("tile-4").unwrap().0;
        assert!(one[2] > 600.0 && one[3] > 350.0, "fills the stage: {one:?}");
    }
}
