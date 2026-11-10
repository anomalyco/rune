//! `El::dir_by_paragraph`: each paragraph of a text in the direction of its own words, as CSS's
//! `unicode-bidi: plaintext` lays a block's paragraphs out.
use super::*;

impl El {
    /// Lays each paragraph of this text (a line a newline ends) out in the direction of its own
    /// first strong character, as CSS's `unicode-bidi: plaintext` does: right to left when that's
    /// Arabic or Hebrew, else left to right, left to right too when it has none, whatever
    /// direction the element itself lays out in. Each paragraph's lines start at its own start
    /// edge. The element's box, padding and place stay where its direction puts them.
    ///
    /// Call it last, after the text's spans, links, pills, highlights and copy settings. A text
    /// whose paragraphs all take one direction stays one text. One whose paragraphs differ
    /// becomes a column of one text per paragraph, which a selection crosses as one inside a
    /// [`El::selection_group`] and copies joined by newlines. An editable, secure, live,
    /// truncated or line-clamped text stays one text in its first paragraph's direction.
    pub fn dir_by_paragraph(mut self) -> El {
        let Some(t) = self.text.as_mut() else { return self };
        t.plaintext = true;
        t.shape = None;
        let paragraphs = paragraphs(&t.content);
        let dirs: Vec<Direction> = paragraphs.iter().map(|&(a, b)| Direction::of(&t.content[a..b]).unwrap_or(Direction::Ltr)).collect();
        let whole = self.cold.input.is_some() || t.redacted || t.live.is_some() || t.advance_in.is_some() || t.truncate || t.truncate_fade || t.max_lines > 0;
        if whole || dirs.iter().all(|&d| d == dirs[0]) {
            return self;
        }
        let Some(t) = self.text.take() else { return self };
        let cursor = self.handlers.cursor;
        let last = paragraphs.len() - 1;
        let pieces: Vec<El> = paragraphs.iter().enumerate().map(|(k, &range)| {
            let mut e = text(String::new()).key(format!("p{k}"));
            let links = paragraph(&t, k, range, k == last, &mut e);
            if let Some(c) = cursor { e = e.cursor(c); }
            if links { e.cold_mut().focus.enabled = true; }
            e
        }).collect();
        // The links' Tab stops went with them.
        if !t.links.is_empty() && let Some(c) = self.cold.get_mut_if_some() { c.focus.enabled = false; }
        if self.role == RoleTag::TEXT { self.role = RoleTag::GROUP; }
        self.style.set_flex_direction(FlexDirection::Column);
        let kept = std::mem::take(&mut self.children);
        self.children = pieces.into_iter().chain(kept).collect();
        self
    }
}

/// The byte ranges of `content`'s paragraphs, the separators between them left out: a UAX #9
/// paragraph separator (a newline) ends each but the last, CR LF as one.
fn paragraphs(content: &str) -> Vec<(usize, usize)> {
    use icu_properties::{CodePointMapData, props::BidiClass};
    let classes = CodePointMapData::<BidiClass>::new();
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = content.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if classes.get(c) != BidiClass::ParagraphSeparator {
            continue;
        }
        let end = match chars.peek() {
            Some(&(j, '\n')) if c == '\r' => {
                chars.next();
                j + 1
            }
            _ => i + c.len_utf8(),
        };
        out.push((start, i));
        start = end;
    }
    out.push((start, content.len()));
    out
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
    p.links = t.links.iter().filter_map(|l| clip(l.start, l.end).map(|(start, end)| Link { start, end, ..l.clone() })).collect();
    p.uncopied = t.uncopied.iter().filter_map(|&(s, e)| clip(s, e)).collect();
    p.rings = t.rings.iter().filter_map(|&(s, e, c, w)| clip(s, e).map(|(s, e)| (s, e, c, w))).collect();
    p.strikes = t.strikes.iter().filter_map(|&(s, e, c)| clip(s, e).map(|(s, e)| (s, e, c))).collect();
    p.highlights = t.highlights.iter().filter_map(|h| clip(h.start, h.end).map(|(start, end)| crate::text::TextHighlight { start, end, ..h.clone() })).collect();
    // A chip lies inside one line; one across a newline has no paragraph to sit in.
    p.pills = t.pills.iter().filter(|c| a32 <= c.start && c.end <= b32).map(|c| crate::text::PillChip { start: c.start - a32, end: c.end - a32, ..c.clone() }).collect();
    p.blocks = t.blocks.get(k).copied().into_iter().collect();
    p.soft_breaks.clear();
    p.copy_breaks = [if k == 0 { t.copy_breaks[0] } else { 1 }, if last { t.copy_breaks[1] } else { 1 }];
    p.copy_prefix = if k == 0 { t.copy_prefix.clone() } else { None };
    // Fractional line heights, so the stacked paragraphs keep the one text's line pitch.
    p.exact_lines = true;
    p.shape = None;
    let links = !p.links.is_empty();
    e.text = Some(crate::cold::SharedBox::new(p));
    links
}
