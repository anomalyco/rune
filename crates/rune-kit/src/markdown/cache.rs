//! Parsed documents kept across commits, and growing a stream's parse from its last block.

use super::*;

/// Estimated bytes of parsed documents kept beyond those on screen (see [`budget`]).
const CACHE_BYTES: usize = 16 << 20;

/// Maximum flattened sibling lines retained inside a single container block after crossing
/// Blink's 511-element nesting bound, so a pathological streamed reply with thousands of
/// 256-deep items never forces megabyte re-parses or multi-thousand-sibling relayouts per token.
const MAX_FLATTENED_TAIL_LINES: usize = 64;

pub(super) type Doc = (Rc<Vec<Block>>, Rc<[Source]>, Rc<[Option<(bool, usize)>]>, Rc<[usize]>, Rc<[BlockMap]>);

#[derive(Clone)]
pub(super) struct Parsed {
    pub(super) doc: Doc,
    pub(super) cuts: Rc<[Cut]>,
    /// When `src` ended inside a capped past-bound container tail (`cut_end == src.len()`),
    /// the byte offset of the last verified past-bound line start so appended tokens that stay
    /// inside that tail reuse `doc` in `O(token_len)`.
    pub(super) capped_tail_from: Option<usize>,
}

pub(super) struct Cache {
    pub(super) docs: Budget<Rc<str>, Parsed>,
    /// The source parsed last, which a stream's next string extends.
    pub(super) last: Option<Rc<str>>,
}

impl Cache {
    fn new(bytes: usize) -> Self {
        Cache { docs: Budget::new(bytes), last: None }
    }
}

thread_local! {
    pub(super) static CACHE: RefCell<Cache> = RefCell::new(Cache::new(CACHE_BYTES));
    /// Sources drawn while still arriving ([`Markdown::streaming`]), parsed with their tail
    /// settled (see [`super::stream`]); kept apart, since the same string parses differently.
    pub(super) static STREAMS: RefCell<Cache> = RefCell::new(Cache::new(CACHE_BYTES / 4));
}

#[cfg(test)]
thread_local! {
    /// (whole documents parsed, streams parsed from their last block).
    pub(super) static PARSES: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

/// Returns `(is_blank, is_root, is_past_bound, prefix_only)` for a single line (without `\n`).
#[inline]
fn classify_line(line: &[u8]) -> (bool, bool, bool, bool) {
    let mut i = 0usize;
    let mut cols = 0usize;
    let mut quotes = 0usize;
    while i < line.len() {
        match line[i] {
            b' ' => {
                cols += 1;
                i += 1;
            }
            b'\t' => {
                cols += 4;
                i += 1;
            }
            b'>' => {
                quotes += 1;
                i += 1;
                if i < line.len() && line[i] == b' ' {
                    i += 1;
                }
            }
            b'\r' => {
                i += 1;
            }
            _ => break,
        }
    }
    let is_blank = i == line.len();
    let prefix_only = is_blank && (cols > 0 || quotes > 0);
    let is_root = !is_blank && cols == 0 && quotes == 0;
    let max_quotes = if cfg!(target_arch = "wasm32") { 385 } else { 510 };
    let is_past_bound = !is_blank && ((cols >= 2 && cols + quotes >= 510) || quotes > max_quotes);
    (is_blank, is_root, is_past_bound, prefix_only)
}

/// Scans `src` (> 32 KiB) for a continuous run of > [`MAX_FLATTENED_TAIL_LINES`] lines nested
/// past Blink's 511-element bound, returning `Some((cut_start, cut_end))` for the first excess
/// run (`cut_end == src.len()` when the run reaches the end of `src`, or the byte start of the
/// next root-level line otherwise).
pub(super) fn find_deep_tail_cut(src: &str) -> Option<(usize, usize)> {
    if src.len() <= 32 * 1024 {
        return None;
    }
    let bytes = src.as_bytes();
    let mut pos = 0usize;
    let mut deep_run = 0usize;
    let mut cut_start: Option<usize> = None;

    while pos < bytes.len() {
        let nl = bytes[pos..].iter().position(|&b| b == b'\n').map_or(bytes.len(), |d| pos + d);
        let line = &bytes[pos..nl];
        let (is_blank, is_root, is_past_bound, prefix_only) = classify_line(line);
        if let Some(cs) = cut_start {
            if is_root {
                return Some((cs, pos));
            }
        } else if is_past_bound {
            deep_run += 1;
            if deep_run > MAX_FLATTENED_TAIL_LINES {
                cut_start = Some(pos);
            }
        } else if !is_blank && !prefix_only {
            deep_run = 0;
        }
        pos = if nl < bytes.len() { nl + 1 } else { bytes.len() };
    }
    cut_start.map(|cs| (cs, bytes.len()))
}

/// When `base` ended inside an active capped deep tail at `from`, checks only `src[from..]` to
/// verify no root-level line has started yet, returning the updated `from` watermark in
/// `O(src.len() - from)`.
fn extend_capped_tail(from: usize, src: &str) -> Option<usize> {
    if from > src.len() {
        return None;
    }
    let bytes = src.as_bytes();
    let mut pos = from;
    let mut next_from = from;
    while pos < bytes.len() {
        if let Some(rel_nl) = bytes[pos..].iter().position(|&b| b == b'\n') {
            let nl = pos + rel_nl;
            let (is_blank, _, is_past_bound, _) = classify_line(&bytes[pos..nl]);
            if !is_blank && !is_past_bound {
                return None;
            }
            if is_past_bound {
                next_from = nl + 1;
            }
            pos = nl + 1;
        } else {
            // Trailing partial line with no newline yet: safe while blank/prefix-only or already past bound.
            let (is_blank, _, is_past_bound, _) = classify_line(&bytes[pos..]);
            if !is_blank && !is_past_bound {
                return None;
            }
            break;
        }
    }
    Some(next_from)
}

fn tail_watermark(src: &str) -> Option<usize> {
    let (_, cut_end) = find_deep_tail_cut(src)?;
    if cut_end < src.len() {
        return None;
    }
    Some(src.rfind('\n').map_or(0, |i| i + 1))
}

/// [`parse`] memoised by source on this thread. Every document the last two commits drew stays
/// cached however many there are; past a memory budget the least recently drawn of the rest go.
pub fn parse_cached(src: &str) -> Rc<Vec<Block>> {
    doc_cached(src).0
}

pub(super) fn doc_cached(src: &str) -> Doc {
    CACHE.with_borrow_mut(|c| cached(c, src, parse_doc))
}

/// [`doc_cached`] for a source still arriving.
pub(super) fn stream_cached(src: &str) -> Doc {
    STREAMS.with_borrow_mut(|c| cached(c, src, parse_streaming))
}

fn try_extend(base: &Parsed, base_len: usize, src: &str, parse: fn(&str, bool) -> ParsedDoc) -> Option<Parsed> {
    if let Some(from) = base.capped_tail_from
        && let Some(next_from) = extend_capped_tail(from, src)
    {
        return Some(Parsed {
            doc: base.doc.clone(),
            cuts: base.cuts.clone(),
            capped_tail_from: Some(next_from),
        });
    }
    extend(base, base_len, src, parse)
}

fn cached(c: &mut Cache, src: &str, parse: fn(&str, bool) -> ParsedDoc) -> Doc {
    if let Some(hit) = c.docs.get(src) {
        return hit.doc.clone();
    }
    let grown = c.last.clone().filter(|last| src.len() > last.len() && src.starts_with(&**last));
    let parsed = grown.as_ref().and_then(|last| try_extend(c.docs.peek(&**last)?, last.len(), src, parse)).unwrap_or_else(|| {
        #[cfg(test)]
        PARSES.with(|p| p.set((p.get().0 + 1, p.get().1)));
        let (blocks, sources, tasks, cuts, starts, maps) = parse(src, true);
        Parsed { doc: (Rc::new(blocks), sources.into(), tasks.into(), starts.into(), maps.into()), cuts: cuts.into(), capped_tail_from: tail_watermark(src) }
    });
    // A stream's earlier string is dead once it grows, unless something drew it this
    // commit; left in, a long stream would fill the budget with its prefixes.
    if let Some(last) = grown
        && !c.docs.used_now(&*last)
    {
        c.docs.remove(&*last);
    }
    let key: Rc<str> = src.into();
    // Parsed text is about as long as its source, plus each block's bookkeeping.
    let bytes = 2 * src.len().min(128 * 1024) + 64 * parsed.cuts.len() + 128;
    c.docs.insert(key.clone(), parsed.clone(), bytes);
    c.last = Some(key);
    parsed.doc
}

/// The parse of `src`, which extends the `base_len` bytes `base` parsed, reusing every
/// top-level block before `base`'s last: appended text can change only the last block or add
/// blocks after it. Whether a line starts a block can depend on the rest of that line (`|`
/// ending a table that `| 1 |` would continue), so the reparse starts at the last block that
/// starts on a finished line. A reference definition (`[x]: url`) can change links anywhere,
/// so a source that may hold one parses whole. A stream's parse may have left its last lines
/// out, so a cut is only ever at or before `base_len`.
fn extend(base: &Parsed, base_len: usize, src: &str, parse: fn(&str, bool) -> ParsedDoc) -> Option<Parsed> {
    let i = base.cuts.iter().rposition(|c| c.at <= base_len && src[c.at..base_len].contains('\n'))?;
    let cut = base.cuts[i];
    if cut.at == 0 || cut.blocks >= base.doc.0.len() || src.contains("]:") {
        return None;
    }
    #[cfg(test)]
    PARSES.with(|p| p.set((p.get().0, p.get().1 + 1)));
    let tail_src = &src[cut.at..];
    let (tail, sources, tasks, cuts, starts, maps) = parse(tail_src, true);
    let mut blocks = base.doc.0[..cut.blocks].to_vec();
    blocks.extend(tail);
    let sources: Rc<[Source]> = base.doc.1[..cut.sources].iter().cloned().chain(sources).collect();
    let tasks = base.doc.2[..cut.tasks].iter().copied()
        .chain(tasks.into_iter().map(|t| t.map(|(checked, at)| (checked, at + cut.at)))).collect();
    let starts = base.doc.3[..cut.blocks].iter().copied().chain(starts.into_iter().map(|at| at + cut.at)).collect();
    let maps = base.doc.4[..cut.blocks].iter().cloned()
        .chain(maps.into_iter().map(|m| m.shift_src(cut.at))).collect();
    let cuts = base.cuts[..i]
        .iter()
        .copied()
        .chain(cuts.into_iter().map(|c| Cut {
            at: c.at + cut.at,
            blocks: c.blocks + cut.blocks,
            sources: c.sources + cut.sources,
            tasks: c.tasks + cut.tasks,
        }))
        .collect();
    let capped_tail_from = tail_watermark(tail_src).map(|off| cut.at + off);
    Some(Parsed { doc: (Rc::new(blocks), sources, tasks, starts, maps), cuts, capped_tail_from })
}
