use super::*;
use crate::app_icon::freedesktop;

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
        record.lock().unwrap().push((probe.is_ready(), probe.size()));
    }));
    let _subscription = image.pixels().source.subscribe(&wake);
    image.finish(mip_chain(2, 1, &[0, 0, 0, 255, 255, 255, 255, 255]));
    assert_eq!(*seen.lock().unwrap(), [(true, Some([2, 1]))]);
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
