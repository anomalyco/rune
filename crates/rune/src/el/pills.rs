//! `El::pill` and `El::pills`: rounded chips around byte ranges of static text, drawn and
//! wrapped like a [`text_input`](crate::text_input)'s [`Mark::Pill`](crate::Mark::Pill) chips.
use std::ops::Range;
use std::rc::Rc;

use crate::Color;
use crate::el::{El, PillStyle};
use crate::text::{BareIcon, PillChip, Replace, Span};

impl El {
    /// Draws bytes `range` of a [`text`](crate::text()) or [`rich`](crate::rich) element as a
    /// pill: a rounded chip in `style`'s colour (default: the rich style's link colour at 20%,
    /// or [`El::pill_colors`]) with its label in `style`'s text colour and its icon before it,
    /// the way a field draws a [`Mark::Pill`](crate::Mark::Pill). The label never splits across
    /// lines, and the chip's padding takes room in the line, so the text around it moves aside.
    /// Selection and copy treat the label as text. Ranges are authored UTF-8 bytes on char
    /// boundaries; empty, out-of-range or overlapping ones are skipped. On a field this does
    /// nothing: style its pills with [`El::pill_style`].
    ///
    /// ```
    /// # use rune::{Color, PillStyle, text};
    /// # let (chip, ink) = (Color::WHITE, Color::BLACK);
    /// # let _ =
    /// text("ask @reviewer to look").pill(4..13, PillStyle::new().bg(chip).fg(ink))
    /// # ;
    /// ```
    pub fn pill(self, range: Range<usize>, style: PillStyle) -> Self {
        self.pills([(range, style)])
    }

    /// [`El::pill`] for several ranges.
    pub fn pills(mut self, pills: impl IntoIterator<Item = (Range<usize>, PillStyle)>) -> Self {
        if self.cold.input.is_some() {
            return self;
        }
        let Some(t) = &mut self.text else { return self };
        for (r, st) in pills {
            let ok = r.start < r.end && t.content.is_char_boundary(r.start) && t.content.is_char_boundary(r.end) && r.end <= t.content.len();
            let (start, end) = (r.start as u32, r.end as u32);
            if !ok || t.cold.pills.iter().any(|p| p.start < end && start < p.end) {
                continue;
            }
            let at = t.cold.pills.partition_point(|p| p.start < start);
            t.cold.pills.insert(at, PillChip {
                start,
                end,
                icon: st.icon,
                image: None,
                bg: st.bg,
                fg: st.fg,
                ring: st.ring,
                radius: st.radius,
                pad_x: st.pad_x,
                margin_x: st.margin_x,
                bare: None,
            });
        }
        t.spans = with_pill_colors(&t.spans, &t.cold.pills);
        self
    }

    /// [`El::pill`] with a leading full-colour [`Pixels`](crate::Pixels) image (such as a
    /// service logo or avatar) drawn in the leading icon's slot and room, scaled to fit without
    /// tinting. Takes precedence over `style.icon(..)`. On a field this does nothing: attach
    /// [`El::pill_image`] instead.
    ///
    /// ```
    /// # use rune::{Color, PillStyle, Pixels, text};
    /// # let chip = Color::WHITE;
    /// # let border = Color::WHITE;
    /// # let gmail_logo = Pixels::new();
    /// # let _ =
    /// text("open @Gmail now").pill_with_image(
    ///     5..11,
    ///     PillStyle::new().bg(chip).ring(border, 1.0).radius(999.0),
    ///     gmail_logo,
    /// )
    /// # ;
    /// ```
    pub fn pill_with_image(mut self, range: Range<usize>, style: PillStyle, image: crate::Pixels) -> Self {
        if self.cold.input.is_some() {
            return self;
        }
        let Some(t) = &mut self.text else { return self };
        let ok = range.start < range.end
            && t.content.is_char_boundary(range.start)
            && t.content.is_char_boundary(range.end)
            && range.end <= t.content.len();
        let (start, end) = (range.start as u32, range.end as u32);
        if !ok || t.cold.pills.iter().any(|p| p.start < end && start < p.end) {
            return self;
        }
        let at = t.cold.pills.partition_point(|p| p.start < start);
        t.cold.pills.insert(at, PillChip {
            start,
            end,
            icon: None,
            image: Some(image),
            bg: style.bg,
            fg: style.fg,
            ring: style.ring,
            radius: style.radius,
            pad_x: style.pad_x,
            margin_x: style.margin_x,
            bare: None,
        });
        t.spans = with_pill_colors(&t.spans, &t.cold.pills);
        self
    }
}

impl El {
    /// Sets `icon` inline in a [`text`](crate::text()) or [`rich`](crate::rich) element, before
    /// byte `at` (a char boundary), in room of its own that moves the text after it aside. It
    /// stays on the line of the word that follows it, as an icon before a link does on a web
    /// page, and sits centred on that word's letters. Its glyph takes `icon`'s colour or the
    /// element's; an image (a site's favicon) draws as it is. Selection and copy skip it. With no
    /// word after `at`, or over a pill, it draws nothing. It is decoration unless it has a name
    /// ([`InlineIcon::label`]) or a tooltip ([`InlineIcon::help`]).
    ///
    /// ```
    /// # use rune::{Icon, InlineIcon, Span, rich};
    /// # const GLOBE: Icon = Icon { name: "globe", view: 16.0, body: "" };
    /// # let spans: Vec<Span> = Vec::new();
    /// # let _ =
    /// rich("see the docs", spans).inline_icon(8, InlineIcon::glyph(&GLOBE).size(14.0).gap(4.0))
    /// # ;
    /// ```
    pub fn inline_icon(mut self, at: usize, icon: InlineIcon) -> Self {
        if self.cold.input.is_some() {
            return self;
        }
        let Some(t) = &mut self.text else { return self };
        if at >= t.content.len() || !t.content.is_char_boundary(at) {
            return self;
        }
        let word = t.content[at..].find(char::is_whitespace).unwrap_or(t.content.len() - at);
        let (start, end) = (at as u32, (at + word) as u32);
        if word == 0 || t.cold.pills.iter().any(|p| p.start < end && start < p.end) {
            return self;
        }
        let at = t.cold.pills.partition_point(|p| p.start < start);
        let InlineIcon { icon, image, size, gap, radius, color, label, help } = icon;
        let bare = Some(Box::new(BareIcon { size, gap, radius, image, replace: None, label, help }));
        t.cold.pills.insert(at, PillChip { start, end, icon, image: None, bg: None, fg: color, ring: None, radius: None, pad_x: None, margin_x: None, bare });
        self
    }

    /// Draws `image` in place of bytes `range` of a [`text`](crate::text()) or
    /// [`rich`](crate::rich) element, standing on the baseline as a glyph does (rendered math
    /// in a sentence). The bytes take no room and draw nothing, but selection, copy and search
    /// still see them. A line grows to fit a taller or deeper image, and one too wide for the
    /// rest of a line moves whole to the next. Ranges are UTF-8 bytes on char boundaries with
    /// no newline; empty, out-of-range or overlapping ones are skipped. On a field this does
    /// nothing: attach [`El::pill_inline_image`] instead.
    ///
    /// ```
    /// # use rune::{InlineImage, Pixels, text};
    /// # let pixels = Pixels::new();
    /// # let _ =
    /// text("so $e^x$ grows").inline_image(3..8, InlineImage::new(pixels, 14.0, 9.0, 0.5))
    /// # ;
    /// ```
    pub fn inline_image(mut self, range: Range<usize>, image: InlineImage) -> Self {
        if self.cold.input.is_some() {
            return self;
        }
        let Some(t) = &mut self.text else { return self };
        let c = &t.content;
        let ok = range.start < range.end
            && range.end <= c.len()
            && c.is_char_boundary(range.start)
            && c.is_char_boundary(range.end)
            && !c.as_bytes()[range.clone()].contains(&b'\n');
        let (start, end) = (range.start as u32, range.end as u32);
        if !ok || t.cold.pills.iter().any(|p| p.start < end && start < p.end) {
            return self;
        }
        let at = t.cold.pills.partition_point(|p| p.start < start);
        let bare = Some(Box::new(image.into_bare()));
        t.cold.pills.insert(at, PillChip { start, end, icon: None, image: None, bg: None, fg: None, ring: None, radius: None, pad_x: None, margin_x: None, bare });
        self
    }

    /// Draws an [`InlineImage`](crate::InlineImage) in place of a [`Mark::Pill`](crate::Mark::Pill)'s
    /// label inside a [`text_input`](crate::text_input), by the pill's id (`None`: draw the pill
    /// as a regular chip).
    ///
    /// Like [`El::inline_image`] on static text, the image stands on the baseline at its own
    /// width, ascent and descent, growing the line when taller and moving whole to the next line
    /// when too wide. The pill stays atomic (the caret steps over it and delete removes it whole),
    /// and its label stays in the field's text for copy, paste and accessibility. When `Some`,
    /// this replaces the pill's chip, label, [`El::pill_style`], [`El::pill_image`] and
    /// [`El::pill_icon`].
    ///
    /// ```
    /// # use std::collections::HashMap;
    /// # use rune::{InlineImage, Pixels, TextEdit, TextStyle, text_input};
    /// # struct App { draft: TextEdit, emoji: HashMap<String, Pixels> }
    /// # let s = App { draft: TextEdit::default(), emoji: HashMap::new() };
    /// # let body = TextStyle::new(14.0, 400.0);
    /// # let _ =
    /// text_input(&s.draft, body).pill_inline_image({
    ///     let emoji = s.emoji.clone();
    ///     move |id| emoji.get(id).cloned().map(|px| InlineImage::new(px, 18.0, 14.0, 4.0))
    /// })
    /// # ;
    /// ```
    pub fn pill_inline_image(mut self, image: impl Fn(&str) -> Option<InlineImage> + 'static) -> Self {
        if let Some(inp) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            inp.pill_inline_image = Some(Rc::new(image));
            self.sync_input_pills();
            self.sync_input_rich();
        }
        self
    }

    /// Reserves an atomic in-flow [`InlineBox`](crate::InlineBox) before UTF-8 byte offset `at`
    /// of a [`text`](crate::text()) or [`rich`](crate::rich) element and lays `child` out inside
    /// that room, like a CSS `display: inline-block` element.
    ///
    /// `child` is sized to `slot`'s width and height, moves and wraps whole with the text line,
    /// grows line height when taller than the line, and receives its own painting, hit testing,
    /// keyboard focus, and accessibility node. Selection and copy over the surrounding text skip
    /// the box. When `truncate` or `max_lines` cuts the text before `at`, the elided child is
    /// hidden from painting, hit testing, focus, and accessibility.
    ///
    /// `at` may equal `text.len()` (for a trailing badge or arrow, or an empty string). When
    /// combined with [`El::live_text`], each live update keeps the box in flow and repositions
    /// `child` inside the element's fixed box without rebuilding the view: a box authored at
    /// `at == placeholder.len()` on a non-empty placeholder stays anchored at the end as the live
    /// string grows or shrinks, while any earlier offset stays at byte `at` clamped down to the
    /// nearest UTF-8 char boundary in the new string. On a [`text_input`](crate::text_input), a
    /// non-text element, or an out-of-range / non-char-boundary `at`, this is a no-op.
    ///
    /// ```
    /// # use rune::{Color, InlineAlign, InlineBox, el, text};
    /// # let chip_bg = Color::WHITE.alpha(0.12);
    /// let slot = InlineBox::new(72.0, 18.0).align(InlineAlign::TextCenter);
    /// let badge = el().fill(chip_bg).radius(4.0).child(text("config.toml"));
    /// let _line = text("Open  and edit it.").wrap().inline(5, slot, badge);
    /// ```
    pub fn inline(mut self, at: usize, slot: crate::text::InlineBox, child: El) -> Self {
        if self.cold.input.is_some() {
            return self;
        }
        let memo_len = self.memo_children().map_or(0, |m| m.children.len());
        let has_memo = self.memo.is_some();
        let Some(t) = self.text.as_mut() else { return self };
        if at > t.content.len() || !t.content.is_char_boundary(at) {
            return self;
        }
        let anchor = crate::el::InlineAnchor {
            at: at as u32,
            trailing: at == t.content.len() && !t.content.is_empty(),
        };
        let child = child.w(slot.width()).h(slot.height());
        let at32 = at as u32;
        if child.flags.has(crate::el::ElFlags::HAS_VIRTUAL) {
            self.flags.set(crate::el::ElFlags::HAS_VIRTUAL, true);
        }
        if !has_memo && self.children.len() == t.cold.inline_boxes.len() {
            let pos = t.cold.inline_boxes.partition_point(|b| b.at <= at32);
            self.children.insert(pos, child);
            for b in &mut t.cold.inline_boxes[pos..] {
                b.child += 1;
            }
            t.cold.inline_boxes.insert(pos, crate::text::InlineBoxRun { at: at32, slot, child: pos as u32 });
            t.cold.inline_anchors.insert(pos, anchor);
        } else {
            self.flags.set(crate::el::ElFlags::MEMO_EXTENDED, has_memo);
            let ord = memo_len + self.children.len();
            self.children.push(child);
            t.cold.inline_boxes.push(crate::text::InlineBoxRun { at: at32, slot, child: ord as u32 });
            t.cold.inline_anchors.push(anchor);
        }
        t.shape = None;
        self
    }
}

/// An image set in text by [`El::inline_image`] or in a field pill by [`El::pill_inline_image`]:
/// `width` points wide, `ascent` points above the baseline and `descent` below it, the image
/// scaled to fit that box.
#[derive(Clone, Debug)]
pub struct InlineImage {
    pixels: crate::Pixels,
    width: f32,
    ascent: f32,
    descent: f32,
    bleed: f32,
}

fn points(v: f32) -> f32 {
    if v.is_finite() { v.max(0.0) } else { 0.0 }
}

impl InlineImage {
    pub fn new(pixels: crate::Pixels, width: f32, ascent: f32, descent: f32) -> Self {
        Self { pixels, width: points(width), ascent: points(ascent), descent: points(descent), bleed: 0.0 }
    }
    /// Draws the image `points` past each edge of its box, for an image with clear margins
    /// (rendered math's padding) that the line shouldn't make room for.
    pub fn bleed(mut self, points: f32) -> Self {
        self.bleed = self::points(points);
        self
    }
    pub(crate) fn into_bare(self) -> BareIcon {
        let Self { pixels, width, ascent, descent, bleed } = self;
        BareIcon {
            size: 0.0,
            gap: 0.0,
            radius: 0.0,
            image: Some(pixels),
            replace: Some(Replace { width, ascent, descent, bleed }),
            label: None,
            help: None,
        }
    }
}

/// An icon set inline in text by [`El::inline_icon`]: a glyph or an image, `size` points square
/// (14 by default), `gap` points before the text after it (4).
#[derive(Clone, Debug)]
pub struct InlineIcon {
    icon: Option<&'static crate::Icon>,
    image: Option<crate::Pixels>,
    size: f32,
    gap: f32,
    radius: f32,
    color: Option<Color>,
    label: Option<String>,
    help: Option<String>,
}

impl InlineIcon {
    /// `icon`, drawn in [`Self::color`] or the element's text colour.
    pub fn glyph(icon: &'static crate::Icon) -> Self {
        Self { icon: Some(icon), image: None, size: 14.0, gap: 4.0, radius: 0.0, color: None, label: None, help: None }
    }
    /// An image (decoded pixels, such as a site's favicon), scaled to fit the square.
    pub fn image(pixels: crate::Pixels) -> Self {
        Self { icon: None, image: Some(pixels), size: 14.0, gap: 4.0, radius: 0.0, color: None, label: None, help: None }
    }
    /// Its side in points.
    pub fn size(mut self, points: f32) -> Self {
        self.size = if points.is_finite() { points.max(0.0) } else { 0.0 };
        self
    }
    /// Its room after it, before the text, in points.
    pub fn gap(mut self, points: f32) -> Self {
        self.gap = if points.is_finite() { points.max(0.0) } else { 0.0 };
        self
    }
    /// An image's corner radius in points.
    pub fn radius(mut self, points: f32) -> Self {
        self.radius = if points.is_finite() { points.max(0.0) } else { 0.0 };
        self
    }
    /// A glyph's colour.
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    /// Its name for assistive technology, as an `<img>`'s `aria-label`: the text's element
    /// gains an image named `label` in the accessibility tree, where the icon draws. Without
    /// one the icon is decoration and a reader skips it. An empty name is none.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        let label = label.into();
        self.label = (!label.is_empty()).then_some(label);
        self
    }
    /// A tooltip shown while the pointer rests on the icon, as [`El::help`] shows an element's
    /// (an HTML `title`): after the same delay, in the same look, under the icon. It is the
    /// icon's own, over any help its element has. A named icon also reads it as its description
    /// when it says something its name doesn't. An empty text is none.
    pub fn help(mut self, text: impl Into<String>) -> Self {
        let text = text.into();
        self.help = (!text.is_empty()).then_some(text);
        self
    }
}

/// `spans` with each pill's label colour over it.
pub(crate) fn with_pill_colors(spans: &[Span], pills: &[PillChip]) -> Vec<Span> {
    if pills.iter().all(|p| p.fg.is_none()) {
        return crate::text::flatten_spans(spans);
    }
    let mut all = spans.to_vec();
    all.extend(pills.iter().filter_map(|p| Some(Span { start: p.start, end: p.end, color: Some(p.fg?), ..Span::default() })));
    crate::text::flatten_spans(&all)
}

#[cfg(test)]
mod tests;
