//! Element path segment encoding and matching.
//!
//! Paths join segments with `/`. Unkeyed children use `#<index>`; keyed children keep their key
//! verbatim whenever it cannot collide with a path separator or positional index. Keys that are
//! empty, contain `/`, match `#<digits>`, or start with the escape marker `#!` are encoded as
//! `#!` followed by the key with `%` as `%25` and `/` as `%2F`, keeping the mapping from
//! ancestor chains to paths injective without changing unambiguous paths.

use std::borrow::Cow;

const ESCAPE_PREFIX: &str = "#!";

/// Whether `seg` is an unkeyed positional segment (`#<digits>`).
#[inline]
pub(crate) fn is_unkeyed_segment(seg: &str) -> bool {
    let b = seg.as_bytes();
    b.len() > 1 && b[0] == b'#' && b[1..].iter().all(u8::is_ascii_digit)
}

/// Whether a user key must be escaped to avoid aliasing path separators, positional indices, or
/// the escape prefix itself.
#[inline]
pub(crate) fn key_needs_escape(key: &str) -> bool {
    let b = key.as_bytes();
    if b.is_empty() {
        return true;
    }
    if b[0] == b'#' && (b.get(1) == Some(&b'!') || is_unkeyed_segment(key)) {
        return true;
    }
    b.contains(&b'/')
}

#[inline(always)]
pub(super) fn push_usize(buf: &mut String, mut n: usize) {
    if n < 10 {
        buf.push((b'0' + n as u8) as char);
        return;
    }
    let mut digits = [0u8; 20];
    let mut i = digits.len();
    while n > 0 {
        i -= 1;
        digits[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    if let Ok(s) = std::str::from_utf8(&digits[i..]) {
        buf.push_str(s);
    }
}

#[cold]
#[inline(never)]
fn push_escaped_key(path: &mut String, key: &str) {
    path.push_str(ESCAPE_PREFIX);
    let mut start = 0;
    for (i, &b) in key.as_bytes().iter().enumerate() {
        let repl = match b {
            b'/' => "%2F",
            b'%' => "%25",
            _ => continue,
        };
        path.push_str(&key[start..i]);
        path.push_str(repl);
        start = i + 1;
    }
    path.push_str(&key[start..]);
}

#[inline]
pub(crate) fn push_segment(path: &mut String, key: Option<&str>, index: usize) {
    if !path.is_empty() {
        path.push('/');
    }
    match key {
        Some(k) if !key_needs_escape(k) => path.push_str(k),
        Some(k) => push_escaped_key(path, k),
        None => {
            path.push('#');
            push_usize(path, index);
        }
    }
}

/// Encodes a single user key into its canonical path segment (escaping ambiguous keys).
#[doc(hidden)]
pub fn path_segment(key: &str) -> String {
    let mut seg = String::with_capacity(key.len() + if key_needs_escape(key) { 8 } else { 0 });
    push_segment(&mut seg, Some(key), 0);
    seg
}

/// Joins an encoded `parent` path with a keyed child segment using the runtime's exact path encoding.
#[doc(hidden)]
pub fn child_path(parent: &str, key: &str) -> String {
    node_path(parent, Some(key), 0)
}

/// Build the owned path directly, without cloning a key or formatting an index into a temporary
/// segment first. Measurement and flattening must use the same identity rule.
pub(in crate::runtime) fn node_path(parent: &str, key: Option<&str>, index: usize) -> String {
    let extra = match key {
        Some(k) => k.len() + if key_needs_escape(k) { 8 } else { 0 },
        None => 21,
    };
    let mut path = String::with_capacity(parent.len() + usize::from(!parent.is_empty()) + extra);
    path.push_str(parent);
    push_segment(&mut path, key, index);
    path
}

/// Decodes one keyed path segment back to the user's key, returning `None` for unkeyed `#<n>`
/// segments or malformed multi-segment inputs.
pub(crate) fn decode_key_segment(seg: &str) -> Option<Cow<'_, str>> {
    if seg.is_empty() || seg.as_bytes().contains(&b'/') || is_unkeyed_segment(seg) {
        return None;
    }
    let Some(body) = seg.strip_prefix(ESCAPE_PREFIX) else {
        return Some(Cow::Borrowed(seg));
    };
    if !body.as_bytes().contains(&b'%') {
        return Some(Cow::Borrowed(body));
    }
    let mut out = String::with_capacity(body.len());
    let bytes = body.as_bytes();
    let mut i = 0;
    let mut start = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let ch = match (bytes[i + 1], bytes[i + 2]) {
                (b'2', b'F') => Some('/'),
                (b'2', b'5') => Some('%'),
                _ => None,
            };
            if let Some(c) = ch {
                out.push_str(&body[start..i]);
                out.push(c);
                i += 3;
                start = i;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&body[start..]);
    Some(Cow::Owned(out))
}

/// Whether encoded path segment `seg` represents user key `key`.
#[inline]
pub(crate) fn segment_matches_key(seg: &str, key: &str) -> bool {
    if !key_needs_escape(key) {
        return seg == key;
    }
    seg.starts_with(ESCAPE_PREFIX) && decode_key_segment(seg).is_some_and(|k| k == key)
}

/// Whether `path` lies inside an element keyed `scope` (always, without one).
pub(crate) fn in_scope(path: &str, scope: Option<&str>) -> bool {
    scope.is_none_or(|s| path.split('/').any(|seg| segment_matches_key(seg, s)))
}

/// Whether `path`'s keyed segments (ignoring unkeyed `#n` indices) end with a contiguous suffix
/// of decoded keys whose `/`-join equals `selector`: `slider/knob` matches
/// `form/#2/slider/#0/knob`, and `row/0/field` matches `#0/rows/#!row%2F0/field`.
pub(crate) fn keyed_suffix_matches(path: &str, selector: &str) -> bool {
    if selector.is_empty() {
        return false;
    }
    let mut rest = selector;
    for seg in path.rsplit('/') {
        let Some(k) = decode_key_segment(seg) else { continue };
        let Some(head) = rest.strip_suffix(k.as_ref()) else { return false };
        if head.is_empty() {
            return true;
        }
        let Some(head) = head.strip_suffix('/') else { return false };
        rest = head;
    }
    false
}

/// The decoded keyed segments of `path`, joined by `/` (skipping unkeyed `#n` segments).
pub(in crate::runtime) fn keyed_scope(path: &str) -> String {
    let mut out = String::new();
    for seg in path.split('/') {
        let Some(k) = decode_key_segment(seg) else { continue };
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(&k);
    }
    out
}
