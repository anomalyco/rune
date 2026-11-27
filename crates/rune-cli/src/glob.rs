//! Cross-platform POSIX-style path glob matcher supporting `*`, `?`, `[...]`, and `**`.

/// Normalizes a relative path to POSIX forward slashes without a leading `./`.
pub fn normalize_rel(path: &str) -> String {
    let mut parts = Vec::new();
    let replaced = path.replace('\\', "/");
    for seg in replaced.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|p| *p != "..") {
                    parts.pop();
                } else {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// Returns `true` if `path` matches `pattern` with segment-aware globbing:
/// `*`, `?`, and `[...]` match within one path segment, and `**` matches zero or more segments.
pub fn matches(path: &str, pattern: &str) -> bool {
    let norm_path = normalize_rel(path);
    let norm_pat = normalize_rel(pattern);
    let path_segs: Vec<&str> = if norm_path.is_empty() {
        Vec::new()
    } else {
        norm_path.split('/').collect()
    };
    let pat_segs: Vec<&str> = if norm_pat.is_empty() {
        Vec::new()
    } else {
        norm_pat.split('/').collect()
    };
    match_segments(&path_segs, &pat_segs)
}

/// Returns `true` if `path` matches any glob in `patterns`.
pub fn matches_any(path: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|pat| matches(path, pat))
}

fn match_segments(path: &[&str], pat: &[&str]) -> bool {
    if pat.is_empty() {
        return path.is_empty();
    }
    if pat[0] == "**" {
        if pat.len() == 1 {
            return true;
        }
        for skip in 0..=path.len() {
            if match_segments(&path[skip..], &pat[1..]) {
                return true;
            }
        }
        return false;
    }
    if path.is_empty() {
        return false;
    }
    match_segment(path[0], pat[0]) && match_segments(&path[1..], &pat[1..])
}

fn match_segment(text: &str, pattern: &str) -> bool {
    let t: Vec<char> = text.chars().collect();
    let p: Vec<char> = pattern.chars().collect();
    match_chars(&t, &p)
}

fn match_chars(t: &[char], p: &[char]) -> bool {
    if p.is_empty() {
        return t.is_empty();
    }
    match p[0] {
        '*' => {
            let mut next_p = 1;
            while next_p < p.len() && p[next_p] == '*' {
                next_p += 1;
            }
            for i in 0..=t.len() {
                if match_chars(&t[i..], &p[next_p..]) {
                    return true;
                }
            }
            false
        }
        '?' => !t.is_empty() && match_chars(&t[1..], &p[1..]),
        '[' => {
            if let Some((matched, rest_p)) = match_class(t.first().copied(), p) {
                matched && match_chars(&t[1..], rest_p)
            } else {
                // Unclosed '[' is treated as a literal character.
                t.first() == Some(&'[') && match_chars(&t[1..], &p[1..])
            }
        }
        ch => t.first() == Some(&ch) && match_chars(&t[1..], &p[1..]),
    }
}

fn match_class(ch: Option<char>, p: &[char]) -> Option<(bool, &[char])> {
    if p.len() < 3 || p[0] != '[' {
        return None;
    }
    let (negated, mut idx) = if p[1] == '!' || p[1] == '^' {
        (true, 2)
    } else {
        (false, 1)
    };
    let start = idx;
    let mut close = None;
    while idx < p.len() {
        if p[idx] == ']' && idx > start {
            close = Some(idx);
            break;
        }
        idx += 1;
    }
    let close = close?;
    let Some(c) = ch else {
        return Some((false, &p[close + 1..]));
    };
    let body = &p[start..close];
    let mut hit = false;
    let mut i = 0;
    while i < body.len() {
        if i + 2 < body.len() && body[i + 1] == '-' {
            if c >= body[i] && c <= body[i + 2] {
                hit = true;
            }
            i += 3;
        } else {
            if c == body[i] {
                hit = true;
            }
            i += 1;
        }
    }
    Some((if negated { !hit } else { hit }, &p[close + 1..]))
}
