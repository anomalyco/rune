//! Monospace and fenced blocks: code blocks, syntax spans, diff fences, Mermaid diagrams, and
//! deeply nested plain tails.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

use super::super::*;
use super::{Renderer, hash_of};

type AsciiMetrics = Option<([f32; 95], f32, bool)>;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct OverflowKey {
    digest: (u64, u64),
    len: usize,
    font: u64,
    avail: u32,
    epoch: u64,
}

#[derive(Default)]
struct OverflowCache {
    map: HashMap<OverflowKey, bool>,
    order: VecDeque<OverflowKey>,
}

impl OverflowCache {
    const CAP: usize = 256;
    fn get(&self, key: &OverflowKey) -> Option<bool> {
        self.map.get(key).copied()
    }
    fn insert(&mut self, key: OverflowKey, value: bool) {
        if self.map.insert(key, value).is_none() {
            self.order.push_back(key);
            while self.order.len() > Self::CAP {
                if let Some(old) = self.order.pop_front() {
                    self.map.remove(&old);
                }
            }
        }
    }
}

thread_local! {
    static ASCII_METRICS: RefCell<HashMap<u64, AsciiMetrics>> = RefCell::new(HashMap::new());
    static OVERFLOW_CACHE: RefCell<OverflowCache> = RefCell::new(OverflowCache::default());
}

/// Cached printable-ASCII advances, widest advance, and `fixed_pitch` for `(font, font_epoch)`,
/// sourced from `TextSystem::ascii_line` (no global mutex on hit).
fn ascii_metrics_cached(font: TextStyle, epoch: u64) -> AsciiMetrics {
    let key = hash_of((font, epoch));
    if let Some(m) = ASCII_METRICS.with_borrow(|c| c.get(&key).copied()) {
        return m;
    }
    let m = rune::text::ascii_metrics(font);
    ASCII_METRICS.with_borrow_mut(|c| {
        if c.len() >= 64 {
            c.clear();
        }
        c.insert(key, m);
    });
    m
}

/// Whether every byte of `s` is printable ASCII (`0x20..0x7f`), eight bytes at a time.
fn is_printable_ascii(s: &[u8]) -> bool {
    let (chunks, rem) = s.as_chunks::<8>();
    for &c in chunks {
        let w = u64::from_le_bytes(c);
        let low = w.wrapping_sub(0x2020_2020_2020_2020) & !w;
        let high = w.wrapping_add(0x0101_0101_0101_0101) | w;
        if (low | high) & 0x8080_8080_8080_8080 != 0 {
            return false;
        }
    }
    rem.iter().all(|b| (0x20..0x7f).contains(b))
}

fn is_printable_ascii_or_tab(s: &[u8]) -> bool {
    s.iter().all(|&b| (0x20..0x7f).contains(&b) || b == b'\t')
}

/// Measures one logical code line in points, matching `Renderer::code` (`mono_tab_stops` for
/// monospace families, `tabbed()`'s 4-column padding for non-monospace).
fn measure_code_line(line: &str, font: TextStyle, mono_family: bool) -> f32 {
    if mono_family || !line.contains('\t') {
        return rune::text::measure(line, &[], font, None).0;
    }
    let mut shown = String::with_capacity(line.len() + 16);
    let mut col = 0usize;
    for ch in line.chars() {
        if ch == '\t' {
            let pad = 3 - (col % 4);
            shown.push('\t');
            shown.extend(std::iter::repeat_n(' ', pad));
            col += pad + 1;
        } else {
            shown.push(ch);
            col += 1;
        }
    }
    rune::text::measure(&shown, &[], font, None).0
}

fn line_overflows(
    line: &str,
    font: TextStyle,
    avail: f32,
    ascii: Option<&([f32; 95], f32, bool)>,
    mono_family: bool,
    non_ascii_ub: f32,
) -> bool {
    let bytes = line.as_bytes();
    let has_tab = bytes.contains(&b'\t');
    if let Some(&(ref advance, widest, fixed_pitch)) = ascii {
        if !has_tab && is_printable_ascii(bytes) {
            if fixed_pitch {
                let col_w = advance[0];
                if col_w > 0.0 {
                    let est = bytes.len() as f32 * col_w;
                    if est > avail + col_w {
                        return true;
                    }
                    if est + col_w < avail {
                        return false;
                    }
                }
                return measure_code_line(line, font, mono_family) > avail;
            }
            if bytes.len() as f32 * widest * 1.05 + 2.0 <= avail {
                return false;
            }
            let sum: f32 = bytes.iter().map(|&b| advance[(b - 0x20) as usize]).sum();
            if sum * 1.05 + 2.0 <= avail {
                return false;
            }
            return measure_code_line(line, font, mono_family) > avail;
        }
        if fixed_pitch && mono_family && font.tracking == 0.0 && is_printable_ascii_or_tab(bytes) {
            let col_w = advance[0];
            if col_w > 0.0 {
                let mut cols = 0usize;
                for &b in bytes {
                    if b == b'\t' {
                        cols += 4 - (cols % 4);
                    } else {
                        cols += 1;
                    }
                }
                let est = cols as f32 * col_w;
                if est > avail + col_w {
                    return true;
                }
                if est + col_w < avail {
                    return false;
                }
            }
            return measure_code_line(line, font, mono_family) > avail;
        }
        if !has_tab {
            let mut ub = 0.0f32;
            for ch in line.chars() {
                let u = ch as u32;
                if (0x20..0x7f).contains(&u) {
                    ub += advance[(u - 0x20) as usize];
                } else {
                    ub += non_ascii_ub;
                }
            }
            if ub * 1.05 + 2.0 <= avail {
                return false;
            }
        }
    }
    measure_code_line(line, font, mono_family) > avail
}

/// Lazily checks whether any unwrapped line of `code` exceeds the code card's inner width at
/// device `scale`, matching `border_layout()`'s pixel-snapped 0.5 pt border (`device::border`).
pub(in crate::markdown) fn code_overflows_at(
    code: &str,
    digest: Option<(u64, u64)>,
    font: TextStyle,
    text_width: Option<f32>,
    inset: f32,
    pad_x: f32,
    scale: f32,
) -> bool {
    let Some(text_w) = text_width else { return false };
    if code.is_empty() {
        return false;
    }
    let s = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    // Match `rune::runtime::commit::device::border` + `pixel_snap::line_width` for `.border(0.5, ..)`.
    let border_side = (0.5 * s + 1.0 / 64.0).floor().max(1.0) / s;
    let avail = text_w - inset - 2.0 * pad_x - 2.0 * border_side;
    if avail <= 0.0 {
        return code.bytes().any(|b| b != b'\n' && b != b'\r');
    }
    let epoch = rune::text::font_epoch();
    let digest = digest.unwrap_or_else(|| {
        let mut h = memo::PairHasher::new();
        code.hash(&mut h);
        h.finish128()
    });
    let key = OverflowKey { digest, len: code.len(), font: hash_of(font), avail: avail.to_bits(), epoch };
    if let Some(hit) = OVERFLOW_CACHE.with_borrow(|c| c.get(&key)) {
        return hit;
    }
    let ascii = ascii_metrics_cached(font, epoch);
    let mono_family = matches!(font.family, Family::Mono | Family::SystemMono);
    let non_ascii_ub = ascii.map_or(font.size, |(_, widest, _)| widest.max(font.size)) * 2.5;
    let mut overflows = false;
    'lines: for raw in code.split('\n') {
        let trimmed = raw.strip_suffix('\r').unwrap_or(raw);
        for line in trimmed.split('\r') {
            if !line.is_empty() && line_overflows(line, font, avail, ascii.as_ref(), mono_family, non_ascii_ub) {
                overflows = true;
                break 'lines;
            }
        }
    }
    OVERFLOW_CACHE.with_borrow_mut(|c| c.insert(key, overflows));
    overflows
}

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
    pub(in crate::markdown) fn wrap_code_default(&self) -> bool {
        self.code_frame.as_ref().is_some_and(|w| w.wrap_default)
    }

    pub(in crate::markdown) fn wraps_code(&self, index: usize, code: &str) -> bool {
        match self.code_frame.as_ref().and_then(|w| w.wrap.as_ref()) {
            Some(f) => f(index, code),
            None => self.wrap_code_default(),
        }
    }

    pub(in crate::markdown) fn code(&self, code: &str, source: Option<&Source>, map: Option<&SpanMap>, wrapped: bool) -> El {
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
        let body = tabbed(code, spans, highlights, s.code_block).font(s.code_block).color(ink).by_line();
        // Under `scroll_wide`, unwrapped code sits inside `scroll_x()` with unconstrained width
        // (`max_width = None`), so `wrap_anywhere().wrap_greedy()` has zero effect while unwrapped
        // and keeps `shape_key`/`style_key` identical across wrap toggles so already-shaped lines
        // rebreak in place without reshaping.
        let body = if wrapped || self.scroll_wide {
            body.wrap_anywhere().wrap_greedy()
        } else if self.wrap_anywhere {
            body.wrap_anywhere()
        } else {
            body
        };
        let body = self.with_reveal(body).copy_breaks(2, 2);
        // `.highlights`' `border: 0.5px` takes its device pixel each side, inside which the padding sits.
        // Code reads left to right in any document, as `markdown.css` isolates it.
        let frame = col().role("code").dir(rune::Direction::Ltr).w_full().radius(s.code_block_radius).fill(fill).border(0.5, border).border_layout();
        if self.scroll_wide {
            // Keep `frame -> scroll -> pad -> body` in both states so toggling wrap keeps `body`'s
            // `NodeId` (preventing word-reveal replay and selection loss), while leaving the
            // unwrapped tree identical to `scroll_wide(true)` without wrapping.
            let viewport = if wrapped {
                col().key("scroll").w_full()
            } else {
                let v = col().key("scroll").w_full().scroll_x();
                if s.wide_fade > 0.0 { v.scroll_edge_fade(s.wide_fade) } else { v }
            };
            let inner = col().px(s.code_block_pad[0]).py(s.code_block_pad[1]);
            let inner = if wrapped {
                inner.w_full().child(self.select(body.w_full()))
            } else {
                inner.child(self.select(body))
            };
            return frame.child(viewport.child(inner));
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
        let mut c = col().role("code").dir(rune::Direction::Ltr).w_full().py(s.code_block_pad[1]).radius(s.code_block_radius).fill(s.code_block_fill).border(0.5, s.code_block_border).border_layout();
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
            let mut line_text = tabbed(shown, Vec::new(), line_hl, s.code_block).font(s.code_block).color(color).wrap();
            if self.wrap_anywhere {
                line_text = line_text.wrap_anywhere();
            }
            let mut r = row()
                .key(format!("l{n}"))
                .w_full()
                .px(s.code_block_pad[0])
                .child(self.select(self.with_reveal(line_text).w_full()
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
            // Mermaid source fallback stays unwrapped; wrapping applies to regular code blocks.
            col().w_full().child(self.code(src, source, map, false).key("source"))
        }
        #[cfg(not(feature = "mermaid"))]
        // Mermaid source fallback stays unwrapped; wrapping applies to regular code blocks.
        self.code(src, source, map, false)
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
            .radius(s.code_block_radius)
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
            let mut chunk_text = tabbed(slice, Vec::new(), chunk_hl, s.code_block).font(s.code_block).color(ink).wrap();
            if self.wrap_anywhere {
                chunk_text = chunk_text.wrap_anywhere();
            }
            let node = self
                .select(
                    self.with_reveal(chunk_text)
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
