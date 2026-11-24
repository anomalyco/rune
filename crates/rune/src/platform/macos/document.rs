//! AppKit document-window properties (`NSWindow.representedURL`, title-from-file, and
//! `NSWindow.documentEdited`), applied without filesystem I/O in the frame loop.

use std::path::Path;

use objc2::rc::Retained;
use objc2_app_kit::NSWindow;
use objc2_foundation::{NSString, NSURL};

use crate::window::spec::{represented_file_title, represented_path_is_dir};

#[cfg(test)]
#[path = "document_tests.rs"]
mod tests;

/// Small seam over `NSWindow` so unit tests verify URL construction, title-from-file
/// derivation, clearing, and edited-state updates without opening a native window.
pub(super) trait DocumentTarget {
    fn set_represented_url(&self, url: Option<&NSURL>);
    fn set_title(&self, title: &NSString);
    fn set_document_edited(&self, edited: bool);
}

impl DocumentTarget for NSWindow {
    fn set_represented_url(&self, url: Option<&NSURL>) {
        self.setRepresentedURL(url);
    }

    fn set_title(&self, title: &NSString) {
        self.setTitle(title);
    }

    fn set_document_edited(&self, edited: bool) {
        self.setDocumentEdited(edited);
    }
}

/// Constructs a file `NSURL` from `path` using `+[NSURL fileURLWithPath:isDirectory:]`, which
/// performs pure string construction with no filesystem `stat`. Returns `None` for empty paths.
pub(super) fn file_url(path: &Path) -> Option<Retained<NSURL>> {
    if path.as_os_str().is_empty() {
        return None;
    }
    let raw = path.to_string_lossy();
    let ns_path = NSString::from_str(&raw);
    Some(NSURL::fileURLWithPath_isDirectory(&ns_path, represented_path_is_dir(path)))
}

pub(super) fn apply_represented_file_to(
    target: &impl DocumentTarget,
    path: Option<&Path>,
    title_from_file: bool,
    fallback_title: Option<&str>,
) {
    let url = path.and_then(file_url);
    target.set_represented_url(url.as_deref());
    if title_from_file {
        if let Some(title) = path.and_then(represented_file_title) {
            target.set_title(&NSString::from_str(&title));
        } else if let Some(fallback) = fallback_title {
            target.set_title(&NSString::from_str(fallback));
        }
    }
}

pub(super) fn apply_document_edited_to(target: &impl DocumentTarget, edited: bool) {
    target.set_document_edited(edited);
}

/// Applies `represented_file` (and optionally derives the title) to an `NSWindow` or adopted
/// `NSPanel`.
pub(crate) fn apply_represented_file(
    window: &NSWindow,
    path: Option<&Path>,
    title_from_file: bool,
    fallback_title: Option<&str>,
) {
    apply_represented_file_to(window, path, title_from_file, fallback_title);
}

/// Applies `documentEdited` to an `NSWindow` or adopted `NSPanel`.
pub(crate) fn apply_document_edited(window: &NSWindow, edited: bool) {
    apply_document_edited_to(window, edited);
}

/// Updates `representedURL` (and title when `title_from_file`) on a winit window or its
/// adopted `NSPanel`.
pub(crate) fn set_represented_file(
    window: &winit::window::Window,
    path: Option<&Path>,
    title_from_file: bool,
    fallback_title: Option<&str>,
) {
    if let Some(window) = super::ns_window(window) {
        apply_represented_file(&window, path, title_from_file, fallback_title);
    }
}

/// Updates `documentEdited` on a winit window or its adopted `NSPanel`.
pub(crate) fn set_document_edited(window: &winit::window::Window, edited: bool) {
    if let Some(window) = super::ns_window(window) {
        apply_document_edited(&window, edited);
    }
}
