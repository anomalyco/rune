//! A whole atlas mask's bytes on the CPU, as the GPU's fill writes them (render/fill.rs).

use std::cell::RefCell;

use super::cover::{hint_y, integrate_x, moved, y_lanes_on};
use super::lanes::{F4, Lanes};
use super::store::OutlineStore;
use crate::text::atlas_fill::{Fill, QueuedFill};

/// The bytes the GPU's atlas fill (render/fill.rs) writes for a `w` x `h` mask whose top-left texel
/// is `at` on its page, into `out`, row by row: the fill instance's arithmetic for each texel's
/// position (its pen is `at + fill.pen` in f32, the fragment's centre `at + texel + 0.5`), the
/// shader's per-piece integrals, and its rounding to the R8 target's levels, ties to even (the
/// fill's shader rounds itself, `params.w < 0`, rather than leave it to the GPU's float-to-unorm
/// conversion, which differs between vendors). Measured against both of blade's GPUs
/// (`Renderer::fill_drift`): the same bytes except texels within about 1e-4 of a rounding boundary
/// (0 to 0.006% of them, one level), where sums in another order, and the GPU's division, square
/// root and fused multiply-adds, round differently. A subpixel mask (coverage 3) is three texels
/// a pixel, each one subpixel's pixel-wide box.
///
/// Texels are filled four at a time ([`F4`]), every lane the same arithmetic as one texel alone,
/// so the bytes don't depend on the lanes. A piece wholly right of a texel adds a constant and one
/// wholly left adds nothing, judged by the piece's whole extent as the shader judges it; and within
/// a row a piece is only the stretch between its two y crossings, so a texel clear of that stretch
/// takes what the integral would give there (a constant right of it, nothing left of it), with a
/// margin far wider than the integral's rounding. The integral runs only where a lane's texel
/// meets a piece's stretch.
pub(crate) fn fill_mask(store: &OutlineStore, fill: &Fill, at: [u32; 2], w: u32, h: u32, out: &mut Vec<u8>) {
    SCRATCH.with_borrow_mut(|s| {
        rows_in::<F4>(s, store, fill, at[1], h);
        texels_in::<F4>(s, fill.coverage, fill.pen[0], at[0], w, h, out, None);
    });
}

/// Groups of `fills` whose masks share their rows, as indices, in first-seen order: one outline
/// and look (record, hint, scale, radius, coverage function), one height and one pen y. A glyph
/// drawn at several x phases is one group; only its texels' x differ.
pub(crate) fn share_rows(fills: &[QueuedFill]) -> Vec<Vec<u32>> {
    let mut groups: Vec<Vec<u32>> = Vec::new();
    let mut index: std::collections::HashMap<[u32; 7], usize> = std::collections::HashMap::with_capacity(fills.len());
    for (i, q) in fills.iter().enumerate() {
        let f = &q.fill;
        let key = [f.record, f.hint, f.scale.to_bits(), f.radius.to_bits(), f.coverage, q.rect[3], f.pen[1].to_bits()];
        let k = *index.entry(key).or_insert_with(|| { groups.push(Vec::new()); groups.len() - 1 });
        groups[k].push(i as u32);
    }
    groups
}

/// Fills one group of [`share_rows`] (`group` indexes `fills`), each mask into its `outs` entry
/// (cleared first): the rows once, then each mask's texels. A mask whose rows land elsewhere
/// (its pen y rounds otherwise on its page) takes rows of its own.
pub(crate) fn fill_shared<L: Lanes>(store: &OutlineStore, fills: &[QueuedFill], group: &[u32], outs: &mut [Vec<u8>], mut census: Option<&mut Census>) {
    SCRATCH.with_borrow_mut(|s| {
        let mut rows_for: Option<usize> = None;
        for (&i, out) in group.iter().zip(outs.iter_mut()) {
            let q = &fills[i as usize];
            let [x, y, w, h] = q.rect;
            let pen_y = y as f32 + q.fill.pen[1];
            let same = rows_for.is_some() && (0..h).all(|row| pen_y - ((y + row) as f32 + 0.5) == s.ly[row as usize]);
            if !same {
                rows_in::<L>(s, store, &q.fill, y, h);
                rows_for = Some(i as usize);
                if let Some(c) = census.as_deref_mut() { c.pieces += s.pieces.len() as u64; c.row_sets += 1; }
            }
            out.clear();
            texels_in::<L>(s, q.fill.coverage, q.fill.pen[0], x, w, h, out, census.as_deref_mut());
        }
    });
}
/// What a fill did: its texels, the integrals it took, the pieces it moved and the pieces that
/// crossed a row.
#[derive(Clone, Copy, Debug, Default)]
pub struct Census {
    pub texels: u64,
    pub integrals: u64,
    /// Texels that came out 0, and 255.
    pub clear: u64,
    pub solid: u64,
    pub pieces: u64,
    pub row_pieces: u64,
    /// Rows worked out, once for each group of masks that share them.
    pub row_sets: u64,
}

/// A margin (rest px) between a texel's box and a piece's row stretch, past which the texel takes
/// the integral's value without computing it: the integral's own rounding is about 1e-5 px at
/// glyph sizes, so this is some thousand times that.
const MARGIN: f32 = 1.0 / 64.0;

thread_local! {
    static SCRATCH: RefCell<Scratch> = RefCell::new(Scratch::default());
}

/// A thread's fill buffers, kept between masks.
#[derive(Default)]
struct Scratch {
    pieces: Vec<Piece>,
    /// Each row's bands; its pieces before and after their y lanes; where each row's start.
    bands: Vec<(u32, u32)>,
    cands: Vec<Cand>,
    rows: Vec<Row>,
    starts: Vec<u32>,
    /// Each row's pieces by left edge (ranges as `rows`), its tails (one more each), its y.
    by_min: Vec<u32>,
    tail: Vec<f32>,
    ly: Vec<f32>,
    /// Per group of texels and piece, the piece's term in each lane; the integrals to fill in.
    slots: Vec<[f32; 4]>,
    started: Vec<[f32; 4]>,
    jobs: Vec<Job>,
}

/// An integral to take: its slot and lane, its row piece, and the texel's x.
#[derive(Clone, Copy)]
struct Job { slot: u32, lane: u8, row: u32, lx: f32 }

/// A piece of one band, moved and hinted once for the whole mask (the same arithmetic the shader
/// does per pixel): its points, its band's moved edges, its y and x extent, whether its x is
/// monotone (dilation moves its end from its control point, which can bend it back), and the
/// rows whose box its y extent reaches.
struct Piece { band: u32, p: [[f32; 2]; 3], be: [f32; 2], connector: bool, x: [f32; 2], monotone_x: bool, rows: [u32; 2] }

/// A piece in a row, before its y lanes: the row, its points' y from the row's centre, the y
/// bounds it's weighed between, the box's bounds in its band, and the lanes once taken.
struct Cand { piece: u32, row: u32, q: [f32; 3], lo: f32, hi: f32, w: [f32; 2], ty: [f32; 2] }

/// A piece within one row: its y lanes, the constant it adds right of a texel, its x extent and
/// points, and its stretch between the row's y crossings.
struct Row { ty: [f32; 2], right: f32, x: [f32; 2], p: [[f32; 2]; 3], stretch: Stretch }

#[derive(Clone, Copy)]
enum Stretch {
    /// It doesn't cross the row: its integral is nothing anywhere.
    Empty,
    /// Its x within the row is `lo..hi` (margin included); right of a texel it adds `right`.
    Within { lo: f32, hi: f32, right: f32 },
    /// Its x isn't monotone: every texel it spans integrates.
    Bent,
}

/// The rows of a mask `h` texels tall whose top-left texel is at y `at_y` on its page: every
/// piece the shader weighs in each row, with its y lanes and constants, and the order a texel
/// sums them in. Masks of one outline and look whose rows sit at the same y share these.
fn rows_in<L: Lanes>(s: &mut Scratch, store: &OutlineStore, fill: &Fill, at_y: u32, h: u32) {
    let t = |i: u32| store.texel(i);
    let f = f32::from_bits;
    let (rec, scale, r, hw) = (fill.record, fill.scale, fill.radius, 0.5f32);
    let pen_y = at_y as f32 + fill.pen[1];
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
    let ly_of = |row: u32| pen_y - ((at_y + row) as f32 + 0.5);
    // The first row from which `test(y, ly)` (false, then true down the mask, as a row's y falls)
    // holds: from an estimate `near`, moved to the exact row.
    let first_row = |near: f32, test: fn(f32, f32) -> bool, y: f32| {
        let mut row = near.clamp(0.0, h as f32) as u32;
        while row > 0 && test(y, ly_of(row - 1)) { row -= 1; }
        while row < h && !test(y, ly_of(row)) { row += 1; }
        row
    };
    let (first, _) = row_bands(ly_of(h.saturating_sub(1)));
    let (_, last) = row_bands(ly_of(0));
    s.pieces.clear();
    // A row's y is `top - row`, near enough to start the search for a piece's rows.
    let top = pen_y - at_y as f32 - 0.5;
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
            let x = [p0[0].min(p2[0]), p0[0].max(p2[0])];
            // The rows the shader weighs it at: not wholly below a row's box, nor wholly above.
            let rows = [first_row(top - y[1] - hw, |y, ly| y - ly > -0.5, y[1]), first_row(top - y[0] + hw, |y, ly| y - ly >= 0.5, y[0])];
            s.pieces.push(Piece { band, p: [p0, p1, p2], be, connector, x, monotone_x: p1[0] >= x[0] && p1[0] <= x[1], rows });
        }
    }
    let pieces = &s.pieces;
    // Every piece's rows, piece by piece, so each row lists its pieces in record order: those the
    // shader weighs there (in the row's bands, reaching into its box), with their y lanes taken
    // four at a time.
    s.bands.clear();
    s.bands.extend((0..h).map(|row| row_bands(ly_of(row))));
    s.cands.clear();
    for (i, pc) in pieces.iter().enumerate() {
        for row in pc.rows[0]..pc.rows[1] {
            let (b0, b1) = s.bands[row as usize];
            if pc.band < b0 || pc.band > b1 { continue; }
            let ly = ly_of(row);
            let (mut wlo, mut whi) = (-hw, hw);
            if !pc.connector {
                wlo = wlo.max(pc.be[0] - ly);
                whi = whi.min(pc.be[1] - ly);
            }
            let q = [pc.p[0][1] - ly, pc.p[1][1] - ly, pc.p[2][1] - ly];
            let lo = wlo.max(q[0].min(q[2]));
            let hi = whi.min(q[0].max(q[2]));
            if hi <= lo { continue; }
            s.cands.push(Cand { piece: i as u32, row, q, lo, hi, w: [wlo, whi], ty: [0.0; 2] });
        }
    }
    for chunk in s.cands.chunks_mut(L::N) {
        let mut v = [[0.0f32; 4]; 5];
        for k in 0..L::N {
            let c = &chunk[k.min(chunk.len() - 1)];
            [v[0][k], v[1][k], v[2][k], v[3][k], v[4][k]] = [c.q[0], c.q[1], c.q[2], c.lo, c.hi];
        }
        let l = |n: usize| L::load(&v[n]);
        let [t0, t1] = y_lanes_on(l(0), l(1), l(2), l(3), l(4)).map(L::store);
        for (k, c) in chunk.iter_mut().enumerate() { c.ty = [t0[k], t1[k]]; }
    }
    // The row pieces, row by row (a stable count by row keeps record order within each).
    s.starts.clear();
    s.starts.resize(h as usize + 2, 0);
    for c in &s.cands { s.starts[c.row as usize + 2] += 1; }
    for k in 1..s.starts.len() - 1 { s.starts[k + 1] += s.starts[k]; }
    s.rows.clear();
    s.rows.extend(s.cands.iter().map(|_| Row { ty: [0.0; 2], right: 0.0, x: [0.0; 2], p: [[0.0; 2]; 3], stretch: Stretch::Empty }));
    for c in &s.cands {
        let pc = &pieces[c.piece as usize];
        let ([q1y, q2y, q3y], ty) = (c.q, c.ty);
        let right = 2.0 * hw * (q3y.clamp(c.w[0], c.w[1]) - q1y.clamp(c.w[0], c.w[1]));
        let stretch = if ty[1] <= ty[0] {
            Stretch::Empty
        } else if !pc.monotone_x {
            Stretch::Bent
        } else {
            // The integral right of a texel: its x lanes both clamp to the same end of `ty`,
            // which leaves `outline_integrate`'s last term over the whole of `ty`.
            let (a2y, a1y) = (q1y - 2.0 * q2y + q3y, 2.0 * (q2y - q1y));
            let d = (ty[1] - ty[0]).max(0.0);
            let tm = 0.5 * (ty[0] + ty[1]);
            let right = d * (2.0 * a2y * tm + a1y) * (2.0 * hw);
            let [p0, p1, p2] = pc.p;
            let (a2x, a1x) = (p0[0] - 2.0 * p1[0] + p2[0], 2.0 * (p1[0] - p0[0]));
            let x = |t: f32| (a2x * t + a1x) * t + p0[0];
            let (xa, xb) = (x(ty[0]), x(ty[1]));
            Stretch::Within { lo: xa.min(xb) - MARGIN, hi: xa.max(xb) + MARGIN, right }
        };
        let slot = &mut s.starts[c.row as usize + 1];
        s.rows[*slot as usize] = Row { ty, right, x: pc.x, p: pc.p, stretch };
        *slot += 1;
    }
    // Each row's pieces by their left edge (stable: equal edges keep record order), and the
    // row's tails: `tail[k]` sums the constants of the pieces from `k` on in that order, which
    // are still wholly right of a texel whose box starts left of their left edge. A texel's sum
    // starts from the tail of the pieces not yet spanning it, then adds each piece's term in
    // that order.
    s.by_min.clear();
    s.tail.clear();
    for row in 0..h as usize {
        let (a, b) = (s.starts[row] as usize, s.starts[row + 1] as usize);
        let rows = &s.rows[a..b];
        for i in 0..rows.len() as u32 {
            let at = s.by_min[a..].partition_point(|&j| rows[j as usize].x[0].total_cmp(&rows[i as usize].x[0]).is_le());
            s.by_min.insert(a + at, i);
        }
        let base = s.tail.len();
        s.tail.resize(base + rows.len() + 1, 0.0);
        for k in (0..rows.len()).rev() { s.tail[base + k] = s.tail[base + k + 1] + rows[s.by_min[a + k] as usize].right; }
    }
    s.ly.clear();
    s.ly.extend((0..h).map(ly_of));
}

/// A mask's texels from its rows ([`rows_in`]): `w` wide, its top-left texel at x `at_x` on its
/// page, its pen `pen_x` texels right of that.
#[allow(clippy::too_many_arguments)]
fn texels_in<L: Lanes>(s: &mut Scratch, coverage: u32, pen_x: f32, at_x: u32, w: u32, h: u32, out: &mut Vec<u8>, mut census: Option<&mut Census>) {
    let hw = 0.5f32;
    let across = if coverage == 3 { 3.0 } else { 1.0 };
    let pen = at_x as f32 + pen_x;
    // A texel's x from the pen (x / 1 is x exactly: only a subpixel mask divides).
    let lx_of = |col: u32| if across == 1.0 { (at_x + col) as f32 + 0.5 - pen } else { ((at_x + col) as f32 + 0.5 - pen) / across };
    let (zero, one, half) = (L::splat(0.0), L::splat(1.0), L::splat(hw));
    for row in 0..h {
        let (a, b) = (s.starts[row as usize] as usize, s.starts[row as usize + 1] as usize);
        let rows = &s.rows[a..b];
        let by_min = &s.by_min[a..b];
        let tail = &s.tail[a + row as usize..b + row as usize + 1];
        let base = out.len();
        out.resize(base + w as usize, 0);
        // No piece in the row: every texel is clear (its sum is the empty tail, zero).
        if rows.is_empty() {
            if let Some(c) = census.as_deref_mut() { c.texels += u64::from(w); c.clear += u64::from(w); }
            continue;
        }
        let ly = s.ly[row as usize];
        // Each group of `L::N` texels (a short last group repeats its last texel) and each piece
        // spanning any of them: per lane, the constant the piece adds there, then the integrals
        // where a lane's texel meets the piece's stretch, taken `L::N` at a time across the row.
        let n = rows.len();
        let groups = (w as usize).div_ceil(L::N);
        s.slots.clear();
        s.slots.resize(groups * n, [0.0; 4]);
        s.started.clear();
        s.jobs.clear();
        for g in 0..groups {
            let c0 = (g * L::N) as u32;
            let lanes = (w - c0).min(L::N as u32) as usize;
            let lxs: [f32; 4] = std::array::from_fn(|k| lx_of(c0 + k.min(lanes - 1) as u32));
            let lx = L::load(&lxs);
            // How many pieces, in left-edge order, have started to span each lane's texel.
            let mut started = zero;
            for (k, &i) in by_min.iter().enumerate() {
                let rp = &rows[i as usize];
                let begun = (L::splat(rp.x[0]) - lx).lt(half);
                started = started + L::select(begun, one, zero);
                let spans = L::and(begun, (L::splat(rp.x[1]) - lx).gt(-half));
                let (constant, integral) = match rp.stretch {
                    Stretch::Empty => continue,
                    Stretch::Within { lo, hi, right } => {
                        let right_of = (L::splat(lo) - lx).ge(half);
                        let left_of = (L::splat(hi) - lx).le(-half);
                        (L::select(L::and(spans, right_of), L::splat(right), zero), L::and_not(L::and_not(spans, right_of), left_of))
                    }
                    Stretch::Bent => (zero, spans),
                };
                let slot = g * n + k;
                s.slots[slot] = constant.store();
                let mut wanted = L::bits(integral) & ((1 << lanes) - 1);
                while wanted != 0 {
                    let lane = wanted.trailing_zeros();
                    wanted &= wanted - 1;
                    s.jobs.push(Job { slot: slot as u32, lane: lane as u8, row: i, lx: lxs[lane as usize] });
                }
            }
            s.started.push(started.store());
        }
        for chunk in s.jobs.chunks(L::N) {
            let mut v = [[0.0f32; 4]; 8];
            for k in 0..L::N {
                let j = chunk[k.min(chunk.len() - 1)];
                let r = &rows[j.row as usize];
                for m in 0..3 {
                    v[2 * m][k] = r.p[m][0] - j.lx;
                    v[2 * m + 1][k] = r.p[m][1] - ly;
                }
                v[6][k] = r.ty[0];
                v[7][k] = r.ty[1];
            }
            let l = |m: usize| L::load(&v[m]);
            let cov = integrate_x([l(0), l(1)], [l(2), l(3)], [l(4), l(5)], [l(6), l(7)], half).store();
            for (k, j) in chunk.iter().enumerate() { s.slots[j.slot as usize][j.lane as usize] = cov[k]; }
        }
        if let Some(c) = census.as_deref_mut() { c.integrals += s.jobs.len() as u64; }
        // Each lane's sum: the tail of the pieces whose left edge is still right of its texel,
        // then every piece's term in left-edge order.
        for g in 0..groups {
            let c0 = g * L::N;
            let lanes = (w as usize - c0).min(L::N);
            let tails: [f32; 4] = s.started[g].map(|k| tail[k as usize]);
            let acc = s.slots[g * n..(g + 1) * n].iter().fold(L::load(&tails), |acc, v| acc + L::load(v)).store();
            for (k, &v) in acc[..lanes].iter().enumerate() { out[base + c0 + k] = byte(v, hw); }
        }
        if let Some(c) = census.as_deref_mut() {
            c.texels += u64::from(w);
            c.row_pieces += rows.len() as u64;
            let row = &out[base..];
            c.clear += row.iter().filter(|&&v| v == 0).count() as u64;
            c.solid += row.iter().filter(|&&v| v == 255).count() as u64;
        }
    }
}

/// A texel's level: its coverage rounded to the R8 target's, ties to even.
#[inline(always)]
fn byte(acc: f32, hw: f32) -> u8 {
    // Adding and taking away 2^23 rounds a value in 0..=255 to an integer, ties to even, in the
    // default rounding mode: `round_ties_even` without a call on CPUs before SSE4.1.
    const SHIFT: f32 = 8_388_608.0;
    (((acc.abs() / (4.0 * hw * hw)).min(1.0) * 255.0 + SHIFT) - SHIFT) as u8
}
