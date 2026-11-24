//! Process-wide runtime font registration via `rune_text::add_font` and `full_faces::arrive`.
//! Lives in its own integration test binary so process-wide registrations never race with
//! parallel unit tests that assert a stable `font_epoch()` across threads.

use std::sync::Arc;

use rune_core::tok::{Family, TextStyle};
use rune_text::{Direction, RichLayoutOptions, TextSystem, add_font, advance, font_epoch, full_faces, ink, measure};

const INTER: &[u8] = include_bytes!("../../rune/assets/Inter-Regular.ttf");
const PLEX: &[u8] = include_bytes!("../../rune/assets/IBMPlexMono-Regular.ttf");
const GEIST: &[u8] = include_bytes!("../../../www/public/fonts/geist.ttf");
const GEIST_MONO: &[u8] = include_bytes!("../../../www/public/fonts/geist-mono.ttf");

#[test]
fn runtime_add_font_propagates_across_text_systems_measure_and_threads() {
    let style = TextStyle::new(14.0, 400.0);
    let plex_named = style.family(Family::Named("IBM Plex Mono"));
    let geist_named = style.family(Family::Named("Geist"));
    let sample = "iiiiiiiiii 0123456789";

    let mut existing = TextSystem::new(&[INTER], ("Inter", "Inter"));
    let before_epoch = font_epoch();
    let before_measure = measure(sample, &[], plex_named, None).0;
    let before_advance = advance(sample, plex_named);
    let before_ink = ink("iii", plex_named);
    assert_eq!(existing.resolved_family(Family::Named("IBM Plex Mono")).as_deref(), Some("Inter"));

    // Invalid and empty inputs are ignored without moving `font_epoch`.
    assert!(add_font(&[][..]).is_empty());
    assert!(add_font(b"not a font".to_vec()).is_empty());
    assert_eq!(font_epoch(), before_epoch);
    assert!(!existing.full_faces_behind());
    assert!(!existing.follow_full_faces());

    // Register static font bytes via `Vec<u8>`.
    let families = add_font(PLEX.to_vec());
    assert_eq!(families, vec!["IBM Plex Mono"]);
    let after_plex_epoch = font_epoch();
    assert_ne!(after_plex_epoch, before_epoch, "font_epoch must advance when a new font is added");

    // Existing TextSystem detects and picks up the arrived face.
    assert!(existing.full_faces_behind());
    assert!(existing.families_behind());
    assert!(existing.follow_full_faces());
    assert!(!existing.full_faces_behind());
    assert_eq!(existing.resolved_family(Family::Named("IBM Plex Mono")).as_deref(), Some("IBM Plex Mono"));

    // `measure`, `advance`, and `ink` follow immediately without rebuilding a TextSystem manually.
    let after_measure = measure(sample, &[], plex_named, None).0;
    let after_advance = advance(sample, plex_named);
    let after_ink = ink("iii", plex_named);
    assert!((after_measure - before_measure).abs() > 10.0, "measure updated: {before_measure} -> {after_measure}");
    assert!((after_advance - before_advance).abs() > 10.0, "advance updated: {before_advance} -> {after_advance}");
    assert_ne!(after_ink, before_ink, "ink cache invalidated and rasterized the new face");

    // Duplicate registration returns family names, does not advance font_epoch or mark behind.
    let dup = add_font(Arc::<[u8]>::from(PLEX));
    assert_eq!(dup, vec!["IBM Plex Mono"]);
    assert_eq!(font_epoch(), after_plex_epoch, "duplicate bytes must not advance font_epoch");
    assert!(!existing.full_faces_behind());
    assert!(!existing.follow_full_faces());

    // A fresh TextSystem built after `add_font` already has the registered font.
    let fresh = TextSystem::new(&[INTER], ("IBM Plex Mono", "IBM Plex Mono"));
    assert_eq!(fresh.resolved_family(Family::Sans).as_deref(), Some("IBM Plex Mono"));
    assert!(!fresh.full_faces_behind());

    // A worker thread sees the updated `font_epoch` and measures with the runtime-added font.
    let (worker_w, worker_epoch) = std::thread::spawn(move || {
        (measure(sample, &[], plex_named, None).0, font_epoch())
    })
    .join()
    .unwrap();
    assert_eq!(worker_w, after_measure);
    assert_ne!(worker_epoch, before_epoch);

    // Register a variable font via `&[u8]`.
    let before_geist = measure(sample, &[], geist_named, None).0;
    let geist_families = add_font(GEIST);
    assert_eq!(geist_families, vec!["Geist"]);
    let after_geist_epoch = font_epoch();
    assert_ne!(after_geist_epoch, after_plex_epoch);
    let after_geist = measure(sample, &[], geist_named, None).0;
    assert!((after_geist - before_geist).abs() > 0.5, "Geist measured differently from Inter: {before_geist} -> {after_geist}");

    // RTL text shapes cleanly with a runtime-added font.
    assert!(existing.follow_full_faces());
    let rtl = existing.layout_rich_with(
        "שלום world 42",
        &[],
        geist_named,
        1.0,
        Some(200.0),
        RichLayoutOptions::new().direction(Direction::Rtl),
    );
    assert!(rtl.width() > 0.0);

    // Batch `arrive` adds new faces atomically and skips already-registered ones.
    let batch = full_faces::arrive(vec![PLEX.to_vec(), GEIST_MONO.to_vec(), b"bad".to_vec()]);
    assert_eq!(batch, vec!["IBM Plex Mono", "Geist Mono"]);
    assert_ne!(font_epoch(), after_geist_epoch);
    assert!(existing.follow_full_faces());
    assert_eq!(existing.resolved_family(Family::Named("Geist Mono")).as_deref(), Some("Geist Mono"));
}
