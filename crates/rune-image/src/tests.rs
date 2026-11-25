use super::*;
use crate::freedesktop;

#[test]
fn mips_filter_linear_premultiplied_pixels_including_odd_edges() {
    let d = mip_chain(3, 1, &[255, 0, 0, 255, 0, 255, 0, 0, 255, 0, 0, 255]).unwrap();
    assert_eq!(d.mips.len(), 2);
    assert_eq!(&d.mips[0].pixels[4..8], &[0, 0, 0, 0]);
    assert_eq!(d.mips[1].pixels[3], 170);
    assert_eq!(d.mips[1].pixels[1], 0); // no transparent-green fringe
    assert!((d.mips[1].pixels[0] as i32 - 213).abs() <= 1);
    let gray = mip_chain(2, 1, &[0, 0, 0, 255, 255, 255, 255, 255]).unwrap();
    assert_eq!(gray.mips[1].pixels, [188, 188, 188, 255]);
    let tail = mip_chain(3, 1, &[255, 255, 255, 1, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap();
    assert_eq!(tail.mips[1].pixels, [0, 0, 0, 0], "alpha rounded to zero cannot emit light");
}

#[test]
fn decoded_pixels_wake_windows_only_once_the_image_reports_ready() {
    let image = Image::new(Arc::new(State { id: [0; 32], pixels: Pixels::new(), decoded: OnceLock::new() }));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (probe, record) = (image.clone(), seen.clone());
    let wake = rune_core::live::Wake::new(Arc::new(move || {
        record.lock().unwrap().push((probe.is_ready(), probe.size(), probe.pixels().size()));
    }));
    let _subscription = image.pixels().source.subscribe(&wake);
    image.finish(mip_chain(2, 1, &[0, 0, 0, 255, 255, 255, 255, 255]));
    assert_eq!(*seen.lock().unwrap(), [(true, Some([2, 1]), (2, 1))]);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn image_wait_and_is_ready_see_decoded_pixels_already_stored() {
    // Hold the pixels' frame lock while a worker finishes decoding: `is_ready` must stay false
    // until the mips are stored, so `Image::wait` and `is_ready` never observe empty pixels.
    let image = Image::new(Arc::new(State { id: [0; 32], pixels: Pixels::new(), decoded: OnceLock::new() }));
    let decoded = mip_chain(2, 1, &[200, 30, 30, 255, 200, 30, 30, 255]);
    let guard = image.pixels().hold_frame();
    let (started_tx, started_rx) = mpsc::channel();
    let worker = image.clone();
    let finish = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        worker.finish(decoded);
    });
    started_rx.recv().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let ready_before_store = image.is_ready();
    drop(guard);
    finish.join().unwrap();
    assert!(!ready_before_store, "image reported ready before its pixels held the decoded mips");
    assert_eq!(image.wait(), Ok([2, 1]));
    assert_eq!(image.pixels().size(), (2, 1));
    let mut atlas = crate::pixels::ImageAtlas::default();
    assert!(atlas.place(image.pixels(), [2.0, 1.0]).is_some(), "decoded pixels place immediately after wait");
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_small_image_queued_behind_a_large_one_decodes_without_waiting_for_it() {
    if std::thread::available_parallelism().map_or(1, |n| n.get()) < 2 { return; }
    let (tx, rx) = mpsc::channel();
    let caller = std::thread::current().id();
    let cache = ImageCache::new(Arc::new(move || { tx.send(std::thread::current().id()).ok(); }));
    let (release, gate) = mpsc::channel();
    let input = Input::Rgba(128, 128, vec![128; 128 * 128 * 4].into());
    let large = cache.request([8; 32], Input::Gated(Box::new(input), gate));
    let inline = cache.rgba(64, 64, vec![255; 64 * 64 * 4]);
    assert_eq!(inline.size(), Some([64, 64]));
    assert_eq!(rx.try_recv(), Ok(caller));
    assert_eq!(rx.try_recv(), Err(mpsc::TryRecvError::Empty));
    // Just over the inline limit: a second worker must finish this one.
    let small = cache.rgba(65, 64, vec![255; 65 * 64 * 4]);
    assert_ne!(rx.recv_timeout(std::time::Duration::from_secs(120)).unwrap(), caller);
    assert_eq!(small.size(), Some([65, 64]));
    assert!(!large.is_ready());
    release.send(()).unwrap();
    rx.recv_timeout(std::time::Duration::from_secs(120)).unwrap();
    assert_eq!(large.size(), Some([128, 128]));
}

#[test]
fn cache_deduplicates_and_wakes_after_async_completion() {
    let (tx, rx) = mpsc::channel();
    let cache = ImageCache::new(Arc::new(move || { tx.send(()).unwrap(); }));
    let queued = || cache.rgba(65, 64, [20, 40, 60, 255].repeat(65 * 64));
    let first = queued();
    let second = queued();
    assert!(Arc::ptr_eq(&first.0, &second.0));
    rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert_eq!(first.size(), Some([65, 64]));
    assert_eq!(first.decoded().unwrap().unwrap().mips[0].pixels, [20, 40, 60, 255].repeat(65 * 64));
    let invalid = cache.load(vec![0, 1, 2]);
    rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert!(invalid.error().is_some());
}

#[test]
fn a_fetch_is_wanted_while_a_handle_or_its_drawn_pixels_live() {
    let image = Image::new(Arc::new(State { id: [0; 32], pixels: Pixels::new(), decoded: OnceLock::new() }));
    let weak = Arc::downgrade(&image.0);
    let drawn = image.pixels().clone();
    drop(image);
    assert!(wanted(&weak), "an element still draws it");
    drop(drawn);
    assert!(!wanted(&weak), "a guess replaced before its fetch runs fetches nothing");
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn drawn_pixels_keep_their_image_cached_after_the_handle_drops() {
    let (tx, rx) = mpsc::channel();
    let cache = ImageCache::new(Arc::new(move || { tx.send(()).ok(); }));
    let image = cache.rgba(1, 1, vec![20, 40, 60, 255]);
    assert_eq!(image.size(), Some([1, 1]));
    assert_eq!(rx.try_recv(), Ok(()), "a fresh inline decode wakes before returning");
    assert_eq!(rx.try_recv(), Err(mpsc::TryRecvError::Empty));
    // A view keeps only the pixels in its element, then asks again on the next build.
    let drawn = image.pixels().clone();
    let first = Arc::downgrade(&image.0);
    drop(image);
    let again = cache.rgba(1, 1, vec![20, 40, 60, 255]);
    assert!(std::ptr::eq(first.as_ptr(), Arc::as_ptr(&again.0)), "the same entry, not a new request");
    assert!(again.is_ready());
    assert_eq!(rx.try_recv(), Err(mpsc::TryRecvError::Empty), "a cache hit does not wake");
    drop((again, drawn));
    assert!(first.upgrade().is_none(), "nothing left draws it, so it's released");
}

#[test]
fn png_jpeg_and_icns_decode_and_malformed_containers_fail() {
    let source = image::RgbImage::from_pixel(8, 4, image::Rgb([20, 40, 60]));
    for format in [image::ImageFormat::Png, image::ImageFormat::Jpeg] {
        let mut encoded = std::io::Cursor::new(Vec::new());
        source.write_to(&mut encoded, format).unwrap();
        let bytes = encoded.into_inner();
        assert_eq!(browser_decodes(&bytes), format == image::ImageFormat::Jpeg, "web decodes JPEG in the browser, PNG itself");
        let d = decode(&bytes).unwrap();
        assert_eq!((d.mips[0].width, d.mips[0].height), (8, 4));
        assert_eq!((d.mips.last().unwrap().width, d.mips.last().unwrap().height), (1, 1));
        if format == image::ImageFormat::Png {
            let mut icns = b"icns".to_vec();
            icns.extend_from_slice(&((bytes.len() + 16) as u32).to_be_bytes());
            icns.extend_from_slice(b"icp4");
            icns.extend_from_slice(&((bytes.len() + 8) as u32).to_be_bytes());
            icns.extend_from_slice(&bytes);
            assert!(!browser_decodes(&icns));
            assert_eq!(decode(&icns).unwrap().mips[0].pixels, d.mips[0].pixels);
            icns[12..16].copy_from_slice(&0u32.to_be_bytes());
            assert!(decode(&icns).is_err());
        }
    }
    assert!(mip_chain(0, 1, &[]).is_err());
    assert!(mip_chain(1, 1, &[0]).is_err());
}

#[test]
fn a_jpeg_turns_upright_by_its_exif_orientation() {
    // Red on the left, blue on the right, stored as the camera saw it.
    let source = image::RgbImage::from_fn(16, 8, |x, _| if x < 8 { image::Rgb([255, 0, 0]) } else { image::Rgb([0, 0, 255]) });
    let mut encoded = std::io::Cursor::new(Vec::new());
    source.write_to(&mut encoded, image::ImageFormat::Jpeg).unwrap();
    let jpeg = encoded.into_inner();
    // An APP1 Exif segment after SOI: a little-endian TIFF header and one IFD entry,
    // Orientation (0x0112) = 6, "rotate 90° clockwise to display".
    let mut tiff = b"II*\0".to_vec();
    tiff.extend_from_slice(&8u32.to_le_bytes());
    tiff.extend_from_slice(&1u16.to_le_bytes());
    tiff.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0]);
    tiff.extend_from_slice(&0u32.to_le_bytes());
    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend_from_slice(&tiff);
    let mut rotated = jpeg[..2].to_vec();
    rotated.extend_from_slice(&[0xff, 0xe1]);
    rotated.extend_from_slice(&(app1.len() as u16 + 2).to_be_bytes());
    rotated.extend_from_slice(&app1);
    rotated.extend_from_slice(&jpeg[2..]);

    let plain = decode(&jpeg).unwrap();
    assert_eq!((plain.mips[0].width, plain.mips[0].height), (16, 8));
    let upright = decode(&rotated).unwrap();
    let top = &upright.mips[0];
    assert_eq!((top.width, top.height), (8, 16), "turned a quarter");
    let at = |x: u32, y: u32| &top.pixels[((y * top.width + x) * 4) as usize..][..3];
    assert!(at(4, 2)[0] > 200 && at(4, 2)[2] < 60, "the left (red) half is now on top: {:?}", at(4, 2));
    assert!(at(4, 13)[2] > 200 && at(4, 13)[0] < 60, "the right (blue) half below: {:?}", at(4, 13));
}

#[test]
fn a_decoder_panic_fails_its_image_and_the_worker_goes_on() {
    let (tx, rx) = mpsc::channel();
    let cache = ImageCache::new(Arc::new(move || { tx.send(()).unwrap(); }));
    let broken = cache.request([7; 32], Input::Panic);
    rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert_eq!(broken.error(), Some(&ImageError("image decoder panicked: decoder bug".into())));
    let next = cache.rgba(65, 64, vec![255; 65 * 64 * 4]);
    rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert_eq!(next.size(), Some([65, 64]), "later queued images still load");
}

#[cfg(feature = "svg")]
#[test]
fn svg_rasterizes_at_the_requested_device_size_from_bytes_mime_files_and_svgz() {
    const SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="8" viewBox="0 0 16 8"><rect width="8" height="8" fill="#ff0000"/></svg>"##;
    let crc = |data: &[u8]| !data.iter().fold(!0u32, |c, &b| (0..8).fold(c ^ b as u32, |c, _| (c >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(c & 1))));
    // Gzip with one stored deflate block.
    let mut svgz = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff, 1];
    svgz.extend_from_slice(&(SVG.len() as u16).to_le_bytes());
    svgz.extend_from_slice(&(!(SVG.len() as u16)).to_le_bytes());
    svgz.extend_from_slice(SVG);
    svgz.extend_from_slice(&crc(SVG).to_le_bytes());
    svgz.extend_from_slice(&(SVG.len() as u32).to_le_bytes());
    let dir = std::env::temp_dir().join(format!("rune-svg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("mark.svg"), SVG).unwrap();
    std::fs::write(dir.join("mark.svgz"), &svgz).unwrap();
    let cache = ImageCache::new(Arc::new(|| {}));
    let cases = [
        ("load: twice its own size", cache.load(SVG), [32, 16]),
        ("load: sniffed svgz", cache.load(svgz.clone()), [32, 16]),
        ("svg: 24 pt at 2x", cache.svg(SVG, 24.0, 2.0), [48, 24]),
        ("svg: 16 pt at 3x", cache.svg(svgz.clone(), 16.0, 3.0), [48, 24]),
        ("mime", cache.load_mime(SVG, "image/svg+xml; charset=utf-8", 8.0, 2.0), [16, 8]),
        ("file .svg", cache.file(dir.join("mark.svg"), 32.0, 2.0), [64, 32]),
        ("file .svgz", cache.file(dir.join("mark.svgz"), 32.0, 1.0), [32, 16]),
    ];
    for (what, image, size) in cases {
        assert_eq!(image.wait(), Ok(size), "{what}");
        let mip = &image.decoded().unwrap().unwrap().mips[0];
        let at = |x: u32, y: u32| &mip.pixels[((y * mip.width + x) * 4) as usize..][..4];
        assert_eq!(at(1, size[1] / 2), [255, 0, 0, 255], "{what}: the left half is red");
        assert_eq!(at(size[0] - 2, size[1] / 2), [0, 0, 0, 0], "{what}: the right half is clear");
    }
    assert!(cache.load_mime(b"<svg".to_vec(), "image/svg+xml", 16.0, 2.0).wait().is_err(), "malformed SVG fails");
    assert!(cache.file(dir.join("missing.svg"), 16.0, 2.0).wait().is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn desktop_entries_resolve_the_closest_theme_size_then_scalable_then_pixmaps() {
    let root = std::env::temp_dir().join(format!("rune-icon-theme-{}", std::process::id()));
    let (user, system) = (root.join("user"), root.join("system"));
    let put = |path: std::path::PathBuf| {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"icon").unwrap();
        path
    };
    let entry = |icon: &str| {
        let path = root.join(format!("{}.desktop", icon.replace('/', "_")));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&path, format!("[Desktop Entry]\nName=App\nIcon={icon}\n[Desktop Action new]\nIcon=other\n")).unwrap();
        path
    };
    let dirs = [user.clone(), system.clone()];
    put(system.join("icons/hicolor/32x32/apps/editor.png"));
    let at_64 = put(user.join("icons/hicolor/64x64/apps/editor.png"));
    let at_2x = put(system.join("icons/hicolor/48x48@2/apps/editor.png"));
    put(system.join("icons/hicolor/256x256/apps/editor.png"));
    let svg = put(system.join("icons/hicolor/scalable/apps/editor.svg"));
    assert_eq!(freedesktop::icon_file(&entry("editor"), 48, &dirs), Some(at_64.clone()), "smallest size covering 48px");
    assert_eq!(freedesktop::icon_file(&entry("editor"), 90, &dirs), Some(at_2x), "48x48@2 is 96px");
    assert_eq!(freedesktop::icon_file(&entry("editor.png"), 48, &dirs), Some(at_64), "an extension in Icon= is ignored");
    std::fs::remove_dir_all(system.join("icons/hicolor/256x256")).unwrap();
    assert_eq!(freedesktop::icon_file(&entry("editor"), 512, &dirs), Some(svg), "scalable beats a too-small bitmap");
    let pixmap = put(system.join("pixmaps/legacy.png"));
    assert_eq!(freedesktop::icon_file(&entry("legacy"), 48, &dirs), Some(pixmap.clone()));
    assert_eq!(freedesktop::icon_file(&entry(pixmap.to_str().unwrap()), 48, &dirs), Some(pixmap), "absolute Icon= path");
    assert_eq!(freedesktop::icon_file(&entry("missing"), 48, &dirs), None);
    assert_eq!(freedesktop::icon_name("[Desktop Action x]\nIcon=no\n"), None, "only the main group counts");
    std::fs::remove_dir_all(&root).unwrap();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn rgba_into_builds_mips_off_thread_and_atlas_selects_downscaled_level() {
    let (tx, rx) = mpsc::channel();
    let caller = std::thread::current().id();
    let cache = ImageCache::new(Arc::new(move || { tx.send(std::thread::current().id()).ok(); }));

    // 256x256 1-pixel checkerboard (black/white), drawn at 37x37 px as in issue #1385.
    let board: Vec<u8> = (0..256u32 * 256)
        .flat_map(|i| {
            let v = if (i % 256 + i / 256) % 2 == 0 { 0 } else { 255 };
            [v, v, v, 255]
        })
        .collect();

    let mip_px = Pixels::new();
    let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let wakes_c = wakes.clone();
    let wake = rune_core::live::Wake::new(Arc::new(move || {
        wakes_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }));
    let _sub = mip_px.source.subscribe(&wake);

    cache.rgba_into(&mip_px, 256, 256, board.clone());
    let worker = rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap();
    assert_ne!(worker, caller, "256x256 rgba_into must build mips off the calling thread");
    assert_eq!(wakes.load(std::sync::atomic::Ordering::SeqCst), 1, "target source wakes once");
    assert_eq!(mip_px.size(), (256, 256));
    assert_eq!(mip_px.with_rgba(|_, _, _| ()), None, "mipmapped Pixels holds mips, not straight RGBA");

    let mut atlas = crate::pixels::ImageAtlas::default();
    let placed = atlas.place(&mip_px, [37.0, 37.0]).unwrap();
    assert!(placed.premultiplied);
    assert_eq!((placed.rect[2], placed.rect[3]), (64, 64), "37x37 draw selects the 64x64 mip level");
    let page = &atlas.pages[placed.page];
    let stride = (page.width * 4) as usize;
    for y in 0..64usize {
        for x in 0..64usize {
            let i = (placed.rect[1] as usize + y) * stride + (placed.rect[0] as usize + x) * 4;
            assert_eq!(&page.data[i..i + 4], &[188, 188, 188, 255], "downscaled checkerboard mip is uniform linear-light grey");
        }
    }

    // Contrast with unmipped Pixels::set: places the full 256x256 straight-alpha level.
    let raw_px = Pixels::new();
    raw_px.set(256, 256, board);
    let raw_placed = atlas.place(&raw_px, [37.0, 37.0]).unwrap();
    assert!(!raw_placed.premultiplied);
    assert_eq!((raw_placed.rect[2], raw_placed.rect[3]), (256, 256));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn rgba_into_inline_fast_path_and_edge_inputs() {
    let (tx, rx) = mpsc::channel();
    let caller = std::thread::current().id();
    let cache = ImageCache::new(Arc::new(move || { tx.send(std::thread::current().id()).ok(); }));
    let px = Pixels::new();

    // Small image (<= 64x64) completes inline on the calling thread.
    cache.rgba_into(&px, 16, 16, [255, 0, 0, 255].repeat(16 * 16));
    assert_eq!(px.size(), (16, 16));
    assert_eq!(rx.try_recv(), Ok(caller));

    let mut atlas = crate::pixels::ImageAtlas::default();
    let placed = atlas.place(&px, [4.0, 4.0]).unwrap();
    assert!(placed.premultiplied);
    assert_eq!((placed.rect[2], placed.rect[3]), (4, 4));

    // Mismatched byte length and oversized dimensions are ignored without modifying target.
    cache.rgba_into(&px, 16, 16, vec![0u8; 4]);
    cache.rgba_into(&px, MAX_SIDE + 1, 1, vec![0u8; (MAX_SIDE as usize + 1) * 4]);
    assert_eq!(px.size(), (16, 16));
    assert_eq!(rx.try_recv(), Err(mpsc::TryRecvError::Empty));

    // Zero dimensions with empty bytes clear target to empty.
    cache.rgba_into(&px, 0, 0, Vec::new());
    assert_eq!(px.size(), (0, 0));
    assert_eq!(atlas.place(&px, [4.0, 4.0]), None);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn rgba_into_is_latest_wins_against_set_replace_and_newer_rgba_into() {
    let wait_dropped = |weak: &Weak<[u8]>| {
        let start = rune_core::time::now();
        while weak.strong_count() > 0 {
            assert!(start.elapsed() < std::time::Duration::from_secs(30), "gated worker did not finish in time");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    };

    let (tx, rx) = mpsc::channel();
    let cache = ImageCache::new(Arc::new(move || { tx.send(()).ok(); }));

    // 1. A gated rgba_into superseded by Pixels::set must not overwrite the newer set frame.
    let px = Pixels::new();
    let (release1, gate1) = mpsc::channel();
    let raw1: Arc<[u8]> = [255, 0, 0, 255].repeat(128 * 128).into();
    let weak1 = Arc::downgrade(&raw1);
    let stale_ticket = cache.rgba_into_gated(&px, 128, 128, raw1, gate1);
    px.set(2, 1, vec![0, 255, 0, 255, 0, 255, 0, 255]);
    // Direct store_mips_if_current with the stale ticket also rejects cleanly.
    let decoded = Arc::new(mip_chain(2, 1, &[255, 0, 0, 255, 255, 0, 0, 255]).unwrap());
    assert!(!px.store_mips_if_current(stale_ticket, decoded));
    // The worker may already have skipped the job and dropped gate1 before decoding.
    release1.send(()).ok();
    wait_dropped(&weak1);
    assert_eq!(rx.try_recv(), Err(mpsc::TryRecvError::Empty), "superseded rgba_into must not wake cache");
    assert_eq!(px.size(), (2, 1));
    assert_eq!(px.with_rgba(|w, h, b| (w, h, b.to_vec())), Some((2, 1, vec![0, 255, 0, 255, 0, 255, 0, 255])));

    // 2. A gated rgba_into superseded by a newer rgba_into must not overwrite the newer mips.
    let px2 = Pixels::new();
    let (release2, gate2) = mpsc::channel();
    let raw2: Arc<[u8]> = [255, 0, 0, 255].repeat(128 * 128).into();
    let weak2 = Arc::downgrade(&raw2);
    let _ = cache.rgba_into_gated(&px2, 128, 128, raw2, gate2);
    cache.rgba_into(&px2, 16, 16, [0, 0, 255, 255].repeat(16 * 16));
    rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert_eq!(px2.size(), (16, 16));
    release2.send(()).ok();
    wait_dropped(&weak2);
    assert_eq!(rx.try_recv(), Err(mpsc::TryRecvError::Empty), "stale gated rgba_into must not wake cache");
    assert_eq!(px2.size(), (16, 16), "stale gated rgba_into must not overwrite newer rgba_into");
    let mut atlas = crate::pixels::ImageAtlas::default();
    let placed = atlas.place(&px2, [16.0, 16.0]).unwrap();
    let page = &atlas.pages[placed.page];
    let i = (placed.rect[1] as usize * page.width as usize + placed.rect[0] as usize) * 4;
    assert_eq!(&page.data[i..i + 4], &[0, 0, 255, 255]);

    // 3. Dropping the target Pixels before its queued rgba_into finishes wakes nothing and skips cleanly.
    let dropped = Pixels::new();
    let (release3, gate3) = mpsc::channel();
    let raw3: Arc<[u8]> = [9, 9, 9, 255].repeat(128 * 128).into();
    let weak3 = Arc::downgrade(&raw3);
    let _ = cache.rgba_into_gated(&dropped, 128, 128, raw3, gate3);
    drop(dropped);
    release3.send(()).ok();
    wait_dropped(&weak3);
    assert_eq!(rx.try_recv(), Err(mpsc::TryRecvError::Empty), "dropped target must not wake cache");
}

#[cfg(target_os = "windows")]
#[test]
fn app_icon_reads_an_executables_icon() {
    let notepad = std::path::Path::new(r"C:\Windows\System32\notepad.exe");
    if !notepad.is_file() { return; }
    let (w, h, pixels) = app_icon_rgba(notepad, 48).unwrap();
    assert_eq!((w, h), (48, 48));
    assert!(pixels.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
}

#[cfg(target_os = "macos")]
#[test]
fn app_icon_rasterizes_calculator_off_the_main_thread_and_shares_handles() {
    let cache = ImageCache::new(Arc::new(|| {}));
    let icon = cache.app_icon("/System/Applications/Calculator.app", 32.0);
    assert!(Arc::ptr_eq(&icon.0, &cache.app_icon("/System/Applications/Calculator.app", 32.0).0));
    assert!(!Arc::ptr_eq(&icon.0, &cache.app_icon_scaled("/System/Applications/Calculator.app", 32.0, 1.0).0));
    assert_eq!(icon.wait(), Ok([64, 64]));
    let pixels = &icon.decoded().unwrap().unwrap().mips[0].pixels;
    let opaque = pixels.chunks_exact(4).filter(|p| p[3] > 0).count();
    assert!(opaque > 64 * 64 / 4, "icon is mostly empty: {opaque} visible pixels");
}

/// A small icon is the large one scaled down, not a different (on macOS 26 and later, a
/// framed "legacy") rendition.
#[cfg(target_os = "macos")]
#[test]
fn a_small_app_icon_matches_the_large_one_scaled_down() {
    let dir = std::env::temp_dir().join(format!("rune-icon-bundle-{}", std::process::id()));
    let app = synthetic_app(&dir);
    let small = app_icon_rgba(&app, 32).unwrap().2;
    let large = app_icon_rgba(&app, 256).unwrap().2;
    std::fs::remove_dir_all(&dir).ok();
    let (mut total, mut count) = (0u32, 0u32);
    for y in 0..32 {
        for x in 0..32 {
            // Straight colour is noise where the icon is nearly transparent.
            let solid = small[(y * 32 + x) * 4 + 3] >= 200;
            for c in (if solid { 0 } else { 3 })..4 {
                let block: u32 = (0..8).flat_map(|dy| (0..8).map(move |dx| (dx, dy)))
                    .map(|(dx, dy)| u32::from(large[((y * 8 + dy) * 256 + x * 8 + dx) * 4 + c])).sum();
                total += (block / 64).abs_diff(u32::from(small[(y * 32 + x) * 4 + c]));
                count += 1;
            }
        }
    }
    // The resampling filter rings on the tile's hard edge, so compare the mean, not the worst
    // pixel. A different rendition (a framed or shrunken tile) differs across the whole icon.
    let mean = total as f32 / count as f32;
    assert!(mean < 12.0, "the 32 px icon differs from the 256 px one by {mean} on average");
}

/// A bundle whose only icon is a generated 256 px gradient, so the test does not depend on
/// the icons the installed macOS ships.
#[cfg(target_os = "macos")]
fn synthetic_app(dir: &std::path::Path) -> std::path::PathBuf {
    let app = dir.join("Synthetic.app");
    let resources = app.join("Contents/Resources");
    std::fs::create_dir_all(&resources).unwrap();
    std::fs::write(app.join("Contents/Info.plist"), concat!(
        r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>"#,
        "<key>CFBundleIdentifier</key><string>dev.rune.synthetic-icon</string>",
        "<key>CFBundleName</key><string>Synthetic</string>",
        "<key>CFBundlePackageType</key><string>APPL</string>",
        "<key>CFBundleIconFile</key><string>icon</string></dict></plist>",
    )).unwrap();
    let mut rgba = Vec::with_capacity(256 * 256 * 4);
    for y in 0..256u32 {
        for x in 0..256u32 {
            let inside = (16..240).contains(&x) && (16..240).contains(&y);
            rgba.extend_from_slice(&[x as u8, y as u8, (x + y) as u8 / 2, if inside { 255 } else { 0 }]);
        }
    }
    let mut png = Vec::new();
    let mut encoder = png::Encoder::new(&mut png, 256, 256);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().unwrap().write_image_data(&rgba).unwrap();
    let mut icns = b"icns".to_vec();
    icns.extend_from_slice(&((png.len() as u32 + 16).to_be_bytes()));
    icns.extend_from_slice(b"ic08");
    icns.extend_from_slice(&((png.len() as u32 + 8).to_be_bytes()));
    icns.extend_from_slice(&png);
    std::fs::write(resources.join("icon.icns"), icns).unwrap();
    app
}

#[cfg(feature = "svg")]
#[test]
fn intrinsic_size_exposes_svg_css_dimensions_independently_of_raster_size() {
    let cache = ImageCache::new(Arc::new(|| {}));

    // Pending image returns None until decoding finishes.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (release, gate) = mpsc::channel();
        let pending = cache.request(
            [9; 32],
            Input::Gated(Box::new(Input::Rgba(4, 2, vec![255; 4 * 2 * 4].into())), gate),
        );
        assert_eq!(pending.intrinsic_size(), None, "pending image has no intrinsic size yet");
        release.send(()).unwrap();
        assert_eq!(pending.wait(), Ok([4, 2]));
        assert_eq!(pending.intrinsic_size(), Some([4.0, 2.0]));
    }

    // Error image returns None.
    let bad = cache.load(b"<svg".to_vec());
    assert!(bad.wait().is_err());
    assert_eq!(bad.intrinsic_size(), None);

    // Small 16x8 SVG: load() supersamples 2x to 32x16, intrinsic_size() stays [16.0, 8.0].
    const SMALL: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="8"><rect width="16" height="8" fill="#2060a0"/></svg>"##;
    let small = cache.load(SMALL);
    assert_eq!(small.wait(), Ok([32, 16]));
    assert_eq!(small.size(), Some([32, 16]));
    assert_eq!(small.intrinsic_size(), Some([16.0, 8.0]));

    // Capped 5000x1 SVG: load() caps raster longest side at 4096, intrinsic_size() keeps [5000.0, 1.0].
    const WIDE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="5000" height="1"><rect width="5000" height="1" fill="#2060a0"/></svg>"##;
    let wide = cache.load(WIDE);
    assert_eq!(wide.wait(), Ok([4096, 1]));
    assert_eq!(wide.size(), Some([4096, 1]));
    assert_eq!(wide.intrinsic_size(), Some([5000.0, 1.0]));

    // Non-integer viewBox dimensions are preserved without integer rounding.
    const FRAC: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16.5 8.25"><rect width="16.5" height="8.25" fill="#2060a0"/></svg>"##;
    let frac = cache.load(FRAC);
    assert!(frac.wait().is_ok());
    assert_eq!(frac.intrinsic_size(), Some([16.5, 8.25]));

    // Explicit raster target sizes (svg, load_scaled, load_mime, file) keep the SVG's intrinsic size.
    let custom = cache.svg(SMALL, 24.0, 2.0);
    assert_eq!(custom.wait(), Ok([48, 24]));
    assert_eq!(custom.intrinsic_size(), Some([16.0, 8.0]));

    let scaled = cache.load_scaled(SMALL, 32.0, 2.0);
    assert_eq!(scaled.wait(), Ok([64, 32]));
    assert_eq!(scaled.intrinsic_size(), Some([16.0, 8.0]));

    let by_mime = cache.load_mime(SMALL, "image/svg+xml", 12.0, 2.0);
    assert_eq!(by_mime.wait(), Ok([24, 12]));
    assert_eq!(by_mime.intrinsic_size(), Some([16.0, 8.0]));

    let dir = std::env::temp_dir().join(format!("rune-svg-intrinsic-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mark.svg");
    std::fs::write(&path, SMALL).unwrap();
    let by_file = cache.file(&path, 20.0, 2.0);
    assert_eq!(by_file.wait(), Ok([40, 20]));
    assert_eq!(by_file.intrinsic_size(), Some([16.0, 8.0]));
    std::fs::remove_dir_all(&dir).ok();

    // Raster PNG fallback returns mip 0's decoded dimensions as [f32; 2].
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(12, 6, image::Rgba([10, 20, 30, 255]))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let raster = cache.load(png.into_inner());
    assert_eq!(raster.wait(), Ok([12, 6]));
    assert_eq!(raster.intrinsic_size(), Some([12.0, 6.0]));
}

#[test]
fn load_scaled_downsamples_rasters_bounds_memory_and_never_upscales() {
    let encode_png = |img: &image::RgbaImage| {
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    };
    let cache = ImageCache::new(Arc::new(|| {}));

    // 512x256 checkerboard PNG -> load_scaled(32 pt at 2x = 64 px) decodes to 64x32.
    let board = image::RgbaImage::from_fn(512, 256, |x, y| {
        let v = if (x + y) % 2 == 0 { 0 } else { 255 };
        image::Rgba([v, v, v, 255])
    });
    let board_png = encode_png(&board);
    let full = cache.load(board_png.clone());
    let thumb = cache.load_scaled(board_png.clone(), 32.0, 2.0);
    let same_key = cache.load_scaled(board_png.clone(), 64.0, 1.0);
    let diff_key = cache.load_scaled(board_png.clone(), 32.0, 1.0);
    assert!(Arc::ptr_eq(&thumb.0, &same_key.0), "same (bytes, max_px) shares one cache handle");
    assert!(!Arc::ptr_eq(&thumb.0, &diff_key.0), "distinct max_px gets its own cache handle");
    assert!(!Arc::ptr_eq(&thumb.0, &full.0), "load_scaled does not collide with full load");

    assert_eq!(full.wait(), Ok([512, 256]));
    assert_eq!(thumb.wait(), Ok([64, 32]));
    assert_eq!(thumb.size(), Some([64, 32]));
    assert_eq!(thumb.intrinsic_size(), Some([64.0, 32.0]));

    let mip_bytes = |img: &Image| -> usize {
        img.decoded().unwrap().unwrap().mips.iter().map(|m| m.pixels.len()).sum()
    };
    let thumb_bytes = mip_bytes(&thumb);
    let full_bytes = mip_bytes(&full);
    assert!(thumb_bytes <= 64 * 32 * 4 * 2, "scaled mip chain stays bounded: {thumb_bytes}");
    assert!(thumb_bytes * 32 < full_bytes, "scaled decode uses far fewer bytes ({thumb_bytes} vs {full_bytes})");
    // Linear-light area downsampling of a black/white checkerboard yields sRGB ~188 grey.
    let top_px = &thumb.decoded().unwrap().unwrap().mips[0].pixels;
    assert_eq!(&top_px[..4], &[188, 188, 188, 255]);

    // Transparent edge does not bleed transparent-green fringe when downsampling.
    let fringe = image::RgbaImage::from_fn(8, 8, |x, _| {
        if x < 4 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 255, 0, 0]) }
    });
    let fringe_thumb = cache.load_scaled(encode_png(&fringe), 1.0, 1.0);
    assert_eq!(fringe_thumb.wait(), Ok([1, 1]));
    let px = &fringe_thumb.decoded().unwrap().unwrap().mips[0].pixels;
    assert_eq!(px[1], 0, "no green fringe from transparent pixels: {px:?}");
    assert!(px[0] > 180 && px[3] == 128, "linear premultiplied red at 50% alpha: {px:?}");

    // Portrait 256x512 -> 32x64; extreme 1000x2 -> 32x1 (height clamped to >= 1).
    let portrait = encode_png(&image::RgbaImage::from_pixel(256, 512, image::Rgba([10, 20, 30, 255])));
    assert_eq!(cache.load_scaled(portrait, 32.0, 2.0).wait(), Ok([32, 64]));
    let strip = encode_png(&image::RgbaImage::from_pixel(1000, 2, image::Rgba([10, 20, 30, 255])));
    assert_eq!(cache.load_scaled(strip, 32.0, 1.0).wait(), Ok([32, 1]));

    // Never upscales a small raster image.
    let tiny = encode_png(&image::RgbaImage::from_pixel(16, 16, image::Rgba([40, 80, 120, 255])));
    assert_eq!(cache.load_scaled(tiny.clone(), 64.0, 2.0).wait(), Ok([16, 16]));

    // Edge target sizes: zero, negative, NaN, infinity clamp to 1 px; huge clamps to MAX_SIDE without upscaling.
    for (size_pt, scale) in [(0.0, 2.0), (-8.0, 2.0), (f32::NAN, 2.0), (f32::INFINITY, 2.0)] {
        assert_eq!(cache.load_scaled(tiny.clone(), size_pt, scale).wait(), Ok([1, 1]));
    }
    assert_eq!(cache.load_scaled(tiny, 100_000.0, 2.0).wait(), Ok([16, 16]));

    // EXIF orientation is applied before downsampling.
    let source = image::RgbImage::from_fn(16, 8, |x, _| if x < 8 { image::Rgb([255, 0, 0]) } else { image::Rgb([0, 0, 255]) });
    let mut encoded = std::io::Cursor::new(Vec::new());
    source.write_to(&mut encoded, image::ImageFormat::Jpeg).unwrap();
    let jpeg = encoded.into_inner();
    let mut tiff = b"II*\0".to_vec();
    tiff.extend_from_slice(&8u32.to_le_bytes());
    tiff.extend_from_slice(&1u16.to_le_bytes());
    tiff.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0]);
    tiff.extend_from_slice(&0u32.to_le_bytes());
    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend_from_slice(&tiff);
    let mut rotated = jpeg[..2].to_vec();
    rotated.extend_from_slice(&[0xff, 0xe1]);
    rotated.extend_from_slice(&(app1.len() as u16 + 2).to_be_bytes());
    rotated.extend_from_slice(&app1);
    rotated.extend_from_slice(&jpeg[2..]);
    let scaled_rot = cache.load_scaled(rotated, 8.0, 1.0);
    assert_eq!(scaled_rot.wait(), Ok([4, 8]));
    let top = &scaled_rot.decoded().unwrap().unwrap().mips[0];
    let at = |x: u32, y: u32| &top.pixels[((y * top.width + x) * 4) as usize..][..3];
    assert!(at(2, 1)[0] > 200 && at(2, 1)[2] < 60, "red half stays on top after orientation + scale");
    assert!(at(2, 6)[2] > 200 && at(2, 6)[0] < 60, "blue half stays on bottom after orientation + scale");

    // ICNS containing a 64x64 PNG downsamples cleanly.
    let icns_png_bytes = encode_png(&image::RgbaImage::from_pixel(64, 64, image::Rgba([20, 40, 60, 255])));
    let mut icns = b"icns".to_vec();
    icns.extend_from_slice(&((icns_png_bytes.len() + 16) as u32).to_be_bytes());
    icns.extend_from_slice(b"icp4");
    icns.extend_from_slice(&((icns_png_bytes.len() + 8) as u32).to_be_bytes());
    icns.extend_from_slice(&icns_png_bytes);
    assert_eq!(cache.load_scaled(icns, 16.0, 1.0).wait(), Ok([16, 16]));

    // Malformed bytes fail cleanly.
    let invalid = cache.load_scaled(vec![0, 1, 2], 32.0, 2.0);
    assert!(invalid.wait().is_err());
    assert_eq!(invalid.intrinsic_size(), None);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn load_scaled_decodes_only_first_frame_of_animated_gif_and_apng() {
    let cache = ImageCache::new(Arc::new(|| {}));

    // Two-frame 128x64 animated GIF: frame 0 red, frame 1 blue.
    let mut gif = Vec::new();
    {
        let mut enc = image::codecs::gif::GifEncoder::new(&mut gif);
        let delay = image::Delay::from_numer_denom_ms(100, 1);
        let f0 = image::Frame::from_parts(image::RgbaImage::from_pixel(128, 64, image::Rgba([255, 0, 0, 255])), 0, 0, delay);
        let f1 = image::Frame::from_parts(image::RgbaImage::from_pixel(128, 64, image::Rgba([0, 0, 255, 255])), 0, 0, delay);
        enc.encode_frames([f0, f1]).unwrap();
    }
    let full_gif = cache.load(gif.clone());
    assert_eq!(full_gif.wait(), Ok([128, 64]));
    assert!(full_gif.decoded().unwrap().unwrap().motion.is_some(), "load() keeps GIF animation");

    let scaled_gif = cache.load_scaled(gif, 16.0, 2.0);
    assert_eq!(scaled_gif.wait(), Ok([32, 16]));
    let d_gif = scaled_gif.decoded().unwrap().unwrap();
    assert!(d_gif.motion.is_none(), "load_scaled() must keep only frame 0 with no animation motion");
    assert_eq!(&d_gif.mips[0].pixels[..4], &[255, 0, 0, 255], "frame 0 is red");

    // Two-frame 128x64 APNG: frame 0 green, frame 1 blue.
    let make_png = |rgba: [u8; 4]| {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(128, 64, image::Rgba(rgba))
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    };
    let png_chunks = |data: &[u8]| -> Vec<([u8; 4], Vec<u8>)> {
        let mut out = Vec::new();
        let mut pos = 8;
        while pos + 12 <= data.len() {
            let len = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
            let kind: [u8; 4] = data[pos + 4..pos + 8].try_into().unwrap();
            let body = data[pos + 8..pos + 8 + len].to_vec();
            out.push((kind, body));
            pos += 12 + len;
        }
        out
    };
    let crc32 = |kind: &[u8; 4], body: &[u8]| -> u32 {
        let mut c = !0u32;
        for &b in kind.iter().chain(body.iter()) {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = (c >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(c & 1));
            }
        }
        !c
    };
    let push_chunk = |out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]| {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out.extend_from_slice(&crc32(kind, body).to_be_bytes());
    };
    let fctl = |seq: u32| {
        let mut b = Vec::with_capacity(26);
        b.extend_from_slice(&seq.to_be_bytes());
        b.extend_from_slice(&128u32.to_be_bytes());
        b.extend_from_slice(&64u32.to_be_bytes());
        b.extend_from_slice(&0u32.to_be_bytes());
        b.extend_from_slice(&0u32.to_be_bytes());
        b.extend_from_slice(&100u16.to_be_bytes());
        b.extend_from_slice(&1000u16.to_be_bytes());
        b.push(0);
        b.push(0);
        b
    };
    let p0 = png_chunks(&make_png([0, 255, 0, 255]));
    let p1 = png_chunks(&make_png([0, 0, 255, 255]));
    let mut apng = b"\x89PNG\r\n\x1a\n".to_vec();
    let ihdr = &p0.iter().find(|(k, _)| k == b"IHDR").unwrap().1;
    push_chunk(&mut apng, b"IHDR", ihdr);
    push_chunk(&mut apng, b"acTL", &[0, 0, 0, 2, 0, 0, 0, 0]);
    push_chunk(&mut apng, b"fcTL", &fctl(0));
    for (k, body) in &p0 {
        if k == b"IDAT" {
            push_chunk(&mut apng, b"IDAT", body);
        }
    }
    push_chunk(&mut apng, b"fcTL", &fctl(1));
    let mut seq = 2u32;
    for (k, body) in &p1 {
        if k == b"IDAT" {
            let mut fdat = Vec::with_capacity(4 + body.len());
            fdat.extend_from_slice(&seq.to_be_bytes());
            fdat.extend_from_slice(body);
            push_chunk(&mut apng, b"fdAT", &fdat);
            seq += 1;
        }
    }
    push_chunk(&mut apng, b"IEND", &[]);

    let full_apng = cache.load(apng.clone());
    assert_eq!(full_apng.wait(), Ok([128, 64]));
    assert!(full_apng.decoded().unwrap().unwrap().motion.is_some(), "load() keeps APNG animation");

    let scaled_apng = cache.load_scaled(apng, 16.0, 2.0);
    assert_eq!(scaled_apng.wait(), Ok([32, 16]));
    let d_apng = scaled_apng.decoded().unwrap().unwrap();
    assert!(d_apng.motion.is_none(), "load_scaled() must keep only frame 0 of APNG");
    assert_eq!(&d_apng.mips[0].pixels[..4], &[0, 255, 0, 255], "frame 0 is green");
}

#[test]
fn to_rgba_reads_decoded_pixels_back_straight_and_svg_at_its_intrinsic_size() {
    let cache = ImageCache::new(Arc::new(|| {}));
    let source = image::RgbaImage::from_fn(3, 2, |x, y| image::Rgba([x as u8 * 100, y as u8 * 200, 30, [255, 128, 0][x as usize]]));
    let mut png = Vec::new();
    source.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let image = cache.load(png);
    image.wait().unwrap();
    let (w, h, rgba) = image.to_rgba().unwrap();
    assert_eq!((w, h), (3, 2));
    for (got, want) in rgba.chunks(4).zip(source.pixels()) {
        match want.0[3] {
            0 => assert_eq!(got, [0; 4], "fully transparent pixels hold no colour"),
            255 => assert_eq!(got, want.0, "opaque pixels round-trip exactly"),
            _ => assert!(got.iter().zip(want.0).all(|(g, w)| g.abs_diff(w) <= 2), "{got:?} vs {:?}", want.0),
        }
    }

    #[cfg(feature = "svg")]
    {
        const SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="8"><rect width="16" height="8" fill="#2060a0"/></svg>"##;
        let svg = cache.load(SVG);
        assert_eq!(svg.wait(), Ok([32, 16]));
        let (w, h, rgba) = svg.to_rgba().unwrap();
        assert_eq!((w, h, rgba.len()), (16, 8, 16 * 8 * 4), "an SVG copies at its intrinsic size, not its 2x raster");
        assert_eq!(&rgba[..4], &[0x20, 0x60, 0xa0, 255]);
    }

    let bad = cache.load(b"not an image".as_slice());
    assert!(bad.wait().is_err());
    assert_eq!(bad.to_rgba(), None);
}
