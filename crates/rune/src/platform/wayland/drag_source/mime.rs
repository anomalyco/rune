//! Offered MIME assembly and RFC 2483 `text/uri-list` encoding for Wayland drag sources.

use std::path::PathBuf;
use std::sync::Arc;

/// Standard MIME types emitted for built-in `DragItem` payloads.
pub(crate) const MIME_URI_LIST: &str = "text/uri-list";
pub(crate) const MIME_PNG: &str = "image/png";
pub(crate) const MIME_TEXT_UTF8: &str = "text/plain;charset=utf-8";
pub(crate) const MIME_TEXT_PLAIN: &str = "text/plain";

/// `wl_data_device_manager::dnd_action` bit constants (from `wayland.xml`).
pub(crate) const DND_ACTION_NONE: u32 = 0;
pub(crate) const DND_ACTION_COPY: u32 = 1;
pub(crate) const DND_ACTION_MOVE: u32 = 2;

/// One offered MIME type and its shared immutable byte payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OfferedMime {
    pub mime: String,
    pub bytes: Arc<[u8]>,
}

/// Maps `DragItem` Copy/Move flags to the Wayland `wl_data_device_manager::dnd_action`
/// bitmask. Wayland's `wl_data_device` protocol has no Link action.
#[inline]
pub(crate) const fn dnd_action_mask(offers_copy: bool, offers_move: bool) -> u32 {
    let mut mask = DND_ACTION_NONE;
    if offers_copy {
        mask |= DND_ACTION_COPY;
    }
    if offers_move {
        mask |= DND_ACTION_MOVE;
    }
    mask
}

/// Encodes local absolute paths as an RFC 2483 `text/uri-list` payload (`file://...\r\n`),
/// percent-encoding raw path bytes on Unix (UTF-8 bytes elsewhere). Skips empty, non-absolute,
/// or NUL-containing paths.
pub(crate) fn encode_uri_list(paths: &[PathBuf]) -> Vec<u8> {
    let mut out = Vec::new();
    for path in paths {
        let raw = path_bytes(path);
        if raw.is_empty() || raw[0] != b'/' || raw.contains(&0) {
            continue;
        }
        out.extend_from_slice(b"file://");
        percent_encode_path(raw, &mut out);
        out.extend_from_slice(b"\r\n");
    }
    out
}

#[cfg(unix)]
fn path_bytes(path: &std::path::Path) -> &[u8] {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes()
}

#[cfg(not(unix))]
fn path_bytes(path: &std::path::Path) -> &[u8] {
    path.to_str().map(str::as_bytes).unwrap_or(&[])
}

fn percent_encode_path(bytes: &[u8], out: &mut Vec<u8>) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &b in bytes {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(b);
        } else {
            out.push(b'%');
            out.push(HEX[(b >> 4) as usize]);
            out.push(HEX[(b & 0x0f) as usize]);
        }
    }
}

/// Assembles the ordered, deduplicated list of offered MIME entries for a Wayland drag source.
///
/// - Preserves explicit `custom` entries in insertion order (later duplicates replace in place).
/// - Appends built-in `text/uri-list` (from `paths`), `image/png` (from `png`), and
///   `text/plain;charset=utf-8` + `text/plain` (from `plain_text`, or path fallback) only when
///   not already supplied by an explicit custom entry, so no duplicate `wl_data_source.offer` is
///   ever emitted.
pub(crate) fn assemble_offers<'a>(
    custom: impl IntoIterator<Item = (&'a str, &'a [u8])>,
    paths: &[PathBuf],
    png: Option<&[u8]>,
    plain_text: Option<&str>,
) -> Vec<OfferedMime> {
    let mut offers: Vec<OfferedMime> = Vec::new();
    for (mime, bytes) in custom {
        if let Some(existing) = offers.iter_mut().find(|o| o.mime == mime) {
            existing.bytes = Arc::from(bytes);
        } else {
            offers.push(OfferedMime { mime: mime.to_owned(), bytes: Arc::from(bytes) });
        }
    }
    let has = |offers: &[OfferedMime], mime: &str| offers.iter().any(|o| o.mime == mime);

    if !paths.is_empty() && !has(&offers, MIME_URI_LIST) {
        let uri_list = encode_uri_list(paths);
        if !uri_list.is_empty() {
            offers.push(OfferedMime {
                mime: MIME_URI_LIST.to_owned(),
                bytes: Arc::from(uri_list),
            });
        }
    }
    if let Some(png_bytes) = png
        && !png_bytes.is_empty()
        && !has(&offers, MIME_PNG)
    {
        offers.push(OfferedMime {
            mime: MIME_PNG.to_owned(),
            bytes: Arc::from(png_bytes),
        });
    }

    let fallback_text: Option<String> = if plain_text.is_none() && !paths.is_empty() {
        let joined: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        Some(joined.join("\n"))
    } else {
        None
    };
    if let Some(text) = plain_text.or(fallback_text.as_deref()) {
        let shared: Arc<[u8]> = Arc::from(text.as_bytes());
        if !has(&offers, MIME_TEXT_UTF8) {
            offers.push(OfferedMime { mime: MIME_TEXT_UTF8.to_owned(), bytes: shared.clone() });
        }
        if !has(&offers, MIME_TEXT_PLAIN) {
            offers.push(OfferedMime { mime: MIME_TEXT_PLAIN.to_owned(), bytes: shared });
        }
    }

    offers
}

/// Assembles offered Wayland MIME entries from a `DragItem`.
///
/// Explicit `DragItem::data` entries keep their insertion order and take precedence over colliding
/// built-in MIME types (`text/uri-list`, `image/png`, `text/plain;charset=utf-8`, `text/plain`),
/// matching macOS and Windows with zero duplicate offers.
pub(crate) fn offers_from_item(item: &crate::drag_out::DragItem) -> Vec<OfferedMime> {
    assemble_offers(item.custom_data(), item.paths(), item.png_data(), item.plain_text())
}
