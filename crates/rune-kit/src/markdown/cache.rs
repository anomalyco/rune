//! Parsed documents kept across commits, and growing a stream's parse from its last block.

use super::*;

/// Estimated bytes of parsed documents kept beyond those on screen (see [`budget`]).
const CACHE_BYTES: usize = 16 << 20;

pub(super) type Doc = (Rc<Vec<Block>>, Rc<[Source]>, Rc<[Option<(bool, usize)>]>, Rc<[usize]>, Rc<[BlockMap]>);

#[derive(Clone)]
pub(super) struct Parsed {
    pub(super) doc: Doc,
    pub(super) cuts: Rc<[Cut]>,
}

pub(super) struct Cache {
    pub(super) docs: Budget<Rc<str>, Parsed>,
    /// The source parsed last, which a stream's next string extends.
    pub(super) last: Option<Rc<str>>,
}

thread_local! {
    pub(super) static CACHE: RefCell<Cache> = RefCell::new(Cache { docs: Budget::new(CACHE_BYTES), last: None });
}

#[cfg(test)]
thread_local! {
    /// (whole documents parsed, streams parsed from their last block).
    pub(super) static PARSES: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

/// [`parse`] memoised by source on this thread. Every document the last two commits drew stays
/// cached however many there are; past a memory budget the least recently drawn of the rest go.
pub fn parse_cached(src: &str) -> Rc<Vec<Block>> {
    doc_cached(src).0
}

pub(super) fn doc_cached(src: &str) -> Doc {
    CACHE.with_borrow_mut(|c| {
        if let Some(hit) = c.docs.get(src) {
            return hit.doc.clone();
        }
        let grown = c.last.clone().filter(|last| src.len() > last.len() && src.starts_with(&**last));
        let parsed = grown.as_ref().and_then(|last| extend(c.docs.peek(&**last)?, last.len(), src)).unwrap_or_else(|| {
            #[cfg(test)]
            PARSES.with(|p| p.set((p.get().0 + 1, p.get().1)));
            let (blocks, sources, tasks, cuts, starts, maps) = parse_doc(src, true);
            Parsed { doc: (Rc::new(blocks), sources.into(), tasks.into(), starts.into(), maps.into()), cuts: cuts.into() }
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
        let bytes = 2 * src.len() + 64 * parsed.cuts.len() + 128;
        c.docs.insert(key.clone(), parsed.clone(), bytes);
        c.last = Some(key);
        parsed.doc
    })
}

/// The parse of `src`, which extends the `base_len` bytes `base` parsed, reusing every
/// top-level block before `base`'s last: appended text can change only the last block or add
/// blocks after it. Whether a line starts a block can depend on the rest of that line (`|`
/// ending a table that `| 1 |` would continue), so the reparse starts at the last block that
/// starts on a finished line. A reference definition (`[x]: url`) can change links anywhere,
/// so a source that may hold one parses whole.
fn extend(base: &Parsed, base_len: usize, src: &str) -> Option<Parsed> {
    let i = base.cuts.iter().rposition(|c| src[c.at..base_len].contains('\n'))?;
    let cut = base.cuts[i];
    if cut.at == 0 || cut.blocks >= base.doc.0.len() || src.contains("]:") {
        return None;
    }
    #[cfg(test)]
    PARSES.with(|p| p.set((p.get().0, p.get().1 + 1)));
    let (tail, sources, tasks, cuts, starts, maps) = parse_doc(&src[cut.at..], true);
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
    Some(Parsed { doc: (Rc::new(blocks), sources, tasks, starts, maps), cuts })
}
