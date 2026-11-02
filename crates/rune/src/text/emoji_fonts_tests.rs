use super::*;

#[test]
fn color_capability_precedes_monochrome_and_preserves_desktop_order() {
    let found = candidates(
        "Outline\t/fonts/outline.ttf\tFalse\nPreferred\t/fonts/custom.ttf\tTrue\nNoto Color Emoji\t/fonts/noto.ttf\tTrue\nPreferred\t/fonts/custom-bold.ttf\tTrue\n",
    );
    assert_eq!(
        found
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["Preferred", "Noto Color Emoji", "Outline"]
    );
}

#[test]
fn missing_color_fonts_keep_a_monochrome_fallback() {
    assert_eq!(
        candidates("DejaVu Sans\t/fonts/dejavu.ttf\tFalse\n"),
        vec![("DejaVu Sans".into(), "/fonts/dejavu.ttf".into())]
    );
    assert!(candidates("broken\n\t/fonts/x\tTrue\nx\trelative.ttf\tTrue\n").is_empty());
}

#[test]
fn unsupported_color_formats_do_not_hide_outline_fallback() {
    assert!(supported_color_format(None, false, false));
    assert!(supported_color_format(Some(&[0, 0, 0, 1]), false, false));
    assert!(!supported_color_format(Some(&[0, 1, 0, 0]), false, false));
    assert!(!supported_color_format(Some(&[0]), false, false));
    assert!(!supported_color_format(None, false, true));
    assert!(supported_color_format(Some(&[0, 1, 0, 0]), true, false));
}

#[test]
fn colrv1_with_v0_base_glyph_records_is_supported() {
    assert!(supported_color_format(Some(&[0, 1, 0, 1]), false, false));
    assert!(supported_color_format(Some(&[0, 1, 1, 0]), false, false));
    assert!(!supported_color_format(Some(&[0, 0, 0, 0]), false, false));
    assert!(!supported_color_format(Some(&[0, 1, 0]), false, false));
}

#[test]
fn registered_symbol_emoji_cannot_bypass_the_format_guard() {
    let mut data = crate::text::DEFAULT_FONTS[0].to_vec();
    let count = u16::from_be_bytes([data[4], data[5]]) as usize;
    let record = (0..count)
        .map(|i| 12 + i * 16)
        .find(|&offset| &data[offset..offset + 4] == b"post")
        .unwrap();
    let offset = u32::from_be_bytes(data[record + 8..record + 12].try_into().unwrap()) as usize;
    data[record..record + 4].copy_from_slice(b"COLR");
    data[offset..offset + 4].copy_from_slice(&[0, 1, 0, 0]);
    let mut records: Vec<_> = data[12..12 + count * 16]
        .chunks_exact(16)
        .map(|record| <[u8; 16]>::try_from(record).unwrap())
        .collect();
    records.sort_by_key(|record| <[u8; 4]>::try_from(&record[..4]).unwrap());
    for (record, bytes) in data[12..12 + count * 16].chunks_exact_mut(16).zip(records) {
        record.copy_from_slice(&bytes);
    }
    assert_eq!(
        FontRef::from_index(&data, 0)
            .unwrap()
            .table(swash::tag_from_bytes(b"COLR"))
            .unwrap()[..4],
        [0, 1, 0, 0]
    );
    let mut collection = Collection::new(CollectionOptions {
        shared: false,
        system_fonts: false,
    });
    collection.register_fonts(
        Blob::new(Arc::new(data)),
        Some(parley::fontique::FontInfoOverride {
            family_name: Some("Noto Color Emoji"),
            ..Default::default()
        }),
    );
    let mut cache = SourceCache::default();
    assert!(
        collection
            .family_by_name("Noto Color Emoji")
            .unwrap()
            .default_font()
            .is_some()
    );
    assert!(!retain_usable_fonts(
        &mut collection,
        &mut cache,
        "Noto Color Emoji"
    ));
    assert!(
        collection
            .family_by_name("Noto Color Emoji")
            .is_none_or(|family| family.fonts().is_empty())
    );
    collection.register_fonts(Blob::new(Arc::new(crate::text::DEFAULT_FONTS[0])), None);
    assert!(retain_usable_fonts(&mut collection, &mut cache, "Inter"));
}

#[test]
fn colrv1_paint_records_make_a_face_eligible() {
    const TEST_GLYPHS: &[u8] = include_bytes!("../../tests/colr/test_glyphs-glyf_colr_1.ttf");
    const NOTO: &[u8] = include_bytes!("../../tests/colr/Noto-COLRv1-grinning.ttf");
    let colr = |font| {
        FontRef::from_index(font, 0)
            .unwrap()
            .table(swash::tag_from_bytes(b"COLR"))
            .unwrap()
            .to_vec()
    };
    let noto = colr(NOTO);
    assert_eq!(noto[..4], [0, 1, 0, 0]);
    assert!(supported_color_format(Some(&noto), false, false));
    assert!(supported_color_format(
        Some(&colr(TEST_GLYPHS)),
        false,
        true
    ));
    let mut no_paints = noto.clone();
    no_paints[14..18].copy_from_slice(&[0; 4]);
    assert!(!supported_color_format(Some(&no_paints), false, false));
    assert!(!supported_color_format(Some(&noto[..20]), false, false));
    assert!(!supported_color_format(None, false, true));
    let mut collection = Collection::new(CollectionOptions {
        shared: false,
        system_fonts: false,
    });
    collection.register_fonts(
        Blob::new(Arc::new(NOTO)),
        Some(parley::fontique::FontInfoOverride {
            family_name: Some("Noto Color Emoji"),
            ..Default::default()
        }),
    );
    let mut cache = SourceCache::default();
    assert!(retain_usable_fonts(
        &mut collection,
        &mut cache,
        "Noto Color Emoji"
    ));
    assert!(
        collection
            .family_by_name("Noto Color Emoji")
            .is_some_and(|family| !family.fonts().is_empty())
    );
}
