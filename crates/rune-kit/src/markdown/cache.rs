//! Parsed documents kept across commits, and growing a stream's parse from its last block.

use super::*;

/// Estimated bytes of parsed documents kept beyond those on screen (see [`budget`]).
const CACHE_BYTES: usize = 16 << 20;

pub(super) type Doc = (Rc<Vec<Block>>, Rc<[Source]>, Rc<[Option<(bool, usize)>]>, Rc<[usize]>, Rc<[BlockMap]>);

#[derive(Clone)]
pub(super) struct Parsed {
    pub(super) doc: Doc,
    pub(super) cuts: Rc<[Cut]>,
    /// When `src` ended inside an active [`BlockMap::DeepTail`] at EOF, holds
    /// `(cut_start, min_indent_cols)` so streamed tokens that continue that deep tail update only
    /// the trailing `DeepTail` block without re-parsing the whole prefix.
    pub(super) eof_deep_tail: Option<(usize, usize)>,
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

fn detect_eof_deep_tail(src: &str, blocks: &[Block], maps: &[BlockMap]) -> Option<(usize, usize)> {
    let mut b = blocks.last()?;
    let mut m = maps.last()?;
    loop {
        match (b, m) {
            (Block::List { items, .. }, BlockMap::List(imaps)) => {
                b = items.last()?.last()?;
                m = imaps.last()?.last()?;
            }
            (Block::Quote(inner), BlockMap::Quote(imaps)) => {
                b = inner.last()?;
                m = imaps.last()?;
            }
            (Block::Para(_), BlockMap::DeepTail { map, .. }) => {
                let first_src = &map.0.first()?.0;
                let last_src = &map.0.last()?.0;
                if src[last_src.end..].bytes().any(|c| !c.is_ascii_whitespace()) {
                    return None;
                }
                let cut_start = src[..first_src.start].rfind('\n').map_or(0, |i| i + 1);
                let nl = src[cut_start..].find('\n').map_or(src.len(), |d| cut_start + d);
                let (cols, blank, fence) = super::parser::line_indent_cols(&src.as_bytes()[cut_start..nl]);
                return (!blank && !fence && cols >= 4).then_some((cut_start, cols));
            }
            _ => return None,
        }
    }
}

fn replace_eof_deep_tail(blocks: &mut [Block], maps: &mut [BlockMap], nb: Block, nm: BlockMap) -> bool {
    let (Some(mut b), Some(mut m)) = (blocks.last_mut(), maps.last_mut()) else { return false };
    loop {
        match (b, m) {
            (Block::List { items, .. }, BlockMap::List(imaps)) => {
                let (Some(ib), Some(im)) = (items.last_mut().and_then(|v| v.last_mut()), imaps.last_mut().and_then(|v| v.last_mut())) else {
                    return false;
                };
                b = ib;
                m = im;
            }
            (Block::Quote(inner), BlockMap::Quote(imaps)) => {
                let (Some(ib), Some(im)) = (inner.last_mut(), imaps.last_mut()) else { return false };
                b = ib;
                m = im;
            }
            (b_leaf @ Block::Para(_), m_leaf @ BlockMap::DeepTail { .. }) => {
                *b_leaf = nb;
                *m_leaf = nm;
                return true;
            }
            _ => return false,
        }
    }
}

fn extend_eof_deep_tail(base: &Parsed, base_len: usize, src: &str) -> Option<Parsed> {
    let (cut_start, min_cols) = base.eof_deep_tail?;
    if cut_start > base_len || base_len > src.len() {
        return None;
    }
    let check_from = src[..base_len].rfind('\n').map_or(cut_start, |i| (i + 1).max(cut_start));
    if src[check_from..].contains("]:") {
        return None;
    }
    let bytes = src.as_bytes();
    let mut pos = check_from;
    while pos < bytes.len() {
        if let Some(rel_nl) = bytes[pos..].iter().position(|&b| b == b'\n') {
            let nl = pos + rel_nl;
            let (cols, blank, fence) = super::parser::line_indent_cols(&bytes[pos..nl]);
            if !blank && (fence || cols < min_cols) {
                return None;
            }
            pos = nl + 1;
        } else {
            let (cols, blank, fence) = super::parser::line_indent_cols(&bytes[pos..]);
            if !blank && (fence || cols < min_cols) {
                return None;
            }
            break;
        }
    }
    let (nb, nm) = super::parser::build_deep_tail(src, cut_start..src.len());
    let mut new_blocks = (*base.doc.0).clone();
    let mut new_maps = base.doc.4.to_vec();
    if !replace_eof_deep_tail(&mut new_blocks, &mut new_maps, nb, nm) {
        return None;
    }
    Some(Parsed {
        doc: (Rc::new(new_blocks), base.doc.1.clone(), base.doc.2.clone(), base.doc.3.clone(), new_maps.into()),
        cuts: base.cuts.clone(),
        eof_deep_tail: Some((cut_start, min_cols)),
    })
}

/// [`parse`] memoised by source on this thread. Every document the last two commits drew stays
/// cached however many there are; past a memory budget the least recently drawn of the rest go.
pub fn parse_cached(src: &str) -> Rc<Vec<Block>> {
    doc_cached(src).0
}

pub(super) fn doc_cached(src: &str) -> Doc {
    CACHE.with_borrow_mut(|c| cached(c, src, parse_doc, false))
}

/// [`doc_cached`] for a source still arriving.
pub(super) fn stream_cached(src: &str) -> Doc {
    STREAMS.with_borrow_mut(|c| cached(c, src, parse_streaming, true))
}

fn try_extend(base: &Parsed, base_len: usize, src: &str, parse: fn(&str, bool) -> ParsedDoc, streaming: bool) -> Option<Parsed> {
    if streaming && let Some(extended) = extend_eof_deep_tail(base, base_len, src) {
        return Some(extended);
    }
    extend(base, base_len, src, parse)
}

fn cached(c: &mut Cache, src: &str, parse: fn(&str, bool) -> ParsedDoc, streaming: bool) -> Doc {
    if let Some(hit) = c.docs.get(src) {
        return hit.doc.clone();
    }
    let grown = c.last.clone().filter(|last| src.len() > last.len() && src.starts_with(&**last));
    let parsed = grown.as_ref().and_then(|last| try_extend(c.docs.peek(&**last)?, last.len(), src, parse, streaming)).unwrap_or_else(|| {
        #[cfg(test)]
        PARSES.with(|p| p.set((p.get().0 + 1, p.get().1)));
        let (blocks, sources, tasks, cuts, starts, maps) = parse(src, true);
        let eof_deep_tail = detect_eof_deep_tail(src, &blocks, &maps);
        Parsed { doc: (Rc::new(blocks), sources.into(), tasks.into(), starts.into(), maps.into()), cuts: cuts.into(), eof_deep_tail }
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
    let maps: Rc<[BlockMap]> = base.doc.4[..cut.blocks].iter().cloned()
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
    let eof_deep_tail = detect_eof_deep_tail(&src, &blocks, &maps);
    Some(Parsed { doc: (Rc::new(blocks), sources, tasks, starts, maps), cuts, eof_deep_tail })
}
