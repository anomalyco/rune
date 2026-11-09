//! Myers' O(ND) difference algorithm in linear space (divide and conquer on the middle snake),
//! over any sequence of comparable items: interned lines for a file, tokens for a line.

use std::ops::Range;

/// One run of an edit script, in order: `Equal` advances both sides, `Delete` the old side
/// and `Insert` the new side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    Equal { old: usize, new: usize, len: usize },
    Delete { old: usize, len: usize },
    Insert { new: usize, len: usize },
}

/// Past this many edits in one subproblem the search stops and splits it where it got
/// furthest: a file rewritten wholesale costs bounded time, and what it shows is no worse.
const MAX_D: usize = 1024;

/// The edit script turning `a` into `b`, with each run of equal items, deletions and
/// insertions coalesced.
pub(crate) fn diff<T: Eq>(a: &[T], b: &[T]) -> Vec<Op> {
    let mut changed_a = vec![false; a.len()];
    let mut changed_b = vec![false; b.len()];
    let size = a.len() + b.len() + 3;
    let mut v = Vs { f: vec![0; size], b: vec![0; size], off: b.len() as isize + 1 };
    conquer(a, 0..a.len(), b, 0..b.len(), &mut v, &mut changed_a, &mut changed_b);
    script(&changed_a, &changed_b)
}

/// The furthest-reaching x per diagonal (`x - y`), forward and backward, indexed from `-off`.
struct Vs {
    f: Vec<isize>,
    b: Vec<isize>,
    off: isize,
}

impl Vs {
    fn i(&self, k: isize) -> usize {
        (k + self.off) as usize
    }
}

fn prefix<T: Eq>(a: &[T], b: &[T]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

fn suffix<T: Eq>(a: &[T], b: &[T]) -> usize {
    a.iter().rev().zip(b.iter().rev()).take_while(|(x, y)| x == y).count()
}

fn conquer<T: Eq>(a: &[T], mut ra: Range<usize>, b: &[T], mut rb: Range<usize>, v: &mut Vs, ca: &mut [bool], cb: &mut [bool]) {
    let p = prefix(&a[ra.clone()], &b[rb.clone()]);
    ra.start += p;
    rb.start += p;
    let s = suffix(&a[ra.clone()], &b[rb.clone()]);
    ra.end -= s;
    rb.end -= s;
    if ra.is_empty() {
        cb[rb].fill(true);
    } else if rb.is_empty() {
        ca[ra].fill(true);
    } else if let Some((x, y)) = middle_snake(a, ra.clone(), b, rb.clone(), v) {
        conquer(a, ra.start..x, b, rb.start..y, v, ca, cb);
        conquer(a, x..ra.end, b, y..rb.end, v, ca, cb);
    } else {
        ca[ra].fill(true);
        cb[rb].fill(true);
    }
}

/// A point on an optimal path through `a[ra]` × `b[rb]` that splits it into two smaller
/// halves; past [`MAX_D`] edits, the furthest point reached on a path from the start.
/// Both ranges are non-empty and differ at both ends.
/// Diagonals are `x - y` in absolute indices; each search widens by one diagonal a step until
/// it meets the box's edge, and a neighbour outside the band reads as never reached.
fn middle_snake<T: Eq>(a: &[T], ra: Range<usize>, b: &[T], rb: Range<usize>, v: &mut Vs) -> Option<(usize, usize)> {
    let (xoff, xlim, yoff, ylim) = (ra.start as isize, ra.end as isize, rb.start as isize, rb.end as isize);
    let (dmin, dmax) = (xoff - ylim, xlim - yoff);
    let (fmid, bmid) = (xoff - yoff, xlim - ylim);
    let (mut fmin, mut fmax, mut bmin, mut bmax) = (fmid, fmid, bmid, bmid);
    let odd = (fmid - bmid) & 1 != 0;
    let at = |x: isize, y: isize| a[x as usize] == b[y as usize];
    let i = fmid + v.off;
    v.f[i as usize] = xoff;
    let i = bmid + v.off;
    v.b[i as usize] = xlim;
    for _ in 1..=MAX_D {
        if fmin > dmin {
            fmin -= 1;
            let i = v.i(fmin - 1);
            v.f[i] = -1;
        } else {
            fmin += 1;
        }
        if fmax < dmax {
            fmax += 1;
            let i = v.i(fmax + 1);
            v.f[i] = -1;
        } else {
            fmax -= 1;
        }
        let mut d = fmax;
        while d >= fmin {
            let (lo, hi) = (v.f[v.i(d - 1)], v.f[v.i(d + 1)]);
            let mut x = if lo >= hi { lo + 1 } else { hi };
            let mut y = x - d;
            while x < xlim && y < ylim && at(x, y) {
                x += 1;
                y += 1;
            }
            let i = v.i(d);
            v.f[i] = x;
            if odd && bmin <= d && d <= bmax && v.b[i] <= x {
                let (x, y) = (x.clamp(xoff, xlim), y.clamp(yoff, ylim));
                return ((x, y) != (xlim, ylim) && (x, y) != (xoff, yoff)).then_some((x as usize, y as usize));
            }
            d -= 2;
        }
        if bmin > dmin {
            bmin -= 1;
            let i = v.i(bmin - 1);
            v.b[i] = isize::MAX;
        } else {
            bmin += 1;
        }
        if bmax < dmax {
            bmax += 1;
            let i = v.i(bmax + 1);
            v.b[i] = isize::MAX;
        } else {
            bmax -= 1;
        }
        let mut d = bmax;
        while d >= bmin {
            let (lo, hi) = (v.b[v.i(d - 1)], v.b[v.i(d + 1)]);
            let mut x = if lo < hi { lo } else { hi - 1 };
            let mut y = x - d;
            while xoff < x && yoff < y && at(x - 1, y - 1) {
                x -= 1;
                y -= 1;
            }
            let i = v.i(d);
            v.b[i] = x;
            if !odd && fmin <= d && d <= fmax && x <= v.f[i] {
                let (x, y) = (x.clamp(xoff, xlim), y.clamp(yoff, ylim));
                return ((x, y) != (xlim, ylim) && (x, y) != (xoff, yoff)).then_some((x as usize, y as usize));
            }
            d -= 2;
        }
    }
    // Too costly to finish: split where the forward search got furthest, as xdiff does, so a
    // long file with many scattered edits still diffs line by line in bounded time.
    let (x, d) = (fmin..=fmax).step_by(2).map(|d| (v.f[v.i(d)], d)).max_by_key(|&(x, d)| 2 * x - d)?;
    let (x, y) = (x.clamp(xoff, xlim), (x - d).clamp(yoff, ylim));
    ((x, y) != (xlim, ylim) && (x, y) != (xoff, yoff)).then_some((x as usize, y as usize))
}

/// The runs of an edit script from each side's changed flags. Deletions come before the
/// insertions they pair with.
fn script(ca: &[bool], cb: &[bool]) -> Vec<Op> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < ca.len() || j < cb.len() {
        let (si, sj) = (i, j);
        while i < ca.len() && j < cb.len() && !ca[i] && !cb[j] {
            i += 1;
            j += 1;
        }
        if i > si {
            out.push(Op::Equal { old: si, new: sj, len: i - si });
        }
        let di = i;
        while i < ca.len() && ca[i] {
            i += 1;
        }
        if i > di {
            out.push(Op::Delete { old: di, len: i - di });
        }
        let dj = j;
        while j < cb.len() && cb[j] {
            j += 1;
        }
        if j > dj {
            out.push(Op::Insert { new: dj, len: j - dj });
        }
        if i == si && j == sj {
            // Unchanged items left on one side only can't happen in a valid script; stop safely.
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply<T: Clone + Eq + std::fmt::Debug>(a: &[T], b: &[T], ops: &[Op]) -> (Vec<T>, usize) {
        let mut out = Vec::new();
        let mut same = 0;
        for op in ops {
            match *op {
                Op::Equal { old, new, len } => {
                    assert_eq!(a[old..old + len], b[new..new + len]);
                    out.extend_from_slice(&a[old..old + len]);
                    same += len;
                }
                Op::Delete { .. } => {}
                Op::Insert { new, len } => out.extend_from_slice(&b[new..new + len]),
            }
        }
        (out, same)
    }

    fn lcs<T: Eq>(a: &[T], b: &[T]) -> usize {
        let mut dp = vec![vec![0usize; b.len() + 1]; a.len() + 1];
        for i in (0..a.len()).rev() {
            for j in (0..b.len()).rev() {
                dp[i][j] = if a[i] == b[j] { dp[i + 1][j + 1] + 1 } else { dp[i + 1][j].max(dp[i][j + 1]) };
            }
        }
        dp[0][0]
    }

    #[test]
    fn scripts_are_minimal_and_rebuild_the_new_side() {
        let mut seed = 0x2545_f491_u64;
        let mut next = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        for round in 0..1200 {
            let (len, alphabet) = if round % 3 == 0 { (40, 3) } else { (14, 4) };
            let (la, lb) = (next(len) as usize, next(len) as usize);
            let a: Vec<u8> = (0..la).map(|_| next(alphabet) as u8).collect();
            let b: Vec<u8> = (0..lb).map(|_| next(alphabet) as u8).collect();
            let ops = diff(&a, &b);
            let (rebuilt, same) = apply(&a, &b, &ops);
            assert_eq!(rebuilt, b, "{a:?} -> {b:?}: {ops:?}");
            assert_eq!(same, lcs(&a, &b), "{a:?} -> {b:?}: {ops:?}");
        }
    }

    #[test]
    fn many_scattered_edits_still_diff_line_by_line() {
        // Far past MAX_D edits: every seventh line changed across 30,000.
        let a: Vec<u32> = (0..30_000).collect();
        let b: Vec<u32> = (0..30_000).map(|i| if i % 7 == 3 { i + 1_000_000 } else { i }).collect();
        let ops = diff(&a, &b);
        let (rebuilt, same) = apply(&a, &b, &ops);
        assert_eq!(rebuilt, b);
        assert_eq!(same, 30_000 - 4_286, "only the changed lines differ");
    }

    #[test]
    fn adversarial_inputs_stay_within_bounds_and_rebuild_the_new_side() {
        // All equal.
        let eq: Vec<u32> = (0..2_500).collect();
        assert_eq!(diff(&eq, &eq), vec![Op::Equal { old: 0, new: 0, len: 2_500 }]);

        // All different, both below and past MAX_D.
        for n in [50usize, 2_000] {
            let a: Vec<u32> = (0..n as u32).collect();
            let b: Vec<u32> = (n as u32..2 * n as u32).collect();
            let ops = diff(&a, &b);
            let (rebuilt, same) = apply(&a, &b, &ops);
            assert_eq!(rebuilt, b);
            assert_eq!(same, 0);
        }

        // Short side matching inside a much longer side (both within and past MAX_D).
        let short: Vec<u32> = (1..=5).collect();
        for len in [20u32, 2_500] {
            let long: Vec<u32> = (0..len).collect();
            let ops = diff(&short, &long);
            let (rebuilt, same) = apply(&short, &long, &ops);
            assert_eq!(rebuilt, long);
            assert_eq!(same, 5);

            let ops_rev = diff(&long, &short);
            let (rebuilt_rev, same_rev) = apply(&long, &short, &ops_rev);
            assert_eq!(rebuilt_rev, short);
            assert_eq!(same_rev, 5);
        }

        // Repeated identical blocks (classic Myers pathology).
        let mut a: Vec<u32> = (0..400).flat_map(|_| [1, 2, 3, 4]).collect();
        let mut b = a.clone();
        a.splice(800..804, [9, 9]);
        b.splice(400..400, [1, 2, 3, 4, 8, 8]);
        let ops = diff(&a, &b);
        let (rebuilt, same) = apply(&a, &b, &ops);
        assert_eq!(rebuilt, b);
        assert!(same >= 1_596);
    }

    #[test]
    fn runs_coalesce_with_deletions_first() {
        let a = ["a", "b", "c", "d"];
        let b = ["a", "x", "y", "d", "e"];
        assert_eq!(
            diff(&a, &b),
            vec![
                Op::Equal { old: 0, new: 0, len: 1 },
                Op::Delete { old: 1, len: 2 },
                Op::Insert { new: 1, len: 2 },
                Op::Equal { old: 3, new: 3, len: 1 },
                Op::Insert { new: 4, len: 1 },
            ]
        );
        assert_eq!(diff::<u8>(&[], &[]), vec![]);
    }

    #[test]
    fn word_changes_cover_changed_words_and_join_across_spaces() {
        use super::super::words::words;
        let (a, b) = words("let total = price * count;", "let total = price * amount + tax;");
        assert_eq!(a, Some(20..25));
        assert_eq!(&"let total = price * amount + tax;"[b.unwrap()], "amount + tax");
        assert_eq!(words("fn main() {}", "SELECT * FROM users"), (Some(0..12), Some(0..19)));
        assert_eq!(words("same", "same"), (None, None));
    }
}
