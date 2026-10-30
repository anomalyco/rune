//! Bare GFM autolinks (`https://…`, `http://…`, `www.…`) found in inline text.

use super::*;

/// Scans `inl.text` outside inline code and existing links for bare GFM autolinks (`https://…`,
/// `http://…`, `www.…`) and wires them into `inl.links` and `inl.spans`.
pub(super) fn autolink_inline(inl: &mut Inline, image_ranges: &[(usize, usize)]) {
    if !inl.text.contains("://") && !inl.text.contains("www.") {
        return;
    }
    let mut excluded: Vec<(usize, usize)> = inl
        .spans
        .iter()
        .filter(|(_, _, st)| matches!(st, Style::Code | Style::CodePad | Style::Link))
        .map(|&(a, b, _)| (a, b))
        .chain(image_ranges.iter().copied())
        .collect();
    excluded.sort_unstable_by_key(|&(a, _)| a);

    let mut found: Vec<(usize, usize, String)> = Vec::new();
    let mut cursor = 0;
    for (ex_a, ex_b) in excluded {
        if cursor < ex_a {
            scan_bare_urls(&inl.text, cursor, ex_a, &mut found);
        }
        cursor = cursor.max(ex_b);
    }
    if cursor < inl.text.len() {
        scan_bare_urls(&inl.text, cursor, inl.text.len(), &mut found);
    }
    if found.is_empty() {
        return;
    }
    let had_links = !inl.links.is_empty();
    for (start, end, dest) in found {
        inl.links.push((start, end, dest));
        inl.spans.push((start, end, Style::Link));
    }
    if had_links {
        inl.links.sort_by_key(|l| l.0);
        // Keep `Style::Link` spans in `inl.spans` ordered by start byte to match `inl.links`.
        let mut link_ranges: Vec<(usize, usize)> = inl.links.iter().map(|&(a, b, _)| (a, b)).collect();
        let mut k = 0;
        for sp in &mut inl.spans {
            if matches!(sp.2, Style::Link) {
                (sp.0, sp.1) = link_ranges[k];
                k += 1;
            }
        }
        let _ = &mut link_ranges;
    }
}

fn scan_bare_urls(text: &str, seg_start: usize, seg_end: usize, out: &mut Vec<(usize, usize, String)>) {
    let bytes = text.as_bytes();
    let mut i = seg_start;
    while i < seg_end {
        let rest = &text[i..seg_end];
        let (prefix_len, is_www) = if rest.get(..8).is_some_and(|s| s.eq_ignore_ascii_case("https://")) {
            (8, false)
        } else if rest.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("http://")) {
            (7, false)
        } else if rest.starts_with("www.") {
            (4, true)
        } else {
            i += rest.chars().next().map_or(1, char::len_utf8);
            continue;
        };
        let valid_prev = i == 0
            || matches!(bytes[i - 1], b' ' | b'\t' | b'\n' | b'*' | b'_' | b'~' | b'[')
            || (bytes[i - 1] == b'(' && (i < 2 || bytes[i - 2] != b']'));
        if !valid_prev {
            i += prefix_len;
            continue;
        }
        let raw_len = rest
            .find(|c: char| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>'))
            .unwrap_or(rest.len());
        let mut end = i + raw_len;
        trim_gfm_autolink(text, i, prefix_len, &mut end);
        let matched = &text[i..end];
        if valid_autolink_domain(&matched[prefix_len..], is_www) {
            let dest = if is_www { format!("http://{matched}") } else { matched.to_owned() };
            out.push((i, end, dest));
            i = end;
        } else {
            i += prefix_len;
        }
    }
}

/// GFM 6.9 extended autolink path trailing-punctuation trimming.
fn trim_gfm_autolink(text: &str, start: usize, prefix_len: usize, end: &mut usize) {
    loop {
        let slice = &text[start..*end];
        if slice.len() <= prefix_len {
            break;
        }
        let Some(last) = slice.chars().next_back() else { break };
        if matches!(last, '?' | '!' | '.' | ',' | ':' | '*' | '_' | '~' | '\'' | '"') {
            *end -= last.len_utf8();
            continue;
        }
        if last == ')' {
            let body = &slice[prefix_len..];
            let opens = body.bytes().filter(|&b| b == b'(').count();
            let closes = body.bytes().filter(|&b| b == b')').count();
            if closes > opens {
                *end -= 1;
                continue;
            }
            break;
        }
        if last == ']' {
            let body = &slice[prefix_len..];
            let opens = body.bytes().filter(|&b| b == b'[').count();
            let closes = body.bytes().filter(|&b| b == b']').count();
            if closes > opens {
                *end -= 1;
                continue;
            }
            break;
        }
        if last == ';' {
            let before_semi = &slice[..slice.len() - 1];
            if let Some(amp) = before_semi.rfind('&') {
                let entity = &before_semi[amp + 1..];
                if !entity.is_empty() && entity.bytes().all(|b| b.is_ascii_alphanumeric()) && amp >= prefix_len {
                    *end = start + amp;
                    continue;
                }
            }
            *end -= 1;
            continue;
        }
        break;
    }
}

/// Validates the authority/domain portion of a bare URL after `https://`, `http://`, or `www.`.
fn valid_autolink_domain(after_prefix: &str, is_www: bool) -> bool {
    let host_end = after_prefix.find(['/', '?', '#']).unwrap_or(after_prefix.len());
    let mut host = &after_prefix[..host_end];
    if let Some((h, port)) = host.rsplit_once(':')
        && !port.is_empty()
        && port.bytes().all(|b| b.is_ascii_digit())
    {
        host = h;
    }
    if host.is_empty()
        || !host.as_bytes()[0].is_ascii_alphanumeric()
        || host.ends_with('.')
        || host.ends_with('-')
        || !host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return false;
    }
    if is_www && !host.contains('.') {
        return false;
    }
    let segments: Vec<&str> = host.split('.').collect();
    if segments.iter().any(|s| s.is_empty()) {
        return false;
    }
    // GFM 6.9: the last two domain segments must not contain underscores.
    if segments.len() >= 2 && segments[segments.len() - 2..].iter().any(|s| s.contains('_')) {
        return false;
    }
    true
}
