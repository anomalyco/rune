//! The weights each of an app's own faces answers, as a stylesheet's `@font-face` declares them
//! with a `font-weight` range. Without a declaration a face answers its own weight, and CSS's
//! matching then sends a weight above 500 to the next heavier face: with IBM Plex Mono's Text
//! (450) and Bold (700) cuts, inline code at 530 drew in Bold, where OpenCode's Desktop, which
//! declares Text for 100-599 and Bold for 600-900, draws it in Text.

use std::sync::{Arc, Mutex};

use parley::fontique::{Blob, Collection, FamilyId, FontInfoOverride, FontWeight};

/// One face's declared weights: the face of `family` whose own weight (its `OS/2` weight class)
/// is `weight` answers every weight in `range`, of either style.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceWeights {
    pub family: &'static str,
    pub weight: f32,
    pub range: (f32, f32),
}

static DECLARED: Mutex<&'static [FaceWeights]> = Mutex::new(&[]);

/// Declares the weights the app's faces answer, as its `@font-face` rules would. Call it before
/// the first [`super::TextSystem`] is made: faces are registered then.
pub fn set_face_weights(faces: &'static [FaceWeights]) {
    if let Ok(mut d) = DECLARED.lock() {
        *d = faces;
    }
}

/// The range declared for the face of `family` with its own `weight` (a browser's page fonts
/// declare it too).
#[cfg(target_arch = "wasm32")]
pub(crate) fn declared(family: &str, weight: f32) -> Option<(f32, f32)> {
    range_in(*DECLARED.lock().ok()?, family, weight)
}

fn range_in(faces: &[FaceWeights], family: &str, weight: f32) -> Option<(f32, f32)> {
    faces.iter().find(|f| f.family.eq_ignore_ascii_case(family) && f.weight == weight).map(|f| f.range)
}

/// Registers the font file `data`, a declared face at the top of its range. Parley's matching
/// knows a face by one weight; at the tops of their ranges, CSS's rules pick each face for every
/// weight its range holds when the ranges meet end to end and each ends above 500, as Desktop's
/// Plex Mono's (599, 900) do: above 500 they take the lightest face at or above the weight asked
/// for, and at or below 500, with no face there, the lightest. Synthetic bold follows the range
/// too: it's added only for a weight more than 200 above the face's top.
pub(super) fn register(collection: &mut Collection, data: &'static [u8]) -> Vec<FamilyId> {
    let faces = DECLARED.lock().map_or(&[][..], |d| *d);
    register_with(collection, data, faces)
}

fn register_with(collection: &mut Collection, data: &'static [u8], faces: &[FaceWeights]) -> Vec<FamilyId> {
    let blob = Blob::new(Arc::new(data));
    let added = collection.register_fonts(blob.clone(), None);
    // An override applies to every face of the file, so a collection file stays as it is.
    if let [(id, fonts)] = added.as_slice()
        && let [font] = fonts.as_slice()
        && let Some(range) = collection.family_name(*id).and_then(|name| range_in(faces, name, font.weight().value()))
    {
        collection.unregister_font(*id, font.width(), font.style(), font.weight());
        let weight = FontInfoOverride { weight: Some(FontWeight::new(range.1)), ..Default::default() };
        collection.register_fonts(blob, Some(weight));
    }
    added.into_iter().map(|(id, _)| id).collect()
}

