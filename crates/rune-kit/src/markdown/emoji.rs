//! Custom emoji pictures: `:name:` shortcodes in prose drawn as the app's images, standing on
//! the baseline one line tall, as a chat app's custom emoji do. They're drawn only, with no hover
//! or press of their own.

use rune::{InlineImage, Pixels};

use super::*;

/// The picture an app has for a shortcode's name.
pub(super) type EmojiWire = Rc<dyn Fn(&str) -> Option<Pixels>>;

/// An emoji's height against the text's size: a little taller than its letters, inside the
/// paragraph's 1.4 em line.
const HEIGHT_EM: f32 = 1.2;
/// How far below the baseline it reaches, so it sits centred on the letters beside it.
const DESCENT_EM: f32 = 0.25;

impl Markdown {
    /// The picture for each `:name:` shortcode in prose (`:party_parrot:` asks for
    /// `"party_parrot"`), or `None` to leave it as written. A shortcode is a name of ASCII
    /// letters, digits, `_`, `+` and `-` between two colons, outside inline code, math and links
    /// that show their own address (a bare URL). The
    /// picture draws 1.2 em tall on the baseline, as wide as its aspect ratio makes it (square
    /// until it has pixels), in place of the shortcode's bytes; selection, copy and search
    /// still see `:name:`. Hand back the same [`Pixels`] for a name each build, from the app's
    /// cache: a document with this set isn't served from Markdown's render cache. Table columns
    /// size for the picture, not the shortcode text. The picture is drawn only: it takes no
    /// hover or press of its own.
    ///
    /// ```
    /// # use rune::Pixels;
    /// # use rune_kit::Markdown;
    /// # let parrot = Pixels::new();
    /// let md = Markdown::new("Shipped :party_parrot:")
    ///     .emoji(move |name| (name == "party_parrot").then(|| parrot.clone()));
    /// # let _ = md;
    /// ```
    pub fn emoji(mut self, f: impl Fn(&str) -> Option<Pixels> + 'static) -> Self {
        self.emoji = Some(Rc::new(f));
        self
    }
}

/// The `:name:` shortcodes in `inl`'s text that don't touch inline code, math or a link that
/// shows its own address (`https://example.com/a:b:c`): each one's byte range (colons included)
/// and its name. Neighbours may share a colon (`:b:smile:` gives
/// `b` and `smile`); [`Renderer::emoji_hits`] keeps the first the app has a picture for.
pub(super) fn shortcodes(inl: &Inline) -> Vec<(Range<usize>, &str)> {
    let s = inl.text.as_bytes();
    let name_byte = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+' | b'-');
    let skip = |a: usize, b: usize| {
        inl.spans.iter().any(|&(x, y, st)| matches!(st, Style::Code | Style::CodePad | Style::Math) && x < b && a < y)
            || inl.links.iter().any(|(x, y, url)| *x < b && a < *y && bidi::shows_address(inl, *x, *y, url))
    };
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(off) = s.get(i..).and_then(|rest| rest.iter().position(|&b| b == b':')) {
        let open = i + off;
        let len = s[open + 1..].iter().take_while(|&&b| name_byte(b)).count();
        let close = open + 1 + len;
        if len == 0 || s.get(close) != Some(&b':') {
            i = open + 1;
            continue;
        }
        if !skip(open, close + 1) {
            out.push((open..close + 1, &inl.text[open + 1..close]));
        }
        i = close;
    }
    out
}

impl Renderer {
    /// The shortcodes in `inl` the app has a picture for, left to right, none sharing a colon.
    fn emoji_hits(&self, inl: &Inline) -> Vec<(Range<usize>, Pixels)> {
        let Some(f) = &self.emoji else { return Vec::new() };
        let mut out: Vec<(Range<usize>, Pixels)> = Vec::new();
        for (range, name) in shortcodes(inl) {
            if out.last().is_some_and(|(r, _)| range.start < r.end) {
                continue;
            }
            if let Some(px) = f(name) {
                out.push((range, px));
            }
        }
        out
    }

    /// Each of [`Self::emoji_hits`] with the width it draws at in `font`.
    fn emoji_sized(&self, inl: &Inline, font: TextStyle) -> Vec<(Range<usize>, Pixels, f32)> {
        let h = font.size * HEIGHT_EM;
        self.emoji_hits(inl).into_iter().map(|(range, px)| {
            let (pw, ph) = px.size();
            let w = if pw > 0 && ph > 0 { h * pw as f32 / ph as f32 } else { h };
            (range, px, w)
        }).collect()
    }

    /// `e`, the text of `inl` in `font`, with each shortcode the app has a picture for drawn as it.
    pub(super) fn with_emoji(&self, mut e: El, inl: &Inline, font: TextStyle) -> El {
        let h = font.size * HEIGHT_EM;
        let descent = font.size * DESCENT_EM;
        for (range, pixels, w) in self.emoji_sized(inl, font) {
            e = e.inline_image(range, InlineImage::new(pixels, w, h - descent, descent));
        }
        e
    }

    /// `inl`'s `(min, max)` content widths in `font`, measured on its text, corrected for the
    /// shortcodes drawn as pictures: each one's text width out, its picture's width in. The
    /// longest unbreakable run (`min`) is only capped at the corrected `max` and kept at least as
    /// wide as the widest picture, so a shortcode glued into a longer word leaves it a little wide.
    pub(super) fn emoji_content_widths(&self, inl: &Inline, font: TextStyle, (min, max): (f32, f32)) -> (f32, f32) {
        let sized = self.emoji_sized(inl, font);
        if sized.is_empty() {
            return (min, max);
        }
        let mut max = max;
        let mut widest = 0.0f32;
        for (range, _, w) in &sized {
            let text_w = inl.text.get(range.clone()).map_or(0.0, |t| rune::text::measure(t, &[], font, None).0);
            max += w - text_w;
            widest = widest.max(*w);
        }
        let max = max.max(widest);
        (min.min(max).max(widest), max)
    }

    /// Feeds what [`Self::with_emoji`] draws for `inl` into a memo digest, without allocating.
    pub(super) fn hash_emoji(&self, inl: &Inline, h: &mut impl Hasher) {
        if self.emoji.is_none() {
            return;
        }
        let hits = self.emoji_hits(inl);
        hits.len().hash(h);
        for (range, px) in hits {
            range.start.hash(h);
            std::fmt::write(&mut super::memo::HashFmt(&mut *h), format_args!("{px:?}")).ok();
        }
    }
}

#[cfg(test)]
#[path = "emoji_tests.rs"]
mod tests;
