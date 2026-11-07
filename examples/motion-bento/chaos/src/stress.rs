//! Stress mode: a wall of live controls, each a small bento of its own, that all move at once,
//! so both builds can be loaded until one of them misses its frames.
//!
//! `--load N` holds N controls. `--ramp A:B` steps from A controls to B, ×√2 a step, holding each
//! for `--hold` seconds. Everything a control shows is a pure function of the seed, its id and the
//! show time, so the two builds draw the same wall at the same moment without sharing anything
//! but this module: each control's bars retarget every 0.16–0.46 s, its reading changes every
//! frame, its switch flips, its tint drifts; every 1.7 s a twelfth of the wall goes wide and every
//! 2 s the whole wall reshuffles, so every control flies to a new place.

use crate::look::{Lch, grid, lch};
use crate::schedule::START;

/// Bars in each control's meter.
pub const BARS: usize = 8;
/// The header above the wall: the title, the load and the HUD.
pub const HEADER: f32 = 120.0;
/// Seconds between reshuffles of the whole wall.
pub const SHUFFLE: f64 = 2.0;
/// Seconds between changes of which controls are wide.
pub const WIDE_EVERY: f64 = 1.7;
/// One control in this many is wide.
pub const WIDE_SHARE: usize = 12;
pub const STATUS: [&str; 6] = ["Idle", "Heat", "Cool", "Boost", "Eco", "Auto"];
pub const HUES: [f32; 5] = [245.0, 200.0, 290.0, 165.0, 70.0];
/// A control's width over its height.
const ASPECT: f32 = 1.5;

/// How many controls the wall holds over time.
#[derive(Clone, Debug, PartialEq)]
pub enum Load {
    Fixed(usize),
    /// Each level for `hold` seconds, the first from [`START`].
    Ramp { levels: Vec<usize>, hold: f64 },
}

impl Load {
    /// From `from` to `to` controls, ×√2 a step (rounded), `to` last.
    pub fn ramp(from: usize, to: usize, hold: f64) -> Load {
        let mut levels = Vec::new();
        let mut n = from.max(1) as f64;
        while (n.round() as usize) < to {
            levels.push(n.round() as usize);
            n *= std::f64::consts::SQRT_2;
        }
        levels.push(to.max(from.max(1)));
        Load::Ramp { levels, hold }
    }

    /// `A:B`, as `--ramp` takes it.
    pub fn parse_ramp(spec: &str, hold: f64) -> Result<Load, String> {
        let (a, b) = spec.split_once(':').ok_or_else(|| format!("--ramp wants FROM:TO, got {spec}"))?;
        let n = |s: &str| s.trim().parse::<usize>().map_err(|_| format!("--ramp: not a count: {s}"));
        let (a, b) = (n(a)?, n(b)?);
        if a == 0 || b < a {
            return Err(format!("--ramp wants 0 < FROM ≤ TO, got {spec}"));
        }
        Ok(Load::ramp(a, b, hold))
    }

    pub fn levels(&self) -> &[usize] {
        match self {
            Load::Fixed(n) => std::slice::from_ref(n),
            Load::Ramp { levels, .. } => levels,
        }
    }

    /// The level index at show time `t`.
    pub fn step_at(&self, t: f64) -> usize {
        match self {
            Load::Fixed(_) => 0,
            Load::Ramp { levels, hold } => (((t - START).max(0.0) / hold) as usize).min(levels.len() - 1),
        }
    }

    pub fn at(&self, t: f64) -> usize {
        self.levels()[self.step_at(t)]
    }

    /// The show a ramp needs: every level held once. `None` for a fixed load.
    pub fn duration(&self) -> Option<f64> {
        match self {
            Load::Fixed(_) => None,
            Load::Ramp { levels, hold } => Some(START + levels.len() as f64 * hold),
        }
    }
}

/// The wall's rectangle in window points, below the header.
pub fn area() -> [f32; 4] {
    let y = grid::PAD - 8.0 + HEADER + grid::GAP;
    [grid::PAD, y, grid::WIDTH - 2.0 * grid::PAD, grid::HEIGHT - grid::PAD - y]
}

/// A control's accent, its bars' top and bottom.
pub fn tint(hue: f32) -> Lch {
    lch(0.72, 0.15, hue)
}
pub fn tint_hi(hue: f32) -> Lch {
    lch(0.82, 0.12, hue)
}
pub fn tint_lo(hue: f32) -> Lch {
    lch(0.52, 0.16, hue)
}
/// A hot control's card and edge.
pub fn hot_fill(hue: f32) -> Lch {
    lch(0.235, 0.035, hue)
}
pub fn hot_edge(hue: f32) -> Lch {
    lch(0.72, 0.15, hue).alpha(0.55)
}

/// One control at one moment.
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub id: u32,
    /// x, y, w, h in window points.
    pub rect: [f32; 4],
    pub wide: bool,
    pub hue: f32,
    pub on: bool,
    /// Its bars average over 0.6: the card takes its tint.
    pub hot: bool,
    /// 0..1 of the meter's height.
    pub bars: [f32; BARS],
    pub progress: f32,
    pub status: &'static str,
    reading: f64,
}

impl Cell {
    pub fn name(&self) -> String {
        format!("Zone {}", self.id + 1)
    }

    /// Changes every frame.
    pub fn reading(&self) -> String {
        format!("{:.2}", self.reading)
    }

    pub fn percent(&self) -> String {
        format!("{:.0}%", self.progress * 100.0)
    }
}

/// Every size inside a control, from the height of the wall's rows, so a control looks the same
/// from 16 on the wall to a thousand, only smaller.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    /// The scale: 1 is a 100-point-tall control.
    pub u: f32,
    pub pad: f32,
    pub gap: f32,
    pub radius: f32,
    pub name: f32,
    pub reading: f32,
    pub small: f32,
    pub dot: f32,
    pub bar_gap: f32,
    pub bars_h: f32,
    pub foot_h: f32,
    pub track_h: f32,
    pub switch_w: f32,
    pub switch_h: f32,
    pub knob: f32,
    pub shadow_y: f32,
    pub shadow_blur: f32,
}

/// Line height for a type size, as both builds set it.
pub fn lh(size: f32) -> f32 {
    (size * 1.3).round()
}

impl Metrics {
    pub fn for_height(h: f32) -> Metrics {
        let u = ((h / 100.0).clamp(0.25, 1.5) * 16.0).round() / 16.0;
        let (pad, gap, name, reading, small) = (7.0 * u, 4.0 * u, 11.0 * u, 16.0 * u, 10.0 * u);
        let foot_h = 12.0 * u;
        let bars_h = (h - 2.0 * pad - 3.0 * gap - lh(name) - lh(reading) - foot_h).max(2.0);
        Metrics {
            u,
            pad,
            gap,
            radius: 12.0 * u,
            name,
            reading,
            small,
            dot: 6.0 * u,
            bar_gap: 3.0 * u,
            bars_h,
            foot_h,
            track_h: 4.0 * u,
            switch_w: 22.0 * u,
            switch_h: 12.0 * u,
            knob: 10.0 * u,
            shadow_y: 4.0 * u,
            shadow_blur: 12.0 * u,
        }
    }
}

fn mix(seed: u64, id: u32, salt: u32, epoch: u64) -> u64 {
    let mut z = seed ^ (id as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ ((salt as u64) << 40) ^ epoch.wrapping_mul(0xd6e8_feb8_6659_fd93);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn unit(x: u64) -> f32 {
    (x >> 40) as f32 / (1u64 << 24) as f32
}

fn epoch(t: f64, every: f64, phase: f32) -> u64 {
    (t / every + phase as f64).max(0.0).floor() as u64
}

/// The wall: its controls at the last [`Wall::update`].
pub struct Wall {
    seed: u64,
    cells: Vec<Cell>,
    /// Ids in the order they flow into the grid, for `order_epoch`.
    order: Vec<u32>,
    order_epoch: Option<(u64, usize)>,
    wide: Vec<bool>,
    wide_epoch: Option<(u64, usize)>,
    pub metrics: Metrics,
    pub cols: usize,
    pub rows: usize,
}

impl Wall {
    pub fn new(seed: u64) -> Wall {
        Wall {
            seed,
            cells: Vec::new(),
            order: Vec::new(),
            order_epoch: None,
            wide: Vec::new(),
            wide_epoch: None,
            metrics: Metrics::for_height(100.0),
            cols: 1,
            rows: 1,
        }
    }

    /// The controls by id: a control keeps its index for as long as it's on the wall.
    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// Moves the wall to show time `t` with `n` controls.
    pub fn update(&mut self, t: f64, n: usize) {
        let seed = self.seed;
        self.cells.truncate(n);
        while self.cells.len() < n {
            let id = self.cells.len() as u32;
            self.cells.push(Cell { id, rect: [0.0; 4], wide: false, hue: 0.0, on: false, hot: false, bars: [0.0; BARS], progress: 0.0, status: "", reading: 0.0 });
        }
        let e = epoch(t, SHUFFLE, 0.5);
        if self.order_epoch != Some((e, n)) {
            self.order_epoch = Some((e, n));
            self.order = (0..n as u32).collect();
            self.order.sort_unstable_by_key(|&id| mix(seed, id, 1, e));
        }
        let e = epoch(t, WIDE_EVERY, 0.0);
        if self.wide_epoch != Some((e, n)) {
            self.wide_epoch = Some((e, n));
            let k = n / WIDE_SHARE;
            let mut ids: Vec<u32> = (0..n as u32).collect();
            if k > 0 {
                ids.select_nth_unstable_by_key(k - 1, |&id| mix(seed, id, 2, e));
            }
            self.wide = vec![false; n];
            for &id in &ids[..k] {
                self.wide[id as usize] = true;
            }
        }
        for c in &mut self.cells {
            let id = c.id;
            let h = |salt: u32| unit(mix(seed, id, salt, 0));
            let eb = epoch(t, 0.16 + 0.3 * h(3) as f64, h(4));
            for (j, b) in c.bars.iter_mut().enumerate() {
                *b = 0.1 + 0.9 * unit(mix(seed, id, 100 + j as u32, eb));
            }
            let level = c.bars.iter().sum::<f32>() / BARS as f32;
            c.hot = level > 0.6;
            c.on = unit(mix(seed, id, 5, epoch(t, 0.7 + 1.2 * h(6) as f64, h(7)))) < 0.5;
            c.hue = HUES[(mix(seed, id, 8, epoch(t, 3.0, h(9))) % HUES.len() as u64) as usize];
            c.progress = ((t * (0.05 + 0.25 * h(10) as f64) + h(11) as f64).fract()) as f32;
            c.status = STATUS[(mix(seed, id, 12, epoch(t, 1.1, h(13))) % STATUS.len() as u64) as usize];
            c.reading = 20.0 + 980.0 * h(14) as f64 + t * (1.5 + 30.0 * h(15) as f64);
            c.wide = self.wide[id as usize];
        }
        self.flow();
    }

    /// Places the controls in reading order: a wide control takes two columns, or one at a
    /// row's end. The rows are sized for `n + n / WIDE_SHARE` slots, so they never overflow.
    fn flow(&mut self) {
        let n = self.cells.len();
        let [ax, ay, aw, ah] = area();
        let slots = (n + n / WIDE_SHARE).max(1);
        let cols = (((slots as f32) * (aw / ah) / ASPECT).sqrt().round() as usize).clamp(1, slots);
        let rows = slots.div_ceil(cols);
        let gap = (10.0 * (40.0 / slots as f32).sqrt()).clamp(3.0, 12.0);
        let cw = (aw - gap * (cols - 1) as f32) / cols as f32;
        let ch = (ah - gap * (rows - 1) as f32) / rows as f32;
        self.metrics = Metrics::for_height(ch);
        self.cols = cols;
        self.rows = rows;
        let (mut col, mut row) = (0usize, 0usize);
        for &id in &self.order {
            let c = &mut self.cells[id as usize];
            let span = if c.wide && col + 2 <= cols { 2 } else { 1 };
            c.rect = [ax + col as f32 * (cw + gap), ay + row as f32 * (ch + gap), span as f32 * cw + (span - 1) as f32 * gap, ch];
            col += span;
            if col >= cols {
                col = 0;
                row += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ramp_steps_by_root_two_and_ends_on_its_target() {
        let l = Load::parse_ramp("16:256", 2.5).unwrap();
        assert_eq!(l.levels(), [16, 23, 32, 45, 64, 91, 128, 181, 256]);
        assert_eq!(l.at(0.0), 16);
        assert_eq!(l.at(START + 2.5), 23);
        assert_eq!(l.at(1e6), 256);
        assert_eq!(l.duration(), Some(START + 9.0 * 2.5));
        assert!(Load::parse_ramp("8", 1.0).is_err());
    }

    #[test]
    fn the_wall_is_a_function_of_time_and_fits_its_area() {
        let [ax, ay, aw, ah] = area();
        for n in [1, 16, 91, 512] {
            let mut a = Wall::new(7);
            let mut b = Wall::new(7);
            for t in [0.0, 1.3, 2.71, 6.05] {
                a.update(t, n);
            }
            b.update(6.05, n);
            assert_eq!(a.cells(), b.cells(), "{n} controls");
            for c in a.cells() {
                let [x, y, w, h] = c.rect;
                assert!(x >= ax - 0.01 && y >= ay - 0.01 && x + w <= ax + aw + 0.01 && y + h <= ay + ah + 0.01, "{n}: {:?}", c.rect);
            }
            assert_eq!(a.cells().iter().filter(|c| c.wide).count(), n / WIDE_SHARE);
        }
    }

    #[test]
    fn readings_change_every_frame() {
        let mut w = Wall::new(3);
        w.update(1.0, 32);
        let before: Vec<String> = w.cells().iter().map(Cell::reading).collect();
        w.update(1.0 + 1.0 / 120.0, 32);
        assert!(w.cells().iter().zip(&before).all(|(c, b)| c.reading() != *b));
    }
}
