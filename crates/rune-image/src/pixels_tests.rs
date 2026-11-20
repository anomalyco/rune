use super::*;

#[test]
#[ignore = "manual staging benchmark; reports measurements, not a timing threshold"]
fn image_staging_costs() {
    use std::hint::black_box;
    let stream = Pixels::new();
    stream.set(224, 128, vec![0; 224 * 128 * 4]);
    let mut atlas = ImageAtlas::default();
    atlas.place(&stream, [224.0, 128.0]);
    atlas.end_frame();
    atlas.pages[0].dirty = None;
    let buffers: Vec<_> = (0..120).map(|_| vec![128; 224 * 128 * 4]).collect();
    let start = rune_core::time::now();
    let mut upload_bytes = 0u64;
    for bytes in buffers {
        stream.set(224, 128, bytes);
        black_box(atlas.place(&stream, [224.0, 128.0]));
        atlas.end_frame();
        let page = &mut atlas.pages[0];
        let (lo, hi) = page.dirty.take().unwrap();
        upload_bytes += u64::from((hi - lo) * page.width * 4);
    }
    eprintln!("stream 224x128: {:.1} us/stage, {} upload bytes/frame, {} page", start.elapsed().as_secs_f64() * 1e6 / 120.0, upload_bytes / 120, atlas.pages.len());
    let cache = crate::ImageCache::new(Arc::new(|| {}));
    let icons: Vec<_> = (0..100).map(|i| cache.rgba(128, 128, [i, 50, 100, 255].repeat(128 * 128))).collect();
    for icon in &icons { icon.wait().unwrap(); }
    let mut atlas = ImageAtlas::default();
    for icon in &icons { atlas.place(icon.pixels(), [32.0; 2]); }
    atlas.end_frame();
    for page in &mut atlas.pages { page.dirty = None; }
    let start = rune_core::time::now();
    for _ in 0..120 {
        for icon in &icons { black_box(atlas.place(icon.pixels(), [32.0; 2])); }
        atlas.end_frame();
    }
    eprintln!("100 static 128->32 icons: {:.1} us/frame, {} pages, {} dirty pages", start.elapsed().as_secs_f64() * 1e6 / 120.0, atlas.pages.len(), atlas.pages.iter().filter(|p| p.dirty.is_some()).count());
}

#[test]
fn fit_letterboxes_on_whole_pixels_and_fill_crops_the_centre() {
    use SurfaceFit::{Fill, Fit};
    const C: SurfacePlace = SurfacePlace::CENTRED;
    let rect = [10.0, 20.0, 40.0, 40.0];
    assert_eq!(fit(None, C, rect, (80, 20)), (rect, [0.0, 0.0, 1.0, 1.0]));
    assert_eq!(fit(Some(Fit), C, rect, (80, 20)), ([10.0, 35.0, 40.0, 10.0], [0.0, 0.0, 1.0, 1.0]));
    assert_eq!(fit(Some(Fit), C, rect, (20, 80)).0, [25.0, 20.0, 10.0, 40.0]);
    assert_eq!(fit(Some(Fill), C, rect, (80, 20)), (rect, [0.375, 0.0, 0.625, 1.0]));
    assert_eq!(fit(Some(Fill), C, rect, (40, 40)), (rect, [0.0, 0.0, 1.0, 1.0]));
    assert_eq!(fit(Some(Fit), C, rect, (0, 0)).0, rect, "an empty image keeps the rect");
}

#[test]
fn frames_copy_in_once_per_generation_and_resize_moves_the_slot() {
    let mut atlas = ImageAtlas::default();
    let px = Pixels::new();
    assert_eq!(atlas.place(&px, [2.0, 1.0]), None, "empty images draw nothing");
    px.set(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 128]);
    assert_eq!(atlas.place(&px, [2.0, 1.0]).unwrap().rect, [0, 0, 2, 1]);
    assert_eq!(atlas.pages[0].dirty.take(), Some((0, 1)));
    assert_eq!(&atlas.pages[0].data[..8], &[255, 0, 0, 255, 0, 255, 0, 128]);
    atlas.place(&px, [2.0, 1.0]);
    assert_eq!(atlas.pages[0].dirty, None, "an unchanged frame uploads nothing");
    px.set(3, 2, vec![7; 24]);
    let [x, y, w, h] = atlas.place(&px, [3.0, 2.0]).unwrap().rect;
    assert_eq!((w, h), (3, 2));
    assert_eq!(atlas.pages[0].data[((y * IMAGE_ATLAS_SIZE + x) * 4) as usize], 7);
    let other = Pixels::new();
    other.set(1, 1, vec![1; 4]);
    atlas.place(&other, [1.0, 1.0]);
    atlas.end_frame();
    atlas.place(&px, [3.0, 2.0]);
    atlas.end_frame();
    assert!(atlas.slots.contains_key(&(px.id, 0)) && atlas.slots.contains_key(&(other.id, 0)), "the first page keeps unshown images");
}

#[test]
fn an_image_shown_again_after_a_while_is_not_copied_again() {
    let mut atlas = ImageAtlas::default();
    let (list, grid) = (Pixels::new(), Pixels::new());
    list.set(32, 32, vec![1; 32 * 32 * 4]);
    grid.set(48, 48, vec![2; 48 * 48 * 4]);
    let shown = atlas.place(&list, [32.0; 2]).unwrap();
    atlas.end_frame();
    for _ in 0..4 {
        atlas.place(&grid, [48.0; 2]);
        atlas.end_frame();
    }
    atlas.pages[0].dirty = None;
    assert_eq!(atlas.place(&list, [32.0; 2]), Some(shown));
    assert_eq!(atlas.pages[0].dirty, None, "no copy, no upload");
}

#[test]
fn a_full_atlas_starts_over_pages_that_show_nothing_before_it_grows() {
    let mut atlas = ImageAtlas::default();
    let old: Vec<Pixels> = (0..4).map(|_| { let p = Pixels::new(); p.set(512, 512, vec![1; 512 * 512 * 4]); p }).collect();
    for p in &old { atlas.place(p, [512.0; 2]); }
    atlas.end_frame();
    atlas.end_frame();
    assert_eq!(atlas.pages.len(), 1);
    let new = Pixels::new();
    new.set(512, 512, vec![2; 512 * 512 * 4]);
    assert_eq!(atlas.place(&new, [512.0; 2]).unwrap().rect, [0, 0, 512, 512]);
    assert_eq!(atlas.pages.len(), 1);
    assert!(!atlas.slots.contains_key(&(old[0].id, 0)), "the page's old images are forgotten");
}

/// A new image earlier in the tree than a body that fills the first page: the body isn't
/// drawn yet this frame when the new image needs room.
#[test]
fn a_full_page_shown_last_frame_is_not_started_over_before_its_images_are_drawn() {
    let mut atlas = ImageAtlas::default();
    let body: Vec<Pixels> = (0..4).map(|i| { let p = Pixels::new(); p.set(512, 512, vec![i; 512 * 512 * 4]); p }).collect();
    let placed: Vec<_> = body.iter().map(|p| atlas.place(p, [512.0; 2]).unwrap()).collect();
    atlas.end_frame();
    atlas.pages[0].dirty = None;
    let new = Pixels::new();
    new.set(64, 64, vec![9; 64 * 64 * 4]);
    assert_eq!(atlas.place(&new, [64.0; 2]).unwrap().page, 1);
    assert_eq!(body.iter().map(|p| atlas.place(p, [512.0; 2]).unwrap()).collect::<Vec<_>>(), placed);
    assert_eq!(atlas.pages[0].dirty, None, "the body copies in nothing");
}

/// An image drawn twice in a frame, resized by its producer in between: the first draw's
/// pixels stay put while this frame still samples them.
#[test]
fn an_image_resized_between_two_draws_keeps_the_first_draws_pixels() {
    let mut atlas = ImageAtlas::default();
    let px = Pixels::new();
    px.set(512, 512, vec![1; 512 * 512 * 4]);
    let fill: Vec<Pixels> = (0..3).map(|_| { let p = Pixels::new(); p.set(512, 512, vec![2; 512 * 512 * 4]); p }).collect();
    atlas.place(&px, [512.0; 2]);
    for p in &fill { atlas.place(p, [512.0; 2]); }
    atlas.end_frame();
    let first = atlas.place(&px, [512.0; 2]).unwrap();
    px.set(600, 600, vec![3; 600 * 600 * 4]);
    let second = atlas.place(&px, [600.0; 2]).unwrap();
    assert_ne!(second.page, first.page);
    let [x, y, ..] = first.rect;
    assert_eq!(atlas.pages[first.page].data[((y * IMAGE_ATLAS_SIZE + x) * 4) as usize], 1);
}

#[test]
fn pages_expand_reclaim_and_keep_streaming_slots_stable() {
    let mut atlas = ImageAtlas::default();
    let stream = Pixels::new();
    stream.set(224, 128, vec![1; 224 * 128 * 4]);
    let first = atlas.place(&stream, [112.0, 64.0]).unwrap();
    assert!(!first.premultiplied);
    let large = Pixels::new();
    large.set(2048, 1, vec![2; 2048 * 4]);
    let second = atlas.place(&large, [2048.0, 1.0]).unwrap();
    assert_ne!(first.page, second.page);
    assert_eq!((atlas.pages[second.page].width, atlas.pages[second.page].height), (2048, 1));
    let full = Pixels::new();
    full.set(1024, 1024, vec![3; 1024 * 1024 * 4]);
    let third = atlas.place(&full, [1024.0; 2]).unwrap();
    assert_eq!(third.page, 2);
    atlas.end_frame();
    stream.set(224, 128, vec![4; 224 * 128 * 4]);
    assert_eq!(atlas.place(&stream, [112.0, 64.0]).unwrap(), first);
    atlas.place(&full, [1024.0; 2]);
    atlas.end_frame();
    assert_eq!(atlas.pages.len(), 3);
    assert!(atlas.pages[second.page].data.is_empty(), "unused middle standalone page releases storage");
    assert_eq!(atlas.place(&stream, [112.0, 64.0]).unwrap(), first);
    atlas.end_frame();
    assert_eq!(atlas.pages.len(), 1, "unused trailing pages release their storage");
    atlas.end_frame();
    assert_eq!(atlas.pages.len(), 1, "the first page keeps its images");
}
