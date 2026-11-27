//! SVG `viewBox` resolution and `preserveAspectRatio="xMidYMid meet"` mapping for [`super::Icon`].
//!
//! Shared across all three icon paths (static monochrome, animated monochrome, and multi-colour
//! `resvg`) so non-square and non-zero-origin viewBoxes centre identically everywhere.

#[cfg(feature = "svg")]
use std::ops::Range;

use swash::zeno::Transform;

use super::anim::parse::MAX_VIEW_DIM;

const MAX_ROOT_ATTRS: usize = 64;

/// Parsed opening tag of a single root `<svg ...>` wrapper in an icon body.
pub(super) struct RootSvg<'a> {
    /// Byte offset of `<svg` in the source body.
    #[cfg(feature = "svg")]
    pub tag_start: usize,
    /// Byte offset immediately after the opening tag's `>` or `/>`.
    #[cfg(feature = "svg")]
    pub tag_end: usize,
    /// Whether the opening tag is self-closing (`<svg .../>`).
    #[cfg(feature = "svg")]
    pub self_closing: bool,
    /// Parsed attributes on the opening `<svg>` tag.
    pub attrs: Vec<SvgAttr<'a>>,
}

pub(super) struct SvgAttr<'a> {
    pub name: &'a str,
    pub value: &'a str,
    #[cfg(feature = "svg")]
    pub span: Range<usize>,
}

/// Resolves an icon's `[min_x, min_y, width, height]` viewBox: if `body` is a single root
/// `<svg ...>` carrying a `viewBox` attribute, that attribute is authoritative (an invalid or
/// zero-sized `viewBox` disables rendering and returns `None`); otherwise falls back to the square
/// `[0, 0, view, view]` box from `Icon::view`.
pub(super) fn resolve(body: &str, view: f32) -> Option<[f32; 4]> {
    if let Some(root) = parse_root_svg(body)
        && let Some(attr) = root.attrs.iter().find(|a| a.name == "viewBox")
    {
        return parse_view_box_attr(attr.value);
    }
    if view.is_finite() && view > 0.0 && view <= MAX_VIEW_DIM {
        Some([0.0, 0.0, view, view])
    } else {
        None
    }
}

/// Maps `viewBox = [min_x, min_y, width, height]` into a square `size × size` device-pixel box
/// following SVG's default `preserveAspectRatio="xMidYMid meet"`.
///
/// For the common square origin-zero box `[0, 0, v, v]`, returns `Transform::scale(size / v, size / v)`
/// directly so existing square icons produce bit-identical coverage.
pub(super) fn transform([vx, vy, vw, vh]: [f32; 4], size: f32) -> Transform {
    let (s, ox, oy) = if vw == vh {
        (size / vw, 0.0, 0.0)
    } else if vw > vh {
        let s = size / vw;
        (s, 0.0, (size - vh * s) * 0.5)
    } else {
        let s = size / vh;
        (s, (size - vw * s) * 0.5, 0.0)
    };
    let tx = if vx == 0.0 { ox } else { ox - vx * s };
    let ty = if vy == 0.0 { oy } else { oy - vy * s };
    if tx == 0.0 && ty == 0.0 {
        Transform::scale(s, s)
    } else {
        Transform::new(s, 0.0, 0.0, s, tx, ty)
    }
}

/// Builds a complete `px × px` square SVG document for `resvg` so colour icons apply the exact
/// same `xMidYMid meet` fitting and centring as monochrome icons, even when `body` is a full
/// `<svg width="..." height="..." viewBox="...">...</svg>` with non-square dimensions.
#[cfg(feature = "svg")]
pub(super) fn svg_document(body: &str, [vx, vy, vw, vh]: [f32; 4], px: u32) -> String {
    if let Some(root) = parse_root_svg(body) {
        let mut out = String::with_capacity(body.len() + 128);
        out.push_str(&body[..root.tag_start]);
        out.push_str(&format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="{px}" height="{px}" viewBox="{vx} {vy} {vw} {vh}""#,
        ));
        for attr in &root.attrs {
            if matches!(
                attr.name,
                "xmlns" | "xmlns:xlink" | "width" | "height" | "x" | "y" | "viewBox" | "preserveAspectRatio"
            ) {
                continue;
            }
            out.push(' ');
            out.push_str(&body[attr.span.clone()]);
        }
        if root.self_closing {
            out.push_str("/>");
        } else {
            out.push('>');
        }
        out.push_str(&body[root.tag_end..]);
        out
    } else {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="{px}" height="{px}" viewBox="{vx} {vy} {vw} {vh}">{body}</svg>"#
        )
    }
}

/// Parses an SVG `viewBox` attribute value: four finite numbers `min-x min-y width height`
/// separated by whitespace and/or at most one comma between adjacent numbers, with positive
/// `width` and `height` within `MAX_VIEW_DIM`.
pub(super) fn parse_view_box_attr(raw: &str) -> Option<[f32; 4]> {
    let bytes = raw.as_bytes();
    let mut i = 0;
    let mut nums = [0.0f32; 4];
    for (slot, num) in nums.iter_mut().enumerate() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if slot > 0 && i < bytes.len() && bytes[i] == b',' {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b',' {
            i += 1;
        }
        if i == start {
            return None;
        }
        let v: f32 = raw[start..i].parse().ok()?;
        if !v.is_finite() {
            return None;
        }
        *num = v;
    }
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i != bytes.len() {
        return None;
    }
    let [vx, vy, vw, vh] = nums;
    if vw <= 0.0 || vh <= 0.0 || vw > MAX_VIEW_DIM || vh > MAX_VIEW_DIM {
        return None;
    }
    Some([vx, vy, vw, vh])
}

/// Advances past ASCII whitespace, XML comments (`<!--...-->`), and processing instructions
/// (`<?...?>`). Returns `None` on an unclosed comment/PI or a `<!` declaration (DTD/CDATA).
fn skip_misc(src: &str, mut pos: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    loop {
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        let rem = &src[pos..];
        if let Some(after) = rem.strip_prefix("<!--") {
            let end = after.find("-->")?;
            pos += 4 + end + 3;
            continue;
        }
        if let Some(after) = rem.strip_prefix("<?") {
            let end = after.find("?>")?;
            pos += 2 + end + 2;
            continue;
        }
        if rem.starts_with("<!") {
            return None;
        }
        return Some(pos);
    }
}

/// Parses the opening tag of `body` if and only if `body` consists of a single root `<svg ...>`
/// element (with optional surrounding whitespace, XML declarations, and comments).
pub(super) fn parse_root_svg(body: &str) -> Option<RootSvg<'_>> {
    let tag_start = skip_misc(body, 0)?;
    let bytes = body.as_bytes();
    if !body[tag_start..].starts_with("<svg") {
        return None;
    }
    let mut i = tag_start + 4;
    let next = *bytes.get(i)?;
    if !next.is_ascii_whitespace() && next != b'>' && next != b'/' {
        return None;
    }

    let mut attrs = Vec::new();
    let (tag_end, self_closing) = loop {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            return None;
        }
        if bytes[i] == b'>' {
            break (i + 1, false);
        }
        if bytes[i] == b'/' {
            if bytes.get(i + 1) != Some(&b'>') {
                return None;
            }
            break (i + 2, true);
        }
        let attr_start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && !matches!(bytes[i], b'=' | b'>' | b'/') {
            i += 1;
        }
        let name = &body[attr_start..i];
        if name.is_empty() {
            return None;
        }
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if bytes.get(i) != Some(&b'=') {
            return None;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let &quote = bytes.get(i).filter(|&&b| b == b'"' || b == b'\'')?;
        i += 1;
        let val_start = i;
        while i < bytes.len() && bytes[i] != quote {
            i += 1;
        }
        if i >= bytes.len() {
            return None;
        }
        let value = &body[val_start..i];
        i += 1;
        if attrs.len() >= MAX_ROOT_ATTRS {
            return None;
        }
        attrs.push(SvgAttr { name, value, #[cfg(feature = "svg")] span: attr_start..i });
    };

    // Confirm no sibling elements follow this root <svg>: walk nested <svg> tags if not self-closing.
    let after_root = if self_closing { tag_end } else { find_matching_svg_close(body, tag_end)? };
    let tail = skip_misc(body, after_root)?;
    if tail != body.len() {
        return None;
    }

    Some(RootSvg { #[cfg(feature = "svg")] tag_start, #[cfg(feature = "svg")] tag_end, #[cfg(feature = "svg")] self_closing, attrs })
}

/// Starting immediately after an opening `<svg ...>`, finds the byte offset immediately after its
/// matching `</svg>`, respecting XML comments, processing instructions, quoted attribute values,
/// and nested `<svg>` elements.
fn find_matching_svg_close(body: &str, mut pos: usize) -> Option<usize> {
    let bytes = body.as_bytes();
    let mut depth = 1usize;
    while pos < bytes.len() {
        let rel = body[pos..].find('<')?;
        pos += rel;
        let rem = &body[pos..];
        if let Some(after) = rem.strip_prefix("<!--") {
            let end = after.find("-->")?;
            pos += 4 + end + 3;
            continue;
        }
        if let Some(after) = rem.strip_prefix("<?") {
            let end = after.find("?>")?;
            pos += 2 + end + 2;
            continue;
        }
        if rem.starts_with("<!") {
            return None;
        }
        if rem.starts_with("</") {
            let close = rem.find('>')?;
            let tag = rem[2..close].trim();
            pos += close + 1;
            if tag == "svg" {
                depth -= 1;
                if depth == 0 {
                    return Some(pos);
                }
            }
            continue;
        }
        // Opening or self-closing tag: scan tag name and skip quoted attribute values until `>`.
        let mut i = pos + 1;
        let name_start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' && bytes[i] != b'/' {
            i += 1;
        }
        let tag = &body[name_start..i];
        let mut self_closing = false;
        while i < bytes.len() {
            match bytes[i] {
                b'"' | b'\'' => {
                    let q = bytes[i];
                    i += 1;
                    while i < bytes.len() && bytes[i] != q {
                        i += 1;
                    }
                    if i >= bytes.len() {
                        return None;
                    }
                    i += 1;
                }
                b'/' if bytes.get(i + 1) == Some(&b'>') => {
                    self_closing = true;
                    i += 2;
                    break;
                }
                b'>' => {
                    i += 1;
                    break;
                }
                _ => i += 1,
            }
        }
        if tag == "svg" && !self_closing {
            depth += 1;
        }
        pos = i;
    }
    None
}
