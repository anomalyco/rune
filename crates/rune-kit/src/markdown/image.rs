//! Images (`![alt](src)`), as `markdown.css` draws them: at their own size up to the text's
//! width, rounded 4, 24 above and below; until the app has the picture (or when it never will),
//! Chromium's broken-image glyph and the alt text.

use rune::{Image, Pixels};

use super::*;

/// The picture an app has for an image's source.
pub(super) type ImageWire = Rc<dyn Fn(&str) -> Option<Image>>;
/// Wraps a shown image (see [`Markdown::image_frame`]).
pub(super) type ImagePress = Rc<dyn Fn(El, &ImageFrame<'_>) -> El>;

/// An image [`Markdown::image_frame`] is wrapping.
#[non_exhaustive]
pub struct ImageFrame<'a> {
    /// Its source as written.
    pub src: &'a str,
    /// Its alt text.
    pub alt: &'a str,
    /// The picture it shows.
    pub image: &'a Image,
}

/// Chromium's 16 point broken-image glyph, as straight-alpha RGBA.
const BROKEN: &[u8; 1024] = include_bytes!("broken-image.rgba");

thread_local! {
    static BROKEN_PIXELS: Pixels = {
        let p = Pixels::new();
        p.set(16, 16, BROKEN.to_vec());
        p
    };
}

impl Markdown {
    /// The picture for an image's source as written (`![alt](src)`): a decoded or decoding
    /// [`Image`] from the app's cache, or `None` when it has none. Until it decodes, and without
    /// one, the image shows Chromium's broken-image glyph and its alt text, as a web page does
    /// for an `<img>` with no picture. Without this, every image does.
    pub fn images(mut self, f: impl Fn(&str) -> Option<Image> + 'static) -> Self {
        self.images = Some(Rc::new(f));
        self
    }

    /// Draws `f(image, frame)` in place of each image that shows its picture, so an app can
    /// open it larger on a press (`markdown.css`'s `cursor: zoom-in`, a button named by its alt
    /// text) or key it for a flight into a viewer.
    pub fn image_frame(mut self, f: impl Fn(El, &ImageFrame<'_>) -> El + 'static) -> Self {
        self.on_image = Some(Rc::new(f));
        self
    }
}

impl Renderer {
    pub(super) fn image(&self, src: &str, alt: &str, title: &str) -> El {
        let shown = self.images.as_ref().and_then(|f| f(src)).filter(|i| i.is_ready() && i.error().is_none()).and_then(|i| Some((i.size()?, i)));
        let Some(([w, _], picture)) = shown else { return self.broken(alt) };
        // Its own size in points, as a browser draws an image's pixels at 1x, up to the text's width.
        let mut e = el().role("image").label(if alt.is_empty() { "Image preview" } else { alt }).w_full().max_w(w as f32).pixels(picture.pixels()).radius(4.0).cursor(Cursor::ZoomIn);
        if !title.is_empty() {
            e = e.help(title);
        }
        match &self.on_image {
            Some(frame) => frame(e, &ImageFrame { src, alt, image: &picture }),
            None => e,
        }
    }

    /// An image without its picture: the broken-image glyph and the alt text after it, one
    /// line tall; nothing for an image with no alt text, as a browser leaves it out.
    fn broken(&self, alt: &str) -> El {
        if alt.is_empty() {
            return el().w_full().h(0.0);
        }
        let glyph = BROKEN_PIXELS.with(|p| el().size(16.0, 16.0).pixels(p).a11y_hidden());
        let ink = self.flow.get().ink.unwrap_or(self.s.text);
        row().role("image").label(alt).w_full().cursor(Cursor::ZoomIn).child(glyph).child(self.with_reveal(text(alt.to_owned()).font(self.s.body).color(ink)).shrink())
    }
}
