//! CSS Color Module Level 4 parsing and `#rrggbb` hex normalization for [`Mark::Color`](crate::Mark::Color).

/// Normalize a `#rgb` or `#rrggbb` hex colour (trimmed, ASCII case-insensitive) to lowercase
/// `#rrggbb`. Returns `None` for any other input.
pub(crate) fn normalize_hex(raw: &str) -> Option<String> {
    let s = raw.trim();
    let hex = s.strip_prefix('#')?;
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match hex.len() {
        3 => {
            let b = hex.as_bytes();
            let mut out = String::with_capacity(7);
            out.push('#');
            for &ch in b {
                let lo = ch.to_ascii_lowercase() as char;
                out.push(lo);
                out.push(lo);
            }
            Some(out)
        }
        6 => {
            let mut out = String::with_capacity(7);
            out.push('#');
            for &ch in hex.as_bytes() {
                out.push(ch.to_ascii_lowercase() as char);
            }
            Some(out)
        }
        _ => None,
    }
}

/// Parse a CSS `color` value into normalized lowercase `#rrggbb`.
///
/// Accepts CSS Color Module Level 4 hex (`#rgb`, `#rrggbb`, opaque/translucent `#rgba` and
/// `#rrggbbaa`), `rgb()`/`rgba()` (comma or space syntax, numbers or percentages),
/// `hsl()`/`hsla()`, and all 148 standard CSS named colours, with optional trailing `!important`.
/// Returns `None` for `currentColor`, `transparent`, `var(...)`, fully transparent alpha (`0`),
/// keywords like `inherit`/`initial`/`unset`, and invalid syntax.
pub(crate) fn parse_css_color(raw: &str) -> Option<String> {
    let s = strip_important(raw.trim());
    if s.is_empty() {
        return None;
    }
    if let Some(hex) = s.strip_prefix('#') {
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        return match hex.len() {
            3 | 6 => normalize_hex(s),
            4 => {
                let b = hex.as_bytes();
                if b[3] == b'0' {
                    return None;
                }
                let mut out = String::with_capacity(7);
                out.push('#');
                for &ch in &b[..3] {
                    let lo = ch.to_ascii_lowercase() as char;
                    out.push(lo);
                    out.push(lo);
                }
                Some(out)
            }
            8 => {
                let b = hex.as_bytes();
                if b[6] == b'0' && b[7] == b'0' {
                    return None;
                }
                let mut out = String::with_capacity(7);
                out.push('#');
                for &ch in &b[..6] {
                    out.push(ch.to_ascii_lowercase() as char);
                }
                Some(out)
            }
            _ => None,
        };
    }

    if let Some(open) = s.find('(') {
        if !s.ends_with(')') {
            return None;
        }
        let func = s[..open].trim();
        let inner = s[open + 1..s.len() - 1].trim();
        if inner.is_empty() || inner.contains('(') || inner.contains(')') {
            return None;
        }
        if func.eq_ignore_ascii_case("rgb") || func.eq_ignore_ascii_case("rgba") {
            return parse_rgb_fn(inner);
        }
        if func.eq_ignore_ascii_case("hsl") || func.eq_ignore_ascii_case("hsla") {
            return parse_hsl_fn(inner);
        }
        return None;
    }

    lookup_named_color(s).map(str::to_owned)
}

/// Parse a legacy HTML `<font color="...">` attribute value into lowercase `#rrggbb`.
/// Accepts CSS colours plus bare 3- or 6-digit hex strings without a leading `#`.
pub(crate) fn parse_font_color(raw: &str) -> Option<String> {
    let s = raw.trim();
    if let Some(c) = parse_css_color(s) {
        return Some(c);
    }
    if matches!(s.len(), 3 | 6) && s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return normalize_hex(&format!("#{s}"));
    }
    None
}

fn strip_important(s: &str) -> &str {
    let Some(bang) = s.rfind('!') else { return s };
    if s[bang + 1..].trim().eq_ignore_ascii_case("important") {
        s[..bang].trim_end()
    } else {
        s
    }
}

/// Split a CSS functional colour argument list into `([c0, c1, c2], optional_alpha)`,
/// supporting both legacy comma-separated (`a, b, c[, alpha]`) and CSS Color 4 space-separated
/// (`a b c [/ alpha]`) forms without mixing them.
fn split_color_args(inner: &str) -> Option<([&str; 3], Option<&str>)> {
    if inner.contains(',') {
        if inner.contains('/') {
            return None;
        }
        let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
        if parts.iter().any(|p| p.is_empty()) {
            return None;
        }
        match parts.as_slice() {
            [a, b, c] => Some(([a, b, c], None)),
            [a, b, c, alpha] => Some(([a, b, c], Some(alpha))),
            _ => None,
        }
    } else {
        let (rgb_part, alpha_part) = match inner.split_once('/') {
            Some((left, right)) => {
                if right.contains('/') {
                    return None;
                }
                let a = right.trim();
                if a.is_empty() {
                    return None;
                }
                (left.trim(), Some(a))
            }
            None => (inner, None),
        };
        let parts: Vec<&str> = rgb_part.split_whitespace().collect();
        match parts.as_slice() {
            [a, b, c] => Some(([a, b, c], alpha_part)),
            _ => None,
        }
    }
}

fn parse_alpha(tok: &str) -> Option<f32> {
    let v = if let Some(pct) = tok.strip_suffix('%') {
        let n: f32 = pct.parse().ok()?;
        if !n.is_finite() {
            return None;
        }
        (n / 100.0).clamp(0.0, 1.0)
    } else {
        let n: f32 = tok.parse().ok()?;
        if !n.is_finite() {
            return None;
        }
        n.clamp(0.0, 1.0)
    };
    Some(v)
}

fn parse_rgb_channel(tok: &str) -> Option<u8> {
    let v = if let Some(pct) = tok.strip_suffix('%') {
        let n: f32 = pct.parse().ok()?;
        if !n.is_finite() {
            return None;
        }
        (n * 2.55).clamp(0.0, 255.0).round() as u8
    } else {
        let n: f32 = tok.parse().ok()?;
        if !n.is_finite() {
            return None;
        }
        n.clamp(0.0, 255.0).round() as u8
    };
    Some(v)
}

fn parse_rgb_fn(inner: &str) -> Option<String> {
    let ([rs, gs, bs], alpha) = split_color_args(inner)?;
    if let Some(a) = alpha
        && parse_alpha(a)? <= 0.0
    {
        return None;
    }
    let r = parse_rgb_channel(rs)?;
    let g = parse_rgb_channel(gs)?;
    let b = parse_rgb_channel(bs)?;
    Some(format!("#{r:02x}{g:02x}{b:02x}"))
}

fn parse_hue_deg(tok: &str) -> Option<f32> {
    let lower = tok.to_ascii_lowercase();
    let deg = if let Some(v) = lower.strip_suffix("deg") {
        v.parse::<f32>().ok()?
    } else if let Some(v) = lower.strip_suffix("grad") {
        v.parse::<f32>().ok()? * 0.9
    } else if let Some(v) = lower.strip_suffix("rad") {
        v.parse::<f32>().ok()?.to_degrees()
    } else if let Some(v) = lower.strip_suffix("turn") {
        v.parse::<f32>().ok()? * 360.0
    } else {
        lower.parse::<f32>().ok()?
    };
    if !deg.is_finite() {
        return None;
    }
    Some(deg.rem_euclid(360.0))
}

fn parse_percent_unit(tok: &str) -> Option<f32> {
    let n: f32 = tok.strip_suffix('%')?.parse().ok()?;
    if !n.is_finite() {
        return None;
    }
    Some((n / 100.0).clamp(0.0, 1.0))
}

fn parse_hsl_fn(inner: &str) -> Option<String> {
    let ([hs, ss, ls], alpha) = split_color_args(inner)?;
    if let Some(a) = alpha
        && parse_alpha(a)? <= 0.0
    {
        return None;
    }
    let h = parse_hue_deg(hs)?;
    let s = parse_percent_unit(ss)?;
    let l = parse_percent_unit(ls)?;
    // CSS Color Module Level 4 hslToRgb.
    let f = |n: f32| -> u8 {
        let k = (n + h / 30.0) % 12.0;
        let a = s * l.min(1.0 - l);
        let c = l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0);
        (c * 255.0).clamp(0.0, 255.0).round() as u8
    };
    Some(format!("#{:02x}{:02x}{:02x}", f(0.0), f(8.0), f(4.0)))
}

fn lookup_named_color(name: &str) -> Option<&'static str> {
    if !name.is_ascii() || name.len() > 20 {
        return None;
    }
    let mut buf = [0u8; 20];
    for (dst, &src) in buf.iter_mut().zip(name.as_bytes()) {
        *dst = src.to_ascii_lowercase();
    }
    let key = std::str::from_utf8(&buf[..name.len()]).ok()?;
    NAMED_COLORS
        .binary_search_by_key(&key, |&(k, _)| k)
        .ok()
        .map(|i| NAMED_COLORS[i].1)
}

/// All 148 standard CSS Color Module Level 4 named colours, sorted by ASCII lowercase name.
pub(crate) const NAMED_COLORS: &[(&str, &str)] = &[
    ("aliceblue", "#f0f8ff"),
    ("antiquewhite", "#faebd7"),
    ("aqua", "#00ffff"),
    ("aquamarine", "#7fffd4"),
    ("azure", "#f0ffff"),
    ("beige", "#f5f5dc"),
    ("bisque", "#ffe4c4"),
    ("black", "#000000"),
    ("blanchedalmond", "#ffebcd"),
    ("blue", "#0000ff"),
    ("blueviolet", "#8a2be2"),
    ("brown", "#a52a2a"),
    ("burlywood", "#deb887"),
    ("cadetblue", "#5f9ea0"),
    ("chartreuse", "#7fff00"),
    ("chocolate", "#d2691e"),
    ("coral", "#ff7f50"),
    ("cornflowerblue", "#6495ed"),
    ("cornsilk", "#fff8dc"),
    ("crimson", "#dc143c"),
    ("cyan", "#00ffff"),
    ("darkblue", "#00008b"),
    ("darkcyan", "#008b8b"),
    ("darkgoldenrod", "#b8860b"),
    ("darkgray", "#a9a9a9"),
    ("darkgreen", "#006400"),
    ("darkgrey", "#a9a9a9"),
    ("darkkhaki", "#bdb76b"),
    ("darkmagenta", "#8b008b"),
    ("darkolivegreen", "#556b2f"),
    ("darkorange", "#ff8c00"),
    ("darkorchid", "#9932cc"),
    ("darkred", "#8b0000"),
    ("darksalmon", "#e9967a"),
    ("darkseagreen", "#8fbc8f"),
    ("darkslateblue", "#483d8b"),
    ("darkslategray", "#2f4f4f"),
    ("darkslategrey", "#2f4f4f"),
    ("darkturquoise", "#00ced1"),
    ("darkviolet", "#9400d3"),
    ("deeppink", "#ff1493"),
    ("deepskyblue", "#00bfff"),
    ("dimgray", "#696969"),
    ("dimgrey", "#696969"),
    ("dodgerblue", "#1e90ff"),
    ("firebrick", "#b22222"),
    ("floralwhite", "#fffaf0"),
    ("forestgreen", "#228b22"),
    ("fuchsia", "#ff00ff"),
    ("gainsboro", "#dcdcdc"),
    ("ghostwhite", "#f8f8ff"),
    ("gold", "#ffd700"),
    ("goldenrod", "#daa520"),
    ("gray", "#808080"),
    ("green", "#008000"),
    ("greenyellow", "#adff2f"),
    ("grey", "#808080"),
    ("honeydew", "#f0fff0"),
    ("hotpink", "#ff69b4"),
    ("indianred", "#cd5c5c"),
    ("indigo", "#4b0082"),
    ("ivory", "#fffff0"),
    ("khaki", "#f0e68c"),
    ("lavender", "#e6e6fa"),
    ("lavenderblush", "#fff0f5"),
    ("lawngreen", "#7cfc00"),
    ("lemonchiffon", "#fffacd"),
    ("lightblue", "#add8e6"),
    ("lightcoral", "#f08080"),
    ("lightcyan", "#e0ffff"),
    ("lightgoldenrodyellow", "#fafad2"),
    ("lightgray", "#d3d3d3"),
    ("lightgreen", "#90ee90"),
    ("lightgrey", "#d3d3d3"),
    ("lightpink", "#ffb6c1"),
    ("lightsalmon", "#ffa07a"),
    ("lightseagreen", "#20b2aa"),
    ("lightskyblue", "#87cefa"),
    ("lightslategray", "#778899"),
    ("lightslategrey", "#778899"),
    ("lightsteelblue", "#b0c4de"),
    ("lightyellow", "#ffffe0"),
    ("lime", "#00ff00"),
    ("limegreen", "#32cd32"),
    ("linen", "#faf0e6"),
    ("magenta", "#ff00ff"),
    ("maroon", "#800000"),
    ("mediumaquamarine", "#66cdaa"),
    ("mediumblue", "#0000cd"),
    ("mediumorchid", "#ba55d3"),
    ("mediumpurple", "#9370db"),
    ("mediumseagreen", "#3cb371"),
    ("mediumslateblue", "#7b68ee"),
    ("mediumspringgreen", "#00fa9a"),
    ("mediumturquoise", "#48d1cc"),
    ("mediumvioletred", "#c71585"),
    ("midnightblue", "#191970"),
    ("mintcream", "#f5fffa"),
    ("mistyrose", "#ffe4e1"),
    ("moccasin", "#ffe4b5"),
    ("navajowhite", "#ffdead"),
    ("navy", "#000080"),
    ("oldlace", "#fdf5e6"),
    ("olive", "#808000"),
    ("olivedrab", "#6b8e23"),
    ("orange", "#ffa500"),
    ("orangered", "#ff4500"),
    ("orchid", "#da70d6"),
    ("palegoldenrod", "#eee8aa"),
    ("palegreen", "#98fb98"),
    ("paleturquoise", "#afeeee"),
    ("palevioletred", "#db7093"),
    ("papayawhip", "#ffefd5"),
    ("peachpuff", "#ffdab9"),
    ("peru", "#cd853f"),
    ("pink", "#ffc0cb"),
    ("plum", "#dda0dd"),
    ("powderblue", "#b0e0e6"),
    ("purple", "#800080"),
    ("rebeccapurple", "#663399"),
    ("red", "#ff0000"),
    ("rosybrown", "#bc8f8f"),
    ("royalblue", "#4169e1"),
    ("saddlebrown", "#8b4513"),
    ("salmon", "#fa8072"),
    ("sandybrown", "#f4a460"),
    ("seagreen", "#2e8b57"),
    ("seashell", "#fff5ee"),
    ("sienna", "#a0522d"),
    ("silver", "#c0c0c0"),
    ("skyblue", "#87ceeb"),
    ("slateblue", "#6a5acd"),
    ("slategray", "#708090"),
    ("slategrey", "#708090"),
    ("snow", "#fffafa"),
    ("springgreen", "#00ff7f"),
    ("steelblue", "#4682b4"),
    ("tan", "#d2b48c"),
    ("teal", "#008080"),
    ("thistle", "#d8bfd8"),
    ("tomato", "#ff6347"),
    ("turquoise", "#40e0d0"),
    ("violet", "#ee82ee"),
    ("wheat", "#f5deb3"),
    ("white", "#ffffff"),
    ("whitesmoke", "#f5f5f5"),
    ("yellow", "#ffff00"),
    ("yellowgreen", "#9acd32"),
];
