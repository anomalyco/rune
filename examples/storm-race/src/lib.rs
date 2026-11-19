//! Storm: a twin of a reference toolkit's stress window, for a measured race. N cells in a grid
//! (the first extra argument, default 1,000), each with a switch whose knob springs across, a bar
//! meter in a rounded well, a number that follows the bar, and a translucent rounded square that
//! pulses over its neighbours. Every spring is retargeted at seeded times, often mid-flight,
//! keeping its velocity, so everything moves all the time. A bar along the top shows N, frames a
//! second and the 95th percentile frame interval.
//!
//! The scene is a pure function of the clock: each retarget happens at its scheduled time, not
//! the frame's, so a frame at time t is the same picture at any frame rate, and the reference
//! app, with the same seed, schedule and springs, draws the same picture. The view is rebuilt
//! from state every frame, as the reference app rebuilds its scene, and the grid follows a
//! resize like the reference app's `st_resize`: it fills the window below the bar, whose size the
//! view reads from its root's laid-out size.

use rune::{Color, Cx, El, Spring, TextStyle, Tx, el, text};

/// The window's initial size in points; a resize moves the grid with it (see [`view`]). The
/// windowless bench and the headless run lay out at this size.
pub const SIZE: (f32, f32) = (1000.0, 720.0);
pub const TOP: f32 = 28.0;
const SEED: u64 = 20261118;
const GAPS: usize = 120;
const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

pub const KNOB: Spring = Spring::new(0.35, 0.25);
pub const BAR: Spring = Spring::new(0.4, 0.15);
pub const PULSE: Spring = Spring::new(0.8, 0.4);

/// SplitMix64: a uniform number in `0..1` and the stream's next state.
fn roll(rng: u64) -> (f32, u64) {
    let s = rng.wrapping_add(GOLDEN);
    let z1 = (s ^ (s >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let z2 = (z1 ^ (z1 >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    let z = z2 ^ (z2 >> 31);
    ((z >> 40) as f32 / 16_777_216.0, s)
}

/// One sprung value: the segment it is on (leaving `x0` with velocity `v0` for `target` at
/// `start`), when it is next retargeted, and its own random stream. `k` is 0 for a knob, 1 for a
/// bar and 2 for a pulse.
#[derive(Clone, Copy)]
pub struct Chan { x0: f32, v0: f32, target: f32, start: f32, next: f32, rng: u64 }

fn spring(k: u64) -> Spring { [KNOB, BAR, PULSE][k as usize] }
fn gap_lo(k: u64) -> f32 { [0.5, 0.12, 0.35][k as usize] }
fn gap_hi(k: u64) -> f32 { [1.5, 0.45, 1.1][k as usize] }

impl Chan {
    /// Channel `k` of cell `i` as the window opens: at rest at a seeded value, first retargeted
    /// at a seeded time within its longest gap.
    fn new(i: u64, k: u64) -> Chan {
        let (a, r) = roll(SEED.wrapping_add((i * 3 + k).wrapping_mul(GOLDEN)));
        let (b, rng) = roll(r);
        let v = match k { 0 => if a < 0.5 { 0.0 } else { 1.0 }, 1 => 0.08 + a * 0.92, _ => 0.55 + a * 0.9 };
        Chan { x0: v, v0: 0.0, target: v, start: 0.0, next: b * gap_hi(k), rng }
    }

    /// After every retarget due by `now`, each at its own scheduled time: a knob flips, a bar or
    /// pulse springs to a new seeded target, and the next gap is drawn.
    fn advance(&mut self, k: u64, now: f32) {
        while self.next <= now {
            let at = self.next;
            let (t, r) = roll(self.rng);
            let target = match k { 0 => 1.0 - self.target, 1 => 0.08 + t * 0.92, _ => 0.55 + t * 0.9 };
            let (gap, rng) = roll(if k == 0 { self.rng } else { r });
            let (x, v) = spring(k).sample(self.x0, self.v0, self.target, at - self.start);
            *self = Chan { x0: x, v0: v, target, start: at, next: at + gap_lo(k) + gap * (gap_hi(k) - gap_lo(k)), rng };
        }
    }

    fn at(&self, k: u64, now: f32) -> f32 {
        let t = now - self.start;
        if t > 0.0 { spring(k).sample(self.x0, self.v0, self.target, t).0 } else { self.x0 }
    }
}

#[derive(Clone, Copy)]
pub struct Cell { knob: Chan, bar: Chan, pulse: Chan }

pub struct State {
    pub n: usize,
    cells: Vec<Cell>,
    now: f32,
    /// The first frame's clock, which `now` counts from, and the last frame's.
    start: Option<f64>,
    last: Option<f64>,
    /// The last `GAPS` frame intervals in seconds, a ring written at `ticks % GAPS`.
    gaps: [f64; GAPS],
    ticks: usize,
    fps: u64,
    p95_tenths: u64,
}

impl State {
    pub fn new(n: usize) -> State {
        let cells = (0..n as u64).map(|i| Cell { knob: Chan::new(i, 0), bar: Chan::new(i, 1), pulse: Chan::new(i, 2) }).collect();
        State { n, cells, now: 0.0, start: None, last: None, gaps: [0.0; GAPS], ticks: 0, fps: 0, p95_tenths: 0 }
    }

    /// The clock moves to `clock` seconds: the frame interval goes in the ring, the top bar's
    /// numbers are refreshed every `GAPS / 4` frames, and every cell's due retargets happen.
    pub fn step(&mut self, clock: f64) {
        let start = *self.start.get_or_insert(clock);
        if let Some(last) = self.last { self.gaps[self.ticks % GAPS] = clock - last; }
        self.last = Some(clock);
        self.ticks += 1;
        if self.ticks % (GAPS / 4) == 0 {
            let have = (self.ticks - 1).min(GAPS);
            let mut sorted: Vec<f64> = (0..have).map(|j| self.gaps[(self.ticks - 1 - j) % GAPS]).collect();
            sorted.sort_by(f64::total_cmp);
            let sum: f64 = sorted.iter().sum();
            self.fps = if sum > 0.0 { (have as f64 / sum).round() as u64 } else { 0 };
            self.p95_tenths = sorted.get((have * 95 / 100).min(have.saturating_sub(1))).map_or(0, |g| (g * 1e4).round() as u64);
        }
        self.now = (clock - start) as f32;
        let now = self.now;
        for c in &mut self.cells {
            c.knob.advance(0, now);
            c.bar.advance(1, now);
            c.pulse.advance(2, now);
        }
    }
}

/// The first extra argument as N, or 1,000.
pub fn n_from_args() -> usize {
    rune::extra_args().first().and_then(|a| a.parse().ok()).unwrap_or(1000)
}

pub fn init() -> State { State::new(n_from_args()) }

/// Every frame moves the clock and asks for the next frame.
pub fn poll(s: &mut State, clock: f64, tx: &mut Tx) -> bool {
    s.step(clock);
    tx.wake_after(std::time::Duration::ZERO);
    true
}

fn c(r: u8, g: u8, b: u8) -> Color {
    let d = |v: u8| rune::color::srgb_decode(v as f32 / 255.0);
    Color { r: d(r), g: d(g), b: d(b), a: 1.0 }
}

fn hue(i: usize) -> Color {
    let [r, g, b] = [[229, 72, 77], [255, 159, 10], [48, 209, 88], [10, 132, 255], [191, 90, 242], [100, 210, 255]][i % 6];
    c(r, g, b)
}

/// The most nearly square grid of at least `n` cells filling `w` by `h` points: columns, cell
/// width, cell height.
pub fn grid(n: usize, w: f32, h: f32) -> (usize, f32, f32) {
    let m = n.max(1);
    let cols = ((m as f32 * w / h).sqrt().ceil() as usize).max(1);
    let rows = m.div_ceil(cols);
    (cols, w / cols as f32, h / rows as f32)
}

/// The grid fills the window below the bar: `Cx::layout_size` of the root, or `SIZE` before the
/// first layout (the windowless bench), so a resize moves the cells exactly as the reference
/// app's `st_resize` does.
pub fn view(s: &State, cx: &mut Cx<State>) -> El {
    let [w, h] = cx.layout_size("root").unwrap_or([SIZE.0, SIZE.1]);
    let (cols, gw, gh) = grid(s.n, w, h - TOP);
    let now = s.now;
    let (off, on, knob_c, well_c, num_c) = (c(58, 62, 70), c(48, 209, 88), c(245, 245, 247), c(30, 33, 38), c(155, 161, 170));
    let digits = TextStyle::new(gh * 0.26, 400.0);
    let mut root = el().key("root").w_full().h_full().fill(c(16, 18, 22));
    // The whole-view fold lands at its full size in one allocation instead of doubling from
    // zero each frame.
    root.reserve_children(cx.view_children());
    root.extend_children(s.cells.iter().enumerate().flat_map(|(i, cell)| {
        let x = (i % cols) as f32 * gw;
        let y = TOP + (i / cols) as f32 * gh;
        let k = cell.knob.at(0, now);
        let b = cell.bar.at(1, now);
        let p = cell.pulse.at(2, now);
        let side = gw.min(gh) * 0.8 * p;
        let alpha = 0.18 + 0.22 * ((p - 0.55) / 0.9).clamp(0.0, 1.0);
        let square = el().absolute(x + gw * 0.5 - side * 0.5, y + gh * 0.5 - side * 0.5).size(side, side)
            .radius(side * 0.3).fill(hue(i).alpha(alpha));
        let (ww, wh) = (gw * 0.16, gh * 0.8);
        let fill = b.clamp(0.0, 1.0);
        let well = el().absolute(x + gw * 0.08, y + gh * 0.1).size(ww, wh).radius(ww.min(wh) * 0.5)
            .clip_rounded(ww.min(wh) * 0.5).fill(well_c)
            .child(el().absolute(0.0, wh * (1.0 - fill)).size(ww, wh * fill).fill(hue(i + 2)));
        let (tw, th) = (gw * 0.56, gh * 0.26);
        let d = th * 0.8;
        let track = el().absolute(x + gw * 0.32, y + gh * 0.12).size(tw, th).radius(th * 0.5)
            .fill(off.mix(on, k.clamp(0.0, 1.0)))
            .child(el().absolute(th * 0.1 + k * (tw - th), th * 0.1).size(d, d).radius(d * 0.5).fill(knob_c));
        let shown = ((fill * 999.0 + 0.5) as u32).min(999);
        let number = text(shown.to_string()).absolute(x + gw * 0.32, y + gh * 0.5).font(digits).color(num_c);
        [square, well, track, number]
    }));
    let label = format!("storm   n {}   fps {}   p95 {}.{} ms", s.n, s.fps, s.p95_tenths / 10, s.p95_tenths % 10);
    root.child(
        el().absolute(0.0, 0.0).size(w, TOP).fill(c(8, 9, 11))
            .child(text(label).absolute(10.0, 6.0).font(TextStyle::new(13.0, 400.0)).color(c(233, 235, 238))),
    )
}
