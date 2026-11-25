use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::*;
use crate::{
    Cx, El, Env, FileDialogKind, Image, ImageCache, ImageMenu, Key, KeyEvent, Label, Mods,
    PopupEntry, Runtime, SurfaceFit, TouchPhase, col, el,
};

fn item_summary(menu: &crate::PopupMenu) -> Vec<(&str, &str, bool)> {
    menu.entries()
        .iter()
        .map(|e| match e {
            PopupEntry::Item(i) => (i.id(), i.title(), i.is_enabled()),
            PopupEntry::Separator => ("---", "", false),
            PopupEntry::Submenu { title, .. } => ("submenu", title.as_str(), true),
        })
        .collect()
}

fn ready_image(cache: &ImageCache) -> (Image, Vec<u8>) {
    let rgba: Vec<u8> = (0..12u8).flat_map(|i| [i * 20, 255 - i * 20, 7, 255]).collect();
    let img = cache.rgba(4, 3, rgba.clone());
    img.wait().unwrap();
    (img, rgba)
}

#[test]
fn build_context_menu_covers_image_combinations_and_linked_images() {
    let _guard = crate::labels::test_lock();

    // 1. Image alone, ready with save + address: Save Image As…, Copy Image, Copy Image Address.
    let m = build_context_menu(ContextMenuTarget {
        has_link: false,
        has_on_link: false,
        image: Some(ImageMenuTarget { has_save: true, ready: true, copy_enabled: true, has_address: true }),
        selectable: false,
        copy_enabled: false,
    }).unwrap();
    assert_eq!(item_summary(&m), [
        ("save-image-as", "Save Image As…", true),
        ("copy-image", "Copy Image", true),
        ("copy-image-address", "Copy Image Address", true),
    ]);

    // 2. Broken / pending image: Save and Copy Image disabled, Copy Image Address still enabled.
    let m = build_context_menu(ContextMenuTarget {
        has_link: false,
        has_on_link: false,
        image: Some(ImageMenuTarget { has_save: true, ready: false, copy_enabled: false, has_address: true }),
        selectable: false,
        copy_enabled: false,
    }).unwrap();
    assert_eq!(item_summary(&m), [
        ("save-image-as", "Save Image As…", false),
        ("copy-image", "Copy Image", false),
        ("copy-image-address", "Copy Image Address", true),
    ]);

    // 3. Ready image exceeding 64 MP: Save enabled, Copy Image disabled, Address enabled.
    let m = build_context_menu(ContextMenuTarget {
        has_link: false,
        has_on_link: false,
        image: Some(ImageMenuTarget { has_save: true, ready: true, copy_enabled: false, has_address: true }),
        selectable: false,
        copy_enabled: false,
    }).unwrap();
    assert_eq!(item_summary(&m), [
        ("save-image-as", "Save Image As…", true),
        ("copy-image", "Copy Image", false),
        ("copy-image-address", "Copy Image Address", true),
    ]);

    // 4. wasm32 (has_save = false) omits Save Image As… even when image is ready.
    let m = build_context_menu(ContextMenuTarget {
        has_link: false,
        has_on_link: false,
        image: Some(ImageMenuTarget { has_save: false, ready: true, copy_enabled: true, has_address: true }),
        selectable: false,
        copy_enabled: false,
    }).unwrap();
    assert_eq!(item_summary(&m), [
        ("copy-image", "Copy Image", true),
        ("copy-image-address", "Copy Image Address", true),
    ]);

    // 5. Image with no save and no address: Copy Image only.
    let m = build_context_menu(ContextMenuTarget {
        has_link: false,
        has_on_link: false,
        image: Some(ImageMenuTarget { has_save: false, ready: true, copy_enabled: true, has_address: false }),
        selectable: false,
        copy_enabled: false,
    }).unwrap();
    assert_eq!(item_summary(&m), [("copy-image", "Copy Image", true)]);

    // 6. Image inside a link (linked image): link items, separator, image items.
    let m = build_context_menu(ContextMenuTarget {
        has_link: true,
        has_on_link: true,
        image: Some(ImageMenuTarget { has_save: true, ready: true, copy_enabled: true, has_address: true }),
        selectable: false,
        copy_enabled: false,
    }).unwrap();
    assert_eq!(item_summary(&m), [
        ("open-link", "Open Link", true),
        ("copy-link", "Copy Link", true),
        ("---", "", false),
        ("save-image-as", "Save Image As…", true),
        ("copy-image", "Copy Image", true),
        ("copy-image-address", "Copy Image Address", true),
    ]);

    // 7. Link + image + selectable text: link items, separator, image items, separator, Copy.
    let m = build_context_menu(ContextMenuTarget {
        has_link: true,
        has_on_link: false,
        image: Some(ImageMenuTarget { has_save: false, ready: true, copy_enabled: true, has_address: false }),
        selectable: true,
        copy_enabled: true,
    }).unwrap();
    assert_eq!(item_summary(&m), [
        ("copy-link", "Copy Link", true),
        ("---", "", false),
        ("copy-image", "Copy Image", true),
        ("---", "", false),
        ("copy", "Copy", true),
    ]);
}

#[test]
fn filename_sanitization_and_magic_byte_inference_cover_all_formats() {
    // Magic bytes for every supported image format.
    assert_eq!(infer_image_ext(b"\x89PNG\r\n\x1a\nrest"), "png");
    assert_eq!(infer_image_ext(b"\xFF\xD8\xFF\xE0"), "jpg");
    assert_eq!(infer_image_ext(b"RIFF\x00\x00\x00\x00WEBPVP8X"), "webp");
    assert_eq!(infer_image_ext(b"GIF87a..."), "gif");
    assert_eq!(infer_image_ext(b"GIF89a..."), "gif");
    assert_eq!(infer_image_ext(b"\x00\x00\x00\x1cftypavif\x00\x00"), "avif");
    assert_eq!(infer_image_ext(b"\x00\x00\x00\x1cftypavis\x00\x00"), "avif");
    assert_eq!(infer_image_ext(b"\x1F\x8B\x08\x00"), "svgz");
    assert_eq!(infer_image_ext(b"  <svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"), "svg");
    assert_eq!(infer_image_ext(b"<?xml version=\"1.0\"?><svg></svg>"), "svg");
    assert_eq!(infer_image_ext(b"BM\x00\x00"), "bmp");
    assert_eq!(infer_image_ext(b"\x00\x00\x01\x00\x01\x00"), "ico");
    assert_eq!(infer_image_ext(b"icns\x00\x00\x00\x08"), "icns");
    assert_eq!(infer_image_ext(b"unknown"), "png");

    // Sanitization strips path traversal, separators, and control chars.
    assert_eq!(sanitize_file_name("../../etc/chart.svg").as_deref(), Some("chart.svg"));
    assert_eq!(sanitize_file_name("C:\\tmp\\photo.webp").as_deref(), Some("photo.webp"));
    assert_eq!(sanitize_file_name(".."), None);
    assert_eq!(sanitize_file_name("a..b.png"), None);
    assert_eq!(sanitize_file_name("   ...  "), None);
    assert_eq!(sanitize_file_name("hi\x00there.png").as_deref(), Some("hithere.png"));

    // Priority: file_name > save_path > URL with known image ext > magic bytes.
    let svg_bytes = SaveSource::Bytes(Arc::from(&b"<svg></svg>"[..]));
    let gif_bytes = SaveSource::Bytes(Arc::from(&b"GIF89a..."[..]));
    let path_src = SaveSource::Path(PathBuf::from("/assets/anim.webp"));
    assert_eq!(suggested_file_name(Some("custom.svg"), Some(&path_src), Some("https://example.com/x.png")), ("custom.svg".into(), "svg"));
    assert_eq!(suggested_file_name(None, Some(&path_src), Some("https://example.com/x.png")), ("anim.webp".into(), "webp"));
    assert_eq!(suggested_file_name(None, Some(&svg_bytes), Some("https://example.com/dir/pic.avif?v=2#top")), ("pic.avif".into(), "avif"));
    assert_eq!(suggested_file_name(None, Some(&svg_bytes), Some("https://example.com/api/image/42")), ("image.svg".into(), "svg"));
    assert_eq!(suggested_file_name(None, Some(&gif_bytes), None), ("image.gif".into(), "gif"));
    assert_eq!(save_filter("svg"), ("SVG Image", &["svg"][..]));
    assert_eq!(save_filter("jpg"), ("JPEG Image", &["jpg", "jpeg"][..]));

    // save_bytes and save_path clear each other, and target respects can_save (wasm32 vs native).
    let cache = ImageCache::new(Arc::new(|| {}));
    let (img, _) = ready_image(&cache);
    let m = ImageMenu::new(&img).save_bytes(b"GIF89a".as_slice()).save_path("/a/b.gif").address("javascript:alert(1)");
    assert_eq!(m.save, Some(SaveSource::Path(PathBuf::from("/a/b.gif"))));
    assert_eq!(m.valid_address(), None, "disallowed URL scheme rejected for Copy Image Address");
    assert!(m.target(true).has_save);
    assert!(!m.target(false).has_save, "wasm32 omits Save Image As…");
    let m2 = m.save_bytes(b"<svg/>".as_slice()).address("https://example.com/a.svg");
    assert!(matches!(m2.save, Some(SaveSource::Bytes(_))));
    assert_eq!(m2.valid_address(), Some("https://example.com/a.svg"));

    // Refusing a non-regular path (directory) without writing to disk.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let err = write_saved_image_sync(&SaveSource::Path(std::env::temp_dir()), &std::env::temp_dir().join("rune-unused.png")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }
}

struct Scene {
    image: Image,
    menu: Option<ImageMenu>,
    wrap_href: Option<String>,
    focusable: bool,
    disabled: bool,
    inert: bool,
    self_secondary: bool,
    ancestor_secondary: bool,
    self_hits: usize,
    ancestor_hits: usize,
}

fn scene_view(s: &Scene, _: &mut Cx<Scene>) -> El {
    let mut pic = el()
        .key("photo")
        .size(160.0, 100.0)
        .role("image")
        .label("Photo")
        .pixels_fit(s.image.pixels(), SurfaceFit::Fit)
        .disabled(s.disabled);
    if s.focusable {
        pic = pic.focusable(true);
    }
    if let Some(m) = &s.menu {
        pic = pic.image_menu(m.clone());
    }
    if s.self_secondary {
        pic = pic.on_secondary_press(|s: &mut Scene, _, _| s.self_hits += 1);
    }
    let inner = match &s.wrap_href {
        Some(url) => el().key("link-wrap").size(160.0, 100.0).href(url.clone()).child(pic),
        None => pic,
    };
    let mut root = col().key("card").size(240.0, 160.0).padding(16.0).child(inner);
    if s.inert {
        root = root.inert();
    }
    if s.ancestor_secondary {
        root = root.on_secondary_press(|s: &mut Scene, _, _| s.ancestor_hits += 1);
    }
    root
}

#[test]
fn runtime_image_menu_copy_save_address_linked_and_overrides() {
    let _label_guard = crate::labels::test_lock();
    let clip = crate::clipboard::memory();
    let cache = ImageCache::new(Arc::new(|| {}));
    let (img, rgba) = ready_image(&cache);
    let raw_svg: Arc<[u8]> = Arc::from(&b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"[..]);

    let saved: Arc<Mutex<Vec<(SaveSource, PathBuf)>>> = Arc::new(Mutex::new(Vec::new()));
    let saved_clone = saved.clone();
    let _save_guard = intercept_saves(move |src, dest| saved_clone.lock().unwrap().push((src, dest)));

    let menu = ImageMenu::new(&img)
        .save_bytes(raw_svg.clone())
        .address("https://example.com/assets/chart.svg");
    let mut rt = Runtime::new(
        Scene {
            image: img.clone(),
            menu: Some(menu),
            wrap_href: None,
            focusable: false,
            disabled: false,
            inert: false,
            self_secondary: false,
            ancestor_secondary: true,
            self_hits: 0,
            ancestor_hits: 0,
        },
        scene_view,
        (240.0, 160.0),
        Env::new(true, 1.0),
    );
    rt.use_clipboard(&clip);
    rt.frame(0.0);

    // ImageMenu does not make the element implicitly focusable.
    let photo = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("photo")).unwrap();
    assert!(!photo.focusable);
    let (x, y) = (photo.rect[0] + 20.0, photo.rect[1] + 20.0);

    // 1. Right-click opens image menu (wins over ancestor on_secondary_press) and claims "app" on web.
    rt.secondary_down(x, y);
    assert_eq!(rt.core().secondary_claim(), "app");
    rt.secondary_up(x, y);
    rt.frame(0.0);
    assert_eq!(rt.state.ancestor_hits, 0);
    assert_eq!(item_summary(rt.popup_menu().expect("image menu")), [
        ("save-image-as", "Save Image As…", true),
        ("copy-image", "Copy Image", true),
        ("copy-image-address", "Copy Image Address", true),
    ]);

    // Copy Image copies the decoded image as PNG via off-thread copy_image path.
    rt.resolve_popup(Some("copy-image")).unwrap();
    rt.frame(0.0);
    let copied = clip.content().unwrap().image().cloned().expect("copied PNG");
    assert_eq!(copied.size(), (4, 3));
    assert_eq!(crate::image::png_rgba8(copied.png()).unwrap().into_raw(), rgba);
    assert!(!rt.frame(1.0 / 60.0), "idle after copy-image");

    // 2. Copy Image Address writes the validated URL to clipboard.
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    rt.frame(0.0);
    rt.resolve_popup(Some("copy-image-address")).unwrap();
    rt.frame(0.0);
    assert_eq!(clip.text().as_deref().map(String::as_str), Some("https://example.com/assets/chart.svg"));

    // 3. Save Image As… opens scripted Save dialog with inferred filename/filter and writes verbatim bytes.
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    rt.frame(0.0);
    rt.resolve_popup(Some("save-image-as")).unwrap();
    rt.frame(0.0);
    let (kind, opts) = rt.file_dialog().expect("save dialog open");
    assert_eq!(kind, FileDialogKind::Save);
    assert_eq!(opts.file_name.as_deref(), Some("chart.svg"));
    assert_eq!(opts.filters, [("SVG Image".to_owned(), vec!["svg".to_owned()])]);
    rt.resolve_file_dialog(Some(PathBuf::from("/exports/chart.svg"))).unwrap();
    rt.frame(0.0);
    assert_eq!(saved.lock().unwrap().as_slice(), [(SaveSource::Bytes(raw_svg.clone()), PathBuf::from("/exports/chart.svg"))]);

    // Cancelling the Save dialog writes nothing.
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    rt.frame(0.0);
    rt.resolve_popup(Some("save-image-as")).unwrap();
    rt.frame(0.0);
    rt.resolve_file_dialog(None).unwrap();
    rt.frame(0.0);
    assert_eq!(saved.lock().unwrap().len(), 1);

    // 4. Copy Image failure leaves clipboard untouched and announces the error (not silent).
    clip.set_text("keep-me");
    clip.fail_writes(Some("clipboard busy"));
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    rt.frame(0.0);
    rt.resolve_popup(Some("copy-image")).unwrap();
    rt.frame(0.0);
    rt.frame(0.0);
    assert_eq!(clip.text().as_deref().map(String::as_str), Some("keep-me"), "failed copy leaves clipboard untouched");
    let tree = rt.accessibility_tree();
    assert!(
        tree.nodes.iter().any(|(_, n)| {
            n.live() == Some(accesskit::Live::Assertive)
                && n.label().is_some_and(|v| v.contains("Couldn't copy image"))
        }),
        "copy image failure must be announced rather than silent",
    );
    clip.fail_writes(None);

    // 5. Image inside El::href link shows link items, separator, then image items.
    rt.update(|s, tx| { s.wrap_href = Some("https://example.com/post".into()); tx.instant(); });
    rt.frame(0.0);
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    rt.frame(0.0);
    assert_eq!(item_summary(rt.popup_menu().expect("linked image menu")), [
        ("open-link", "Open Link", true),
        ("copy-link", "Copy Link", true),
        ("---", "", false),
        ("save-image-as", "Save Image As…", true),
        ("copy-image", "Copy Image", true),
        ("copy-image-address", "Copy Image Address", true),
    ]);
    rt.resolve_popup(Some("open-link")).unwrap();
    rt.frame(0.0);
    assert_eq!(rt.take_url_requests(), ["https://example.com/post"]);

    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    rt.frame(0.0);
    rt.resolve_popup(Some("copy-link")).unwrap();
    rt.frame(0.0);
    assert_eq!(clip.text().as_deref().map(String::as_str), Some("https://example.com/post"));

    // 6. Element's own on_secondary_press always wins over El::image_menu.
    rt.update(|s, tx| { s.wrap_href = None; s.self_secondary = true; tx.instant(); });
    rt.frame(0.0);
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    rt.frame(0.0);
    assert_eq!(rt.state.self_hits, 1);
    assert!(rt.popup_menu().is_none());

    // 7. Plain image without image_menu gets no default image menu (falls through to ancestor).
    rt.update(|s, _| { s.self_secondary = false; s.menu = None; });
    rt.frame(0.0);
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    rt.frame(0.0);
    assert_eq!(rt.state.ancestor_hits, 1);
    assert!(rt.popup_menu().is_none());
}

#[test]
fn broken_image_disabled_inert_keyboard_touch_accesskit_and_labels() {
    let _label_guard = crate::labels::test_lock();
    let clip = crate::clipboard::memory();
    let cache = ImageCache::new(Arc::new(|| {}));
    let bad = cache.load(b"not an image".as_slice());
    assert!(bad.wait().is_err());

    let menu = ImageMenu::new(&bad)
        .save_path("/tmp/broken.png")
        .address("https://example.com/broken.png");
    let mut rt = Runtime::new(
        Scene {
            image: bad,
            menu: Some(menu),
            wrap_href: None,
            focusable: true,
            disabled: false,
            inert: false,
            self_secondary: false,
            ancestor_secondary: false,
            self_hits: 0,
            ancestor_hits: 0,
        },
        scene_view,
        (240.0, 160.0),
        Env::new(true, 1.0),
    );
    rt.use_clipboard(&clip);
    rt.frame(0.0);

    let photo = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("photo")).unwrap();
    let (x, y) = (photo.rect[0] + 20.0, photo.rect[1] + 20.0);

    // 1. Broken image: Save and Copy Image disabled, Copy Image Address still enabled.
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    rt.frame(0.0);
    assert_eq!(item_summary(rt.popup_menu().expect("broken image menu")), [
        ("save-image-as", "Save Image As…", false),
        ("copy-image", "Copy Image", false),
        ("copy-image-address", "Copy Image Address", true),
    ]);
    rt.resolve_popup(Some("copy-image-address")).unwrap();
    rt.frame(0.0);
    assert_eq!(clip.text().as_deref().map(String::as_str), Some("https://example.com/broken.png"));

    // 2. Keyboard Menu key and Shift+F10 on focusable image open the menu.
    rt.update(|_, tx| tx.focus("photo", true));
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::ContextMenu, mods: Mods::default() });
    rt.frame(0.0);
    assert!(rt.popup_menu().is_some(), "ContextMenu key opens image menu");
    rt.resolve_popup(None).unwrap();
    rt.frame(0.0);

    rt.key(KeyEvent { key: Key::Function(10), mods: Mods { shift: true, ..Mods::default() } });
    rt.frame(0.0);
    assert!(rt.popup_menu().is_some(), "Shift+F10 opens image menu");
    rt.resolve_popup(None).unwrap();
    rt.frame(0.0);

    // 3. Touch long-press opens the image menu.
    rt.touch(1, TouchPhase::Started, x, y, 0.0);
    rt.frame(0.0);
    rt.frame(0.6);
    assert!(rt.popup_menu().is_some(), "touch long-press opens image menu");
    rt.resolve_popup(None).unwrap();
    rt.touch(1, TouchPhase::Ended, x, y, 0.6);
    rt.frame(0.0);

    // 4. AccessKit advertises ShowContextMenu and dispatches it.
    let tree = rt.accessibility_tree();
    let node = tree.nodes.iter().find(|(_, n)| n.label() == Some("Photo")).expect("Photo a11y node");
    assert!(node.1.supports_action(accesskit::Action::ShowContextMenu));
    let req = accesskit::ActionRequest {
        action: accesskit::Action::ShowContextMenu,
        target_tree: accesskit::TreeId::ROOT,
        target_node: node.0,
        data: None,
    };
    assert!(rt.core_mut().a11y_action(&req));
    rt.frame(0.0);
    assert!(rt.popup_menu().is_some(), "AccessKit ShowContextMenu opens image menu");
    rt.resolve_popup(None).unwrap();
    rt.frame(0.0);

    // 5. set_labels localises SaveImageAs, CopyImage, and CopyImageAddress.
    crate::set_labels(|l| match l {
        Label::SaveImageAs => Some("Enregistrer l'image…".into()),
        Label::CopyImage => Some("Copier l'image".into()),
        Label::CopyImageAddress => Some("Copier l'adresse".into()),
        _ => None,
    });
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    rt.frame(0.0);
    assert_eq!(item_summary(rt.popup_menu().expect("localised image menu")), [
        ("save-image-as", "Enregistrer l'image…", false),
        ("copy-image", "Copier l'image", false),
        ("copy-image-address", "Copier l'adresse", true),
    ]);
    rt.resolve_popup(None).unwrap();
    rt.frame(0.0);

    // 6. Disabled and inert elements never open the image menu.
    rt.update(|s, _| s.disabled = true);
    rt.frame(0.0);
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    rt.frame(0.0);
    assert!(rt.popup_menu().is_none(), "disabled image must not open menu");

    rt.update(|s, _| { s.disabled = false; s.inert = true; });
    rt.frame(0.0);
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    rt.frame(0.0);
    assert!(rt.popup_menu().is_none(), "inert image must not open menu");
}

#[test]
fn save_path_active_descendant_64mp_limit_and_linux_popup_fallback() {
    let _label_guard = crate::labels::test_lock();

    // 64 MP boundary matches valid_image_size.
    assert!(crate::clipboard::valid_image_size(8192, 8192));
    assert!(!crate::clipboard::valid_image_size(8193, 8192));
    assert!(!crate::clipboard::valid_image_size(0, 10));

    let clip = crate::clipboard::memory();
    let cache = ImageCache::new(Arc::new(|| {}));
    let (img, rgba) = ready_image(&cache);
    let src_path = PathBuf::from("/assets/animated.webp");

    let saved: Arc<Mutex<Vec<(SaveSource, PathBuf)>>> = Arc::new(Mutex::new(Vec::new()));
    let saved_clone = saved.clone();
    let _save_guard = intercept_saves(move |src, dest| saved_clone.lock().unwrap().push((src, dest)));

    // 1. Active descendant with image_menu opens on ContextMenu key + Save Image As… with save_path.
    struct ListScene {
        image: Image,
        src_path: PathBuf,
    }
    fn list_view(s: &ListScene, _: &mut Cx<ListScene>) -> El {
        col()
            .key("list")
            .size(240.0, 160.0)
            .role("listbox")
            .focusable(true)
            .active_descendant("item-0")
            .child(
                el()
                    .key("item-0")
                    .role("option")
                    .size(160.0, 80.0)
                    .pixels_fit(s.image.pixels(), SurfaceFit::Fit)
                    .image_menu(
                        ImageMenu::new(&s.image)
                            .save_path(s.src_path.clone())
                            .address("https://example.com/animated.webp"),
                    ),
            )
    }
    let mut rt = Runtime::new(
        ListScene { image: img.clone(), src_path: src_path.clone() },
        list_view,
        (240.0, 160.0),
        Env::new(true, 1.0),
    );
    rt.use_clipboard(&clip);
    rt.frame(0.0);
    rt.update(|_, tx| tx.focus("list", true));
    rt.frame(0.0);

    rt.key(KeyEvent { key: Key::ContextMenu, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(item_summary(rt.popup_menu().expect("active descendant image menu")), [
        ("save-image-as", "Save Image As…", true),
        ("copy-image", "Copy Image", true),
        ("copy-image-address", "Copy Image Address", true),
    ]);
    rt.resolve_popup(Some("save-image-as")).unwrap();
    rt.frame(0.0);
    let (kind, opts) = rt.file_dialog().expect("save dialog for save_path");
    assert_eq!(kind, FileDialogKind::Save);
    assert_eq!(opts.file_name.as_deref(), Some("animated.webp"));
    assert_eq!(opts.filters, [("WebP Image".to_owned(), vec!["webp".to_owned()])]);
    rt.resolve_file_dialog(Some(PathBuf::from("/exports/animated.webp"))).unwrap();
    rt.frame(0.0);
    assert_eq!(
        saved.lock().unwrap().as_slice(),
        [(SaveSource::Path(src_path), PathBuf::from("/exports/animated.webp"))],
    );

    // 2. Unified Popups queue delivers the image menu to Linux App::on_popup_menu fallback.
    struct FallbackScene {
        image: Image,
        popup: Option<crate::PopupMenuRequest>,
    }
    fn fallback_view(s: &FallbackScene, _: &mut Cx<FallbackScene>) -> El {
        el()
            .key("pic")
            .size(160.0, 100.0)
            .pixels_fit(s.image.pixels(), SurfaceFit::Fit)
            .image_menu(ImageMenu::new(&s.image).address("https://example.com/fallback.png"))
    }
    fn init_fallback() -> FallbackScene {
        let cache = ImageCache::new(Arc::new(|| {}));
        let (image, _) = ready_image(&cache);
        FallbackScene { image, popup: None }
    }
    let mut fb = crate::App::new(crate::AppSpec::new(
        "image-fallback",
        (200.0, 140.0),
        init_fallback,
        fallback_view,
    ))
    .on_popup_menu(|s: &mut FallbackScene, req: crate::PopupMenuRequest, _| {
        s.popup = Some(req);
    })
    .headless_runtime((200.0, 140.0), Env::new(true, 1.0));
    fb.use_clipboard(&clip);
    fb.core_mut().host.force_popup_menu_fallback = true;
    fb.frame(0.0);

    let (x, y) = fb.find("pic").unwrap();
    fb.secondary_down(x, y);
    fb.secondary_up(x, y);
    fb.frame(0.0);
    let req = fb.state.popup.take().expect("image menu delivered to on_popup_menu");
    assert_eq!(item_summary(req.menu()), [
        ("copy-image", "Copy Image", true),
        ("copy-image-address", "Copy Image Address", true),
    ]);
    fb.update(|_, tx| tx.resolve_popup_menu(Some("copy-image")));
    fb.frame(0.0);
    let copied = clip.content().unwrap().image().cloned().expect("copied PNG via fallback");
    assert_eq!(copied.size(), (4, 3));
    assert_eq!(crate::image::png_rgba8(copied.png()).unwrap().into_raw(), rgba);
}
