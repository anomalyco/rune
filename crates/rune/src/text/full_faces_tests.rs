use std::sync::Arc;

use parley::fontique::Blob;

use super::super::*;

const PLEX: &[u8] = include_bytes!("../../assets/IBMPlexMono-Regular.ttf");
const FULL: [&[u8]; 3] = [
    include_bytes!("../../assets/web/Inter-Regular.ttf"),
    include_bytes!("../../assets/web/Inter-Medium.ttf"),
    include_bytes!("../../assets/web/Inter-SemiBold.ttf"),
];
const LATIN: [&[u8]; 3] = [
    include_bytes!("../../assets/web/latin/Inter-Regular.ttf"),
    include_bytes!("../../assets/web/latin/Inter-Medium.ttf"),
    include_bytes!("../../assets/web/latin/Inter-SemiBold.ttf"),
];

/// Each glyph's pen position and advance, and whether it's the missing-glyph box.
fn shaped(ts: &mut TextSystem, texts: &[&str]) -> Vec<(u32, u32, bool)> {
    let mut out = Vec::new();
    for text in texts {
        for style in [TextStyle::new(13.0, 400.0), TextStyle::new(15.0, 500.0), TextStyle::new(24.0, 600.0)] {
            let layout = ts.layout_rich(text, &[], style, 2.0, None);
            for line in layout.lines() {
                for item in line.items() {
                    if let PositionedLayoutItem::GlyphRun(run) = item {
                        out.extend(run.glyphs().map(|g| (g.x.to_bits(), g.advance.to_bits(), g.id == 0)));
                    }
                }
            }
        }
    }
    out
}

const LATIN_TEXTS: [&str; 3] = [
    "Rune → office “quotes” fi ffl AVAWAY 1/2 ½ €100 ™ — …",
    "Ýüñçø Łódź Čeština Ştefan Øresund Œuvre ß",
    "⌘⇧⌥⌃ ⌫ ⎋ ↩ ⏎ «guillemets» ‹›",
];

/// The cut faces set Latin text exactly as the full ones do: same glyphs, kerning and
/// ligatures, at the same positions, none of them missing.
#[test]
fn the_latin_cut_sets_latin_text_like_the_full_faces() {
    let mut cut = TextSystem::new(&[LATIN[0], LATIN[1], LATIN[2], PLEX], ("Inter", "IBM Plex Mono"));
    let mut full = TextSystem::new(&[FULL[0], FULL[1], FULL[2], PLEX], ("Inter", "IBM Plex Mono"));
    let (a, b) = (shaped(&mut cut, &LATIN_TEXTS), shaped(&mut full, &LATIN_TEXTS));
    assert!(a.len() > 200);
    assert!(a.iter().all(|g| !g.2), "a Latin glyph is missing from the cut");
    assert_eq!(a, b);
}

/// Swapping the full faces in sets text the cut lacked as the full faces do, and leaves Latin
/// text where it was.
#[test]
fn swapping_in_the_full_faces_sets_what_the_cut_lacked() {
    let texts = ["Ωμέγα Привет café"];
    let mut full = TextSystem::new(&[FULL[0], FULL[1], FULL[2], PLEX], ("Inter", "IBM Plex Mono"));
    let want = shaped(&mut full, &texts);
    let mut ts = TextSystem::new(&[LATIN[0], LATIN[1], LATIN[2], PLEX], ("Inter", "IBM Plex Mono"));
    assert_ne!(shaped(&mut ts, &texts), want, "the cut has Greek and Cyrillic");
    let latin = shaped(&mut ts, &LATIN_TEXTS);
    ts.swap_faces(&FULL.map(|f| Blob::new(Arc::new(f))));
    ts.drop_shaped();
    assert_eq!(shaped(&mut ts, &texts), want);
    assert_eq!(shaped(&mut ts, &LATIN_TEXTS), latin);
}
