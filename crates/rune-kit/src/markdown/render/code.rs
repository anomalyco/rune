//! Monospace and fenced blocks: code blocks, syntax spans, diff fences, Mermaid diagrams, and
//! deeply nested plain tails.

use super::super::*;
use super::Renderer;

/// Shell languages, which [`MarkdownStyle::shell_code`] styles apart (`markdown.tsx`'s list).
const SHELLS: [&str; 7] = ["bash", "sh", "shell", "zsh", "fish", "console", "terminal"];
/// Plain-text fence names, which [`MarkdownStyle::plain_code_text`] colours as a fence with none.
const PLAIN_TEXT: [&str; 4] = ["text", "txt", "plain", "plaintext"];

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
    pub(in crate::markdown) fn code(&self, code: &str, source: Option<&Source>, map: Option<&SpanMap>) -> El {
        let s = &self.s;
        let highlights = self.text_highlights_for(code, map);
        let shell = s.shell.filter(|_| source.and_then(|src| src.lang.as_deref()).is_some_and(|l| SHELLS.iter().any(|sh| l.eq_ignore_ascii_case(sh))));
        let plain = source.and_then(|src| src.lang.as_deref()).is_none_or(|l| PLAIN_TEXT.iter().any(|p| l.eq_ignore_ascii_case(p)));
        let (fill, border, ink) = match shell {
            Some(sh) => (sh.fill, sh.border, sh.text),
            None if plain => (s.code_block_fill, s.code_block_border, s.plain_code_text.or(s.code_text).unwrap_or(s.text)),
            None => (s.code_block_fill, s.code_block_border, s.code_text.unwrap_or(s.text)),
        };
        let spans = if shell.is_some() { Vec::new() } else { self.syntax_spans(code, source) };
        let body = self.with_reveal(tabbed(code, spans, highlights, s.code_block).font(s.code_block).color(ink).by_line()).copy_breaks(2, 2);
        // `.highlights`' `border: 0.5px` takes its device pixel each side, inside which the padding sits.
        // Code reads left to right in any document, as `markdown.css` isolates it.
        let frame = col().role("code").dir(rune::Direction::Ltr).w_full().radius(6.0).fill(fill).border(0.5, border).border_layout();
        if self.scroll_wide {
            // The text sizes to its longest line inside the viewport, which scrolls under the
            // padding so lines run to the frame's edge.
            let viewport = col().key("scroll").w_full().scroll_x();
            let viewport = if s.wide_fade > 0.0 { viewport.scroll_edge_fade(s.wide_fade) } else { viewport };
            return frame.child(viewport.child(col().px(s.code_block_pad[0]).py(s.code_block_pad[1]).child(self.select(body))));
        }
        frame.px(s.code_block_pad[0]).py(s.code_block_pad[1]).child(self.select(body.w_full()))
    }

    /// Colour-only spans, so highlighting never moves a glyph: a block streaming in draws its
    /// newest line plain and colours it in place once the line ends.
    pub(in crate::markdown) fn syntax_spans(&self, code: &str, source: Option<&Source>) -> Vec<Span> {
        let Some(src) = source.filter(|_| self.s.highlight) else { return Vec::new() };
        let lang = src.lang.as_deref();
        // The built-in highlighter leaves other languages plain; skip the cache for those.
        if self.highlighter.is_builtin() && !lang.is_some_and(crate::syntax::known) {
            return Vec::new();
        }
        let Some(prefix) = code.get(..src.coloured) else { return Vec::new() };
        self.s.syntax.spans(&self.highlighter.highlight(lang, prefix), 0)
    }

    /// One row per line, so added and removed lines are tinted edge to edge.
    pub(in crate::markdown) fn diff(&self, src: &str, map: Option<&SpanMap>) -> El {
        let s = &self.s;
        let all_hl = self.text_highlights_for(src, map);
        let mut c = col().role("code").dir(rune::Direction::Ltr).w_full().py(s.code_block_pad[1]).radius(6.0).fill(s.code_block_fill).border(0.5, s.code_block_border).border_layout();
        let mut line_start = 0usize;
        let last = src.split('\n').count() - 1;
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
                .child(self.select(self.with_reveal(tabbed(shown, Vec::new(), line_hl, s.code_block).font(s.code_block).color(color).wrap()).w_full()
                    .copy_breaks(if n == 0 { 2 } else { 1 }, if n == last { 2 } else { 1 })));
            if let Some(t) = tint {
                r = r.fill(t);
            }
            c = c.child(r);
        }
        c
    }

    /// A diagram, drawn from its complete lines while its fence is still open; until any read
    /// (or for a type it doesn't draw), its source as a code block. While [`Renderer::streaming`],
    /// a fence whose header may yet draw shows nothing until it does, so the source never
    /// flashes before the diagram.
    pub(in crate::markdown) fn mermaid(&self, src: &str, source: Option<&Source>, map: Option<&SpanMap>, index: usize) -> El {
        #[cfg(not(feature = "mermaid"))]
        let _ = index;
        #[cfg(feature = "mermaid")]
        {
            let open = source.is_some_and(|s| s.open);
            let lines = if open { &src[..source.map_or(src.len(), |s| s.coloured.min(src.len()))] } else { src };
            // The card sits in a bare holder, so a fence that draws nothing yet holds its place
            // and the card mounts into it (fading in) rather than the block turning into one.
            let holder = || col().w_full().transition(rune::Transition::none());
            if let Some(d) = crate::mermaid::diagram(lines, &self.s.diagram, self.streaming, open) {
                let card = d.key("card");
                let card = match &self.diagram_frame {
                    Some(f) => f(card, &DiagramFrame { index, source: lines, open }),
                    None => card,
                };
                return holder().child(card);
            }
            if open && self.streaming && crate::mermaid::may_draw(lines) {
                return holder();
            }
            col().w_full().child(self.code(src, source, map).key("source"))
        }
        #[cfg(not(feature = "mermaid"))]
        self.code(src, source, map)
    }

    pub(super) fn deep_tail(&self, inl: &Inline, lines: usize, map: &SpanMap) -> El {
        const CHUNK_CAP: usize = 32 * 1024;
        let s = &self.s;
        let flow = self.flow.get();
        let pad = " ".repeat(flow.cols);
        let label = rune::labels::label(rune::Label::MoreNestedLines(lines));
        let header = el()
            .key("more")
            .role("note")
            .label(label.clone())
            .w_full()
            .child(self.with_reveal(text(label)).font(s.code_block).color(s.muted).w_full().a11y_hidden());
        let all_hl = self.text_highlights_for(&inl.text, Some(map));
        let ink = flow.ink.unwrap_or(s.text);
        let mut card = col()
            .key("deep-tail")
            .w_full()
            .gap(4.0)
            .px(10.0)
            .py(8.0)
            .radius(6.0)
            .fill(s.code_block_fill)
            .border(0.5, s.code_block_border)
            .border_layout()
            .child(header);

        let full = inl.text.as_str();
        let mut start = 0usize;
        let mut chunk_idx = 0usize;
        while start < full.len() || chunk_idx == 0 {
            let max_end = (start + CHUNK_CAP).min(full.len());
            let end = if max_end == full.len() {
                full.len()
            } else if let Some(rel_nl) = full.as_bytes()[start..max_end].iter().rposition(|&b| b == b'\n') {
                start + rel_nl
            } else {
                let mut boundary = max_end;
                while boundary > start && !full.is_char_boundary(boundary) {
                    boundary -= 1;
                }
                if boundary > start {
                    boundary
                } else {
                    let mut fwd = max_end + 1;
                    while fwd < full.len() && !full.is_char_boundary(fwd) {
                        fwd += 1;
                    }
                    fwd
                }
            };
            let slice = &full[start..end];
            let chunk_hl: Vec<TextHighlight> = all_hl
                .iter()
                .filter_map(|h| {
                    let r = h.range();
                    let a = r.start.max(start);
                    let b = r.end.min(end);
                    (a < b).then(|| h.clone().with_range((a - start)..(b - start)))
                })
                .collect();
            let is_last = end >= full.len();
            let node = self
                .select(
                    self.with_reveal(tabbed(slice, Vec::new(), chunk_hl, s.code_block).font(s.code_block).color(ink).wrap())
                        .w_full()
                        .copy_prefix(pad.clone())
                        .copy_breaks(if chunk_idx == 0 { 2 } else { 1 }, if is_last { 2 } else { 1 }),
                )
                .key(format!("c{chunk_idx}"));
            card = card.child(node);
            chunk_idx += 1;
            if is_last {
                break;
            }
            start = if full.as_bytes().get(end) == Some(&b'\n') { end + 1 } else { end };
        }
        card
    }
}
