//! outline.wgsl's coverage on the CPU, line for line in f32: masks placed before any renderer
//! can fill them (the first frame, a runtime drawn without one) come out as the GPU fills them.

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
            acc += integrate(q1, q2, q3, lo, hi, hw);
        }
    }
    (acc.abs() / (4.0 * hw * hw)).min(1.0)
}

/// The bytes the GPU's atlas fill (render/fill.rs) writes for a `w` x `h` mask whose top-left texel
/// is `at` on its page, into `out`, row by row: the fill instance's arithmetic for each texel's
/// position (its pen is `at + fill.pen` in f32, the fragment's centre `at + texel + 0.5`), the
/// shader's per-piece integrals, and its rounding to the R8 target's levels, ties to even (the
/// fill's shader rounds itself, `params.w < 0`, rather than leave it to the GPU's float-to-unorm
/// conversion, which differs between vendors). A row's pieces are found and moved once; a piece
/// wholly right of a texel adds a constant and one wholly left adds nothing, so each texel
/// integrates only the few that cross it. Measured against both of blade's GPUs
/// (`Renderer::fill_drift`): the same bytes except texels within about 1e-4 of a rounding boundary
/// (0 to 0.006% of them, one level), where sums in another order, and the GPU's division, square
/// root and fused multiply-adds, round differently. A subpixel mask (coverage 3) is three texels
/// a pixel, each one subpixel's pixel-wide box.
pub(crate) fn fill_mask(store: &OutlineStore, fill: &crate::text::atlas_fill::Fill, at: [u32; 2], w: u32, h: u32, out: &mut Vec<u8>) {
    // A piece of one band, moved and hinted once for the whole mask (the same arithmetic the
    // shader does per pixel): its points, its band's moved edges, its y and x extent.
    struct Piece { band: u32, p: [[f32; 2]; 3], be: [f32; 2], connector: bool, y: [f32; 2], x: [f32; 2] }
    struct Row { ty: [f32; 2], wlo: f32, whi: f32, right: f32, minx: f32, maxx: f32, p: [[f32; 2]; 3] }
    let t = |i: u32| store.texel(i);
    let f = f32::from_bits;
    let (rec, scale, r, hw) = (fill.record, fill.scale, fill.radius, 0.5f32);
    let across = if fill.coverage == 3 { 3.0 } else { 1.0 };
    let pen = [at[0] as f32 + fill.pen[0], at[1] as f32 + fill.pen[1]];
    let head = t(rec);
    let (count, y0, inv_h) = (head[1], f(head[2]), f(head[3]));
    let hint = (fill.hint != 0).then(|| [t(fill.hint), t(fill.hint + 1), t(fill.hint + 2)].map(|v| v.map(f)));
    let hy = |y: f32| hint.map_or(y, |[a, b, c]| hint_y(y, a, b, c));
    let reach = if hint.is_some() { super::HINT_REACH } else { 0.0 };
    // The bands the mask's rows reach, as the shader picks them per pixel.
    let row_bands = |ly: f32| if count > 1 {
        let last = (count - 1) as f32;
        let band = |u: f32| ((u / scale - y0) * inv_h).floor().clamp(0.0, last) as u32;
        (band(ly - hw - 2.0 * r - reach), band(ly + hw + reach))
    } else {
        (0, 0)
    };
    let ly_of = |row: u32| pen[1] - ((at[1] + row) as f32 + 0.5);
    let (first, _) = row_bands(ly_of(h.saturating_sub(1)));
    let (_, last) = row_bands(ly_of(0));
    let mut pieces: Vec<Piece> = Vec::new();
    for band in first..=last.max(first) {
        let (e0, e1) = if count > 1 { ((y0 + band as f32 / inv_h) * scale, (y0 + (band + 1) as f32 / inv_h) * scale) } else { (-3.0e38, 3.0e38) };
        let entry = t(head[0] + band);
        for k in 0..entry[1] {
            let (t0, t1) = (t(entry[0] + 2 * k), t(entry[0] + 2 * k + 1));
            let code = t1[2];
            let (oa, ob) = (moved(code & 15, r), moved((code >> 4) & 15, r));
            let at = |x: u32, y: u32, o: [f32; 2]| [f(x) * scale + o[0], f(y) * scale + o[1]];
            let (mut p0, mut p1, mut p2) = (at(t0[0], t0[1], oa), at(t0[2], t0[3], oa), at(t1[0], t1[1], ob));
            let be = [hy(e0 + oa[1]), hy(e1 + oa[1])];
            (p0[1], p1[1], p2[1]) = (hy(p0[1]), hy(p1[1]), hy(p2[1]));
            let connector = code & super::build::CONNECTOR != 0;
            if connector {
                p1 = [(p0[0] + p2[0]) * 0.5, (p0[1] + p2[1]) * 0.5];
            }
            let (ylo, yhi) = (p0[1].min(p2[1]), p0[1].max(p2[1]));
            let y = if connector { [ylo, yhi] } else { [ylo.max(be[0]), yhi.min(be[1])] };
            pieces.push(Piece { band, p: [p0, p1, p2], be, connector, y, x: [p0[0].min(p2[0]), p0[0].max(p2[0])] });
        }
    }
    let mut rows: Vec<Row> = Vec::new();
    let mut active: Vec<usize> = Vec::new();
    let mut by_min: Vec<usize> = Vec::new();
    let mut tail: Vec<f32> = Vec::new();
    for row in 0..h {
        let ly = ly_of(row);
        let (b0, b1) = row_bands(ly);
        rows.clear();
        for pc in &pieces {
            // Outside the row's bands, or wholly above or below its box: nothing, as in the shader.
            if pc.band < b0 || pc.band > b1 || pc.y[1] - ly <= -hw || pc.y[0] - ly >= hw { continue; }
            let (mut wlo, mut whi) = (-hw, hw);
            if !pc.connector {
                wlo = wlo.max(pc.be[0] - ly);
                whi = whi.min(pc.be[1] - ly);
            }
            let (q1y, q3y) = (pc.p[0][1] - ly, pc.p[2][1] - ly);
            let lo = wlo.max(q1y.min(q3y));
            let hi = whi.min(q1y.max(q3y));
            if hi <= lo { continue; }
            let right = 2.0 * hw * (q3y.clamp(wlo, whi) - q1y.clamp(wlo, whi));
            let ty = y_lanes(q1y, pc.p[1][1] - ly, q3y, lo, hi);
            rows.push(Row { ty, wlo, whi, right, minx: pc.x[0], maxx: pc.x[1], p: pc.p });
        }
        // Pieces by their left edge; `tail[k]` sums the constants of those from `k` on, which are
        // still wholly right of a texel whose box starts left of their left edge.
        by_min.clear();
        by_min.extend(0..rows.len());
        by_min.sort_by(|&a, &b| rows[a].minx.total_cmp(&rows[b].minx));
        tail.clear();
        tail.resize(rows.len() + 1, 0.0);
        for k in (0..rows.len()).rev() { tail[k] = tail[k + 1] + rows[by_min[k]].right; }
        active.clear();
        let mut next = 0;
        for col in 0..w {
            let lx = ((at[0] + col) as f32 + 0.5 - pen[0]) / across;
            while next < by_min.len() && rows[by_min[next]].minx - lx < hw {
                active.push(by_min[next]);
                next += 1;
            }
            active.retain(|&i| rows[i].maxx - lx > -hw);
            let mut acc = tail[next];
            for &i in &active {
                let pc = &rows[i];
                let rel = |p: [f32; 2]| [p[0] - lx, p[1] - ly];
                let (q1, q2, q3) = (rel(pc.p[0]), rel(pc.p[1]), rel(pc.p[2]));
                if q1[0].min(q3[0]) >= hw {
                    acc += 2.0 * hw * (q3[1].clamp(pc.wlo, pc.whi) - q1[1].clamp(pc.wlo, pc.whi));
                } else {
                    acc += integrate_x(q1, q2, q3, pc.ty, hw);
                }
            }
            out.push(((acc.abs() / (4.0 * hw * hw)).min(1.0) * 255.0).round_ties_even() as u8);
        }
    }
}

fn moved(code: u32, r: f32) -> [f32; 2] {
    [((code & 3) as f32 - 1.0) * r, ((code >> 2) & 3) as f32 * r]
}

fn hint_y(y: f32, a: [f32; 4], b: [f32; 4], c: [f32; 4]) -> f32 {
    if y < a[1] { return y; }
    if y <= a[2] { return (y - a[1]) * c[0] + a[1]; }
    if y <= b[0] { return (y - a[2]) * c[1] + a[3]; }
    if y <= b[2] { return y + (b[1] - b[0]); }
    (y - b[2]) * c[2] + b[3]
}

fn inside(a2: [f32; 2], a1: [f32; 2], x0: f32, ta: f32, tb: f32, hx: f32) -> f32 {
    if tb <= ta { return 0.0; }
    let tm = 0.5 * (ta + tb);
    let d = 0.5 * (tb - ta);
    let x_mid = (a2[0] * tm + a1[0]) * tm + x0 + hx;
    let dmid = [2.0 * a2[0] * tm + a1[0], 2.0 * a2[1] * tm + a1[1]];
    2.0 * d * x_mid * dmid[1] + (2.0 * d * d * d / 3.0) * (a2[0] * dmid[1] + 2.0 * a2[1] * dmid[0])
}

/// `outline_integrate`, its four lanes (y lo, y hi, x -hx, x +hx) written out.
fn integrate(q1: [f32; 2], q2: [f32; 2], q3: [f32; 2], lo: f32, hi: f32, hx: f32) -> f32 {
    let ty = y_lanes(q1[1], q2[1], q3[1], lo, hi);
    integrate_x(q1, q2, q3, ty, hx)
}

/// Where a piece's y crosses `lo` and `hi`, from 0 to 1: the integral's first two lanes, which
/// depend only on the row, so a mask's fill takes them once per row and piece.
fn y_lanes(q1: f32, q2: f32, q3: f32, lo: f32, hi: f32) -> [f32; 2] {
    let (a2, a1) = (q1 - 2.0 * q2 + q3, 2.0 * (q2 - q1));
    let rising = q3 >= q1;
    let (v0, v1) = if rising { (lo, hi) } else { (hi, lo) };
    [lane(a2, a1, q1, q3, v0, rising), lane(a2, a1, q1, q3, v1, rising)]
}

/// One lane of `outline_integrate`: where the quadratic `a2 t^2 + a1 t + q1` reaches `v`.
#[inline(always)]
fn lane(a2: f32, a1: f32, q1: f32, q3: f32, v: f32, rising: bool) -> f32 {
    let sg = if rising { 1.0 } else { -1.0 };
    let c = q1 - v;
    let sat0 = c * sg >= 0.0;
    let sat1 = (q3 - v) * sg <= 0.0;
    let sq = (a1 * a1 - 4.0 * a2 * c).max(0.0).sqrt();
    let qq = -0.5 * (a1 + if a1 >= 0.0 { sq } else { -sq });
    let use_r1 = (a1 < 0.0) == rising;
    let (num, den) = if use_r1 { (qq, a2) } else { (c, qq) };
    let t_raw = if den != 0.0 { (num / den).clamp(0.0, 1.0) } else { 0.0 };
    if sat0 { 0.0 } else if sat1 { 1.0 } else { t_raw }
}

/// The rest of `outline_integrate`, given its y lanes `t`.
#[inline(always)]
fn integrate_x(q1: [f32; 2], q2: [f32; 2], q3: [f32; 2], t: [f32; 2], hx: f32) -> f32 {
    if t[1] <= t[0] { return 0.0; }
    let a2 = [q1[0] - 2.0 * q2[0] + q3[0], q1[1] - 2.0 * q2[1] + q3[1]];
    let a1 = [2.0 * (q2[0] - q1[0]), 2.0 * (q2[1] - q1[1])];
    let x_rising = q3[0] >= q1[0];
    let tx = [lane(a2[0], a1[0], q1[0], q3[0], -hx, x_rising), lane(a2[0], a1[0], q1[0], q3[0], hx, x_rising)];
    let tc = [tx[0].clamp(t[0], t[1]), tx[1].clamp(t[0], t[1])];
    let t1 = if x_rising { tc[0] } else { tc[1] };
    let t2 = (if x_rising { tc[1] } else { tc[0] }).max(t1);
    let mut acc = inside(a2, a1, q1[0], t1, t2, hx);
    let rab = if x_rising { [t2, t[1]] } else { [t[0], t1] };
    let d = (rab[1] - rab[0]).max(0.0);
    let tm = 0.5 * (rab[0] + rab[1]);
    acc += d * (2.0 * a2[1] * tm + a1[1]) * (2.0 * hx);
    acc
}
