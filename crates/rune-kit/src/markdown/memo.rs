//! Memoising settled Markdown subtrees (`blocks_in`, list item bodies, code cards, and table
//! rows) across streaming commits so appending to the open trailing block rebuilds and lays out
//! only what is still arriving.

use std::hash::{Hash, Hasher};

use super::parser::{BlockMap, Source, SpanMap};
use super::render::{Flow, Lead, Renderer};
use super::*;

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

thread_local! {
    static BASE: Cell<Option<(usize, (u64, u64))>> = const { Cell::new(None) };
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

fn compute_base(r: &Renderer) -> (u64, u64) {
    rune::memo::digest(&(
        &r.s,
        &r.highlights,
        r.reveal.to_bits(),
        r.streaming,
        r.tasks_read_only,
        r.selectable,
        r.scroll_wide,
        r.on_link.as_ref().map(|w| w.1),
        r.on_link_secondary.as_ref().map(|w| w.1),
    ))
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

fn hash_span(sp: &Span, h: &mut impl Hasher) {
    (
        sp.start,
        sp.end,
        sp.weight.map(f32::to_bits),
        sp.mono,
        sp.size.map(f32::to_bits),
        sp.color,
        sp.bg,
        sp.bg_pad.to_bits(),
        sp.underline,
        sp.italic,
    )
        .hash(h);
}

fn hash_align(a: Align, h: &mut impl Hasher) {
    match a {
        Align::Start => 0u8,
        Align::Center => 1u8,
        Align::End => 2u8,
    }
    .hash(h);
}

fn hash_flow(f: Flow, h: &mut impl Hasher) {
    (f.lists, f.ink, f.inset.to_bits(), f.cols).hash(h);
    match f.lead {
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
    /// Appends `inl` and any resolved link outputs (`code_link`, `link_look`, `link_ring`,
    /// `link_help`) to `out`, returning `false` when `inl` invokes a stateful closure that cannot
    /// be hashed safely (`on_link` with captures, or async `link_favicon`).
    pub(super) fn record_inline(&self, inl: &Inline, out: &mut Vec<u64>) -> bool {
        let resolved = self.with_code_links(inl);
        if resolved.links.is_empty() {
            out.push(super::render::hash_of((0u8, &*resolved)));
            return true;
        }
        if self.on_link.as_ref().is_some_and(|w| w.1.is_none())
            || self.on_link_secondary.as_ref().is_some_and(|w| w.1.is_none())
            || (self.s.link_icon.is_some() && self.link_favicon.is_some())
        {
            return false;
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (1u8, &*resolved).hash(&mut h);
        for (a, b, url) in &resolved.links {
            self.code_link_color(*a, *b, &resolved).hash(&mut h);
            if let Some(look) = &self.link_look {
                match look(url) {
                    None => 0u8.hash(&mut h),
                    Some((sp, lp)) => {
                        1u8.hash(&mut h);
                        hash_span(&sp, &mut h);
                        format!("{lp:?}").hash(&mut h);
                    }
                }
            }
            if let Some(ring) = &self.link_ring {
                ring(url).map(|(c, w)| (c, w.to_bits())).hash(&mut h);
            }
            if let Some(help) = &self.link_help {
                help(url).hash(&mut h);
            }
        }
        out.push(h.finish());
        true
    }

    fn record_block(&self, b: &Block, out: &mut Vec<u64>) -> bool {
        match b {
            Block::Para(inl) => {
                out.push(1);
                self.record_inline(inl, out)
            }
            Block::Heading(level, inl) => {
                out.push(2 | (u64::from(*level) << 8));
                self.record_inline(inl, out)
            }
            Block::Code(code) => {
                if !self.highlighter.is_builtin() || self.code_frame.is_some() {
                    return false;
                }
                out.push(super::render::hash_of((3u8, code)));
                true
            }
            Block::Diff(src) => {
                out.push(super::render::hash_of((4u8, src)));
                true
            }
            Block::Mermaid(src) => {
                #[cfg(feature = "mermaid")]
                if self.diagram_frame.is_some() {
                    return false;
                }
                if !self.highlighter.is_builtin() {
                    return false;
                }
                out.push(super::render::hash_of((5u8, src)));
                true
            }
            Block::Rule => {
                out.push(6);
                true
            }
            Block::Image { src, alt, title } => {
                if self.images.is_some() || self.on_image.is_some() {
                    return false;
                }
                out.push(super::render::hash_of((7u8, src, alt, title)));
                true
            }
            Block::Quote(inner) => {
                if self.quote_frame.is_some() {
                    return false;
                }
                out.push(super::render::hash_of((8u8, inner.len())));
                inner.iter().all(|c| self.record_block(c, out))
            }
            Block::List { start, items } => {
                out.push(super::render::hash_of((9u8, start, items.len())));
                for item in items {
                    out.push(item.len() as u64);
                    if !item.iter().all(|c| self.record_block(c, out)) {
                        return false;
                    }
                }
                true
            }
            Block::Table(t) => {
                if t.open && self.streaming {
                    return false;
                }
                let mut h = std::collections::hash_map::DefaultHasher::new();
                (10u8, t.open, t.align.len(), t.head.len(), t.rows.len()).hash(&mut h);
                for a in &t.align {
                    hash_align(*a, &mut h);
                }
                out.push(h.finish());
                if !t.head.iter().all(|c| self.record_inline(c, out)) {
                    return false;
                }
                for row in &t.rows {
                    out.push(row.len() as u64);
                    if !row.iter().all(|c| self.record_inline(c, out)) {
                        return false;
                    }
                }
                true
            }
        }
    }

    pub(super) fn settled_block_digest(
        &self,
        b: &Block,
        map: &BlockMap,
        key: &str,
        top: f32,
        dir: Option<rune::Direction>,
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
        let mut content = Vec::with_capacity(8);
        if !self.record_block(b, &mut content) {
            return None;
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        hash_flow(self.flow.get(), &mut h);
        for s in srcs {
            (&s.lang, s.len, s.coloured, s.open).hash(&mut h);
        }
        tasks.hash(&mut h);
        if !self.highlights.is_empty() {
            hash_block_map(map, &mut h);
        }
        Some(rune::memo::digest(&(1u8, self.base_hash(), key, top.to_bits(), dir, content, h.finish())))
    }

    pub(super) fn settled_item_digest(
        &self,
        item: &[Block],
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
        let mut content = Vec::with_capacity(item.len() * 2 + 2);
        for b in item {
            if !self.record_block(b, &mut content) {
                return None;
            }
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
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
        Some(rune::memo::digest(&(2u8, self.base_hash(), content, h.finish())))
    }

    pub(super) fn settled_code_card_digest(&self, code: &str, source: Option<&Source>, map: Option<&SpanMap>) -> Option<(u64, u64)> {
        if !self.highlighter.is_builtin() || source.is_some_and(|s| s.open) {
            return None;
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
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
        Some(rune::memo::digest(&(3u8, self.base_hash(), code, h.finish())))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn settled_table_row_digest(
        &self,
        cells: &[Inline],
        maps: &[SpanMap],
        align: &[Align],
        table_dir: Option<rune::Direction>,
        natural: &[f32],
        min: &[f32],
        widths: Option<&[f32]>,
        font: TextStyle,
        color: Color,
        pad: [f32; 2],
        cell_role: &'static str,
        key: &str,
        extra: u64,
    ) -> Option<(u64, u64)> {
        let mut content = Vec::with_capacity(cells.len() + 1);
        for c in cells {
            if !self.record_inline(c, &mut content) {
                return None;
            }
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (cell_role, key, extra, font, color, pad.map(f32::to_bits), table_dir).hash(&mut h);
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
        Some(rune::memo::digest(&(4u8, self.base_hash(), content, h.finish())))
    }
}
