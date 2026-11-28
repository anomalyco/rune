//! Platform-independent Android document picker request/response translation over
//! [`FileHandle`](crate::FileHandle).
//!
//! Kept free of JNI and Android SDK types so MIME-type mapping, request construction, path/folder
//! refusals, URI display-name fallbacks, and runtime dialog fencing are unit-tested on every host.

use crate::file_dialog::{
    DialogAnswer, FileDialogError, FileDialogKind, FileDialogOptions, FileHandle,
    FileHandleProvider,
};
use crate::runtime::RuntimeCore;

/// Which Android Storage Access Framework intent `RuneView` should launch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PickAction {
    /// `Intent.ACTION_OPEN_DOCUMENT` (single or multi-select).
    Open { multiple: bool },
    /// `Intent.ACTION_CREATE_DOCUMENT` with an optional suggested file name (`Intent.EXTRA_TITLE`).
    Create { file_name: String },
}

/// A handle-based file dialog converted into parameters for `dev.rune.RuneView.startDocumentPick`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PickRequest {
    pub(crate) id: u64,
    pub(crate) action: PickAction,
    /// Primary MIME type passed to `Intent.setType`.
    pub(crate) mime_type: String,
    /// Additional MIME types passed to `Intent.EXTRA_MIME_TYPES` for multi-filter open requests.
    pub(crate) extra_mime_types: Vec<String>,
}

/// Outcome of polling `RuntimeCore` for a pending file dialog on Android.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PickStep {
    /// No new dialog is waiting to be shown (either none is open or one is already showing).
    None,
    /// A path-based or folder dialog was refused with an error; schedule another frame to run its
    /// `on_answer` callback.
    Refused,
    /// A handle-based open or save dialog was claimed and should be launched by `RuneView`.
    Start(PickRequest),
}

/// One picked document reported back by `RuneView`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PickedDocument {
    pub(crate) uri: String,
    pub(crate) name: String,
}

/// Raw outcome reported back by `RuneView` for a pending `PickRequest`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PickAnswer {
    /// User chose one or more documents (or created one to save).
    Picked(Vec<PickedDocument>),
    /// User cancelled the picker.
    Cancelled,
    /// The picker could not be launched or failed.
    Error(String),
}

const ERR_FOLDER_UNSUPPORTED: &str = "folder dialogs are not supported on Android";
const ERR_PATH_UNSUPPORTED: &str =
    "path-based file dialogs are not supported on Android; use pick_file, pick_files, or pick_save_file";

/// Claims the next pending file dialog from `rt`, refusing path-based and folder dialogs and
/// translating handle-based open/save dialogs into a [`PickRequest`].
pub(crate) fn take_pick(rt: &mut RuntimeCore) -> PickStep {
    let Some((kind, _)) = rt.file_dialog() else {
        return PickStep::None;
    };
    let handles = rt.file_dialog_handles();
    if kind == FileDialogKind::Folder {
        if let Some((id, ..)) = rt.take_file_dialog() {
            rt.answer_file_dialog(id, Err(FileDialogError(ERR_FOLDER_UNSUPPORTED.into())));
            return PickStep::Refused;
        }
        return PickStep::None;
    }
    if !handles {
        if let Some((id, ..)) = rt.take_file_dialog() {
            rt.answer_file_dialog(id, Err(FileDialogError(ERR_PATH_UNSUPPORTED.into())));
            return PickStep::Refused;
        }
        return PickStep::None;
    }
    let Some((id, kind, multiple, options)) = rt.take_file_dialog() else {
        return PickStep::None;
    };
    PickStep::Start(build_request(id, kind, multiple, &options))
}

/// Delivers a picker outcome back to `rt`, wrapping each picked URI in a [`FileHandle`] backed by
/// `make_provider`.
pub(crate) fn deliver_pick<P: FileHandleProvider + 'static>(
    rt: &mut RuntimeCore,
    id: u64,
    answer: PickAnswer,
    make_provider: impl Fn(String) -> P,
) {
    let outcome = match answer {
        PickAnswer::Cancelled => Ok(DialogAnswer::Handles(None)),
        PickAnswer::Error(msg) => Err(FileDialogError(msg)),
        PickAnswer::Picked(docs) if docs.is_empty() => Ok(DialogAnswer::Handles(None)),
        PickAnswer::Picked(docs) => {
            let handles = docs
                .into_iter()
                .map(|doc| {
                    let name = if doc.name.is_empty() {
                        fallback_display_name(&doc.uri)
                    } else {
                        doc.name
                    };
                    FileHandle::from_provider(name, make_provider(doc.uri))
                })
                .collect();
            Ok(DialogAnswer::Handles(Some(handles)))
        }
    };
    rt.answer_file_dialog(id, outcome);
}

pub(crate) fn build_request(
    id: u64,
    kind: FileDialogKind,
    multiple: bool,
    options: &FileDialogOptions,
) -> PickRequest {
    match kind {
        FileDialogKind::Save => {
            let file_name = options.file_name.clone().unwrap_or_default();
            let mime_type = create_mime_type(&file_name, &options.filters);
            PickRequest {
                id,
                action: PickAction::Create { file_name },
                mime_type,
                extra_mime_types: Vec::new(),
            }
        }
        FileDialogKind::Open | FileDialogKind::Folder => {
            let (mime_type, extra_mime_types) = open_mime_types(&options.filters);
            PickRequest {
                id,
                action: PickAction::Open { multiple },
                mime_type,
                extra_mime_types,
            }
        }
    }
}

/// Computes `(primary_type, extra_mime_types)` for `ACTION_OPEN_DOCUMENT`.
///
/// If no filters are provided, or any filter includes a wildcard or an unrecognized extension,
/// falls back to `("*/*", [])` so DocumentsUI never greys out valid files.
pub(crate) fn open_mime_types(filters: &[(String, Vec<String>)]) -> (String, Vec<String>) {
    let mut mimes: Vec<String> = Vec::new();
    let mut any_extension = false;
    for (_, extensions) in filters {
        for raw in extensions {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            any_extension = true;
            let Some(mime) = filter_entry_to_mime(trimmed) else {
                return ("*/*".into(), Vec::new());
            };
            if mime == "*/*" {
                return ("*/*".into(), Vec::new());
            }
            if !mimes.iter().any(|existing| existing == &mime) {
                mimes.push(mime);
            }
        }
    }
    if !any_extension || mimes.is_empty() {
        ("*/*".into(), Vec::new())
    } else if mimes.len() == 1 {
        (mimes.remove(0), Vec::new())
    } else {
        ("*/*".into(), mimes)
    }
}

/// Computes the concrete MIME type for `ACTION_CREATE_DOCUMENT`:
/// 1. Extension of `file_name` when recognized.
/// 2. First concrete MIME type from `filters`.
/// 3. `"application/octet-stream"`.
pub(crate) fn create_mime_type(file_name: &str, filters: &[(String, Vec<String>)]) -> String {
    if let Some((_, ext)) = file_name.rsplit_once('.')
        && !ext.is_empty()
        && let Some(mime) = extension_to_mime(ext)
    {
        return mime.to_owned();
    }
    for (_, extensions) in filters {
        for raw in extensions {
            if let Some(mime) = filter_entry_to_mime(raw.trim())
                && !mime.contains('*')
            {
                return mime;
            }
        }
    }
    "application/octet-stream".into()
}

fn filter_entry_to_mime(entry: &str) -> Option<String> {
    if entry == "*" || entry == "*.*" || entry == "*/*" {
        return Some("*/*".into());
    }
    if let Some((ty, sub)) = entry.split_once('/')
        && !ty.is_empty()
        && !sub.is_empty()
        && !entry.chars().any(char::is_whitespace)
    {
        return Some(entry.to_ascii_lowercase());
    }
    let ext = entry.trim_start_matches('*').trim_start_matches('.');
    if ext.is_empty() || ext == "*" {
        return Some("*/*".into());
    }
    extension_to_mime(ext).map(str::to_owned)
}

/// Maps common file extensions to IANA MIME types recognized by Android DocumentsUI.
pub(crate) fn extension_to_mime(ext: &str) -> Option<&'static str> {
    let lower = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    let mime = match lower.as_str() {
        // Text & structured data
        "txt" | "text" | "log" | "ini" | "cfg" | "conf" | "env" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" | "cjs" => "text/javascript",
        "ts" | "mts" | "cts" => "text/typescript",
        "json" | "jsonl" => "application/json",
        "xml" => "application/xml",
        "yaml" | "yml" => "application/yaml",
        "toml" => "application/toml",
        "rtf" => "application/rtf",
        "ics" => "text/calendar",
        "vcf" => "text/vcard",
        // Code
        "rs" => "text/x-rust",
        "py" => "text/x-python",
        "c" | "h" => "text/x-c",
        "cc" | "cpp" | "cxx" | "hpp" => "text/x-c++",
        "java" => "text/x-java-source",
        "kt" | "kts" => "text/x-kotlin",
        "sh" => "application/x-sh",
        "sql" => "application/sql",
        // Images
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "tif" | "tiff" => "image/tiff",
        // Audio
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "flac" => "audio/flac",
        "aac" => "audio/aac",
        "m4a" => "audio/mp4",
        "weba" => "audio/webm",
        // Video
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        "ogv" => "video/ogg",
        // Documents & archives
        "pdf" => "application/pdf",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "epub" => "application/epub+zip",
        "zip" => "application/zip",
        "gz" | "gzip" => "application/gzip",
        "tar" => "application/x-tar",
        "7z" => "application/x-7z-compressed",
        "rar" => "application/vnd.rar",
        // Fonts & binaries
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "wasm" => "application/wasm",
        "bin" => "application/octet-stream",
        _ => return None,
    };
    Some(mime)
}

/// Derives a fallback display name when a document provider returns no `DISPLAY_NAME` column.
pub(crate) fn fallback_display_name(uri: &str) -> String {
    let without_query = uri.split(['?', '#']).next().unwrap_or(uri);
    let last_segment = without_query.rsplit('/').next().unwrap_or("");
    let decoded = percent_decode(last_segment);
    let candidate = decoded.rsplit(['/', ':']).next().unwrap_or("").trim();
    if candidate.is_empty() {
        "document".into()
    } else {
        candidate.to_owned()
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
