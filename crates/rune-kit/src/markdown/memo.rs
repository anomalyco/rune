//! Memoising settled Markdown subtrees (`blocks_with_dirs`, list item bodies, code cards, and
//! table rows) across streaming commits so appending to the open trailing block rebuilds and lays
//! out only what is still arriving.
//!
//! Each parsed block, list item and table row carries a pre-computed 128-bit content digest
//! (`PreBlock` / `PreItem` / `PreRow`), seeded per process from [`RandomState`] and carried across
//! [`super::cache`]'s `extend` (and invalidated along the trailing spine on `extend_eof_deep_tail`),
//! so settled blocks do not re-hash their text on later tokens.

use std::collections::hash_map::{DefaultHasher, RandomState};
use std::hash::{BuildHasher, Hash, Hasher};
use std::sync::LazyLock;

use super::parser::{BlockMap, Source, SpanMap};
use super::render::{Flow, Lead, Renderer};
use super::*;

static SEEDS: LazyLock<(RandomState, RandomState)> = LazyLock::new(|| (RandomState::new(), RandomState::new()));

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
    pub(super) fn finish128(&self) -> (u64, u64) { (self.a.finish(), self.b.finish()) }
}

impl Hasher for PairHasher {
    fn finish(&self) -> u64 { self.a.finish() }
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
/// Inline math: like display math, its block builds every time, asking the math cache.
pub(super) const HAS_MATH: u8 = 1 << 6;
pub(super) const HAS_DIFF: u8 = 1 << 7;

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
    pub(super) inner: Box<[Rc<PreBlock>]>,
}

pub(super) struct PreBlock {
    pub(super) digest: (u64, u64),
    pub(super) flags: u8,
    pub(super) links: Box<[Rc<str>]>,
    pub(super) src_n: u32,
    pub(super) task_n: u32,
    pub(super) items: Box<[PreItem]>,
    pub(super) inner: Box<[Rc<PreBlock>]>,
    pub(super) table: Option<Box<PreTable>>,
}

impl PreRow {
    fn heap_bytes(&self) -> usize {
        const RC_HEADER: usize = 2 * std::mem::size_of::<usize>();
        self.links.iter().map(|u| std::mem::size_of::<Rc<str>>() + RC_HEADER + u.len()).sum()
    }
}

impl PreBlock {
    /// Heap bytes owned beneath this top-level block (nested items, child blocks, table rows,
    /// and copied link URLs), counted iteratively (`O(1)` call stack); a plain top-level block
    /// with none of those adds `0`.
    pub(super) fn heap_bytes(&self) -> usize {
        use std::mem::size_of;
        const RC_HEADER: usize = 2 * size_of::<usize>();
        let mut total = 0usize;
        let mut stack: Vec<(&Self, bool)> = vec![(self, false)];
        while let Some((pb, count_rc)) = stack.pop() {
            if count_rc { total += size_of::<Rc<Self>>() + RC_HEADER + size_of::<Self>(); }
            let own_urls = pb.items.is_empty() && pb.inner.is_empty() && pb.table.is_none();
            total += pb.links.len() * size_of::<Rc<str>>()
                + if own_urls { pb.links.iter().map(|u| RC_HEADER + u.len()).sum::<usize>() } else { 0 };
            for it in &pb.items {
                total += size_of::<PreItem>() + it.links.len() * size_of::<Rc<str>>();
                for c in &it.inner { stack.push((c, true)); }
            }
            for c in &pb.inner { stack.push((c, true)); }
            if let Some(t) = &pb.table {
                total += size_of::<PreTable>() + t.head.heap_bytes() + t.rows.len() * size_of::<PreRow>()
                    + t.rows.iter().map(PreRow::heap_bytes).sum::<usize>();
            }
        }
        total
    }
}

pub(super) fn source_count(root: &Block) -> usize {
    let (mut n, mut stack) = (0usize, vec![root]);
    while let Some(b) = stack.pop() {
        match b {
            Block::Code(_) | Block::Mermaid(_) => n += 1,
            Block::List { items, .. } => stack.extend(items.iter().flatten()),
            Block::Quote(inner) => stack.extend(inner.iter()),
            _ => {}
        }
    }
    n
}

pub(super) fn task_count(root: &Block) -> usize {
    let (mut n, mut stack) = (0usize, vec![root]);
    while let Some(b) = stack.pop() {
        match b {
            Block::List { items, .. } => { n += items.len(); stack.extend(items.iter().flatten()); }
            Block::Quote(inner) => stack.extend(inner.iter()),
            _ => {}
        }
    }
    n
}

fn scan_inline(inl: &Inline, h: &mut PairHasher, flags: &mut u8, links: &mut Vec<Rc<str>>) {
    (0u8, inl).hash(h);
    if inl.spans.iter().any(|s| matches!(s.2, Style::Code)) { *flags |= HAS_INLINE_CODE; }
    if inl.spans.iter().any(|s| matches!(s.2, Style::Math)) { *flags |= HAS_MATH; }
    for (_, _, url) in &inl.links { links.push(Rc::from(url.as_str())); }
}

fn precompute_row(cells: &[Inline]) -> PreRow {
    let mut h = PairHasher::new();
    cells.len().hash(&mut h);
    let (mut flags, mut links) = (0u8, Vec::new());
    for c in cells { scan_inline(c, &mut h, &mut flags, &mut links); }
    PreRow { digest: h.finish128(), flags, links: links.into_boxed_slice() }
}

pub(super) fn rebuild_quote_meta(inner: Box<[Rc<PreBlock>]>) -> Rc<PreBlock> {
    let mut h = PairHasher::new();
    (8u8, inner.len()).hash(&mut h);
    let (mut flags, mut links, mut src_n, mut task_n) = (HAS_QUOTE, Vec::new(), 0u32, 0u32);
    for cp in &inner {
        cp.digest.hash(&mut h);
        flags |= cp.flags;
        links.extend_from_slice(&cp.links);
        src_n += cp.src_n;
        task_n += cp.task_n;
    }
    Rc::new(PreBlock { digest: h.finish128(), flags, links: links.into_boxed_slice(), src_n, task_n, items: Box::default(), inner, table: None })
}

pub(super) fn rebuild_item_meta(children: Box<[Rc<PreBlock>]>) -> PreItem {
    let mut ih = PairHasher::new();
    children.len().hash(&mut ih);
    let (mut flags, mut links, mut src_n, mut task_n) = (0u8, Vec::new(), 0u32, 0u32);
    for cp in &children {
        cp.digest.hash(&mut ih);
        flags |= cp.flags;
        links.extend_from_slice(&cp.links);
        src_n += cp.src_n;
        task_n += cp.task_n;
    }
    PreItem { digest: ih.finish128(), flags, links: links.into_boxed_slice(), src_n, task_n, inner: children }
}

pub(super) fn rebuild_list_meta(start: Option<u64>, items: Box<[PreItem]>) -> Rc<PreBlock> {
    let mut h = PairHasher::new();
    (9u8, start, items.len()).hash(&mut h);
    let (mut flags, mut links, mut src_n, mut task_n) = (0u8, Vec::new(), 0u32, items.len() as u32);
    for it in &items {
        it.digest.hash(&mut h);
        flags |= it.flags;
        links.extend_from_slice(&it.links);
        src_n += it.src_n;
        task_n += it.task_n;
    }
    Rc::new(PreBlock { digest: h.finish128(), flags, links: links.into_boxed_slice(), src_n, task_n, items, inner: Box::default(), table: None })
}

/// Precomputes `root_b`'s 128-bit digest tree iteratively (`O(1)` call stack at up to `511` nested
/// container levels).
pub(super) fn precompute_block(root_b: &Block, root_map: Option<&BlockMap>) -> Rc<PreBlock> {
    enum Step<'a> {
        Enter(&'a Block, Option<&'a BlockMap>),
        FinishQuote(usize),
        FinishItem(usize),
        FinishList(Option<u64>, usize),
    }
    let (mut vals, mut items_stack, mut stack) = (Vec::new(), Vec::new(), vec![Step::Enter(root_b, root_map)]);
    while let Some(step) = stack.pop() {
        match step {
            Step::Enter(b, map) => match (b, map) {
                (Block::Quote(inner), map_opt) => {
                    let inner_maps = match map_opt { Some(BlockMap::Quote(m)) => m.as_slice(), _ => &[][..] };
                    stack.push(Step::FinishQuote(inner.len()));
                    for (i, child) in inner.iter().enumerate().rev() { stack.push(Step::Enter(child, inner_maps.get(i))); }
                }
                (Block::List { start, items }, map_opt) => {
                    let item_maps = match map_opt { Some(BlockMap::List(m)) => m.as_slice(), _ => &[][..] };
                    stack.push(Step::FinishList(*start, items.len()));
                    for (k, item) in items.iter().enumerate().rev() {
                        let imap = item_maps.get(k).map_or(&[][..], Vec::as_slice);
                        stack.push(Step::FinishItem(item.len()));
                        for (j, child) in item.iter().enumerate().rev() { stack.push(Step::Enter(child, imap.get(j))); }
                    }
                }
                _ => {
                    let (mut h, mut flags, mut links, mut table, mut src_n) = (PairHasher::new(), 0u8, Vec::new(), None, 0u32);
                    match (b, map) {
                        (Block::Para(inl), Some(BlockMap::DeepTail { lines, .. })) => (11u8, *lines, &inl.text).hash(&mut h),
                        (Block::Para(inl), _) => { 1u8.hash(&mut h); scan_inline(inl, &mut h, &mut flags, &mut links); }
                        (Block::Heading(level, inl), _) => { (2u8, *level).hash(&mut h); scan_inline(inl, &mut h, &mut flags, &mut links); }
                        (Block::Code(code), _) => { flags |= HAS_CODE; src_n = 1; (3u8, code).hash(&mut h); }
                        (Block::Diff(src), map_opt) => { flags |= HAS_DIFF; let open = matches!(map_opt, Some(BlockMap::Diff { open: true, .. })); (4u8, open, src).hash(&mut h); }
                        (Block::Mermaid(src), _) => { flags |= HAS_MERMAID; src_n = 1; (5u8, src).hash(&mut h); }
                        (Block::Rule, _) => 6u8.hash(&mut h),
                        (Block::Image { src, alt, title }, _) => { flags |= HAS_IMAGE; (7u8, src, alt, title).hash(&mut h); }
                        (Block::Table(t), _) => {
                            if t.open { flags |= HAS_OPEN_TABLE; }
                            (10u8, t.open, t.align.len(), t.head.len(), t.rows.len()).hash(&mut h);
                            for a in &t.align { hash_align(*a, &mut h); }
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
                            table = Some(Box::new(PreTable { head, rows: rows.into_boxed_slice() }));
                        }
                        (Block::Quote(_) | Block::List { .. }, _) => unreachable!(),
                    }
                    vals.push(Rc::new(PreBlock { digest: h.finish128(), flags, links: links.into_boxed_slice(), src_n, task_n: 0, items: Box::default(), inner: Box::default(), table }));
                }
            },
            Step::FinishQuote(n) => { let inner = vals.split_off(vals.len() - n).into_boxed_slice(); vals.push(rebuild_quote_meta(inner)); }
            Step::FinishItem(n) => { let children = vals.split_off(vals.len() - n).into_boxed_slice(); items_stack.push(rebuild_item_meta(children)); }
            Step::FinishList(start, n) => { let items = items_stack.split_off(items_stack.len() - n).into_boxed_slice(); vals.push(rebuild_list_meta(start, items)); }
        }
    }
    vals.pop().expect("root PreBlock")
}

thread_local! {
    static BASE: Cell<Option<(usize, (u64, u64))>> = const { Cell::new(None) };
    static BLOCK_BUILDS: Cell<usize> = const { Cell::new(0) };
}

#[doc(hidden)]
pub fn reset_block_builds() { BLOCK_BUILDS.set(0); }
#[doc(hidden)]
pub fn block_builds() -> usize { BLOCK_BUILDS.get() }
pub(super) fn note_block_build() { BLOCK_BUILDS.set(BLOCK_BUILDS.get() + 1); }

pub(super) fn with_renderer<R>(r: &Renderer, f: impl FnOnce() -> R) -> R {
    struct Reset(Option<(usize, (u64, u64))>);
    impl Drop for Reset { fn drop(&mut self) { BASE.set(self.0); } }
    let _reset = Reset(BASE.replace(Some((std::ptr::from_ref(r).addr(), compute_base(r)))));
    f()
}

/// Compiler-checked destructuring of every [`Renderer`] field: adding a field to `Renderer` fails
/// to compile here until it is either hashed in the base or guarded per block/link below.
fn compute_base(r: &Renderer) -> (u64, u64) {
    let Renderer {
        s, highlights, reveal, streaming, tasks_read_only, selectable, scroll_wide, wrap_anywhere, on_link, on_link_secondary,
        link_look: _, link_help: _, link_ring: _, link_favicon: _, code_link: _,
        highlighter: _, sources: _, next_source: _, next_diff: _, tasks: _, next_task: _, on_task: _,
        #[cfg(feature = "mermaid")]
        diagram_frame: _,
        code_frame: _, diff_frame: _, quote_frame: _, images: _, on_image: _,
        tentative: _, current_block_key: _, flow: _,
    } = r;
    let mut h = PairHasher::new();
    (s, highlights, reveal.to_bits(), streaming, tasks_read_only, selectable, scroll_wide, wrap_anywhere,
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
            (1u8, cols, before).hash(h);
            match lead {
                Lead::Bullet => 0u8.hash(h),
                Lead::Number(n) => (1u8, n).hash(h),
                Lead::Task(done) => (2u8, done).hash(h),
            }
        }
    }
}

fn hash_span_map(m: &SpanMap, h: &mut impl Hasher) { m.0.hash(h); }

fn hash_block_map(root: &BlockMap, h: &mut impl Hasher) {
    enum Step<'a> { Map(&'a BlockMap), Len(usize) }
    let mut stack = vec![Step::Map(root)];
    while let Some(step) = stack.pop() {
        match step {
            Step::Len(n) => n.hash(h),
            Step::Map(m) => match m {
                BlockMap::Inline(s) => { 1u8.hash(h); hash_span_map(s, h); }
                BlockMap::Code(s) => { 2u8.hash(h); hash_span_map(s, h); }
                BlockMap::List(items) => {
                    (3u8, items.len()).hash(h);
                    for item in items.iter().rev() {
                        for b in item.iter().rev() { stack.push(Step::Map(b)); }
                        stack.push(Step::Len(item.len()));
                    }
                }
                BlockMap::Quote(inner) => {
                    (4u8, inner.len()).hash(h);
                    for b in inner.iter().rev() { stack.push(Step::Map(b)); }
                }
                BlockMap::Table { head, rows } => {
                    (5u8, head.len()).hash(h);
                    for s in head { hash_span_map(s, h); }
                    rows.len().hash(h);
                    for row in rows {
                        row.len().hash(h);
                        for s in row { hash_span_map(s, h); }
                    }
                }
                BlockMap::DeepTail { lines, map } => { (6u8, *lines).hash(h); hash_span_map(map, h); }
                BlockMap::Diff { open, map } => { (7u8, *open).hash(h); hash_span_map(map, h); }
                BlockMap::None => 0u8.hash(h),
            },
        }
    }
}

impl Renderer {
    fn can_memo_links(&self, has_links: bool) -> bool {
        !has_links
            || (!self.on_link.as_ref().is_some_and(|w| w.1.is_none())
                && !self.on_link_secondary.as_ref().is_some_and(|w| w.1.is_none()))
    }

    fn can_memo_flags(&self, flags: u8) -> bool {
        if flags & HAS_MATH != 0 { return false; }
        if flags & HAS_CODE != 0 && (!self.highlighter.is_builtin() || self.code_frame.as_ref().is_some_and(|w| w.frame.is_some() || w.wrap.is_some())) { return false; }
        if flags & HAS_MERMAID != 0 {
            #[cfg(feature = "mermaid")]
            if self.diagram_frame.is_some() { return false; }
            if !self.highlighter.is_builtin() { return false; }
        }
        if flags & HAS_IMAGE != 0 && (self.images.is_some() || self.on_image.is_some()) { return false; }
        if flags & HAS_QUOTE != 0 && self.quote_frame.is_some() { return false; }
        if flags & HAS_DIFF != 0 && self.diff_frame.is_some() { return false; }
        if flags & HAS_OPEN_TABLE != 0 && self.streaming { return false; }
        true
    }

    fn hash_link_url(&self, url: &str, h: &mut PairHasher) {
        if let Some(look) = &self.link_look {
            match look(url) {
                None => 0u8.hash(h),
                Some((sp, lp)) => {
                    1u8.hash(h);
                    hash_span(&sp, h);
                    std::fmt::write(&mut HashFmt(h), format_args!("{lp:?}")).ok();
                }
            }
        }
        if let Some(ring) = &self.link_ring { ring(url).map(|(c, w)| (c, w.to_bits())).hash(h); }
        if let Some(help) = &self.link_help { help(url).hash(h); }
        if self.s.link_icon.is_some()
            && let Some(host) = super::link_icon::link_host(url)
            && host != "github.com"
            && let Some(fav) = &self.link_favicon
        {
            match fav(url) {
                None => 0u8.hash(h),
                Some(px) => { 1u8.hash(h); std::fmt::write(&mut HashFmt(h), format_args!("{px:?}")).ok(); }
            }
        }
    }

    pub(super) fn hash_inline_direct(&self, inl: &Inline, h: &mut PairHasher) -> bool {
        if inl.spans.iter().any(|s| matches!(s.2, Style::Math)) { return false; }
        let resolved = self.with_code_links(inl);
        if !self.can_memo_links(!resolved.links.is_empty()) { return false; }
        (1u8, &*resolved).hash(h);
        for (a, b, url) in &resolved.links {
            self.code_link_color(*a, *b, &resolved).hash(h);
            self.hash_link_url(url, h);
        }
        true
    }

    fn hash_block_direct(&self, root_b: &Block, root_map: Option<&BlockMap>, h: &mut PairHasher) -> bool {
        enum Step<'a> { Block(&'a Block, Option<&'a BlockMap>), Len(usize) }
        let mut stack = vec![Step::Block(root_b, root_map)];
        while let Some(step) = stack.pop() {
            match step {
                Step::Len(n) => n.hash(h),
                Step::Block(b, map) => match (b, map) {
                    (Block::Para(inl), Some(BlockMap::DeepTail { lines, .. })) => (11u8, *lines, &inl.text).hash(h),
                    (Block::Para(inl), _) => { 1u8.hash(h); if !self.hash_inline_direct(inl, h) { return false; } }
                    (Block::Heading(level, inl), _) => { (2u8, *level).hash(h); if !self.hash_inline_direct(inl, h) { return false; } }
                    (Block::Code(code), _) => { if !self.can_memo_flags(HAS_CODE) { return false; } (3u8, self.wrap_code_default(), code).hash(h); }
                    (Block::Diff(src), map_opt) => { if !self.can_memo_flags(HAS_DIFF) { return false; } let open = matches!(map_opt, Some(BlockMap::Diff { open: true, .. })); (4u8, open, src).hash(h); }
                    (Block::Mermaid(src), _) => { if !self.can_memo_flags(HAS_MERMAID) { return false; } (5u8, src).hash(h); }
                    (Block::Rule, _) => 6u8.hash(h),
                    (Block::Image { src, alt, title }, _) => { if !self.can_memo_flags(HAS_IMAGE) { return false; } (7u8, src, alt, title).hash(h); }
                    (Block::Quote(inner), map_opt) => {
                        if !self.can_memo_flags(HAS_QUOTE) { return false; }
                        let inner_maps = match map_opt { Some(BlockMap::Quote(m)) => m.as_slice(), _ => &[][..] };
                        (8u8, inner.len()).hash(h);
                        for (i, c) in inner.iter().enumerate().rev() { stack.push(Step::Block(c, inner_maps.get(i))); }
                    }
                    (Block::List { start, items }, map_opt) => {
                        let item_maps = match map_opt { Some(BlockMap::List(m)) => m.as_slice(), _ => &[][..] };
                        (9u8, *start, items.len()).hash(h);
                        for (k, item) in items.iter().enumerate().rev() {
                            let imap = item_maps.get(k).map_or(&[][..], Vec::as_slice);
                            for (j, c) in item.iter().enumerate().rev() { stack.push(Step::Block(c, imap.get(j))); }
                            stack.push(Step::Len(item.len()));
                        }
                    }
                    (Block::Table(t), _) => {
                        if t.open && self.streaming { return false; }
                        (10u8, t.open, t.align.len(), t.head.len(), t.rows.len()).hash(h);
                        for a in &t.align { hash_align(*a, h); }
                        if !t.head.iter().all(|c| self.hash_inline_direct(c, h))
                            || !t.rows.iter().all(|row| { row.len().hash(h); row.iter().all(|c| self.hash_inline_direct(c, h)) })
                        {
                            return false;
                        }
                    }
                },
            }
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn settled_block_digest(
        &self, b: &Block, pre: Option<&PreBlock>, map: &BlockMap, key: &str, top: f32,
        dir: Option<rune::Direction>, cap_prose: bool, (src_at, src_n): (usize, usize), (task_at, task_n): (usize, usize),
    ) -> Option<(u64, u64)> {
        let srcs = self.sources.get(src_at..src_at + src_n)?;
        if srcs.iter().any(|s| s.open) { return None; }
        let tasks = self.tasks.get(task_at..task_at + task_n)?;
        if self.on_task.is_some() && !self.tasks_read_only && tasks.iter().any(Option::is_some) { return None; }
        let mut h = PairHasher::new();
        (1u8, self.base_hash(), key, top.to_bits(), dir, cap_prose).hash(&mut h);
        match pre.filter(|p| self.code_link.is_none() || p.flags & HAS_INLINE_CODE == 0) {
            Some(p) => {
                if !self.can_memo_flags(p.flags) || !self.can_memo_links(!p.links.is_empty()) { return None; }
                p.digest.hash(&mut h);
                if p.flags & HAS_CODE != 0 { self.wrap_code_default().hash(&mut h); }
                for url in &p.links { self.hash_link_url(url, &mut h); }
            }
            None => if !self.hash_block_direct(b, Some(map), &mut h) { return None; },
        }
        hash_flow(self.flow.get(), &mut h);
        for s in srcs { (&s.lang, s.len, s.coloured, s.open).hash(&mut h); }
        // A block with display math builds every time its document does: each build asks the math
        // cache, so a host's row notes what is still rendering (`math::collect`). The equation's
        // own element stays memoised (`settled_math_card_digest`).
        #[cfg(feature = "math")]
        if srcs.iter().any(|s| crate::math::is_math_fence(s.lang.as_deref())) {
            return None;
        }
        tasks.hash(&mut h);
        if !self.highlights.is_empty() { hash_block_map(map, &mut h); }
        Some(h.finish128())
    }

    pub(super) fn settled_item_digest(
        &self, item: &[Block], pre: Option<&PreItem>, imap: &[BlockMap],
        (src_at, src_n): (usize, usize), (task_at, task_n): (usize, usize),
    ) -> Option<(u64, u64)> {
        let srcs = self.sources.get(src_at..src_at + src_n)?;
        if srcs.iter().any(|s| s.open) { return None; }
        let tasks = self.tasks.get(task_at..task_at + task_n)?;
        if self.on_task.is_some() && !self.tasks_read_only && tasks.iter().any(Option::is_some) { return None; }
        let mut h = PairHasher::new();
        (2u8, self.base_hash()).hash(&mut h);
        match pre.filter(|p| self.code_link.is_none() || p.flags & HAS_INLINE_CODE == 0) {
            Some(p) => {
                if !self.can_memo_flags(p.flags) || !self.can_memo_links(!p.links.is_empty()) { return None; }
                p.digest.hash(&mut h);
                if p.flags & HAS_CODE != 0 { self.wrap_code_default().hash(&mut h); }
                for url in &p.links { self.hash_link_url(url, &mut h); }
            }
            None => {
                item.len().hash(&mut h);
                for (j, b) in item.iter().enumerate() {
                    if !self.hash_block_direct(b, imap.get(j), &mut h) { return None; }
                }
            }
        }
        hash_flow(self.flow.get(), &mut h);
        for s in srcs { (&s.lang, s.len, s.coloured, s.open).hash(&mut h); }
        // A block with display math builds every time its document does: each build asks the math
        // cache, so a host's row notes what is still rendering (`math::collect`). The equation's
        // own element stays memoised (`settled_math_card_digest`).
        #[cfg(feature = "math")]
        if srcs.iter().any(|s| crate::math::is_math_fence(s.lang.as_deref())) {
            return None;
        }
        tasks.hash(&mut h);
        if !self.highlights.is_empty() {
            imap.len().hash(&mut h);
            for m in imap { hash_block_map(m, &mut h); }
        }
        Some(h.finish128())
    }

    pub(super) fn settled_code_card_digest(&self, code: &str, source: Option<&Source>, map: Option<&SpanMap>, wrapped: bool) -> Option<(u64, u64)> {
        if !self.highlighter.is_builtin() || source.is_some_and(|s| s.open) { return None; }
        let mut h = PairHasher::new();
        (3u8, self.base_hash(), wrapped, code).hash(&mut h);
        match source {
            Some(s) => (1u8, &s.lang, s.len, s.coloured, s.open).hash(&mut h),
            None => 0u8.hash(&mut h),
        }
        if !self.highlights.is_empty() && let Some(m) = map { hash_span_map(m, &mut h); }
        Some(h.finish128())
    }

    pub(super) fn settled_diff_card_digest(&self, src: &str, open: bool, map: Option<&SpanMap>) -> Option<(u64, u64)> {
        if open { return None; }
        let mut h = PairHasher::new();
        (6u8, self.base_hash(), src).hash(&mut h);
        if !self.highlights.is_empty() && let Some(m) = map { hash_span_map(m, &mut h); }
        Some(h.finish128())
    }

    #[cfg(feature = "math")]
    pub(super) fn settled_math_card_digest(&self, code: &str, ink: Color, avail: Option<f32>, rendered_id: u64) -> (u64, u64) {
        let mut h = PairHasher::new();
        (5u8, self.base_hash(), ink, avail.map(f32::to_bits), rendered_id, code).hash(&mut h);
        h.finish128()
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
                if !self.can_memo_flags(p.flags) || !self.can_memo_links(!p.links.is_empty()) { return None; }
                p.digest.hash(&mut h);
                for url in &p.links { self.hash_link_url(url, &mut h); }
            }
            None => {
                cells.len().hash(&mut h);
                for c in cells { if !self.hash_inline_direct(c, &mut h) { return None; } }
            }
        }
        align.len().hash(&mut h);
        for a in align { hash_align(*a, &mut h); }
        for w in natural { w.to_bits().hash(&mut h); }
        for w in min { w.to_bits().hash(&mut h); }
        match widths {
            None => 0u8.hash(&mut h),
            Some(ws) => { 1u8.hash(&mut h); for w in ws { w.to_bits().hash(&mut h); } }
        }
        if !self.highlights.is_empty() {
            maps.len().hash(&mut h);
            for m in maps { hash_span_map(m, &mut h); }
        }
        Some(h.finish128())
    }

    pub(super) fn table_row_width_key(
        &self, cells: &[Inline], pre: Option<&PreRow>, n: usize,
        fkey: &(u32, u32, u32, u32, String), inline_code_key: (u32, u32, u32, u32), epoch: u64,
    ) -> (u64, u64) {
        let mut h = PairHasher::new();
        (n, fkey, inline_code_key, epoch, cells.len().min(n)).hash(&mut h);
        match pre.filter(|p| cells.len() <= n && (self.code_link.is_none() || p.flags & HAS_INLINE_CODE == 0)) {
            Some(p) => {
                p.digest.hash(&mut h);
                if let Some(look) = &self.link_look {
                    for url in &p.links {
                        look(url).map(|(sp, _)| (sp.weight.map(f32::to_bits), sp.size.map(f32::to_bits), sp.mono, sp.italic)).hash(&mut h);
                    }
                }
            }
            None => {
                for inl in cells.iter().take(n) {
                    let resolved = self.with_code_links(inl);
                    resolved.hash(&mut h);
                    if let Some(look) = &self.link_look {
                        for (_, _, url) in &resolved.links {
                            look(url).map(|(sp, _)| (sp.weight.map(f32::to_bits), sp.size.map(f32::to_bits), sp.mono, sp.italic)).hash(&mut h);
                        }
                    }
                }
            }
        }
        h.finish128()
    }
}
