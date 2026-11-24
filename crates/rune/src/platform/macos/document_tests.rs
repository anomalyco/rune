use std::cell::{Cell, RefCell};
use std::path::Path;

use objc2_foundation::{NSString, NSURL};

use super::*;

#[derive(Default)]
struct FakeWindow {
    url_path: RefCell<Option<String>>,
    url_is_dir: Cell<bool>,
    url_calls: Cell<usize>,
    title: RefCell<Option<String>>,
    title_calls: Cell<usize>,
    edited: Cell<bool>,
    edited_calls: Cell<usize>,
}

impl DocumentTarget for FakeWindow {
    fn set_represented_url(&self, url: Option<&NSURL>) {
        self.url_calls.set(self.url_calls.get() + 1);
        *self.url_path.borrow_mut() = url.and_then(|u| u.path().map(|p| p.to_string()));
        self.url_is_dir.set(url.is_some_and(|u| u.hasDirectoryPath()));
    }

    fn set_title(&self, title: &NSString) {
        self.title_calls.set(self.title_calls.get() + 1);
        *self.title.borrow_mut() = Some(title.to_string());
    }

    fn set_document_edited(&self, edited: bool) {
        self.edited_calls.set(self.edited_calls.get() + 1);
        self.edited.set(edited);
    }
}

#[test]
fn file_url_builds_pure_file_urls_without_disk_stat() {
    assert!(file_url(Path::new("")).is_none());

    // Path does not need to exist on disk.
    let file = file_url(Path::new("/nonexistent/rune-doc/notes.md")).expect("file URL");
    assert!(file.isFileURL());
    assert!(!file.hasDirectoryPath());
    assert_eq!(file.path().map(|p| p.to_string()).as_deref(), Some("/nonexistent/rune-doc/notes.md"));

    // Trailing slash marks a directory without calling stat().
    let dir = file_url(Path::new("/nonexistent/rune-doc/")).expect("dir URL");
    assert!(dir.isFileURL());
    assert!(dir.hasDirectoryPath());
    assert_eq!(dir.path().map(|p| p.to_string()).as_deref(), Some("/nonexistent/rune-doc"));

    // Spaces and non-ASCII characters round-trip through NSURL::path().
    let unicode = file_url(Path::new("/nonexistent/My Notes (日本語).txt")).expect("unicode URL");
    assert!(unicode.isFileURL());
    assert_eq!(
        unicode.path().map(|p| p.to_string()).as_deref(),
        Some("/nonexistent/My Notes (日本語).txt"),
    );
}

#[test]
fn apply_represented_file_updates_url_and_optional_title_from_file() {
    let win = FakeWindow::default();

    // title_from_file = false sets representedURL and leaves title untouched.
    apply_represented_file_to(&win, Some(Path::new("/tmp/rune-notes/groceries.md")), false, Some("Notes"));
    assert_eq!(win.url_path.borrow().as_deref(), Some("/tmp/rune-notes/groceries.md"));
    assert!(!win.url_is_dir.get());
    assert_eq!(win.title_calls.get(), 0);

    // title_from_file = true sets both representedURL and the file name as title.
    apply_represented_file_to(&win, Some(Path::new("/tmp/rune-notes/lisbon.md")), true, Some("Notes"));
    assert_eq!(win.url_path.borrow().as_deref(), Some("/tmp/rune-notes/lisbon.md"));
    assert_eq!(win.title.borrow().as_deref(), Some("lisbon.md"));
    assert_eq!(win.title_calls.get(), 1);

    // Clearing with None clears representedURL and restores the fallback title.
    apply_represented_file_to(&win, None, true, Some("Notes"));
    assert_eq!(win.url_path.borrow().as_deref(), None);
    assert_eq!(win.title.borrow().as_deref(), Some("Notes"));
    assert_eq!(win.title_calls.get(), 2);

    // Clearing with an empty path also clears representedURL and restores fallback.
    apply_represented_file_to(&win, Some(Path::new("")), true, Some("Untitled"));
    assert_eq!(win.url_path.borrow().as_deref(), None);
    assert_eq!(win.title.borrow().as_deref(), Some("Untitled"));
    assert_eq!(win.title_calls.get(), 3);
}

#[test]
fn apply_document_edited_forwards_state() {
    let win = FakeWindow::default();
    assert!(!win.edited.get());

    apply_document_edited_to(&win, true);
    assert!(win.edited.get());
    assert_eq!(win.edited_calls.get(), 1);

    apply_document_edited_to(&win, false);
    assert!(!win.edited.get());
    assert_eq!(win.edited_calls.get(), 2);
}
