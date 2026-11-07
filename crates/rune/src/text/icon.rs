//! Icons: a subset of SVG (path, rect, circle, ellipse, line; fill/stroke, caps, joins, evenodd,
//! and `opacity`, `fill-opacity`, `stroke-opacity` on each element).

use swash::zeno::{Cap, Fill, Join, Mask, Stroke, Transform};

use super::{Atlas, Rasterized, svg_path};

/// An SVG icon body as found in `@opencode/ui`'s catalog: elements inside a square viewBox,
/// drawn in its element's colour. A body that names its own colours (`fill="#…"`, gradients),
/// such as a file-type badge, draws in them instead, as full SVG (groups, transforms, `use`).
#[derive(Debug)]
pub struct Icon {
    pub name: &'static str,
    pub view: f32,
    pub body: &'static str,
}

fn attr<'a>(el: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = el;
    loop {
        let i = rest.find(name)?;
        let before = rest[..i].chars().last();
        let after = &rest[i + name.len()..];
        if before.is_some_and(|c| c.is_whitespace()) && after.starts_with("=\"") {
            let v = &after[2..];
            return Some(&v[..v.find('"')?]);
        }
        rest = &rest[i + name.len()..];
    }
}

fn num(el: &str, name: &str) -> f32 {
    attr(el, name).and_then(|v| v.trim_end_matches("px").parse().ok()).unwrap_or(0.0)
}

/// SVG path data for one icon element, in viewBox units.
fn element_path(el: &str) -> Option<String> {
    let tag = el.trim_start_matches('<').split_whitespace().next()?;
    Some(match tag {
        "path" => attr(el, "d")?.to_owned(),
        "rect" => {
            let (x, y, w, h) = (num(el, "x"), num(el, "y"), num(el, "width"), num(el, "height"));
            let r = num(el, "rx").min(w / 2.0).min(h / 2.0);
            if r <= 0.0 {
                format!("M{x} {y}h{w}v{h}h{}Z", -w)
            } else {
                format!(
                    "M{} {y}h{}a{r} {r} 0 0 1 {r} {r}v{}a{r} {r} 0 0 1 {} {r}h{}a{r} {r} 0 0 1 {} {}v{}a{r} {r} 0 0 1 {r} {}Z",
                    x + r,
                    w - 2.0 * r,
                    h - 2.0 * r,
                    -r,
                    -(w - 2.0 * r),
                    -r,
                    -r,
                    -(h - 2.0 * r),
                    -r
                )
            }
        }
        "circle" | "ellipse" => {
            let (cx, cy) = (num(el, "cx"), num(el, "cy"));
            let (rx, ry) = if tag == "circle" { (num(el, "r"), num(el, "r")) } else { (num(el, "rx"), num(el, "ry")) };
            format!(
                "M{} {cy}a{rx} {ry} 0 1 0 {} 0a{rx} {ry} 0 1 0 {} 0Z",
                cx - rx,
                2.0 * rx,
                -2.0 * rx
            )
        }
        "line" => format!("M{} {}L{} {}", num(el, "x1"), num(el, "y1"), num(el, "x2"), num(el, "y2")),
        _ => return None,
    })
}

pub(super) fn rasterize_icon(icon: &Icon, px: u32, atlas: &mut Atlas) -> Rasterized {
    if px == 0 {
        return Rasterized::Empty;
    }
    if px + 4 > super::ATLAS_SIZE {
        return Rasterized::Unplaced;
    }
    let cov = icon_coverage(icon, px);
    if !cov.iter().any(|&c| c != 0) {
        return Rasterized::Empty;
    }
    // Pad by one transparent texel on every side so sub-pixel, scaled and turned quads can
    // expand by a texel and filter out to zero without clipping or sampling a neighbour.
    let stride = (px + 2) as usize;
    let mut padded = vec![0u8; stride * stride];
    for row in 0..px as usize {
        let src = &cov[row * px as usize..(row + 1) * px as usize];
        let dst = (row + 1) * stride + 1;
        padded[dst..dst + px as usize].copy_from_slice(src);
    }
    match atlas.put(px + 2, px + 2, &padded, 0, 0) {
        Some(g) => Rasterized::Placed(g),
        None => Rasterized::Unplaced,
    }
}

/// `icon`'s coverage at `px` × `px` device pixels, row-major, as the glyph atlas stores it.
pub fn icon_coverage(icon: &Icon, px: u32) -> Vec<u8> {
    let s = px as f32 / icon.view;
    let mut acc = vec![0u8; (px * px) as usize];
    let mut buf = vec![0u8; (px * px) as usize];
    for raw in icon.body.split('>') {
        let el = raw.trim();
        if el.is_empty() || el.starts_with("</") {
            continue;
        }
        let Some(d) = element_path(el) else { continue };
        let d = svg_path::parse(&d);
        let stroke = attr(el, "stroke").is_some_and(|v| v != "none");
        let fill = attr(el, "fill").map_or(!stroke, |v| v != "none");
        let opacity = |name: &str| attr(el, name).and_then(|v| v.trim().parse::<f32>().ok()).map_or(1.0, |v| v.clamp(0.0, 1.0));
        let whole = opacity("opacity");
        let mut paint = |style: swash::zeno::Style<'_>, alpha: f32| {
            buf.iter_mut().for_each(|b| *b = 0);
            Mask::new(d.as_slice())
                .style(style)
                .transform(Some(Transform::scale(s, s)))
                .size(px, px)
                .render_into(&mut buf, None);
            for (a, &b) in acc.iter_mut().zip(buf.iter()) {
                let (fa, fb) = (*a as u32, (b as f32 * alpha).round() as u32);
                *a = (fa + fb - fa * fb / 255).min(255) as u8;
            }
        };
        if fill {
            let rule = if attr(el, "fill-rule") == Some("evenodd") { Fill::EvenOdd } else { Fill::NonZero };
            paint(rule.into(), whole * opacity("fill-opacity"));
        }
        if stroke {
            let w = attr(el, "stroke-width").and_then(|v| v.parse().ok()).unwrap_or(1.0f32);
            // The transform scales the width with the geometry.
            let mut st = Stroke::new(w);
            st.cap(match attr(el, "stroke-linecap") {
                Some("round") => Cap::Round,
                Some("square") => Cap::Square,
                _ => Cap::Butt,
            });
            st.join(match attr(el, "stroke-linejoin") {
                Some("round") => Join::Round,
                Some("bevel") => Join::Bevel,
                _ => Join::Miter,
            });
            paint(st.into(), whole * opacity("stroke-opacity"));
        }
    }
    acc
}


#[cfg(test)]
#[path = "icon_tests.rs"]
mod tests;
