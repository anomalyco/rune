//! `El::dir_by_paragraph`: each paragraph of a text in the direction of its own words, as CSS's
//! `unicode-bidi: plaintext` lays a block's paragraphs out.
use super::*;
use rune_text::paragraph_ranges as paragraphs;

impl El {
    /// Lays out each newline-ended line as its own `Layout`, reusing
    /// unchanged lines across commits (shaping only changed lines; per-commit keying reuses
    /// unchanged prefix and suffix keys even when layout spans are present, and long
    /// plain/colour-only ASCII interiors in a
    /// start-aligned, LTR, fixed-pitch monospace face wait as runs even when other lines carry
    /// layout spans; if the monospace slot resolves to a proportional fallback—such as on a host
    /// with no monospace font installed and none bundled—lines shape eagerly instead) while staying
    /// one selectable text element in its own direction.
    ///
    /// Silently falls back to a single layout when the text does not wrap, is truncated or
    /// line-clamped, is live (`El::live` / `El::advance_in`), is redacted, or is a shared-run
    /// cell (multi-line inputs already lay out by paragraph with greedy wrapping); a text flight
    /// landing on a `by_line` node is dropped so the text appears in place without flying.
    pub fn by_line(mut self) -> El {
        if let Some(t) = self.text.as_mut() {
            t.by_line = true;
        }
        self
    }

    /// Lays each paragraph of this text (a line a newline ends) out in the direction of its own
    /// first strong character, as CSS's `unicode-bidi: plaintext` does: right to left when that's
    /// Arabic or Hebrew, else left to right, left to right too when it has none, whatever
    /// direction the element itself lays out in. Each paragraph's lines start at its own start
    /// edge. The element's box, padding and place stay where its direction puts them.
    ///
    /// Call it last, after the text's spans, links, pills, highlights and copy settings. A text
    /// whose paragraphs all take one direction stays one text. One whose paragraphs differ
    /// becomes a column of one text per paragraph, which a selection crosses as one inside a
    /// [`El::selection_group`] and copies joined by newlines. A multi-line text field stays one
    /// field and lays each paragraph out in its own direction as it's edited, its mention pills
    /// isolated (`@src/app.ts` stays left to right on an Arabic line), as a `contenteditable`
    /// with `unicode-bidi: plaintext` does; its own direction (`El::dir(Direction::Auto)`)
    /// comes from its text outside its pills, and left to right while it's empty, its
    /// placeholder laid out in the placeholder's own. A secure, live, truncated or
    /// line-clamped text, or a one-line field, stays one text in its first paragraph's direction.
    pub fn dir_by_paragraph(mut self) -> El {
        let Some(t) = self.text.as_mut() else { return self };
        t.dir = step_text_dir(t.dir, TextDirEvent::ByParagraph);
        t.shape = None;
        if self.cold.input.is_some() {
            return self;
        }
        // Each paragraph's direction skips its isolates, and its piece takes their marks.
        t.apply_isolates();
        let paragraphs = paragraphs(&t.content);
        // A paragraph with no strong character reads the way the one before it does, and the
        // first ones the way the element does (None here, as it isn't flattened yet).
        let dirs: Vec<Option<Direction>> = paragraphs
            .iter()
            .scan(None, |last, &(a, b)| {
                *last = Direction::of(&t.content[a..b]).or(*last);
                Some(*last)
            })
            .collect();
        let whole = t.redacted || t.cold.live.is_some() || t.cold.advance_in.is_some() || !t.cold.inline_boxes.is_empty() || t.truncate || t.truncate_fade || t.max_lines > 0;
        if whole || dirs.iter().all(|&d| d == dirs[0]) {
            return self;
        }
        let Some(t) = self.text.take() else { return self };
        let cursor = self.handlers.cursor;
        // The I-beam `El::selectable` gave stays its own on every piece, so an
        // `El::selection_enabled(false)` subtree still takes it back; the column, which keeps no
        // text, drops it.
        let given = t.flags.has(crate::el::text_spec::TextFlags::SELECTABLE_CURSOR) && cursor == Some(Cursor::Text);
        if given { self.handlers.cursor = None; }
        let a11y_disabled = self.cold.a11y.disabled;
        let focus_disabled = self.cold.focus.disabled;
        let focus_opt_out = self.cold.focus.opt_out;
        let last = paragraphs.len() - 1;
        let pieces: Vec<El> = paragraphs.iter().enumerate().map(|(k, &range)| {
            let mut e = text(String::new()).key(format!("p{k}"));
            // A line of digits or punctuation under an Arabic one reads right to left with it.
            if let Some(d) = dirs[k].filter(|_| Direction::of(&t.content[range.0..range.1]).is_none()) {
                e = e.dir(d);
            }
            let links = paragraph(&t, k, range, k == last, &mut e);
            if let Some(c) = cursor { e = e.cursor(c); }
            if given && let Some(pt) = &mut e.text { pt.flags.set(crate::el::text_spec::TextFlags::SELECTABLE_CURSOR, true); }
            if a11y_disabled { e.cold_mut().a11y.disabled = true; }
            if focus_disabled || focus_opt_out {
                let f = &mut *e.cold_mut().focus;
                f.disabled = focus_disabled;
                f.opt_out = true;
            } else if links {
                e.cold_mut().focus.enabled = true;
            }
            e
        }).collect();
        // The links' Tab stops went with them.
        if !t.cold.links.is_empty() && let Some(c) = self.cold.get_mut_if_some() { c.focus.enabled = false; }
        if self.role == RoleTag::TEXT { self.role = RoleTag::GROUP; }
        self.style.set_flex_direction(FlexDirection::Column);
        let kept = std::mem::take(&mut self.children);
        self.children = pieces.into_iter().chain(kept).collect();
        self
    }
}

/// Fills `e` with paragraph `k` of `t`, bytes `a..b`: its words and the spans, links, pills,
/// highlights and marks over them, copying with a newline either side but where `t` began or
/// ended. Whether it has links.
fn paragraph(t: &TextSpec, k: usize, (a, b): (usize, usize), last: bool, e: &mut El) -> bool {
    let (a32, b32) = (a as u32, b as u32);
    let clip = |s: u32, e: u32| {
        let (s, e) = (s.max(a32), e.min(b32));
        (s < e).then(|| (s - a32, e - a32))
    };
    let mut p = t.clone();
    p.content = t.content[a..b].to_owned();
    p.spans = t.spans.iter().filter_map(|s| clip(s.start, s.end).map(|(start, end)| crate::text::Span { start, end, ..s.clone() })).collect();
    // A plain text's paragraphs share its empty cold fields.
    if !t.cold.is_empty() {
        let cold = &mut *p.cold;
        cold.links = t.cold.links.iter().filter_map(|l| clip(l.start, l.end).map(|(start, end)| Link { start, end, ..l.clone() })).collect();
        cold.uncopied = t.cold.uncopied.iter().filter_map(|&(s, e)| clip(s, e)).collect();
        cold.rings = t.cold.rings.iter().filter_map(|&(s, e, c, w)| clip(s, e).map(|(s, e)| (s, e, c, w))).collect();
        cold.strikes = t.cold.strikes.iter().filter_map(|&(s, e, c)| clip(s, e).map(|(s, e)| (s, e, c))).collect();
        cold.overlines = t.cold.overlines.iter().filter_map(|&(s, e, c)| clip(s, e).map(|(s, e)| (s, e, c))).collect();
        cold.span_boxes = t.cold.span_boxes.iter().filter_map(|b| {
            let (start, end) = clip(b.start, b.end)?;
            let (cs, ce) = (b.start < a32, b.end > b32);
            let mut style = b.style;
            if cs || ce {
                let c = style.corner_radii();
                style = style
                    .pad_x(if cs { 0.0 } else { style.pad_left() }, if ce { 0.0 } else { style.pad_right() })
                    .corners([
                        if cs { 0.0 } else { c[0] },
                        if ce { 0.0 } else { c[1] },
                        if ce { 0.0 } else { c[2] },
                        if cs { 0.0 } else { c[3] },
                    ]);
            }
            Some(crate::text::SpanBoxRun { start, end, style })
        }).collect();
        cold.highlights = t.cold.highlights.iter().filter_map(|h| clip(h.start, h.end).map(|(start, end)| crate::text::TextHighlight { start, end, ..h.clone() })).collect();
        cold.line_fills = t.cold.line_fills.iter().filter_map(|f| clip(f.start, f.end).map(|(start, end)| crate::el::LineFill { start, end, ..*f })).collect();
        // A chip lies inside one line; one across a newline has no paragraph to sit in.
        cold.pills = t.cold.pills.iter().filter(|c| a32 <= c.start && c.end <= b32).map(|c| crate::text::PillChip { start: c.start - a32, end: c.end - a32, ..c.clone() }).collect();
        cold.blocks = t.cold.blocks.get(k).copied().into_iter().collect();
        cold.soft_breaks.clear();
        if k > 0 {
            cold.text_indent = 0.0;
        }
        cold.copy_breaks = [if k == 0 { t.cold.copy_breaks[0] } else { 1 }, if last { t.cold.copy_breaks[1] } else { 1 }];
        cold.copy_prefix = if k == 0 { t.cold.copy_prefix.clone() } else { None };
    }
    // Fractional line heights, so the stacked paragraphs keep the one text's line pitch.
    p.exact_lines = true;
    p.shape = None;
    let links = !p.cold.links.is_empty();
    e.text = Some(crate::cold::SharedBox::new(p));
    links
}
