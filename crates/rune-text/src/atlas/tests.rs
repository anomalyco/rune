use super::*;

#[test]
fn small_menu_bar_label_allocates_at_most_256_kib_in_text_atlas() {
    let mut text = TextSystem::default();
    assert!(text.atlas.data.is_empty(), "unused atlas allocates zero bytes");
    let style = TextStyle::new(13.0, 400.0);
    let layout = text.layout_rich("12:34 PM  82%", &[], style, 2.0, None);
    let mut quads = Vec::new();
    text.glyphs_for_layout(&layout, 2.0, [0.0, 0.0], true, &mut quads);
    assert!(!quads.is_empty(), "label produced glyph quads");
    assert_eq!(
        text.atlas.data.len(),
        ATLAS_CPU_START_ROWS as usize * ATLAS_SIZE as usize,
        "small overlay label must allocate 256 KiB (128 rows), not 4 MiB (got {} bytes)",
        text.atlas.data.len(),
    );
    assert!(text.atlas.pages.is_empty());
}

#[test]
fn cpu_atlas_starts_small_and_grows_in_power_of_two_row_steps_with_intact_pixels() {
    let mut atlas = Atlas::new();
    assert!(atlas.data.is_empty());

    // Place a 2000x80 mask on row 1 -> stays in initial 128-row (256 KiB) allocation.
    let g0 = atlas.put(2000, 80, &vec![41u8; 2000 * 80], 0, 0).expect("first band");
    assert_eq!(g0.page, 0);
    assert_eq!(atlas.data.len(), 128 * ATLAS_SIZE as usize);

    // Next 2000x80 mask wraps to row 82 -> needs row 163 -> doubles to 256 rows (512 KiB).
    let g1 = atlas.put(2000, 80, &vec![82u8; 2000 * 80], 0, 0).expect("second band");
    assert_eq!(g1.page, 0);
    assert_eq!(atlas.data.len(), 256 * ATLAS_SIZE as usize);

    // Cross 256 -> 512 rows (1 MiB).
    let g2 = atlas.put(2000, 150, &vec![123u8; 2000 * 150], 0, 0).expect("third band");
    assert_eq!(g2.page, 0);
    assert_eq!(atlas.data.len(), 512 * ATLAS_SIZE as usize);

    // Cross 512 -> 1024 rows (2 MiB).
    let g3 = atlas.put(2000, 400, &vec![164u8; 2000 * 400], 0, 0).expect("fourth band");
    assert_eq!(g3.page, 0);
    assert_eq!(atlas.data.len(), 1024 * ATLAS_SIZE as usize);

    // Cross 1024 -> 2048 rows (4 MiB).
    let g4 = atlas.put(2000, 600, &vec![205u8; 2000 * 600], 0, 0).expect("fifth band");
    assert_eq!(g4.page, 0);
    assert_eq!(atlas.data.len(), 2048 * ATLAS_SIZE as usize);

    // Every earlier mask kept its exact pixels across all four resizes.
    for (g, want) in [(g0, 41u8), (g1, 82), (g2, 123), (g3, 164), (g4, 205)] {
        let at = ((g.y + g.h / 2) * ATLAS_SIZE + g.x + g.w / 2) as usize;
        assert_eq!(atlas.data[at], want, "mask at y={} corrupted after growth", g.y);
    }

    // Overflow onto secondary page 1: page 1 ALSO starts at 128 rows (256 KiB), not 4 MiB,
    // and grows cleanly to 256 rows (512 KiB) when a second band crosses row 128.
    let fill_tail = atlas.put(2000, 700, &vec![10u8; 2000 * 700], 0, 0).expect("fill page 0 tail");
    assert_eq!(fill_tail.page, 0);
    let p1_a = atlas.put(2000, 80, &vec![211u8; 2000 * 80], 0, 0).expect("page 1 first band");
    assert_eq!(p1_a.page, 1);
    assert_eq!(atlas.pages.len(), 1);
    assert_eq!(atlas.pages[0].data.len(), 128 * ATLAS_SIZE as usize, "secondary page starts at 256 KiB");

    let p1_b = atlas.put(2000, 80, &vec![222u8; 2000 * 80], 0, 0).expect("page 1 second band");
    assert_eq!(p1_b.page, 1);
    assert_eq!(atlas.pages[0].data.len(), 256 * ATLAS_SIZE as usize, "secondary page grows to 512 KiB");
    assert_eq!(atlas.pages[0].data[((p1_a.y + 40) * ATLAS_SIZE + p1_a.x + 10) as usize], 211);
    assert_eq!(atlas.pages[0].data[((p1_b.y + 40) * ATLAS_SIZE + p1_b.x + 10) as usize], 222);

    // resend_rows() stays within each page's allocated CPU rows.
    atlas.resend_rows();
    let (_, p0_hi) = atlas.dirty.expect("page 0 dirty");
    assert!(p0_hi as usize * ATLAS_SIZE as usize <= atlas.data.len());
    let (_, p1_hi) = atlas.pages[0].dirty.expect("page 1 dirty");
    assert!(p1_hi as usize * ATLAS_SIZE as usize <= atlas.pages[0].data.len());
}
