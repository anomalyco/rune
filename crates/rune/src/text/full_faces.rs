//! Faces a web app adds after its first frame: the full Inter faces, and the fonts it names with
//! `App::font_url_later`.
//!
//! A web app without Inter of its own embeds Inter cut to Latin, Greek and Cyrillic text
//! (`scripts/web-fonts.sh`) and fetches the full faces after its first frame. The cut faces keep
//! each of their glyphs' outline, advance, kerning and layout features, so text they cover stays
//! where it is and only text they lacked moves. When faces arrive, every text system registers
//! them in place of any with the same family, width, style and weight, and drops what it shaped.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use parley::fontique::Blob;

use super::TextSystem;

static ARRIVED: Mutex<Vec<Blob<u8>>> = Mutex::new(Vec::new());
/// `ARRIVED`'s length, read without the lock on every frame.
static COUNT: AtomicUsize = AtomicUsize::new(0);
/// A text system registered the embedded Inter, so the full faces are worth fetching.
#[cfg(all(target_arch = "wasm32", feature = "embedded-fonts"))]
static EMBEDDED_INTER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The full faces' files, which a web build serves beside its `app.js`.
#[cfg(all(target_arch = "wasm32", feature = "embedded-fonts"))]
pub(crate) const FILES: &[&str] = &["Inter-Regular.ttf", "Inter-Medium.ttf", "Inter-SemiBold.ttf"];

/// Notes that a text system set text in the embedded Inter: no font of the app's is Inter.
#[cfg(all(target_arch = "wasm32", feature = "embedded-fonts"))]
pub(super) fn note_embedded_inter() {
    EMBEDDED_INTER.store(true, Ordering::Relaxed);
}

/// Whether the full Inter faces would replace any: an app that brings its own Inter has none of
/// the cut ones registered.
#[cfg(all(target_arch = "wasm32", feature = "embedded-fonts"))]
pub(crate) fn embedded_inter() -> bool {
    EMBEDDED_INTER.load(Ordering::Relaxed)
}

/// Hands over faces: measurement uses them from now on, and every text system from its next
/// frame.
#[cfg(target_arch = "wasm32")]
pub(crate) fn arrive(faces: Vec<Vec<u8>>) {
    let Ok(mut arrived) = ARRIVED.lock() else { return };
    arrived.extend(faces.into_iter().map(|f| Blob::new(std::sync::Arc::new(f))));
    COUNT.store(arrived.len(), Ordering::Release);
    drop(arrived);
    super::measure::note_faces_arrived();
}

impl TextSystem {
    /// Takes up the faces that arrived since it last looked; true when any did.
    pub(crate) fn follow_full_faces(&mut self) -> bool {
        if COUNT.load(Ordering::Acquire) <= self.faces_taken {
            return false;
        }
        let Some(faces) = ARRIVED.lock().ok().map(|a| a[self.faces_taken..].to_vec()) else { return false };
        self.faces_taken += faces.len();
        self.swap_faces(&faces);
        self.drop_shaped();
        true
    }

    /// Registers each face in place of those with its family, width, style and weight.
    pub(super) fn swap_faces(&mut self, faces: &[Blob<u8>]) {
        let collection = &mut self.font_cx.collection;
        for blob in faces {
            for (family, fonts) in collection.register_fonts(blob.clone(), None) {
                for font in fonts {
                    collection.unregister_font(family, font.width(), font.style(), font.weight());
                }
            }
            collection.register_fonts(blob.clone(), None);
        }
    }
}

#[cfg(test)]
#[path = "full_faces_tests.rs"]
mod tests;
