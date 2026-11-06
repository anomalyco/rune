//! The page's small parts: hairlines, paragraphs with inline code, buttons and links.

use rune::{Color, El, Link, LinkPaint, Path, Shape, Span, TextStyle, Tx, canvas, el, rich, row, text};

use crate::tokens::*;

pub fn hairline(key: &str) -> El {
    el().key(key).w_full().h(1.0).fill(line()).a11y_hidden()
}

/// `s` with each `` `code` `` set in mono 13 at the text's full strength, as the page sets
/// identifiers inside prose.
pub fn prose(key: &str, s: &str, style: TextStyle, color: Color) -> El {
    let mut content = String::with_capacity(s.len());
    let mut spans = Vec::new();
    for (i, piece) in s.split('`').enumerate() {
        let at = content.len();
        content.push_str(piece);
        if i % 2 == 1 {
            spans.push(Span::new(at, content.len()).mono().size(13.0).color(fg()));
        }
    }
    rich(content, spans).key(key).font(style).color(color).wrap().selectable(true)
}

/// A section's paragraph under its heading.
pub fn intro(key: &str, s: &str) -> El {
    prose(key, s, TEXT_16, fg2()).max_w(MEASURE).mt(16.0)
}

#[derive(Clone, Copy, PartialEq)]
pub enum Button {
    /// White, with GitHub's mark.
    GitHub,
    /// White: the main action.
    Primary,
    /// Quiet: the rest.
    Secondary,
}

/// A pill button that is a link.
pub fn button(key: &str, label: &str, href: &str, kind: Button) -> El {
    let primary = kind != Button::Secondary;
    let (fill, hover, ink) = if primary { (fg(), Color::WHITE, Color::BLACK) } else { (surface2(), Color::WHITE.alpha(0.04), fg()) };
    let mut b = row().key(key).href(href).h(40.0).px(16.0).gap(8.0).center_y().radius(20.0).fill(fill).hover(hover).press_scale(0.98);
    if kind == Button::GitHub {
        b = b.child(github_mark(Color::BLACK));
    }
    if !primary {
        b = b.border(1.0, line());
    }
    b.child(text(label.to_owned()).font(TEXT_14_MEDIUM).color(ink))
}

/// `s` with `[label](url)` links in it, the links at full strength.
pub fn linked(key: &str, s: &str, style: TextStyle, color: Color) -> El {
    let mut content = String::with_capacity(s.len());
    let mut links = Vec::new();
    let mut rest = s;
    while let Some(open) = rest.find('[') {
        let Some(mid) = rest[open..].find("](").map(|m| open + m) else { break };
        let Some(close) = rest[mid..].find(')').map(|c| mid + c) else { break };
        content.push_str(&rest[..open]);
        let at = content.len();
        content.push_str(&rest[open + 1..mid]);
        links.push((at..content.len(), rest[mid + 2..close].to_owned()));
        rest = &rest[close + 1..];
    }
    content.push_str(rest);
    let spans = links.iter().map(|(r, _)| Span::new(r.start, r.end).color(fg())).collect();
    let mut t = rich(content, spans).key(key).font(style).color(color).wrap().selectable(true);
    for (r, url) in links {
        t = t.link(Link::new(r, url).hover(LinkPaint::default().underline(fg())));
    }
    t.on_link(|_: &mut crate::State, url: &str, tx: &mut Tx| {
        let _ = tx.open_url(url);
    })
}

/// A text link in a row of them: quiet until hovered.
pub fn nav_link(key: &str, label: &str, href: &str, style: TextStyle) -> El {
    row().key(key).href(href).hover_group()
        .child(text(label.to_owned()).font(style).color(fg2()).group_color(fg()))
}

/// GitHub's mark, 16 points.
pub fn github_mark(color: Color) -> El {
    canvas(Shape::new().view_box(16.0, 16.0).fill_path(&mark_path(), color)).key("mark").size(16.0, 16.0).a11y_hidden()
}

/// GitHub's octocat mark on a 16-unit square, from its SVG path.
fn mark_path() -> Path {
    svg_path("M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27s1.36.09 2 .27c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0 0 16 8c0-4.42-3.58-8-8-8")
}

/// The numbers in an SVG path's data, in order.
fn numbers(d: &str) -> Vec<f32> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut Vec<f32>| {
        if let Ok(v) = cur.parse() {
            out.push(v);
        }
        cur.clear();
    };
    for c in d.chars() {
        match c {
            '0'..='9' => cur.push(c),
            '.' if cur.contains('.') => {
                flush(&mut cur, &mut out);
                cur.push(c);
            }
            '.' => cur.push(c),
            '-' => {
                flush(&mut cur, &mut out);
                cur.push(c);
            }
            _ => flush(&mut cur, &mut out),
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// A path from SVG path data with the commands GitHub's mark uses (M, C, c, s, A as a straight
/// line's worth of curve) in absolute and relative forms.
pub fn svg_path(d: &str) -> Path {
    let mut path = Path::new();
    let mut at = [0.0f32; 2];
    let mut start = at;
    let mut ctrl = at;
    let mut cmds = Vec::new();
    let mut piece = String::new();
    for c in d.chars() {
        if c.is_ascii_alphabetic() {
            if !piece.is_empty() {
                cmds.push(std::mem::take(&mut piece));
            }
        }
        piece.push(c);
    }
    cmds.push(piece);
    for cmd in cmds {
        let mut chars = cmd.chars();
        let Some(op) = chars.next() else { continue };
        let n = numbers(chars.as_str());
        let rel = op.is_ascii_lowercase();
        let off = |p: [f32; 2], at: [f32; 2]| if rel { [p[0] + at[0], p[1] + at[1]] } else { p };
        match op.to_ascii_uppercase() {
            'M' => {
                for (i, p) in n.chunks_exact(2).enumerate() {
                    at = off([p[0], p[1]], at);
                    path = if i == 0 { path.move_to(at) } else { path.line_to(at) };
                    if i == 0 {
                        start = at;
                    }
                }
                ctrl = at;
            }
            'L' => {
                for p in n.chunks_exact(2) {
                    at = off([p[0], p[1]], at);
                    path = path.line_to(at);
                }
                ctrl = at;
            }
            'C' => {
                for p in n.chunks_exact(6) {
                    let (c1, c2, end) = (off([p[0], p[1]], at), off([p[2], p[3]], at), off([p[4], p[5]], at));
                    path = path.cubic_to(c1, c2, end);
                    ctrl = c2;
                    at = end;
                }
            }
            'S' => {
                for p in n.chunks_exact(4) {
                    let c1 = [2.0 * at[0] - ctrl[0], 2.0 * at[1] - ctrl[1]];
                    let (c2, end) = (off([p[0], p[1]], at), off([p[2], p[3]], at));
                    path = path.cubic_to(c1, c2, end);
                    ctrl = c2;
                    at = end;
                }
            }
            'A' => {
                // The mark's one arc is a quarter turn of a circle; a quadratic through its
                // corner draws it closely enough at 16 points.
                for p in n.chunks_exact(7) {
                    let end = off([p[5], p[6]], at);
                    let corner = [end[0], at[1]];
                    path = path.quad_to(corner, end);
                    at = end;
                    ctrl = at;
                }
            }
            'Z' => {
                path = path.close();
                at = start;
                ctrl = at;
            }
            _ => {}
        }
    }
    path
}

pub fn shape_dot(center: [f32; 2], r: f32, color: Color, shape: Shape) -> Shape {
    let pts: Vec<[f32; 2]> = (0..12)
        .map(|i| {
            let a = i as f32 / 12.0 * std::f32::consts::TAU;
            [center[0] + r * a.cos(), center[1] + r * a.sin()]
        })
        .collect();
    shape.fill(pts, color)
}
