//! RFC 2483 `text/uri-list` decoder for local `file://` paths.

use std::path::PathBuf;

/// Parses an RFC 2483 `text/uri-list` payload into local filesystem paths.
///
/// Ignores comment lines (`#...`), blank lines, non-`file://` schemes, foreign hosts (anything
/// other than empty authority or `localhost`), malformed percent escapes, and paths containing
/// NUL bytes.
pub(crate) fn parse_uri_list(mut bytes: &[u8]) -> Vec<PathBuf> {
    while bytes.last() == Some(&0) {
        bytes = &bytes[..bytes.len() - 1];
    }
    let mut paths = Vec::new();
    for raw_line in bytes.split(|&b| b == b'\n') {
        let line = raw_line.strip_suffix(b"\r").unwrap_or(raw_line);
        if line.is_empty() || line[0] == b'#' || line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        if let Some(path) = parse_file_uri(line) {
            paths.push(path);
        }
    }
    paths
}

fn parse_file_uri(line: &[u8]) -> Option<PathBuf> {
    if line.len() < 8 || !line[..7].eq_ignore_ascii_case(b"file://") {
        return None;
    }
    let after_scheme = &line[7..];
    let slash = after_scheme.iter().position(|&b| b == b'/')?;
    let host = &after_scheme[..slash];
    if !host.is_empty() && !host.eq_ignore_ascii_case(b"localhost") {
        return None;
    }
    let path_part = &after_scheme[slash..];
    let path_end = path_part.iter().position(|&b| b == b'?' || b == b'#').unwrap_or(path_part.len());
    let decoded = percent_decode(&path_part[..path_end])?;
    if decoded.is_empty() || decoded[0] != b'/' || decoded.contains(&0) {
        return None;
    }
    bytes_to_path(decoded)
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn percent_decode(input: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' {
            if i + 2 >= input.len() {
                return None;
            }
            let hi = hex_val(input[i + 1])?;
            let lo = hex_val(input[i + 2])?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(input[i]);
            i += 1;
        }
    }
    Some(out)
}

#[cfg(unix)]
fn bytes_to_path(bytes: Vec<u8>) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

#[cfg(not(unix))]
fn bytes_to_path(bytes: Vec<u8>) -> Option<PathBuf> {
    String::from_utf8(bytes).ok().map(PathBuf::from)
}
