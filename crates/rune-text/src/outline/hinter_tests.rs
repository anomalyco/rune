//! The hinting instance a face's glyphs share at one size.

use super::*;

const INTER: &[u8] = include_bytes!("../../../rune/assets/Inter-Regular.ttf");

fn glyph(ch: char) -> u16 {
    use skrifa::MetadataProvider;
    skrifa::FontRef::new(INTER).unwrap().charmap().map(ch).unwrap().to_u32() as u16
}

fn made() -> usize {
    HINTERS_MADE.get()
}

#[test]
fn a_faces_glyphs_at_one_size_share_one_hinting_instance() {
    let before = made();
    for ch in "Lightsof".chars() {
        assert!(skrifa_hinted(INTER, 1, 0, &[], glyph(ch), 13.0, true).is_some(), "{ch}");
    }
    assert_eq!(made() - before, 1, "eight glyphs, one instance");
    // Another size, another font and the other hinting mode each take their own.
    skrifa_hinted(INTER, 1, 0, &[], glyph('L'), 14.0, true).unwrap();
    skrifa_hinted(INTER, 2, 0, &[], glyph('L'), 13.0, true).unwrap();
    skrifa_hinted(INTER, 1, 0, &[], glyph('L'), 13.0, false).unwrap();
    assert_eq!(made() - before, 4);
    skrifa_hinted(INTER, 1, 0, &[], glyph('i'), 13.0, true).unwrap();
    assert_eq!(made() - before, 4, "the first one is still kept");
}

#[test]
fn a_kept_hinting_instance_draws_what_a_new_one_draws() {
    // A new thread has kept nothing: each of its outlines comes from an instance made for it.
    let fresh = |ch: char| std::thread::spawn(move || skrifa_hinted(INTER, 3, 0, &[], glyph(ch), 13.0, true).unwrap()).join().unwrap();
    for ch in ['g', 'L', 'g', 'o', 'L'] {
        let kept = skrifa_hinted(INTER, 3, 0, &[], glyph(ch), 13.0, true).unwrap();
        assert_eq!(format!("{kept:?}"), format!("{:?}", fresh(ch)), "{ch}");
    }
}

#[test]
fn the_stalest_hinting_instance_goes_when_one_more_than_the_thread_keeps_is_made() {
    let before = made();
    for size in 0..HINTERS_KEPT {
        skrifa_hinted(INTER, 4, 0, &[], glyph('a'), 10.0 + size as f32, true).unwrap();
    }
    // The first is used again, so the second is now the stalest.
    skrifa_hinted(INTER, 4, 0, &[], glyph('b'), 10.0, true).unwrap();
    skrifa_hinted(INTER, 4, 0, &[], glyph('a'), 40.0, true).unwrap();
    assert_eq!(made() - before, HINTERS_KEPT + 1);
    skrifa_hinted(INTER, 4, 0, &[], glyph('a'), 10.0, true).unwrap();
    assert_eq!(made() - before, HINTERS_KEPT + 1, "the one used last stayed");
    skrifa_hinted(INTER, 4, 0, &[], glyph('a'), 11.0, true).unwrap();
    assert_eq!(made() - before, HINTERS_KEPT + 2, "the stalest went");
}
