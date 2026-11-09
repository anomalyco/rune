//! Which removed line sits beside which added line, as Pierre (OpenCode Desktop's diff view)
//! pairs them: by position, unless a run with more lines on one side matches better shifted
//! (`realignChangeContentBySimilarity`, which `parsePatchFiles` runs on every file).

use super::model::{Hunk, Line, LineKind};
use super::words;

/// Pierre's `MAX_ALIGNMENT_COMPARISONS`: a run needing more line comparisons than this pairs
/// by position.
const MAX_COMPARISONS: usize = 4096;
/// Pierre's `MIN_IMPROVEMENT_PER_PAIR`: how much more alike, per pair, a shifted pairing must
/// be than pairing by position.
const MIN_GAIN: f64 = 0.5;

/// A row of a hunk in split view, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitRow {
    /// Line `i` of the hunk alone: an unchanged line on both halves, a removed line on the old
    /// half, an added line on the new half.
    Line(usize),
    /// Removed line `del` on the old half beside added line `add` on the new half.
    Pair { del: usize, add: usize },
}

impl Hunk {
    /// The hunk's rows in split view: each line once, a paired removed line beside its added
    /// line ([`Line::pair`]), and every other line on a row of its own.
    pub fn split_rows(&self) -> impl Iterator<Item = SplitRow> + '_ {
        self.lines.iter().enumerate().filter_map(|(i, l)| match (l.kind, l.pair) {
            (LineKind::Added, Some(_)) => None,
            (LineKind::Removed, Some(add)) => Some(SplitRow::Pair { del: i, add }),
            _ => Some(SplitRow::Line(i)),
        })
    }
}

/// Lays out and pairs every run of changed lines in `hunks` as Pierre does: a run's removed
/// lines first, then its added ones, the first of each paired in order, and their word changes
/// marked. A run with more lines on one side pairs where its lines match best instead
/// ([`blocks`]), which splits it into the unpaired lines before the pairs, the pairs, and the
/// unpaired lines after them, each laid out removed first. Lines never cross an unchanged one.
pub(super) fn pair_lines(hunks: &mut [Hunk]) {
    for h in hunks {
        let mut lines = Vec::with_capacity(h.lines.len());
        let mut run = (Vec::new(), Vec::new());
        for mut l in std::mem::take(&mut h.lines) {
            l.pair = None;
            l.changes.clear();
            match l.kind {
                LineKind::Removed => run.0.push(l),
                LineKind::Added => run.1.push(l),
                LineKind::Context => {
                    push_run(&mut lines, &mut run);
                    lines.push(l);
                }
            }
        }
        push_run(&mut lines, &mut run);
        h.lines = lines;
    }
}

/// Appends a run of removed and added lines to `lines`, block by block, pairing each block's
/// lines (`iterateOverDiff` pairs a change block's lines by index; `renderDiffWithHighlighter`
/// marks the word changes of each pair).
fn push_run(lines: &mut Vec<Line>, run: &mut (Vec<Line>, Vec<Line>)) {
    let blocks = blocks(&run.0, &run.1);
    let (mut removed, mut added) = (std::mem::take(&mut run.0).into_iter(), std::mem::take(&mut run.1).into_iter());
    for (d, a) in blocks {
        let start = lines.len();
        lines.extend(removed.by_ref().take(d));
        lines.extend(added.by_ref().take(a));
        for k in 0..d.min(a) {
            let (i, j) = (start + k, start + d + k);
            let (old, new) = words::words(&lines[i].text, &lines[j].text);
            lines[i].pair = Some(j);
            lines[i].changes = old.into_iter().collect();
            lines[j].pair = Some(i);
            lines[j].changes = new.into_iter().collect();
        }
    }
}

/// The blocks a run of `removed` and `added` lines splits into, as (removed, added) counts:
/// the whole run, unless it has more lines on one side and pairing its lines shifted past some
/// of the longer side's matches better than pairing them in order (`realignChangeBlock`).
fn blocks(removed: &[Line], added: &[Line]) -> Vec<(usize, usize)> {
    let (d, a) = (removed.len(), added.len());
    let (pairs, surplus) = (d.min(a), d.abs_diff(a));
    if pairs == 0 || surplus == 0 || pairs * (surplus + 1) > MAX_COMPARISONS {
        return vec![(d, a)];
    }
    let (removed, added): (Vec<Vec<u16>>, Vec<Vec<u16>>) = (removed.iter().map(stripped).collect(), added.iter().map(stripped).collect());
    let longer_added = a > d;
    let mut best = (0, -1.0);
    for offset in 0..=surplus {
        let (shift_removed, shift_added) = if longer_added { (0, offset) } else { (offset, 0) };
        let score: f64 = (0..pairs).map(|k| similarity(&removed[k + shift_removed], &added[k + shift_added])).sum();
        if offset == 0 {
            best = (0, score + pairs as f64 * MIN_GAIN);
        } else if score > best.1 {
            best = (offset, score);
        }
    }
    let offset = best.0;
    if offset == 0 {
        return vec![(d, a)];
    }
    let blocks = if longer_added { [(0, offset), (pairs, pairs), (0, a - pairs - offset)] } else { [(offset, 0), (pairs, pairs), (d - pairs - offset, 0)] };
    blocks.into_iter().filter(|&(d, a)| d + a > 0).collect()
}

/// A line's text without whitespace, in UTF-16 code units, as Pierre compares lines
/// (`line.replace(/\s+/g, "")`).
fn stripped(l: &Line) -> Vec<u16> {
    let mut out = Vec::with_capacity(l.text.len());
    let mut buf = [0; 2];
    for c in l.text.chars().filter(|&c| !js_space(c)) {
        out.extend_from_slice(c.encode_utf16(&mut buf));
    }
    out
}

/// Whether JavaScript's `\s` matches `c`: Unicode white space less U+0085, plus U+FEFF.
fn js_space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

/// How alike two lines are, 0 to 1: the share of the longer one their common start and end
/// cover (`lineSimilarity`).
fn similarity(a: &[u16], b: &[u16]) -> f64 {
    if a == b {
        return 1.0;
    }
    let (max, min) = (a.len().max(b.len()), a.len().min(b.len()));
    if min == 0 {
        return 0.0;
    }
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suffix = a.iter().rev().zip(b.iter().rev()).take(min - prefix).take_while(|(x, y)| x == y).count();
    (prefix + suffix) as f64 / max as f64
}
