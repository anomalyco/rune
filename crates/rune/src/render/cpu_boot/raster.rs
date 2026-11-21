//! Painting tiles: each thread takes the next tile, and draws its instances in painter order
//! straight into the 8-bit target, blending each as the GPU's
//! premultiplied source-over does on a `Bgra8Unorm` target: the destination read as stored, the
//! result rounded back to 8 bits after every instance. A layer drawn to its own target draws into
//! a scratch tile holding what an `Rgba16Float` texel holds, composited at its end.

mod glyph;
mod segments;

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use glyph::Pow;
use segments::{Occluder, Segment, centres};

use super::bin::{Bins, Show, Solid, TILE_H, TILE_W, interior, intersect};
use super::backdrop;
use super::blend::{self, Src, q};
use super::shade::{self, Clip};
use super::{HalfRounding, Report, Scene, Target};
use crate::render::instance::shader_consts;
use crate::render::{Instance, Kind};

const TILE_PX: usize = (TILE_W * TILE_H) as usize;

/// Paints 	iles on xec's threads; backdrop blurs, taken between phases, go on wide's.
pub(super) fn run(scene: &Scene<'_>, bins: &Bins, target: &mut Target<'_>, tiles: &[u32], exec: &dyn Exec, wide: &dyn Exec) -> Report {
    let surface = Surface { base: target.pixels.as_mut_ptr(), len: target.pixels.len(), stride: target.stride };
    let clear = scene.clear.map(|c| c.clamp(0.0, 1.0) * 255.0);
    let clear = [q(clear[2]), q(clear[1]), q(clear[0]), q(clear[3])];
    let mut report = Report { threads: exec.threads(), ..Report::default() };
    if tiles.is_empty() { return report; }
    // Each record's blur, or None when no tile painted this frame shows it.
    let mut blurred: Vec<Option<backdrop::Blurred>> = Vec::with_capacity(bins.blurs.len());
    // A backdrop blur splits the frame: what's under it is painted everywhere first.
    let cuts: Vec<u32> = bins.blurs.iter().map(|(n, _)| *n).chain([u32::MAX]).collect();
    let mut from = 0;
    for (phase, &until) in cuts.iter().enumerate() {
        let next = AtomicUsize::new(0);
        let done = &blurred;
        let sums = Mutex::new(Report::default());
        let worker = || {
            let mut stats = Report::default();
            let mut pow = Pow::default();
            let mut layers: Vec<Scratch> = Vec::new();
            let mut occ: Vec<Occluder> = Vec::new();
            let mut painter = Painter { scene, bins, surface, stats: &mut stats, pow: &mut pow, layers: &mut layers, depth: 0, tile: [0; 4],
                occ: &mut occ, n: 0, breaks: Vec::new(), segs: Vec::new(), blurred: done };
            // A big frame goes out a page-wide run of tiles at a time (16 tiles: 1,024 pixels of
            // each row, about a page), so threads fault in pages of their own rather than all
            // queuing on the same ones (a fresh buffer: 12,969 to 9,992 µs); a small one tile at
            // a time, so a thread that drew cheap tiles takes more.
            let chunk = if tiles.len() > 512 { 16 } else { 1 };
            loop {
                let at = next.fetch_add(chunk, Ordering::Relaxed);
                if at >= tiles.len() { break; }
                for &t in &tiles[at..(at + chunk).min(tiles.len())] {
                let t = t as usize;
                let began = crate::time::now();
                painter.tile = painter.tile_rect(t as u32 % bins.cols, t as u32 / bins.cols);
                if phase == 0 && !bins.covered[t] {
                    painter.surface.fill(painter.tile, clear);
                    painter.stats.cleared += 1;
                }
                let list = bins.tile(t);
                let (a, b) = (list.partition_point(|&n| n < from), list.partition_point(|&n| n < until));
                painter.occluders(&list[a..b]);
                for &n in &list[a..b] {
                    let b = bins.bounds[n as usize];
                    let tile = painter.tile;
                    let r = [(b[0] as u32).max(tile[0]), (b[1] as u32).max(tile[1]), (b[2] as u32).min(tile[2]), (b[3] as u32).min(tile[3])];
                    if r[2] > r[0] && r[3] > r[1] { painter.draw(n, r); }
                }
                painter.depth = 0;
                painter.stats.slowest_tile_ns = painter.stats.slowest_tile_ns.max(super::ns(began.elapsed()));
                }
            }
            drop(painter);
            let mut s = sums.lock().unwrap_or_else(|e| e.into_inner());
            s.span_px += stats.span_px;
            s.shaded_px += stats.shaded_px;
            s.glyph_px += stats.glyph_px;
            s.cleared += stats.cleared;
            s.slowest_tile_ns = s.slowest_tile_ns.max(stats.slowest_tile_ns);
        };
        exec.run(&worker);
        let s = sums.into_inner().unwrap_or_else(|e| e.into_inner());
        report.span_px += s.span_px;
        report.shaded_px += s.shaded_px;
        report.glyph_px += s.glyph_px;
        report.cleared += s.cleared;
        report.slowest_tile_ns = report.slowest_tile_ns.max(s.slowest_tile_ns);
        if let Some((n, rec)) = bins.blurs.get(phase) {
            let px: &[u8] = &*target.pixels;
            let stride = target.stride;
            let b = bins.bounds[*n as usize];
            let shown = tiles.iter().any(|&t| {
                let (tx, ty) = (t % bins.cols, t / bins.cols);
                ((tx * TILE_W) as i32) < b[2] && (((tx + 1) * TILE_W) as i32) > b[0] && ((ty * TILE_H) as i32) < b[3] && (((ty + 1) * TILE_H) as i32) > b[1]
            });
            let began = crate::time::now();
            blurred.push(shown.then(|| backdrop::blur(rec, scene.size, scene.half, wide, |x, y| {
                let at = y as usize * stride + x as usize * 4;
                [px[at + 2] as f32 / 255.0, px[at + 1] as f32 / 255.0, px[at] as f32 / 255.0, px[at + 3] as f32 / 255.0]
            })));
            report.blur_ns += super::ns(began.elapsed());
        }
        from = until;
    }
    report
}
/// Runs a painting job on some threads at once, the caller among them, and returns when all are
/// done.
pub(super) trait Exec {
    fn threads(&self) -> usize;
    fn run(&self, job: &(dyn Fn() + Sync));
}

/// Threads made for the one job (`paint`), joined at its end.
pub(super) struct Spawn(pub usize);

impl Exec for Spawn {
    fn threads(&self) -> usize { self.0.max(1) }

    fn run(&self, job: &(dyn Fn() + Sync)) {
        std::thread::scope(|s| {
            for _ in 1..self.0 { s.spawn(job); }
            job();
        });
    }
}

/// What an `Rgba16Float` texel keeps of `v` on the GPU that takes over (`HalfRounding`). Colours
/// here stay far below half's largest value.
#[inline]
pub(super) fn f16(v: f32, r: HalfRounding) -> f32 {
    let b = v.to_bits();
    if v.abs() >= 6.103_515_6e-5 {
        // 10 of f32's 23 mantissa bits stay; to nearest, add half of what goes (and one more
        // when what stays is odd) first.
        let up = match r { HalfRounding::TowardZero => 0, HalfRounding::NearestEven => 0x0fff + ((b >> 13) & 1) };
        return f32::from_bits((b + up) & 0xffff_e000);
    }
    // Subnormal halves are whole multiples of 2^-24.
    const M: f32 = 16_777_216.0;
    match r {
        HalfRounding::TowardZero => (v * M).trunc() / M,
        HalfRounding::NearestEven => ((v * M + 12_582_912.0) - 12_582_912.0) / M,
    }
}

/// `f16` on four values at once: F16C's conversion where the CPU has it.
#[inline]
#[allow(unsafe_code)]
pub(super) fn f16x4(v: [f32; 4], r: HalfRounding) -> [f32; 4] {
    #[cfg(target_arch = "x86_64")]
    if std::arch::is_x86_feature_detected!("f16c") {
        // SAFETY: the CPU has F16C (checked).
        return unsafe { f16x4_f16c(v, r) };
    }
    v.map(|x| f16(x, r))
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "f16c")]
#[allow(unsafe_code)]
unsafe fn f16x4_f16c(v: [f32; 4], r: HalfRounding) -> [f32; 4] {
    use std::arch::x86_64::*;
    let mut out = [0.0f32; 4];
    // SAFETY: both arrays hold four floats.
    unsafe {
        let x = _mm_loadu_ps(v.as_ptr());
        let h = match r {
            HalfRounding::TowardZero => _mm_cvtps_ph::<_MM_FROUND_TO_ZERO>(x),
            HalfRounding::NearestEven => _mm_cvtps_ph::<_MM_FROUND_TO_NEAREST_INT>(x),
        };
        _mm_storeu_ps(out.as_mut_ptr(), _mm_cvtph_ps(h));
    }
    out
}
/// The target's pixels, shared by the threads: each writes only the tile it holds, and tiles
/// don't overlap.
#[derive(Clone, Copy)]
struct Surface {
    base: *mut u8,
    len: usize,
    stride: usize,
}

// SAFETY: `run` hands each tile to one thread; a thread writes only pixels of its tile, and the
// target outlives the scope its threads run in.
#[allow(unsafe_code)]
unsafe impl Send for Surface {}
// SAFETY: as above; `run` partitions the target into disjoint tiles across threads and joins
// before the target borrow ends.
#[allow(unsafe_code)]
unsafe impl Sync for Surface {}

impl Surface {
    #[inline]
    fn at(&self, x: u32, y: u32) -> usize { y as usize * self.stride + x as usize * 4 }

    /// `n` pixels of row `y` from `x`, all inside the target and the caller's tile.
    #[inline]
    #[allow(unsafe_code)]
    fn row(&mut self, x: u32, y: u32, n: u32) -> &mut [u8] {
        let at = self.at(x, y);
        let len = n as usize * 4;
        assert!(at + len <= self.len);
        // SAFETY: in bounds (checked), and only this tile's thread touches these bytes.
        unsafe { std::slice::from_raw_parts_mut(self.base.add(at), len) }
    }
    /// Writes `px` over `r`.
    fn fill(&mut self, r: [u32; 4], px: [u8; 4]) {
        for y in r[1]..r[3] { blend::fill_row(self.row(r[0], y, r[2] - r[0]), px); }
    }

    /// Blends premultiplied RGBA `c` (0..=1) over pixels `x0..x1` of row `y`.
    #[inline]
    fn span(&mut self, x0: u32, x1: u32, y: u32, c: [f32; 4]) {
        blend::over_row(self.row(x0, y, x1 - x0), &Src::new(c));
    }

    #[inline]
    fn blend(&mut self, x: u32, y: u32, c: [f32; 4]) {
        blend::over_rgba(self.row(x, y, 1), c);
    }

    #[inline]
    fn blend_lcd(&mut self, x: u32, y: u32, c: [f32; 4], k: [f32; 3]) {
        blend::over_lcd(self.row(x, y, 1), c, k);
    }
}
/// A layer's target over one tile: premultiplied RGBA as `Rgba16Float` holds it, cleared to
/// zero, and how it blends what's drawn into it.
struct Scratch {
    px: Vec<[f32; 4]>,
    additive: bool,
    /// The plan it draws (`Bins::layers`).
    plan: usize,
}

struct Painter<'p, 's> {
    scene: &'p Scene<'s>,
    bins: &'p Bins,
    surface: Surface,
    stats: &'p mut Report,
    pow: &'p mut Pow,
    /// The thread's scratch tiles, reused; the first `depth` are open layers, innermost last.
    layers: &'p mut Vec<Scratch>,
    depth: usize,
    /// The tile being drawn, `[x0, y0, x1, y1)`.
    tile: [u32; 4],
    /// The opaque boxes in this tile that draw straight into the window: a pixel one of them
    /// paints whole needs nothing drawn under it.
    occ: &'p mut Vec<Occluder>,
    /// The instance being drawn.
    n: u32,
    /// Scratch for `segments`, kept between instances.
    breaks: Vec<u32>,
    segs: Vec<Segment>,
    /// The blurs of the frame's backdrop blur records painted so far, in order.
    blurred: &'p [Option<backdrop::Blurred>],
}

impl Painter<'_, '_> {
    /// Tile `tx`'s pixels `[x0, y0, x1, y1)` in the target.
    fn tile_rect(&self, tx: u32, ty: u32) -> [u32; 4] {
        let (w, h) = self.scene.size;
        [tx * TILE_W, ty * TILE_H, ((tx + 1) * TILE_W).min(w), ((ty + 1) * TILE_H).min(h)]
    }

    #[inline]
    fn slot(&self, x: u32, y: u32) -> usize { ((y - self.tile[1]) * TILE_W + (x - self.tile[0])) as usize }

    /// Blends premultiplied `c` into pixel `(x, y)` of whatever is being drawn into.
    #[inline]
    fn put(&mut self, x: u32, y: u32, c: [f32; 4]) {
        if self.depth == 0 {
            // Under half a step in every channel it rounds to nothing and leaves the pixel as is:
            // a shadow's tail, a glyph's faintest edge.
            if c[0].max(c[1]).max(c[2]).max(c[3]) * 255.0 <= 0.5 { return; }
            return self.surface.blend(x, y, c);
        }
        let s = self.slot(x, y);
        let top = &mut self.layers[self.depth - 1];
        let d = &mut top.px[s];
        let half = self.scene.half;
        let c = f16x4(c, half);
        let k = if top.additive { 1.0 } else { 1.0 - c[3] };
        *d = f16x4(std::array::from_fn(|ch| c[ch] + d[ch] * k), half);
    }

    fn put_span(&mut self, x0: u32, x1: u32, y: u32, c: [f32; 4]) {
        if x1 <= x0 { return; }
        self.stats.span_px += u64::from(x1 - x0);
        if self.depth == 0 { return self.surface.span(x0, x1, y, c); }
        let half = self.scene.half;
        let c16 = f16x4(c, half);
        let top = &mut self.layers[self.depth - 1];
        let (row, x0t) = (((y - self.tile[1]) * TILE_W) as usize, self.tile[0]);
        let px = &mut top.px[row + (x0 - x0t) as usize..row + (x1 - x0t) as usize];
        // Opaque over anything is itself: `c + d × 0`.
        if c16[3] == 1.0 && !top.additive { return px.fill(c16); }
        let k = if top.additive { 1.0 } else { 1.0 - c16[3] };
        for d in px { *d = f16x4(std::array::from_fn(|ch| c16[ch] + d[ch] * k), half); }
    }

    /// Draws instance `n` over pixels `r` (inside its bounds and this tile).
    fn draw(&mut self, n: u32, r: [u32; 4]) {
        let i = &self.scene.instances[n as usize];
        self.n = n;
        match i.kind() {
            Some(Kind::Layer) => self.open(n),
            Some(Kind::LayerEnd) => self.close(i, r),
            Some(Kind::Glyph) => { let clip = Clip::of(i); self.glyph(i, r, &clip); }
            Some(Kind::Image) => { let clip = Clip::of(i); self.image(i, r, &clip); }
            Some(kind @ (Kind::Rect | Kind::Gradient)) => { let clip = Clip::of(i); self.shaded(i, kind, r, &clip, interior(i)); }
            Some(Kind::Shadow) => { let clip = Clip::of(i); self.shadow(i, r, &clip); }
            Some(Kind::Radial) => {
                let clip = Clip::of(i);
                let solid = Solid {
                    edges: [i.rect[0], i.rect[1], i.rect[0] + i.rect[2], i.rect[1] + i.rect[3]],
                    radii: i.radii,
                    inset: i.params[2].max(shader_consts::SOFT_MIN),
                };
                self.shaded(i, Kind::Radial, r, &clip, Some(solid));
            }
            Some(kind @ (Kind::Inset | Kind::Segment | Kind::Triangle | Kind::Dashed)) => {
                let clip = Clip::of(i);
                self.shaded(i, kind, r, &clip, None);
            }
            Some(Kind::BackdropBlur) => { let clip = Clip::of(i); self.backdrop(i, r, &clip); }
            Some(Kind::Effect) | None => {}
        }
    }

    /// A layer drawn to its own target begins: a cleared scratch tile.
    fn open(&mut self, n: u32) {
        let Some(&plan) = self.bins.scope.get(&n) else { return };
        let l = &self.bins.layers[plan as usize];
        if l.show != Show::Render { return; }
        if self.layers.len() == self.depth { self.layers.push(Scratch { px: vec![[0.0; 4]; TILE_PX], additive: false, plan: 0 }); }
        let s = &mut self.layers[self.depth];
        s.px.fill([0.0; 4]);
        s.additive = l.additive;
        s.plan = plan as usize;
        self.depth += 1;
    }

    /// The innermost layer ends: its tile composited over pixels `r` of its parent, by its
    /// opacity, clip and hole (shader.wgsl's `KIND_LAYER` branch).
    fn close(&mut self, end: &Instance, r: [u32; 4]) {
        let _ = end;
        if self.depth == 0 { return; }
        self.depth -= 1;
        let plan = self.layers[self.depth].plan;
        let l = self.bins.layers[plan];
        let begin = &self.scene.instances[l.begin as usize];
        let clip = Clip::of(begin);
        let opacity = begin.fill[3];
        let segs = self.segments(r, None, &clip);
        let mut k = 0;
        for y in r[1]..r[3] {
            let py = y as f32 + 0.5;
            while segs[k].y1 <= y { k += 1; }
            let (whole, vis) = (segs[k].whole, segs[k].vis);
            // Where the clip is whole and there's no hole, the composite is each texel by the
            // opacity: a row at a time, into the window or the parent layer.
            let (fa, fb) = if !l.additive && l.hole[3] <= 0.0 { centres(whole, r[0], r[2]) } else { (r[2], r[2]) };
            for (va, vb) in vis.iter() {
                let (a, b) = (fa.max(va), fb.min(vb));
                if b <= a { continue; }
                let at = self.slot(a, y);
                let n = (b - a) as usize;
                if self.depth == 0 {
                    let src = &self.layers[0].px[at..at + n];
                    if opacity == 1.0 { blend::composite_row(self.surface.row(a, y, b - a), src) } else { blend::composite_row_by(self.surface.row(a, y, b - a), src, opacity) }
                    continue;
                }
                let (outer, inner) = self.layers.split_at_mut(self.depth);
                let parent = &mut outer[self.depth - 1];
                for (d, s) in parent.px[at..at + n].iter_mut().zip(&inner[0].px[at..at + n]) {
                    if *s == [0.0; 4] { continue; }
                    let c = if opacity == 1.0 { *s } else { s.map(|v| v * opacity) };
                    // Opaque over anything is itself (a texel is already in half precision).
                    if c[3] == 1.0 && !parent.additive && opacity == 1.0 { *d = c; continue; }
                    if c == [0.0; 4] { continue; }
                    let c = f16x4(c, self.scene.half);
                    let k = if parent.additive { 1.0 } else { 1.0 - c[3] };
                    *d = f16x4(std::array::from_fn(|ch| c[ch] + d[ch] * k), self.scene.half);
                }
            }
            for x in vis.iter().flat_map(|(a, b)| a..b).filter(|&x| x < fa || x >= fb) {
                let s = self.layers[self.depth].px[self.slot(x, y)];
                if s == [0.0; 4] { continue; }
                let px = x as f32 + 0.5;
                let s = if l.additive { s.map(|v| v.clamp(0.0, 1.0)) } else { s };
                let a = if px >= whole.0 && px <= whole.1 { 1.0 } else { clip.alpha([px, py]) };
                let h = hole_alpha([px, py], l.hole);
                // Whole and opaque over the window: the texel itself, in 8-bit steps.
                if self.depth == 0 && s[3] == 1.0 && opacity == 1.0 && a == 1.0 && h == 1.0 {
                    self.surface.row(x, y, 1).copy_from_slice(&blend::opaque(s));
                    continue;
                }
                let c = s.map(|v| v * opacity * a * h);
                if c != [0.0; 4] { self.put(x, y, c); }
            }
        }
        self.segs = segs;
    }

    /// A box kind pixel by pixel through `shade::boxes`, with `solid`'s interior in one colour per
    /// row where the clip is whole.
    fn shaded(&mut self, i: &Instance, kind: Kind, r: [u32; 4], clip: &Clip, solid: Option<Solid>) {
        let gradient_row = kind == Kind::Gradient && i.params[0] <= 0.0;
        let segs = self.segments(r, solid, clip);
        let mut k = 0;
        for y in r[1]..r[3] {
            let py = y as f32 + 0.5;
            while segs[k].y1 <= y { k += 1; }
            let (whole, (ia, ib)) = (segs[k].whole, segs[k].solid);
            let c = if gradient_row {
                let t = ((py - i.rect[1]) / i.rect[3].max(1.0)).clamp(0.0, 1.0);
                std::array::from_fn(|ch| i.fill[ch] * (1.0 - t) + i.uv[ch] * t)
            } else { i.fill };
            if kind == Kind::Radial && shade::linear_row_outside(i, py, r[0] as f32 + 0.5, r[2] as f32 - 0.5) { continue; }
            for (va, vb) in segs[k].vis.iter() {
                for x in va..ia.clamp(va, vb) { self.pixel(i, kind, clip, whole, x, y); }
                if kind == Kind::Radial {
                    // A gradient's colour moves along the span; only its coverage is whole there.
                    for x in ia.max(va)..ib.min(vb) {
                        let c = shade::radial(i, [x as f32 + 0.5, py], true);
                        if c != [0.0; 4] { self.put(x, y, c); }
                    }
                } else {
                    self.put_span(ia.max(va), ib.min(vb), y, c);
                }
                for x in ib.clamp(va, vb)..vb { self.pixel(i, kind, clip, whole, x, y); }
            }
        }
        self.segs = segs;
    }

    /// A `KIND_SHADOW`, its y samples taken once a row and each column's sum once for every row
    /// that samples the same (all but the rows near its corners), so a pixel costs a blend.
    fn shadow(&mut self, i: &Instance, r: [u32; 4], clip: &Clip) {
        let mut key = None;
        let mut sums = [f32::NAN; TILE_W as usize];
        let cx = i.rect[0] + i.rect[2] * 0.5;
        let segs = self.segments(r, None, clip);
        let mut k = 0;
        for y in r[1]..r[3] {
            let py = y as f32 + 0.5;
            while segs[k].y1 <= y { k += 1; }
            let (whole, vis) = (segs[k].whole, segs[k].vis);
            let row = shade::shadow_row(i, py);
            if key != Some(row.key()) {
                key = Some(row.key());
                sums = [f32::NAN; TILE_W as usize];
            }
            // Where the row's colour is one value, it's a span; a pixel short of each end, so
            // rounding in `px - cx` can't put an end pixel on the wrong side.
            let (fa, fb, flat) = match row.flat().filter(|_| i.uv[2] <= 0.0) {
                Some((reach, cov)) => {
                    let (a, b) = centres(intersect((cx - reach, cx + reach), whole), r[0], r[2]);
                    (a + 1, b.saturating_sub(1).max(a + 1), i.fill.map(|v| v * cov))
                }
                None => (r[2], r[2], [0.0; 4]),
            };
            for (va, vb) in vis.iter() {
                if fb > fa { self.put_span(fa.max(va), fb.min(vb), y, flat); }
            }
            for x in vis.iter().flat_map(|(a, b)| a..b).filter(|&x| x < fa || x >= fb) {
                let px = x as f32 + 0.5;
                let dx = px - cx;
                let cov = if row.inside(dx) { shade::SHADOW_INSIDE } else {
                    let s = &mut sums[(x - self.tile[0]) as usize];
                    if s.is_nan() { *s = row.sum(dx); }
                    *s
                };
                let c = shade::shadow_cov(i, &row, px, cov);
                if c == [0.0; 4] { continue; }
                let a = if px >= whole.0 && px <= whole.1 { 1.0 } else { clip.alpha([px, py]) };
                self.stats.shaded_px += 1;
                if a > 0.0 { self.put(x, y, c.map(|v| v * a)); }
            }
        }
        self.segs = segs;
    }

    #[inline]
    fn pixel(&mut self, i: &Instance, kind: Kind, clip: &Clip, whole: (f32, f32), x: u32, y: u32) {
        let p = [x as f32 + 0.5, y as f32 + 0.5];
        let c = shade::boxes(i, kind, p);
        if c == [0.0; 4] { return; }
        let a = if p[0] >= whole.0 && p[0] <= whole.1 { 1.0 } else { clip.alpha(p) };
        self.stats.shaded_px += 1;
        if a > 0.0 { self.put(x, y, c.map(|v| v * a)); }
    }

    /// A `KIND_BACKDROP_BLUR`'s composite, from the blur painted before it.
    fn backdrop(&mut self, i: &Instance, r: [u32; 4], clip: &Clip) {
        let Some(at) = self.bins.blurs.iter().position(|(n, _)| *n == self.n) else { return };
        let Some(Some(b)) = self.blurred.get(at) else { return };
        for y in r[1]..r[3] {
            for x in r[0]..r[2] {
                let c = backdrop::composite(b, i, clip, [x as f32 + 0.5, y as f32 + 0.5]);
                if c != [0.0; 4] { self.put(x, y, c); }
            }
        }
    }

    /// A `KIND_IMAGE` from its image page.
    fn image(&mut self, i: &Instance, r: [u32; 4], clip: &Clip) {
        let Some(&page) = self.scene.images.get(i.params[0] as usize) else { return };
        let segs = self.segments(r, None, clip);
        let mut k = 0;
        for y in r[1]..r[3] {
            let py = y as f32 + 0.5;
            while segs[k].y1 <= y { k += 1; }
            let (whole, vis) = (segs[k].whole, segs[k].vis);
            for x in vis.iter().flat_map(|(a, b)| a..b) {
                let p = [x as f32 + 0.5, py];
                let c = shade::image(i, &page, p);
                if c == [0.0; 4] { continue; }
                let a = if p[0] >= whole.0 && p[0] <= whole.1 { 1.0 } else { clip.alpha(p) };
                self.stats.shaded_px += 1;
                self.put(x, y, c.map(|v| v * a));
            }
        }
        self.segs = segs;
    }
}

/// `hole_alpha`: a layer composite's round hole (`El::cutout_circle`).
fn hole_alpha(p: [f32; 2], h: [f32; 4]) -> f32 {
    if h[3] <= 0.0 { return 1.0; }
    let (dx, dy) = (p[0] - h[0], p[1] - h[1]);
    (((dx * dx + dy * dy).sqrt() - (h[2] + h[3]) * 0.5) / (h[3] - h[2]).max(1.0) + 0.5).clamp(0.0, 1.0)
}
