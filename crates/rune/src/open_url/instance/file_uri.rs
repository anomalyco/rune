//! Local `file://` URIs as paths (Linux): what a desktop entry's `%U` and
//! `org.freedesktop.Application.Open` hand the app for a file.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

/// A `file://` URI's local path, percent-decoded; none for another host or a relative path.
pub(crate) fn file_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.get(..7).filter(|s| s.eq_ignore_ascii_case("file://")).map(|_| &uri[7..])?;
    let at = rest.find('/')?;
    let host = &rest[..at];
    if !(host.is_empty() || host.eq_ignore_ascii_case("localhost")) {
        return None;
    }
    let path = rest[at..].split(['?', '#']).next()?;
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    if out.contains(&0) {
        return None;
    }
    Some(PathBuf::from(OsString::from_vec(out)))
}

#[cfg(test)]
#[path = "file_uri_tests.rs"]
mod tests;
