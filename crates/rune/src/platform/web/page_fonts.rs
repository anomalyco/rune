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

/// [`share`] one font file at a time in the page's idle time (1 s at the latest each), so the
/// frames and reads that follow the first one aren't held behind it: shared at once, a page's
/// fonts held its first reads' answers back 34 ms.
pub(super) fn share_later(fonts: &'static [&'static [u8]]) {
    if fonts.is_empty() {
        return;
    }
    wasm_bindgen_futures::spawn_local(async move {
        for &font in fonts {
            let _ = wasm_bindgen_futures::JsFuture::from(super::platform::next_idle(1000)).await;
            share([font]);
        }
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
                let declared = crate::text::face_weights::declared(&family, face.weight().value()).map(|r| format!("{} {}", r.0, r.1));
                let weight = declared.or_else(|| face.axes().iter().find(|a| a.tag.to_string() == "wght").map(|a| format!("{} {}", a.min, a.max)))
                    .unwrap_or_else(|| face.weight().value().to_string());
                let style = match face.style() { FontStyle::Normal => "normal", FontStyle::Italic => "italic", FontStyle::Oblique(_) => "oblique" };
                add_font_face(&family, bytes, &weight, style);
            }
        }
    }
}
