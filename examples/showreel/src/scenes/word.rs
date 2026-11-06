//! ᚱUNE, the wordmark from the site's pixel grid, built from liquid-metal tiles that spring in
//! along the letters and then flow as one surface. `card` adds the repository line under it.

use rune::{Cx, El, el};

use crate::gfx::CHROME;
use crate::kit::*;
use crate::State;

const GLYPHS: [&str; 4] = [
    "###....|####...|##.##..|##..##.|##..##.|##.##..|####...|##.##..|##..##.|##...##|##...##",
    "##...##|##...##|##...##|##...##|##...##|##...##|##...##|##...##|##...##|#######|.#####.",
    "###...##|####..##|####..##|####..##|##.##.##|##.##.##|##.##.##|##..####|##..####|##..####|##...###",
    "######|######|##....|##....|#####.|#####.|##....|##....|##....|######|######",
];

fn ink(rows: &[&str], dx: i32) -> Vec<(i32, i32)> {
    let mut v = Vec::new();
    for (y, r) in rows.iter().enumerate() {
        for (x, ch) in r.chars().enumerate() {
            if ch == '#' {
                v.push((x as i32 + dx, y as i32));
            }
        }
    }
    v
}

/// Every inked cell of the wordmark, letters fitted one clear pixel apart (diagonals included).
pub fn cells() -> (Vec<(i32, i32, usize)>, i32) {
    let mut out: Vec<(i32, i32, usize)> = Vec::new();
    let mut at = 0;
    let mut width = 0;
    let mut prev: Option<Vec<(i32, i32)>> = None;
    for (g, glyph) in GLYPHS.iter().enumerate() {
        let rows: Vec<&str> = glyph.split('|').collect();
        let b = ink(&rows, 0);
        if let Some(a) = &prev {
            let mut x = at + 1;
            while a.iter().any(|(ax, ay)| b.iter().any(|(bx, by)| (bx + x - ax).abs() <= 1 && (by - ay).abs() <= 1)) {
                x += 1;
            }
            at = x;
        }
        let placed = ink(&rows, at);
        out.extend(placed.iter().map(|&(x, y)| (x, y, g)));
        width = at + rows[0].len() as i32;
        prev = Some(placed);
    }
    (out, width)
}

pub fn view(s: &State, _: &mut Cx<State>, card: bool) -> El {
    let t = s.t;
    let (cells, width) = cells();
    let cell = if card { 19.0 } else { 25.0 };
    let tile = cell - 1.0;
    let total_w = width as f32 * cell;
    let total_h = 11.0 * cell;
    // A slow push in, about the frame's centre.
    let zoom = if card { 1.0 } else { 1.0 + 0.05 * ease(t, 0.0, 4.0) };
    let ox = (W - total_w * zoom) / 2.0;
    let oy = if card { 168.0 } else { (H - total_h * zoom) / 2.0 };
    let mut root = el().w(W).h(H).fill(c(0x000000));
    for &(x, y, g) in &cells {
        // A wave left to right, each letter a beat behind the last, rows rippling down.
        let delay = 0.08 + g as f64 * 0.11 + x as f64 * 0.012 + y as f64 * 0.018;
        let k = track(t, 0.0, &[(delay, 1.0, sp(0.55, 0.38))]);
        let size = tile * zoom * k.max(0.0);
        let cx = ox + (x as f32 + 0.5) * cell * zoom;
        let cy = oy + (y as f32 + 0.5) * cell * zoom;
        if size < 0.5 {
            continue;
        }
        root = root.child(
            fx(CHROME, [4.0, 0.0, 1.25, 0.0])
                .key(format!("c{x}_{y}"))
                .absolute(cx - size / 2.0, cy - size / 2.0)
                .size(size, size)
                .radius(2.5 * zoom * k),
        );
    }
    if card {
        let a = ease(t, 0.9, 0.6);
        root = root.child(
            el().absolute(0.0, oy + total_h + 46.0 - 8.0 * a)
                .w(W)
                .center()
                .child(mono_a("github.com/anomalyco/rune", 22.0, 0xb8bcc6, a)),
        );
    }
    root
}
