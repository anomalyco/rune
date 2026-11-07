//! Soft Radar's dots and keyframes (`@opencode/ui` components/spinner-radar.ts): a 5×5 dot grid
//! whose lit head sweeps clockwise around each square ring, leaving a fading trail, with the
//! centre dot steady. Computed as the web does, in f64 with its 3-decimal rounding, so every dot,
//! stop and resting pose lands where Desktop's do.

use std::sync::OnceLock;

use rune::Cycle;

const SPAN: i32 = 5;
/// Dot side and pitch, in a 16-unit box.
const SIZE: f64 = 2.0;
const PITCH: f64 = 3.0;
/// Seconds per loop at 1×.
pub(super) const LOOP: f32 = 1.8;
/// Resting opacity of an unlit dot.
const DIM: f64 = 0.2;
/// The trail's ease-in-out fade over 48% of the loop, sampled every 4%, from a level below the head's.
const FADE: [f64; 13] = [1.0, 0.987, 0.934, 0.812, 0.599, 0.384, 0.238, 0.143, 0.081, 0.041, 0.017, 0.004, 0.0];
/// Wakes while only spinners move: a display's rate, as the compositor runs Desktop's.
const FPS: f32 = 60.0;

/// One dot in the 16-unit box: where it sits, its opacity under reduced motion (the sweep frozen
/// a fifth of the way in) and, unless it's the steady centre, its loop.
pub(super) struct Dot {
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub still: f32,
    pub cycle: Option<Cycle>,
}

pub(super) fn dots() -> &'static [Dot] {
    static DOTS: OnceLock<Vec<Dot>> = OnceLock::new();
    DOTS.get_or_init(layout)
}

type Stops = Vec<(f64, f64)>;

fn layout() -> Vec<Dot> {
    let trail = trail();
    let cells: Vec<(i32, i32)> = (0..SPAN * SPAN).map(|i| (i % SPAN - (SPAN - 1) / 2, i / SPAN - (SPAN - 1) / 2)).collect();
    let steady = |c: (i32, i32)| c == (0, 0);
    let ring = |(gx, gy): (i32, i32)| gx.abs().max(gy.abs());
    // Each dot's shape depends on how far it sits from its neighbours on its ring, in loop fractions.
    let keys: Vec<Option<(f64, f64)>> = cells
        .iter()
        .map(|&c| {
            if steady(c) {
                return None;
            }
            let others: Vec<f64> = cells.iter().filter(|&&o| o != c && !steady(o) && ring(o) == ring(c)).map(|&o| angle(o)).collect();
            let behind = others.iter().map(|&o| frac(angle(c) - o)).fold(f64::INFINITY, f64::min);
            let ahead = others.iter().map(|&o| frac(o - angle(c))).fold(f64::INFINITY, f64::min);
            Some((round(behind), round(ahead)))
        })
        .collect();
    let mut unique: Vec<(f64, f64)> = Vec::new();
    for key in keys.iter().flatten() {
        if !unique.contains(key) {
            unique.push(*key);
        }
    }
    let shapes: Vec<Stops> = unique.iter().map(|&(behind, ahead)| head_stops(&trail, behind * 100.0, ahead * 100.0)).collect();
    let frames: Vec<&'static [(f32, f32)]> = shapes
        .iter()
        .map(|stops| {
            let frames: Vec<(f32, f32)> = stops.iter().map(|&(percent, level)| ((round(percent) / 100.0) as f32, round(DIM + (1.0 - DIM) * level) as f32)).collect();
            &*Box::leak(frames.into_boxed_slice())
        })
        .collect();
    cells
        .iter()
        .zip(&keys)
        .map(|(&c, key)| {
            let shape = key.and_then(|k| unique.iter().position(|&u| u == k)).unwrap_or(0);
            let at = angle(c);
            // The steady centre glows like the fresh trail, so the head stays the brightest dot.
            let level = if steady(c) { trail[0].1 } else { level_at(&shapes[shape], frac(0.22 - at) * 100.0) };
            let still = round(DIM + (1.0 - DIM) * level) as f32;
            Dot {
                x: round(8.0 + f64::from(c.0) * PITCH - SIZE / 2.0) as f32,
                y: round(8.0 + f64::from(c.1) * PITCH - SIZE / 2.0) as f32,
                size: SIZE as f32,
                still,
                // Every dot shares one loop on the wall clock, so all spinners on screen stay in
                // phase (`syncSpinPhase`); a dot lights `at` of the way around.
                cycle: key.map(|_| Cycle { period: LOOP, phase: round(at) as f32, stops: frames[shape], still, fps: FPS }),
            }
        })
        .collect()
}

/// The trail a dot leaves once the head has passed: (percent of the loop, level from 0 dim to 1 lit).
fn trail() -> Stops {
    let mut trail: Stops = FADE.iter().enumerate().map(|(i, &level)| (i as f64 * 4.0, round(0.55 * level))).collect();
    trail.push((100.0, 0.0));
    trail
}

/// One dot's level through the loop, 0% being the moment the head passes it, with `behind` and
/// `ahead` the percent of the loop to its neighbours on its ring. It's lit while it's the dot
/// nearest the head, then the trail takes over.
fn head_stops(trail: &[(f64, f64)], behind: f64, ahead: f64) -> Stops {
    let edge = 1.5;
    let off = ahead / 2.0 + edge;
    let on = 100.0 - behind / 2.0;
    let mut stops = vec![(0.0, 1.0), (ahead / 2.0, 1.0), (off, level_at(trail, off))];
    stops.extend(trail.iter().copied().filter(|&(percent, _)| percent > off && percent < 100.0 && percent < on - edge));
    stops.extend([(on - edge, 0.0), (on, 1.0), (100.0, 1.0)]);
    stops
}

/// Clockwise from 12 o'clock, 0–1.
fn angle((gx, gy): (i32, i32)) -> f64 {
    frac(f64::from(gx).atan2(-f64::from(gy)) / std::f64::consts::TAU)
}

fn level_at(stops: &[(f64, f64)], percent: f64) -> f64 {
    match stops.iter().position(|&(at, _)| at >= percent) {
        None | Some(0) => stops.first().map_or(0.0, |s| s.1),
        Some(next) => {
            let ((p0, l0), (p1, l1)) = (stops[next - 1], stops[next]);
            let span = if p1 - p0 == 0.0 { 1.0 } else { p1 - p0 };
            l0 + (l1 - l0) * (percent - p0) / span
        }
    }
}

fn frac(v: f64) -> f64 {
    v - v.floor()
}

/// JavaScript's `Math.round(v * 1000) / 1000`: halves round up.
fn round(v: f64) -> f64 {
    (v * 1000.0 + 0.5).floor() / 1000.0
}
