//! Blocks to elements: block keys and margins, code and diff blocks, lists, quotes, tables and
//! diagrams.

use super::*;

pub(super) struct Renderer {
    pub(super) s: MarkdownStyle,
    pub(super) highlights: Vec<(Vec<Range<usize>>, Highlight)>,
    pub(super) reveal: f32,
    pub(super) streaming: bool,
    pub(super) on_link: Option<LinkWire>,
    pub(super) on_link_secondary: Option<LinkWire>,
    pub(super) link_look: Option<LinkLook>,
    pub(super) on_task: Option<TaskWire>,
    pub(super) tasks_read_only: bool,
    pub(super) link_ring: Option<LinkRing>,
    pub(super) selectable: bool,
    /// `Markdown::scroll_wide`.
    pub(super) scroll_wide: bool,
    pub(super) highlighter: Highlighter,
    pub(super) sources: Rc<[Source]>,
    /// The next of `sources`, advanced at each [`Block::Code`] in document order.
    pub(super) next_source: Cell<usize>,
    pub(super) tasks: Rc<[Option<(bool, usize)>]>,
    /// The next of `tasks`, advanced at each list item in document order.
    pub(super) next_task: Cell<usize>,
    pub(super) current_block_key: RefCell<Rc<str>>,
}

/// The keys [`Renderer::blocks`] gives `n` blocks.
pub(super) fn rendered_keys(n: usize, keys: &[String]) -> Vec<String> {
    let mut used = Vec::new();
    (0..n).map(|i| if keys.is_empty() { format!("b{i}") } else { block_key(i, keys, &mut used) }).collect()
}

/// [`Markdown::copy_source`]: the source of the top-level blocks (keyed `names`, starting at
/// `starts`) that `sel` touches, `None` for part of one block.
pub(super) fn source_copy(src: &str, starts: &[usize], names: &[String], sel: &rune::TextSelection) -> Option<String> {
    // The longest key that is the path or its first segments: an app's key may hold a `/`.
    let block_of = |path: &str| names.iter().enumerate()
        .filter(|(_, k)| path == k.as_str() || path.strip_prefix(k.as_str()).is_some_and(|rest| rest.starts_with('/')))
        .max_by_key(|(_, k)| k.len()).map(|(b, _)| b);
    let mut touched: Option<(usize, usize)> = None;
    for t in sel.texts().iter().filter(|t| t.is_selected()) {
        let Some(b) = block_of(t.path()) else { continue };
        touched = Some(touched.map_or((b, b), |(first, _)| (first, b)));
    }
    let (first, last) = touched?;
    if first == last && !sel.texts().iter().filter(|t| block_of(t.path()) == Some(first)).all(|t| t.is_whole() || t.is_empty()) {
        return None;
    }
    if (first == 0 && last + 1 >= starts.len()) || sel.texts().iter().all(|t| t.is_whole() || t.is_empty()) {
        return Some(src.to_owned());
    }
    let start = *starts.get(first)?;
    let end = starts.get(last + 1).copied().unwrap_or(src.len());
    Some(src.get(start..end)?.trim_end().to_owned())
}

pub(super) fn block_key(i: usize, keys: &[String], used: &mut Vec<String>) -> String {
    if let Some(k) = keys.get(i).filter(|k| !k.is_empty() && !used.iter().any(|u| u == *k)) {
        used.push(k.clone());
        return k.clone();
    }
    let taken = |cand: &str, used: &[String]| {
        used.iter().any(|u| u == cand) || keys.get(i + 1..).is_some_and(|rest| rest.iter().any(|k| k == cand))
    };
    let mut cand = format!("b{i}");
    let mut n = 0usize;
    while taken(&cand, used) {
        cand = format!("b{n}");
        n += 1;
    }
    used.push(cand.clone());
    cand
}

/// (margin-top, margin-bottom) per block, collapsed like CSS margins.
pub(super) fn margins(b: &Block, s: &MarkdownStyle) -> (f32, f32) {
    match b {
        Block::Para(_) => (0.0, 12.0),
        Block::Heading(1, _) => (28.0, 12.0),
        Block::Heading(2, _) => (24.0, 10.0),
        Block::Heading(..) => (20.0, 8.0),
        Block::Code { .. } | Block::Diff(_) | Block::Mermaid(_) => (12.0, 24.0),
        Block::List { .. } => (8.0, 12.0),
        Block::Quote(_) => (16.0, 16.0),
        Block::Rule => (32.0, 32.0),
        Block::Table(_) => (s.table_margin[0], s.table_margin[1]),
    }
}

/// Code as rich text with `spans` and `highlights` (byte ranges of `code`). Text layout widens
/// tabs in a monospace `style` to four-column stops itself; in any other family it draws a tab
/// as one space, so each tab is padded with uncopied spaces to the next stop: lines line up and a
/// copy still gets the tab.
fn tabbed(code: &str, mut spans: Vec<Span>, mut highlights: Vec<TextHighlight>, style: TextStyle) -> El {
    if !code.contains('\t') || matches!(style.family, Family::Mono | Family::SystemMono) {
        return rich(code.to_owned(), spans).text_highlights(highlights);
    }
    let mut shown = String::with_capacity(code.len() + code.len() / 8);
    // (source byte, shown byte, pad, padding so far including this tab's)
    let mut tabs: Vec<(usize, usize, usize, usize)> = Vec::new();
    let mut col = 0;
    for (i, c) in code.char_indices() {
        if c == '\t' {
            let pad = 3 - col % 4;
            let total = tabs.last().map_or(0, |t| t.3) + pad;
            tabs.push((i, shown.len(), pad, total));
            shown.push('\t');
            shown.extend(std::iter::repeat_n(' ', pad));
            col += pad + 1;
            continue;
        }
        col = if c == '\n' { 0 } else { col + 1 };
        shown.push(c);
    }
    let shift = |b: u32| match tabs.partition_point(|t| (t.0 as u32) < b) {
        0 => b,
        n => b + tabs[n - 1].3 as u32,
    };
    for sp in &mut spans {
        (sp.start, sp.end) = (shift(sp.start), shift(sp.end));
    }
    for hl in &mut highlights {
        let r = hl.range();
        *hl = hl.clone().with_range(shift(r.start as u32) as usize..shift(r.end as u32) as usize);
    }
    tabs.iter().fold(rich(shown, spans).text_highlights(highlights), |el, t| el.uncopied(t.1 + 1..t.1 + 1 + t.2))
}

impl Renderer {
    pub(super) fn spans(&self, inl: &Inline, base: f32) -> Vec<Span> {
        let s = &self.s;
        let links: Vec<Span> = if inl.links.is_empty() {
            Vec::new()
        } else {
            inl.spans.iter().filter(|s| matches!(s.2, Style::Link)).map(|&(a, b, _)| self.link_span(a, b, inl)).collect()
        };
        let mut next_link = 0;
        let mut out = Vec::with_capacity(inl.spans.len());
        for &(a, b, st) in &inl.spans {
            if matches!(st, Style::Link) {
                let sp = links[next_link].clone();
                next_link += 1;
                let at = out.iter().position(|p: &Span| p.start < sp.end && sp.start < p.end).unwrap_or(out.len());
                out.insert(at, sp);
                continue;
            }
            if matches!(st, Style::Strike) {
                continue;
            }
            let in_link = links.iter().find(|l| l.start <= a as u32 && b as u32 <= l.end);
            let mut sp = Span::new(a, b);
            // Code in a link is a chip in the link's colour, not underlined as well.
            let no_line = in_link.is_some_and(|l| l.underline.is_some()).then_some(Color::TRANSPARENT);
            match st {
                Style::Code => {
                    let link_base = in_link.and_then(|l| l.size).unwrap_or(base);
                    sp.mono = true;
                    sp.weight = Some(s.inline_code_weight);
                    sp.size = Some(link_base * s.inline_code_scale);
                    sp.underline = no_line;
                    if in_link.is_none_or(|l| l.bg.is_none()) {
                        sp = sp.bg(s.code_bg, s.inline_code_pad);
                    }
                }
                Style::CodePad if no_line.is_none() => continue,
                Style::CodePad => sp.underline = no_line,
                Style::Strong => sp.weight = Some(s.strong_weight),
                Style::Em => sp.italic = true,
                Style::Strike | Style::Link => unreachable!(),
            }
            out.push(sp);
        }
        out
    }

    /// Resolves `hl` against [`MarkdownStyle`] into a [`TextHighlight`] for `range`.
    pub(super) fn make_text_highlight(&self, idx: usize, range: Range<usize>, hl: &Highlight) -> TextHighlight {
        let s = &self.s;
        let bg = hl.bg.unwrap_or(if hl.current { s.highlight_current_bg } else { s.highlight_bg });
        let fg = hl.fg.or(if hl.current { s.highlight_current_fg } else { s.highlight_fg });
        let ring = hl.ring.or(if hl.current { s.highlight_current_ring } else { s.highlight_ring });
        let key = hl.key.clone().unwrap_or_else(|| Markdown::highlight_key(idx));
        let mut th = TextHighlight::new(range, bg).key(key);
        if let Some(c) = fg {
            th = th.color(c);
        }
        if let Some((rc, rw)) = ring {
            th = th.ring(rc, rw);
        }
        th
    }

    pub(super) fn text_highlights_for(&self, text: &str, map: Option<&SpanMap>) -> Vec<TextHighlight> {
        let Some(map) = map.filter(|_| !self.highlights.is_empty()) else { return Vec::new() };
        let mut out = Vec::new();
        for (idx, (src_ranges, hl)) in self.highlights.iter().enumerate() {
            for r in map.map_ranges(src_ranges, text) {
                out.push(self.make_text_highlight(idx, r, hl));
            }
        }
        out
    }

    /// `e`, a text, selectable when the markdown is (after its links, so it shows the I-beam).
    pub(super) fn select(&self, e: El) -> El {
        if self.selectable { e.selectable(true).selection_color(self.s.selection) } else { e }
    }

    /// `e`, the text of `inl`, with its strikethroughs, highlights, links and selectable, copying
    /// without inline code's padding.
    pub(super) fn select_inline(&self, mut e: El, inl: &Inline, map: Option<&SpanMap>) -> El {
        for &(a, b, st) in &inl.spans {
            if matches!(st, Style::Strike) {
                e = e.strike(a, b, None);
            }
        }
        e = e.text_highlights(self.text_highlights_for(&inl.text, map));
        let e = self.select(self.wire_link(e, inl));
        if !self.selectable { return e; }
        inl.spans.iter().filter(|s| matches!(s.2, Style::CodePad)).fold(e, |e, s| e.uncopied(s.0..s.1))
    }

    pub(super) fn para(&self, inl: &Inline, map: Option<&SpanMap>, style: TextStyle, color: Color) -> El {
        let e = rich(inl.text.clone(), self.spans(inl, style.size)).font(style).color(color).reveal(self.reveal).w_full();
        self.select_inline(e, inl, map)
    }

    pub(super) fn blocks(&self, blocks: &[Block], maps: &[BlockMap], keys: &[String], top_level: bool) -> El {
        let mut c = col().w_full();
        let mut prev_mb = 0.0f32;
        let names = rendered_keys(blocks.len(), keys);
        let empty = BlockMap::None;
        for ((i, b), key) in blocks.iter().enumerate().zip(names) {
            let (mt, mb) = margins(b, &self.s);
            let top = if i == 0 { 0.0 } else { mt.max(prev_mb) };
            if top_level {
                self.current_block_key.replace(key.clone().into());
            }
            c = c.child(self.block(b, maps.get(i).unwrap_or(&empty)).key(key).mt(top));
            prev_mb = mb;
        }
        c
    }

    pub(super) fn code(&self, code: &str, source: Option<&Source>, map: Option<&SpanMap>) -> El {
        let s = &self.s;
        let highlights = self.text_highlights_for(code, map);
        let body = tabbed(code, self.syntax_spans(code, source), highlights, s.code_block).font(s.code_block).color(s.text).reveal(self.reveal);
        let frame = col().w_full().radius(6.0).fill(s.code_block_fill).border(0.5, s.code_block_border);
        if self.scroll_wide {
            // The text sizes to its longest line inside the viewport, which scrolls under the
            // padding so lines run to the frame's edge.
            return frame.child(col().key("scroll").w_full().scroll_x().scroll_edge_fade(12.0)
                .child(col().px(s.code_block_pad[0]).py(s.code_block_pad[1]).child(self.select(body))));
        }
        frame.px(s.code_block_pad[0]).py(s.code_block_pad[1]).child(self.select(body.w_full()))
    }

    /// Colour-only spans, so highlighting never moves a glyph: a block streaming in draws its
    /// newest line plain and colours it in place once the line ends.
    pub(super) fn syntax_spans(&self, code: &str, source: Option<&Source>) -> Vec<Span> {
        let Some(src) = source.filter(|_| self.s.highlight) else { return Vec::new() };
        let lang = src.lang.as_deref();
        // The built-in scanners leave other languages plain; skip the cache for those.
        if self.highlighter.is_builtin() && !lang.is_some_and(crate::syntax::known) {
            return Vec::new();
        }
        let Some(prefix) = code.get(..src.coloured) else { return Vec::new() };
        self.s.syntax.spans(&self.highlighter.highlight(lang, prefix), 0)
    }

    /// One row per line, so added and removed lines are tinted edge to edge.
    pub(super) fn diff(&self, src: &str, map: Option<&SpanMap>) -> El {
        let s = &self.s;
        let all_hl = self.text_highlights_for(src, map);
        let mut c = col().w_full().py(s.code_block_pad[1]).radius(6.0).fill(s.code_block_fill).border(0.5, s.code_block_border);
        let mut line_start = 0usize;
        for (n, line) in src.split('\n').enumerate() {
            let line_end = line_start + line.len();
            let line_hl: Vec<TextHighlight> = all_hl
                .iter()
                .filter_map(|h| {
                    let r = h.range();
                    let a = r.start.max(line_start);
                    let b = r.end.min(line_end);
                    (a < b).then(|| h.clone().with_range((a - line_start)..(b - line_start)))
                })
                .collect();
            line_start = line_end + 1;
            let line = line.strip_suffix('\r').unwrap_or(line);
            let header = ["@@", "diff ", "index ", "--- ", "+++ "].iter().any(|p| line.starts_with(p));
            let (tint, color) = match line.as_bytes().first() {
                _ if header => (None, s.diff_hunk),
                Some(b'+') => (Some(s.diff_add), s.diff_add_text),
                Some(b'-') => (Some(s.diff_remove), s.diff_remove_text),
                _ => (None, s.text),
            };
            let shown = if line.is_empty() { " " } else { line };
            let mut r = row()
                .key(format!("l{n}"))
                .w_full()
                .px(s.code_block_pad[0])
                .child(self.select(tabbed(shown, Vec::new(), line_hl, s.code_block).font(s.code_block).color(color).wrap().reveal(self.reveal).w_full()));
            if let Some(t) = tint {
                r = r.fill(t);
            }
            c = c.child(r);
        }
        c
    }

    #[cfg_attr(not(feature = "mermaid"), allow(clippy::unused_self))]
    pub(super) fn mermaid(&self, src: &str, map: Option<&SpanMap>) -> El {
        #[cfg(feature = "mermaid")]
        {
            match crate::mermaid::diagram(src, &self.s.diagram) {
                Some(d) => d,
                None => col()
                    .w_full()
                    .gap(6.0)
                    .child(self.code(src, None, map).key("source"))
                    .child(text("Diagram not supported").font(TextStyle { size: 12.0, line_height: 16.0, ..self.s.body }).color(self.s.muted).key("hint")),
            }
        }
        #[cfg(not(feature = "mermaid"))]
        self.code(src, None, map)
    }

    pub(super) fn task_item(&self, k: usize, item: &[Block], item_maps: &[BlockMap], item_idx: usize, checked: bool) -> El {
        let t = &self.s.task;
        let interactive = self.on_task.is_some() && !self.tasks_read_only;
        let (fill, border) = if checked {
            (t.fill_on, t.border_on)
        } else {
            (Color::TRANSPARENT, t.border_off)
        };
        let mut control = el()
            .key("control")
            .size(16.0, 16.0)
            .ml(8.0)
            .mr(8.0)
            .mt(((self.s.body.line_height - 16.0) * 0.5).max(0.0))
            .center()
            .radius(t.radius)
            .fill(fill)
            .border(1.0, border)
            .motion(t.motion)
            .focus_ring(1.0, 0.0, t.focus_ring)
            .child(icon(t.check, 10.0).key("mark").color(t.mark).opacity(if checked { 1.0 } else { 0.0 }));
        if interactive {
            control = control.hover(t.hover);
        }
        let mut r = row()
            .key(format!("i{k}"))
            .role("checkbox")
            .label(item_label(item))
            .focus_selected(checked)
            .focusable(interactive)
            .w_full()
            .child(control)
            .child(self.blocks(item, item_maps, &[], false).grow(1.0).shrink());
        if let Some(on_task) = self.on_task.as_ref().filter(|_| !self.tasks_read_only) {
            let block_key = self.current_block_key.borrow().clone();
            r = on_task(r.cursor(Cursor::Pointer), block_key, item_idx, !checked);
        }
        r
    }

    pub(super) fn block(&self, b: &Block, map: &BlockMap) -> El {
        let s = &self.s;
        let inl_map = match map {
            BlockMap::Inline(m) => Some(m),
            _ => None,
        };
        let code_map = match map {
            BlockMap::Code(m) => Some(m),
            _ => None,
        };
        match b {
            Block::Para(inl) => self.para(inl, inl_map, s.body, s.text),
            Block::Heading(l, inl) => self.para(inl, inl_map, s.headings[(*l as usize - 1).min(2)], if *l >= 4 { s.muted } else { s.text }).role("heading").level(u32::from(*l)),
            Block::Code(code) => {
                let n = self.next_source.get();
                self.next_source.set(n + 1);
                self.code(code, self.sources.get(n).filter(|src| src.len == code.len()), code_map)
            }
            Block::Diff(src) => self.diff(src, code_map),
            Block::Mermaid(src) => self.mermaid(src, code_map),
            Block::Table(t) => {
                let (head_maps, row_maps) = match map {
                    BlockMap::Table { head, rows } => (head.as_slice(), rows.as_slice()),
                    _ => (&[][..], &[][..]),
                };
                self.table(t, head_maps, row_maps)
            }
            Block::List { start, items } => {
                let item_maps = match map {
                    BlockMap::List(m) => m.as_slice(),
                    _ => &[][..],
                };
                let mut c = col().w_full().gap(s.list_gap);
                for (k, item) in items.iter().enumerate() {
                    let imap = item_maps.get(k).map_or(&[][..], Vec::as_slice);
                    let task_idx = self.next_task.get();
                    self.next_task.set(task_idx + 1);
                    if let Some((checked, marker_offset)) = self.tasks.get(task_idx).copied().flatten() {
                        c = c.child(self.task_item(k, item, imap, marker_offset, checked));
                        continue;
                    }
                    let marker = match start {
                        Some(n) => format!("{}.", n + k as u64),
                        None => "•".into(),
                    };
                    c = c.child(
                        row()
                            .key(format!("i{k}"))
                            .w_full()
                            .child(
                                text(marker).font(s.body).color(s.muted).w(s.list_indent).align(Align::Center).reveal(self.reveal),
                            )
                            .child(self.blocks(item, imap, &[], false).grow(1.0).shrink()),
                    );
                }
                c
            }
            Block::Quote(inner) => {
                let inner_maps = match map {
                    BlockMap::Quote(m) => m.as_slice(),
                    _ => &[][..],
                };
                row()
                    .w_full()
                    .child(el().w(2.0).h_full().radius(1.0).fill(s.quote_rule))
                    .child(self.blocks(inner, inner_maps, &[], false).grow(1.0).shrink().pl(12.0))
            }
            Block::Rule => el().w_full().h(0.5).fill(s.rule),
        }
    }
}

fn item_label(blocks: &[Block]) -> String {
    let mut out = String::new();
    for b in blocks {
        let text = match b {
            Block::Para(inl) | Block::Heading(_, inl) => inl.text.replace(PAD, ""),
            Block::Code(s) | Block::Diff(s) | Block::Mermaid(s) => s.clone(),
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

pub(super) fn hash_of(v: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}
