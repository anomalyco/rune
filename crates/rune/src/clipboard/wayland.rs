//! The clipboard and primary selection on Linux and the BSDs built without the `x11` feature:
//! Wayland's data-control protocol through wl-clipboard-rs, which arboard uses for Wayland too.
//! Text, HTML beside its text, PNG images and files (`text/uri-list`), one form per write as
//! arboard writes them; a secret is offered with KDE's password-manager hint.
use std::io::Read;

use wl_clipboard_rs::copy::{self, MimeSource, MimeType, Source};
use wl_clipboard_rs::paste::{self, Seat};

use super::*;
use crate::platform::wayland_drop::machine as drop;

const PNG: &str = "image/png";
const URI_LIST: &str = "text/uri-list";
const HTML: &str = "text/html";

/// A compositor with data control (wlroots' or `ext-data-control-v1`). Each read and write opens
/// its own connection; a write's source is served from wl-clipboard-rs's own thread until
/// another client takes the selection.
pub(super) struct Clipboard;

impl Clipboard {
    /// None when there's no Wayland display, or it offers no data control.
    pub(super) fn open() -> Option<Self> {
        wl_clipboard_rs::utils::is_primary_selection_supported().ok().map(|_| Self)
    }

    fn write(&self, which: copy::ClipboardType, sources: Vec<MimeSource>, secret: bool) -> Result<(), String> {
        let mut options = copy::Options::new();
        options.clipboard(which).sensitive(secret);
        options.copy_multi(sources).map_err(|e| e.to_string())
    }

    /// The selection's bytes in `mime`, at most `limit` of them (none: all of them); None when
    /// it offers no such form or holds more.
    fn read(&self, which: paste::ClipboardType, mime: paste::MimeType<'_>, limit: Option<usize>) -> Option<Vec<u8>> {
        let (mut pipe, _) = paste::get_contents(which, Seat::Unspecified, mime).ok()?;
        let mut bytes = Vec::new();
        match limit {
            Some(limit) => { (&mut pipe).take(limit as u64 + 1).read_to_end(&mut bytes).ok()?; }
            None => { pipe.read_to_end(&mut bytes).ok()?; }
        }
        limit.is_none_or(|limit| bytes.len() <= limit).then_some(bytes)
    }

    fn read_text(&self, which: paste::ClipboardType) -> Option<String> {
        self.read(which, paste::MimeType::Text, None).and_then(|b| String::from_utf8(b).ok())
    }

    /// Writes text alone to the primary selection.
    pub(super) fn write_primary(&self, text: String) {
        self.write(copy::ClipboardType::Primary, vec![bytes(text.into_bytes(), MimeType::Text)], false).ok();
    }

    /// The primary selection's text, when it has any.
    pub(super) fn read_primary(&self) -> Option<String> {
        self.read_text(paste::ClipboardType::Primary).filter(|t| !t.is_empty())
    }
}

fn bytes(bytes: impl Into<Box<[u8]>>, mime_type: MimeType) -> MimeSource {
    MimeSource { source: Source::Bytes(bytes.into()), mime_type }
}

impl Backend for Clipboard {
    /// The files, else the image, else HTML beside the text, else the text. A secret goes as
    /// text alone.
    fn write(&mut self, copy: &ClipboardWrite) -> Result<(), String> {
        let content = &copy.content;
        let text = || bytes(content.text().as_bytes(), MimeType::Text);
        let sources = if copy.clear_after.is_some() || !content.is_rich() {
            vec![text()]
        } else if !content.files().is_empty() {
            vec![bytes(uri_list(content.files()).into_bytes(), MimeType::Specific(URI_LIST.into()))]
        } else if let Some(image) = content.image() {
            vec![bytes(image.png(), MimeType::Specific(PNG.into()))]
        } else if let Some(html) = content.html() {
            vec![text(), bytes(html.as_bytes(), MimeType::Specific(HTML.into()))]
        } else {
            vec![text()]
        };
        Clipboard::write(self, copy::ClipboardType::Regular, sources, copy.clear_after.is_some())
    }

    fn text(&mut self) -> Option<Zeroizing<String>> {
        self.read_text(paste::ClipboardType::Regular).map(Zeroizing::new)
    }

    /// Text, HTML, and with `all` a PNG image and files.
    fn content(&mut self, all: bool) -> Option<ClipboardContent> {
        let regular = paste::ClipboardType::Regular;
        let text = self.text();
        let html = self.read(regular, paste::MimeType::Specific(HTML), Some(html::MAX_RICH_BYTES))
            .and_then(|b| String::from_utf8(b).ok()).filter(|h| !h.is_empty());
        let image = all.then(|| self.read(regular, paste::MimeType::Specific(PNG), Some(MAX_IMAGE_BYTES))).flatten()
            .and_then(ClipboardImage::from_png);
        let files = all.then(|| self.read(regular, paste::MimeType::Specific(URI_LIST), Some(drop::MAX_URI_LIST_BYTES))).flatten()
            .map(|b| drop::parse_uri_list(&b)).unwrap_or_default();
        if text.is_none() && html.is_none() && image.is_none() && files.is_empty() { return None; }
        let mut content = ClipboardContent::from(text.unwrap_or_default()).with_files(files);
        if let Some(html) = html { content = content.with_html(html); }
        if let Some(image) = image { content = content.with_image(image); }
        Some(content)
    }

    fn revision(&self) -> Option<u64> { None }

    fn clear(&mut self) -> Result<(), String> {
        copy::clear(copy::ClipboardType::Regular, copy::Seat::All).map_err(|e| e.to_string())
    }
}

/// `paths` as a `text/uri-list`: one `file://` URI per line, each byte outside RFC 3986's
/// unreserved set and `/` percent-encoded.
pub(super) fn uri_list(paths: &[std::path::PathBuf]) -> String {
    use std::fmt::Write as _;
    use std::os::unix::ffi::OsStrExt as _;
    let mut out = String::new();
    for path in paths {
        let Ok(path) = std::path::absolute(path) else { continue };
        out.push_str("file://");
        for &b in path.as_os_str().as_bytes() {
            if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'.' | b'_' | b'~') {
                out.push(b as char);
            } else {
                write!(out, "%{b:02X}").ok();
            }
        }
        out.push_str("\r\n");
    }
    out
}

#[cfg(test)]
#[path = "wayland_tests.rs"]
mod tests;
