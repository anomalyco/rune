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
fn fit_in_handles_sub_pixel_negative_and_non_finite_boxes_without_panicking() {
    use SurfaceFit::{Fill, Fit};
    const C: SurfacePlace = SurfacePlace::CENTRED;
    const UNSCALED: SurfacePlace = SurfacePlace { unscaled: true, ..C };

    // Boxes narrower or shorter than 1 px (both continuous and within 1/512 of 0 or 1, where
    // pixel snapping runs) stay finite, positive, and inside the box across every mode.
    for (w, h) in [
        (0.5, 40.0),
        (40.0, 0.5),
        (0.5, 0.5),
        (0.001, 40.0),
        (40.0, 0.001),
        (0.999, 40.0),
        (40.0, 0.999),
        (0.001, 0.001),
    ] {
        let rect = [10.0, 20.0, w, h];
        for (mode, place) in [(None, C), (Some(Fit), C), (Some(Fill), C), (Some(Fit), UNSCALED), (Some(Fill), UNSCALED)] {
            for snap in [0.0, 0.5, 1.0, f32::NAN] {
                let (out, crop) = fit_in(mode, place, rect, (80, 20), snap);
                assert!(out.iter().all(|v| v.is_finite()), "non-finite rect for {mode:?} {place:?} ({w}, {h}) snap={snap}: {out:?}");
                assert!(out[2] > 0.0 && out[2] <= w, "width out of bounds for ({w}, {h}): {out:?}");
                assert!(out[3] > 0.0 && out[3] <= h, "height out of bounds for ({w}, {h}): {out:?}");
                assert!(out[0] >= rect[0] - 1e-4 && out[0] + out[2] <= rect[0] + w + 1e-4, "x out of bounds for ({w}, {h}): {out:?}");
                assert!(out[1] >= rect[1] - 1e-4 && out[1] + out[3] <= rect[1] + h + 1e-4, "y out of bounds for ({w}, {h}): {out:?}");
                assert!(crop.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)), "bad crop: {crop:?}");
                assert!(crop[0] <= crop[2] && crop[1] <= crop[3], "inverted crop: {crop:?}");
            }
        }
    }

    // Aspect ratio is kept when only one axis is under 1 px and the other scales to >= 1 px.
    assert_eq!(fit(Some(Fit), C, [10.0, 20.0, 0.5, 40.0], (20, 80)), ([10.0, 39.0, 0.5, 2.0], [0.0, 0.0, 1.0, 1.0]));
    assert_eq!(fit(Some(Fit), C, [10.0, 20.0, 40.0, 0.5], (80, 20)), ([29.0, 20.0, 2.0, 0.5], [0.0, 0.0, 1.0, 1.0]));
    assert_eq!(fit(Some(Fit), C, [10.0, 20.0, 0.5, 0.5], (80, 20)), ([10.0, 20.0, 0.5, 0.5], [0.0, 0.0, 1.0, 1.0]));

    // Zero, negative, NaN and infinite box sizes return the rect and whole crop without panicking.
    for (w, h) in [
        (0.0, 40.0),
        (40.0, 0.0),
        (-10.0, 40.0),
        (40.0, -10.0),
        (-0.5, -0.5),
        (f32::NAN, 40.0),
        (40.0, f32::NAN),
        (f32::NAN, f32::NAN),
        (f32::INFINITY, 40.0),
        (40.0, f32::INFINITY),
        (f32::NEG_INFINITY, 40.0),
    ] {
        let rect = [10.0, 20.0, w, h];
        for (mode, place) in [(None, C), (Some(Fit), C), (Some(Fill), C), (Some(Fit), UNSCALED)] {
            let (out, crop) = fit_in(mode, place, rect, (80, 20), 1.0);
            assert_eq!(out.map(f32::to_bits), rect.map(f32::to_bits));
            assert_eq!(crop, [0.0, 0.0, 1.0, 1.0]);
        }
    }
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

#[test]
fn clear_drops_pages_slots_and_replaces_cleanly() {
    let mut atlas = ImageAtlas::default();
    assert!(!atlas.clear());
    let big = Pixels::new();
    big.set(2048, 2, vec![7; 2048 * 2 * 4]);
    let small = Pixels::new();
    small.set(4, 4, vec![9; 4 * 4 * 4]);
    atlas.place(&big, [2048.0, 2.0]).unwrap();
    atlas.place(&small, [4.0, 4.0]).unwrap();
    atlas.end_frame();
    assert_eq!(atlas.pages.len(), 2);
    assert!(atlas.has_slot(&big, 0));
    assert!(atlas.clear());
    assert!(atlas.pages.is_empty());
    assert!(!atlas.has_slot(&big, 0));
    assert!(!atlas.has_slot(&small, 0));
    assert!(!atlas.clear());
    let placed = atlas.place(&small, [4.0, 4.0]).unwrap();
    assert_eq!(placed.page, 0);
    assert_eq!(atlas.pages[0].dirty, Some((0, 4)));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn pixels_set_does_not_wait_for_atlas_copy_and_copy_stays_consistent() {
    use std::sync::atomic::AtomicUsize;

    let px = Pixels::new();
    let (w, h) = (16u32, 8u32);
    let first_pixel = [10u8, 20, 30, 255];
    let second_pixel = [90u8, 80, 70, 255];
    px.set(w, h, first_pixel.repeat((w * h) as usize));

    let wakes = Arc::new(AtomicUsize::new(0));
    let wakes_counter = wakes.clone();
    let wake = rune_core::live::Wake::new(Arc::new(move || {
        wakes_counter.fetch_add(1, Ordering::SeqCst);
    }));
    let _sub = px.source.subscribe(&wake);

    let (mid_tx, mid_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let producer_px = px.clone();
    let producer = std::thread::spawn(move || {
        if mid_rx.recv().is_err() {
            return;
        }
        producer_px.set(w, h, second_pixel.repeat((w * h) as usize));
        done_tx.send(()).ok();
    });

    let mut atlas = ImageAtlas::default();
    let placed = atlas.place_at_with_copy_hook(&px, [w as f32, h as f32], None, |line| {
        if line == (h as usize) / 2 - 1 {
            // Confirm the frame lock is already released before handing off to the producer;
            // if the copy still holds the lock, fail immediately instead of deadlocking.
            drop(px.inner.frame.try_lock().expect("atlas copy held pixels.frame lock; a producer's Pixels::set would block behind it"));
            mid_tx.send(()).unwrap();
            done_rx.recv().unwrap();
        }
    }).unwrap().0;
    drop(mid_tx);
    producer.join().unwrap();

    assert!(wakes.load(Ordering::SeqCst) >= 1, "concurrent Pixels::set must wake subscribers");

    let page = &mut atlas.pages[placed.page];
    assert_eq!(page.dirty.take(), Some((placed.rect[1], placed.rect[1] + h)));
    let stride = (page.width * 4) as usize;
    let first_row = first_pixel.repeat(w as usize);
    for line in 0..h as usize {
        let start = (placed.rect[1] as usize + line) * stride + placed.rect[0] as usize * 4;
        assert_eq!(&page.data[start..start + (w as usize * 4)], first_row.as_slice(), "row {line} must match the snapshotted first frame");
    }

    let next = atlas.place(&px, [w as f32, h as f32]).unwrap();
    assert_eq!(next, placed, "same-size streaming frame reuses its atlas slot");
    let page = &mut atlas.pages[next.page];
    assert_eq!(page.dirty.take(), Some((next.rect[1], next.rect[1] + h)));
    let second_row = second_pixel.repeat(w as usize);
    for line in 0..h as usize {
        let start = (next.rect[1] as usize + line) * stride + next.rect[0] as usize * 4;
        assert_eq!(&page.data[start..start + (w as usize * 4)], second_row.as_slice(), "row {line} must match the frame set mid-copy");
    }

    assert_eq!(atlas.place(&px, [w as f32, h as f32]), Some(next));
    assert_eq!(atlas.pages[next.page].dirty, None);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn pixels_set_resize_during_copy_finishes_old_frame_and_next_place_resizes() {
    let assert_rect_filled = |atlas: &ImageAtlas, placed: Placed, pixel: [u8; 4]| {
        let [x, y, w, h] = placed.rect;
        let page = &atlas.pages[placed.page];
        let stride = (page.width * 4) as usize;
        let expected_row = pixel.repeat(w as usize);
        for line in 0..h as usize {
            let start = (y as usize + line) * stride + x as usize * 4;
            assert_eq!(&page.data[start..start + w as usize * 4], expected_row.as_slice(), "row {line}");
        }
    };

    let px = Pixels::new();
    let mut atlas = ImageAtlas::default();
    let cases = [
        ((8u32, 4u32), [1u8, 2, 3, 255], (16u32, 6u32), [4u8, 5, 6, 255]),
        ((16u32, 6u32), [4u8, 5, 6, 255], (IMAGE_ATLAS_SIZE + 1, 2u32), [7u8, 8, 9, 255]),
    ];

    px.set(cases[0].0.0, cases[0].0.1, cases[0].1.repeat((cases[0].0.0 * cases[0].0.1) as usize));
    for ((old_w, old_h), old_px, (new_w, new_h), new_px) in cases {
        let (mid_tx, mid_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let producer_px = px.clone();
        let producer = std::thread::spawn(move || {
            if mid_rx.recv().is_err() { return; }
            producer_px.set(new_w, new_h, new_px.repeat((new_w * new_h) as usize));
            done_tx.send(()).ok();
        });

        let placed = atlas.place_at_with_copy_hook(&px, [old_w as f32, old_h as f32], None, |line| {
            if line == (old_h as usize) / 2 - 1 {
                mid_tx.send(()).unwrap();
                done_rx.recv().unwrap();
            }
        }).unwrap().0;
        drop(mid_tx);
        producer.join().unwrap();

        assert_eq!((placed.rect[2], placed.rect[3]), (old_w, old_h));
        assert_rect_filled(&atlas, placed, old_px);
        atlas.pages[placed.page].dirty = None;

        // Next placement sees the resized frame and copies it completely.
        let resized = atlas.place(&px, [new_w as f32, new_h as f32]).unwrap();
        assert_eq!((resized.rect[2], resized.rect[3]), (new_w, new_h));
        assert_eq!(atlas.pages[resized.page].dirty.take(), Some((resized.rect[1], resized.rect[1] + new_h)));
        assert_rect_filled(&atlas, resized, new_px);
        // Set a fresh generation at the same size so the second iteration also enters the copy loop.
        px.set(new_w, new_h, new_px.repeat((new_w * new_h) as usize));
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn pixels_concurrent_sets_during_copy_stay_bounded_to_one_extra_frame() {
    let px = Pixels::new();
    let (w, h) = (8u32, 4u32);
    px.set(w, h, [10u8, 0, 0, 255].repeat((w * h) as usize));
    let w1 = px.rgba_weak().unwrap();
    assert_eq!(w1.strong_count(), 1);

    let (mid_tx, mid_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let producer_px = px.clone();
    let w1_producer = w1.clone();
    let producer = std::thread::spawn(move || {
        if mid_rx.recv().is_err() { return; }
        // While place_at is copying frame 1, frame 1 is held by both Frame and place_at.
        assert_eq!(w1_producer.strong_count(), 2);

        producer_px.set(w, h, [20u8, 0, 0, 255].repeat((w * h) as usize));
        let w2 = producer_px.rgba_weak().unwrap();
        // Frame replaced frame 1 with frame 2: only the in-flight copy keeps frame 1 alive (peak = 2 frames).
        assert_eq!(w1_producer.strong_count(), 1);
        assert_eq!(w2.strong_count(), 1);

        // Further sets while frame 1 is still copying drop uncopied intermediates immediately.
        producer_px.set(w, h, [30u8, 0, 0, 255].repeat((w * h) as usize));
        let w3 = producer_px.rgba_weak().unwrap();
        assert_eq!(w2.strong_count(), 0, "superseded uncopied frame 2 must be freed immediately");
        assert_eq!(w1_producer.strong_count(), 1);
        assert_eq!(w3.strong_count(), 1);

        producer_px.set(w, h, [40u8, 0, 0, 255].repeat((w * h) as usize));
        let w4 = producer_px.rgba_weak().unwrap();
        assert_eq!(w3.strong_count(), 0, "superseded uncopied frame 3 must be freed immediately");
        assert_eq!(w1_producer.strong_count(), 1);
        assert_eq!(w4.strong_count(), 1);

        done_tx.send(()).ok();
    });

    let mut atlas = ImageAtlas::default();
    let placed = atlas.place_at_with_copy_hook(&px, [w as f32, h as f32], None, |line| {
        if line == 1 {
            mid_tx.send(()).unwrap();
            done_rx.recv().unwrap();
        }
    }).unwrap().0;
    drop(mid_tx);
    producer.join().unwrap();

    // Once place_at returns, the in-flight snapshot drops immediately; only frame 4 stays live.
    assert_eq!(w1.strong_count(), 0, "in-flight snapshot must drop when place_at returns");
    let w4 = px.rgba_weak().unwrap();
    assert_eq!(w4.strong_count(), 1);

    // Next placement copies frame 4 and retains no Arc reference to it.
    let next = atlas.place(&px, [w as f32, h as f32]).unwrap();
    assert_eq!(next, placed);
    assert_eq!(w4.strong_count(), 1, "atlas must not retain a reference to the frame buffer");
    let page = &atlas.pages[next.page];
    let offset = (next.rect[1] as usize * page.width as usize + next.rect[0] as usize) * 4;
    assert_eq!(&page.data[offset..offset + 4], &[40, 0, 0, 255]);

    // Clearing releases the final frame buffer.
    px.set(0, 0, Vec::new());
    assert_eq!(w4.strong_count(), 0);
    assert!(px.rgba_weak().is_none());
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn pixels_set_reuses_arc_in_place_when_unshared_and_during_coalesced_updates() {
    let px = Pixels::new();
    let (w, h) = (8u32, 4u32);
    px.set(w, h, [1u8, 2, 3, 255].repeat((w * h) as usize));
    let first_arc = px.rgba_arc_ptr().unwrap();

    // Steady-state set -> place -> set reuses the same Arc allocation in place.
    let mut atlas = ImageAtlas::default();
    assert!(atlas.place(&px, [w as f32, h as f32]).is_some());
    for i in 2..=6u8 {
        px.set(w, h, [i, 2, 3, 255].repeat((w * h) as usize));
        assert_eq!(px.rgba_arc_ptr(), Some(first_arc), "steady-state set #{i} must reuse the Arc");
        assert!(atlas.place(&px, [w as f32, h as f32]).is_some());
    }

    // While place_at holds a clone of first_arc mid-copy, the first concurrent set allocates a
    // second Arc, and subsequent coalesced sets reuse that second Arc in place.
    px.set(w, h, [10u8, 2, 3, 255].repeat((w * h) as usize));
    assert_eq!(px.rgba_arc_ptr(), Some(first_arc));

    let (mid_tx, mid_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let producer_px = px.clone();
    let first_addr = first_arc as usize;
    let producer = std::thread::spawn(move || {
        if mid_rx.recv().is_err() { return 0usize; }
        producer_px.set(w, h, [20u8, 2, 3, 255].repeat((w * h) as usize));
        let second_addr = producer_px.rgba_arc_ptr().unwrap() as usize;
        assert_ne!(second_addr, first_addr, "concurrent set during copy must not mutate the in-flight Arc");

        producer_px.set(w, h, [30u8, 2, 3, 255].repeat((w * h) as usize));
        assert_eq!(producer_px.rgba_arc_ptr().unwrap() as usize, second_addr, "coalesced set during copy reuses the second Arc");

        producer_px.set(w, h, [40u8, 2, 3, 255].repeat((w * h) as usize));
        assert_eq!(producer_px.rgba_arc_ptr().unwrap() as usize, second_addr);

        done_tx.send(()).ok();
        second_addr
    });

    atlas.place_at_with_copy_hook(&px, [w as f32, h as f32], None, |line| {
        if line == 1 {
            mid_tx.send(()).unwrap();
            done_rx.recv().unwrap();
        }
    }).unwrap();
    drop(mid_tx);
    let second_addr = producer.join().unwrap();

    // After the copy finishes, steady-state streaming keeps reusing second_addr in place.
    assert!(atlas.place(&px, [w as f32, h as f32]).is_some());
    px.set(w, h, [50u8, 2, 3, 255].repeat((w * h) as usize));
    assert_eq!(px.rgba_arc_ptr().unwrap() as usize, second_addr);
}

#[test]
fn pixels_rgba_returns_snapshot_and_empty_for_mips() {
    let px = Pixels::new();
    assert!(px.rgba().is_empty());
    px.set(2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(px.rgba(), vec![1, 2, 3, 4, 5, 6, 7, 8]);
    px.set(0, 0, Vec::new());
    assert!(px.rgba().is_empty());
    let decoded = Arc::new(crate::mip_chain(2, 1, &[200, 30, 30, 255, 200, 30, 30, 255]).unwrap());
    px.store_mips(decoded);
    assert!(px.rgba().is_empty(), "decoded mips report empty raw rgba");
    assert_eq!(px.size(), (2, 1));
}

#[test]
fn pixels_with_rgba_borrows_zero_copy_releases_lock_and_returns_none_for_empty_or_mips() {
    let px = Pixels::new();
    assert_eq!(px.with_rgba(|w, h, bytes| (w, h, bytes.len())), None, "empty handle returns None");

    let original = vec![10, 20, 30, 255, 40, 50, 60, 128];
    let original_ptr = original.as_ptr();
    px.set(2, 1, original);
    let first_arc = px.rgba_arc_ptr().unwrap();

    let got = px.with_rgba(|w, h, bytes| {
        assert_eq!(bytes.as_ptr(), original_ptr, "with_rgba must borrow without copying the buffer");
        // The frame lock is already released before the closure runs.
        drop(px.inner.frame.try_lock().expect("with_rgba must drop the frame lock before invoking callback"));
        assert_eq!(px.size(), (2, 1), "calling Pixels::size inside with_rgba must not deadlock");
        (w, h, bytes.to_vec())
    });
    assert_eq!(got, Some((2, 1, vec![10, 20, 30, 255, 40, 50, 60, 128])));

    // After with_rgba returns, the Arc refcount drops back to 1 so set keeps its in-place reuse.
    px.set(2, 1, vec![99; 8]);
    assert_eq!(px.rgba_arc_ptr(), Some(first_arc), "subsequent set must keep Arc::get_mut reuse after with_rgba returns");

    // Calling set/replace inside with_rgba does not deadlock and preserves the snapshotted bytes
    // for the callback while publishing the replacement.
    let replaced_inside = px.with_rgba(|w, h, bytes| {
        assert_eq!((w, h), (2, 1));
        assert_eq!(bytes, &[99; 8]);
        let returned = px.replace(1, 1, vec![1, 2, 3, 255]);
        assert!(returned.is_empty(), "replace while with_rgba holds the Arc cannot reclaim it yet");
        assert_eq!(bytes, &[99; 8], "snapshotted slice stays valid after concurrent replace");
        bytes[0]
    });
    assert_eq!(replaced_inside, Some(99));
    assert_eq!(px.with_rgba(|w, h, b| (w, h, b.to_vec())), Some((1, 1, vec![1, 2, 3, 255])));

    // Clearing to empty returns None again.
    px.set(0, 0, Vec::new());
    assert_eq!(px.with_rgba(|_, _, _| ()), None);

    // Decoded mips return None.
    let decoded = Arc::new(crate::mip_chain(2, 1, &[200, 30, 30, 255, 200, 30, 30, 255]).unwrap());
    px.store_mips(decoded);
    assert_eq!(px.with_rgba(|_, _, _| ()), None);
}

#[test]
fn pixels_replace_recycles_buffers_in_steady_state_coalesced_and_edge_transitions() {
    let px = Pixels::new();
    let (w, h) = (8u32, 4u32);
    let len = (w * h * 4) as usize;

    // First frame has nothing to return.
    let buf_a = vec![1u8; len];
    let ptr_a = buf_a.as_ptr();
    let cap_a = buf_a.capacity();
    let prev = px.replace(w, h, buf_a);
    assert!(prev.is_empty() && prev.capacity() == 0, "first frame returns empty Vec");

    // Second frame returns buf_a with identical pointer and capacity.
    let buf_b = vec![2u8; len];
    let ptr_b = buf_b.as_ptr();
    let cap_b = buf_b.capacity();
    let mut spare = px.replace(w, h, buf_b);
    assert_eq!(spare.as_ptr(), ptr_a);
    assert_eq!(spare.capacity(), cap_a);

    // Steady-state ping-pong across place -> end_frame -> replace never reallocates.
    let mut atlas = ImageAtlas::default();
    let mut expected_next = (ptr_b, cap_b);
    let mut other = (ptr_a, cap_a);
    for i in 3..=10u8 {
        assert!(atlas.place(&px, [w as f32, h as f32]).is_some());
        atlas.end_frame();
        spare.fill(i);
        let returned = px.replace(w, h, spare);
        assert_eq!((returned.as_ptr(), returned.capacity()), expected_next, "frame {i} must return prior buffer");
        spare = returned;
        std::mem::swap(&mut expected_next, &mut other);
    }

    // Back-to-back coalesced replaces without an atlas copy return each displaced buffer.
    let (wakes, wake) = {
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = count.clone();
        (count, rune_core::live::Wake::new(Arc::new(move || { c.fetch_add(1, Ordering::SeqCst); })))
    };
    let _sub = px.source.subscribe(&wake);
    for i in 20..=25u8 {
        spare.fill(i);
        let returned = px.replace(w, h, spare);
        assert_eq!((returned.as_ptr(), returned.capacity()), expected_next);
        spare = returned;
        std::mem::swap(&mut expected_next, &mut other);
    }
    assert_eq!(wakes.load(Ordering::SeqCst), 1, "coalesced replaces wake once");

    // Dimension change still returns the prior buffer for caller reuse.
    let big = vec![7u8; 16 * 6 * 4];
    let big_ptr = big.as_ptr();
    let big_cap = big.capacity();
    let returned = px.replace(16, 6, big);
    assert_eq!((returned.as_ptr(), returned.capacity()), expected_next);

    // Clearing to empty when unshared returns the displaced RGBA buffer.
    let cleared = px.replace(0, 0, Vec::new());
    assert_eq!((cleared.as_ptr(), cleared.capacity()), (big_ptr, big_cap));
    assert_eq!(px.size(), (0, 0));

    // Replacing Empty returns empty Vec.
    assert!(px.replace(0, 0, Vec::new()).is_empty());

    // Replacing decoded Mips returns empty Vec.
    let decoded = Arc::new(crate::mip_chain(2, 1, &[200, 30, 30, 255, 200, 30, 30, 255]).unwrap());
    px.store_mips(decoded);
    assert!(px.replace(2, 1, vec![9; 8]).is_empty());
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn pixels_replace_during_concurrent_atlas_copy_returns_empty_and_drops_old_after_copy() {
    let px = Pixels::new();
    let (w, h) = (8u32, 4u32);
    let len = (w * h * 4) as usize;
    assert!(px.replace(w, h, vec![10u8; len]).is_empty());
    let w1 = px.rgba_weak().unwrap();

    let (mid_tx, mid_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let producer_px = px.clone();
    let producer = std::thread::spawn(move || {
        if mid_rx.recv().is_err() { return; }
        // First replace while place_at holds frame 1's Arc cannot reclaim frame 1.
        let b2 = vec![20u8; len];
        let ptr2 = b2.as_ptr();
        let cap2 = b2.capacity();
        assert!(producer_px.replace(w, h, b2).is_empty(), "in-flight copy prevents reclaiming frame 1");

        // Second coalesced replace during the same copy reclaims frame 2.
        let b3 = vec![30u8; len];
        let returned = producer_px.replace(w, h, b3);
        assert_eq!((returned.as_ptr(), returned.capacity()), (ptr2, cap2));
        done_tx.send(()).ok();
    });

    let mut atlas = ImageAtlas::default();
    let placed = atlas.place_at_with_copy_hook(&px, [w as f32, h as f32], None, |line| {
        if line == 1 {
            mid_tx.send(()).unwrap();
            done_rx.recv().unwrap();
        }
    }).unwrap().0;
    drop(mid_tx);
    producer.join().unwrap();

    // Once place_at returns, frame 1 drops cleanly and frame 3 is current.
    assert_eq!(w1.strong_count(), 0, "in-flight snapshot drops when place_at finishes");
    let page = &atlas.pages[placed.page];
    let offset = (placed.rect[1] as usize * page.width as usize + placed.rect[0] as usize) * 4;
    assert_eq!(&page.data[offset..offset + 4], &[10, 10, 10, 10]);
    assert_eq!(px.with_rgba(|_, _, bytes| bytes[0]), Some(30));
}

#[test]
#[ignore = "manual lock-hold measurement; reports timings, not a timing threshold"]
fn pixels_place_lock_hold_time_1080x2400() {
    let (w, h) = (1080u32, 2400u32);
    let stream = Pixels::new();
    stream.set(w, h, vec![1; (w * h * 4) as usize]);
    let mut atlas = ImageAtlas::default();
    let t0 = rune_core::time::now();
    assert!(atlas.place(&stream, [w as f32, h as f32]).is_some());
    let first_total_us = t0.elapsed().as_secs_f64() * 1e6;
    let first_lock_us = atlas.last_lock_hold.as_secs_f64() * 1e6;
    atlas.end_frame();

    let mut steady_total = Vec::with_capacity(21);
    let mut steady_lock = Vec::with_capacity(21);
    let mut unchanged_lock = Vec::with_capacity(21);
    let frames: Vec<Vec<u8>> = (0..21u8).map(|i| vec![i; (w * h * 4) as usize]).collect();
    for frame in frames {
        stream.set(w, h, frame);
        let t = rune_core::time::now();
        assert!(atlas.place(&stream, [w as f32, h as f32]).is_some());
        steady_total.push(t.elapsed().as_secs_f64() * 1e6);
        steady_lock.push(atlas.last_lock_hold.as_secs_f64() * 1e6);
        assert!(atlas.place(&stream, [w as f32, h as f32]).is_some());
        unchanged_lock.push(atlas.last_lock_hold.as_secs_f64() * 1e6);
        atlas.end_frame();
    }
    steady_total.sort_by(f64::total_cmp);
    steady_lock.sort_by(f64::total_cmp);
    unchanged_lock.sort_by(f64::total_cmp);
    eprintln!(
        "1080x2400 place: first total={first_total_us:.1} us lock={first_lock_us:.1} us; steady median total={:.1} us lock={:.1} us; unchanged median lock={:.1} us",
        steady_total[10], steady_lock[10], unchanged_lock[10]
    );
}

#[test]
fn place_at_filtered_nearest_requests_mip_zero_while_linear_uses_downsampled_mip() {
    let cache = crate::ImageCache::new(Arc::new(|| {}));
    let mut rgba = Vec::with_capacity(64 * 64 * 4);
    for y in 0..64u32 {
        for x in 0..64u32 {
            let v = if ((x / 8) + (y / 8)) % 2 == 0 { 0u8 } else { 255u8 };
            rgba.extend_from_slice(&[v, v, v, 255]);
        }
    }
    let img = cache.rgba(64, 64, rgba);
    assert_eq!(img.wait(), Ok([64, 64]));

    let mut atlas = ImageAtlas::default();
    let (lin, _) = atlas.place_at_filtered(img.pixels(), [16.0, 16.0], None, ImageFilter::Linear).unwrap();
    assert_eq!([lin.rect[2], lin.rect[3]], [16, 16], "Linear selects the 16x16 mip");
    assert!(!atlas.has_slot(img.pixels(), 0));
    assert!(atlas.has_slot(img.pixels(), 2));

    let (near, _) = atlas.place_at_filtered(img.pixels(), [16.0, 16.0], None, ImageFilter::Nearest).unwrap();
    assert_eq!([near.rect[2], near.rect[3]], [64, 64], "Nearest requests mip 0 even when scaled down");
    assert!(atlas.has_slot(img.pixels(), 0));
    assert!(atlas.has_slot(img.pixels(), 2), "Linear and Nearest slots coexist in the same atlas");
    assert!(lin.premultiplied && near.premultiplied);
}
