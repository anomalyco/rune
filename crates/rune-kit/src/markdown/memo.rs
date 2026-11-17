//! Memoising settled Markdown subtrees (`blocks_in`, list item bodies, code cards, and table
//! rows) across streaming commits so appending to the open trailing block rebuilds and lays out
//! only what is still arriving.
//!
//! Each parsed block, list item and table row carries a pre-computed 128-bit content digest
//! (`PreBlock` / `PreItem` / `PreRow`), seeded per process from [`RandomState`] and carried across
//! [`super::cache`]'s `extend`, so settled blocks do not re-hash their text on later tokens.

use std::collections::hash_map::{DefaultHasher, RandomState};
use std::hash::{BuildHasher, Hash, Hasher};
use std::sync::LazyLock;

use super::parser::{BlockMap, Source, SpanMap};
use super::render::{Flow, Lead, Renderer};
use super::*;

static SEEDS: LazyLock<(RandomState, RandomState)> =
    LazyLock::new(|| (RandomState::new(), RandomState::new()));

/// Two independent 128-bit-keyed SipHash-1-3 lanes fed directly from the hashed inputs, with no
/// intermediate 64-bit reduction.
pub(super) struct PairHasher {
    a: DefaultHasher,
    b: DefaultHasher,
}

impl PairHasher {
    pub(super) fn new() -> Self {
        let (sa, sb) = &*SEEDS;
        Self { a: sa.build_hasher(), b: sb.build_hasher() }
    }

    pub(super) fn finish128(&self) -> (u64, u64) {
        (self.a.finish(), self.b.finish())
    }
}

impl Hasher for PairHasher {
    fn finish(&self) -> u64 {
        self.a.finish()
    }
    fn write(&mut self, bytes: &[u8]) {
        self.a.write(bytes);
        self.b.write(bytes);
    }
}

struct HashFmt<'a, H>(&'a mut H);

impl<H: Hasher> std::fmt::Write for HashFmt<'_, H> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0.write(s.as_bytes());
        Ok(())
    }
}

pub(super) const HAS_CODE: u8 = 1 << 0;
pub(super) const HAS_MERMAID: u8 = 1 << 1;
pub(super) const HAS_IMAGE: u8 = 1 << 2;
pub(super) const HAS_QUOTE: u8 = 1 << 3;
pub(super) const HAS_OPEN_TABLE: u8 = 1 << 4;
pub(super) const HAS_INLINE_CODE: u8 = 1 << 5;

#[derive(Clone)]
pub(super) struct PreRow {
    pub(super) digest: (u64, u64),
    pub(super) flags: u8,
    pub(super) links: Box<[Rc<str>]>,
}

#[derive(Clone)]
pub(super) struct PreTable {
    pub(super) head: PreRow,
    pub(super) rows: Box<[PreRow]>,
}

#[derive(Clone)]
pub(super) struct PreItem {
    pub(super) digest: (u64, u64),
    pub(super) flags: u8,
    pub(super) links: Box<[Rc<str>]>,
    pub(super) src_n: u32,
    pub(super) task_n: u32,
    pub(super) inner: Box<[PreBlock]>,
}

#[derive(Clone)]
pub(super) struct PreBlock {
    pub(super) digest: (u64, u64),
    pub(super) flags: u8,
    pub(super) links: Box<[Rc<str>]>,
    pub(super) src_n: u32,
    pub(super) task_n: u32,
    pub(super) items: Box<[PreItem]>,
    pub(super) inner: Box<[PreBlock]>,
    pub(super) table: Option<Box<PreTable>>,
}

pub(super) fn source_count(b: &Block) -> usize {
    match b {
        Block::Code(_) | Block::Mermaid(_) => 1,
        Block::List { items, .. } => items.iter().flatten().map(source_count).sum(),
        Block::Quote(inner) => inner.iter().map(source_count).sum(),
        _ => 0,
    }
}

pub(super) fn task_count(b: &Block) -> usize {
    match b {
        Block::List { items, .. } => items.len() + items.iter().flatten().map(task_count).sum::<usize>(),
        Block::Quote(inner) => inner.iter().map(task_count).sum(),
        _ => 0,
    }
}

fn scan_inline(inl: &Inline, h: &mut PairHasher, flags: &mut u8, links: &mut Vec<Rc<str>>) {
    (0u8, inl).hash(h);
    if inl.spans.iter().any(|s| matches!(s.2, Style::Code)) {
        *flags |= HAS_INLINE_CODE;
    }
    for (_, _, url) in &inl.links {
        links.push(Rc::from(url.as_str()));
    }
}

fn precompute_row(cells: &[Inline]) -> PreRow {
    let mut h = PairHasher::new();
    cells.len().hash(&mut h);
    let mut flags = 0u8;
    let mut links = Vec::new();
    for c in cells {
        scan_inline(c, &mut h, &mut flags, &mut links);
    }
    PreRow { digest: h.finish128(), flags, links: links.into_boxed_slice() }
}

pub(super) fn precompute_block(b: &Block) -> PreBlock {
    let mut h = PairHasher::new();
    let mut flags = 0u8;
    let mut links = Vec::new();
    let mut items_out = Vec::new();
    let mut inner_out = Vec::new();
    let mut table_out = None;
    match b {
        Block::Para(inl) => {
            1u8.hash(&mut h);
            scan_inline(inl, &mut h, &mut flags, &mut links);
        }
        Block::Heading(level, inl) => {
            (2u8, *level).hash(&mut h);
            scan_inline(inl, &mut h, &mut flags, &mut links);
        }
        Block::Code(code) => {
            flags |= HAS_CODE;
            (3u8, code).hash(&mut h);
        }
        Block::Diff(src) => {
            (4u8, src).hash(&mut h);
        }
        Block::Mermaid(src) => {
            flags |= HAS_MERMAID;
            (5u8, src).hash(&mut h);
        }
        Block::Rule => {
            6u8.hash(&mut h);
        }
        Block::Image { src, alt, title } => {
            flags |= HAS_IMAGE;
            (7u8, src, alt, title).hash(&mut h);
        }
        Block::Quote(inner) => {
            flags |= HAS_QUOTE;
            (8u8, inner.len()).hash(&mut h);
            inner_out.reserve(inner.len());
            for child in inner {
                let cp = precompute_block(child);
                cp.digest.hash(&mut h);
                flags |= cp.flags;
                links.extend_from_slice(&cp.links);
                inner_out.push(cp);
            }
        }
        Block::List { start, items } => {
            (9u8, *start, items.len()).hash(&mut h);
            items_out.reserve(items.len());
            for item in items {
                let mut ih = PairHasher::new();
                item.len().hash(&mut ih);
                let (mut iflags, mut ilinks, mut isrc, mut itask) = (0u8, Vec::new(), 0u32, 0u32);
                let mut children = Vec::with_capacity(item.len());
                for child in item {
                    let cp = precompute_block(child);
                    cp.digest.hash(&mut ih);
                    iflags |= cp.flags;
                    ilinks.extend_from_slice(&cp.links);
                    isrc += cp.src_n;
                    itask += cp.task_n;
                    children.push(cp);
                }
                let idigest = ih.finish128();
                idigest.hash(&mut h);
                flags |= iflags;
                links.extend_from_slice(&ilinks);
                items_out.push(PreItem {
                    digest: idigest,
                    flags: iflags,
                    links: ilinks.into_boxed_slice(),
                    src_n: isrc,
                    task_n: itask,
                    inner: children.into_boxed_slice(),
                });
            }
        }
        Block::Table(t) => {
            if t.open {
                flags |= HAS_OPEN_TABLE;
            }
            (10u8, t.open, t.align.len(), t.head.len(), t.rows.len()).hash(&mut h);
            for a in &t.align {
                hash_align(*a, &mut h);
            }
            let head = precompute_row(&t.head);
            head.digest.hash(&mut h);
            flags |= head.flags;
            links.extend_from_slice(&head.links);
            let mut rows = Vec::with_capacity(t.rows.len());
            for r in &t.rows {
                let pr = precompute_row(r);
                pr.digest.hash(&mut h);
                flags |= pr.flags;
                links.extend_from_slice(&pr.links);
                rows.push(pr);
            }
            table_out = Some(Box::new(PreTable { head, rows: rows.into_boxed_slice() }));
        }
    }
    let src_n = match b {
        Block::Code(_) | Block::Mermaid(_) => 1,
        Block::List { .. } => items_out.iter().map(|i| i.src_n).sum(),
        Block::Quote(_) => inner_out.iter().map(|c| c.src_n).sum(),
        _ => 0,
    };
    let task_n = match b {
        Block::List { items, .. } => items.len() as u32 + items_out.iter().map(|i| i.task_n).sum::<u32>(),
        Block::Quote(_) => inner_out.iter().map(|c| c.task_n).sum(),
        _ => 0,
    };
    PreBlock {
        digest: h.finish128(),
        flags,
        links: links.into_boxed_slice(),
        src_n,
        task_n,
        items: items_out.into_boxed_slice(),
        inner: inner_out.into_boxed_slice(),
        table: table_out,
    }
}

thread_local! {
    static BASE: Cell<Option<(usize, (u64, u64))>> = const { Cell::new(None) };
    static BLOCK_BUILDS: Cell<usize> = const { Cell::new(0) };
}

#[doc(hidden)]
pub fn reset_block_builds() {
    BLOCK_BUILDS.set(0);
}

#[doc(hidden)]
pub fn block_builds() -> usize {
    BLOCK_BUILDS.get()
}

pub(super) fn note_block_build() {
    BLOCK_BUILDS.set(BLOCK_BUILDS.get() + 1);
}

pub(super) fn with_renderer<R>(r: &Renderer, f: impl FnOnce() -> R) -> R {
    struct Reset(Option<(usize, (u64, u64))>);
    impl Drop for Reset {
        fn drop(&mut self) {
            BASE.set(self.0);
        }
    }
    let prev = BASE.replace(Some((std::ptr::from_ref(r).addr(), compute_base(r))));
    let _reset = Reset(prev);
    f()
}

/// Compiler-checked destructuring of every [`Renderer`] field: adding a field to `Renderer` fails
/// to compile here until it is either hashed in the base or guarded per block/link below.
fn compute_base(r: &Renderer) -> (u64, u64) {
    let Renderer {
        s, highlights, reveal, streaming, tasks_read_only, selectable, scroll_wide, on_link, on_link_secondary,
        // Evaluated per link in `hash_link_url` / `can_memo_links`:
        link_look: _, link_help: _, link_ring: _, link_favicon: _, code_link: _,
        // Guarded per block/item in `can_memo_flags` and `settled_*_digest`:
        highlighter: _, sources: _, next_source: _, tasks: _, next_task: _, on_task: _,
        #[cfg(feature = "mermaid")]
        diagram_frame: _,
        code_frame: _, quote_frame: _, images: _, on_image: _,
        // Per-block / per-pass state hashed explicitly (`dir`, `key`, `flow`):
        tentative: _, current_block_key: _, flow: _,
    } = r;
    let mut h = PairHasher::new();
    (s, highlights, reveal.to_bits(), streaming, tasks_read_only, selectable, scroll_wide,
        on_link.as_ref().map(|w| w.1), on_link_secondary.as_ref().map(|w| w.1)).hash(&mut h);
    h.finish128()
}

impl Renderer {
    fn base_hash(&self) -> (u64, u64) {
        let addr = std::ptr::from_ref(self).addr();
        match BASE.get() {
            Some((a, d)) if a == addr => d,
            _ => compute_base(self),
        }
    }
}

/// Compiler-checked destructuring of every [`Span`] field.
fn hash_span(sp: &Span, h: &mut impl Hasher) {
    let Span { start, end, weight, mono, size, color, bg, bg_pad, underline, italic } = *sp;
    (start, end, weight.map(f32::to_bits), mono, size.map(f32::to_bits), color, bg, bg_pad.to_bits(), underline, italic).hash(h);
}

fn hash_align(a: Align, h: &mut impl Hasher) {
    (match a { Align::Start => 0u8, Align::Center => 1u8, Align::End => 2u8 }).hash(h);
}

fn hash_flow(f: Flow, h: &mut impl Hasher) {
    let Flow { lists, ink, inset, cols, lead } = f;
    (lists, ink, inset.to_bits(), cols).hash(h);
    match lead {
        None => 0u8.hash(h),
        Some((cols, lead, before)) => {
            1u8.hash(h);
            (cols, before).hash(h);
            match lead {
                Lead::Bullet => 0u8.hash(h),
                Lead::Number(n) => (1u8, n).hash(h),
                Lead::Task(done) => (2u8, done).hash(h),
            }
        }
    }
}

fn hash_span_map(m: &SpanMap, h: &mut impl Hasher) {
    m.0.hash(h);
}

fn hash_block_map(m: &BlockMap, h: &mut impl Hasher) {
    match m {
        BlockMap::Inline(s) => {
            1u8.hash(h);
            hash_span_map(s, h);
        }
        BlockMap::Code(s) => {
            2u8.hash(h);
            hash_span_map(s, h);
        }
        BlockMap::List(items) => {
            3u8.hash(h);
            items.len().hash(h);
            for item in items {
                item.len().hash(h);
                for b in item {
                    hash_block_map(b, h);
                }
            }
        }
        BlockMap::Quote(inner) => {
            4u8.hash(h);
            inner.len().hash(h);
            for b in inner {
                hash_block_map(b, h);
            }
        }
        BlockMap::Table { head, rows } => {
            5u8.hash(h);
            head.len().hash(h);
            for s in head {
                hash_span_map(s, h);
            }
            rows.len().hash(h);
            for row in rows {
                row.len().hash(h);
                for s in row {
                    hash_span_map(s, h);
                }
            }
        }
        BlockMap::None => 0u8.hash(h),
    }
}

impl Renderer {
    fn can_memo_links(&self, has_links: bool) -> bool {
        if !has_links {
            return true;
        }
        // `on_link` / `on_link_secondary`: safe only when zero-sized (`TypeId` present in base).
        !self.on_link.as_ref().is_some_and(|w| w.1.is_none())
            && !self.on_link_secondary.as_ref().is_some_and(|w| w.1.is_none())
    }

    fn can_memo_flags(&self, flags: u8) -> bool {
        // `Block::Code`: refused when custom `highlighter` or `code_frame` is set (closed card memos inside `code_frame`).
        if flags & HAS_CODE != 0 && (!self.highlighter.is_builtin() || self.code_frame.is_some()) {
            return false;
        }
        // `Block::Mermaid`: refused when `diagram_frame` or custom `highlighter` is set.
        if flags & HAS_MERMAID != 0 {
            #[cfg(feature = "mermaid")]
            if self.diagram_frame.is_some() {
                return false;
            }
            if !self.highlighter.is_builtin() {
                return false;
            }
        }
        // `Block::Image`: refused when `images` or `on_image` hook is set.
        if flags & HAS_IMAGE != 0 && (self.images.is_some() || self.on_image.is_some()) {
            return false;
        }
        // `Block::Quote`: refused when `quote_frame` hook is set.
        if flags & HAS_QUOTE != 0 && self.quote_frame.is_some() {
            return false;
        }
        // `Block::Table`: refused at block level while open and streaming (settled rows memo inside `table.rs`).
        if flags & HAS_OPEN_TABLE != 0 && self.streaming {
            return false;
        }
        true
    }

    fn hash_link_url(&self, url: &str, h: &mut PairHasher) {
        if let Some(look) = &self.link_look {
            match look(url) {
                None => 0u8.hash(h),
                Some((sp, lp)) => {
                    1u8.hash(h);
                    hash_span(&sp, h);
                    let _ = std::fmt::write(&mut HashFmt(h), format_args!("{lp:?}"));
                }
            }
        }
        if let Some(ring) = &self.link_ring {
            ring(url).map(|(c, w)| (c, w.to_bits())).hash(h);
        }
        if let Some(help) = &self.link_help {
            help(url).hash(h);
        }
        if self.s.link_icon.is_some()
            && let Some(host) = super::link_icon::link_host(url)
            && host != "github.com"
            && let Some(fav) = &self.link_favicon
        {
            match fav(url) {
                None => 0u8.hash(h),
                Some(px) => {
                    1u8.hash(h);
                    let _ = std::fmt::write(&mut HashFmt(h), format_args!("{px:?}"));
                }
            }
        }
    }

    pub(super) fn hash_inline_direct(&self, inl: &Inline, h: &mut PairHasher) -> bool {
        let resolved = self.with_code_links(inl);
        if !self.can_memo_links(!resolved.links.is_empty()) {
            return false;
        }
        (1u8, &*resolved).hash(h);
        for (a, b, url) in &resolved.links {
            self.code_link_color(*a, *b, &resolved).hash(h);
            self.hash_link_url(url, h);
        }
        true
    }

    fn hash_block_direct(&self, b: &Block, h: &mut PairHasher) -> bool {
        match b {
            Block::Para(inl) => {
                1u8.hash(h);
                self.hash_inline_direct(inl, h)
            }
            Block::Heading(level, inl) => {
                (2u8, *level).hash(h);
                self.hash_inline_direct(inl, h)
            }
            Block::Code(code) => {
                if !self.can_memo_flags(HAS_CODE) {
                    return false;
                }
                (3u8, code).hash(h);
                true
            }
            Block::Diff(src) => {
                (4u8, src).hash(h);
                true
            }
            Block::Mermaid(src) => {
                if !self.can_memo_flags(HAS_MERMAID) {
                    return false;
                }
                (5u8, src).hash(h);
                true
            }
            Block::Rule => {
                6u8.hash(h);
                true
            }
            Block::Image { src, alt, title } => {
                if !self.can_memo_flags(HAS_IMAGE) {
                    return false;
                }
                (7u8, src, alt, title).hash(h);
                true
            }
            Block::Quote(inner) => {
                if !self.can_memo_flags(HAS_QUOTE) {
                    return false;
                }
                (8u8, inner.len()).hash(h);
                inner.iter().all(|c| self.hash_block_direct(c, h))
            }
            Block::List { start, items } => {
                (9u8, *start, items.len()).hash(h);
                items.iter().all(|item| {
                    item.len().hash(h);
                    item.iter().all(|c| self.hash_block_direct(c, h))
                })
            }
            Block::Table(t) => {
                if t.open && self.streaming {
                    return false;
                }
                (10u8, t.open, t.align.len(), t.head.len(), t.rows.len()).hash(h);
                for a in &t.align {
                    hash_align(*a, h);
                }
                t.head.iter().all(|c| self.hash_inline_direct(c, h))
                    && t.rows.iter().all(|row| {
                        row.len().hash(h);
                        row.iter().all(|c| self.hash_inline_direct(c, h))
                    })
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn settled_block_digest(
        &self, b: &Block, pre: Option<&PreBlock>, map: &BlockMap, key: &str, top: f32,
        dir: Option<rune::Direction>, (src_at, src_n): (usize, usize), (task_at, task_n): (usize, usize),
    ) -> Option<(u64, u64)> {
        let srcs = self.sources.get(src_at..src_at + src_n)?;
        if srcs.iter().any(|s| s.open) {
            return None;
        }
        let tasks = self.tasks.get(task_at..task_at + task_n)?;
        if self.on_task.is_some() && !self.tasks_read_only && tasks.iter().any(Option::is_some) {
            return None;
        }
        let mut h = PairHasher::new();
        (1u8, self.base_hash(), key, top.to_bits(), dir).hash(&mut h);
        match pre.filter(|p| self.code_link.is_none() || p.flags & HAS_INLINE_CODE == 0) {
            Some(p) => {
                if !self.can_memo_flags(p.flags) || !self.can_memo_links(!p.links.is_empty()) {
                    return None;
                }
                p.digest.hash(&mut h);
                for url in &p.links {
                    self.hash_link_url(url, &mut h);
                }
            }
            None => {
                if !self.hash_block_direct(b, &mut h) {
                    return None;
                }
            }
        }
        hash_flow(self.flow.get(), &mut h);
        for s in srcs {
            (&s.lang, s.len, s.coloured, s.open).hash(&mut h);
        }
        tasks.hash(&mut h);
        if !self.highlights.is_empty() {
            hash_block_map(map, &mut h);
        }
        Some(h.finish128())
    }

    pub(super) fn settled_item_digest(
        &self,
        item: &[Block],
        pre: Option<&PreItem>,
        imap: &[BlockMap],
        (src_at, src_n): (usize, usize),
        (task_at, task_n): (usize, usize),
    ) -> Option<(u64, u64)> {
        let srcs = self.sources.get(src_at..src_at + src_n)?;
        if srcs.iter().any(|s| s.open) {
            return None;
        }
        let tasks = self.tasks.get(task_at..task_at + task_n)?;
        if self.on_task.is_some() && !self.tasks_read_only && tasks.iter().any(Option::is_some) {
            return None;
        }
        let mut h = PairHasher::new();
        (2u8, self.base_hash()).hash(&mut h);
        match pre.filter(|p| self.code_link.is_none() || p.flags & HAS_INLINE_CODE == 0) {
            Some(p) => {
                if !self.can_memo_flags(p.flags) || !self.can_memo_links(!p.links.is_empty()) {
                    return None;
                }
                p.digest.hash(&mut h);
                for url in &p.links {
                    self.hash_link_url(url, &mut h);
                }
            }
            None => {
                item.len().hash(&mut h);
                for b in item {
                    if !self.hash_block_direct(b, &mut h) {
                        return None;
                    }
                }
            }
        }
        hash_flow(self.flow.get(), &mut h);
        for s in srcs {
            (&s.lang, s.len, s.coloured, s.open).hash(&mut h);
        }
        tasks.hash(&mut h);
        if !self.highlights.is_empty() {
            imap.len().hash(&mut h);
            for m in imap {
                hash_block_map(m, &mut h);
            }
        }
        Some(h.finish128())
    }

    pub(super) fn settled_code_card_digest(&self, code: &str, source: Option<&Source>, map: Option<&SpanMap>) -> Option<(u64, u64)> {
        if !self.highlighter.is_builtin() || source.is_some_and(|s| s.open) {
            return None;
        }
        let mut h = PairHasher::new();
        (3u8, self.base_hash(), code).hash(&mut h);
        if let Some(s) = source {
            (1u8, &s.lang, s.len, s.coloured, s.open).hash(&mut h);
        } else {
            0u8.hash(&mut h);
        }
        if !self.highlights.is_empty()
            && let Some(m) = map
        {
            hash_span_map(m, &mut h);
        }
        Some(h.finish128())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn settled_table_row_digest(
        &self, cells: &[Inline], pre: Option<&PreRow>, maps: &[SpanMap], align: &[Align],
        table_dir: Option<rune::Direction>, natural: &[f32], min: &[f32], widths: Option<&[f32]>,
        font: TextStyle, color: Color, pad: [f32; 2], cell_role: &'static str, key: &str, extra: u64,
    ) -> Option<(u64, u64)> {
        let mut h = PairHasher::new();
        (4u8, self.base_hash(), cell_role, key, extra, font, color, pad.map(f32::to_bits), table_dir).hash(&mut h);
        match pre.filter(|p| self.code_link.is_none() || p.flags & HAS_INLINE_CODE == 0) {
            Some(p) => {
                if !self.can_memo_links(!p.links.is_empty()) {
                    return None;
                }
                p.digest.hash(&mut h);
                for url in &p.links {
                    self.hash_link_url(url, &mut h);
                }
            }
            None => {
                cells.len().hash(&mut h);
                for c in cells {
                    if !self.hash_inline_direct(c, &mut h) {
                        return None;
                    }
                }
            }
        }
        align.len().hash(&mut h);
        for a in align {
            hash_align(*a, &mut h);
        }
        for w in natural {
            w.to_bits().hash(&mut h);
        }
        for w in min {
            w.to_bits().hash(&mut h);
        }
        match widths {
            None => 0u8.hash(&mut h),
            Some(ws) => {
                1u8.hash(&mut h);
                for w in ws {
                    w.to_bits().hash(&mut h);
                }
            }
        }
        if !self.highlights.is_empty() {
            maps.len().hash(&mut h);
            for m in maps {
                hash_span_map(m, &mut h);
            }
        }
        Some(h.finish128())
    }

    pub(super) fn table_row_width_key(
        &self,
        cells: &[Inline],
        pre: Option<&PreRow>,
        n: usize,
        fkey: &(u32, u32, u32, u32, String),
        inline_code_key: (u32, u32, u32, u32),
        epoch: u64,
    ) -> (u64, u64) {
        let mut h = PairHasher::new();
        (n, fkey, inline_code_key, epoch, cells.len().min(n)).hash(&mut h);
        match pre.filter(|p| cells.len() <= n && (self.code_link.is_none() || p.flags & HAS_INLINE_CODE == 0)) {
            Some(p) => {
                p.digest.hash(&mut h);
                if let Some(look) = &self.link_look {
                    for url in &p.links {
                        look(url)
                            .map(|(sp, _)| (sp.weight.map(f32::to_bits), sp.size.map(f32::to_bits), sp.mono, sp.italic))
                            .hash(&mut h);
                    }
                }
            }
            None => {
                for inl in cells.iter().take(n) {
                    let resolved = self.with_code_links(inl);
                    resolved.hash(&mut h);
                    if let Some(look) = &self.link_look {
                        for (_, _, url) in &resolved.links {
                            look(url)
                                .map(|(sp, _)| (sp.weight.map(f32::to_bits), sp.size.map(f32::to_bits), sp.mono, sp.italic))
                                .hash(&mut h);
                        }
                    }
                }
            }
        }
        h.finish128()
    }
}
