//! Paragraph boundary keying and prefix/suffix key reuse across edits and streaming appends.

use std::hash::{Hash, Hasher};

use super::*;

/// A paragraph found in a text: its bytes, direction, key and isolated pills, and for a run of
/// waiting lines its height, width and line count.
pub(super) type Found = (usize, usize, Flow, u64, Box<[(u32, u32)]>, Option<(f32, f32, u32)>);

/// What lets lines wait as runs while they're keyed: printable ASCII's measures at the field's
/// width, and whether the lines are an edit's (whose ends are shaped at once, as the caret's
/// window) or a waiting run's keyed again at another width.
#[derive(Clone, Copy)]
pub(super) struct Runs<'a> {
    pub(super) ascii: &'a AsciiLine,
    pub(super) max_width: Option<f32>,
    pub(super) edit: bool,
    pub(super) eager: (usize, usize),
}

/// Start byte of the line after one ending at `end` (`\r\n` or `\n`).
pub(super) fn next_line_start(b: &[u8], end: usize) -> usize {
    if b.get(end) == Some(&b'\r') { end + 2 } else { end + 1 }
}

/// `\n` bytes in `s`, counted eight at a time.
pub(super) fn count_newlines(s: &[u8]) -> usize {
    let mut chunks = s.chunks_exact(8);
    let mut n = 0usize;
    for c in &mut chunks {
        let w = u64::from_le_bytes(c.try_into().unwrap()) ^ 0x0a0a_0a0a_0a0a_0a0a;
        n += ((w.wrapping_sub(0x0101_0101_0101_0101) & !w & 0x8080_8080_8080_8080).count_ones()) as usize;
    }
    n + chunks.remainder().iter().filter(|&&b| b == b'\n').count()
}

/// Keys the paragraphs of `spec`'s content bytes `a..z` (a paragraph's start to a paragraph's
/// end) onto `found`. A paragraph with no strong character reads the way the one before it
/// does, the first `start_flow`; with `forced`, every paragraph reads that way. With `runs`, more
/// than [`LAZY_MIN`] lines of an edit (or any number of a run keyed again) put the plain ASCII
/// lines that fit on one line, away from the edit's ends, in runs: one record per run, keyed by
/// its bytes, its height the sum of its lines'.
#[allow(clippy::too_many_arguments)]
pub(super) fn key_paragraphs(spec: &TextInput<'_>, layout_spans: &[Span], style_key: u64, disjoint_spans: bool, a: usize, z: usize, start_flow: Flow, forced: Option<Flow>, runs: Option<Runs<'_>>, scan: &mut Option<LineScan>, found: &mut Vec<Found>) {
    // The other width's keying of these bytes, this layout pass, read them already.
    let key = [spec.content.as_ptr() as usize, spec.content.len(), a, z];
    let cache = scan.take_if(|s| runs.is_some() && s.key == key);
    let lines = match (&cache, runs) {
        (Some(c), _) => c.ends.len(),
        (None, Some(_)) => count_newlines(&spec.content.as_bytes()[a..z]) + 1,
        (None, None) => 0,
    };
    let runs = runs.filter(|r| !r.edit || lines > LAZY_MIN || r.ascii.narrow(r.max_width));
    let mut record = (runs.is_some() && cache.is_none()).then(|| LineScan { key, ends: Vec::with_capacity(lines), flags: Vec::with_capacity(lines) });
    let mut split = spec.content[a..z].split('\n');
    // The run being gathered: its first byte, last byte, height, width, lines and last flow.
    let mut run: Option<(usize, usize, f32, f32, u32, Flow)> = None;
    let close = |run: &mut Option<(usize, usize, f32, f32, u32, Flow)>, found: &mut Vec<Found>| {
        if let Some((a, b, height, w, n, flow)) = run.take() {
            let mut h = hash::KeyHasher::default();
            (&spec.content[a..b], style_key, flow == Flow::Rtl, n, "run").hash(&mut h);
            let run_spans: Vec<Span> = spans_in(layout_spans, a, b, &[], disjoint_spans).collect();
            for s in span::layout_spans(&run_spans) {
                s.hash_layout(&mut h);
            }
            found.push((a, b, flow, h.finish(), Box::default(), Some((height, w, n))));
        }
    };
    let mut start = a;
    for j in 0.. {
        let (raw, known) = match &cache {
            Some(c) if j < c.ends.len() => (&spec.content[start..a + c.ends[j] as usize], Some(c.flags[j])),
            Some(_) => break,
            None => match split.next() {
                Some(words) => (words, None),
                None => break,
            },
        };
        let raw_len = raw.len();
        let words = if start + raw_len < z { raw.strip_suffix('\r').unwrap_or(raw) } else { raw };
        let (a, b, k) = (start, start + words.len(), found.len());
        start += raw_len + 1;
        let last_flow = run.map(|r| r.5).or_else(|| found.last().map(|f| f.2)).unwrap_or(start_flow);
        if let Some(r) = runs {
            // A measure of the narrowest width is never shown: none of its lines needs shaping.
            let near = r.edit && !r.ascii.narrow(r.max_width) && (j < r.eager.0 || j + r.eager.1 >= lines);
            let floor = run.map_or(0.0, |r| r.3);
            let printable = known.map(|f| f & PRINTABLE != 0);
            // Printable ASCII: its letters are its only strong characters, all left to right.
            let letter = || known.map_or_else(|| words.bytes().any(|b| b.is_ascii_alphabetic()), |f| f & LETTER != 0);
            let flow = forced.or_else(|| letter().then_some(Flow::Ltr)).unwrap_or(last_flow);
            let fits = if near || words.is_empty() || printable == Some(false) || (spec.by_line && flow == Flow::Rtl) {
                None
            } else if r.ascii.narrow(r.max_width) {
                r.ascii.narrowest(words, floor)
            } else {
                r.ascii.one_line(words, r.max_width, floor, printable == Some(true))
            };
            if let Some(rec) = &mut record {
                let shown = fits.is_some() || (!words.is_empty() && lazy::printable(words.as_bytes()));
                rec.ends.push((start - 1 - key[2]) as u32);
                rec.flags.push(if shown { PRINTABLE | if letter() { LETTER } else { 0 } } else { 0 });
            }
            if let Some((height, w)) = fits {
                match &mut run {
                    Some(run) => { run.1 = b; run.2 += height; run.3 = run.3.max(w); run.4 += 1; run.5 = flow; }
                    None => run = Some((a, b, height, w, 1, flow)),
                }
                // A run holds at most RUN_LINES: an edit inside one keys only its run again.
                if run.is_some_and(|r| r.4 as usize >= RUN_LINES) {
                    close(&mut run, found);
                }
                continue;
            }
            close(&mut run, found);
        }
        let isolates: Box<[(u32, u32)]> = spec.pills.iter().filter(|p| inside(p, a, b)).map(|p| (p.start - a as u32, p.end - a as u32)).collect();
        let flow = forced.or_else(|| flow_of(words, &isolates)).unwrap_or(last_flow);
        let mut h = hash::KeyHasher::default();
        (words, style_key, flow == Flow::Rtl, spec.blocks.get(k).copied().unwrap_or(Block::Paragraph)).hash(&mut h);
        let line_spans: Vec<Span> = spans_in(layout_spans, a, b, &isolates, disjoint_spans).collect();
        for s in span::layout_spans(&line_spans) {
            s.hash_layout(&mut h);
        }
        for p in pills_in(spec.pills, a, b, &isolates) {
            (p.start, p.end, p.icon.is_some(), p.bare.as_ref().map(|b| (b.size.to_bits(), b.gap.to_bits()))).hash(&mut h);
        }
        found.push((a, b, flow, h.finish(), isolates, None));
    }
    close(&mut run, found);
    *scan = record.or(cache);
}

/// A line of a [`LineScan`] is printable ASCII (not empty), and holds a letter.
const PRINTABLE: u8 = 1;
const LETTER: u8 = 2;

/// A text's lines between two of its bytes as one width's keying read them (`key_paragraphs`):
/// where each ends and what it holds, so the other width's keying of a long paste, in the same
/// layout pass, reads these instead of the bytes. `key` is the text's address and length and the
/// two bytes; `TextSystem::forget_diff` drops it when a pass begins.
pub(crate) struct LineScan {
    key: [usize; 4],
    ends: Vec<u32>,
    flags: Vec<u8>,
}

/// The paragraphs an edit left whole, with their keys: those before it (`head`) and after it
/// (`tail`, moved to their new bytes), and the direction the paragraph before the tail had.
pub(super) struct Reuse {
    pub(super) head: Vec<Found>,
    pub(super) tail: Vec<Found>,
    pub(super) entering: Flow,
}

/// `prev`'s paragraphs that `content` keeps byte for byte, newline included, when `prev` was a
/// plain text in the same style and starting direction. The edit is what lies between the
/// texts' common start and common end.
pub(super) fn reuse_keys(prev: &ParagraphLayout, content: &str, style_key: u64, start_flow: Flow, last: &mut Option<([usize; 4], usize, usize)>) -> Option<Reuse> {
    let keyed = prev.keyed.as_ref().filter(|k| k.style_key == style_key && k.start_flow == start_flow)?;
    let (old, new) = (keyed.content.as_bytes(), content.as_bytes());
    // The same two texts as the last comparison this commit (both are alive till it ends).
    let texts = [old.as_ptr() as usize, old.len(), new.as_ptr() as usize, new.len()];
    let (prefix, suffix) = match *last {
        Some((t, p, s)) if t == texts => (p, s),
        _ => {
            let prefix = common_start(old, new);
            let suffix = common_end(old, new).min(old.len().min(new.len()) - prefix);
            *last = Some((texts, prefix, suffix));
            (prefix, suffix)
        }
    };
    let ps = &prev.paragraphs;
    // Before the edit: its newline too. After it: the newline before it too.
    let h = ps.partition_point(|p| next_line_start(old, p.end) <= prefix);
    let t = ps.partition_point(|p| p.start + suffix <= old.len());
    let moved = |at: usize| at + new.len() - old.len();
    Some(Reuse {
        head: ps[..h].iter().map(|p| (p.start, p.end, p.flow, p.key, Box::default(), p.run.then_some((p.height, p.widths[0], p.lines)))).collect(),
        tail: ps[t..].iter().map(|p| (moved(p.start), moved(p.end), p.flow, p.key, Box::default(), p.run.then_some((p.height, p.widths[0], p.lines)))).collect(),
        entering: t.checked_sub(1).map_or(start_flow, |i| ps[i].flow),
    })
}

/// How many bytes two texts share from their start: compared 4 KB at a time (one memcmp each;
/// a 1.7 MB paste in a few dozen), then byte by byte inside the first block that differs.
pub fn common_start(a: &[u8], b: &[u8]) -> usize {
    let n = a.len().min(b.len());
    let mut at = 0;
    while at + 4096 <= n && a[at..at + 4096] == b[at..at + 4096] { at += 4096; }
    at + a[at..n].iter().zip(&b[at..n]).take_while(|(x, y)| x == y).count()
}

/// How many bytes two texts share at their end, as `common_start` counts.
pub fn common_end(a: &[u8], b: &[u8]) -> usize {
    let n = a.len().min(b.len());
    let mut at = 0;
    while at + 4096 <= n && a[a.len() - at - 4096..a.len() - at] == b[b.len() - at - 4096..b.len() - at] { at += 4096; }
    at + a[..a.len() - at].iter().rev().zip(b[..b.len() - at].iter().rev()).take(n - at).take_while(|(x, y)| x == y).count()
}

/// A paragraph's direction: its first strong character's outside its pills, or None with none.
pub(super) fn flow_of(words: &str, isolates: &[(u32, u32)]) -> Option<Flow> {
    let mut at = 0;
    for &(a, b) in isolates.iter().chain([&(words.len() as u32, words.len() as u32)]) {
        if let Some(d) = words.get(at..a as usize).and_then(Direction::of) {
            return Some(if d == Direction::Rtl { Flow::Rtl } else { Flow::Ltr });
        }
        at = at.max(b as usize);
    }
    None
}

/// `words` with each isolated pill's label between FSI and PDI.
pub(super) fn with_isolates(words: &str, isolates: &[(u32, u32)]) -> String {
    let mut out = String::with_capacity(words.len() + 2 * ISOLATE * isolates.len());
    let mut at = 0;
    for &(a, b) in isolates {
        out.push_str(&words[at..a as usize]);
        out.push('\u{2068}');
        out.push_str(&words[a as usize..b as usize]);
        out.push('\u{2069}');
        at = b as usize;
    }
    out.push_str(&words[at..]);
    out
}
