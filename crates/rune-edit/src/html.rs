//! Safe HTML subset import and export for [`TextEdit`](super::TextEdit).
//!
//! Supported elements:
//! - Inline: `<b>`, `<strong>`, `<i>`, `<em>`, `<u>`, `<ins>`, `<s>`, `<del>`, `<strike>`,
//!   `<a href="...">`, `<code>`, `<font color="...">`, CSS `style` attributes (`font-weight`,
//!   `font-style`, `text-decoration`, `text-decoration-line`, `color`), and `<span data-pill="id">`
//!   for [`Mark::Pill`].
//! - Block: `<p>`, `<div>`, `<br>`, `<ul>`, `<ol>`, `<li>`, `<blockquote>`, `<pre>`.
//! - Stripped with content: `<script>`, `<style>`, `<head>`.
//! - Unknown tags drop their wrapper and keep their text. Nothing is executed, and dangerous
//!   link schemes (`javascript:`, `vbscript:`, `data:`) are rejected.

use super::Mark;

pub(crate) mod color;
mod parse;
mod serialize;

pub(crate) use color::normalize_hex;
pub(crate) use parse::parse_html;
pub(crate) use serialize::serialize_html;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ListKind {
    Bullet,
    Numbered,
}

/// Safe URL check for `<a href="...">`: allows `http:`, `https:`, `mailto:` and `tel:`, and
/// scheme-less URLs. Browsers drop ASCII tabs and newlines anywhere in a URL and leading
/// controls and spaces, so `java\tscript:` is checked as `javascript:`; anything else with a
/// scheme (`javascript:`, `data:`, `file:`, app-specific schemes) is rejected.
fn is_safe_href(url: &str) -> bool {
    let cleaned: String = url
        .chars()
        .filter(|c| !c.is_control() && !c.is_whitespace())
        .take(64)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if cleaned.is_empty() {
        return false;
    }
    let scheme_len = cleaned
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
        .unwrap_or(cleaned.len());
    if !cleaned[scheme_len..].starts_with(':') {
        // No scheme: a relative path, `www.example.com` or `#fragment`.
        return true;
    }
    matches!(&cleaned[..scheme_len], "http" | "https" | "mailto" | "tel")
}

/// Decodes HTML entities (`&amp;`, `&lt;`, `&gt;`, `&quot;`, `&apos;`, `&nbsp;`, `&#123;`, `&#x1F600;`).
pub(crate) fn decode_entities(input: &str) -> String {
    if !input.contains('&') {
        return input.to_owned();
    }
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp + 1..];
        // Entities are short: look for `;` in the next 33 bytes only, so a run of bare `&`s
        // stays linear.
        let window = &rest.as_bytes()[..rest.len().min(33)];
        let Some(semi) = window.iter().position(|&b| b == b';') else {
            out.push('&');
            continue;
        };
        let entity = &rest[..semi];
        if entity.is_empty() || entity.contains(char::is_whitespace) {
            out.push('&');
            continue;
        }
        let decoded = if let Some(hex) = entity.strip_prefix("#x").or_else(|| entity.strip_prefix("#X")) {
            u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
        } else if let Some(dec) = entity.strip_prefix('#') {
            dec.parse::<u32>().ok().and_then(char::from_u32)
        } else {
            match entity {
                "amp" | "AMP" => Some('&'),
                "lt" | "LT" => Some('<'),
                "gt" | "GT" => Some('>'),
                "quot" | "QUOT" => Some('"'),
                "apos" | "#39" => Some('\''),
                "nbsp" | "NBSP" => Some('\u{00a0}'),
                "copy" => Some('©'),
                "mdash" => Some('—'),
                "ndash" => Some('–'),
                "hellip" => Some('…'),
                "lsquo" => Some('‘'),
                "rsquo" => Some('’'),
                "ldquo" => Some('“'),
                "rdquo" => Some('”'),
                "bull" => Some('•'),
                // Browsers decode these in an `href` too, so `javascript&colon;` is still a scheme.
                "colon" => Some(':'),
                "Tab" => Some('\t'),
                "NewLine" => Some('\n'),
                _ => None,
            }
        };
        if let Some(ch) = decoded {
            out.push(ch);
            rest = &rest[semi + 1..];
        } else {
            out.push('&');
        }
    }
    out.push_str(rest);
    out
}

fn escape_html(s: &str, out: &mut String) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
}

/// Extracts an HTML attribute's value from a tag tail (`href="..."`, `style='...'`, `href=foo`).
fn extract_attr(attrs: &str, name: &str) -> Option<String> {
    let bytes = attrs.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let key_start = i;
        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'-' | b'_' | b':')) {
            i += 1;
        }
        if i == key_start {
            i += 1;
            continue;
        }
        let key = &attrs[key_start..i];
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let val = if bytes[i] == b'"' || bytes[i] == b'\'' {
            let quote = bytes[i];
            i += 1;
            let val_start = i;
            while i < bytes.len() && bytes[i] != quote {
                i += 1;
            }
            let v = &attrs[val_start..i];
            if i < bytes.len() {
                i += 1;
            }
            v
        } else {
            let val_start = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' && bytes[i] != b'/' {
                i += 1;
            }
            &attrs[val_start..i]
        };
        if key.eq_ignore_ascii_case(name) {
            return Some(decode_entities(val));
        }
    }
    None
}

/// Parses inline CSS `style="..."` for `font-weight`, `font-style`, `text-decoration`, and `color`.
fn parse_style_marks(style: &str, add: &mut Vec<Mark>, remove: &mut Vec<Mark>) {
    for decl in style.split(';') {
        let Some((prop, val)) = decl.split_once(':') else { continue };
        let prop = prop.trim();
        let val = val.trim();
        if prop.eq_ignore_ascii_case("font-weight") {
            let v = val.split_whitespace().next().unwrap_or("");
            if v.eq_ignore_ascii_case("bold")
                || v.eq_ignore_ascii_case("bolder")
                || v.parse::<f32>().is_ok_and(|w| w >= 600.0)
            {
                add.push(Mark::Bold);
            } else if v.eq_ignore_ascii_case("normal")
                || v.eq_ignore_ascii_case("lighter")
                || v.parse::<f32>().is_ok_and(|w| w < 600.0)
            {
                remove.push(Mark::Bold);
            }
        } else if prop.eq_ignore_ascii_case("font-style") {
            let v = val.split_whitespace().next().unwrap_or("");
            if v.eq_ignore_ascii_case("italic") || v.eq_ignore_ascii_case("oblique") {
                add.push(Mark::Italic);
            } else if v.eq_ignore_ascii_case("normal") {
                remove.push(Mark::Italic);
            }
        } else if prop.eq_ignore_ascii_case("text-decoration")
            || prop.eq_ignore_ascii_case("text-decoration-line")
        {
            for tok in val.split_whitespace() {
                if tok.eq_ignore_ascii_case("underline") {
                    add.push(Mark::Underline);
                } else if tok.eq_ignore_ascii_case("line-through") {
                    add.push(Mark::Strike);
                } else if tok.eq_ignore_ascii_case("none") {
                    remove.push(Mark::Underline);
                    remove.push(Mark::Strike);
                }
            }
        } else if prop.eq_ignore_ascii_case("color")
            && let Some(hex) = color::parse_css_color(val)
        {
            add.push(Mark::Color(hex));
        }
    }
}
