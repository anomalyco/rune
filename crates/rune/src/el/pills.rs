//! `El::pill` and `El::pills`: rounded chips around byte ranges of static text, drawn and
//! wrapped like a [`text_input`](crate::text_input)'s [`Mark::Pill`](crate::Mark::Pill) chips.
use std::ops::Range;

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
    /// ```ignore
    /// text("ask @reviewer to look").pill(4..13, PillStyle::new().bg(chip).fg(ink))
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
    /// word after `at`, or over a pill, it draws nothing.
    ///
    /// ```ignore
    /// rich("see the docs", spans).inline_icon(8, InlineIcon::glyph(&GLOBE).size(14.0).gap(4.0))
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
        let InlineIcon { icon, image, size, gap, radius, color } = icon;
        let bare = Some(Box::new(BareIcon { size, gap, radius, image, replace: None }));
        t.cold.pills.insert(at, PillChip { start, end, icon, image: None, bg: None, fg: color, ring: None, radius: None, bare });
        self
    }

    /// Draws `image` in place of bytes `range` of a [`text`](crate::text()) or
    /// [`rich`](crate::rich) element, standing on the baseline as a glyph does (rendered math
    /// in a sentence). The bytes take no room and draw nothing, but selection, copy and search
    /// still see them. A line grows to fit a taller or deeper image, and one too wide for the
    /// rest of a line moves whole to the next. Ranges are UTF-8 bytes on char boundaries with
    /// no newline; empty, out-of-range or overlapping ones are skipped. On a field this does
    /// nothing.
    ///
    /// ```ignore
    /// text("so $e^x$ grows").inline_image(3..8, InlineImage::new(pixels, 14.0, 9.0, 0.5))
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
        let InlineImage { pixels, width, ascent, descent, bleed } = image;
        let replace = Some(Replace { width, ascent, descent, bleed });
        let bare = Some(Box::new(BareIcon { size: 0.0, gap: 0.0, radius: 0.0, image: Some(pixels), replace }));
        t.cold.pills.insert(at, PillChip { start, end, icon: None, image: None, bg: None, fg: None, ring: None, radius: None, bare });
        self
    }
}

/// An image set in text by [`El::inline_image`]: `width` points wide, `ascent` points above
/// the baseline and `descent` below it, the image scaled to fit that box.
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
}

impl InlineIcon {
    /// `icon`, drawn in [`Self::color`] or the element's text colour.
    pub fn glyph(icon: &'static crate::Icon) -> Self {
        Self { icon: Some(icon), image: None, size: 14.0, gap: 4.0, radius: 0.0, color: None }
    }
    /// An image (decoded pixels, such as a site's favicon), scaled to fit the square.
    pub fn image(pixels: crate::Pixels) -> Self {
        Self { icon: None, image: Some(pixels), size: 14.0, gap: 4.0, radius: 0.0, color: None }
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
