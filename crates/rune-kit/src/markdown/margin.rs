//! The space around lists, quotes and rules, beside [`MarkdownStyle::paragraph_margin`].

use super::MarkdownStyle;
use super::table::pts;

impl MarkdownStyle {
    /// The space above and below a list (8 and 12 points by default), collapsed with a
    /// neighbour's margin (and through a block quote that wraps the list). A list nested in a
    /// list item keeps its own 4 each way.
    ///
    /// ```
    /// # use rune_kit::MarkdownStyle;
    /// let _s = MarkdownStyle::neutral(false).paragraph_margin(0.0, 6.0).list_margin(0.0, 6.0);
    /// ```
    pub fn list_margin(mut self, top: f32, bottom: f32) -> Self {
        self.block_margin[0] = [pts(top), pts(bottom)];
        self
    }
    /// The space above and below a block quote (16 and 16 points by default), collapsed with a
    /// neighbour's margin.
    ///
    /// ```
    /// # use rune_kit::MarkdownStyle;
    /// let _s = MarkdownStyle::neutral(false).quote_margin(8.0, 8.0);
    /// ```
    pub fn quote_margin(mut self, top: f32, bottom: f32) -> Self {
        self.block_margin[1] = [pts(top), pts(bottom)];
        self
    }
    /// The space above and below a horizontal rule (32 and 32 points by default), collapsed with
    /// a neighbour's margin.
    ///
    /// ```
    /// # use rune_kit::MarkdownStyle;
    /// let _s = MarkdownStyle::neutral(false).rule_margin(16.0, 16.0);
    /// ```
    pub fn rule_margin(mut self, top: f32, bottom: f32) -> Self {
        self.block_margin[2] = [pts(top), pts(bottom)];
        self
    }
}
