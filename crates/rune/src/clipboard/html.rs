//! Strips active content from HTML that came from another app, and packs/unpacks Windows
//! `CF_HTML` (`HTML Format`).
//!
//! A small tokenizer, not a parser: it keeps text and ordinary tags and drops anything that
//! can run code or load something. Where it can't tell, it drops more, never less. What it
//! returns is still untrusted input to parse, not markup to hand to a web view as is.

use std::borrow::Cow;

/// Cap rich clipboard payloads so a multi-megabyte pasteboard never stalls the worker.
pub(crate) const MAX_RICH_BYTES: usize = 4 * 1024 * 1024;

/// Elements dropped with everything inside them.
const DROP_WITH_CONTENT: &[&str] = &[
    "script", "iframe", "object", "applet", "noscript", "noembed", "noframes", "template", "frameset",
    "svg", "math", "xml", "portal", "fencedframe",
];
/// Elements dropped on their own; their content (if any) is sanitised as ordinary markup.
const DROP_TAG: &[&str] = &[
    "base", "link", "meta", "embed", "frame", "param", "form",
    "title", "textarea", "xmp", "plaintext", "listing",
];
/// Attributes that hold a URL (or, for `srcset`, a list of them).
const URL_ATTRIBUTES: &[&str] = &[
    "href", "src", "xlink:href", "background", "poster", "cite", "longdesc", "lowsrc",
    "dynsrc", "data", "codebase", "ping", "srcset", "manifest", "icon", "profile", "classid", "archive",
];
const DROP_ATTRIBUTES: &[&str] = &["srcdoc", "http-equiv", "action", "formaction"];

/// HTML from outside the app with scripts, event handlers, frames, embeds, forms, raw-text
/// elements, `javascript:` and other non-web URLs, and CSS that imports or loads anything
/// removed. Comments, doctypes and processing instructions go too. Text and ordinary
/// formatting (tags, `class`, inline `style` and `<style>` that only style) stay.
pub fn sanitize_html(raw: &str) -> String {
    let capped = clamp_utf8(raw, MAX_RICH_BYTES);
    // Strip NULs before tokenizing so `<scr\0ipt>`, `on\0error` or `u\0rl(` can't reassemble after.
    let clean_cow: Cow<'_, str> = if capped.contains('\0') {
        Cow::Owned(capped.chars().filter(|&c| c != '\0').collect())
    } else {
        Cow::Borrowed(capped)
    };
    let html = clean_cow.as_ref();
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while let Some(lt) = html[i..].find('<').map(|n| i + n) {
        out.push_str(&html[i..lt]);
        let rest = &html[lt..];
        let at = |n: usize| rest.as_bytes().get(n).copied().unwrap_or(0);
        if rest.starts_with("<!--") {
            let Some(end) = comment_end(rest) else { return out };
            i = lt + end;
        } else if rest.get(..9).is_some_and(|s| s.eq_ignore_ascii_case("<![cdata[")) {
            let Some(end) = rest[9..].find("]]>") else { return out };
            i = lt + 9 + end + 3;
        } else if rest.starts_with("<!") || rest.starts_with("<?") || (at(1) == b'/' && !at(2).is_ascii_alphabetic()) {
            let Some(end) = rest.find('>') else { return out };
            i = lt + end + 1;
        } else if at(1) == b'/' {
            let Some(tag) = Tag::parse(&rest[2..]) else { return out };
            i = lt + 2 + tag.len;
            if !dropped(&tag.name) && tag.name != "style" {
                out.push_str("</");
                out.push_str(&tag.name);
                out.push('>');
            }
        } else if at(1).is_ascii_alphabetic() {
            let Some(tag) = Tag::parse(&rest[1..]) else { return out };
            i = lt + 1 + tag.len;
            if DROP_WITH_CONTENT.contains(&tag.name.as_str()) || tag.name == "style" {
                let Some((inner, close)) = until_close(&html[i..], &tag.name) else { return out };
                if tag.name == "style" && !tag.closed && css_is_inert(inner) && css_is_inert(&decode_entities(inner)) {
                    tag.write(&mut out);
                    out.push_str(inner);
                    out.push_str("</style>");
                }
                i += close;
            } else if !DROP_TAG.contains(&tag.name.as_str()) {
                tag.write(&mut out);
            }
        } else {
            out.push_str("&lt;");
            i = lt + 1;
        }
    }
    out.push_str(&html[i..]);
    out
}

fn clamp_utf8(s: &str, max: usize) -> &str {
    if s.len() <= max { return s; }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) { end -= 1; }
    &s[..end]
}

/// Byte length of an HTML5 comment starting with `<!--`, including abrupt `<!-->` / `<!--->`
/// and `--!>` closers.
fn comment_end(rest: &str) -> Option<usize> {
    if rest.starts_with("<!-->") { return Some(5); }
    if rest.starts_with("<!--->") { return Some(6); }
    let body = rest.get(4..)?;
    let bytes = body.as_bytes();
    let mut j = 0;
    while j + 2 < bytes.len() {
        if bytes[j] == b'-' && bytes[j + 1] == b'-' {
            if bytes[j + 2] == b'>' { return Some(4 + j + 3); }
            if bytes[j + 2] == b'!' && bytes.get(j + 3) == Some(&b'>') { return Some(4 + j + 4); }
        }
        j += 1;
    }
    None
}

fn dropped(name: &str) -> bool { DROP_WITH_CONTENT.contains(&name) || DROP_TAG.contains(&name) }

/// The text before `</name`, and the byte length through that end tag's `>`. Scans in place
/// without allocating a lowercase copy of the remaining document.
fn until_close<'a>(html: &'a str, name: &str) -> Option<(&'a str, usize)> {
    let bytes = html.as_bytes();
    let needle = name.as_bytes();
    let mut i = 0;
    while let Some(rel) = bytes[i..].iter().position(|&b| b == b'<') {
        let at = i + rel;
        let after_name = at + 2 + needle.len();
        if bytes.get(at + 1) == Some(&b'/')
            && bytes.get(at + 2..after_name).is_some_and(|s| s.eq_ignore_ascii_case(needle))
        {
            let boundary = bytes.get(after_name).copied().unwrap_or(b'>');
            if !boundary.is_ascii_alphanumeric() && !matches!(boundary, b'-' | b':' | b'_') {
                let end = bytes[after_name..].iter().position(|&b| b == b'>')?;
                return Some((&html[..at], after_name + end + 1));
            }
        }
        i = at + 1;
    }
    None
}

struct Tag {
    name: String,
    attributes: Vec<(String, Option<String>)>,
    closed: bool,
    /// Bytes from the tag name through `>`.
    len: usize,
}

impl Tag {
    /// A tag starting at its name; `None` when it never ends.
    fn parse(s: &str) -> Option<Tag> {
        let b = s.as_bytes();
        let name_len = b.iter().position(|c| !(c.is_ascii_alphanumeric() || matches!(c, b'-' | b':' | b'_'))).unwrap_or(b.len());
        let name = s[..name_len].to_ascii_lowercase();
        let (mut i, mut attributes, mut closed) = (name_len, Vec::new(), false);
        loop {
            while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
                closed = b[i] == b'/';
                i += 1;
            }
            match b.get(i) {
                None => return None,
                Some(b'>') => return Some(Tag { name, attributes, closed, len: i + 1 }),
                _ => closed = false,
            }
            let start = i;
            while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'/' | b'>' | b'=') { i += 1; }
            if i == start { i += 1; } // A stray `=`.
            let attr = s[start..i].to_ascii_lowercase();
            while i < b.len() && b[i].is_ascii_whitespace() { i += 1; }
            let mut value = None;
            if b.get(i) == Some(&b'=') {
                i += 1;
                while i < b.len() && b[i].is_ascii_whitespace() { i += 1; }
                match b.get(i) {
                    Some(&q) if q == b'"' || q == b'\'' => {
                        let end = s[i + 1..].find(q as char)? + i + 1;
                        value = Some(s[i + 1..end].to_owned());
                        i = end + 1;
                    }
                    _ => {
                        let start = i;
                        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' { i += 1; }
                        value = Some(s[start..i].to_owned());
                    }
                }
            }
            if keep_attribute(&attr, value.as_deref()) { attributes.push((attr, value)); }
        }
    }

    fn write(&self, out: &mut String) {
        out.push('<');
        out.push_str(&self.name);
        for (name, value) in &self.attributes {
            out.push(' ');
            out.push_str(name);
            if let Some(value) = value {
                out.push_str("=\"");
                for c in value.chars() {
                    match c {
                        '"' => out.push_str("&quot;"),
                        '<' => out.push_str("&lt;"),
                        '>' => out.push_str("&gt;"),
                        _ => out.push(c),
                    }
                }
                out.push('"');
            }
        }
        if self.closed { out.push_str(" /"); }
        out.push('>');
    }
}

fn keep_attribute(name: &str, value: Option<&str>) -> bool {
    if name.is_empty() || !name.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b':' | b'_' | b'.')) {
        return false;
    }
    if name.starts_with("on") || DROP_ATTRIBUTES.contains(&name) { return false; }
    let value = value.unwrap_or("");
    if name == "style" { return css_is_inert(value) && css_is_inert(&decode_entities(value)); }
    if name == "srcset" { return srcset_is_safe(value); }
    if URL_ATTRIBUTES.contains(&name) { return url_is_safe(value, name == "src"); }
    true
}

/// Validates an `img`/`source` `srcset` list, where a `data:image/…;base64,…` candidate itself
/// contains a comma before its payload.
fn srcset_is_safe(raw: &str) -> bool {
    let decoded = decode_entities(raw);
    let mut rest = decoded.trim();
    if rest.is_empty() { return false; }
    while !rest.is_empty() {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
        if rest.is_empty() { break; }
        let url_end = if rest.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("data:")) {
            // A `data:` URL ends at whitespace (before a width/density descriptor) or after its payload's comma.
            let after_comma = rest.find(',').map_or(0, |c| c + 1);
            after_comma + rest[after_comma..].find(|c: char| c.is_whitespace() || c == ',').unwrap_or(rest.len() - after_comma)
        } else {
            rest.find(|c: char| c.is_whitespace() || c == ',').unwrap_or(rest.len())
        };
        if !url_is_safe(&rest[..url_end], true) { return false; }
        rest = &rest[url_end..];
        if let Some(comma) = rest.find(',') {
            rest = &rest[comma + 1..];
        } else {
            break;
        }
    }
    true
}

/// No imports, loads, bindings or scripts; nothing a CSS comment or escape could hide.
/// Checked both before and after stripping `/* … */` comments so neither `ex/**/pression(`
/// nor a `"/*"` inside a CSS string literal can smuggle `@import` or `url(`.
fn css_is_inert(css: &str) -> bool {
    let mut stripped = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        stripped.push_str(&rest[..start]);
        let Some(end) = rest[start + 2..].find("*/") else { return false };
        rest = &rest[start + 2 + end + 2..];
    }
    stripped.push_str(rest);
    css_tokens_clean(css) && css_tokens_clean(&stripped)
}

fn css_tokens_clean(css: &str) -> bool {
    let plain: String = css.chars().filter(|c| !c.is_whitespace() && !c.is_control()).collect::<String>().to_ascii_lowercase();
    !["expression(", "javascript:", "vbscript:", "data:", "@import", "url(", "image-set(", "behavior:", "-moz-binding", "</", "\\", "<"]
        .iter()
        .any(|bad| plain.contains(bad))
}

/// A relative URL, or one whose scheme is web, mail, phone or an attachment; `image` also
/// allows raster `data:` images.
fn url_is_safe(raw: &str, image: bool) -> bool {
    let url: String = decode_entities(raw).chars().filter(|c| !c.is_control() && !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
    let Some(colon) = url.find(':') else {
        // Reject un-decoded `&` entities that could hide a colon, or protocol-relative `//` / `\\`.
        return !url.starts_with("//") && !url.starts_with("\\\\") && !url.contains('&');
    };
    if url[..colon].contains(['/', '?', '#', '&']) {
        return !url.starts_with("//") && !url.starts_with("\\\\") && !url[..colon].contains('&');
    }
    match &url[..colon] {
        "http" | "https" | "mailto" | "tel" | "cid" => true,
        "data" => image && ["png", "jpeg", "jpg", "gif", "webp"].iter().any(|t| {
            let prefix = format!("data:image/{t}");
            url.starts_with(&prefix) && matches!(url.as_bytes().get(prefix.len()), Some(b';' | b','))
        }),
        _ => false,
    }
}

/// Numeric references (including arbitrarily many leading zeros) and the named ones that can
/// hide a scheme, delimiter or CSS function.
fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let body_len = rest[1..].bytes().take_while(|c| c.is_ascii_alphanumeric() || *c == b'#').count();
        let body = &rest[1..1 + body_len];
        let decoded = if let Some(num) = body.strip_prefix('#') {
            let (raw_digits, radix, prefix_len) = match num.strip_prefix(['x', 'X']) {
                Some(hex) => (hex, 16, 2),
                None => (num, 10, 1),
            };
            let digit_len = raw_digits.bytes().take_while(|&c| (c as char).is_digit(radix)).count();
            let digits = &raw_digits[..digit_len];
            if digits.is_empty() {
                None
            } else {
                let trimmed = digits.trim_start_matches('0');
                let n = if trimmed.is_empty() { Some(0) } else { u32::from_str_radix(trimmed, radix).ok() };
                let ch = n.and_then(char::from_u32).filter(|&c| c != '\0').unwrap_or('\u{fffd}');
                Some((ch, prefix_len + digit_len))
            }
        } else {
            let named = match body.to_ascii_lowercase().as_str() {
                "colon" => Some(':'), "tab" => Some('\t'), "newline" => Some('\n'), "lpar" => Some('('), "rpar" => Some(')'),
                "sol" => Some('/'), "bsol" => Some('\\'), "amp" => Some('&'), "lt" | "nvlt" => Some('<'), "gt" | "nvgt" => Some('>'),
                "quot" => Some('"'), "apos" => Some('\''), "nbsp" => Some(' '), "num" => Some('#'), "period" => Some('.'),
                "commat" => Some('@'), "semi" => Some(';'), "equals" => Some('='), "grave" => Some('`'),
                "plus" => Some('+'), "comma" => Some(','), "dollar" => Some('$'),
                _ => None,
            };
            named.map(|c| (c, body.len()))
        };
        match decoded {
            Some((c, used)) => {
                out.push(c);
                rest = &rest[1 + used..];
                if rest.starts_with(';') { rest = &rest[1..]; }
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Packs an HTML fragment into a Windows `CF_HTML` (`HTML Format`) payload whose header
/// offsets (`StartHTML`, `EndHTML`, `StartFragment`, `EndFragment`) are UTF-8 byte offsets
/// from the start of the payload, including the header. arboard writes Windows copies itself;
/// this builds payloads for [`unpack_cf_html`]'s tests.
#[cfg(test)]
pub(crate) fn pack_cf_html(fragment: &str) -> String {
    const PREFIX: &str = "<html>\r\n<body>\r\n<!--StartFragment-->";
    const SUFFIX: &str = "<!--EndFragment-->\r\n</body>\r\n</html>";
    const START_HTML: usize = 105;
    let start_frag = START_HTML + PREFIX.len();
    let end_frag = start_frag + fragment.len();
    let end_html = end_frag + SUFFIX.len();
    let out = format!(
        "Version:0.9\r\nStartHTML:{START_HTML:010}\r\nEndHTML:{end_html:010}\r\nStartFragment:{start_frag:010}\r\nEndFragment:{end_frag:010}\r\n{PREFIX}{fragment}{SUFFIX}"
    );
    debug_assert_eq!(&out.as_bytes()[start_frag..end_frag], fragment.as_bytes());
    debug_assert_eq!(out.len(), end_html);
    out
}

/// Extracts the HTML fragment (or document) from a Windows `CF_HTML` (`HTML Format`) buffer.
/// Tolerates `-1` offsets (`StartHTML:-1`, `StartFragment:-1`, etc.), `0000000000` offsets,
/// trailing NUL padding, `<!--StartFragment-->` / `<!--EndFragment-->` comments, and
/// out-of-bounds offsets from other apps.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn unpack_cf_html(raw: &[u8]) -> Option<String> {
    let len = raw.iter().position(|&b| b == 0).unwrap_or(raw.len()).min(MAX_RICH_BYTES);
    let raw = &raw[..len];
    if raw.is_empty() { return None; }
    let text = std::str::from_utf8(raw).ok()?;

    let (mut start_html, mut end_html, mut start_frag, mut end_frag) = (None, None, None, None);
    let mut header_end = 0;
    let mut saw_header = false;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.starts_with('<') { break; }
        let Some((key, val)) = trimmed.split_once(':') else { break };
        saw_header = true;
        header_end += line.len();
        let offset = val.trim().parse::<i64>().ok().and_then(|n| usize::try_from(n).ok());
        match key.trim() {
            "StartHTML" => start_html = offset,
            "EndHTML" => end_html = offset,
            "StartFragment" => start_frag = offset,
            "EndFragment" => end_frag = offset,
            _ => {}
        }
    }

    let slice = |s: usize, e: usize| {
        (s >= header_end && s <= e && e <= text.len() && text.is_char_boundary(s) && text.is_char_boundary(e)).then(|| &text[s..e])
    };
    if let (Some(s), Some(e)) = (start_frag, end_frag) && let Some(frag) = slice(s, e) && !frag.is_empty() {
        return Some(frag.to_owned());
    }
    const START_MARK: &str = "<!--StartFragment-->";
    const END_MARK: &str = "<!--EndFragment-->";
    if let Some(s) = text.find(START_MARK).map(|i| i + START_MARK.len())
        && let Some(e) = text[s..].find(END_MARK).map(|i| s + i)
    {
        return Some(text[s..e].to_owned());
    }
    if let (Some(s), Some(e)) = (start_html, end_html) && let Some(doc) = slice(s, e) && !doc.is_empty() {
        return Some(doc.to_owned());
    }
    let body = if saw_header { &text[header_end..] } else { text }.trim();
    (!body.is_empty()).then(|| body.to_owned())
}

#[cfg(test)]
#[path = "html_tests.rs"]
mod tests;
