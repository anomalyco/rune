//! Text builders: font, spans and decorations, input pills, wrapping and truncation, reveal,
//! links, placeholder and caret, selection, and edit callbacks.

use super::*;

impl El {
    /// Keeps this text's height its lines' own, unrounded (22.4 points for a line at a 22.4
    /// line height, 24.375 device pixels for a 19.5 point line at 1.25x), as a browser lays out
    /// a paragraph: a stack of texts lands where a web page's does rather than gaining up to a
    /// pixel per text. Every text does this unless it asks for [`El::rounded_lines`].
    pub fn exact_lines(mut self) -> Self {
        if let Some(t) = &mut self.text {
            t.exact_lines = true;
        }
        self
    }
    /// Rounds this text's height up to whole device pixels instead of keeping its lines' own
    /// ([`El::exact_lines`]), as Rune laid out every text before: a box sized by one text keeps
    /// its edges on the pixel grid at any scale.
    pub fn rounded_lines(mut self) -> Self {
        if let Some(t) = &mut self.text {
            t.exact_lines = false;
        }
        self
    }
    #[inline]
    pub fn font(mut self, s: TextStyle) -> Self {
        self.set_font(s);
        self
    }
    /// Replaces the string of a [`text`] element, keeping its style, colour and every other
    /// setting: a styled text cloned as a template takes each use's words. Spans and other byte
    /// ranges of the old string are dropped. Does nothing to other elements or to a text input.
    pub fn set_content(&mut self, content: impl Into<String>) -> &mut Self {
        if self.cold.input.is_none()
            && let Some(t) = &mut self.text
        {
            let redacted = t.redacted;
            let content = content.into();
            if redacted {
                t.replace_content(content);
                if let Some(mut src) = t.source.take() {
                    text_spec::wipe_string(&mut src.text);
                }
            } else {
                t.content = content;
                t.shape = None;
                t.source = None;
            }
            t.spans.clear();
            if let Some(cold) = t.cold.get_mut_if_some() {
                if redacted {
                    text_spec::wipe_links(&mut cold.links);
                } else {
                    cold.links.clear();
                }
                cold.uncopied.clear();
                cold.strikes.clear();
                cold.overlines.clear();
                cold.span_boxes.clear();
                cold.highlights.clear();
                cold.line_fills.clear();
                cold.pills.clear();
                cold.rings.clear();
                cold.blocks.clear();
                cold.soft_breaks.clear();
            }
        }
        self
    }
    /// [`El::font`] in place.
    #[inline]
    pub fn set_font(&mut self, s: TextStyle) -> &mut Self {
        if let Some(t) = &mut self.text {
            t.style = s;
        }
        if self.cold.input.is_some() {
            self.sync_input_rich();
        }
        self
    }
    /// Per-frame styled or coloured byte ranges (syntax highlights, mentions, inline styles)
    /// painted over a [`text`], [`rich`] or [`text_input`] element without mutating [`TextEdit`].
    /// On a [`text_input`], byte offsets are relative to `edit.text` and shift automatically
    /// across any active IME preedit.
    pub fn spans(mut self, spans: impl Into<Vec<Span>>) -> Self {
        let spans = spans.into();
        if let Some(inp) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            inp.decorations = spans;
            self.sync_input_rich();
        } else if let Some(t) = &mut self.text {
            t.spans = crate::el::pills::with_pill_colors(&spans, &t.cold.pills);
        }
        self
    }
    /// Alias for [`El::spans`]: per-frame syntax or highlight [`Span`]s painted over text
    /// (for example `SyntaxTheme::from_kit(&kit).spans(&highlights, code_offset)`).
    pub fn decorations(self, spans: impl Into<Vec<Span>>) -> Self {
        self.spans(spans)
    }
    /// An inset `color` border ring `width` points wide inside the rounded [`Span::bg`] of bytes
    /// `start..end`. Touching runs of the range on one line share one ring; wrapped lines each
    /// get their own. No background means no ring. Later overlapping rings win.
    /// Ranges refer to authored UTF-8 bytes and follow text case and truncation. For static
    /// text only: editable text does not rebase these ranges after edits. Rebuild ranges when
    /// replacing the content. During streaming reveal a ringed chip fades with its last word
    /// on each line; selection is painted over the chip and below the glyphs.
    pub fn span_ring(mut self, start: usize, end: usize, color: Color, width: f32) -> Self {
        if let Some(t) = &mut self.text && start < end && width > 0.0 && width.is_finite() {
            t.cold.rings.push((start as u32, end as u32, color, width));
        }
        self
    }
    /// The corner radius in points of this text's rounded [`Span::bg`] backgrounds (4 by
    /// default), as a stylesheet's `border-radius` on one kind of span: a diff's changed words
    /// at 3, say, while inline code keeps 4.
    pub fn span_radius(mut self, radius: f32) -> Self {
        if let Some(t) = &mut self.text && radius.is_finite() {
            t.cold.span_radius = radius.max(0.0);
        }
        self
    }
    /// Inset border rings `(start, end, color, width)` for multiple byte ranges (see [`El::span_ring`]).
    pub fn span_rings(mut self, rings: impl IntoIterator<Item = (usize, usize, Color, f32)>) -> Self {
        for (start, end, color, width) in rings {
            self = self.span_ring(start, end, color, width);
        }
        self
    }
    /// Draws bytes `range` of a [`text`](crate::text()) or [`rich`] element inside a
    /// padded [`SpanBox`](crate::SpanBox) with its own layout-affecting horizontal padding,
    /// per-corner radii, and optional inset border.
    ///
    /// Unlike [`Span::bg`], `SpanBox` padding reserves horizontal room in the line so adjacent
    /// inline chips (such as a search filter's `from:` key and `ada` value) meet without
    /// overlapping, and distinct adjacent ranges are never merged. UTF-8 byte offsets, selection
    /// and copy remain unchanged. Ranges are authored UTF-8 bytes on char boundaries; empty,
    /// out-of-range, or overlapping ones are skipped. Ignored on [`text_input`] fields.
    ///
    /// ```no_run
    /// # use rune::{Color, SpanBox, text};
    /// let chip = text("from:ada")
    ///     .span_box(0..5, SpanBox::new().bg(Color::WHITE.alpha(0.12)).pad_x(6.0, 4.0).corners([4.0, 0.0, 0.0, 4.0]))
    ///     .span_box(5..8, SpanBox::new().bg(Color::WHITE.alpha(0.22)).pad_x(4.0, 6.0).corners([0.0, 4.0, 4.0, 0.0]));
    /// ```
    pub fn span_box(self, range: std::ops::Range<usize>, style: crate::text::SpanBox) -> Self {
        self.span_boxes([(range, style)])
    }
    /// Padded [`SpanBox`](crate::SpanBox)es for multiple non-overlapping byte ranges (see [`El::span_box`]).
    pub fn span_boxes(mut self, boxes: impl IntoIterator<Item = (std::ops::Range<usize>, crate::text::SpanBox)>) -> Self {
        if self.cold.input.is_some() {
            return self;
        }
        let Some(t) = &mut self.text else { return self };
        for (r, style) in boxes {
            let ok = r.start < r.end
                && r.end <= t.content.len()
                && t.content.is_char_boundary(r.start)
                && t.content.is_char_boundary(r.end)
                && style.has_effect();
            let (start, end) = (r.start as u32, r.end as u32);
            if !ok || t.cold.span_boxes.iter().any(|b| b.start < end && start < b.end) {
                continue;
            }
            let at = t.cold.span_boxes.partition_point(|b| b.start < start);
            t.cold.span_boxes.insert(at, crate::text::SpanBoxRun { start, end, style });
            t.shape = None;
        }
        self
    }
    /// A strikethrough line across bytes `start..end` in `color` (`None`: the span's or text's
    /// colour). Wrapped lines each get their own segment. On a [`text_input`], byte offsets are
    /// relative to `edit.text` and shift across any active IME preedit, as [`El::spans`] do.
    pub fn strike(mut self, start: usize, end: usize, color: Option<Color>) -> Self {
        if start >= end {
            return self;
        }
        if let Some(inp) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            inp.decoration_strikes.push((start as u32, end as u32, color));
            self.sync_input_rich();
        } else if let Some(t) = &mut self.text {
            t.cold.strikes.push((start as u32, end as u32, color));
        }
        self
    }
    /// Strikethrough lines `(start, end, color)` for multiple byte ranges (see [`El::strike`]).
    pub fn strikes(mut self, strikes: impl IntoIterator<Item = (usize, usize, Option<Color>)>) -> Self {
        if let Some(inp) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            let before = inp.decoration_strikes.len();
            inp.decoration_strikes.extend(
                strikes.into_iter().filter(|&(s, e, _)| s < e).map(|(s, e, c)| (s as u32, e as u32, c)),
            );
            if inp.decoration_strikes.len() != before {
                self.sync_input_rich();
            }
        } else if let Some(t) = &mut self.text {
            for (start, end, color) in strikes {
                if start < end {
                    t.cold.strikes.push((start as u32, end as u32, color));
                }
            }
        }
        self
    }
    /// An overline above bytes `start..end` in `color` (`None`: the span's or text's colour),
    /// matching CSS `text-decoration-line: overline` and ANSI SGR 53/55. Wrapped lines each get
    /// their own segment, and overlines coexist with [`Span::underline`] and [`El::strike`]. On
    /// a [`text_input`], byte offsets are relative to `edit.text` and shift across any active
    /// IME preedit, as [`El::spans`] and [`El::strike`] do.
    ///
    /// ```no_run
    /// # use rune::{Color, text};
    /// let el = text("OVER plain").overline(0, 4, Some(Color::WHITE));
    /// ```
    pub fn overline(mut self, start: usize, end: usize, color: Option<Color>) -> Self {
        if start >= end {
            return self;
        }
        if let Some(inp) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            inp.decoration_overlines.push((start as u32, end as u32, color));
            self.sync_input_rich();
        } else if let Some(t) = &mut self.text {
            t.cold.overlines.push((start as u32, end as u32, color));
        }
        self
    }
    /// Overlines `(start, end, color)` for multiple byte ranges (see [`El::overline`]).
    pub fn overlines(mut self, overlines: impl IntoIterator<Item = (usize, usize, Option<Color>)>) -> Self {
        if let Some(inp) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            let before = inp.decoration_overlines.len();
            inp.decoration_overlines.extend(
                overlines.into_iter().filter(|&(s, e, _)| s < e).map(|(s, e, c)| (s as u32, e as u32, c)),
            );
            if inp.decoration_overlines.len() != before {
                self.sync_input_rich();
            }
        } else if let Some(t) = &mut self.text {
            for (start, end, color) in overlines {
                if start < end {
                    t.cold.overlines.push((start as u32, end as u32, color));
                }
            }
        }
        self
    }
    /// A paint-only [`TextHighlight`](crate::TextHighlight) over bytes of the text.
    ///
    /// Drawn from the shaped layout's glyph boxes without splitting runs, so adding, moving or
    /// clearing highlights never reshapes text or shifts line breaks.
    pub fn text_highlight(mut self, highlight: crate::text::TextHighlight) -> Self {
        if let Some(t) = &mut self.text && highlight.start < highlight.end {
            t.cold.highlights.push(highlight);
        }
        self
    }
    /// Paint-only [`TextHighlight`](crate::TextHighlight)s over bytes of the text (see [`El::text_highlight`]).
    pub fn text_highlights(mut self, highlights: impl IntoIterator<Item = crate::text::TextHighlight>) -> Self {
        for h in highlights {
            self = self.text_highlight(h);
        }
        self
    }
    /// A display-only full-width line fill and/or left vertical bar ([`LineFill`]) painted under
    /// every visual line intersected by its UTF-8 byte range (see [`El::line_fills`]).
    pub fn line_fill(self, fill: LineFill) -> Self {
        self.line_fills([fill])
    }
    /// Display-only full-width line fills and left bars ([`LineFill`]) painted under text across
    /// the field's full width without mutating [`TextEdit`] or undo history (for fenced code
    /// blocks, block quotes, and similar plain-text decorations). Calling this more than once
    /// appends to earlier fills. On a [`text_input`], byte offsets are relative to `edit.text`
    /// and shift automatically across any active IME preedit, as [`El::decorations`] and
    /// [`El::strikes`] do.
    ///
    /// ```
    /// # use rune::{Color, LineFill, TextEdit, TextStyle, text_input};
    /// # struct App { edit: TextEdit }
    /// # let s = App { edit: TextEdit::default() };
    /// # let body = TextStyle::new(14.0, 400.0);
    /// # let (code_start, code_end, quote_start, quote_end) = (0, 0, 0, 0);
    /// # let (code_bg, quote_bar) = (Color::WHITE, Color::WHITE);
    /// # let _ =
    /// text_input(&s.edit, body)
    ///     .line_fills([LineFill::new(code_start, code_end).bg(code_bg)])
    ///     .line_fills([LineFill::new(quote_start, quote_end).left_bar(2.0, quote_bar)])
    /// # ;
    /// ```
    pub fn line_fills(mut self, fills: impl IntoIterator<Item = LineFill>) -> Self {
        if let Some(inp) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            let before = inp.line_fills.len();
            inp.line_fills.extend(fills.into_iter().filter(LineFill::is_visible));
            if inp.line_fills.len() != before {
                self.sync_input_rich();
            }
        } else if let Some(t) = &mut self.text {
            for f in fills {
                if f.is_visible() {
                    t.cold.line_fills.push(f);
                }
            }
        }
        self
    }
    /// Set the visual palette for rich inline marks (`code`, `link`) and paragraph blocks
    /// (`quote`, `code block`, list markers) on a [`text_input`].
    pub fn rich_style(mut self, style: crate::edit::RichStyle) -> Self {
        if let Some(inp) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            inp.rich_style = style;
            self.sync_input_rich();
        } else if let Some(t) = &mut self.text {
            t.cold.rich_style = Some(style);
        }
        self
    }
    /// The chip colour of each [`Mark::Pill`](crate::Mark::Pill) in a [`text_input`], and its
    /// label and icon colour (`None`: the text's). The chip defaults to the rich style's link
    /// colour at 20% opacity.
    pub fn pill_colors(mut self, bg: Color, fg: Option<Color>) -> Self {
        if let Some(t) = &mut self.text {
            t.cold.pill_colors = Some((bg, fg));
        }
        if self.cold.input.is_some() {
            self.sync_input_rich();
        }
        self
    }
    /// The leading icon for each [`Mark::Pill`](crate::Mark::Pill) chip in a [`text_input`],
    /// by the pill's id (`None`: no icon). The icon takes the label's colour.
    ///
    /// ```
    /// # use rune::{Icon, TextEdit, TextStyle, text_input};
    /// # const FILE: Icon = Icon { name: "file", view: 16.0, body: "" };
    /// # struct App { draft: TextEdit }
    /// # let s = App { draft: TextEdit::default() };
    /// # let body = TextStyle::new(14.0, 400.0);
    /// # let _ =
    /// text_input(&s.draft, body).pill_icon(|id| id.starts_with("file:").then_some(&FILE))
    /// # ;
    /// ```
    ///
    /// The closure may capture state; it runs each time the field is built, so a pill follows
    /// whatever it captured. For colours too, see [`El::pill_style`].
    pub fn pill_icon(mut self, icon: impl Fn(&str) -> Option<&'static crate::Icon> + 'static) -> Self {
        if let Some(inp) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            inp.pill_icon = Some(Rc::new(icon));
            self.sync_input_pills();
            self.sync_input_rich();
        }
        self
    }
    /// A leading full-colour [`Pixels`](crate::Pixels) image (such as a service logo or avatar)
    /// for each [`Mark::Pill`](crate::Mark::Pill) chip in a [`text_input`], by the pill's id
    /// (`None`: no image). Drawn in the leading icon's slot and room, scaled to fit while keeping
    /// its aspect ratio, without tinting. When `Some`, it takes precedence over
    /// [`PillStyle::icon`] and [`El::pill_icon`].
    ///
    /// ```
    /// # use std::collections::HashMap;
    /// # use rune::{Pixels, TextEdit, TextStyle, text_input};
    /// # struct App { draft: TextEdit, logos: HashMap<String, Pixels> }
    /// # let s = App { draft: TextEdit::default(), logos: HashMap::new() };
    /// # let body = TextStyle::new(14.0, 400.0);
    /// # let _ =
    /// text_input(&s.draft, body).pill_image({
    ///     let logos = s.logos.clone();
    ///     move |id| logos.get(id).cloned()
    /// })
    /// # ;
    /// ```
    pub fn pill_image(mut self, image: impl Fn(&str) -> Option<crate::Pixels> + 'static) -> Self {
        if let Some(inp) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            inp.pill_image = Some(Rc::new(image));
            self.sync_input_pills();
            self.sync_input_rich();
        }
        self
    }
    /// The look of each [`Mark::Pill`](crate::Mark::Pill) chip in a [`text_input`] by the
    /// pill's id: chip colour, label colour and icon, so a person and an agent can differ.
    /// What the style leaves unset falls back to [`El::pill_colors`] and [`El::pill_icon`].
    /// The closure may capture app state; it runs each time the field is built, and what it
    /// returns is drawn, so a cached subtree restyles when that state changes.
    ///
    /// ```
    /// # use std::collections::HashSet;
    /// # use rune::{Color, Icon, PillStyle, TextEdit, TextStyle, text_input};
    /// # const PURPLE: Color = Color::WHITE;
    /// # const BOT: Icon = Icon { name: "bot", view: 16.0, body: "" };
    /// # struct App { draft: TextEdit, agents: HashSet<String> }
    /// # let s = App { draft: TextEdit::default(), agents: HashSet::new() };
    /// # let body = TextStyle::new(14.0, 400.0);
    /// # let _ =
    /// text_input(&s.draft, body).pill_style({
    ///     let agents = s.agents.clone();
    ///     move |id| if agents.contains(id) { PillStyle::default().bg(PURPLE).icon(&BOT) } else { PillStyle::default() }
    /// })
    /// # ;
    /// ```
    pub fn pill_style(mut self, style: impl Fn(&str) -> PillStyle + 'static) -> Self {
        if let Some(inp) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            inp.pill_style = Some(Rc::new(style));
            self.sync_input_pills();
            self.sync_input_rich();
        }
        self
    }

    /// Secondary press (right-click or macOS control-click) on a [`Mark::Pill`](crate::Mark::Pill)
    /// inside this [`text_input`]. Called with the pill's id and the press point
    /// in window points; return `true` to handle it (opening a pill-specific menu) or `false` to
    /// fall through to the field's standard context menu.
    pub fn on_pill_secondary<S: 'static>(mut self, f: impl Fn(&mut S, &str, PointerPress, &mut Tx) -> bool + 'static) -> Self {
        if let Some(inp) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            inp.on_pill_secondary = Some(Rc::new(move |s, id, press, tx| f(downcast::<S>(s), id, press, tx)));
        }
        self
    }
    pub(super) fn sync_input_pills(&mut self) {
        let (Some(inp), Some(t)) = (&self.cold.input, &mut self.text) else { return };
        if inp.edit.is_plain() {
            t.cold.pills.clear();
            return;
        }
        t.cold.pills = inp
            .edit
            .display_pills()
            .into_iter()
            .map(|(start, end, id)| {
                if let Some(inline) = inp.pill_inline_image.as_ref().and_then(|f| f(id)) {
                    return crate::text::PillChip {
                        start,
                        end,
                        icon: None,
                        image: None,
                        bg: None,
                        fg: None,
                        ring: None,
                        radius: None,
                        pad_x: None,
                        margin_x: None,
                        bare: Some(Box::new(inline.into_bare())),
                    };
                }
                let st = inp.pill_style.as_ref().map(|f| f(id)).unwrap_or_default();
                let image = inp.pill_image.as_ref().and_then(|f| f(id));
                let icon = if image.is_some() {
                    None
                } else {
                    st.icon.or_else(|| inp.pill_icon.as_ref().and_then(|f| f(id)))
                };
                crate::text::PillChip { start, end, icon, image, bg: st.bg, fg: st.fg, ring: st.ring, radius: st.radius, pad_x: st.pad_x, margin_x: st.margin_x, bare: None }
            })
            .collect();
    }
    pub(crate) fn sync_input_rich(&mut self) {
        let (Some(inp), Some(t)) = (&self.cold.input, &mut self.text) else { return };
        t.cold.rich_style = Some(inp.rich_style);
        if inp.edit.is_plain()
            && inp.decorations.is_empty()
            && inp.decoration_strikes.is_empty()
            && inp.decoration_overlines.is_empty()
            && inp.line_fills.is_empty()
        {
            // A plain field lays out and draws exactly as before rich editing existed.
            t.spans.clear();
            t.cold.strikes.clear();
            t.cold.overlines.clear();
            t.cold.line_fills.clear();
            t.cold.blocks.clear();
            t.cold.soft_breaks.clear();
            t.cold.pills.clear();
            return;
        }
        let (mut base_spans, mut strikes, rich_links, blocks) =
            inp.edit.rich_display_specs(t.style, inp.rich_style);
        for d in &inp.decorations {
            for (ds, de) in inp.edit.map_range_to_display(d.start as usize, d.end as usize) {
                let mut sp = d.clone();
                sp.start = ds;
                sp.end = de;
                base_spans.push(sp);
            }
        }
        for &(start, end, color) in &inp.decoration_strikes {
            for (ds, de) in inp.edit.map_range_to_display(start as usize, end as usize) {
                strikes.push((ds, de, color));
            }
        }
        let mut overlines = Vec::with_capacity(inp.decoration_overlines.len());
        for &(start, end, color) in &inp.decoration_overlines {
            for (ds, de) in inp.edit.map_range_to_display(start as usize, end as usize) {
                overlines.push((ds, de, color));
            }
        }
        let pill_fg = t.cold.pill_colors.and_then(|(_, fg)| fg);
        base_spans.extend(t.cold.pills.iter().filter(|p| p.bare.is_none()).filter_map(|p| {
            Some(Span { start: p.start, end: p.end, color: Some(p.fg.or(pill_fg)?), ..Span::default() })
        }));
        let mut flat = crate::text::flatten_spans(&base_spans);
        for s in &mut flat {
            if s.underline == Some(Color::TRANSPARENT) {
                s.underline = Some(s.color.unwrap_or(t.color));
            }
        }
        let mut line_fills = Vec::with_capacity(inp.line_fills.len());
        for &fill in &inp.line_fills {
            if !fill.is_visible() {
                continue;
            }
            let mapped = inp.edit.map_range_to_display(fill.start as usize, fill.end as usize);
            if let (Some(&(ds, _)), Some(&(_, de))) = (mapped.first(), mapped.last())
                && ds < de
            {
                line_fills.push(LineFill { start: ds, end: de, ..fill });
            }
        }
        t.spans = flat;
        t.cold.strikes = strikes;
        t.cold.overlines = overlines;
        t.cold.line_fills = line_fills;
        t.cold.blocks = blocks;
        t.cold.soft_breaks = inp.edit.soft_breaks().to_vec();
        // Replace links generated from `Mark::Link` while keeping any explicit `.link(...)` calls.
        t.cold.links = rich_links;
    }
    /// Wrap text at the element's width.
    #[inline]
    pub fn wrap(mut self) -> Self {
        self.set_wrap();
        self
    }
    /// [`El::wrap`] in place.
    #[inline]
    pub fn set_wrap(&mut self) -> &mut Self {
        if let Some(t) = &mut self.text {
            t.wrap = true;
        }
        self
    }
    /// Keep text unwrapped so lines break only at explicit newlines (`\n`), matching HTML's
    /// `<textarea wrap="off">` and CSS's `white-space: pre`.
    ///
    /// On a multi-line [`text_input`], this clears wrapping without enabling [`El::single_line`]:
    /// the field keeps newlines, Up/Down navigation and multi-line editing while measuring the
    /// natural width of its longest line. Place it inside an [`El::scroll_x`] or [`El::scroll_xy`]
    /// viewport so long lines scroll sideways and keep the caret in view.
    ///
    /// ```no_run
    /// # use rune::{TextEdit, TextStyle, col, text_input};
    /// let code = TextEdit::new("let long_line = 1;\nlet next = 2;");
    /// let editor = col().size(320.0, 160.0).scroll_xy()
    ///     .child(text_input(&code, TextStyle::new(13.0, 400.0).mono()).nowrap());
    /// ```
    #[inline]
    pub fn nowrap(mut self) -> Self {
        self.set_nowrap();
        self
    }
    /// [`El::nowrap`] in place.
    #[inline]
    pub fn set_nowrap(&mut self) -> &mut Self {
        if let Some(t) = &mut self.text {
            t.wrap = false;
            t.wrap_anywhere = false;
        }
        self
    }
    /// Wrap text at the element's width, breaking inside a word only when the word alone would
    /// overflow the line (`overflow-wrap: anywhere`) while breaking normal words at spaces, and
    /// lowering the text's min-content width. Does not change flex shrinking; use
    /// [`El::shrink`] to allow a flex row to narrow the element.
    pub fn wrap_anywhere(mut self) -> Self {
        if self.cold.input.as_ref().is_some_and(|i| i.single_line) {
            return self;
        }
        if let Some(t) = &mut self.text {
            t.wrap = true;
            t.wrap_anywhere = true;
        }
        self
    }
    /// Break this text's lines greedily when the app wraps text prettily
    /// ([`crate::text::set_pretty_wrap`]): text that grows while it's read, such as a reply
    /// still streaming in, whose last lines would move as each word arrives.
    pub fn wrap_greedy(mut self) -> Self {
        if let Some(t) = &mut self.text {
            t.flags.set(crate::el::text_spec::TextFlags::GREEDY, true);
        }
        self
    }
    /// Whether this is text that wraps (`.wrap()`), for containers that let such text shrink
    /// to their width.
    pub fn wraps(&self) -> bool {
        self.text.as_ref().is_some_and(|t| t.wrap)
    }
    /// One line, cut with "…" when wider than the element. Sets `flex-shrink: 1` and lowers an
    /// automatic `min-width` to `0` while preserving any explicit [`El::min_w`] and leaving
    /// `min-height` untouched (override an earlier explicit minimum with `.min_w(0.0)`). A
    /// [`text_input`] ignores it and the other truncations.
    #[inline]
    pub fn truncate(mut self) -> Self {
        self.set_truncate();
        self
    }
    /// [`El::truncate`] in place. Sets `flex-shrink: 1` and lowers an automatic `min-width` to
    /// `0` while preserving any explicit [`El::min_w`] and leaving `min-height` untouched.
    #[inline]
    pub fn set_truncate(&mut self) -> &mut Self {
        self.set_elide(Elide::End);
        self
    }
    /// [`El::truncate`] keeping both ends: "…" replaces the middle, as AppKit's
    /// `NSLineBreakByTruncatingMiddle` does for paths and file names. Cuts fall between
    /// graphemes and refit when the element resizes; copying and the accessibility text keep
    /// the whole string. Preserves any explicit [`El::min_w`] and leaves `min-height` untouched.
    pub fn truncate_middle(mut self) -> Self {
        self.set_elide(Elide::Middle);
        self
    }
    /// [`El::truncate`] keeping the end: "…" replaces the start. Preserves any explicit
    /// [`El::min_w`] and leaves `min-height` untouched.
    pub fn truncate_head(mut self) -> Self {
        self.set_elide(Elide::Head);
        self
    }
    fn set_elide(&mut self, elide: Elide) {
        // A field's caret and selection index its own text, which a cut would break.
        if self.cold.input.is_some() { return; }
        self.set_one_line(|t| {
            t.truncate = true;
            t.elide = elide;
            t.truncate_fade = false;
        });
    }
    /// One unwrapped line that shrinks with its parent; `cut` sets how an overflow is cut.
    fn one_line(mut self, cut: impl FnOnce(&mut TextSpec)) -> Self {
        self.set_one_line(cut);
        self
    }
    fn set_one_line(&mut self, cut: impl FnOnce(&mut TextSpec)) {
        if let Some(t) = &mut self.text {
            t.wrap = false;
            t.wrap_anywhere = false;
            t.max_lines = 0;
            cut(t);
        }
        self.style.set_flex_shrink(1.0);
        if self.style.min_size().width.is_auto() {
            self.style.set_min_width(length(0.0));
        }
    }
    /// Wrapped text that stops after `lines` lines, "…" ending the last when more would follow,
    /// as CSS's `line-clamp`. Combine with [`El::elide_middle`] or [`El::elide_head`] in either
    /// order to keep the trailing text (for example a file extension) across multiple lines.
    /// The element measures at most `lines` lines tall. The cut falls between graphemes and
    /// refits when the element resizes; spans and links keep their looks across the visible parts,
    /// and copying and the accessibility text keep the whole string. 0 shows every line. A
    /// [`text_input`] ignores it; use [`auto_grow`] there.
    pub fn max_lines(mut self, lines: usize) -> Self {
        if self.cold.input.is_some() { return self; }
        if let Some(t) = &mut self.text {
            t.wrap = true;
            t.max_lines = lines;
            t.truncate = false;
            t.truncate_fade = false;
        }
        self
    }
    /// Put the ellipsis in the middle ("…" replaces the middle, keeping both ends) without
    /// changing wrapping or [`El::max_lines`], matching AppKit's `NSLineBreakByTruncatingMiddle`
    /// with `maximumNumberOfLines` and Pango's `PANGO_ELLIPSIZE_MIDDLE`.
    ///
    /// Combine with [`El::max_lines`] in either order (`.max_lines(2).elide_middle()` or
    /// `.elide_middle().max_lines(2)`). For single-line middle truncation, see
    /// [`El::truncate_middle`]. A [`text_input`] ignores it.
    ///
    /// ```no_run
    /// # use rune::text;
    /// let label = text("Quarterly report final revised version two of the budget.xlsx")
    ///     .w(120.0)
    ///     .max_lines(2)
    ///     .elide_middle();
    /// ```
    pub fn elide_middle(mut self) -> Self {
        if self.cold.input.is_some() { return self; }
        if let Some(t) = &mut self.text {
            t.elide = Elide::Middle;
        }
        self
    }
    /// Put the ellipsis at the start ("…" replaces the head, keeping the end) without changing
    /// wrapping or [`El::max_lines`], matching AppKit's `NSLineBreakByTruncatingHead` with
    /// `maximumNumberOfLines` and Pango's `PANGO_ELLIPSIZE_START`.
    ///
    /// Combine with [`El::max_lines`] in either order (`.max_lines(2).elide_head()` or
    /// `.elide_head().max_lines(2)`). For single-line head truncation, see [`El::truncate_head`].
    /// A [`text_input`] ignores it.
    ///
    /// ```no_run
    /// # use rune::text;
    /// let path = text("~/Projects/rune/crates/rune-kit/src/components/appearance_panel.rs")
    ///     .w(140.0)
    ///     .max_lines(2)
    ///     .elide_head();
    /// ```
    pub fn elide_head(mut self) -> Self {
        if self.cold.input.is_some() { return self; }
        if let Some(t) = &mut self.text {
            t.elide = Elide::Head;
        }
        self
    }
    /// One line, clipped with a 1.5 em trailing fade only when it overflows. No ellipsis. Sets
    /// `flex-shrink: 1` and lowers an automatic `min-width` to `0` while preserving any explicit
    /// [`El::min_w`] and leaving `min-height` untouched.
    pub fn truncate_fade(self) -> Self {
        self.truncate_fade_width(TRUNCATE_FADE_EM)
    }
    /// [`El::truncate_fade`] with a fade `em` ems wide. Preserves any explicit [`El::min_w`] and
    /// leaves `min-height` untouched.
    pub fn truncate_fade_width(self, em: f32) -> Self {
        self.one_line(|t| {
            t.truncate = false;
            t.truncate_fade = true;
            // The usual width is the cold block's own: only another one writes (and allocates) it.
            let em = em.max(0.0);
            if t.cold.fade_em != em {
                t.cold.fade_em = em;
            }
        })
    }
    /// Streaming word reveal (see [`TextSpec::reveal`]).
    pub fn reveal(mut self, word_fade: f32) -> Self {
        if let Some(t) = &mut self.text {
            t.reveal = word_fade.max(0.0);
        }
        self
    }
    /// Keep a text field to one line: long text scrolls sideways under the caret instead of
    /// wrapping. Fields are multi-line by default.
    pub fn single_line(mut self) -> Self {
        if let Some(i) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            i.single_line = true;
            i.edit.set_single_line(true);
            if let Some(t) = &mut self.text {
                t.wrap = false;
                t.wrap_anywhere = false;
                if i.edit.is_masked() {
                    t.replace_content(i.edit.display().0);
                }
            }
        }
        self
    }
    /// Make a byte range of this text pressable (see [`Link`]). Only the range's shaped glyph
    /// boxes take the pointer, which shows a hand over them; the rest of the text stays
    /// transparent to input. Each link is a Tab stop that Enter or Space presses.
    pub fn link(mut self, link: Link) -> Self {
        if let Some(t) = &mut self.text {
            t.cold.links.push(link);
            if !self.cold.focus.opt_out && !self.cold.focus.disabled {
                self.cold_mut().focus.enabled = true;
            }
        }
        self
    }
    /// The pointer's shape over this text's links while the text is [`El::selectable`] (or sets no
    /// cursor of its own): the hand unless this names another, such as [`Cursor::Default`] for a
    /// link that reads as plain text. Selectable text keeps the I-beam for itself, so
    /// [`El::cursor`] can't say this. Text that isn't selectable and sets a cursor shows that over
    /// its links, and a text field's links keep the hand.
    pub fn link_cursor(mut self, cursor: Cursor) -> Self {
        if let Some(t) = &mut self.text {
            t.cold.link_cursor = Some(cursor);
        }
        self
    }
    /// Called with a [`Link`]'s id when it is clicked, or pressed with Enter or Space while
    /// focused. Read [`Tx::link_rect`] inside the handler for the activated link's first drawn
    /// line box `[left, top, width, height]` in window points (for anchoring a popover or preview
    /// card beside the link on both pointer and keyboard activation). A middle click on a link
    /// calls it too, as a browser opens a link in a new tab: [`Tx::button`] is then
    /// [`PointerButton::Middle`](crate::PointerButton::Middle), for a handler whose link only
    /// opens in place to ignore.
    pub fn on_link<S: 'static>(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self {
        if let Some(t) = &mut self.text {
            t.cold.on_link = Some(LinkFn(Rc::new(move |s, id, tx| f(downcast::<S>(s), id, tx))));
        }
        self
    }
    /// Called with a [`Link`]'s id and the point (window points) when a secondary press
    /// (right-click, macOS control-click) lands on its glyphs: a menu for that link. It takes
    /// the press, so ancestors' [`El::on_secondary_press`] and the text's Copy menu don't see
    /// it; elsewhere in the text they still do.
    pub fn on_link_secondary<S: 'static>(mut self, f: impl Fn(&mut S, &str, PointerPress, &mut Tx) + 'static) -> Self {
        if let Some(t) = &mut self.text {
            t.cold.on_link_secondary = Some(LinkSecondaryFn(Rc::new(move |s, id, p, tx| f(downcast::<S>(s), id, p, tx))));
        }
        self
    }
    /// Called when the pointer enters, switches between, or leaves this text's [`Link`] spans:
    /// `(Some(id), Some([left, top, width, height]))` in window points for the hovered link's
    /// hit line box on enter or switch, and `(None, None)` when hover ends (pointer leaves the
    /// link or window, link scrolls away under a stationary pointer, an overlying hit cover
    /// occludes it, or the text unmounts, rebuilds, or becomes disabled/inert).
    ///
    /// Moving within the same link does not re-fire. Read [`Tx::pressing`] inside the handler
    /// to tell whether a mouse button is held down (for example while drag-selecting text).
    /// Disabled links and touch gestures never report hover. Windows without an `on_link_hover`
    /// handler pay no pointer-move work.
    ///
    /// ```
    /// # use rune::{Cx, El, Env, Link, Runtime, col, text};
    /// # #[derive(Default)] struct S { hovered: Option<(String, [f32; 4])> }
    /// # fn view(_: &S, _: &mut Cx<S>) -> El {
    /// col().size(240.0, 80.0).child(
    ///     text("Open the docs")
    ///         .link(Link::new(9..13, "https://example.com/docs"))
    ///         .on_link_hover(|s: &mut S, url, rect, _| {
    ///             s.hovered = url.zip(rect).map(|(u, r)| (u.to_owned(), r));
    ///         }),
    /// )
    /// # }
    /// # let mut rt = Runtime::new(S::default(), view, (240.0, 80.0), Env::new(true, 1.0));
    /// # rt.frame(0.0);
    /// # let (x, y) = rt.find("https://example.com/docs").unwrap();
    /// # rt.pointer_move(x, y);
    /// # rt.frame(0.0);
    /// # assert_eq!(rt.state.hovered.as_ref().map(|(u, _)| u.as_str()), Some("https://example.com/docs"));
    /// # rt.pointer_leave();
    /// # rt.frame(0.0);
    /// # assert!(rt.state.hovered.is_none());
    /// ```
    pub fn on_link_hover<S: 'static>(
        mut self,
        f: impl Fn(&mut S, Option<&str>, Option<[f32; 4]>, &mut Tx) + 'static,
    ) -> Self {
        if let Some(t) = &mut self.text {
            t.cold.on_link_hover = Some(LinkHoverFn(Rc::new(move |s, id, rect, tx| {
                f(downcast::<S>(s), id, rect, tx)
            })));
        }
        self
    }
    /// Mark this text field's misspelled words, as a browser's `spellcheck` does: the platform's
    /// checker (Windows' spell checking API, AppKit's `NSSpellChecker` on macOS, installed system
    /// Hunspell dictionaries on Linux or fetched Hunspell dictionaries on the web when the
    /// default-on `hunspell` feature is enabled) puts a red
    /// marker under each word the caret has left, and the field's menu offers its suggestions and
    /// Learn Spelling. Off by default; a secure field never checks.
    pub fn spellcheck(mut self, on: bool) -> Self {
        if let Some(i) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            i.spellcheck = on;
        }
        self
    }
    pub fn placeholder(mut self, s: impl Into<String>, c: Color) -> Self {
        if let Some(i) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            i.placeholder = s.into();
            i.placeholder_color = c;
            i.placeholder_inked = true;
        }
        self
    }
    /// Lay this field's placeholder out in `style` rather than the field's own, as CSS's
    /// `::placeholder` can: a phone's field types at 16 pt (so iOS doesn't zoom to it) while its
    /// hint keeps the design's size.
    pub fn placeholder_font(mut self, style: TextStyle) -> Self {
        if let Some(i) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            i.placeholder_style = Some(style);
        }
        self
    }
    /// The insertion caret and selection highlight colours for a text field or
    /// [`El::caret_browsing`] selectable text. Without it, the caret follows the text colour
    /// ([`El::color`], else [`Cx::foreground`](crate::Cx::foreground), else white), matching CSS
    /// `caret-color: auto`, and a text field's selection uses that colour at 20%.
    pub fn caret(mut self, caret: Color, selection: Color) -> Self {
        if let Some(i) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            i.caret = caret;
            i.selection = selection;
            i.caret_inked = true;
            i.selection_inked = true;
        }
        if let Some(t) = &mut self.text {
            t.cold.caret = Some(caret);
            t.cold.selection = selection;
        }
        self
    }
    /// Lets people select this read-only text and copy it, as SwiftUI's
    /// `.textSelection(.enabled)`: drag across it, double-click a word, triple-click a paragraph,
    /// shift-click to extend; Cmd+C (Ctrl+C elsewhere) copies and Cmd+A selects all of it while it
    /// holds the selection; a right-click offers Copy on macOS. It shows the I-beam, isn't a Tab
    /// stop, and a press anywhere else clears the selection. Selecting never re-lays out the
    /// text. Set the highlight with [`El::selection_color`].
    ///
    /// Inside a row that handles input, the row keeps it: a click on the text presses the row,
    /// hover and press paint reach it, and its `on_secondary_press` wins over Copy. A drag that
    /// selects text doesn't press. Inside a draggable (`on_drag`, `drag_out`) row, dragging
    /// drags the row, and double- and triple-clicks still select.
    #[inline]
    pub fn selectable(mut self, on: bool) -> Self {
        self.set_selectable(on);
        self
    }
    /// [`El::selectable`] in place.
    pub fn set_selectable(&mut self, on: bool) -> &mut Self {
        if let Some(t) = &mut self.text {
            if self.cold.input.is_some() || t.redacted { return self; }
            t.selectable = on;
            if on && self.handlers.cursor.is_none() {
                self.handlers.cursor = Some(Cursor::Text);
                t.flags.set(crate::el::text_spec::TextFlags::SELECTABLE_CURSOR, true);
            } else if !on
                && t.flags.has(crate::el::text_spec::TextFlags::SELECTABLE_CURSOR)
                && self.handlers.cursor == Some(Cursor::Text)
            {
                self.handlers.cursor = None;
                t.flags.set(crate::el::text_spec::TextFlags::SELECTABLE_CURSOR, false);
            }
        }
        self
    }
    /// Makes the [`El::selectable`] texts inside this element one selection, in tree order, as
    /// the paragraphs of a web page are: a drag from one into another selects across them (the
    /// gaps between included), Shift-click extends into any of them, Cmd+A selects them all and
    /// Cmd+C copies their text joined by a newline (or the blank line [`El::copy_breaks`] asks
    /// for, after any [`El::copy_prefix`]), or a tab between texts side by side (table cells). Double- and triple-clicks still take a word or paragraph of one text. Text that
    /// isn't selectable (a list marker) is skipped. Outside a group each text is its own.
    pub fn selection_group(mut self, on: bool) -> Self {
        self.flags.set(ElFlags::SELECTION_GROUP, on);
        self
    }
    /// Opts an [`El::selectable`] text element or [`El::selection_group`] container into keyboard
    /// caret browsing, as in Chrome's caret browsing mode: the element becomes focusable via click
    /// or Tab, draws a 1.5 pt caret when focused and collapsed (coloured by [`El::caret`], else
    /// [`El::color`] / [`Cx::foreground`](crate::Cx::foreground)), moves by character, word, visual
    /// line, page, and line or document `Home`/`End`, extends the selection with `Shift` across
    /// mounted sibling texts and unmounted `virtual_rows` / `virtualize` rows in tree order (keeping
    /// offscreen anchors pinned for copy), collapses a non-empty selection on plain `Left`/`Right`,
    /// scrolls enclosing viewports to keep the caret in view, preserves `Cmd/Ctrl+C` and
    /// `Cmd/Ctrl+A`, and publishes the caret and selection to accessibility. Off by default,
    /// leaving ordinary [`El::selectable`] text non-focusable and untouched.
    ///
    /// ```
    /// use rune::{El, col, text};
    ///
    /// let _doc: El = col()
    ///     .selection_group(true)
    ///     .caret_browsing(true)
    ///     .child(text("First paragraph").selectable(true))
    ///     .child(text("Second paragraph").selectable(true));
    /// ```
    pub fn caret_browsing(mut self, on: bool) -> Self {
        if on || self.cold.get().is_some() {
            self.cold_mut().focus.caret_browsing = on;
        }
        self
    }
    /// What copying a text selection inside this element puts on the clipboard (Cmd+C and the
    /// Copy menu) instead of the selected text as drawn, so a document drawn from source can
    /// copy the source. `f` sees every [`El::selectable`] text inside this element that the
    /// selection could reach (those of its selection group), in tree order, each with its
    /// selected range (empty when it isn't selected) and its path relative to this element.
    /// Return `None` to copy the drawn text after all ([`TextSelection::text`]). When a
    /// selection spans unbuilt virtual rows, collection stops at 10,000 unbuilt rows or 4 MiB of
    /// UTF-8 text and sets [`TextSelection::truncated`]. The nearest such element around both ends
    /// of the selection answers. Kit's `Markdown::copy_source` is one.
    ///
    /// ```
    /// # use rune::col;
    /// # let note = String::from("# Note");
    /// # let _ =
    /// col().selection_group(true)
    ///     .selection_copy(move |sel| sel.texts().iter().all(|t| t.is_whole()).then(|| note.clone()))
    /// # ;
    /// ```
    ///
    /// [`TextSelection::text`]: crate::TextSelection::text
    /// [`TextSelection::truncated`]: crate::TextSelection::truncated
    pub fn selection_copy(mut self, f: impl Fn(&crate::TextSelection) -> Option<String> + 'static) -> Self {
        self.handlers.selection_copy = Some(Rc::new(f));
        self
    }
    /// Hears the [`El::selectable`] text selection change inside this element: when one starts
    /// (the first selected character of a drag, a double-click's word, Cmd+A,
    /// [`Tx::select_text`]), changes, leaves this element or clears. [`SelectionChange`] holds
    /// the whole selection while any of it is inside this element, `None` otherwise, and says
    /// whether it [`started`](crate::SelectionChange::started) or
    /// [`cleared`](crate::SelectionChange::cleared) here. It runs at most once a frame, however
    /// many times a drag moved the selection, before that frame's commit, and once more when the
    /// press making it releases ([`dragging`](crate::SelectionChange::dragging)). A press leaving
    /// only a caret isn't heard. Call [`Tx::live_only`] when the view reads nothing that changed.
    ///
    /// ```
    /// # use rune::col;
    /// # struct Notes { whole_note_highlight: bool }
    /// # let _ =
    /// col().selection_group(true).on_selection_change(|s: &mut Notes, change, _| {
    ///     if change.started() { s.whole_note_highlight = false; }
    /// })
    /// # ;
    /// ```
    ///
    /// [`SelectionChange`]: crate::SelectionChange
    pub fn on_selection_change<S: 'static>(mut self, f: impl Fn(&mut S, &crate::SelectionChange, &mut Tx) + 'static) -> Self {
        self.handlers.on_selection_change = Some(Rc::new(move |s, change, tx| f(downcast::<S>(s), change, tx)));
        self
    }
    /// Draws and lays out, but the pointer and keyboard pass through this element and
    /// everything inside it, as with HTML's `inert`: a press, hover, scroll, drop or tooltip
    /// goes to whatever is under it (an ancestor or an element drawn below), along with that
    /// element's cursor. Handlers inside never run from input, nothing inside takes focus (Tab
    /// skips it, a field asking for focus doesn't get it, and focus already inside drops), and
    /// accessibility reports it inert. `on_layout` and `Cx::layout` still see its rects. For a
    /// document drawn under an editor that takes its presses itself.
    pub fn inert(mut self) -> Self {
        self.flags.set(ElFlags::INERT, true);
        self
    }
    /// Bytes of [`El::selectable`] text that draw but aren't copied, such as the spacing
    /// characters Markdown pads inline code with. A double-click's word stops at them.
    pub fn uncopied(mut self, range: std::ops::Range<usize>) -> Self {
        if let Some(t) = &mut self.text && range.start < range.end {
            let at = t.cold.uncopied.partition_point(|r| (r.0 as usize) < range.start);
            t.cold.uncopied.insert(at, (range.start as u32, range.end as u32));
        }
        self
    }
    /// How many newlines copying puts between this [`El::selectable`] text and the text copied
    /// before it, and the text after it, when a [`El::selection_group`] selection takes more than
    /// one: the larger of the two texts' asks, one by default. Two leaves a blank line, as a web
    /// page's paragraphs copy. Texts side by side still take a tab.
    ///
    /// ```
    /// # use rune::text;
    /// # let para = "first para";
    /// # let _ =
    /// text(para).selectable(true).copy_breaks(2, 2) // "first para\n\nsecond para"
    /// # ;
    /// ```
    pub fn copy_breaks(mut self, before: u8, after: u8) -> Self {
        if let Some(t) = &mut self.text { t.cold.copy_breaks = [before, after]; }
        self
    }
    /// Text copying puts before this [`El::selectable`] text when a selection takes it from its
    /// start along with other texts or covers its whole copied text, without drawing it: a list
    /// item's `"- "` or `"1. "`, whose drawn marker isn't selectable. A partial copy of this text
    /// alone leaves it out.
    pub fn copy_prefix(mut self, prefix: impl Into<std::rc::Rc<str>>) -> Self {
        if let Some(t) = &mut self.text { t.cold.copy_prefix = Some(prefix.into()); }
        self
    }
    /// A selection as a browser paints `::selection`, in a text field or [`El::selectable`]
    /// text: its highlight and the ink its selected glyphs take, one pair while the window is
    /// active and another while it isn't ([`SelectionStyle::chromium`](crate::SelectionStyle::chromium)
    /// is Chromium's default). It takes over [`El::caret`]'s and [`El::selection_color`]'s
    /// highlight.
    pub fn selection_style(mut self, style: crate::SelectionStyle) -> Self {
        if let Some(i) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) { i.selection_style = Some(style); }
        if let Some(t) = &mut self.text { t.cold.selection_style = Some(style); }
        self
    }
    /// The selection highlight of [`El::selectable`] text or a text field, drawn alike for both.
    pub fn selection_color(mut self, color: Color) -> Self {
        if let Some(i) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            i.selection = color;
            i.selection_inked = true;
        }
        if let Some(t) = &mut self.text { t.cold.selection = color; }
        self
    }
    /// Binds the field to a `TextEdit` in app state. `f` may capture (a row index, say); wrap
    /// a closure in [`edit_access`] so its returned borrow is tied to the state.
    pub fn on_edit<S: 'static>(mut self, f: impl for<'a> Fn(&'a mut S) -> &'a mut TextEdit + 'static) -> Self {
        if let Some(i) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            i.access = Some(Rc::new(edit_fn(move |s| f(downcast::<S>(s)))));
        }
        self
    }
    /// Called when Return submits this text field. In a multi-line field (the default),
    /// Shift+Return and Option+Return insert a newline (as does plain Return inside a list, quote
    /// or code block), while plain Return in a paragraph calls `on_submit` — or presses the
    /// [`El::default_button`] when the field has none.
    pub fn on_submit<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        if let Some(i) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            i.submit = Some(Rc::new(move |s, tx| f(downcast::<S>(s), tx)));
        }
        self
    }
    /// Let dependent layout reflow when this input changes; the typed text remains immediate.
    pub fn animate_edit_layout(mut self) -> Self {
        if let Some(i) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) { i.animate_layout = true; }
        self
    }
}

impl El {
    /// Paint the latest text in this element's fixed layout box without rebuilding the view or
    /// reflowing siblings. `text("00:00")` reserves that placeholder's size; explicit
    /// `.size(w, h)` also works, and overflow is clipped.
    ///
    /// Plain single-line text (including [`El::truncate_fade`]) shapes one line directly.
    /// [`El::wrap`], [`El::truncate`] (and `truncate_middle` / `truncate_head`), and
    /// [`El::max_lines`] re-wrap or re-elide inside the existing box width. [`El::inline`] boxes
    /// stay in flow and reposition their child elements on every live update (a box placed at the
    /// end of a non-empty placeholder stays anchored at the end as length changes; any earlier
    /// offset stays at its authored byte clamped down to the nearest UTF-8 char boundary). Where
    /// plain string replacement cannot apply (a text editor, non-text element, [`El::spans`],
    /// [`El::reveal`], [`El::secure`], links, pills, or byte-indexed text decorations), the
    /// binding is ignored and the element keeps its authored text without panicking.
    ///
    /// ```no_run
    /// # use rune::{LiveText, text};
    /// let caption = LiveText::new("Starting...");
    /// let el = text("Placeholder").size(160.0, 40.0).max_lines(2).live_text(&caption);
    /// caption.set("Live caption that wraps and elides inside the box");
    /// ```
    pub fn live_text(mut self, value: &crate::LiveText) -> Self {
        if self.cold.input.is_none()
            && let Some(spec) = &mut self.text
        {
            spec.cold.live = Some(value.clone());
        }
        self
    }
    #[inline]
    pub fn color(mut self, c: Color) -> Self {
        self.set_color(c);
        self
    }
    /// [`El::color`] in place.
    #[inline]
    pub fn set_color(&mut self, c: Color) -> &mut Self {
        if let Some(t) = &mut self.text {
            t.color = c;
            t.inked = true;
        }
        if let Some(cold) = self.vis.cold.get_mut_if_some() && let Some((_, ic)) = &mut cold.icon {
            *ic = c;
            cold.icon_inked = true;
        }
        let has_input = if let Some(i) = self.cold.get_mut_if_some().and_then(|cold| cold.input.as_deref_mut()) {
            if !i.placeholder_inked { i.placeholder_color = c.alpha(0.4); }
            if !i.caret_inked { i.caret = c; }
            if !i.selection_inked { i.selection = c.alpha(0.2); }
            true
        } else {
            false
        };
        if has_input {
            self.sync_input_rich();
        }
        self
    }
    #[inline]
    pub fn align(mut self, a: Align) -> Self {
        self.set_align(a);
        self
    }
    /// [`El::align`] in place.
    #[inline]
    pub fn set_align(&mut self, a: Align) -> &mut Self {
        if let Some(t) = &mut self.text {
            t.align = a;
        }
        self
    }
    /// Vertically aligns a text element's or [`text_input`](crate::text_input)'s laid-out lines
    /// inside its content box when the box is taller than the text, as CSS Box Alignment
    /// `align-content` does (`Align::Center` matches HTML `<input type="text">` vertical
    /// centring). Defaults to [`Align::Start`] (top of the content box).
    ///
    /// Moves glyphs, placeholder, selection highlight, caret, IME candidate area, inline links,
    /// pills, pointer hit-testing, [`Runtime::text_offset_at`](crate::Runtime::text_offset_at),
    /// [`Cx::caret`](crate::Cx::caret), [`Cx::field_selection_rects`](crate::Cx::field_selection_rects),
    /// and accessibility text-run bounds together. Auto-height fields and boxes no taller than the
    /// text receive zero vertical shift.
    ///
    /// ```no_run
    /// # use rune::{Align, TextEdit, TextStyle, text_input};
    /// let edit = TextEdit::new("Search");
    /// let field = text_input(&edit, TextStyle::new(14.0, 400.0).lh(20.0))
    ///     .single_line()
    ///     .h(56.0)
    ///     .px(12.0)
    ///     .align_y(Align::Center);
    /// ```
    #[inline]
    pub fn align_y(mut self, a: Align) -> Self {
        self.set_align_y(a);
        self
    }
    /// [`El::align_y`] in place.
    #[inline]
    pub fn set_align_y(&mut self, a: Align) -> &mut Self {
        if let Some(t) = &mut self.text && t.cold.align_y != a {
            t.cold.align_y = a;
        }
        self
    }
    /// Draw this text in capitals or lower case without changing its string, like SwiftUI's
    /// `textCase`: `text("Text channels").text_case(TextCase::Uppercase)` draws "TEXT CHANNELS".
    /// The dev tree, tools and link labels keep the authored text, and [`Span`] and [`Link`]
    /// ranges stay byte ranges of it. Casing follows `str::to_uppercase`, so "ß" draws "SS".
    /// Text fields draw what is typed and ignore it.
    pub fn text_case(mut self, case: crate::TextCase) -> Self {
        if let Some(t) = &mut self.text {
            t.case = Some(case);
        }
        self
    }
    /// Spread each wrapped line but the last to fill the width; the last line and unwrapped text
    /// keep [`El::align`].
    pub fn justify(mut self) -> Self {
        if let Some(t) = &mut self.text {
            t.justify = true;
        }
        self
    }
    /// Indents the first formatted line of a [`text`](crate::text()), [`rich`], or
    /// [`text_input`] element by `points` from its inline-start edge,
    /// matching CSS Text Module Level 3 `text-indent: <length>`.
    ///
    /// Positive values indent the first line inward; negative values pull it outward into leading
    /// padding (for example `.pl(48.0).text_indent(-48.0)` for a hanging indent). Mirrors to the
    /// right edge in right-to-left text. Subsequent wrapped lines and lines after explicit `\n`
    /// breaks stay unindented. Non-finite values are ignored.
    ///
    /// ```no_run
    /// # use rune::text;
    /// let msg = text("10:42 Ada: Wrapping message body aligned under the body column.")
    ///     .w(280.0)
    ///     .pl(48.0)
    ///     .wrap()
    ///     .text_indent(-48.0);
    /// ```
    #[inline]
    pub fn text_indent(mut self, points: f32) -> Self {
        self.set_text_indent(points);
        self
    }
    /// [`El::text_indent`] in place.
    #[inline]
    pub fn set_text_indent(&mut self, points: f32) -> &mut Self {
        if points.is_finite()
            && let Some(t) = &mut self.text
        {
            let points = if points == 0.0 { 0.0 } else { points };
            if t.cold.text_indent != points {
                t.cold.text_indent = points;
            }
        }
        self
    }
    /// Measure this text as the grapheme at byte `at` of `run`, shaped as one line in the same
    /// style, instead of on its own rounded up to a device pixel. Neighbouring cells of one run
    /// then sit at its fractional, kerned advances and together take its width, and each draws
    /// the run's own glyphs for its grapheme (contextual forms included).
    pub fn advance_in(mut self, run: &crate::text::Run, at: usize) -> Self {
        if let Some(t) = &mut self.text {
            t.cold.advance_in = Some((run.clone(), at));
        }
        self
    }
    /// A filled disc in a list's outside marker column. The column's inline end is where the
    /// item's text begins; its top is the top of the first line. Its width, and thus wrapping,
    /// is up to the caller. The disc's diameter and inset resolve from the primary font's
    /// metrics in whole device pixels during layout, also after a zoom or display change.
    pub fn outside_list_disc(mut self, font: TextStyle) -> Self {
        let (a, d) = crate::text::primary_extents(font).unwrap_or((0.8, 0.2));
        self.cold_mut().list_disc = Some([a * font.size, d.abs() * font.size, font.line_px()]);
        self.radius(999.0).a11y_hidden()
    }
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
