use std::sync::Arc;

use parley::PositionedLayoutItem;
use parley::fontique::Blob;
use rune_core::tok::Family;

use super::{MAX_FONT_BYTES, Registry};
use crate::*;

const PLEX: &[u8] = include_bytes!("../../../rune/assets/IBMPlexMono-Regular.ttf");
const GEIST: &[u8] = include_bytes!("../../../../www/public/fonts/geist.ttf");
const FULL: [&[u8]; 3] = [
    include_bytes!("../../../rune/assets/web/Inter-Regular.ttf"),
    include_bytes!("../../../rune/assets/web/Inter-Medium.ttf"),
    include_bytes!("../../../rune/assets/web/Inter-SemiBold.ttf"),
];
const FIRST: [&[u8]; 3] = [
    include_bytes!("../../../rune/assets/web/first/Inter-Regular.ttf"),
    include_bytes!("../../../rune/assets/web/first/Inter-Medium.ttf"),
    include_bytes!("../../../rune/assets/web/first/Inter-SemiBold.ttf"),
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

const FIRST_TEXTS: [&str; 5] = [
    "Rune → office “quotes” fi ffl AVAWAY 1/2 ½ €100 ™ — …",
    "Ýüñçø Łódź Čeština Ştefan Øresund Œuvre ß",
    "⌘⇧⌥⌃ ⌫ ⎋ ↩ ⏎ «guillemets» ‹›",
    "Ωμέγα Ελληνικά ΐΰ ϊϋ ς; Άλφα",
    "Привет, мир! Ґанок ЇЄІ Ђорђе Љубљана ЎЏ",
];

/// The cut faces set Latin, Greek and Cyrillic text exactly as the full ones do: same glyphs,
/// kerning and ligatures, at the same positions, none of them missing.
#[test]
fn the_first_cut_sets_latin_greek_and_cyrillic_like_the_full_faces() {
    let mut cut = TextSystem::new(&[FIRST[0], FIRST[1], FIRST[2], PLEX], ("Inter", "IBM Plex Mono"));
    let mut full = TextSystem::new(&[FULL[0], FULL[1], FULL[2], PLEX], ("Inter", "IBM Plex Mono"));
    let (a, b) = (shaped(&mut cut, &FIRST_TEXTS), shaped(&mut full, &FIRST_TEXTS));
    assert!(a.len() > 400);
    assert!(a.iter().all(|g| !g.2), "a glyph is missing from the cut");
    assert_eq!(a, b);
}

/// Swapping the full faces in sets text the cut lacked as the full faces do, and leaves the
/// text the cut had where it was.
#[test]
fn swapping_in_the_full_faces_sets_what_the_cut_lacked() {
    let texts = ["Tiếng Việt ἀρχή Әліппе"];
    let mut full = TextSystem::new(&[FULL[0], FULL[1], FULL[2], PLEX], ("Inter", "IBM Plex Mono"));
    let want = shaped(&mut full, &texts);
    let mut ts = TextSystem::new(&[FIRST[0], FIRST[1], FIRST[2], PLEX], ("Inter", "IBM Plex Mono"));
    assert_ne!(shaped(&mut ts, &texts), want, "the cut has Vietnamese, polytonic Greek or Kazakh");
    let first = shaped(&mut ts, &FIRST_TEXTS);
    ts.swap_faces(&FULL.map(|f| Blob::new(Arc::new(f))));
    ts.drop_shaped();
    assert_eq!(shaped(&mut ts, &texts), want);
    assert_eq!(shaped(&mut ts, &FIRST_TEXTS), first);
}

#[test]
fn registry_rejects_empty_oversized_and_invalid_bytes() {
    assert_eq!(MAX_FONT_BYTES, 64 * 1024 * 1024);
    let mut reg = Registry::default();

    assert_eq!(reg.add_with_limit(Arc::from(&[][..]), MAX_FONT_BYTES), (Vec::new(), false));
    assert_eq!(reg.add_with_limit(Arc::from(&b"not a valid font file"[..]), MAX_FONT_BYTES), (Vec::new(), false));
    // Rejects a valid font that exceeds the byte limit by 1 byte, and accepts at the exact limit.
    assert_eq!(reg.add_with_limit(Arc::from(PLEX), PLEX.len() - 1), (Vec::new(), false));
    assert_eq!(reg.faces.len(), 0);

    let (families, added) = reg.add_with_limit(Arc::from(PLEX), PLEX.len());
    assert_eq!(families, vec!["IBM Plex Mono"]);
    assert!(added);
    assert_eq!(reg.faces.len(), 1);
}

#[test]
fn registry_registers_static_and_variable_fonts_and_deduplicates_by_content() {
    let mut reg = Registry::default();
    let (first, added_first) = reg.add_with_limit(Arc::from(PLEX.to_vec()), MAX_FONT_BYTES);
    assert_eq!(first, vec!["IBM Plex Mono"]);
    assert!(added_first);

    // Identical bytes in a distinct allocation return the same family names without duplicating.
    let (dup, added_dup) = reg.add_with_limit(Arc::from(PLEX.to_vec()), MAX_FONT_BYTES);
    assert_eq!(dup, first);
    assert!(!added_dup);
    assert_eq!(reg.faces.len(), 1);

    // Variable font registers cleanly alongside static faces.
    let (var_families, added_var) = reg.add_with_limit(Arc::from(GEIST), MAX_FONT_BYTES);
    assert_eq!(var_families, vec!["Geist"]);
    assert!(added_var);
    assert_eq!(reg.faces.len(), 2);
}

#[test]
fn registry_batch_deduplicates_and_skips_invalid_faces() {
    let mut reg = Registry::default();
    let (names, added) = reg.add_batch_with_limit(
        vec![Vec::new(), b"bad".to_vec(), PLEX.to_vec(), PLEX.to_vec(), GEIST.to_vec()],
        MAX_FONT_BYTES,
    );
    assert_eq!(names, vec!["IBM Plex Mono", "Geist"]);
    assert_eq!(added, 2);
    assert_eq!(reg.faces.len(), 2);

    let (again, added_again) = reg.add_batch_with_limit(vec![PLEX.to_vec(), GEIST.to_vec()], MAX_FONT_BYTES);
    assert_eq!(again, vec!["IBM Plex Mono", "Geist"]);
    assert_eq!(added_again, 0);
    assert_eq!(reg.faces.len(), 2);
}

#[test]
fn swap_faces_resolves_named_family_and_keeps_rtl_text_shaping() {
    let mut ts = TextSystem::new(&[FULL[0]], ("Inter", "Inter"));
    let style = TextStyle::new(14.0, 400.0).family(Family::Named("IBM Plex Mono"));
    assert_eq!(ts.resolved_family(Family::Named("IBM Plex Mono")).as_deref(), Some("Inter"));
    let before_w = ts.layout_rich("iiiiiiiiii", &[], style, 1.0, None).full_width();

    ts.swap_faces(&[Blob::new(Arc::new(PLEX))]);
    ts.drop_shaped();
    assert_eq!(ts.resolved_family(Family::Named("IBM Plex Mono")).as_deref(), Some("IBM Plex Mono"));
    let after_w = ts.layout_rich("iiiiiiiiii", &[], style, 1.0, None).full_width();
    assert!((after_w - before_w).abs() > 10.0, "expected IBM Plex Mono width change: {before_w} -> {after_w}");

    let rtl_layout = ts.layout_rich_with(
        "مرحبا hello 123",
        &[],
        style,
        1.0,
        Some(180.0),
        RichLayoutOptions::new().direction(Direction::Rtl),
    );
    assert!(rtl_layout.width() > 0.0);
}
