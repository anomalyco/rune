//! `El::highlight_texts`: paint-only highlights on every text a subtree holds, such as the
//! matches of a search, without wrapping each `text` or `rich` call that builds them.
use super::*;

impl El {
    /// Calls `f` with what each [`text`](crate::text()) and [`rich`] element in this subtree
    /// shows (this element included), and adds the [`TextHighlight`](crate::TextHighlight)s it
    /// returns, as [`El::text_highlight`] would, so a settings search can mark its matches on
    /// whatever page it opens, inline links and styled runs included. Highlights only paint:
    /// they never reshape text or move line breaks.
    ///
    /// Ranges are UTF-8 bytes of the string `f` is given; an empty one, or one that ends past
    /// the text or off a char boundary, is skipped. Text fields, secure ([`El::secure`]) and
    /// live ([`El::live_text`]) texts are left alone and `f` isn't called for them. It sees the
    /// subtree as built so far: [`memo`](crate::memo) subtrees (key their deps on the query
    /// instead) and rows that [`virtual_rows`](crate::virtual_rows) builds later are not visited,
    /// so call it inside those builders too.
    ///
    /// ```
    /// use rune::{Span, TextHighlight, col, oklch, rich, text};
    ///
    /// // An ASCII query against an ASCII-lowercased copy keeps byte offsets those of `s`
    /// // (`to_lowercase` can change UTF-8 lengths, as `İ` does).
    /// let query = "dark";
    /// let page = col()
    ///     .child(text("Dark mode"))
    ///     .child(rich("Follow the system's dark setting", vec![Span::new(0, 6).weight(600.0)]))
    ///     .highlight_texts(|s| {
    ///         let lower = s.to_ascii_lowercase();
    ///         lower.match_indices(query).map(|(at, m)| TextHighlight::new(at..at + m.len(), oklch(0.9, 0.15, 95.0))).collect::<Vec<_>>()
    ///     });
    /// # let _ = page;
    /// ```
    pub fn highlight_texts<I: IntoIterator<Item = crate::text::TextHighlight>>(mut self, mut f: impl FnMut(&str) -> I) -> Self {
        let mut each = |s: &str| f(s).into_iter().collect::<Vec<_>>();
        self.highlight_texts_in(&mut each);
        self
    }

    fn highlight_texts_in(&mut self, f: &mut dyn FnMut(&str) -> Vec<crate::text::TextHighlight>) {
        // A memo placeholder stands for a shared, cached subtree: leave it as it was cached.
        let shown = self.memo.is_none() && self.cold.input.is_none();
        if let Some(t) = self.text.as_mut().filter(|t| shown && !t.redacted && t.cold.live.is_none()) {
            let found = f(t.content_str());
            let content = t.content_str();
            let fits = |h: &crate::text::TextHighlight| {
                let (a, b) = (h.start as usize, h.end as usize);
                a < b && b <= content.len() && content.is_char_boundary(a) && content.is_char_boundary(b)
            };
            let found: Vec<_> = found.into_iter().filter(fits).collect();
            if !found.is_empty() {
                t.cold.highlights.extend(found);
            }
        }
        for child in &mut self.children {
            child.highlight_texts_in(f);
        }
    }
}

#[cfg(test)]
#[path = "highlight_texts_tests.rs"]
mod tests;
