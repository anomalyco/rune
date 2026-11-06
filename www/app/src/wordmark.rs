//! ᚱUNE on Anomaly's pixel grid: letters 11 pixels tall, 2-pixel stems, notched corners, fitted one
//! clear pixel apart. The R is the rune ᚱ (raidō), so the name is spelled with a real rune and
//! still reads as RUNE. Drawn as one shape, so the static page gets it as an inline SVG.
//!
//! A light runs down the ᚱ's strokes on hover (and every 5 to 9 s in the masthead while it's on
//! screen): each cell warms in 40 ms and cools with a 180 ms time constant.

use rune::{El, Shape, canvas};

use crate::tokens::{glint, ink};

const RUNE: [&str; 11] = [
    "###....", "####...", "##.##..", "##..##.", "##..##.", "##.##..", "####...", "##.##..", "##..##.", "##...##", "##...##",
];
const U: [&str; 11] = [
    "##...##", "##...##", "##...##", "##...##", "##...##", "##...##", "##...##", "##...##", "##...##", "#######", ".#####.",
];
const N: [&str; 11] = [
    "###...##", "####..##", "####..##", "####..##", "##.##.##", "##.##.##", "##.##.##", "##..####", "##..####", "##..####",
    "##...###",
];
const E: [&str; 11] = [
    "######", "######", "##....", "##....", "#####.", "#####.", "##....", "##....", "##....", "######", "######",
];

pub const CAP: usize = 11;

/// The glint's travel along the ᚱ, its rise and its cooling time constant, in seconds.
const TRAVEL: f64 = 0.7;
const RISE: f64 = 0.04;
const TAU: f64 = 0.18;

fn ink_cells(rows: &[&str]) -> Vec<(i32, i32)> {
    rows.iter().enumerate().flat_map(|(y, r)| r.bytes().enumerate().filter(|&(_, c)| c == b'#').map(move |(x, _)| (x as i32, y as i32))).collect()
}

/// Where `next` starts after `prev` (placed at `at`): as far left as keeps one clear pixel
/// between their ink in every direction, diagonals included.
fn fit(prev: &[&str], at: i32, next: &[&str]) -> i32 {
    let a: Vec<_> = ink_cells(prev).into_iter().map(|(x, y)| (x + at, y)).collect();
    let b = ink_cells(next);
    let mut x = at + 1;
    while a.iter().any(|&(ax, ay)| b.iter().any(|&(bx, by)| (bx + x - ax).abs() <= 1 && (by - ay).abs() <= 1)) {
        x += 1;
    }
    x
}

pub struct Glyphs {
    /// Each glyph's rows and where it starts.
    pub glyphs: Vec<(&'static [&'static str], i32)>,
    pub width: usize,
    /// The ᚱ's cells with their place along its carve path, 0 to 1.
    pub carve: Vec<(i32, i32, f32)>,
}

impl Glyphs {
    pub fn new() -> Glyphs {
        let mut glyphs: Vec<(&'static [&'static str], i32)> = Vec::new();
        for rows in [&RUNE[..], &U[..], &N[..], &E[..]] {
            let x = glyphs.last().map_or(0, |&(prev, at)| fit(prev, at, rows));
            glyphs.push((rows, x));
        }
        let &(last, at) = glyphs.last().unwrap();
        let width = at as usize + last[0].len();
        Glyphs { glyphs, width, carve: carve_order(&RUNE) }
    }
}

/// Rows to rectangles, merging each row's runs so no seams show between pixels.
fn runs(rows: &[&str], dx: i32, out: &mut Vec<[f32; 4]>) {
    for (y, row) in rows.iter().enumerate() {
        let row = row.as_bytes();
        let mut x = 0;
        while x < row.len() {
            if row[x] != b'#' {
                x += 1;
                continue;
            }
            let start = x;
            while x < row.len() && row[x] == b'#' {
                x += 1;
            }
            out.push([(dx + start as i32) as f32, y as f32, (x - start) as f32, 1.0]);
        }
    }
}

fn rect(r: [f32; 4]) -> Vec<[f32; 2]> {
    let [x, y, w, h] = r;
    vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]]
}

/// ᚱ's carve order: the stave top to bottom, then the bowl out and back, then the leg. Each
/// cell gets the arc length (0 to 1) of its nearest point on that path.
fn carve_order(rows: &[&str]) -> Vec<(i32, i32, f32)> {
    let path: [[[f32; 2]; 2]; 4] =
        [[[1.0, 0.5], [1.0, 10.5]], [[1.5, 0.5], [5.0, 3.5]], [[5.0, 3.5], [1.5, 6.5]], [[2.5, 6.5], [6.0, 10.5]]];
    let lens: Vec<f32> = path.iter().map(|[a, b]| (b[0] - a[0]).hypot(b[1] - a[1])).collect();
    let total: f32 = lens.iter().sum();
    ink_cells(rows)
        .into_iter()
        .map(|(x, y)| {
            let c = [x as f32 + 0.5, y as f32 + 0.5];
            let (mut best, mut arc, mut acc) = (f32::INFINITY, 0.0, 0.0);
            for (i, [a, b]) in path.iter().enumerate() {
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let p = (((c[0] - a[0]) * dx + (c[1] - a[1]) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
                let d = (a[0] + dx * p - c[0]).hypot(a[1] + dy * p - c[1]);
                if d < best - 1e-6 {
                    best = d;
                    arc = acc + p * lens[i];
                }
                acc += lens[i];
            }
            (x, y, arc / total)
        })
        .collect()
}

fn ease_in_out(p: f64) -> f64 {
    if p < 0.5 { 2.0 * p * p } else { 1.0 - (-2.0 * p + 2.0).powi(2) / 2.0 }
}

/// When the head passes `order` (0 to 1) along the carve, in seconds after the start.
fn passes(order: f32) -> f64 {
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..20 {
        let m = (lo + hi) / 2.0;
        if ease_in_out(m) < f64::from(order) { lo = m } else { hi = m }
    }
    (lo + hi) / 2.0 * TRAVEL
}

/// How lit a cell at `order` is `t` seconds into a glint (0 to 1).
pub fn heat(order: f32, t: f64) -> f32 {
    let since = t - passes(order);
    if since < 0.0 {
        0.0
    } else if since < RISE {
        (since / RISE) as f32
    } else {
        let h = (-(since - RISE) / TAU).exp() as f32;
        if h > 0.004 { h } else { 0.0 }
    }
}

/// How long a glint runs before every cell has cooled.
pub const GLINT_SECONDS: f64 = TRAVEL + RISE + TAU * 6.0;

/// How the ᚱ is lit.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Glow {
    /// At rest.
    #[default]
    Off,
    /// A light running down the strokes, this many seconds in.
    Glint(f64),
    /// Reduced motion: the whole letter lit this much (0 to 1), with no travel.
    Lit(f32),
}

/// The wordmark at `cell` points a pixel, lit as `glow` says.
pub fn wordmark(g: &Glyphs, cell: f32, glow: Glow) -> El {
    let mut rects = Vec::new();
    for &(rows, x) in &g.glyphs {
        runs(rows, x, &mut rects);
    }
    let mut shape = Shape::new().view_box(g.width as f32, CAP as f32);
    for r in rects {
        shape = shape.fill(rect(r), ink());
    }
    let hot = glint();
    for &(x, y, order) in &g.carve {
        let h = match glow {
            Glow::Off => 0.0,
            Glow::Glint(t) => heat(order, t),
            Glow::Lit(level) => level,
        };
        if h > 0.0 {
            shape = shape.fill(rect([x as f32, y as f32, 1.0, 1.0]), hot.alpha(h));
        }
    }
    canvas(shape).key("wordmark").size(cell * g.width as f32, cell * CAP as f32).a11y_hidden()
}

/// Reduced motion's stand-in for a glint, `t` seconds in: the letter fades to lit in 90 ms,
/// holds half a second and fades back. None once it's over.
pub fn lit(t: f64) -> Option<f32> {
    const FADE: f64 = 0.09;
    const HOLD: f64 = 0.5;
    let level = if t < 0.0 {
        return None;
    } else if t < FADE {
        t / FADE
    } else if t < FADE + HOLD {
        1.0
    } else if t < 2.0 * FADE + HOLD {
        1.0 - (t - FADE - HOLD) / FADE
    } else {
        return None;
    };
    Some(level as f32)
}

/// When each wordmark's light started, and when the masthead's next idle one is due.
#[derive(Default)]
pub struct Glints {
    masthead: Option<f64>,
    brand: Option<f64>,
    next: Option<f64>,
    /// Reduced motion, as the view last saw it: idle glints skip.
    reduced: bool,
    seed: u32,
}

impl Glints {
    pub fn masthead(&mut self, now: f64) {
        start(&mut self.masthead, now);
    }

    pub fn brand(&mut self, now: f64) {
        start(&mut self.brand, now);
    }

    /// The masthead came on screen or left: idle glints run every 5 to 9 s while it shows.
    pub fn masthead_visible(&mut self, on: bool, reduced: bool, now: f64) {
        self.reduced = reduced;
        self.next = on.then(|| now + self.pause());
    }

    fn pause(&mut self) -> f64 {
        // xorshift: the gaps only need to not line up.
        let mut x = self.seed.wrapping_add(0x9e37_79b9) | 1;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.seed = x;
        5.0 + 4.0 * f64::from(x) / f64::from(u32::MAX)
    }

    /// Advances to `now`: returns (changed, still moving) and sets when to look again.
    pub fn tick(&mut self, now: f64) -> (bool, bool, Option<f64>) {
        if let Some(at) = self.next
            && now >= at
        {
            if !self.reduced {
                start(&mut self.masthead, now);
            }
            self.next = Some(now + self.pause());
        }
        let mut changed = false;
        let mut moving = false;
        for g in [&mut self.masthead, &mut self.brand] {
            if let Some(at) = *g {
                changed = true;
                if now - at >= GLINT_SECONDS {
                    *g = None;
                } else {
                    moving = true;
                }
            }
        }
        (changed, moving, self.next)
    }

    pub fn glow(&self, which: Which, now: f64, reduced: bool) -> Glow {
        let Some(at) = (match which {
            Which::Masthead => self.masthead,
            Which::Brand => self.brand,
        }) else {
            return Glow::Off;
        };
        let t = now - at;
        if reduced {
            lit(t).map_or(Glow::Off, Glow::Lit)
        } else if (0.0..GLINT_SECONDS).contains(&t) {
            Glow::Glint(t)
        } else {
            Glow::Off
        }
    }
}

#[derive(Clone, Copy)]
pub enum Which {
    Masthead,
    Brand,
}

/// Starts a light unless one is still running, so hovering in and out doesn't restart it.
fn start(at: &mut Option<f64>, now: f64) {
    if at.is_none_or(|a| now - a >= GLINT_SECONDS) {
        *at = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_glints_wait_five_to_nine_seconds_and_stop_off_screen() {
        let mut g = Glints::default();
        g.masthead_visible(true, false, 0.0);
        let (_, moving, next) = g.tick(1.0);
        let at = next.unwrap();
        assert!(!moving && (5.0..=9.0).contains(&at), "{at}");
        let (changed, moving, _) = g.tick(at);
        assert!(changed && moving);
        assert!(matches!(g.glow(Which::Masthead, at + 0.1, false), Glow::Glint(_)));
        g.masthead_visible(false, false, at + 0.2);
        let (_, _, next) = g.tick(at + GLINT_SECONDS + 1.0);
        assert_eq!(next, None);
        assert_eq!(g.glow(Which::Masthead, at + GLINT_SECONDS + 1.0, false), Glow::Off);
    }

    #[test]
    fn reduced_motion_lights_on_hover_only() {
        let mut g = Glints::default();
        g.masthead_visible(true, true, 0.0);
        let (_, _, next) = g.tick(0.0);
        let (changed, ..) = g.tick(next.unwrap());
        assert!(!changed, "no idle light under reduced motion");
        g.masthead(20.0);
        assert_eq!(g.glow(Which::Masthead, 20.3, true), Glow::Lit(1.0));
    }

    #[test]
    fn letters_fit_one_clear_pixel_apart() {
        let g = Glyphs::new();
        let starts: Vec<i32> = g.glyphs.iter().map(|&(_, x)| x).collect();
        assert_eq!(starts[0], 0);
        assert!(starts.windows(2).all(|w| w[1] > w[0]), "{starts:?}");
        assert_eq!(g.carve.len(), ink_cells(&RUNE).len());
        assert!(g.carve.iter().all(|&(_, _, o)| (0.0..=1.0).contains(&o)));
    }

    #[test]
    fn a_glint_warms_the_stave_first_and_cools_to_nothing() {
        assert!(heat(0.0, 0.03) > heat(0.9, 0.03));
        assert_eq!(heat(0.5, GLINT_SECONDS), 0.0);
    }
}
