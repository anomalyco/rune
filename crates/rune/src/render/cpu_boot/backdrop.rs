//! `KIND_BACKDROP_BLUR` on the CPU: `render/backdrop_blur.rs`'s dual-filter pyramid over what is
//! painted below the record, pass for pass. Down passes (backdrop_blur.wgsl's `fs_down`, 13 taps)
//! from the window's pixels at half resolution through the levels the blur's width needs, up passes
//! (`fs_up`, a 9-tap tent mixed with the finer level) back to half resolution, each level stored as
//! `Rgba16Float` holds it, then the composite (backdrop_composite.wgsl) inside the record's rounded
//! rect and clip. Only the record's region is blurred, as on the GPU.

use std::sync::atomic::{AtomicU32, Ordering};

use super::raster::{Exec, f16};
use super::shade::{Clip, sd_rounded};
use crate::render::Instance;

const MIPS: usize = 7;
const STEP: u32 = 1 << (MIPS - 1);
/// `backdrop_blur.rs`'s `SIGMAS`.
const SIGMAS: [f32; MIPS] = [1.58, 4.53, 9.62, 19.51, 39.15, 78.36, 156.2];

/// `backdrop_blur.rs`'s `levels`.
fn levels(sigma: f32, top: usize) -> (usize, f32) {
    let top = top.clamp(1, SIGMAS.len() - 1);
    let sigma = sigma.clamp(SIGMAS[0], SIGMAS[top]);
    let n = (1..=top).find(|&n| sigma <= SIGMAS[n]).unwrap_or(top);
    let (a, b) = (SIGMAS[n - 1], SIGMAS[n]);
    (n, ((sigma * sigma - a * a) / (b * b - a * a)).clamp(0.0, 1.0))
}

/// One record's blur: where it reads, and its pyramid's place in the window.
#[derive(Clone, Copy, Debug)]
pub(super) struct Record {
    /// Up passes start at level `n`, mixing `share` of it with the finer level.
    n: usize,
    share: f32,
    /// Window pixels blurred: the rect plus the blur's reach, inside the viewport.
    pub region: [f32; 4],
    origin: [f32; 2],
    /// Level 0's size in texels (half resolution).
    size: [u32; 2],
}

/// The record's blur plan (`prepare_backdrop_blur`), or None when its region is empty.
pub(super) fn plan(i: &Instance, viewport: (u32, u32)) -> Option<Record> {
    let sigma = i.params[0];
    let (n, share) = levels(sigma, MIPS - 1);
    let reach = sigma.min(SIGMAS[n]) * 3.0 + 2.0;
    let v = [viewport.0 as f32, viewport.1 as f32];
    let region = [(i.rect[0] - reach).max(0.0), (i.rect[1] - reach).max(0.0), (i.rect[0] + i.rect[2] + reach).min(v[0]), (i.rect[1] + i.rect[3] + reach).min(v[1])];
    if region[2] <= region[0] || region[3] <= region[1] { return None; }
    let align = (2u32 << n) as f32;
    let origin = [(region[0] / align).floor() * align, (region[1] / align).floor() * align];
    // Level 0 covers the region; any size that does places every texel the same, as the texture
    // the GPU grows to fit its largest region only scales the uv both sides share.
    let size = [0, 1].map(|k| ((((region[k + 2] - origin[k]) / 2.0).ceil() as u32).next_multiple_of(STEP)).max(STEP));
    Some(Record { n, share, region, origin, size })
}

/// The composite's pixels: the rect inside its clip, by whole pixels (the GPU's scissor).
pub(super) fn bounds(i: &Instance, viewport: (u32, u32)) -> [i32; 4] {
    let (r, c) = (i.rect, i.clip);
    let x0 = r[0].max(c[0]).floor().clamp(0.0, viewport.0 as f32);
    let y0 = r[1].max(c[1]).floor().clamp(0.0, viewport.1 as f32);
    let x1 = (r[0] + r[2]).min(c[2]).ceil().clamp(0.0, viewport.0 as f32);
    let y1 = (r[1] + r[3]).min(c[3]).ceil().clamp(0.0, viewport.1 as f32);
    if x1 <= x0 || y1 <= y0 { return [0; 4]; }
    [x0 as i32, y0 as i32, x1 as i32, y1 as i32]
}

/// A level: `w × h` texels of premultiplied RGBA, as `Rgba16Float` keeps them.
struct Level {
    w: u32,
    h: u32,
    px: Vec<[f32; 4]>,
}

impl Level {
    fn new(w: u32, h: u32) -> Level { Level { w, h, px: vec![[0.0; 4]; (w * h) as usize] } }

    /// A linear-filtered sample at `uv`, clamped to the edge, as the pyramid's sampler takes it.
    fn sample(&self, uv: [f32; 2]) -> [f32; 4] {
        bilinear(uv, self.w, self.h, |x, y| self.px[(y * self.w + x) as usize])
    }
}

/// One axis of a linear-filtered sample at `u` (0..=1) over `n` texels, clamped to the edge:
/// the two texels and the weight of the second, as `bilinear` takes them.
#[derive(Clone, Copy)]
struct Axis {
    i0: u32,
    i1: u32,
    f: f32,
}

fn axis(u: f32, n: u32) -> Axis {
    let x = u * n as f32 - 0.5;
    let x0 = x.floor();
    let i = |v: f32| (v as i32).clamp(0, n as i32 - 1) as u32;
    Axis { i0: i(x0), i1: i(x0 + 1.0), f: x - x0 }
}

/// `bilinear` from its two axes.
#[inline]
fn tap(x: &Axis, y: &Axis, texel: impl Fn(u32, u32) -> [f32; 4]) -> [f32; 4] {
    let mix = |a: [f32; 4], b: [f32; 4], t: f32| -> [f32; 4] { std::array::from_fn(|c| a[c] * (1.0 - t) + b[c] * t) };
    mix(mix(texel(x.i0, y.i0), texel(x.i1, y.i0), x.f), mix(texel(x.i0, y.i1), texel(x.i1, y.i1), x.f), y.f)
}

fn bilinear(uv: [f32; 2], w: u32, h: u32, texel: impl Fn(u32, u32) -> [f32; 4]) -> [f32; 4] {
    let (x, y) = (uv[0] * w as f32 - 0.5, uv[1] * h as f32 - 0.5);
    let (fx0, fy0) = (x.floor(), y.floor());
    let (fx, fy) = (x - fx0, y - fy0);
    let ix = |v: f32| (v as i32).clamp(0, w as i32 - 1) as u32;
    let iy = |v: f32| (v as i32).clamp(0, h as i32 - 1) as u32;
    let (x0, x1, y0, y1) = (ix(fx0), ix(fx0 + 1.0), iy(fy0), iy(fy0 + 1.0));
    let mix = |a: [f32; 4], b: [f32; 4], t: f32| -> [f32; 4] { std::array::from_fn(|c| a[c] * (1.0 - t) + b[c] * t) };
    mix(mix(texel(x0, y0), texel(x1, y0), fx), mix(texel(x0, y1), texel(x1, y1), fx), fy)
}

/// Runs `row(y, texels)` for rows `y0..y1` of `out`, a row at a time on `exec`'s threads.
fn rows(exec: &dyn Exec, out: &mut Level, y0: u32, y1: u32, row: &(dyn Fn(u32, &mut [[f32; 4]]) + Sync)) {
    struct Rows(*mut [f32; 4]);
    // SAFETY: each row goes to one thread (the counter hands each out once), and rows don't
    // overlap; `out` outlives the run, which joins before returning.
    unsafe impl Sync for Rows {}
    let (base, w) = (&Rows(out.px.as_mut_ptr()), out.w as usize);
    let next = AtomicU32::new(y0);
    exec.run(&|| loop {
        let y = next.fetch_add(1, Ordering::Relaxed);
        if y >= y1 { break; }
        // SAFETY: row `y` is inside `out` (y < y1 <= h) and only this thread has it.
        let texels = unsafe { std::slice::from_raw_parts_mut(base.0.add(y as usize * w), w) };
        row(y, texels);
    });
}

/// A record's blurred result, level 0 of the up chain, and where it sits.
pub(super) struct Blurred {
    up: Level,
    place: [f32; 4],
}

/// Blurs the window's pixels under `rec`: `window(x, y)` reads pixel `(x, y)` as the target
/// holds it, R, G, B, A in 0..=1.
pub(super) fn blur(rec: &Record, viewport: (u32, u32), half: super::HalfRounding, exec: &dyn Exec, window: impl Fn(u32, u32) -> [f32; 4] + Sync) -> Blurred {
    let [pw, ph] = rec.size;
    let extent = [pw as f32 * 2.0, ph as f32 * 2.0];
    let v = [viewport.0 as f32, viewport.1 as f32];
    let inside = [(rec.region[0] - rec.origin[0]) / extent[0], (rec.region[1] - rec.origin[1]) / extent[1],
        (rec.region[2] - rec.origin[0]) / extent[0], (rec.region[3] - rec.origin[1]) / extent[1]];
    let dims = |m: usize| ((pw >> m).max(1), (ph >> m).max(1));
    // Each level covers the region plus two texels, the reach of its taps into the next.
    let scissor = |m: usize| {
        let k = (2u32 << m) as f32;
        let (w, h) = dims(m);
        let x = (((rec.region[0] - rec.origin[0]) / k).floor() as u32).saturating_sub(2).min(w - 1);
        let y = (((rec.region[1] - rec.origin[1]) / k).floor() as u32).saturating_sub(2).min(h - 1);
        let r = (((rec.region[2] - rec.origin[0]) / k).ceil() as u32 + 2).min(w);
        let b = (((rec.region[3] - rec.origin[1]) / k).ceil() as u32 + 2).min(h);
        (x, y, r.max(x + 1), b.max(y + 1))
    };
    let window_region = [rec.region[0] / v[0], rec.region[1] / v[1], rec.region[2] / v[0], rec.region[3] / v[1]];
    let window_map = [extent[0] / v[0], extent[1] / v[1], rec.origin[0] / v[0], rec.origin[1] / v[1]];
    // The region's pixels as floats, once: the first pass reads each a few dozen times. A
    // bilinear read at the region's edge reaches one pixel past it.
    let (cx0, cy0) = ((rec.region[0].floor() as u32).saturating_sub(1), (rec.region[1].floor() as u32).saturating_sub(1));
    let (cx1, cy1) = (((rec.region[2].ceil() as u32) + 1).min(viewport.0), ((rec.region[3].ceil() as u32) + 1).min(viewport.1));
    let cw = cx1 - cx0;
    let cache: Vec<[f32; 4]> = (cy0..cy1).flat_map(|y| (cx0..cx1).map(move |x| (x, y))).map(|(x, y)| window(x, y)).collect();
    let window = |x: u32, y: u32| if x >= cx0 && x < cx1 && y >= cy0 && y < cy1 { cache[((y - cy0) * cw + (x - cx0)) as usize] } else { window(x, y) };
    // Every tap's place along x depends on the texel's x alone, and along y on its y: each
    // pass takes them once a column and once a row (the same expressions as a tap would), then
    // filters with them.
    let mut down: Vec<Level> = Vec::with_capacity(rec.n + 1);
    for m in 0..=rec.n {
        let (w, h) = dims(m);
        let mut out = Level::new(w, h);
        let (sx0, sy0, sx1, sy1) = scissor(m);
        let offsets = [-2.0f32, -1.0, 0.0, 1.0, 2.0];
        if m == 0 {
            let t = [1.0 / v[0], 1.0 / v[1]];
            let xs: Vec<[Axis; 5]> = (sx0..sx1).map(|tx| {
                let u = (tx as f32 + 0.5) / w as f32;
                offsets.map(|o| axis((u * window_map[0] + window_map[2] + t[0] * o).clamp(window_region[0], window_region[2]), viewport.0))
            }).collect();
            let ys: Vec<[Axis; 5]> = (sy0..sy1).map(|ty| {
                let u = (ty as f32 + 0.5) / h as f32;
                offsets.map(|o| axis((u * window_map[1] + window_map[3] + t[1] * o).clamp(window_region[1], window_region[3]), viewport.1))
            }).collect();
            rows(exec, &mut out, sy0, sy1, &|ty, row| {
                let ay = &ys[(ty - sy0) as usize];
                for (tx, ax) in (sx0..sx1).zip(&xs) {
                    row[tx as usize] = down13(|o| tap(&ax[(o[0] + 2.0) as usize], &ay[(o[1] + 2.0) as usize], &window)).map(|v| f16(v, half));
                }
            });
        } else {
            let src = &down[m - 1];
            let t = [1.0 / src.w as f32, 1.0 / src.h as f32];
            let xs: Vec<[Axis; 5]> = (sx0..sx1).map(|tx| {
                let u = (tx as f32 + 0.5) / w as f32;
                offsets.map(|o| axis((u + t[0] * o).clamp(inside[0], inside[2]), src.w))
            }).collect();
            let ys: Vec<[Axis; 5]> = (sy0..sy1).map(|ty| {
                let u = (ty as f32 + 0.5) / h as f32;
                offsets.map(|o| axis((u + t[1] * o).clamp(inside[1], inside[3]), src.h))
            }).collect();
            let texel = |x: u32, y: u32| src.px[(y * src.w + x) as usize];
            rows(exec, &mut out, sy0, sy1, &|ty, row| {
                let ay = &ys[(ty - sy0) as usize];
                for (tx, ax) in (sx0..sx1).zip(&xs) {
                    row[tx as usize] = down13(|o| tap(&ax[(o[0] + 2.0) as usize], &ay[(o[1] + 2.0) as usize], &texel)).map(|v| f16(v, half));
                }
            });
        }
        down.push(out);
    }
    let mut coarse: Option<Level> = None;
    for m in (0..rec.n).rev() {
        let (w, h) = dims(m);
        let mut out = Level::new(w, h);
        let src = coarse.as_ref().unwrap_or(&down[rec.n]);
        let fine = &down[m];
        let weight = if m + 1 == rec.n { rec.share } else { 1.0 };
        let t = [1.0 / src.w as f32, 1.0 / src.h as f32];
        let (sx0, sy0, sx1, sy1) = scissor(m);
        // Per column: the coarse level's three taps (−t, 0, +t) and the fine level's one.
        let xs: Vec<([Axis; 3], Axis)> = (sx0..sx1).map(|tx| {
            let u = (tx as f32 + 0.5) / w as f32;
            ([-t[0], 0.0, t[0]].map(|o| axis((u + o).clamp(inside[0], inside[2]), src.w)), axis(u.clamp(inside[0], inside[2]), fine.w))
        }).collect();
        let ys: Vec<([Axis; 3], Axis)> = (sy0..sy1).map(|ty| {
            let u = (ty as f32 + 0.5) / h as f32;
            ([-t[1], 0.0, t[1]].map(|o| axis((u + o).clamp(inside[1], inside[3]), src.h)), axis(u.clamp(inside[1], inside[3]), fine.h))
        }).collect();
        let coarse_texel = |x: u32, y: u32| src.px[(y * src.w + x) as usize];
        let fine_texel = |x: u32, y: u32| fine.px[(y * fine.w + x) as usize];
        rows(exec, &mut out, sy0, sy1, &|ty, row| {
            let ay = &ys[(ty - sy0) as usize];
            for (tx, ax) in (sx0..sx1).zip(&xs) {
                // `s(dx, dy)`: the coarse level at offset (dx, dy) texels, each −1, 0 or 1.
                let s = |dx: usize, dy: usize| tap(&ax.0[dx], &ay.0[dy], &coarse_texel);
                let add = |a: [f32; 4], b: [f32; 4]| -> [f32; 4] { std::array::from_fn(|c| a[c] + b[c]) };
                let scale = |a: [f32; 4], k: f32| a.map(|v| v * k);
                // `fs_up`'s sums, in its order.
                let mut c = scale(s(1, 1), 4.0);
                c = add(c, scale(add(add(add(s(2, 1), s(0, 1)), s(1, 2)), s(1, 0)), 2.0));
                c = add(c, add(add(add(s(2, 2), s(0, 0)), s(2, 0)), s(0, 2)));
                let c = c.map(|v| v / 16.0);
                let f = tap(&ax.1, &ay.1, &fine_texel);
                row[tx as usize] = std::array::from_fn(|ch| f16(f[ch] * (1.0 - weight) + c[ch] * weight, half));
            }
        });
        coarse = Some(out);
    }    Blurred { up: coarse.unwrap_or_else(|| down.swap_remove(0)), place: [rec.origin[0], rec.origin[1], extent[0], extent[1]] }
}

/// `fs_down`'s 13 taps, `s` sampling at a texel offset, summed in its order.
fn down13(s: impl Fn([f32; 2]) -> [f32; 4]) -> [f32; 4] {
    let (a, b, c) = (s([-2.0, -2.0]), s([0.0, -2.0]), s([2.0, -2.0]));
    let (d, e) = (s([-1.0, -1.0]), s([1.0, -1.0]));
    let (f, g, h) = (s([-2.0, 0.0]), s([0.0, 0.0]), s([2.0, 0.0]));
    let (i, j) = (s([-1.0, 1.0]), s([1.0, 1.0]));
    let (k, l, m) = (s([-2.0, 2.0]), s([0.0, 2.0]), s([2.0, 2.0]));
    std::array::from_fn(|ch| {
        (d[ch] + e[ch] + i[ch] + j[ch]) * 0.125 + (a[ch] + b[ch] + g[ch] + f[ch]) * 0.03125 + (b[ch] + c[ch] + h[ch] + g[ch]) * 0.03125
            + (f[ch] + g[ch] + l[ch] + k[ch]) * 0.03125 + (g[ch] + h[ch] + m[ch] + l[ch]) * 0.03125
    })
}

/// The composite at pixel centre `p`: the blur inside the rounded rect, cut by the clip, at the
/// record's opacity.
pub(super) fn composite(b: &Blurred, i: &Instance, clip: &Clip, p: [f32; 2]) -> [f32; 4] {
    let half = [i.rect[2] * 0.5, i.rect[3] * 0.5];
    let d = sd_rounded([p[0] - i.rect[0] - half[0], p[1] - i.rect[1] - half[1]], half, i.radii);
    let cov = (0.5 - d).clamp(0.0, 1.0) * clip.alpha(p) * i.fill[3];
    let col = b.up.sample([(p[0] - b.place[0]) / b.place[2], (p[1] - b.place[1]) / b.place[3]]);
    col.map(|v| v * cov)
}
