//! `El::pill` and `El::pills`: rounded chips around byte ranges of static text, drawn and
//! wrapped like a [`text_input`](crate::text_input)'s [`Mark::Pill`](crate::Mark::Pill) chips.
use std::ops::Range;

use crate::el::{El, PillStyle};
use crate::text::{PillChip, Span};

impl El {
    /// Draws bytes `range` of a [`text`](crate::text) or [`rich`](crate::rich) element as a
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
        if self.input.is_some() {
            return self;
        }
        let Some(t) = &mut self.text else { return self };
        for (r, st) in pills {
            let ok = r.start < r.end && t.content.is_char_boundary(r.start) && t.content.is_char_boundary(r.end) && r.end <= t.content.len();
            let (start, end) = (r.start as u32, r.end as u32);
            if !ok || t.pills.iter().any(|p| p.start < end && start < p.end) {
                continue;
            }
            let at = t.pills.partition_point(|p| p.start < start);
            t.pills.insert(at, PillChip { start, end, icon: st.icon, bg: st.bg, fg: st.fg });
        }
        t.spans = with_pill_colors(&t.spans, &t.pills);
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
