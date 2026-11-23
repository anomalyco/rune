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
