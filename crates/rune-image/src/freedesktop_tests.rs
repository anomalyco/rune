use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::{AtomicU64, Ordering}, mpsc};

use crate::{ImageCache, ImageError, freedesktop, themed_icon_rgba};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("rune-fd-{tag}-{}-{id}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &Path { &self.0 }
    fn write(&self, rel: &str, bytes: &[u8]) -> PathBuf {
        let p = self.0.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        p
    }
}

impl Drop for TempDir {
    fn drop(&mut self) { std::fs::remove_dir_all(&self.0).ok(); }
}

fn solid_png(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(w, h, image::Rgba(rgba))
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

const SAMPLE_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><rect x="2" y="2" width="20" height="20" rx="4" fill="#009fff"/></svg>"##;

#[test]
fn parse_index_theme_reads_spec_metadata_defaults_and_rejects_traversal() {
    let text = "\
# comment line\r\n\
[Icon Theme]\r\n\
Name=Demo\r\n\
Inherits=ParentA, ../evil, ParentB\r\n\
Directories=48x48/apps,scalable/mimetypes,32x32/places,../escape,/abs/path,no-size/apps\r\n\
ScaledDirectories=48x48@2/apps,48x48/apps\r\n\
\r\n\
[48x48/apps]\r\n\
Size=48\r\n\
Type=Fixed\r\n\
Context=Applications\r\n\
\r\n\
[48x48@2/apps]\r\n\
Size=48\r\n\
Scale=2\r\n\
Type=Fixed\r\n\
\r\n\
[scalable/mimetypes]\r\n\
Size=128\r\n\
MinSize=16\r\n\
MaxSize=512\r\n\
Type=Scalable\r\n\
Context=MimeTypes\r\n\
\r\n\
[32x32/places]\r\n\
Size=32\r\n\
\r\n\
[no-size/apps]\r\n\
Type=Fixed\r\n";

    let idx = freedesktop::parse_index_theme(text);
    assert_eq!(idx.inherits, vec!["ParentA", "ParentB"]);
    assert_eq!(idx.directories.len(), 4, "invalid/traversal/missing-size dirs are skipped and duplicates deduplicated");

    let fixed = &idx.directories[0];
    assert_eq!(fixed.rel_path, "48x48/apps");
    assert_eq!(fixed.dir_type, freedesktop::DirType::Fixed);
    assert_eq!((fixed.size, fixed.scale), (48, 1));
    assert_eq!(fixed.context.as_deref(), Some("Applications"));
    assert!(fixed.matches_size(48, 1));
    assert!(!fixed.matches_size(49, 1));
    assert!(!fixed.matches_size(48, 2));

    let scalable = &idx.directories[1];
    assert_eq!(scalable.rel_path, "scalable/mimetypes");
    assert_eq!(scalable.dir_type, freedesktop::DirType::Scalable);
    assert_eq!((scalable.min_size, scalable.max_size), (16, 512));
    assert!(scalable.matches_size(24, 1));
    assert!(!scalable.matches_size(12, 1));
    assert_eq!(scalable.size_distance(12, 1), 4);
    assert_eq!(scalable.size_distance(48, 2), 0, "96 px sits inside 16..=512");

    let thresh = &idx.directories[2];
    assert_eq!(thresh.rel_path, "32x32/places");
    assert_eq!(thresh.dir_type, freedesktop::DirType::Threshold, "spec default Type is Threshold");
    assert_eq!(thresh.threshold, 2, "spec default Threshold is 2");
    assert!(thresh.matches_size(30, 1));
    assert!(thresh.matches_size(34, 1));
    assert!(!thresh.matches_size(35, 1));
    assert_eq!(thresh.size_distance(35, 1), 1);

    let hidpi = &idx.directories[3];
    assert_eq!(hidpi.rel_path, "48x48@2/apps");
    assert_eq!((hidpi.size, hidpi.scale), (48, 2));
    assert!(hidpi.matches_size(48, 2));
    assert!(!hidpi.matches_size(48, 1));
    assert_eq!(hidpi.size_distance(90, 1), 6);
}

#[test]
fn selection_honors_fixed_threshold_scalable_scale_and_downscale_tie_break() {
    let tmp = TempDir::new("select");
    let share = tmp.path().join("share");
    tmp.write(
        "share/icons/SpecTheme/index.theme",
        b"[Icon Theme]\nName=SpecTheme\nDirectories=32x32/apps,48x48/apps,64x64/apps,48x48@2/apps,scalable/apps\n\
[32x32/apps]\nSize=32\nType=Fixed\n\
[48x48/apps]\nSize=48\nType=Threshold\nThreshold=4\n\
[64x64/apps]\nSize=64\nType=Fixed\n\
[48x48@2/apps]\nSize=48\nScale=2\nType=Fixed\n\
[scalable/apps]\nSize=128\nMinSize=128\nMaxSize=512\nType=Scalable\n",
    );
    let p32 = tmp.write("share/icons/SpecTheme/32x32/apps/app.png", b"p32");
    let p48 = tmp.write("share/icons/SpecTheme/48x48/apps/app.png", b"p48");
    let p64 = tmp.write("share/icons/SpecTheme/64x64/apps/app.png", b"p64");
    let p48_2x = tmp.write("share/icons/SpecTheme/48x48@2/apps/app.png", b"p48_2x");
    let svg = tmp.write("share/icons/SpecTheme/scalable/apps/app.svg", SAMPLE_SVG);
    // Icon with only 32 and 64 to test tie-breaking at 48.
    tmp.write("share/icons/SpecTheme/32x32/apps/tie.png", b"t32");
    let tie64 = tmp.write("share/icons/SpecTheme/64x64/apps/tie.png", b"t64");

    let icon_dirs = freedesktop::icon_dirs_from(&[share.clone()], None);
    let pixmap_dirs = freedesktop::pixmap_dirs_from(&[share]);
    let cache = freedesktop::ThemeCache::new(16, 1024 * 1024);
    let find = |name, size, scale| {
        freedesktop::find_icon_in_dirs(&[name], Some("SpecTheme"), size, scale, &icon_dirs, &pixmap_dirs, &cache)
    };

    assert_eq!(find("app", 32, 1), Some(p32));
    assert_eq!(find("app", 45, 1), Some(p48.clone()), "within Threshold 48 +/- 4");
    assert_eq!(find("app", 52, 1), Some(p48), "upper bound of Threshold 48 +/- 4");
    assert_eq!(find("app", 64, 1), Some(p64));
    assert_eq!(find("app", 48, 2), Some(p48_2x), "exact Scale=2 match");
    assert_eq!(find("app", 256, 1), Some(svg.clone()), "within Scalable MinSize..=MaxSize");
    assert_eq!(find("app", 128, 2), Some(svg), "256 effective px hits Scalable with distance 0");
    assert_eq!(find("tie", 48, 1), Some(tie64), "equal distance 16 prefers covering 64 over upscaling 32");
}

#[test]
fn inherits_walks_breadth_first_defers_hicolor_and_survives_cycles() {
    let tmp = TempDir::new("inherits");
    let share = tmp.path().join("share");
    let make_theme = |name: &str, inherits: &str| {
        tmp.write(
            &format!("share/icons/{name}/index.theme"),
            format!("[Icon Theme]\nName={name}\nInherits={inherits}\nDirectories=48x48/apps\n[48x48/apps]\nSize=48\nType=Fixed\n").as_bytes(),
        );
    };
    make_theme("Child", "ParentA,ParentB");
    make_theme("ParentA", "GrandA,hicolor");
    make_theme("ParentB", "Child"); // cycle back to Child
    make_theme("GrandA", "ParentA"); // cycle back to ParentA
    make_theme("hicolor", "");

    let icon_dirs = freedesktop::icon_dirs_from(&[share.clone()], None);
    let pixmap_dirs = freedesktop::pixmap_dirs_from(&[share]);
    let cache = freedesktop::ThemeCache::new(16, 1024 * 1024);

    let chain_names: Vec<String> = freedesktop::theme_chain("Child", &icon_dirs, &cache)
        .into_iter().map(|(n, _)| n).collect();
    assert_eq!(chain_names, vec!["Child", "ParentA", "ParentB", "GrandA", "hicolor"]);

    // Direct parent ParentB must beat grandparent GrandA (breadth-first order).
    let in_parent_b = tmp.write("share/icons/ParentB/48x48/apps/bfs.png", b"pb");
    tmp.write("share/icons/GrandA/48x48/apps/bfs.png", b"ga");
    assert_eq!(
        freedesktop::find_icon_in_dirs(&["bfs"], Some("Child"), 48, 1, &icon_dirs, &pixmap_dirs, &cache),
        Some(in_parent_b),
    );

    // Icon only in hicolor is found; icon only in pixmaps is found after hicolor.
    let in_hicolor = tmp.write("share/icons/hicolor/48x48/apps/only-hi.png", b"hi");
    let in_pixmaps = tmp.write("share/pixmaps/only-pix.png", b"px");
    assert_eq!(
        freedesktop::find_icon_in_dirs(&["only-hi"], Some("Child"), 48, 1, &icon_dirs, &pixmap_dirs, &cache),
        Some(in_hicolor),
    );
    assert_eq!(
        freedesktop::find_icon_in_dirs(&["only-pix"], Some("Child"), 48, 1, &icon_dirs, &pixmap_dirs, &cache),
        Some(in_pixmaps),
    );
}

#[test]
fn fallback_names_search_across_contexts_and_prefer_specific_hicolor_over_generic_active() {
    let tmp = TempDir::new("fallback");
    let share = tmp.path().join("share");
    tmp.write(
        "share/icons/Active/index.theme",
        b"[Icon Theme]\nName=Active\nInherits=hicolor\nDirectories=48x48/apps,48x48/mimetypes\n\
[48x48/apps]\nSize=48\nType=Fixed\nContext=Applications\n\
[48x48/mimetypes]\nSize=48\nType=Fixed\nContext=MimeTypes\n",
    );
    tmp.write(
        "share/icons/hicolor/index.theme",
        b"[Icon Theme]\nName=Hicolor\nDirectories=48x48/mimetypes\n[48x48/mimetypes]\nSize=48\nType=Fixed\n",
    );
    let active_plain = tmp.write("share/icons/Active/48x48/mimetypes/text-plain.png", b"plain");
    let hicolor_python = tmp.write("share/icons/hicolor/48x48/mimetypes/text-x-python.png", b"py");
    let app_icon = tmp.write("share/icons/Active/48x48/apps/custom-app.png", b"app");

    let icon_dirs = freedesktop::icon_dirs_from(&[share.clone()], None);
    let pixmap_dirs = freedesktop::pixmap_dirs_from(&[share]);
    let cache = freedesktop::ThemeCache::new(16, 1024 * 1024);

    assert_eq!(
        freedesktop::find_icon_in_dirs(&["missing-mime", "text-x-python", "text-plain"], Some("Active"), 48, 1, &icon_dirs, &pixmap_dirs, &cache),
        Some(hicolor_python),
        "specific name in hicolor beats generic fallback name in Active",
    );
    assert_eq!(
        freedesktop::find_icon_in_dirs(&["missing-mime", "text-plain"], Some("Active"), 48, 1, &icon_dirs, &pixmap_dirs, &cache),
        Some(active_plain),
    );
    assert_eq!(
        freedesktop::find_icon_in_dirs(&["custom-app"], Some("Active"), 48, 1, &icon_dirs, &pixmap_dirs, &cache),
        Some(app_icon),
    );
    assert_eq!(
        freedesktop::find_icon_in_dirs(&[], Some("Active"), 48, 1, &icon_dirs, &pixmap_dirs, &cache),
        None,
    );
}

#[test]
fn detect_desktop_theme_reads_gtk_and_kde_configs_by_desktop_preference() {
    let tmp = TempDir::new("detect");
    let cfg = tmp.path().join("config");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::create_dir_all(&home).unwrap();

    assert_eq!(freedesktop::detect_desktop_theme_from(Some(&cfg), Some(&home), None), None);

    tmp.write("home/.gtkrc-2.0", b"gtk-icon-theme-name=\"Humanity\"\n");
    assert_eq!(
        freedesktop::detect_desktop_theme_from(Some(&cfg), Some(&home), Some("GNOME")),
        Some("Humanity".to_owned()),
    );

    tmp.write("config/gtk-3.0/settings.ini", b"[Settings]\ngtk-icon-theme-name = Papirus\n");
    assert_eq!(
        freedesktop::detect_desktop_theme_from(Some(&cfg), Some(&home), Some("GNOME")),
        Some("Papirus".to_owned()),
    );

    tmp.write("config/gtk-4.0/settings.ini", b"[Settings]\ngtk-icon-theme-name = 'Yaru'\n");
    tmp.write("config/kdeglobals", b"[General]\nColorScheme=Breeze\n[Icons]\nTheme=breeze-dark\n");
    assert_eq!(
        freedesktop::detect_desktop_theme_from(Some(&cfg), Some(&home), Some("ubuntu:GNOME")),
        Some("Yaru".to_owned()),
        "GTK 4 settings.ini wins on GNOME",
    );
    assert_eq!(
        freedesktop::detect_desktop_theme_from(Some(&cfg), Some(&home), Some("KDE")),
        Some("breeze-dark".to_owned()),
        "kdeglobals wins on KDE",
    );
}

#[test]
fn themed_icon_decodes_png_svg_distinguishes_not_found_from_corrupt_and_deduplicates() {
    let tmp = TempDir::new("decode");
    let share = tmp.path().join("share");
    tmp.write(
        "share/icons/Fixture/index.theme",
        b"[Icon Theme]\nName=Fixture\nInherits=hicolor\nDirectories=48x48/apps,scalable/mimetypes\n\
[48x48/apps]\nSize=48\nType=Fixed\n\
[scalable/mimetypes]\nSize=48\nMinSize=16\nMaxSize=512\nType=Scalable\n",
    );
    let png = solid_png(48, 48, [32, 160, 240, 255]);
    tmp.write("share/icons/Fixture/48x48/apps/fixture-app.png", &png);
    tmp.write("share/icons/Fixture/48x48/apps/corrupt-app.png", b"not a valid png");
    tmp.write("share/icons/Fixture/scalable/mimetypes/text-x-rust.svg", SAMPLE_SVG);
    let desktop = tmp.write(
        "share/applications/fixture.desktop",
        b"[Desktop Entry]\nType=Application\nName=Fixture\nExec=true\nIcon=fixture-app\n",
    );

    // Desktop entry resolves inside active Fixture theme (issue #21 repro).
    assert_eq!(
        freedesktop::icon_file_scaled(&desktop, 48, 1, Some("Fixture"), &[share.clone()], None),
        Some(share.join("icons/Fixture/48x48/apps/fixture-app.png")),
    );

    // Synchronous themed_icon_rgba_in validation and PNG decode.
    assert!(themed_icon_rgba(&["fixture-app"], Some("Fixture"), 0, 1).is_err());
    assert!(themed_icon_rgba(&["fixture-app"], Some("Fixture"), 1025, 1).is_err());
    assert!(themed_icon_rgba(&["fixture-app"], Some("Fixture"), 48, 0).is_err());
    let (w, h, rgba) = freedesktop::themed_icon_rgba_in(&["fixture-app"], Some("Fixture"), 48, 1, &[share.clone()], None)
        .unwrap().expect("fixture-app resolves");
    assert_eq!((w, h), (48, 48));
    assert_eq!(&rgba[..4], &[32, 160, 240, 255]);
    assert_eq!(
        freedesktop::themed_icon_rgba_in(&["does-not-exist"], Some("Fixture"), 48, 1, &[share.clone()], None),
        Ok(None),
    );
    assert!(
        freedesktop::themed_icon_rgba_in(&["corrupt-app"], Some("Fixture"), 48, 1, &[share.clone()], None).is_err(),
    );

    // Off-thread ImageCache::themed_icon_in pipeline.
    let (tx, rx) = mpsc::channel();
    let caller = std::thread::current().id();
    let cache = ImageCache::new(Arc::new(move || { tx.send(std::thread::current().id()).ok(); }));

    let ok_img = cache.themed_icon_in(&["fixture-app"], Some("Fixture"), 48.0, 1.0, vec![share.clone()]);
    let same_img = cache.themed_icon_in(&["fixture-app"], Some("Fixture"), 48.0, 1.0, vec![share.clone()]);
    let diff_scale = cache.themed_icon_in(&["fixture-app"], Some("Fixture"), 48.0, 2.0, vec![share.clone()]);
    assert!(Arc::ptr_eq(&ok_img.0, &same_img.0), "identical themed icon requests share a handle");
    assert!(!Arc::ptr_eq(&ok_img.0, &diff_scale.0), "different scale gets its own handle");
    assert_eq!(ok_img.wait(), Ok([48, 48]));
    let worker = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert_ne!(worker, caller, "themed icon lookup/decode runs off the calling thread");
    assert!(!ok_img.is_not_found());
    assert_eq!(ok_img.error(), None);

    #[cfg(feature = "svg")]
    {
        let svg_img = cache.themed_icon_in(&["missing-first", "text-x-rust"], Some("Fixture"), 48.0, 2.0, vec![share.clone()]);
        assert_eq!(svg_img.wait(), Ok([96, 96]), "scalable SVG rasterizes at size_pt * scale");
        assert_eq!(svg_img.intrinsic_size(), Some([24.0, 24.0]));
        assert!(!svg_img.is_not_found());
        let pixels = &svg_img.decoded().unwrap().unwrap().mips[0].pixels;
        assert!(pixels.chunks_exact(4).any(|p| p[3] > 0), "rasterized SVG has visible pixels");
    }

    let missing = cache.themed_icon_in(&["no-such-icon", "also-missing"], Some("Fixture"), 48.0, 1.0, vec![share.clone()]);
    assert!(missing.wait().is_err());
    assert!(missing.is_ready());
    assert!(missing.error().is_some());
    assert!(missing.is_not_found(), "missing themed icon sets is_not_found()");

    let corrupt = cache.themed_icon_in(&["corrupt-app"], Some("Fixture"), 48.0, 1.0, vec![share.clone()]);
    assert!(corrupt.wait().is_err());
    assert!(corrupt.is_ready());
    assert!(corrupt.error().is_some());
    assert!(!corrupt.is_not_found(), "corrupt icon file must NOT set is_not_found()");

    // Image::is_not_found() also separates missing vs corrupt on ImageCache::file.
    let missing_file = cache.file(share.join("missing.png"), 32.0, 1.0);
    assert!(missing_file.wait().is_err());
    assert!(missing_file.is_not_found());
    let corrupt_file = cache.file(share.join("icons/Fixture/48x48/apps/corrupt-app.png"), 32.0, 1.0);
    assert!(corrupt_file.wait().is_err());
    assert!(!corrupt_file.is_not_found());

    // Non-finite and zero sizes clamp safely without panicking.
    let edge = cache.themed_icon(&[], None, f32::NAN, 0.0);
    assert!(edge.wait().is_err());
    assert!(edge.is_not_found());
    // ImageError remains a single-field tuple struct.
    let e: ImageError = ImageError("example".into());
    assert_eq!(e.0, "example");
}

#[test]
fn theme_cache_bounds_entries_and_bytes_with_lru_eviction_and_reuses_parsed_index() {
    let tmp = TempDir::new("cache");
    let share = tmp.path().join("share");
    let icon_dirs = freedesktop::icon_dirs_from(&[share.clone()], None);
    let pixmap_dirs = freedesktop::pixmap_dirs_from(&[share]);

    // Entry cap (64 themes): load 80 distinct themes and keep theme-00 hot.
    let cache = freedesktop::ThemeCache::new(freedesktop::MAX_CACHED_THEMES, freedesktop::MAX_CACHE_BYTES);
    for i in 0..80usize {
        let name = format!("theme-{i:02}");
        tmp.write(
            &format!("share/icons/{name}/index.theme"),
            format!("[Icon Theme]\nName={name}\nDirectories=48x48/apps\n[48x48/apps]\nSize=48\nType=Fixed\n").as_bytes(),
        );
        let _ = cache.get_or_load(&name, &icon_dirs);
        if i == 63 {
            // Touch theme-00 so theme-01 becomes the LRU candidate instead.
            let _ = cache.get_or_load("theme-00", &icon_dirs);
        }
    }
    assert_eq!(cache.len(), freedesktop::MAX_CACHED_THEMES, "cache never exceeds 64 themes");
    assert!(cache.contains("theme-00", &icon_dirs), "recently touched theme stays cached");
    assert!(!cache.contains("theme-01", &icon_dirs), "least-recently-used theme is evicted");
    assert!(cache.contains("theme-79", &icon_dirs));

    // Verify cached index is reused without re-reading index.theme from disk.
    let icon_path = tmp.write("share/icons/theme-79/48x48/apps/cached-icon.png", b"ok");
    std::fs::remove_file(tmp.path().join("share/icons/theme-79/index.theme")).unwrap();
    assert_eq!(
        freedesktop::find_icon_in_dirs(&["cached-icon"], Some("theme-79"), 48, 1, &icon_dirs, &pixmap_dirs, &cache),
        Some(icon_path),
        "cached theme metadata resolves newly added icons without re-parsing index.theme",
    );

    // Byte cap: small budget evicts older entries so total_bytes never exceeds max_bytes.
    let single_bytes = cache.total_bytes() / cache.len();
    let byte_cap = single_bytes * 3;
    let small = freedesktop::ThemeCache::new(64, byte_cap);
    for i in 0..10usize {
        let name = format!("theme-{i:02}");
        let _ = small.get_or_load(&name, &icon_dirs);
        assert!(small.total_bytes() <= byte_cap, "total_bytes {} exceeded cap {byte_cap}", small.total_bytes());
    }
    assert!(small.len() <= 3, "byte cap bounded entry count to {}", small.len());
}
