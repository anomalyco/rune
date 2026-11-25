//! Iterative block and container rendering: top-level blocks, block quotes, lists, tasks, and
//! leaf block dispatch.

use super::super::*;
use super::{Flow, Lead, Renderer, collapsed, collapsed_bottom, collapsed_top, rendered_keys};

enum RenderFrame<'a> {
    Blocks {
        blocks: &'a [Block],
        pre: Option<&'a [Rc<memo::PreBlock>]>,
        maps: &'a [BlockMap],
        names: Vec<String>,
        top_level: bool,
        in_item: bool,
        open_tail: bool,
        allow_memo: bool,
        idx: usize,
        prev_mb: f32,
        after_heading: bool,
        last_dir: Option<rune::Direction>,
        col: El,
        pending: Option<(String, f32, bool, bool, Option<rune::Direction>)>,
    },
    Quote {
        quote_block: &'a Block,
        inner: &'a [Block],
        inner_maps: &'a [BlockMap],
        saved_flow: Flow,
        first_task: usize,
    },
    List {
        list_block: &'a Block,
        start: Option<u64>,
        items: &'a [Vec<Block>],
        pre_items: Option<&'a [memo::PreItem]>,
        item_maps: &'a [Vec<BlockMap>],
        saved_flow: Flow,
        indent: f32,
        space: f32,
        open_tail: bool,
        allow_memo: bool,
        idx: usize,
        col: El,
        pending_item: Option<(usize, f32, Option<(bool, usize)>, bool)>,
    },
}

impl Renderer {
    /// `blocks` in a column, spaced by their collapsed margins. A heading's bottom margin alone
    /// sets the gap under it (CSS's `h2 + p { margin-top: 0 }`). In a list item (`in_item`) the
    /// first paragraph has no margins, a later one 8 above, and a nested list 4 above and below,
    /// as a web page spaces an `li`'s children.
    pub(in crate::markdown) fn blocks_with_dirs(
        &self,
        blocks: &[Block],
        pre: Option<&[Rc<memo::PreBlock>]>,
        maps: &[BlockMap],
        keys: &[String],
        top_level: bool,
        dirs: &bidi::Dirs,
    ) -> El {
        let mut quote_plain_cache = plain::QuotePlainCache::default();
        self.run_frames(
            vec![RenderFrame::Blocks {
                blocks,
                pre,
                maps,
                names: rendered_keys(blocks.len(), keys),
                top_level,
                in_item: false,
                open_tail: self.streaming,
                allow_memo: true,
                idx: 0,
                prev_mb: 0.0,
                after_heading: false,
                last_dir: if top_level { self.tentative } else { None },
                col: col().w_full(),
                pending: None,
            }],
            dirs,
            &mut quote_plain_cache,
        )
    }

    fn unmemoised_block(
        &self,
        b: &Block,
        pb: Option<&memo::PreBlock>,
        map: &BlockMap,
        dirs: &bidi::Dirs,
        quote_plain_cache: &mut plain::QuotePlainCache,
    ) -> El {
        match b {
            Block::Quote(inner) => {
                let inner_maps = match map { BlockMap::Quote(m) => m.as_slice(), _ => &[][..] };
                let q = self.s.quote;
                let saved = self.flow.get();
                let first_task = self.next_task.get();
                self.flow.set(Flow { ink: q.text.or(saved.ink), inset: saved.inset + q.width + q.pad, ..saved });
                self.run_frames(
                    vec![
                        RenderFrame::Quote { quote_block: b, inner, inner_maps, saved_flow: saved, first_task },
                        RenderFrame::Blocks {
                            blocks: inner,
                            pre: pb.map(|p| &*p.inner),
                            maps: inner_maps,
                            names: rendered_keys(inner.len(), &[]),
                            top_level: false,
                            in_item: false,
                            open_tail: false,
                            allow_memo: false,
                            idx: 0,
                            prev_mb: 0.0,
                            after_heading: false,
                            last_dir: None,
                            col: col().w_full(),
                            pending: None,
                        },
                    ],
                    dirs,
                    quote_plain_cache,
                )
            }
            Block::List { start, items } => {
                let item_maps = match map { BlockMap::List(m) => m.as_slice(), _ => &[][..] };
                let saved = self.flow.get();
                let indent = match self.s.list_nested {
                    Some([bullets, numbers]) if saved.lists > 0 => if start.is_some() { numbers } else { bullets },
                    _ => self.s.list_indent,
                };
                let space = if self.s.list_outside { rune::text::measure("x x", &[], self.s.body, None).0 - rune::text::measure("xx", &[], self.s.body, None).0 } else { 0.0 };
                self.run_frames(
                    vec![RenderFrame::List {
                        list_block: b,
                        start: *start,
                        items,
                        pre_items: pb.map(|p| &*p.items),
                        item_maps,
                        saved_flow: saved,
                        indent,
                        space,
                        open_tail: false,
                        allow_memo: false,
                        idx: 0,
                        col: list_container(*start),
                        pending_item: None,
                    }],
                    dirs,
                    quote_plain_cache,
                )
            }
            _ => self.leaf_block(b, pb, map),
        }
    }

    fn run_frames<'a>(
        &self,
        mut stack: Vec<RenderFrame<'a>>,
        dirs: &bidi::Dirs,
        quote_plain_cache: &mut plain::QuotePlainCache,
    ) -> El {
        const EMPTY: &BlockMap = &BlockMap::None;
        let mut ret: Option<El> = None;

        while let Some(top_frame) = stack.last_mut() {
            match top_frame {
                RenderFrame::Blocks { blocks, pre, maps, names, top_level, in_item, open_tail, allow_memo, idx, prev_mb, after_heading, last_dir, col: cur_col, pending } => {
                    if let Some(child_el) = ret.take() {
                        let (key, top, prose, cap_prose, own) = pending.take().unwrap();
                        let drawn = if prose { bidi::directed(child_el, own.or(*last_dir)) } else { child_el };
                        let drawn = cap_prose_width(drawn, self.s.prose_width.filter(|_| cap_prose));
                        if own.is_some() { *last_dir = own; }
                        *cur_col = std::mem::replace(cur_col, el()).child(drawn.key(key).mt(top));
                        continue;
                    }
                    let mut descended = false;
                    let blocks_slice = *blocks;
                    let pre_slice = *pre;
                    let maps_slice = *maps;
                    while *idx < blocks_slice.len() {
                        let i = *idx;
                        *idx += 1;
                        let b = &blocks_slice[i];
                        if self.streaming && matches!(b, Block::Code(c) if c.is_empty()) {
                            let n = self.next_source.get();
                            if self.sources.get(n).is_some_and(|s| s.open && s.len == 0) {
                                self.next_source.set(n + 1);
                                continue;
                            }
                        }
                        let (mt, mb) = collapsed(b, &self.s, i, *in_item);
                        let top = match b {
                            _ if i == 0 => 0.0,
                            Block::Heading(..) | Block::Rule | Block::Image { .. } => mt.max(*prev_mb),
                            _ if *after_heading => *prev_mb,
                            _ => mt.max(*prev_mb),
                        };
                        *prev_mb = mb;
                        *after_heading = matches!(b, Block::Heading(..));
                        let key = std::mem::take(&mut names[i]);
                        if *top_level { self.current_block_key.replace(key.clone().into()); }
                        if i > 0 { self.flow.set(Flow { lead: None, ..self.flow.get() }); }
                        let own = dirs.of(b);
                        let prose = !matches!(b, Block::Code(_) | Block::Diff(_) | Block::Mermaid(_) | Block::Image { .. } | Block::Rule);
                        let cap_prose = *top_level && is_prose_block(b);
                        let dir = prose.then(|| own.or(*last_dir)).flatten();
                        let map = maps_slice.get(i).unwrap_or(EMPTY);
                        let pb = pre_slice.and_then(|ps| ps.get(i)).map(Rc::as_ref);
                        let child_open_tail = *open_tail && i + 1 == blocks_slice.len();
                        let is_deep_tail = matches!((b, map), (Block::Para(_), BlockMap::DeepTail { .. }));
                        let can_memo_here = *allow_memo && !child_open_tail && !(self.streaming && is_deep_tail);
                        if can_memo_here {
                            let (src_at, src_n) = (self.next_source.get(), pb.map_or_else(|| memo::source_count(b), |p| p.src_n as usize));
                            let (task_at, task_n) = (self.next_task.get(), pb.map_or_else(|| memo::task_count(b), |p| p.task_n as usize));
                            if let Some(digest) = self.settled_block_digest(b, pb, map, &key, top, dir, cap_prose, (src_at, src_n), (task_at, task_n)) {
                                let key_c = key.clone();
                                let cached_el = rune::memo::cached(digest, || {
                                    memo::note_block_build();
                                    self.next_source.set(src_at);
                                    self.next_task.set(task_at);
                                    let mut sub_qpc = plain::QuotePlainCache::default();
                                    let child_el = self.unmemoised_block(b, pb, map, dirs, &mut sub_qpc);
                                    debug_assert_eq!(self.next_source.get(), src_at + src_n);
                                    debug_assert_eq!(self.next_task.get(), task_at + task_n);
                                    let drawn = if prose { bidi::directed(child_el, dir) } else { child_el };
                                    cap_prose_width(drawn, self.s.prose_width.filter(|_| cap_prose)).key(key_c).mt(top)
                                });
                                self.next_source.set(src_at + src_n);
                                self.next_task.set(task_at + task_n);
                                if own.is_some() { *last_dir = own; }
                                *cur_col = std::mem::replace(cur_col, el()).child(cached_el);
                                continue;
                            }
                        }
                        let am = *allow_memo;
                        match b {
                            Block::Quote(inner) => {
                                *pending = Some((key, top, prose, cap_prose, own));
                                let inner_maps = match map { BlockMap::Quote(m) => m.as_slice(), _ => &[][..] };
                                let q = self.s.quote;
                                let saved = self.flow.get();
                                let first_task = self.next_task.get();
                                self.flow.set(Flow { ink: q.text.or(saved.ink), inset: saved.inset + q.width + q.pad, ..saved });
                                stack.push(RenderFrame::Quote { quote_block: b, inner, inner_maps, saved_flow: saved, first_task });
                                stack.push(RenderFrame::Blocks { blocks: inner, pre: pb.map(|p| &*p.inner), maps: inner_maps, names: rendered_keys(inner.len(), &[]), top_level: false, in_item: false, open_tail: child_open_tail, allow_memo: am, idx: 0, prev_mb: 0.0, after_heading: false, last_dir: None, col: col().w_full(), pending: None });
                                descended = true;
                                break;
                            }
                            Block::List { start, items } => {
                                *pending = Some((key, top, prose, cap_prose, own));
                                let item_maps = match map { BlockMap::List(m) => m.as_slice(), _ => &[][..] };
                                let saved = self.flow.get();
                                let indent = match self.s.list_nested {
                                    Some([bullets, numbers]) if saved.lists > 0 => if start.is_some() { numbers } else { bullets },
                                    _ => self.s.list_indent,
                                };
                                let space = if self.s.list_outside { rune::text::measure("x x", &[], self.s.body, None).0 - rune::text::measure("xx", &[], self.s.body, None).0 } else { 0.0 };
                                stack.push(RenderFrame::List { list_block: b, start: *start, items, pre_items: pb.map(|p| &*p.items), item_maps, saved_flow: saved, indent, space, open_tail: child_open_tail, allow_memo: am, idx: 0, col: list_container(*start), pending_item: None });
                                descended = true;
                                break;
                            }
                            _ => {
                                let child_el = self.leaf_block(b, pb, map);
                                let drawn = if prose { bidi::directed(child_el, dir) } else { child_el };
                                let drawn = cap_prose_width(drawn, self.s.prose_width.filter(|_| cap_prose));
                                if own.is_some() { *last_dir = own; }
                                *cur_col = std::mem::replace(cur_col, el()).child(drawn.key(key).mt(top));
                            }
                        }
                    }
                    if !descended {
                        let Some(RenderFrame::Blocks { col: done, .. }) = stack.pop() else { unreachable!() };
                        ret = Some(done);
                    }
                }
                RenderFrame::Quote { .. } => {
                    let Some(RenderFrame::Quote { quote_block, inner, inner_maps, saved_flow, first_task }) = stack.pop() else { unreachable!() };
                    let drawn = ret.take().unwrap();
                    self.flow.set(saved_flow);
                    let q = self.s.quote;
                    let quote = bidi::directed(
                        row().role("blockquote").w_full()
                            .child(el().w(q.width).h_full().radius((q.width / 2.0).min(1.0)).fill(self.s.quote_rule))
                            .child(drawn.shrink().pl(q.pad)),
                        dirs.of(quote_block),
                    );
                    ret = Some(match &self.quote_frame {
                        Some(f) => {
                            let txt = self.plain_text_rc(quote_block, inner, inner_maps, first_task, Flow { lead: None, ..saved_flow }, quote_plain_cache);
                            f(quote, &QuoteFrame { text: &txt })
                        }
                        None => quote,
                    });
                }
                RenderFrame::List { start, items, pre_items, item_maps, saved_flow, indent, space, open_tail, allow_memo, idx, col: list_col, pending_item, .. } => {
                    let n = items.len();
                    let real_n = n - usize::from(n > 0 && matches!(item_maps.last().map(Vec::as_slice), Some([BlockMap::DeepTail { .. }])));
                    if let Some(item_body) = ret.take() {
                        let (k, gap_mt, task, is_deep_tail) = pending_item.take().unwrap();
                        if is_deep_tail {
                            *list_col = std::mem::replace(list_col, el()).child(
                                row().key(format!("i{k}")).w_full().mt(gap_mt).pl(*indent).child(item_body.shrink()),
                            );
                        } else if let Some((checked, marker_offset)) = task {
                            *list_col = std::mem::replace(list_col, el()).child(self.task_item_body((k, real_n), *indent, &items[k], item_body, marker_offset, checked).mt(gap_mt));
                        } else {
                            let marker = self.with_reveal(self.list_marker(*start, k, *indent, *space));
                            let marker = if start.is_some() { marker.role("list-marker") } else { marker.a11y_hidden() };
                            *list_col = std::mem::replace(list_col, el()).child(
                                row().key(format!("i{k}")).role("listitem").position_in_set(k + 1, real_n).w_full().mt(gap_mt)
                                    .child(marker)
                                    .child(item_body.shrink()),
                            );
                        }
                        continue;
                    }
                    if *idx < n {
                        let k = *idx;
                        *idx += 1;
                        let items_slice = *items;
                        let pre_items_slice = *pre_items;
                        let imaps_slice = *item_maps;
                        let item = &items_slice[k];
                        let ipre = pre_items_slice.and_then(|ps| ps.get(k));
                        let imap = imaps_slice.get(k).map_or(&[][..], Vec::as_slice);
                        let is_deep_tail = matches!(imap, [BlockMap::DeepTail { .. }]);
                        let item_open_tail = *open_tail && k + 1 == n;
                        let am = *allow_memo;
                        let gap_mt = list_item_gap(&self.s, items_slice, k);
                        let task_idx = self.next_task.get();
                        self.next_task.set(task_idx + 1);
                        let sf = *saved_flow;
                        let ind = *indent;
                        let sp = *space;
                        let st = *start;
                        if is_deep_tail {
                            self.flow.set(Flow { lists: sf.lists + 1, inset: sf.inset + ind, cols: sf.cols, lead: None, ..sf });
                            *pending_item = Some((k, gap_mt, None, true));
                            stack.push(RenderFrame::Blocks { blocks: item, pre: ipre.map(|p| &*p.inner), maps: imap, names: rendered_keys(item.len(), &[]), top_level: false, in_item: true, open_tail: item_open_tail, allow_memo: false, idx: 0, prev_mb: 0.0, after_heading: false, last_dir: None, col: col().w_full(), pending: None });
                        } else {
                            let task = self.tasks.get(task_idx).copied().flatten();
                            let lead = match (task, st) {
                                (Some((checked, _)), _) => Lead::Task(checked),
                                (None, Some(num)) => Lead::Number(num + k as u64),
                                (None, None) => Lead::Bullet,
                            };
                            let item_inset = if task.is_some() {
                                ind.max(self.s.task.left + self.s.task.size)
                            } else {
                                ind
                            };
                            self.flow.set(Flow { lists: sf.lists + 1, inset: sf.inset + item_inset, cols: sf.cols + lead.width(), lead: Some((sf.cols, lead, if k == 0 && sf.lists == 0 { 2 } else { 1 })), ..sf });
                            if task.is_none() && am && !item_open_tail {
                                let (src_at, src_n) = (self.next_source.get(), ipre.map_or_else(|| item.iter().map(memo::source_count).sum::<usize>(), |p| p.src_n as usize));
                                let (task_at, task_n) = (self.next_task.get(), ipre.map_or_else(|| item.iter().map(memo::task_count).sum::<usize>(), |p| p.task_n as usize));
                                if let Some(digest) = self.settled_item_digest(item, ipre, imap, (src_at, src_n), (task_at, task_n)) {
                                    let body = rune::memo::cached(digest, || {
                                        memo::note_block_build();
                                        self.next_source.set(src_at);
                                        self.next_task.set(task_at);
                                        let mut sub_qpc = plain::QuotePlainCache::default();
                                        let b = self.run_frames(
                                            vec![RenderFrame::Blocks { blocks: item, pre: ipre.map(|p| &*p.inner), maps: imap, names: rendered_keys(item.len(), &[]), top_level: false, in_item: true, open_tail: false, allow_memo: false, idx: 0, prev_mb: 0.0, after_heading: false, last_dir: None, col: col().w_full(), pending: None }],
                                            dirs,
                                            &mut sub_qpc,
                                        ).shrink();
                                        debug_assert_eq!(self.next_source.get(), src_at + src_n);
                                        debug_assert_eq!(self.next_task.get(), task_at + task_n);
                                        b
                                    });
                                    self.next_source.set(src_at + src_n);
                                    self.next_task.set(task_at + task_n);
                                    let marker = self.with_reveal(self.list_marker(st, k, ind, sp));
                                    let marker = if st.is_some() { marker.role("list-marker") } else { marker.a11y_hidden() };
                                    *list_col = std::mem::replace(list_col, el()).child(
                                        row().key(format!("i{k}")).role("listitem").position_in_set(k + 1, real_n).w_full().mt(gap_mt)
                                            .child(marker)
                                            .child(body),
                                    );
                                    continue;
                                }
                            }
                            *pending_item = Some((k, gap_mt, task, false));
                            stack.push(RenderFrame::Blocks { blocks: item, pre: ipre.map(|p| &*p.inner), maps: imap, names: rendered_keys(item.len(), &[]), top_level: false, in_item: true, open_tail: item_open_tail, allow_memo: am, idx: 0, prev_mb: 0.0, after_heading: false, last_dir: None, col: col().w_full(), pending: None });
                        }
                    } else {
                        let Some(RenderFrame::List { list_block, saved_flow, col: done, .. }) = stack.pop() else { unreachable!() };
                        self.flow.set(saved_flow);
                        ret = Some(bidi::directed(done, dirs.of(list_block)));
                    }
                }
            }
        }
        ret.unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    fn task_item_body(&self, (k, n): (usize, usize), indent: f32, item: &[Block], item_body: El, item_idx: usize, checked: bool) -> El {
        let t = &self.s.task;
        let interactive = self.on_task.is_some() && !self.tasks_read_only;
        let (fill, border) = if checked {
            (t.fill_on, t.border_on)
        } else {
            (t.fill_off, t.border_off)
        };
        let mut control = el()
            .key("control")
            .size(t.size, t.size)
            .ml(t.left)
            .mr((indent - t.left - t.size).max(0.0))
            .mt(t.top.unwrap_or(((self.s.body.line_height - t.size) * 0.5).max(0.0)))
            .center()
            .radius(t.radius)
            .fill(fill)
            .border(1.0, border)
            .token_motion(t.motion)
            .focus_ring(1.0, 0.0, t.focus_ring)
            .child(icon(t.check, t.size * 0.625).key("mark").color(t.mark).opacity(if checked { 1.0 } else { 0.0 }))
            .a11y_hidden();
        if interactive {
            control = control.hover(t.hover);
        }
        let mut r = row()
            .key(format!("i{k}"))
            .role("checkbox")
            .label(item_label(item))
            .checked(checked)
            .read_only(!interactive)
            .position_in_set(k + 1, n)
            .focus_selected(checked)
            .focusable(interactive)
            .w_full()
            .child(control)
            .child(item_body.shrink());
        if let Some(on_task) = self.on_task.as_ref().filter(|_| !self.tasks_read_only) {
            let block_key = self.current_block_key.borrow().clone();
            r = on_task(r.cursor(Cursor::Pointer), block_key, item_idx, !checked);
        }
        r
    }

    fn list_marker(&self, start: Option<u64>, k: usize, indent: f32, space: f32) -> El {
        let s = &self.s;
        if start.is_none() && s.list_outside {
            el().w(indent).h(s.body.line_px()).child(el().outside_list_disc(s.body).fill(s.muted))
        } else {
            let words = start.map_or_else(|| "•".into(), |num| format!("{}.", num + k as u64));
            // CSS Lists Level 3 / Blink UA stylesheet sets `::marker { font-variant-numeric: tabular-nums; }`
            // so `1.` aligns with `2.` and `3.`.
            let font = if start.is_some() { TextStyle { features: table::with_tnum(s.body.features), ..s.body } } else { s.body };
            let marker = text(words).font(font).color(s.muted).exact_lines().w(indent);
            if s.list_outside { marker.align(Align::End).pr(space) } else { marker.align(Align::Center) }
        }
    }

    fn leaf_block(&self, b: &Block, pre: Option<&memo::PreBlock>, map: &BlockMap) -> El {
        let s = &self.s;
        if let (Block::Para(inl), BlockMap::DeepTail { lines, map: tail_map }) = (b, map) {
            return self.deep_tail(inl, *lines, tail_map);
        }
        let inl_map = match map {
            BlockMap::Inline(m) => Some(m),
            _ => None,
        };
        let code_map = match map {
            BlockMap::Code(m) | BlockMap::Diff { map: m, .. } => Some(m),
            _ => None,
        };
        match b {
            Block::Para(inl) => self.para(inl, inl_map, s.body, self.flow.get().ink.unwrap_or(s.text)),
            Block::Heading(l, inl) => self.para(inl, inl_map, s.headings[(*l as usize).clamp(1, 4) - 1], if *l >= 4 { s.muted } else { s.text }).role("heading").level(u32::from(*l)),
            Block::Code(code) => {
                let n = self.next_source.get();
                self.next_source.set(n + 1);
                let source = self.sources.get(n).filter(|src| src.len == code.len());
                #[cfg(feature = "math")]
                if !source.is_some_and(|s| s.open)
                    && crate::math::is_math_fence(source.and_then(|s| s.lang.as_deref()))
                {
                    let ink = self.flow.get().ink.unwrap_or(self.s.text);
                    if let crate::math::MathStatus::Ready(rendered) =
                        crate::math::poll_or_enqueue(code, self.s.body.size, ink, self.s.math_scale)
                    {
                        let avail = self.s.text_width.map(|w| (w - self.flow.get().inset).max(0.0));
                        let digest = self.settled_math_card_digest(code, ink, avail, rendered.id);
                        return rune::memo::cached(digest, || {
                            memo::note_block_build();
                            crate::math::view(code, &rendered, self.s.wide_fade, avail, self.scroll_wide)
                        });
                    }
                }
                let wrapped = self.wraps_code(n, code);
                match self.code_frame.as_ref().and_then(|w| w.frame.as_ref()) {
                    Some(f) => {
                        let card = match self.settled_code_card_digest(code, source, code_map, wrapped) {
                            Some(digest) => rune::memo::cached(digest, || {
                                memo::note_block_build();
                                self.code(code, source, code_map, wrapped).key("card")
                            }),
                            None => self.code(code, source, code_map, wrapped).key("card"),
                        };
                        f(card, &CodeFrame {
                            index: n,
                            source: code,
                            language: source.and_then(|s| s.lang.as_deref()),
                            open: source.is_some_and(|s| s.open),
                            wrapped,
                            digest: pre.map(|p| p.digest),
                            font: self.s.code_block,
                            text_width: self.s.text_width,
                            inset: self.flow.get().inset,
                            pad_x: self.s.code_block_pad[0],
                        })
                    }
                    None if self.code_frame.as_ref().is_some_and(|w| w.wrap.is_some()) => match self.settled_code_card_digest(code, source, code_map, wrapped) {
                        Some(digest) => rune::memo::cached(digest, || {
                            memo::note_block_build();
                            self.code(code, source, code_map, wrapped)
                        }),
                        None => self.code(code, source, code_map, wrapped),
                    },
                    None => self.code(code, source, code_map, wrapped),
                }
            }
            Block::Diff(src) => {
                let n = self.next_diff.get();
                self.next_diff.set(n + 1);
                let open = matches!(map, BlockMap::Diff { open: true, .. });
                match &self.diff_frame {
                    Some(f) => {
                        let card = match self.settled_diff_card_digest(src, open, code_map) {
                            Some(digest) => rune::memo::cached(digest, || {
                                memo::note_block_build();
                                self.diff(src, code_map).key("card")
                            }),
                            None => self.diff(src, code_map).key("card"),
                        };
                        f(card, &DiffFrame { index: n, source: src, open })
                    }
                    None => self.diff(src, code_map),
                }
            }
            Block::Mermaid(src) => {
                let n = self.next_source.get();
                self.next_source.set(n + 1);
                self.mermaid(src, self.sources.get(n).filter(|s| s.len == src.len()), code_map, n)
            }
            Block::Table(t) => {
                let (head_maps, row_maps) = match map {
                    BlockMap::Table { head, rows } => (head.as_slice(), rows.as_slice()),
                    _ => (&[][..], &[][..]),
                };
                bidi::directed(self.table(t, pre.and_then(|p| p.table.as_deref()), head_maps, row_maps), bidi::table_dir(t))
            }
            Block::Image { src, alt, title } => self.image(src, alt, title),
            Block::Rule if s.rule.a <= 0.0 => el().w_full().h(0.0),
            Block::Rule => el().w_full().h(0.5).fill(s.rule),
            Block::List { .. } | Block::Quote(_) => unreachable!(),
        }
    }
}

fn list_container(start: Option<u64>) -> El {
    let c = col().role("list").w_full();
    match start {
        Some(n) => c.list_start(n),
        None => c,
    }
}

fn is_prose_block(b: &Block) -> bool {
    matches!(b, Block::Para(_) | Block::Heading(..) | Block::List { .. } | Block::Quote(_))
}

fn cap_prose_width(el: El, width: Option<f32>) -> El {
    match width {
        Some(w) => el.max_w(w),
        None => el,
    }
}

fn list_item_gap(s: &MarkdownStyle, items: &[Vec<Block>], k: usize) -> f32 {
    if k == 0 {
        return 0.0;
    }
    let prev_edge = items[k - 1].last().map_or(0.0, |l| collapsed_bottom(l, s, items[k - 1].len() - 1, true));
    let cur_edge = items[k].first().map_or(0.0, |f| collapsed_top(f, s, 0, true));
    s.list_gap.max(prev_edge).max(cur_edge)
}

fn item_label(blocks: &[Block]) -> String {
    let mut out = String::new();
    for b in blocks {
        let text = match b {
            Block::Para(inl) | Block::Heading(_, inl) => unpadded(&inl.text),
            Block::Code(s) | Block::Diff(s) | Block::Mermaid(s) => s.clone(),
            Block::Image { alt, .. } => alt.clone(),
            _ => continue,
        };
        if !text.is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&text);
        }
    }
    out
}
