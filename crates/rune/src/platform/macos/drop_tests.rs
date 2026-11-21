#![allow(unsafe_code)]

use super::*;
use objc2_foundation::NSData;

/// Whether this session has a pasteboard server. A headless or SSH-only session (no window
/// server) has none, and `pasteboardWithUniqueName` returns nil there, so the round-trip
/// tests skip with a note instead of panicking.
fn pasteboard_available() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let probe = std::panic::catch_unwind(|| {
            // SAFETY: probes whether the session's pasteboard server accepts a temporary unique
            // pasteboard and releases it before returning.
            unsafe {
                let pb: Option<Retained<NSPasteboard>> = msg_send![class!(NSPasteboard), pasteboardWithUniqueName];
                let Some(pb) = pb else { return false };
                pb.clearContents();
                let item = NSPasteboardItem::new();
                let kind = NSString::from_str("public.utf8-plain-text");
                let ok = item.setString_forType(&NSString::from_str("probe"), &kind)
                    && pb.writeObjects(&NSArray::from_retained_slice(&[objc2::runtime::ProtocolObject::from_retained(item)]))
                    && pb.stringForType(&kind).is_some_and(|s| s.to_string() == "probe");
                let () = msg_send![&*pb, releaseGlobally];
                ok
            }
        });
        let ok = probe.unwrap_or(false);
        if !ok { eprintln!("macos_drop: no pasteboard in this session (headless?); skipping pasteboard round-trip tests"); }
        ok
    })
}

/// A private pasteboard, released when dropped.
struct Board(Retained<NSPasteboard>);

impl Drop for Board {
    fn drop(&mut self) {
        // SAFETY: `self.0` is a private pasteboard created via `pasteboardWithUniqueName` and released once on drop.
        unsafe { msg_send![&*self.0, releaseGlobally] }
    }
}

impl std::ops::Deref for Board {
    type Target = NSPasteboard;
    fn deref(&self) -> &NSPasteboard { &self.0 }
}

/// A private pasteboard holding `items`, or None without a pasteboard.
fn board(items: &[&[(&str, &[u8])]]) -> Option<Board> {
    if !pasteboard_available() { return None; }
    let pb = Board(NSPasteboard::pasteboardWithUniqueName());
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
    push(key, FileStep::Enter(vec!["a".into()], Some((1.0, 1.0))));
    push(key, FileStep::Move((2.0, 2.0)));
    push(key, FileStep::Move((3.0, 3.0)));
    assert!(over(key));
    push(key, FileStep::Drop(vec!["a".into()], Some((3.0, 3.0))));
    assert!(!over(key));
    let steps = DESTS.with(|d| std::mem::take(&mut d.borrow_mut().get_mut(&key).unwrap().steps));
    assert_eq!(steps, [
        FileStep::Enter(vec!["a".into()], Some((1.0, 1.0))),
        FileStep::Move((3.0, 3.0)),
        FileStep::Drop(vec!["a".into()], Some((3.0, 3.0))),
    ]);
}
