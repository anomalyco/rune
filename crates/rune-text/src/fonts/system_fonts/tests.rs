use std::path::PathBuf;

use super::*;

fn bundled_font(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../rune/assets").join(name)
}

/// A font file already loaded through `load_files` for another purpose still registers its
/// cached `FamilyId`s as CJK fallbacks without reloading the file, while excluding primary
/// families and deduplicating repeated calls.
#[test]
fn register_system_cjk_registers_already_loaded_files() {
    let mut collection = Collection::new(CollectionOptions { shared: false, system_fonts: false });
    let mut loaded = LoadedFiles::new();
    let mono_path = bundled_font("IBMPlexMono-Regular.ttf");
    let primary_path = bundled_font("Inter-Regular.ttf");

    let primary_ids = load_files(&mut collection, &mut loaded, [primary_path.clone()]);
    let mono_ids = load_files(&mut collection, &mut loaded, [mono_path.clone()]);
    assert_eq!(primary_ids.len(), 1);
    assert_eq!(mono_ids.len(), 1);
    let mono_id = mono_ids[0];
    let faces_before = collection.family(mono_id).expect("loaded family").fonts().len();

    let mut all = Vec::new();
    register_system_cjk(
        &mut collection,
        &mut loaded,
        vec![primary_path.clone(), mono_path.clone()],
        ("Inter", "OtherMono"),
        &mut all,
    );

    assert_eq!(all, vec![mono_id], "cached non-primary family should be added to CJK fallbacks");
    assert_eq!(collection.family(mono_id).expect("loaded family").fonts().len(), faces_before, "already-loaded file must not be reloaded");
    for script in [b"Hani", b"Hira", b"Kana", b"Hang", b"Bopo"] {
        let registered: Vec<FamilyId> = collection.fallback_families(FallbackKey::new(Script::from_bytes(*script), None)).collect();
        assert_eq!(registered, vec![mono_id], "script {} should register cached family", std::str::from_utf8(script).unwrap());
    }

    // Calling again with the same already-loaded files keeps `all` deduplicated.
    register_system_cjk(&mut collection, &mut loaded, vec![primary_path, mono_path], ("Inter", "OtherMono"), &mut all);
    assert_eq!(all, vec![mono_id]);
}

/// After `apply_families` switches to missing families on a host with no system fonts, both
/// slots still fall back to the bundled app fonts and measure and draw just like a fresh launch.
#[test]
fn apply_families_missing_family_falls_back_to_bundled_fonts_without_system_fonts() {
    const INTER: &[u8] = include_bytes!("../../../../rune/assets/Inter-Regular.ttf");
    const PLEX: &[u8] = include_bytes!("../../../../rune/assets/IBMPlexMono-Regular.ttf");
    let sans = TextStyle::new(14.0, 400.0);
    let mono = sans.mono();

    let measure_and_draw = |ts: &mut TextSystem, text: &str, style: TextStyle| -> (f32, usize) {
        let layout = ts.layout_rich(text, &[], style, 2.0, None);
        let mut quads = Vec::new();
        ts.glyphs_for_layout(&layout, 2.0, [0.0, 0.0], false, &mut quads);
        (layout.full_width(), quads.len())
    };

    // Strip any host system fonts loaded into `symbol_families` so this test exercises the
    // fontless-host path deterministically on Windows, macOS and Linux alike.
    let mut fresh = TextSystem::new_local(&[INTER, PLEX], ("Rune Missing Family", "Rune Missing Mono"));
    fresh.symbol_families.clear();

    let mut switched = TextSystem::new_local(&[INTER, PLEX], ("Inter", "IBM Plex Mono"));
    let (launch_sans_w, _) = measure_and_draw(&mut switched, "Hamburgefonstiv 0123", sans);
    let (launch_mono_w, _) = measure_and_draw(&mut switched, "let mono = 42;", mono);
    assert!(launch_sans_w > 0.0 && launch_mono_w > 0.0);

    switched.apply_families("Rune Missing Family", "Rune Missing Mono");
    switched.symbol_families.clear();

    for (sample, style) in [("Hamburgefonstiv 0123", sans), ("let mono = 42;", mono)] {
        let (fw, fq) = measure_and_draw(&mut fresh, sample, style);
        let (sw, sq) = measure_and_draw(&mut switched, sample, style);
        assert!(fw > 0.0 && fq > 0, "fresh launch must measure and draw {sample:?}");
        assert_eq!(sw, fw, "after apply_families, {sample:?} must match fresh launch width");
        assert_eq!(sq, fq, "after apply_families, {sample:?} must draw the same glyphs as launch");
    }

    // Switching back restores the original families and widths.
    switched.apply_families("Inter", "IBM Plex Mono");
    assert_eq!(measure_and_draw(&mut switched, "Hamburgefonstiv 0123", sans).0, launch_sans_w);
    assert_eq!(measure_and_draw(&mut switched, "let mono = 42;", mono).0, launch_mono_w);

    // A single missing slot when only one bundled font exists (the default Inter) also falls
    // back to that bundled font on a host with no system fonts, both at launch and after a switch.
    let mut fresh_one = TextSystem::new_local(&[], ("Inter", "Rune Missing Mono"));
    fresh_one.symbol_families.clear();
    let mut switched_one = TextSystem::new_local(&[], ("Inter", "ui-monospace"));
    assert!(switched_one.system_mono.is_none(), "launch must not load system monospace fonts");
    assert!(switched_one.font_files.is_empty(), "launch must not open system font files");
    switched_one.apply_families("Inter", "Rune Missing Mono");
    switched_one.symbol_families.clear();
    let (fw_one, fq_one) = measure_and_draw(&mut fresh_one, "let mono = 42;", mono);
    let (sw_one, sq_one) = measure_and_draw(&mut switched_one, "let mono = 42;", mono);
    assert!(fw_one > 0.0 && fq_one > 0, "fresh single-missing-slot launch must measure and draw");
    assert_eq!(sw_one, fw_one, "switched single-missing-slot must match fresh launch width");
    assert_eq!(sq_one, fq_one, "switched single-missing-slot must draw the same glyphs");
}

#[test]
fn finalize_sorts_deduplicates_and_excludes_hidden_families() {
    use super::installed::{RawFamily, finalize};

    let families = finalize(vec![
        RawFamily { name: ".SF NS".into(), monospace: false, weights: vec![400] },
        RawFamily { name: ".LastResort".into(), monospace: false, weights: vec![400] },
        RawFamily { name: "   ".into(), monospace: false, weights: vec![400] },
        RawFamily { name: "".into(), monospace: false, weights: vec![400] },
        RawFamily { name: "Menlo".into(), monospace: false, weights: vec![700] },
        RawFamily { name: "menlo".into(), monospace: true, weights: vec![400, 700] },
        RawFamily { name: "iA Writer Mono".into(), monospace: true, weights: vec![50, 400, 950] },
        RawFamily { name: "Inter".into(), monospace: false, weights: vec![] },
        RawFamily { name: "Arial".into(), monospace: false, weights: vec![700, 400, 400] },
    ]);

    let names: Vec<&str> = families.iter().map(|f| f.name()).collect();
    assert_eq!(names, vec!["Arial", "iA Writer Mono", "Inter", "Menlo"]);

    let arial = &families[0];
    assert!(!arial.monospace());
    assert_eq!(arial.weights(), &[400, 700]);

    let ia = &families[1];
    assert!(ia.monospace());
    assert_eq!(ia.weights(), &[100, 400, 900]);

    let inter = &families[2];
    assert!(!inter.monospace());
    assert_eq!(inter.weights(), &[400]);

    let menlo = &families[3];
    assert!(menlo.monospace());
    assert_eq!(menlo.weights(), &[400, 700]);
}

#[test]
fn push_weight_range_expands_variable_wght_steps() {
    use super::installed::{clamp_weight, push_weight_range};

    let mut w = Vec::new();
    push_weight_range(&mut w, 200.0, 700.0);
    assert_eq!(w, vec![200, 300, 400, 500, 600, 700]);

    let mut all = Vec::new();
    push_weight_range(&mut all, 100.0, 900.0);
    assert_eq!(all, vec![100, 200, 300, 400, 500, 600, 700, 800, 900]);

    let mut frac = Vec::new();
    push_weight_range(&mut frac, 250.0, 650.0);
    assert_eq!(frac, vec![300, 400, 500, 600]);

    // Degenerate or non-finite ranges add nothing.
    let mut empty = Vec::new();
    push_weight_range(&mut empty, 400.0, 400.0);
    push_weight_range(&mut empty, 700.0, 300.0);
    push_weight_range(&mut empty, f32::NAN, 700.0);
    push_weight_range(&mut empty, 100.0, f32::INFINITY);
    assert!(empty.is_empty());

    assert_eq!(clamp_weight(50.0), 100);
    assert_eq!(clamp_weight(350.0), 350);
    assert_eq!(clamp_weight(950.0), 900);
    assert_eq!(clamp_weight(f32::NAN), 400);
}

#[test]
fn apple_weight_trait_maps_standard_stops() {
    use super::installed::apple_weight_trait_to_css;

    assert_eq!(apple_weight_trait_to_css(-1.0), 100);
    assert_eq!(apple_weight_trait_to_css(-0.80), 100);
    assert_eq!(apple_weight_trait_to_css(-0.60), 200);
    assert_eq!(apple_weight_trait_to_css(-0.40), 300);
    assert_eq!(apple_weight_trait_to_css(0.00), 400);
    assert_eq!(apple_weight_trait_to_css(0.23), 500);
    assert_eq!(apple_weight_trait_to_css(0.30), 600);
    assert_eq!(apple_weight_trait_to_css(0.40), 700);
    assert_eq!(apple_weight_trait_to_css(0.56), 800);
    assert_eq!(apple_weight_trait_to_css(0.62), 900);
    assert_eq!(apple_weight_trait_to_css(1.0), 900);
    assert_eq!(apple_weight_trait_to_css(f64::NAN), 400);
}

#[test]
fn reader_is_monospace_distinguishes_bundled_faces_and_handles_invalid_inputs() {
    use std::io::Cursor;
    use super::installed::reader_is_monospace;

    const INTER: &[u8] = include_bytes!("../../../../rune/assets/Inter-Regular.ttf");
    const PLEX: &[u8] = include_bytes!("../../../../rune/assets/IBMPlexMono-Regular.ttf");

    assert!(!reader_is_monospace(&mut Cursor::new(INTER), 0), "Inter is proportional");
    assert!(reader_is_monospace(&mut Cursor::new(PLEX), 0), "IBM Plex Mono is fixed-pitch");
    assert!(!reader_is_monospace(&mut Cursor::new(PLEX), 1), "non-TTC face index > 0 returns false");
    assert!(!reader_is_monospace(&mut Cursor::new(b""), 0));
    assert!(!reader_is_monospace(&mut Cursor::new(b"not a font"), 0));

    // Minimal TTC wrapping two synthetic SFNT headers: face 0 proportional, face 1 fixed-pitch.
    let face0_off = 20u32; // 12-byte TTC header + 2 * 4-byte offsets
    let post0_off = face0_off + 12 + 16;
    let face1_off = post0_off + 16;
    let post1_off = face1_off + 12 + 16;
    let mut ttc = Vec::new();
    ttc.extend_from_slice(b"ttcf");
    ttc.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    ttc.extend_from_slice(&2u32.to_be_bytes());
    ttc.extend_from_slice(&face0_off.to_be_bytes());
    ttc.extend_from_slice(&face1_off.to_be_bytes());
    // Face 0: post.isFixedPitch = 0
    ttc.extend_from_slice(&[0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    ttc.extend_from_slice(b"post");
    ttc.extend_from_slice(&0u32.to_be_bytes());
    ttc.extend_from_slice(&post0_off.to_be_bytes());
    ttc.extend_from_slice(&16u32.to_be_bytes());
    ttc.extend_from_slice(&[0u8; 16]);
    // Face 1: post.isFixedPitch = 1
    ttc.extend_from_slice(&[0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    ttc.extend_from_slice(b"post");
    ttc.extend_from_slice(&0u32.to_be_bytes());
    ttc.extend_from_slice(&post1_off.to_be_bytes());
    ttc.extend_from_slice(&16u32.to_be_bytes());
    ttc.extend_from_slice(&[0u8; 12]);
    ttc.extend_from_slice(&1u32.to_be_bytes());

    assert!(!reader_is_monospace(&mut Cursor::new(&ttc), 0));
    assert!(reader_is_monospace(&mut Cursor::new(&ttc), 1));
    assert!(!reader_is_monospace(&mut Cursor::new(&ttc), 2));
}

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
#[test]
fn installed_families_are_sorted_deduplicated_and_public() {
    let first = installed_families();
    let second = installed_families();
    assert_eq!(first.as_ptr(), second.as_ptr(), "installed_families must cache in OnceLock");
    #[cfg(any(windows, target_os = "macos"))]
    assert!(!first.is_empty(), "desktop host should report installed system font families");

    for fam in first {
        assert!(!fam.name().is_empty(), "family name must be non-empty");
        assert!(!fam.name().starts_with('.'), "hidden system family leaked: {}", fam.name());
        assert!(!fam.weights().is_empty(), "family {} must report at least one weight", fam.name());
        for &w in fam.weights() {
            assert!((100..=900).contains(&w), "weight {w} out of 100..=900 for {}", fam.name());
        }
        for pair in fam.weights().windows(2) {
            assert!(pair[0] < pair[1], "weights not strictly sorted/deduped for {}: {:?}", fam.name(), fam.weights());
        }
    }

    for pair in first.windows(2) {
        let a = pair[0].name().to_lowercase();
        let b = pair[1].name().to_lowercase();
        assert!(a < b, "families not strictly sorted/deduped case-insensitively: {:?} vs {:?}", pair[0].name(), pair[1].name());
    }
}

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
#[test]
fn installed_families_classify_platform_monospace_and_proportional_faces() {
    let families = installed_families();
    let find = |name: &str| families.iter().find(|f| f.name().eq_ignore_ascii_case(name));

    #[cfg(windows)]
    {
        let mono = ["Consolas", "Cascadia Mono", "Courier New"]
            .into_iter()
            .find_map(find)
            .expect("Windows should have Consolas, Cascadia Mono, or Courier New");
        assert!(mono.monospace(), "{} should be classified as monospace", mono.name());
        assert!(mono.weights().contains(&400), "{} should include weight 400: {:?}", mono.name(), mono.weights());

        let prop = ["Segoe UI", "Arial", "Times New Roman"]
            .into_iter()
            .find_map(find)
            .expect("Windows should have Segoe UI, Arial, or Times New Roman");
        assert!(!prop.monospace(), "{} should be classified as proportional", prop.name());
    }

    #[cfg(target_os = "macos")]
    {
        let menlo = find("Menlo").expect("macOS should list Menlo");
        assert!(menlo.monospace(), "Menlo should be classified as monospace");
        assert!(menlo.weights().contains(&400), "Menlo weights: {:?}", menlo.weights());
        assert!(menlo.weights().contains(&700), "Menlo weights: {:?}", menlo.weights());

        let helvetica = find("Helvetica").expect("macOS should list Helvetica");
        assert!(!helvetica.monospace(), "Helvetica should be classified as proportional");
    }

    #[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
    {
        for name in ["DejaVu Sans Mono", "Liberation Mono", "Noto Sans Mono", "Ubuntu Mono"] {
            if let Some(f) = find(name) {
                assert!(f.monospace(), "{name} should be classified as monospace");
            }
        }
        for name in ["DejaVu Sans", "Liberation Sans", "Noto Sans", "Ubuntu"] {
            if let Some(f) = find(name) {
                assert!(!f.monospace(), "{name} should be classified as proportional");
            }
        }
    }
}

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
#[test]
fn installed_families_round_trip_through_text_system_set_families() {
    let families = installed_families();
    let mono = families.iter().find(|f| f.monospace());
    let prop = families.iter().find(|f| !f.monospace());
    let (Some(prop), Some(mono)) = (prop, mono) else { return };

    let mut ts = TextSystem::new_local(&[], ("Inter", "ui-monospace"));
    assert!(ts.font_files.is_empty(), "default launch must not load system font files");

    let sans_arg = format!("\"{}\"", prop.name().replace('\\', "\\\\").replace('"', "\\\""));
    let mono_arg = format!("\"{}\"", mono.name().replace('\\', "\\\\").replace('"', "\\\""));
    assert!(ts.set_families(&sans_arg, &mono_arg));

    let style = TextStyle::new(14.0, 400.0);
    let sans_layout = ts.layout_rich("Hamburgefonstiv", &[], style, 2.0, None);
    let mono_layout = ts.layout_rich("let x = 42;", &[], style.mono(), 2.0, None);
    assert!(sans_layout.full_width() > 0.0);
    assert!(mono_layout.full_width() > 0.0);
}

#[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
#[test]
fn fontconfig_extracts_names_weights_and_monospace_from_collection_paths() {
    let mut collection = Collection::new(CollectionOptions { shared: false, system_fonts: false });
    collection.load_fonts_from_paths([
        bundled_font("Inter-Regular.ttf"),
        bundled_font("Inter-Medium.ttf"),
        bundled_font("Inter-SemiBold.ttf"),
        bundled_font("IBMPlexMono-Regular.ttf"),
    ]);

    let families = installed::finalize(fontconfig::installed_families_from_collection(&mut collection));
    let names: Vec<&str> = families.iter().map(|f| f.name()).collect();
    assert_eq!(names, vec!["IBM Plex Mono", "Inter"]);

    let plex = &families[0];
    assert!(plex.monospace(), "IBM Plex Mono should be classified as monospace");
    assert_eq!(plex.weights(), &[400]);

    let inter = &families[1];
    assert!(!inter.monospace(), "Inter should be classified as proportional");
    assert_eq!(inter.weights(), &[400, 500, 600]);
}
