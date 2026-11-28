//! Static, self-contained SVG serialization for a laid-out Mermaid [`Scene`].
//!
//! Emits plain SVG 1.1 geometry (`<rect>`, `<path>`, `<text>`/`<tspan>`) with no scripts,
//! embedded images, external fonts, or `url()` references so the output is safe for rich copy,
//! HTML/DOCX/PDF export, and lightbox save-as-SVG.

use std::fmt::Write;

use rune::{Align, Color, LineCap, TextStyle};

use super::{CARD_PAD, Label, Op, Scene, TITLE_GAP, measure, title_font};
use crate::DiagramStyle;

/// Serializes `scene` styled by `paint` into a self-contained static SVG document.
///
/// When `framed` is true (the default for `render_svg`, `Drawing::to_svg`, and
/// `DrawingModel::to_svg`), wraps the title and fitted diagram inside the rounded surface card
/// with padding and border. When false, emits only the title and fitted diagram with a tight
/// `viewBox`.
pub(crate) fn render(scene: &Scene, paint: &DiagramStyle, framed: bool) -> String {
    let title_m = scene.title.as_deref().map(|t| measure(t, title_font(paint)));
    let title_h = title_m.as_ref().map_or(0.0, |t| t.size[1] + TITLE_GAP);
    let content_w = title_m.as_ref().map_or(scene.size[0], |t| scene.size[0].max(t.size[0]));
    let content_h = scene.size[1] + title_h;
    let pad = if framed { CARD_PAD } else { 0.0 };
    let total_w = (content_w + 2.0 * pad).max(1.0);
    let total_h = (content_h + 2.0 * pad).max(1.0);

    let mut out = String::with_capacity(2048 + scene.ops.len() * 64 + scene.labels.len() * 96);
    out.push_str(r#"<svg xmlns="http://www.w3.org/2000/svg" width=""#);
    fmt_num(&mut out, total_w);
    out.push_str(r#"" height=""#);
    fmt_num(&mut out, total_h);
    out.push_str(r#"" viewBox="0 0 "#);
    fmt_num(&mut out, total_w);
    out.push(' ');
    fmt_num(&mut out, total_h);
    out.push_str("\">\n");

    if framed {
        let bw = 0.5f32;
        out.push_str(r#"  <rect x="0.25" y="0.25" width=""#);
        fmt_num(&mut out, (total_w - bw).max(0.0));
        out.push_str(r#"" height=""#);
        fmt_num(&mut out, (total_h - bw).max(0.0));
        out.push_str(r#"" rx="6" ry="6""#);
        push_paint_attr(&mut out, "fill", paint.surface);
        if paint.surface_border.a > 1e-4 {
            push_paint_attr(&mut out, "stroke", paint.surface_border);
            out.push_str(r#" stroke-width="0.5""#);
        }
        out.push_str("/>\n");
    }

    if let Some(t) = &title_m {
        let lbl = Label {
            text: t.text.clone(),
            at: [(total_w - t.size[0]) * 0.5, pad],
            size: t.size,
            font: title_font(paint),
            color: paint.text,
            wrap: t.wrap,
            align: Align::Center,
        };
        emit_label(&mut out, "  ", &lbl);
    }

    let dx = (total_w - scene.size[0]) * 0.5;
    let dy = pad + title_h;
    let scaled = (scene.scale - 1.0).abs() > 1e-4;
    let shifted = dx.abs() > 1e-3 || dy.abs() > 1e-3;
    out.push_str("  <g");
    if shifted || scaled {
        out.push_str(r#" transform=""#);
        if shifted {
            out.push_str("translate(");
            fmt_num(&mut out, dx);
            out.push(' ');
            fmt_num(&mut out, dy);
            out.push(')');
        }
        if scaled {
            if shifted {
                out.push(' ');
            }
            out.push_str("scale(");
            fmt_num(&mut out, scene.scale);
            out.push(')');
        }
        out.push('"');
    }
    out.push_str(">\n");

    let [ox, oy] = scene.ops_offset;
    for op in &scene.ops {
        emit_op(&mut out, op, ox, oy);
    }
    for l in &scene.labels {
        emit_label(&mut out, "    ", l);
    }

    out.push_str("  </g>\n</svg>\n");
    out
}

fn emit_op(out: &mut String, op: &Op, ox: f32, oy: f32) {
    match op {
        Op::Fill(points, color) => {
            if points.len() < 3 || color.a <= 1e-4 {
                return;
            }
            out.push_str(r#"    <path d=""#);
            push_points(out, points, true, ox, oy);
            out.push('"');
            push_paint_attr(out, "fill", *color);
            out.push_str("/>\n");
        }
        Op::Stroke { points, closed, width, cap, color } => {
            if points.len() < 2 || !width.is_finite() || *width <= 0.0 || color.a <= 1e-4 {
                return;
            }
            out.push_str(r#"    <path d=""#);
            push_points(out, points, *closed, ox, oy);
            out.push_str(r#"" fill="none""#);
            push_paint_attr(out, "stroke", *color);
            out.push_str(r#" stroke-width=""#);
            fmt_num(out, *width);
            out.push_str(r#"" stroke-linecap=""#);
            out.push_str(match cap {
                LineCap::Butt => "butt",
                LineCap::Round => "round",
            });
            out.push_str(r#"" stroke-linejoin="round"/>"#);
            out.push('\n');
        }
    }
}

fn push_points(out: &mut String, points: &[[f32; 2]], closed: bool, ox: f32, oy: f32) {
    for (i, &[x, y]) in points.iter().enumerate() {
        out.push(if i == 0 { 'M' } else { 'L' });
        fmt_num(out, x + ox);
        out.push(' ');
        fmt_num(out, y + oy);
        if i + 1 < points.len() {
            out.push(' ');
        }
    }
    if closed {
        out.push_str(" Z");
    }
}

fn emit_label(out: &mut String, indent: &str, l: &Label) {
    if l.text.is_empty() || l.color.a <= 1e-4 {
        return;
    }
    let lines = rune::text::measure_lines(&l.text, l.font, l.wrap.then_some(l.size[0] + 2.0));
    if lines.is_empty() {
        return;
    }
    let (anchor_x, anchor) = match l.align {
        Align::Center => (l.at[0] + l.size[0] * 0.5, "middle"),
        Align::End => (l.at[0] + l.size[0], "end"),
        Align::Start => (l.at[0], "start"),
    };
    let family = svg_font_family(l.font.family);

    if lines.len() == 1 {
        let (ref range, baseline) = lines[0];
        let text = l.text.get(range.clone()).unwrap_or("");
        if text.is_empty() {
            return;
        }
        out.push_str(indent);
        out.push_str(r#"<text x=""#);
        fmt_num(out, anchor_x);
        out.push_str(r#"" y=""#);
        fmt_num(out, l.at[1] + baseline);
        out.push('"');
        push_text_attrs(out, l.color, l.font, &family, anchor);
        out.push('>');
        push_xml_escaped(out, text);
        out.push_str("</text>\n");
        return;
    }

    out.push_str(indent);
    out.push_str(r#"<text x=""#);
    fmt_num(out, anchor_x);
    out.push('"');
    push_text_attrs(out, l.color, l.font, &family, anchor);
    out.push('>');
    for (range, baseline) in lines {
        let text = l.text.get(range).unwrap_or("");
        if text.is_empty() {
            continue;
        }
        out.push_str(r#"<tspan x=""#);
        fmt_num(out, anchor_x);
        out.push_str(r#"" y=""#);
        fmt_num(out, l.at[1] + baseline);
        out.push_str(r#"">"#);
        push_xml_escaped(out, text);
        out.push_str("</tspan>");
    }
    out.push_str("</text>\n");
}

fn push_text_attrs(out: &mut String, color: Color, font: TextStyle, family: &str, anchor: &str) {
    push_paint_attr(out, "fill", color);
    out.push_str(r#" font-family=""#);
    push_xml_escaped(out, family);
    out.push_str(r#"" font-size=""#);
    fmt_num(out, font.size);
    out.push('"');
    if (font.weight - 400.0).abs() >= 0.5 {
        out.push_str(r#" font-weight=""#);
        fmt_num(out, font.weight.round());
        out.push('"');
    }
    if font.italic {
        out.push_str(r#" font-style="italic""#);
    }
    if anchor != "start" {
        out.push_str(r#" text-anchor=""#);
        out.push_str(anchor);
        out.push('"');
    }
}

fn svg_font_family(family: rune::tok::Family) -> String {
    match family {
        rune::tok::Family::Sans => {
            "system-ui, -apple-system, 'Segoe UI', Roboto, Helvetica, Arial, sans-serif".to_owned()
        }
        rune::tok::Family::Mono | rune::tok::Family::SystemMono => {
            "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace".to_owned()
        }
        rune::tok::Family::Named(list) => {
            let mut names = Vec::new();
            for raw in list.split(',') {
                let clean: String = raw
                    .trim()
                    .trim_matches(['\'', '"'])
                    .chars()
                    .filter(|&c| !c.is_control() && !matches!(c, '"' | '\'' | ';' | '(' | ')' | '<' | '>'))
                    .collect();
                let clean = clean.trim();
                if clean.is_empty() || clean.to_ascii_lowercase().contains("url") {
                    continue;
                }
                if clean.contains(char::is_whitespace) {
                    names.push(format!("'{clean}'"));
                } else {
                    names.push(clean.to_owned());
                }
            }
            if names.is_empty() {
                "system-ui, sans-serif".to_owned()
            } else {
                names.push("sans-serif".to_owned());
                names.join(", ")
            }
        }
    }
}

fn push_paint_attr(out: &mut String, attr: &str, c: Color) {
    if c.a <= 1e-4 {
        out.push(' ');
        out.push_str(attr);
        out.push_str(r#"="none""#);
        return;
    }
    let ch = |v: f32| -> u8 {
        let s = rune::color::srgb_encode(v.clamp(0.0, 1.0));
        (s * 255.0).round().clamp(0.0, 255.0) as u8
    };
    let (r, g, b) = (ch(c.r), ch(c.g), ch(c.b));
    let _ = write!(out, " {attr}=\"#{r:02x}{g:02x}{b:02x}\"");
    let a = c.a.clamp(0.0, 1.0);
    if a < 0.999 {
        out.push(' ');
        out.push_str(attr);
        out.push_str(r#"-opacity=""#);
        fmt_alpha(out, a);
        out.push('"');
    }
}

fn fmt_num(out: &mut String, v: f32) {
    if !v.is_finite() {
        out.push('0');
        return;
    }
    let r = (v * 100.0).round() / 100.0;
    let r = if r == 0.0 { 0.0 } else { r };
    if (r - r.round()).abs() < 1e-4 {
        let _ = write!(out, "{}", r.round() as i64);
    } else {
        let s = format!("{r:.2}");
        out.push_str(s.trim_end_matches('0').trim_end_matches('.'));
    }
}

fn fmt_alpha(out: &mut String, a: f32) {
    let r = (a * 1000.0).round() / 1000.0;
    let s = format!("{r:.3}");
    out.push_str(s.trim_end_matches('0').trim_end_matches('.'));
}

fn push_xml_escaped(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c if c < '\x20' && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
}

#[cfg(test)]
#[path = "svg_tests.rs"]
mod tests;
