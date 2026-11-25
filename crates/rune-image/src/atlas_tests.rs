use super::*;

#[test]
fn dropped_thumbnails_sharing_page_with_persistent_image_are_reclaimed() {
    let mut atlas = ImageAtlas::default();
    let persistent = Pixels::new();
    persistent.set(64, 64, [10, 20, 30, 255].repeat(64 * 64));
    let persistent_placed = atlas.place(&persistent, [64.0; 2]).unwrap();

    for f in 0..40u8 {
        assert_eq!(atlas.place(&persistent, [64.0; 2]), Some(persistent_placed));
        for t in 0..4u8 {
            let thumb = Pixels::new();
            thumb.set(256, 256, [f, t, 99, 255].repeat(256 * 256));
            assert!(atlas.place(&thumb, [256.0; 2]).is_some());
        }
        atlas.end_frame();
    }

    let bytes_after_frames: usize = atlas.pages.iter().map(|p| p.data.len()).sum();
    assert_eq!(
        atlas.pages.len(),
        1,
        "dropped thumbnails must reuse freed shelves on page 0 instead of spilling (pages={}, slots={}, bytes={bytes_after_frames})",
        atlas.pages.len(),
        atlas.slots.len(),
    );
    assert_eq!(
        atlas.slots.len(),
        5,
        "only the persistent image and the last frame's 4 thumbnails may remain (got {})",
        atlas.slots.len(),
    );
    assert!(bytes_after_frames <= 4 * 1024 * 1024);

    atlas.rest();
    let bytes_after_rest: usize = atlas.pages.iter().map(|p| p.data.len()).sum();
    assert_eq!(atlas.pages.len(), 1);
    assert_eq!(atlas.slots.len(), 1, "rest must forget dropped thumbnails on shared pages");
    assert_eq!(bytes_after_rest, 1024 * 128 * 4, "rest shrinks page 0 back to 128 rows (512 KiB)");
    assert_eq!(atlas.place(&persistent, [64.0; 2]), Some(persistent_placed));
    let [x, y, ..] = persistent_placed.rect;
    let off = ((y * atlas.pages[0].width + x) * 4) as usize;
    assert_eq!(&atlas.pages[0].data[off..off + 4], &[10, 20, 30, 255]);
}

#[test]
fn variable_size_dropped_thumbnails_coalesce_empty_bands_without_growing_pages() {
    let mut atlas = ImageAtlas::default();
    let persistent = Pixels::new();
    persistent.set(64, 64, [11, 22, 33, 255].repeat(64 * 64));
    let pin = atlas.place(&persistent, [64.0; 2]).unwrap();

    let sizes = [(200u32, 200u32), (300, 300), (400, 400), (450, 450), (180, 240), (320, 190)];
    for round in 0..24usize {
        let (w, h) = sizes[round % sizes.len()];
        assert_eq!(atlas.place(&persistent, [64.0; 2]), Some(pin));
        for k in 0..2u8 {
            let thumb = Pixels::new();
            thumb.set(w, h, [round as u8, k, 77, 255].repeat((w * h) as usize));
            assert!(atlas.place(&thumb, [w as f32, h as f32]).is_some());
        }
        atlas.end_frame();
    }

    assert!(atlas.pages.len() <= 2, "variable-size transient bursts stay bounded (pages={})", atlas.pages.len());
    assert_eq!(atlas.slots.len(), 3, "only persistent + last frame's 2 thumbnails remain");
    atlas.rest();
    assert_eq!(atlas.pages.len(), 1);
    assert_eq!(atlas.slots.len(), 1);
    assert_eq!(atlas.pages[0].height, 128);
}

#[test]
fn persistent_icons_across_thumbnail_bursts_do_not_pin_successive_pages() {
    let mut atlas = ImageAtlas::default();
    let mut icons = Vec::new();
    for i in 0..20u8 {
        let icon = Pixels::new();
        icon.set(64, 64, [i, 120, 240, 255].repeat(64 * 64));
        icons.push(icon);
        for ic in &icons {
            assert!(atlas.place(ic, [64.0; 2]).is_some());
        }
        for t in 0..3u8 {
            let thumb = Pixels::new();
            thumb.set(512, 512, [i, t, 42, 255].repeat(512 * 512));
            assert!(atlas.place(&thumb, [512.0; 2]).is_some());
        }
        atlas.end_frame();
    }

    let bytes: usize = atlas.pages.iter().map(|p| p.data.len()).sum();
    assert!(atlas.pages.len() <= 3, "persistent icons must not pin 20 pages (pages={})", atlas.pages.len());
    assert!(bytes <= 12 * 1024 * 1024, "bytes={bytes}");
    assert_eq!(atlas.slots.len(), 23, "20 live icons + 3 last-frame thumbs");

    atlas.rest();
    let rest_bytes: usize = atlas.pages.iter().map(|p| p.data.len()).sum();
    assert_eq!(atlas.pages.len(), 1);
    assert_eq!(atlas.slots.len(), 20);
    assert_eq!(rest_bytes, 1024 * 128 * 4, "all 20 icons pack into 128 rows on page 0");
}

#[test]
fn rest_forgets_unshown_live_slots_and_animation_starts_on_shared_pages() {
    let mut atlas = ImageAtlas::default();
    let persistent = Pixels::new();
    persistent.set(32, 32, [1, 2, 3, 255].repeat(32 * 32));
    let unshown_still = Pixels::new();
    unshown_still.set(64, 64, [4, 5, 6, 255].repeat(64 * 64));

    // Build a 2-frame animated image so starts gets an entry for it.
    let anim = Pixels::new();
    let f0 = crate::mip_chain(16, 16, &[255, 0, 0, 255].repeat(16 * 16)).unwrap().mips;
    let f1 = crate::mip_chain(16, 16, &[0, 255, 0, 255].repeat(16 * 16)).unwrap().mips;
    let motion = crate::animated::Motion {
        frames: vec![f1],
        ends: vec![0.1, 0.2],
        plays: None,
    };
    anim.store_mips(Arc::new(crate::Decoded { mips: f0, motion: Some(Box::new(motion)), intrinsic: None }));

    // Frame 0: draw all three on page 0.
    let pin = atlas.place(&persistent, [32.0; 2]).unwrap();
    assert_eq!(atlas.place(&unshown_still, [64.0; 2]).unwrap().page, 0);
    assert_eq!(atlas.place_at(&anim, [16.0; 2], Some(1.0)).unwrap().0.page, 0);
    assert!(atlas.starts.contains_key(&anim.id));
    atlas.end_frame();

    // Frame 1: draw only persistent. Page 0 stays touched, so live unshown slots stay cached until rest().
    assert_eq!(atlas.place(&persistent, [32.0; 2]), Some(pin));
    atlas.end_frame();
    assert!(atlas.has_slot(&unshown_still, 0));
    assert!(atlas.has_slot(&anim, 0));

    // rest() forgets images not shown in the last frame, even on shared page 0, and cleans starts.
    atlas.rest();
    assert!(atlas.has_slot(&persistent, 0));
    assert!(!atlas.has_slot(&unshown_still, 0), "rest must drop unshown live slots on shared pages");
    assert!(!atlas.has_slot(&anim, 0), "rest must drop unshown animated slots on shared pages");
    assert!(!atlas.starts.contains_key(&anim.id), "rest must clean starts entries for evicted animations");

    // Returning after eviction uploads again cleanly on that frame.
    atlas.pages[0].dirty = None;
    let back = atlas.place(&unshown_still, [64.0; 2]).unwrap();
    assert_eq!(back.page, 0);
    assert!(atlas.pages[0].dirty.is_some(), "returning image marks dirty rows for upload");
    let [x, y, ..] = back.rect;
    let off = ((y * atlas.pages[0].width + x) * 4) as usize;
    assert_eq!(&atlas.pages[0].data[off..off + 4], &[4, 5, 6, 255]);
}

#[test]
fn atlas_enforces_page_byte_and_slot_bounds_and_reuploads_evicted_images() {
    // 1. Burst past IMAGE_ATLAS_MAX_PAGES (16) and IMAGE_ATLAS_MAX_BYTES (64 MiB) in one frame,
    //    then keep only the 20 persistent icons while the 20 large images stay live in memory.
    let mut atlas = ImageAtlas::default();
    let count = IMAGE_ATLAS_MAX_PAGES + 4;
    let icons: Vec<Pixels> = (0..count)
        .map(|i| {
            let p = Pixels::new();
            p.set(64, 64, [i as u8, 50, 100, 255].repeat(64 * 64));
            p
        })
        .collect();
    let bigs: Vec<Pixels> = (0..count)
        .map(|i| {
            let p = Pixels::new();
            p.set(900, 900, [i as u8, 200, 10, 255].repeat(900 * 900));
            p
        })
        .collect();

    // Frame 0 draws every icon + big image: nothing currently shown may fail or be blanked.
    for i in 0..count {
        assert!(atlas.place(&icons[i], [64.0; 2]).is_some());
        assert!(atlas.place(&bigs[i], [900.0; 2]).is_some());
    }
    atlas.end_frame();
    assert!(atlas.pages.len() >= count, "burst frame genuinely exceeded IMAGE_ATLAS_MAX_PAGES");

    // Frame 1 draws only the 20 icons (bigs are still alive!). Bound enforcement evicts unshown bigs,
    // shrinks shelf rows, and flags overflow pages >= IMAGE_ATLAS_MAX_PAGES to relocate.
    for ic in &icons {
        assert!(atlas.place(ic, [64.0; 2]).is_some());
    }
    atlas.end_frame();
    let bytes_f1: usize = atlas.pages.iter().map(|p| p.data.len()).sum();
    assert!(bytes_f1 <= IMAGE_ATLAS_MAX_BYTES, "bytes {bytes_f1} > {IMAGE_ATLAS_MAX_BYTES}");

    // Frame 2 relocates overflow icons onto earlier pages and drops pages >= IMAGE_ATLAS_MAX_PAGES.
    for ic in &icons {
        assert!(atlas.place(ic, [64.0; 2]).is_some());
    }
    atlas.end_frame();
    let bytes_f2: usize = atlas.pages.iter().map(|p| p.data.len()).sum();
    assert!(atlas.pages.len() <= IMAGE_ATLAS_MAX_PAGES, "pages {} > {IMAGE_ATLAS_MAX_PAGES}", atlas.pages.len());
    assert!(bytes_f2 <= IMAGE_ATLAS_MAX_BYTES, "bytes {bytes_f2} > {IMAGE_ATLAS_MAX_BYTES}");

    // An evicted large image returning on frame 3 uploads again with exact pixels.
    assert!(!atlas.has_slot(&bigs[0], 0));
    let returned = atlas.place(&bigs[0], [900.0; 2]).unwrap();
    assert!(atlas.pages[returned.page].dirty.is_some());
    let [rx, ry, ..] = returned.rect;
    let roff = ((ry * atlas.pages[returned.page].width + rx) * 4) as usize;
    assert_eq!(&atlas.pages[returned.page].data[roff..roff + 4], &[0, 200, 10, 255]);

    // 2. Live-handle streaming across 16 pinned shared pages stays within pages & bytes bounds.
    let mut bounded = ImageAtlas::default();
    let pins: Vec<Pixels> = (0..IMAGE_ATLAS_MAX_PAGES)
        .map(|i| {
            let p = Pixels::new();
            p.set(64, 64, [i as u8, 10, 20, 255].repeat(64 * 64));
            p
        })
        .collect();
    let mut live_thumbs = Vec::new();
    for (i, pin) in pins.iter().enumerate() {
        assert!(bounded.place(pin, [64.0; 2]).is_some());
        for t in 0..12u8 {
            let th = Pixels::new();
            th.set(256, 256, [i as u8, t, 88, 255].repeat(256 * 256));
            assert!(bounded.place(&th, [256.0; 2]).is_some());
            live_thumbs.push(th);
        }
    }
    bounded.end_frame();
    // Advance one frame drawing only the pins so the thumbnails become cached (not shown).
    for p in &pins {
        assert!(bounded.place(p, [64.0; 2]).is_some());
    }
    bounded.end_frame();
    // Stream 40 more live thumbnails while keeping all pins active: must stay within bounds.
    for round in 0..10u8 {
        for p in &pins {
            assert!(bounded.place(p, [64.0; 2]).is_some());
        }
        for k in 0..4u8 {
            let th = Pixels::new();
            th.set(256, 256, [200 + round, k, 33, 255].repeat(256 * 256));
            assert!(bounded.place(&th, [256.0; 2]).is_some());
            live_thumbs.push(th);
        }
        bounded.end_frame();
        let b: usize = bounded.pages.iter().map(|p| p.data.len()).sum();
        assert!(bounded.pages.len() <= IMAGE_ATLAS_MAX_PAGES, "round {round}: pages={}", bounded.pages.len());
        assert!(b <= IMAGE_ATLAS_MAX_BYTES, "round {round}: bytes={b}");
    }

    // 3. Slot-count bound (IMAGE_ATLAS_MAX_SLOTS = 4096) with live 16x16 icons held in memory.
    let mut slot_atlas = ImageAtlas::default();
    let anchor = Pixels::new();
    anchor.set(16, 16, [255, 255, 0, 255].repeat(16 * 16));
    let mut kept = Vec::new();
    for frame in 0..40u32 {
        assert!(slot_atlas.place(&anchor, [16.0; 2]).is_some());
        for k in 0..128u32 {
            let ic = Pixels::new();
            ic.set(16, 16, [(frame & 0xff) as u8, (k & 0xff) as u8, 5, 255].repeat(16 * 16));
            assert!(slot_atlas.place(&ic, [16.0; 2]).is_some());
            kept.push(ic);
        }
        slot_atlas.end_frame();
        assert!(slot_atlas.slots.len() <= IMAGE_ATLAS_MAX_SLOTS, "frame {frame}: slots={}", slot_atlas.slots.len());
        assert!(slot_atlas.pages.len() <= IMAGE_ATLAS_MAX_PAGES);
        let b: usize = slot_atlas.pages.iter().map(|p| p.data.len()).sum();
        assert!(b <= IMAGE_ATLAS_MAX_BYTES);
    }
    // An evicted early icon comes back and uploads cleanly.
    assert!(!slot_atlas.has_slot(&kept[0], 0));
    let re = slot_atlas.place(&kept[0], [16.0; 2]).unwrap();
    assert!(slot_atlas.pages[re.page].dirty.is_some());
    let [x, y, ..] = re.rect;
    let off = ((y * slot_atlas.pages[re.page].width + x) * 4) as usize;
    assert_eq!(&slot_atlas.pages[re.page].data[off..off + 4], &[0, 0, 5, 255]);
}

#[test]
fn image_cache_counts_and_eviction_do_not_retain_dropped_images() {
    let cache = crate::ImageCache::new(Arc::new(|| {}));
    let persistent = cache.rgba(32, 32, [10, 20, 30, 255].repeat(32 * 32));
    persistent.wait().unwrap();
    let transient = cache.rgba(64, 64, [200, 100, 50, 255].repeat(64 * 64));
    transient.wait().unwrap();
    let transient_id = transient.id();

    let mut atlas = ImageAtlas::default();
    let pin = atlas.place(persistent.pixels(), [32.0; 2]).unwrap();
    let tr = atlas.place(transient.pixels(), [64.0; 2]).unwrap();
    assert_eq!(tr.page, 0);
    atlas.end_frame();

    // Dropping the transient Image must immediately drop the cache's strong State.
    drop(transient);
    assert!(cache.entries.lock().unwrap().get(&transient_id).and_then(std::sync::Weak::upgrade).is_none());

    // Next frame draws only persistent: dead transient slot is evicted from shared page 0.
    assert_eq!(atlas.place(persistent.pixels(), [32.0; 2]), Some(pin));
    atlas.end_frame();
    assert_eq!(atlas.slots.len(), 1);

    // A live cached image evicted by rest() still deduplicates in ImageCache and re-uploads cleanly.
    let returning = cache.rgba(64, 64, [50, 150, 250, 255].repeat(64 * 64));
    returning.wait().unwrap();
    let dup = cache.rgba(64, 64, [50, 150, 250, 255].repeat(64 * 64));
    assert_eq!(returning.id(), dup.id());
    assert_eq!(returning.pixels().signature().0, dup.pixels().signature().0);
    atlas.place(persistent.pixels(), [32.0; 2]).unwrap();
    atlas.place(returning.pixels(), [64.0; 2]).unwrap();
    atlas.end_frame();
    atlas.place(persistent.pixels(), [32.0; 2]).unwrap();
    atlas.end_frame();
    atlas.rest();
    assert!(!atlas.has_slot(returning.pixels(), 0));
    atlas.pages[0].dirty = None;
    let again = atlas.place(returning.pixels(), [64.0; 2]).unwrap();
    assert!(atlas.pages[again.page].dirty.is_some());
}

#[test]
fn mid_frame_resize_keeps_first_draw_and_reclaims_old_rect_next_frame() {
    let mut atlas = ImageAtlas::default();
    let px = Pixels::new();
    px.set(512, 512, [11, 22, 33, 255].repeat(512 * 512));
    let first = atlas.place(&px, [512.0; 2]).unwrap();
    assert_eq!(first.page, 0);
    px.set(600, 600, [44, 55, 66, 255].repeat(600 * 600));
    let second = atlas.place(&px, [600.0; 2]).unwrap();
    assert_ne!(second.page, first.page);

    // End of frame 0: page 0 stays pinned by deferred_this_frame so frame 0's first draw reads intact pixels.
    atlas.end_frame();
    let [fx, fy, ..] = first.rect;
    let foff = ((fy * atlas.pages[0].width + fx) * 4) as usize;
    assert!(!atlas.pages[0].data.is_empty());
    assert_eq!(&atlas.pages[0].data[foff..foff + 4], &[11, 22, 33, 255]);

    // End of frame 1 (drawing only the resized image): deferred rect expires and page 0 empties.
    assert_eq!(atlas.place(&px, [600.0; 2]), Some(second));
    atlas.end_frame();
    assert!(atlas.pages[0].data.is_empty(), "page 0 freed once deferred mid-frame resize rect expires");
}

#[test]
fn steady_overflow_scene_keeps_pages_resident_without_per_frame_copies_and_reclaims_after_shrink() {
    for &(w, h, label) in &[(900u32, 900u32, "900x900 shelf"), (1200u32, 1200u32, "1200x1200 standalone")] {
        let count = IMAGE_ATLAS_MAX_PAGES + 4;
        let imgs: Vec<Pixels> = (0..count)
            .map(|i| {
                let p = Pixels::new();
                p.set(w, h, [i as u8, 80, 160, 255].repeat((w * h) as usize));
                p
            })
            .collect();

        let mut atlas = ImageAtlas::default();
        let mut prev_placed: Vec<Placed> = imgs.iter().map(|p| atlas.place(p, [w as f32, h as f32]).unwrap()).collect();
        atlas.end_frame();
        assert_eq!(atlas.pages.len(), count, "{label}: initial frame allocates {count} pages");
        for page in &mut atlas.pages {
            page.dirty = None;
        }

        // Steady state: all 20 images remain shown across 5 consecutive frames.
        for frame in 1..=5usize {
            #[cfg(debug_assertions)]
            let copies_before = atlas.copies;
            let mut moved = 0usize;
            let mut rows_copied = 0usize;
            for (i, img) in imgs.iter().enumerate() {
                let placed = atlas.place_at_with_copy_hook(img, [w as f32, h as f32], None, |_| {
                    rows_copied += 1;
                }).unwrap().0;
                if placed != prev_placed[i] {
                    moved += 1;
                    prev_placed[i] = placed;
                }
            }
            let dirty_pages = atlas.pages.iter().filter(|p| p.dirty.is_some()).count();
            let bytes_copied = rows_copied * (w as usize) * 4;
            #[cfg(debug_assertions)]
            let copies_delta = atlas.copies - copies_before;
            #[cfg(not(debug_assertions))]
            let copies_delta = if rows_copied > 0 { 1 } else { 0 };
            assert_eq!(atlas.pages.len(), count, "{label} frame {frame}: page count stays stable");
            assert_eq!(moved, 0, "{label} frame {frame}: no steady-state overflow slot may move");
            assert_eq!(copies_delta, 0, "{label} frame {frame}: steady-state overflow must not re-copy slots on CPU");
            assert_eq!(bytes_copied, 0, "{label} frame {frame}: steady-state overflow must copy 0 bytes");
            assert_eq!(dirty_pages, 0, "{label} frame {frame}: steady-state overflow must leave every page.dirty == None");
            atlas.end_frame();
        }

        // Shrink visible set below IMAGE_ATLAS_MAX_PAGES (keep 6 from 0..16 + the 4 overflow images 16..20).
        let kept: Vec<usize> = (0..6).chain(IMAGE_ATLAS_MAX_PAGES..count).collect();
        for &i in &kept {
            assert!(atlas.place(&imgs[i], [w as f32, h as f32]).is_some());
        }
        atlas.end_frame();
        // Next frame relocates the 4 overflow images into the freed pages < 16.
        for &i in &kept {
            assert!(atlas.place(&imgs[i], [w as f32, h as f32]).is_some());
        }
        atlas.end_frame();
        assert!(
            atlas.pages.len() <= IMAGE_ATLAS_MAX_PAGES,
            "{label}: after shrink, pages {} must drop to <= {IMAGE_ATLAS_MAX_PAGES}",
            atlas.pages.len(),
        );
        assert_eq!(
            atlas.pages.iter().filter(|p| !p.data.is_empty()).count(),
            kept.len(),
            "{label}: only the {} kept pages stay allocated",
            kept.len(),
        );
        for &i in &kept {
            let p = atlas.place(&imgs[i], [w as f32, h as f32]).unwrap();
            let [x, y, ..] = p.rect;
            let off = ((y * atlas.pages[p.page].width + x) * 4) as usize;
            assert_eq!(&atlas.pages[p.page].data[off..off + 4], &[i as u8, 80, 160, 255]);
        }
    }
}
