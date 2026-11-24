//! Inline text rendering: style spans, inline code padding, highlights, selection, reveal, and
//! paragraphs.

use super::super::*;
use super::{Renderer, hash_of};

/// Inline math's size against the text around it, on top of KaTeX's own 1.21 em: 1.1 em, so
/// its digits and letters stand as tall as the sans text's beside them.
#[cfg(feature = "math")]
const INLINE_MATH_SIZE: f32 = 1.1 / 1.21;

thread_local! {
    static PAD_SIZES: RefCell<std::collections::HashMap<u64, f32>> = RefCell::new(std::collections::HashMap::new());
}

/// The size that sets inline code's padding spaces (`PAD`, in the text's own `font`) to `pad`
/// points a side, so the text beside the chip sits where a page's `padding: 0 4px` puts it.
/// Solved twice: a variable font's optical size moves its spaces' widths with the size.
fn pad_size(font: TextStyle, pad: f32) -> f32 {
    let key = hash_of((font, pad.to_bits(), rune::text::font_epoch()));
    if let Some(size) = PAD_SIZES.with_borrow(|m| m.get(&key).copied()) {
        return size;
    }
    let probe = PAD.repeat(64);
    let wide = |size: f32| (rune::text::measure(&probe, &[], TextStyle { size, ..font }, None).0 / 64.0).max(0.1);
    let first = font.size * pad / wide(font.size);
    let size = first * pad / wide(first);
    PAD_SIZES.with_borrow_mut(|m| m.insert(key, size));
    size
}

impl Renderer {
    pub(in crate::markdown) fn spans(&self, inl: &Inline, font: TextStyle) -> Vec<Span> {
        let inl = &*self.with_code_links(inl);
        let s = &self.s;
        let base = font.size;
        let links: Vec<Span> = if inl.links.is_empty() {
            Vec::new()
        } else {
            inl.spans.iter().filter(|s| matches!(s.2, Style::Link)).map(|&(a, b, _)| self.link_span(a, b, inl)).collect()
        };
        let mut next_link = 0;
        let mut out = Vec::with_capacity(inl.spans.len());
        // Code keeps its own weight in bold text, as `markdown.css`'s `:not(pre) > code` sets it, so
        // its spans go after the rest: of overlapping spans, the later one's weight wins.
        let mut code = Vec::new();
        for &(a, b, st) in &inl.spans {
            if matches!(st, Style::Link) {
                let sp = links[next_link].clone();
                next_link += 1;
                let at = out.iter().position(|p: &Span| p.start < sp.end && sp.start < p.end).unwrap_or(out.len());
                out.insert(at, sp);
                continue;
            }
            if matches!(st, Style::Strike | Style::Math) {
                continue;
            }
            let in_link = links.iter().find(|l| l.start <= a as u32 && b as u32 <= l.end);
            let mut sp = Span::new(a, b);
            match st {
                Style::Code if self.code_link_color(a, b, inl).is_some() => {
                    sp.mono = true;
                    sp.weight = Some(s.inline_code_weight);
                    sp.size = Some(base * s.inline_code_scale);
                    sp.color = self.code_link_color(a, b, inl);
                    code.push(sp);
                    continue;
                }
                Style::Code => {
                    let link_base = in_link.and_then(|l| l.size).unwrap_or(base);
                    sp.mono = true;
                    sp.weight = Some(s.inline_code_weight);
                    sp.size = Some(link_base * s.inline_code_scale);
                    // Code inside a link reads as part of the link's label: it inherits the link's
                    // colour, underline and hover with no code-pill background or padding.
                    if in_link.is_none() {
                        sp.color = s.inline_code_text;
                        sp = sp.bg(s.code_bg, s.inline_code_pad);
                    }
                    code.push(sp);
                    continue;
                }
                Style::CodePad => {
                    if in_link.is_none() {
                        sp.size = Some(pad_size(font, s.inline_code_pad));
                    }
                }
                Style::Strong => sp.weight = Some(s.strong_weight),
                Style::Em => sp.italic = true,
                Style::Strike | Style::Link | Style::Math => unreachable!(),
            }
            out.push(sp);
        }
        out.extend(code);
        out
    }

    /// Resolves `hl` against [`MarkdownStyle`] into a [`TextHighlight`] for `range`.
    fn make_text_highlight(&self, idx: usize, range: Range<usize>, hl: &Highlight) -> TextHighlight {
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
        if let Some(pad) = hl.pad {
            th = th.pad(pad);
        }
        if let Some(radius) = hl.radius {
            th = th.radius(radius);
        }
        if let Some(vpad) = hl.vpad {
            th = th.vpad(vpad);
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
    pub(in crate::markdown) fn select(&self, e: El) -> El {
        if self.selectable { e.selectable(true).selection_color(self.s.selection) } else { e }
    }

    /// `e`, the text of `inl`, with its strikethroughs, highlights, links and selectable, copying
    /// without inline code's padding.
    pub(in crate::markdown) fn select_inline(&self, mut e: El, inl: &Inline, map: Option<&SpanMap>) -> El {
        for &(a, b, st) in &inl.spans {
            if matches!(st, Style::Strike) {
                e = e.strike(a, b, None);
            }
        }
        e = e.text_highlights(self.text_highlights_for(&inl.text, map));
        let e = self.select(self.wire_link(self.link_icons(e, inl), inl));
        if !self.selectable { return e; }
        inl.spans.iter().filter(|s| matches!(s.2, Style::CodePad)).fold(e, |e, s| e.uncopied(s.0..s.1))
    }

    pub(in crate::markdown) fn with_reveal(&self, e: El) -> El {
        if self.reveal >= 0.0 {
            e.reveal(self.reveal)
        } else if self.streaming {
            e.reveal(0.0)
        } else {
            e
        }
    }

    /// `e`, the text of `inl` in `font`, with each inline math drawn over its source once it has
    /// rendered in `ink`. Until then, or when it fails, the source shows as written.
    pub(in crate::markdown) fn with_math(&self, e: El, inl: &Inline, font: TextStyle, ink: Color) -> El {
        #[cfg(feature = "math")]
        {
            let mut e = e;
            for &(a, b, st) in &inl.spans {
                let Some(raw) = (st == Style::Math).then(|| inl.text.get(a..b)).flatten() else { continue };
                let size = font.size * INLINE_MATH_SIZE;
                if let crate::math::MathStatus::Ready(r) = crate::math::poll_inline(raw, size, ink, self.s.math_scale) {
                    let m = r.margin_pt;
                    let image = rune::InlineImage::new(r.pixels, r.width_pt - 2.0 * m, r.baseline_pt - m, r.height_pt - r.baseline_pt - m);
                    e = e.inline_image(a..b, image.bleed(m));
                }
            }
            e
        }
        #[cfg(not(feature = "math"))]
        {
            let _ = (inl, font, ink);
            e
        }
    }

    pub(in crate::markdown) fn para(&self, inl: &Inline, map: Option<&SpanMap>, style: TextStyle, color: Color) -> El {
        // Lines keep their own 22.4 points, as a page's do, so a long list doesn't drift a
        // point per item.
        let mut e = rich(inl.text.clone(), self.spans(inl, style)).font(style).color(color).exact_lines();
        if self.wrap_anywhere {
            e = e.wrap_anywhere();
        }
        let e = self.with_reveal(self.with_math(e, inl, style, color)).w_full();
        let e = bidi::directed(bidi::isolated(e, inl), bidi::inline_dir(inl));
        // A copy puts a blank line around a paragraph, one newline around a list item's first
        // with its marker before it.
        let e = match self.flow.get().lead {
            Some((cols, lead, before)) => e.copy_prefix(format!("{}{}", " ".repeat(cols), lead.marker())).copy_breaks(before, 1),
            None => e.copy_breaks(2, 2),
        };
        self.select_inline(e, inl, map)
    }
}
