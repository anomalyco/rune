//! The app's fonts as the page's own (`FontFace`s in `document.fonts`), so the semantic mirror's
//! text lays out in the faces Rune draws: the browser's Find highlight lands on the glyphs, and a
//! link the pointer is over is where it's drawn. Shared once the first frame is up, from the
//! bytes Rune already has: nothing downloads twice.

use std::sync::Arc;

use parley::fontique::{Blob, Collection, CollectionOptions, FontStyle};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/src/platform/web/platform.js")]
extern "C" {
    #[wasm_bindgen(js_name = addFontFace)]
    fn add_font_face(family: &str, bytes: &[u8], weight: &str, style: &str);
}

/// [`share`] in a task of its own, so the frame just presented isn't followed by a long one.
pub(super) fn share_later(fonts: &'static [&'static [u8]]) {
    if fonts.is_empty() {
        return;
    }
    wasm_bindgen_futures::spawn_local(async move {
        let _ = wasm_bindgen_futures::JsFuture::from(super::platform::next_task()).await;
        share(fonts.iter().copied());
    });
}

/// Adds each font file's faces to the page under the family names Rune knows them by.
pub(super) fn share<'a>(fonts: impl IntoIterator<Item = &'a [u8]>) {
    let mut collection = Collection::new(CollectionOptions { shared: false, system_fonts: false });
    for bytes in fonts {
        for (id, faces) in collection.register_fonts(Blob::new(Arc::new(bytes.to_vec())), None) {
            let Some(family) = collection.family_name(id).map(str::to_owned) else { continue };
            // A collection file's later faces would need their own file; its first is the one.
            for face in faces.iter().filter(|f| f.index() == 0) {
                let weight = face.axes().iter().find(|a| a.tag.to_string() == "wght")
                    .map_or_else(|| face.weight().value().to_string(), |a| format!("{} {}", a.min, a.max));
                let style = match face.style() { FontStyle::Normal => "normal", FontStyle::Italic => "italic", FontStyle::Oblique(_) => "oblique" };
                add_font_face(&family, bytes, &weight, style);
            }
        }
    }
}
