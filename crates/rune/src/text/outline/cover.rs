//! outline.wgsl's coverage on the CPU, line for line in f32: masks placed before any renderer
//! can fill them (the first frame, a runtime drawn without one) come out as the GPU fills them.
//! The integrals are written once over [`Lanes`], so a mask fill can take four at a time and
//! give the bits the one-lane form gives.

use super::lanes::Lanes;
use super::store::OutlineStore;

/// `outline_coverage`: how much of a pixel `2 * hw` rest px square at `local` (rest px from the
/// pen, y up) the glyph of record `rec` covers, `scale` rest px a font unit, dilated by `r` rest
/// px and hinted by record `hint` (0: none).
// The CPU renderer's glyphs drawn straight from outlines use this (render/cpu_boot, IB-2).
#[allow(dead_code)]
pub(crate) fn coverage(store: &OutlineStore, rec: u32, hint: u32, local: [f32; 2], scale: f32, r: f32, hw: f32) -> f32 {
    let t = |i: u32| store.texel(i);
    let f = f32::from_bits;
    let head = t(rec);
    let (count, y0, inv_h) = (head[1], f(head[2]), f(head[3]));
    let h = (hint != 0).then(|| [t(hint), t(hint + 1), t(hint + 2)].map(|v| v.map(f)));
    let hy = |y: f32| h.map_or(y, |[a, b, c]| hint_y(y, a, b, c));
    let reach = if h.is_some() { super::HINT_REACH } else { 0.0 };
    let (b0, b1) = if count > 1 {
        let last = (count - 1) as f32;
        let band = |u: f32| ((u / scale - y0) * inv_h).floor().clamp(0.0, last) as u32;
        (band(local[1] - hw - 2.0 * r - reach), band(local[1] + hw + reach))
    } else {
        (0, 0)
    };
    let mut acc = 0.0f32;
    for band in b0..=b1 {
        let (e0, e1) = if count > 1 { ((y0 + band as f32 / inv_h) * scale, (y0 + (band + 1) as f32 / inv_h) * scale) } else { (-3.0e38, 3.0e38) };
        let entry = t(head[0] + band);
        let sorted = entry[1] > super::build::SORT_MIN as u32;
        for k in 0..entry[1] {
            let (t0, t1) = (t(entry[0] + 2 * k), t(entry[0] + 2 * k + 1));
            if f(t1[3]) * scale + r - local[0] <= -hw {
                if sorted { break; }
                continue;
            }
            let code = t1[2];
            let (oa, ob) = (moved(code & 15, r), moved((code >> 4) & 15, r));
            let at = |x: u32, y: u32, o: [f32; 2]| [f(x) * scale + o[0], f(y) * scale + o[1]];
            let (mut p0, mut p1, mut p2) = (at(t0[0], t0[1], oa), at(t0[2], t0[3], oa), at(t1[0], t1[1], ob));
            let (mut wlo, mut whi) = (-hw, hw);
            let (be0, be1) = (hy(e0 + oa[1]), hy(e1 + oa[1]));
            (p0[1], p1[1], p2[1]) = (hy(p0[1]), hy(p1[1]), hy(p2[1]));
            if code & super::build::CONNECTOR != 0 {
                p1 = [(p0[0] + p2[0]) * 0.5, (p0[1] + p2[1]) * 0.5];
            } else {
                wlo = wlo.max(be0 - local[1]);
                whi = whi.min(be1 - local[1]);
            }
            let rel = |p: [f32; 2]| [p[0] - local[0], p[1] - local[1]];
            let (q1, q2, q3) = (rel(p0), rel(p1), rel(p2));
            let lo = wlo.max(q1[1].min(q3[1]));
            let hi = whi.min(q1[1].max(q3[1]));
            if hi <= lo || q1[0].max(q3[0]) <= -hw {
                continue;
            }
            if q1[0].min(q3[0]) >= hw {
                acc += 2.0 * hw * (q3[1].clamp(wlo, whi) - q1[1].clamp(wlo, whi));
                continue;
            }
            let ty = y_lanes(q1[1], q2[1], q3[1], lo, hi);
            acc += integrate_x(q1, q2, q3, ty, hw);
        }
    }
    (acc.abs() / (4.0 * hw * hw)).min(1.0)
}

pub(super) fn moved(code: u32, r: f32) -> [f32; 2] {
    [((code & 3) as f32 - 1.0) * r, ((code >> 2) & 3) as f32 * r]
}

pub(super) fn hint_y(y: f32, a: [f32; 4], b: [f32; 4], c: [f32; 4]) -> f32 {
    if y < a[1] { return y; }
    if y <= a[2] { return (y - a[1]) * c[0] + a[1]; }
    if y <= b[0] { return (y - a[2]) * c[1] + a[3]; }
    if y <= b[2] { return y + (b[1] - b[0]); }
    (y - b[2]) * c[2] + b[3]
}

/// Where a piece's y crosses `lo` and `hi`, from 0 to 1: `outline_integrate`'s first two lanes,
/// which depend only on the row, so a mask's fill takes them once per row and piece.
pub(super) fn y_lanes(q1: f32, q2: f32, q3: f32, lo: f32, hi: f32) -> [f32; 2] {
    y_lanes_on(q1, q2, q3, lo, hi)
}

/// [`y_lanes`] on any lanes.
#[inline(always)]
pub(super) fn y_lanes_on<L: Lanes>(q1: L, q2: L, q3: L, lo: L, hi: L) -> [L; 2] {
    let two = L::splat(2.0);
    let (a2, a1) = (q1 - two * q2 + q3, two * (q2 - q1));
    let rising = q3.ge(q1);
    let (v0, v1) = (L::select(rising, lo, hi), L::select(rising, hi, lo));
    [lane(a2, a1, q1, q3, v0, rising), lane(a2, a1, q1, q3, v1, rising)]
}

/// One lane of `outline_integrate`: where the quadratic `a2 t^2 + a1 t + q1` reaches `v`.
#[inline(always)]
fn lane<L: Lanes>(a2: L, a1: L, q1: L, q3: L, v: L, rising: L::Mask) -> L {
    let (zero, one) = (L::splat(0.0), L::splat(1.0));
    let sg = L::select(rising, one, -one);
    let c = q1 - v;
    let sat0 = (c * sg).ge(zero);
    let sat1 = ((q3 - v) * sg).le(zero);
    let sq = (a1 * a1 - L::splat(4.0) * a2 * c).max(zero).sqrt();
    let qq = L::splat(-0.5) * (a1 + L::select(a1.ge(zero), sq, -sq));
    let use_r1 = L::same(a1.lt(zero), rising);
    let (num, den) = (L::select(use_r1, qq, c), L::select(use_r1, a2, qq));
    let valid = den.ne(zero);
    let t_raw = L::select(valid, (num / L::select(valid, den, one)).clamp(zero, one), zero);
    L::select(sat0, zero, L::select(sat1, one, t_raw))
}

/// The box-filtered winding of a piece `q1 q2 q3` (relative to the texel's centre) within the
/// rows its y lanes `t` bound, `2 * hx` wide: the rest of `outline_integrate`.
#[inline(always)]
pub(super) fn integrate_x<L: Lanes>(q1: [L; 2], q2: [L; 2], q3: [L; 2], t: [L; 2], hx: L) -> L {
    let (zero, two) = (L::splat(0.0), L::splat(2.0));
    let a2 = [q1[0] - two * q2[0] + q3[0], q1[1] - two * q2[1] + q3[1]];
    let a1 = [two * (q2[0] - q1[0]), two * (q2[1] - q1[1])];
    let x_rising = q3[0].ge(q1[0]);
    let tx = [lane(a2[0], a1[0], q1[0], q3[0], -hx, x_rising), lane(a2[0], a1[0], q1[0], q3[0], hx, x_rising)];
    let tc = [tx[0].clamp(t[0], t[1]), tx[1].clamp(t[0], t[1])];
    let t1 = L::select(x_rising, tc[0], tc[1]);
    let t2 = L::select(x_rising, tc[1], tc[0]).max(t1);
    let inside = {
        let half = L::splat(0.5);
        let tm = half * (t1 + t2);
        let d = half * (t2 - t1);
        let x_mid = (a2[0] * tm + a1[0]) * tm + q1[0] + hx;
        let dmid = [two * a2[0] * tm + a1[0], two * a2[1] * tm + a1[1]];
        let v = two * d * x_mid * dmid[1] + (two * d * d * d / L::splat(3.0)) * (a2[0] * dmid[1] + two * a2[1] * dmid[0]);
        L::select(t2.le(t1), zero, v)
    };
    let rab = [L::select(x_rising, t2, t[0]), L::select(x_rising, t[1], t1)];
    let d = (rab[1] - rab[0]).max(zero);
    let tm = L::splat(0.5) * (rab[0] + rab[1]);
    let acc = inside + d * (two * a2[1] * tm + a1[1]) * (two * hx);
    L::select(t[1].le(t[0]), zero, acc)
}
