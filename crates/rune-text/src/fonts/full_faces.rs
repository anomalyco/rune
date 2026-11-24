//! Faces added to the process-wide font collection at runtime: via [`add_font`] on any
//! platform, and on web for the full Inter faces and `App::font_url_later`.
//!
//! A web app without Inter of its own embeds Inter cut to Latin, Greek and Cyrillic text
//! (`scripts/web-fonts.sh`) and fetches the full faces after its first frame. The cut faces keep
//! each of their glyphs' outline, advance, kerning and layout features, so text they cover stays
//! where it is and only text they lacked moves. When faces arrive, every text system registers
//! them in place of any with the same family, width, style and weight, and drops what it shaped.

use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use parley::fontique::{Blob, Collection, CollectionOptions};

use crate::TextSystem;

/// Maximum byte length accepted for a single font blob registered at runtime (64 MiB).
pub(crate) const MAX_FONT_BYTES: usize = 64 * 1024 * 1024;

struct RegisteredFace {
    hash: u64,
    blob: Blob<u8>,
    families: Vec<String>,
}

#[derive(Default)]
struct Registry {
    faces: Vec<RegisteredFace>,
}

impl Registry {
    fn find(&self, hash: u64, bytes: &[u8]) -> Option<&[String]> {
        self.faces
            .iter()
            .find(|face| face.hash == hash && face.blob.data() == bytes)
            .map(|face| face.families.as_slice())
    }

    /// Inserts a pre-validated face if not already present. Returns its family names and whether
    /// it was newly added.
    fn insert(&mut self, hash: u64, blob: Blob<u8>, families: Vec<String>) -> (Vec<String>, bool) {
        if let Some(existing) = self.find(hash, blob.data()) {
            return (existing.to_vec(), false);
        }
        self.faces.push(RegisteredFace { hash, blob, families: families.clone() });
        (families, true)
    }

    #[cfg(test)]
    fn add_with_limit(&mut self, bytes: Arc<[u8]>, max_bytes: usize) -> (Vec<String>, bool) {
        if bytes.is_empty() || bytes.len() > max_bytes {
            return (Vec::new(), false);
        }
        let hash = content_hash(&bytes);
        if let Some(existing) = self.find(hash, &bytes) {
            return (existing.to_vec(), false);
        }
        let blob = Blob::new(Arc::new(bytes));
        let families = extract_families(&blob);
        if families.is_empty() {
            return (Vec::new(), false);
        }
        self.insert(hash, blob, families)
    }

    #[cfg(test)]
    fn add_batch_with_limit(&mut self, faces: Vec<Vec<u8>>, max_bytes: usize) -> (Vec<String>, usize) {
        let mut names = Vec::new();
        let mut added = 0;
        for raw in faces {
            let (registered, is_new) = self.add_with_limit(Arc::from(raw), max_bytes);
            if is_new {
                added += 1;
            }
            for name in registered {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        (names, added)
    }
}

static ARRIVED: Mutex<Registry> = Mutex::new(Registry { faces: Vec::new() });
/// `ARRIVED`'s length, read without the lock on every frame and by [`crate::font_epoch`].
static COUNT: AtomicUsize = AtomicUsize::new(0);
/// A text system registered the embedded Inter, so the full faces are worth fetching.
#[cfg(all(target_arch = "wasm32", feature = "embedded-fonts"))]
static EMBEDDED_INTER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The full faces' files, which a web build serves beside its `app.js`.
#[cfg(all(target_arch = "wasm32", feature = "embedded-fonts"))]
#[doc(hidden)]
pub const FILES: &[&str] = &["Inter-Regular.ttf", "Inter-Medium.ttf", "Inter-SemiBold.ttf"];

/// Notes that a text system set text in the embedded Inter: no font of the app's is Inter.
#[cfg(all(target_arch = "wasm32", feature = "embedded-fonts"))]
pub(crate) fn note_embedded_inter() {
    EMBEDDED_INTER.store(true, Ordering::Relaxed);
}

/// Whether the full Inter faces would replace any: an app that brings its own Inter has none of
/// the cut ones registered.
#[cfg(all(target_arch = "wasm32", feature = "embedded-fonts"))]
#[doc(hidden)]
pub fn embedded_inter() -> bool {
    EMBEDDED_INTER.load(Ordering::Relaxed)
}

fn content_hash(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    hasher.write(bytes);
    hasher.finish()
}

fn extract_families(blob: &Blob<u8>) -> Vec<String> {
    let mut collection = Collection::new(CollectionOptions { shared: false, system_fonts: false });
    let registered = collection.register_fonts(blob.clone(), None);
    let mut families = Vec::new();
    for (id, _) in registered {
        if let Some(name) = collection.family_name(id)
            && !families.iter().any(|existing: &String| existing == name)
        {
            families.push(name.to_owned());
        }
    }
    families
}

/// Number of font blobs registered at runtime so far, read lock-free.
pub(crate) fn count() -> usize {
    COUNT.load(Ordering::Acquire)
}

/// Registers font bytes into the process-wide font collection at runtime, returning the
/// registered family names in registration order.
///
/// Accepts raw font file bytes (TTF, OTF, TTC, or variable fonts) as `Vec<u8>`, `&[u8]`,
/// `Box<[u8]>`, or `Arc<[u8]>`. Read font files off the UI thread and pass their bytes here;
/// this function performs no file I/O.
///
/// Every existing and future [`TextSystem`] picks up the added faces on its next sync (and
/// [`crate::measure`], [`crate::advance`], [`crate::ink`], and [`crate::font_epoch`] follow
/// immediately). Select a returned family with `Tx::set_font_families`,
/// `Runtime::set_font_families`, or `Family::Named`.
///
/// Empty slices, blobs larger than 64 MiB, and unrecognised font data are ignored and return an
/// empty list. Registering bytes that are already registered returns their family names without
/// duplicating data or advancing [`crate::font_epoch`].
///
/// ```no_run
/// let bytes: Vec<u8> = std::fs::read("path/to/MyFont.ttf").unwrap();
/// let families = rune_text::add_font(bytes);
/// assert!(!families.is_empty());
/// ```
pub fn add_font(bytes: impl Into<Arc<[u8]>>) -> Vec<String> {
    add_font_checked(bytes.into()).0
}

/// Like [`add_font`], also returning whether the blob was newly registered (`false` when
/// rejected or already present).
#[doc(hidden)]
pub fn add_font_checked(bytes: Arc<[u8]>) -> (Vec<String>, bool) {
    if bytes.is_empty() || bytes.len() > MAX_FONT_BYTES {
        return (Vec::new(), false);
    }
    let hash = content_hash(&bytes);
    if let Ok(arrived) = ARRIVED.lock()
        && let Some(existing) = arrived.find(hash, &bytes)
    {
        return (existing.to_vec(), false);
    }
    let blob = Blob::new(Arc::new(bytes));
    let families = extract_families(&blob);
    if families.is_empty() {
        return (Vec::new(), false);
    }
    let Ok(mut arrived) = ARRIVED.lock() else { return (Vec::new(), false) };
    let (families, added) = arrived.insert(hash, blob, families);
    if added {
        COUNT.store(arrived.faces.len(), Ordering::Release);
    }
    (families, added)
}

/// Hands over a batch of faces atomically: measurement uses them from now on, and every text
/// system from its next frame. Returns the deduplicated family names registered across the batch.
#[doc(hidden)]
pub fn arrive(faces: Vec<Vec<u8>>) -> Vec<String> {
    let mut prepared: Vec<(u64, Blob<u8>, Vec<String>)> = Vec::with_capacity(faces.len());
    let mut names: Vec<String> = Vec::new();
    for raw in faces {
        if raw.is_empty() || raw.len() > MAX_FONT_BYTES {
            continue;
        }
        let hash = content_hash(&raw);
        if let Some((_, _, existing)) = prepared.iter().find(|(h, b, _)| *h == hash && b.data() == raw.as_slice()) {
            for name in existing {
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }
            continue;
        }
        if let Ok(arrived) = ARRIVED.lock()
            && let Some(existing) = arrived.find(hash, &raw)
        {
            for name in existing {
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }
            continue;
        }
        let blob = Blob::new(Arc::new(Arc::<[u8]>::from(raw)));
        let families = extract_families(&blob);
        if families.is_empty() {
            continue;
        }
        for name in &families {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        prepared.push((hash, blob, families));
    }
    if prepared.is_empty() {
        return names;
    }
    let Ok(mut arrived) = ARRIVED.lock() else { return Vec::new() };
    let mut any_added = false;
    for (hash, blob, families) in prepared {
        let (_, added) = arrived.insert(hash, blob, families);
        any_added |= added;
    }
    if any_added {
        COUNT.store(arrived.faces.len(), Ordering::Release);
    }
    names
}

impl TextSystem {
    /// Whether runtime-added faces arrived since this text system last followed them.
    #[doc(hidden)]
    pub fn full_faces_behind(&self) -> bool {
        COUNT.load(Ordering::Acquire) > self.faces_taken
    }

    /// Takes up the faces that arrived since it last looked; true when any did.
    #[doc(hidden)]
    pub fn follow_full_faces(&mut self) -> bool {
        if COUNT.load(Ordering::Acquire) <= self.faces_taken {
            return false;
        }
        let Some(faces) = ARRIVED
            .lock()
            .ok()
            .map(|a| a.faces[self.faces_taken..].iter().map(|f| f.blob.clone()).collect::<Vec<_>>())
        else {
            return false;
        };
        if faces.is_empty() {
            return false;
        }
        self.faces_taken += faces.len();
        self.swap_faces(&faces);
        self.sync_cjk_fallbacks();
        self.drop_shaped();
        true
    }

    /// Registers each face in place of those with its family, width, style and weight.
    pub(super) fn swap_faces(&mut self, faces: &[Blob<u8>]) {
        self.fallbacks.clear_color_family_cache();
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
