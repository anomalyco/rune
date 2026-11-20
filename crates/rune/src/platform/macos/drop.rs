//! Drags from other apps onto a window, on macOS. winit's window takes only file paths, so a
//! drag carrying none was refused: the screenshot thumbnail, Photos and browsers' images send a
//! file promise, Safari and canvases bare image data. The content view becomes the window's
//! drag destination instead (a per-instance subclass, as `own_window_dragging` makes): a file
//! promise is received into a temporary folder and image data is written there as a file, so a
//! drop still reaches `El::on_drop_files` as paths. AppKit says where the drag is at every step,
//! so the host needn't sample the pointer.

#![allow(unsafe_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::{class, msg_send, sel};
use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSView};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSPoint, NSString, NSURL};

use crate::window::FileStep;

#[derive(Default)]
struct Dest {
    steps: Vec<FileStep>,
    /// A drag this view takes is over it.
    over: bool,
}

thread_local! {
    /// Each content view's steps since its host last took them, by the view's address.
    static DESTS: RefCell<HashMap<usize, Dest>> = RefCell::default();
}

const FILE_URL: &str = "public.file-url";
const PROMISED_TYPE: &str = "com.apple.pasteboard.promised-file-content-type";
/// Image data a drop writes to a file, by pasteboard type, with the file's extension. TIFF, what
/// most Mac apps drag, is written as PNG.
const IMAGES: [(&str, &str); 6] = [
    ("public.png", "png"), ("public.jpeg", "jpg"), ("com.compuserve.gif", "gif"),
    ("org.webmproject.webp", "webp"), ("public.heic", "heic"), ("public.tiff", "png"),
];

/// What one pasteboard item gives a drop.
#[derive(Debug, PartialEq)]
enum Item {
    File(PathBuf),
    /// A file the source writes when asked, with its type's extension when known.
    Promise(Option<&'static str>),
    /// Image data: its pasteboard type and file extension.
    Image(&'static str, &'static str),
}

impl Item {
    /// The path a drag over the window reports: the file, else a stand-in named for its type,
    /// which is all an `El::accepts_files` filter reads.
    fn hover_path(&self) -> PathBuf {
        match self {
            Item::File(path) => path.clone(),
            Item::Promise(ext) => PathBuf::from(ext.map_or("File".into(), |e| format!("File.{e}"))),
            Item::Image(_, ext) => PathBuf::from(format!("Image.{ext}")),
        }
    }
}

/// The content view of `window` takes drags of files, file promises and image data.
pub(crate) fn install(window: &winit::window::Window) {
    let Some(view) = crate::platform::macos::content_view(window) else { return };
    // SAFETY: main thread (windows are created there), on winit's live content view.
    unsafe {
        let base = view.class();
        if !base.name().to_string_lossy().contains("RuneDrops_") {
            let Ok(name) = std::ffi::CString::new(format!("RuneDrops_{}", base.name().to_string_lossy())) else { return };
            let class = match ClassBuilder::new(&name, base) {
                Some(mut b) => {
                    b.add_method(sel!(draggingEntered:), entered as unsafe extern "C" fn(_, _, _) -> _);
                    b.add_method(sel!(draggingUpdated:), updated as unsafe extern "C" fn(_, _, _) -> _);
                    b.add_method(sel!(draggingExited:), exited as unsafe extern "C" fn(_, _, _));
                    b.add_method(sel!(prepareForDragOperation:), prepare as unsafe extern "C" fn(_, _, _) -> _);
                    b.add_method(sel!(performDragOperation:), perform as unsafe extern "C" fn(_, _, _) -> _);
                    b.register()
                }
                None => match AnyClass::get(&name) { Some(c) => c, None => return },
            };
            // Only this instance's class changes, to a subclass without ivars.
            AnyObject::set_class(&view, class);
        }
        view.registerForDraggedTypes(&NSArray::from_retained_slice(&registered_types()));
    }
}

/// The steps of drags over `window` since the last call.
pub(crate) fn take(window: &winit::window::Window) -> Vec<FileStep> {
    let Some(view) = crate::platform::macos::content_view(window) else { return Vec::new() };
    let key = Retained::as_ptr(&view) as usize;
    DESTS.with(|d| d.borrow_mut().get_mut(&key).map(|d| std::mem::take(&mut d.steps)).unwrap_or_default())
}

fn push(key: usize, step: FileStep) {
    DESTS.with(|d| {
        let mut d = d.borrow_mut();
        let dest = d.entry(key).or_default();
        // A move replaces the move before it: the host wants only the latest position.
        if matches!(step, FileStep::Move(_)) && matches!(dest.steps.last(), Some(FileStep::Move(_))) { dest.steps.pop(); }
        match step {
            FileStep::Enter(..) | FileStep::Move(_) => dest.over = true,
            FileStep::Drop(..) | FileStep::Leave => dest.over = false,
        }
        dest.steps.push(step);
    });
}

fn over(key: usize) -> bool { DESTS.with(|d| d.borrow().get(&key).is_some_and(|d| d.over)) }

fn registered_types() -> Vec<Retained<NSString>> {
    let mut types: Vec<Retained<NSString>> = [FILE_URL, "NSFilenamesPboardType"].into_iter()
        .chain(IMAGES.iter().map(|(t, _)| *t)).map(NSString::from_str).collect();
    types.extend(promise_types());
    types
}

/// The types a file promise's item carries, new (`NSFilePromiseProvider`) and old.
fn promise_types() -> Vec<Retained<NSString>> {
    // SAFETY: a class method returning an array of strings.
    unsafe {
        let types: Retained<NSArray<NSString>> = msg_send![class!(NSFilePromiseReceiver), readableDraggedTypes];
        types.to_vec()
    }
}

/// The operation a drag gets: copy, else what the source calls generic, else none (a Rune
/// window's own drag-out offers nothing to the app).
fn operation(mask: usize) -> usize {
    const COPY: usize = 1;
    const GENERIC: usize = 4;
    if mask & COPY != 0 { COPY } else if mask & GENERIC != 0 { GENERIC } else { 0 }
}

/// What each item of `pb` gives a drop, in order. An item that gives nothing is left out.
unsafe fn items(pb: &NSPasteboard) -> Vec<(Retained<NSPasteboardItem>, Item)> {
    let promises: Vec<String> = promise_types().iter().map(|t| t.to_string()).collect();
    let Some(list) = pb.pasteboardItems() else { return Vec::new() };
    list.to_vec().into_iter().filter_map(|item| {
        let types: Vec<String> = item.types().iter().map(|t| t.to_string()).collect();
        let has = |t: &str| types.iter().any(|x| x == t);
        let found = if has(FILE_URL) {
            let url = item.stringForType(&NSString::from_str(FILE_URL))?;
            file_path(&url).map(Item::File)
        } else if types.iter().any(|t| promises.contains(t)) {
            let uti = item.stringForType(&NSString::from_str(PROMISED_TYPE)).map(|s| s.to_string());
            Some(Item::Promise(uti.as_deref().and_then(extension)))
        } else {
            IMAGES.iter().find(|(t, _)| has(t)).map(|&(t, ext)| Item::Image(t, ext))
        };
        found.map(|f| (item, f))
    }).collect()
}

/// The path of a `file://` URL, resolving the file-reference form (`file:///.file/id=…`).
pub(crate) fn file_path(url: &NSString) -> Option<PathBuf> {
    let url = NSURL::URLWithString(url)?;
    let url = url.filePathURL()?;
    Some(PathBuf::from(url.path()?.to_string()))
}

/// A file extension for a few common types a promise names.
fn extension(uti: &str) -> Option<&'static str> {
    Some(match uti {
        "public.png" => "png",
        "public.jpeg" => "jpg",
        "public.tiff" => "tiff",
        "public.heic" => "heic",
        "com.compuserve.gif" => "gif",
        "org.webmproject.webp" => "webp",
        "com.adobe.pdf" => "pdf",
        "public.plain-text" | "public.utf8-plain-text" => "txt",
        "com.apple.quicktime-movie" => "mov",
        "public.mpeg-4" => "mp4",
        _ => return None,
    })
}

/// A new folder for one drop's files under the system's temporary folder.
fn drop_dir() -> Option<PathBuf> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let stamp = crate::time::system_now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let dir = std::env::temp_dir().join("rune-drops").join(format!("{}-{stamp}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).ok().map(|_| dir)
}

/// `stem.ext` in `dir`, numbered when the name is taken.
fn free_name(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    (1..).map(|i| dir.join(if i == 1 { format!("{stem}.{ext}") } else { format!("{stem} {i}.{ext}") }))
        .find(|p| !p.exists()).unwrap_or_else(|| dir.join(format!("{stem}.{ext}")))
}

/// The name an image takes from the URL it came from, if the item has one (a browser's image).
fn image_stem(item: &NSPasteboardItem) -> String {
    let url = item.stringForType(&NSString::from_str("public.url")).map(|u| u.to_string());
    url.as_deref().and_then(url_stem).unwrap_or_else(|| "Image".into())
}

fn url_stem(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("://")?;
    let (_, path) = rest.split(['?', '#']).next()?.split_once('/')?;
    let last = path.trim_end_matches('/').rsplit('/').next()?;
    let stem = last.rsplit_once('.').map_or(last, |(s, _)| s);
    let stem: String = stem.chars().filter(|c| !matches!(c, ':' | '\\' | '\0')).take(80).collect();
    (!stem.is_empty() && !stem.starts_with('.')).then_some(stem)
}

/// Writes an item's image to `dir`, as PNG when it's TIFF.
unsafe fn write_image(item: &NSPasteboardItem, kind: &str, ext: &str, dir: &Path) -> Option<PathBuf> {
    let data = item.dataForType(&NSString::from_str(kind))?;
    // SAFETY: `data` is retained for the duration of this function and is not mutated while borrowed.
    let bytes = unsafe { data.as_bytes_unchecked() };
    let png;
    let bytes = if kind == "public.tiff" {
        png = crate::clipboard::tiff_to_png(bytes)?;
        png.png()
    } else { bytes };
    let path = free_name(dir, &image_stem(item), ext);
    std::fs::write(&path, bytes).ok().map(|_| path)
}

/// The drag's position in `view`, in points from its top-left.
unsafe fn location(view: &NSView, info: &AnyObject) -> (f32, f32) {
    // SAFETY: `info` is a live `NSDraggingInfo` from AppKit on the main thread; `-draggingLocation` returns `NSPoint`.
    let p: NSPoint = unsafe { msg_send![info, draggingLocation] };
    let p = view.convertPoint_fromView(p, None);
    let y = if view.isFlipped() { p.y } else { view.bounds().size.height - p.y };
    (p.x as f32, y as f32)
}

fn key(view: &AnyObject) -> usize { view as *const AnyObject as usize }

unsafe fn view(this: &AnyObject) -> &NSView {
    // SAFETY: these methods are only on the content view's subclass.
    unsafe { &*(this as *const AnyObject).cast::<NSView>() }
}

unsafe fn pasteboard(info: &AnyObject) -> Retained<NSPasteboard> {
    // SAFETY: `info` is a live `NSDraggingInfo` from AppKit on the main thread; `-draggingPasteboard` returns a non-null `NSPasteboard`.
    unsafe { msg_send![info, draggingPasteboard] }
}

/// A drag entered or moved: Enter when it starts to be taken, then Move.
unsafe fn update(this: &AnyObject, info: &AnyObject) -> usize {
    autoreleasepool(|_| {
        // SAFETY: called on the main thread from AppKit drag destination callbacks; `this` is this
        // subclassed `NSView`, `info` is a live `NSDraggingInfo`, and the selectors match `NSDraggingInfo`.
        unsafe {
            let key = key(this);
            let mask: usize = msg_send![info, draggingSourceOperationMask];
            let op = operation(mask);
            let at = location(view(this), info);
            if op == 0 {
                if over(key) { push(key, FileStep::Leave); }
                return 0;
            }
            if over(key) {
                push(key, FileStep::Move(at));
                return op;
            }
            let paths: Vec<PathBuf> = items(&pasteboard(info)).iter().map(|(_, item)| item.hover_path()).collect();
            if paths.is_empty() { return 0; }
            push(key, FileStep::Enter(paths, Some(at)));
            op
        }
    })
}

unsafe extern "C" fn entered(this: &AnyObject, _: Sel, info: &AnyObject) -> usize {
    // SAFETY: AppKit invokes `-draggingEntered:` on the main thread with this subclassed `NSView` and a live `NSDraggingInfo`.
    unsafe { update(this, info) }
}

unsafe extern "C" fn updated(this: &AnyObject, _: Sel, info: &AnyObject) -> usize {
    // SAFETY: AppKit invokes `-draggingUpdated:` on the main thread with this subclassed `NSView` and a live `NSDraggingInfo`.
    unsafe { update(this, info) }
}

unsafe extern "C" fn exited(this: &AnyObject, _: Sel, _info: *mut AnyObject) {
    if over(key(this)) { push(key(this), FileStep::Leave); }
}

unsafe extern "C" fn prepare(this: &AnyObject, _: Sel, _info: &AnyObject) -> Bool { Bool::new(over(key(this))) }

/// The drop: files at once, image data once written, and file promises once their sources
/// have written them, all in one `FileStep::Drop`.
unsafe extern "C" fn perform(this: &AnyObject, _: Sel, info: &AnyObject) -> Bool {
    let key = key(this);
    autoreleasepool(|_| {
        // SAFETY: AppKit invokes `-performDragOperation:` on the main thread with this subclassed
        // `NSView` and a live `NSDraggingInfo`.
        unsafe {
            let at = location(view(this), info);
            let pb = pasteboard(info);
            let items = items(&pb);
            let mut dir = None;
            let mut paths = Vec::new();
            let mut promised = false;
            for (item, found) in &items {
                match found {
                    Item::File(path) => paths.push(path.clone()),
                    Item::Image(kind, ext) => {
                        if dir.is_none() { dir = drop_dir(); }
                        if let Some(path) = dir.as_deref().and_then(|d| write_image(item, kind, ext, d)) { paths.push(path); }
                    }
                    Item::Promise(_) => promised = true,
                }
            }
            if promised && let Some(dir) = dir.or_else(drop_dir) && receive(key, &pb, &dir, paths.clone(), at) {
                return Bool::YES;
            }
            if paths.is_empty() {
                push(key, FileStep::Leave);
                return Bool::NO;
            }
            push(key, FileStep::Drop(paths, Some(at)));
            Bool::YES
        }
    })
}

/// Asks each promise's source to write its files into `dir`. Once all have answered, `paths`
/// and the promised files drop at `at`.
unsafe fn receive(key: usize, pb: &NSPasteboard, dir: &Path, paths: Vec<PathBuf>, at: (f32, f32)) -> bool {
    // SAFETY: called on the main thread with a live `NSPasteboard`; Objective-C selectors match
    // `NSPasteboard`/`NSFilePromiseReceiver`/`NSOperationQueue`, `keep` retains each receiver
    // until its reader block finishes (where AppKit passes a live `NSURL` when `error` is null),
    // and `deliver` hops back to `mainQueue` before touching thread-local state.
    unsafe {
        let classes = NSArray::from_retained_slice(&[Retained::<AnyObject>::retain(class!(NSFilePromiseReceiver) as *const AnyClass as *mut AnyObject).unwrap()]);
        let Some(receivers): Option<Retained<NSArray<AnyObject>>> = msg_send![pb, readObjectsForClasses: &*classes, options: &*NSDictionary::<NSString, AnyObject>::new()]
        else { return false };
        let receivers = receivers.to_vec();
        let files: usize = receivers.iter().map(|r| {
            let types: Retained<NSArray<NSString>> = msg_send![r, fileTypes];
            types.count().max(1)
        }).sum();
        if files == 0 { return false; }
        // The sources write off the main thread; the drop is queued back on it.
        let queue: Retained<AnyObject> = msg_send![class!(NSOperationQueue), new];
        let pending = Arc::new(Mutex::new((files, paths)));
        let destination = NSURL::fileURLWithPath_isDirectory(&NSString::from_str(&dir.to_string_lossy()), true);
        for receiver in &receivers {
            let pending = pending.clone();
            // The receiver lives until its files are in.
            let keep = receiver.clone();
            let reader = block2::RcBlock::new(move |url: *mut NSURL, error: *mut NSError| {
                let _ = &keep;
                let path = if error.is_null() { url.as_ref().and_then(|u| u.path()).map(|p| PathBuf::from(p.to_string())) } else { None };
                let mut p = pending.lock().unwrap_or_else(|e| e.into_inner());
                p.0 = p.0.saturating_sub(1);
                p.1.extend(path);
                if p.0 > 0 { return; }
                let paths = std::mem::take(&mut p.1);
                drop(p);
                let deliver = block2::RcBlock::new(move || {
                    if paths.is_empty() { push(key, FileStep::Leave) } else { push(key, FileStep::Drop(paths.clone(), Some(at))) }
                });
                let main: Retained<AnyObject> = msg_send![class!(NSOperationQueue), mainQueue];
                let _: () = msg_send![&main, addOperationWithBlock: &*deliver];
            });
            let _: () = msg_send![receiver, receivePromisedFilesAtDestination: &*destination, options: &*NSDictionary::<NSString, AnyObject>::new(),
                operationQueue: &*queue, reader: &*reader];
        }
        true
    }
}

#[cfg(test)]
#[path = "drop_tests.rs"]
mod tests;
