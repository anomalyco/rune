#![allow(unsafe_code)]

use super::*;
use crate::platform::macos::test_pasteboard::{self, Board};
use objc2_foundation::NSData;

/// A private pasteboard holding `items`, or None without a pasteboard.
fn board(items: &[&[(&str, &[u8])]]) -> Option<Board> {
    let pb = test_pasteboard::unique()?;
    pb.clearContents();
    let items: Vec<Retained<NSPasteboardItem>> = items.iter().map(|types| {
        let item = NSPasteboardItem::new();
        for (t, data) in *types { item.setData_forType(&NSData::with_bytes(data), &NSString::from_str(t)); }
        item
    }).collect();
    let objects: Vec<Retained<objc2::runtime::ProtocolObject<dyn objc2_app_kit::NSPasteboardWriting>>> =
        items.into_iter().map(|i| objc2::runtime::ProtocolObject::from_retained(i)).collect();
    assert!(pb.writeObjects(&NSArray::from_retained_slice(&objects)));
    Some(pb)
}

fn png() -> Vec<u8> {
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, 2, 1);
    e.set_color(png::ColorType::Rgba);
    e.write_header().unwrap().write_image_data(&[255, 0, 0, 255, 0, 255, 0, 255]).unwrap();
    out
}

fn found(pb: &NSPasteboard) -> Vec<Item> {
    // SAFETY: `pb` is a live `NSPasteboard` created by this test.
    unsafe { items(pb) }.into_iter().map(|(_, i)| i).collect()
}

#[test]
fn a_file_url_is_its_path() {
    let Some(pb) = board(&[&[(FILE_URL, b"file:///tmp/notes%20one.txt")]]) else { return };
    assert_eq!(found(&pb), [Item::File("/tmp/notes one.txt".into())]);
}

#[test]
fn image_data_without_a_file_is_an_image() {
    let Some(pb) = board(&[&[("public.png", &png())], &[("public.url", b"https://e.com/a.png"), ("public.tiff", b"II*\0")]]) else { return };
    assert_eq!(found(&pb), [Item::Image("public.png", "png"), Item::Image("public.tiff", "png")]);
}

#[test]
fn a_file_url_wins_over_the_image_beside_it() {
    let Some(pb) = board(&[&[(FILE_URL, b"file:///tmp/a.png"), ("public.png", &png())]]) else { return };
    assert_eq!(found(&pb), [Item::File("/tmp/a.png".into())]);
}

#[test]
fn a_promise_is_named_for_its_type() {
    let Some(pb) = board(&[&[("com.apple.NSFilePromiseItemMetaData", b"x"), (PROMISED_TYPE, b"public.png")]]) else { return };
    assert_eq!(found(&pb), [Item::Promise(Some("png"))]);
}

#[test]
fn hover_paths_stand_in_for_files_to_come() {
    assert_eq!(Item::File("/tmp/a.txt".into()).hover_path(), PathBuf::from("/tmp/a.txt"));
    assert_eq!(Item::Promise(Some("png")).hover_path(), PathBuf::from("File.png"));
    assert_eq!(Item::Promise(None).hover_path(), PathBuf::from("File"));
    assert_eq!(Item::Image("public.tiff", "png").hover_path(), PathBuf::from("Image.png"));
}

#[test]
fn promised_types_map_to_extensions() {
    assert_eq!(extension("public.jpeg"), Some("jpg"));
    assert_eq!(extension("com.adobe.pdf"), Some("pdf"));
    assert_eq!(extension("public.data"), None);
}

#[test]
fn file_urls_resolve_to_paths() {
    assert_eq!(file_path(&NSString::from_str("file:///tmp/notes%20one.txt")), Some(PathBuf::from("/tmp/notes one.txt")));
    assert_eq!(file_path(&NSString::from_str("https://e.com/a.png")), None);
}

#[test]
fn taken_names_are_numbered() {
    let dir = drop_dir().unwrap();
    assert_eq!(free_name(&dir, "Image", "png"), dir.join("Image.png"));
    std::fs::write(dir.join("Image.png"), b"").unwrap();
    std::fs::write(dir.join("Image 2.png"), b"").unwrap();
    assert_eq!(free_name(&dir, "Image", "png"), dir.join("Image 3.png"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn text_alone_gives_nothing() {
    let Some(pb) = board(&[&[("public.utf8-plain-text", b"hello")], &[("public.url", b"https://e.com/")]]) else { return };
    assert!(found(&pb).is_empty());
}

#[test]
fn parallel_boards_do_not_collide() {
    if !test_pasteboard::available() { return; }
    const THREADS: usize = 8;
    const ROUNDS: usize = 20;
    for round in 0..ROUNDS {
        let start = std::sync::Arc::new(std::sync::Barrier::new(THREADS));
        std::thread::scope(|scope| {
            for slot in 0..THREADS {
                let start = start.clone();
                scope.spawn(move || {
                    let path = format!("/tmp/drop-test-{round}-{slot}.txt");
                    let url = format!("file://{path}");
                    start.wait();
                    let pb = board(&[&[(FILE_URL, url.as_bytes())]]).expect("pasteboard available");
                    std::thread::sleep(std::time::Duration::from_millis(1));
                    assert_eq!(found(&pb), [Item::File(PathBuf::from(path))]);
                });
            }
        });
    }
}

#[test]
fn images_are_written_named_for_their_url() {
    let Some(pb) = board(&[&[("public.png", &png())], &[("public.png", &png())], &[("public.url", b"https://e.com/img/kitten.jpg?w=2"), ("public.png", &png())]]) else { return };
    let dir = drop_dir().unwrap();
    // SAFETY: `pb` is a live `NSPasteboard` created by this test.
    let entries = unsafe { items(&pb) };
    let paths: Vec<PathBuf> = entries.iter().filter_map(|(item, f)| match f {
        Item::Image(k, e) => {
            // SAFETY: `item` is a live `NSPasteboardItem` returned from `items(&pb)`.
            unsafe { write_image(item, k, e, &dir) }
        }
        _ => None,
    }).collect();
    let names: Vec<_> = paths.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["Image.png", "Image 2.png", "kitten.png"]);
    assert_eq!(std::fs::read(&paths[0]).unwrap(), png());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn url_stems() {
    assert_eq!(url_stem("https://e.com/a/cat.photo.png?x=1").as_deref(), Some("cat.photo"));
    assert_eq!(url_stem("https://e.com/"), None);
    assert_eq!(url_stem("https://e.com"), None);
    assert_eq!(url_stem("data:image/png;base64,AAAA"), None);
}

#[test]
fn the_operation_follows_the_source() {
    assert_eq!(operation(1 | 2 | 16), 1);
    assert_eq!(operation(4), 4);
    assert_eq!(operation(0), 0);
    assert_eq!(operation(2), 0);
}

#[test]
fn moves_coalesce_and_steps_track_the_drag() {
    let key = 0xd0d0;
    let _guard = register(key);
    push(key, FileStep::Enter(vec!["a".into()], Some((1.0, 1.0))));
    push(key, FileStep::Move((2.0, 2.0)));
    push(key, FileStep::Move((3.0, 3.0)));
    assert!(over(key));
    push(key, FileStep::Drop(vec!["a".into()], Some((3.0, 3.0))));
    assert!(!over(key));
    let steps = take_key(key);
    assert_eq!(steps, [
        FileStep::Enter(vec!["a".into()], Some((1.0, 1.0))),
        FileStep::Move((3.0, 3.0)),
        FileStep::Drop(vec!["a".into()], Some((3.0, 3.0))),
    ]);
}

#[test]
fn dropping_guard_prunes_view_and_rejects_stale_promises_on_recycled_address() {
    let key = 0xd0d1;
    let first = register(key);
    let stale_token = first.token;
    push(key, FileStep::Enter(vec!["stale".into()], Some((1.0, 1.0))));
    assert!(over(key));

    drop(first);
    assert!(DESTS.with(|d| !d.borrow().contains_key(&key)), "dropping the guard must prune DESTS");
    assert!(!over(key));

    // A late promise arriving after the window closed must not re-insert into DESTS.
    push_for(key, stale_token, FileStep::Drop(vec!["late".into()], Some((2.0, 2.0))));
    assert!(DESTS.with(|d| !d.borrow().contains_key(&key)));

    // A new window allocated at the same content view address starts clean and ignores the
    // closed window's late promise.
    let second = register(key);
    assert_ne!(second.token, stale_token);
    assert!(!over(key));
    assert!(take_key(key).is_empty());

    push_for(key, stale_token, FileStep::Drop(vec!["late".into()], Some((2.0, 2.0))));
    assert!(take_key(key).is_empty());

    push_for(key, second.token, FileStep::Drop(vec!["fresh".into()], Some((4.0, 5.0))));
    assert_eq!(take_key(key), [FileStep::Drop(vec!["fresh".into()], Some((4.0, 5.0)))]);

    drop(second);
    assert!(DESTS.with(|d| !d.borrow().contains_key(&key)));
}

#[test]
fn kvo_observed_drop_subclass_in_both_orders() {
    use objc2_foundation::NSObject;
    let key = NSString::from_str("probe");
    let observer = NSObject::new();

    // Order 1: Rune subclasses first, then KVO observes and removes the observer.
    let below = NSObject::new();
    // SAFETY: `below` and `observer` are live `NSObject` instances; KVO selectors match Foundation.
    unsafe {
        assert!(subclass_view(&below));
        assert!(has_subclass(&below, b"RuneDrops_"));
        let _: () = msg_send![&*below, addObserver: &*observer, forKeyPath: &*key, options: 0usize, context: std::ptr::null_mut::<std::ffi::c_void>()];
        let kvo_class = below.class();
        assert!(kvo_class.name().to_bytes().starts_with(b"NSKVONotifying_"));
        assert!(subclass_view(&below), "already-subclassed view stays valid beneath KVO");
        assert!(std::ptr::eq(below.class(), kvo_class), "KVO's class must stay on top");
        let _: () = msg_send![&*below, removeObserver: &*observer, forKeyPath: &*key];
    }

    // Order 2: KVO observes first, then Rune runs, then KVO removes the observer.
    let above = NSObject::new();
    // SAFETY: `above` and `observer` are live `NSObject` instances; KVO selectors match Foundation.
    unsafe {
        let _: () = msg_send![&*above, addObserver: &*observer, forKeyPath: &*key, options: 0usize, context: std::ptr::null_mut::<std::ffi::c_void>()];
        let kvo_class = above.class();
        assert!(kvo_class.name().to_bytes().starts_with(b"NSKVONotifying_"));
        assert!(!subclass_view(&above), "skip already KVO-observed view");
        assert!(std::ptr::eq(above.class(), kvo_class), "KVO's class must stay on top");
        let _: () = msg_send![&*above, removeObserver: &*observer, forKeyPath: &*key];
    }
}
