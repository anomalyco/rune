//! A page's static first frame (`--html`): the laid-out scene as HTML elements with CSS taken
//! from each element's flexbox style, paint and text, so the browser draws the same page before
//! the wasm arrives, and readers, crawlers and link previews see real text, headings and links.
//! The layout stays the browser's (flex, percentages, wrapping), so it holds between the widths
//! it was taken at; `page` folds several widths into `@media` rules.

use std::fmt::Write;

use super::*;
use crate::a11y::roles;
use crate::runtime::web_a11y::anchor_id;

mod css;
mod page;
pub(crate) use page::page;

/// One element of the exported tree, in document order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExportNode {
    pub path: String,
    pub depth: usize,
    pub tag: &'static str,
    /// Attributes besides `class`, already escaped: ` id="views" href="…"`.
    pub attrs: String,
    /// Inner HTML for text and shapes; children follow otherwise.
    pub inner: String,
    /// CSS declarations, property then value.
    pub decls: Vec<(&'static str, String)>,
    /// Laid-out width in points, which an image's `sizes` follows across widths.
    pub width: f32,
    /// A text element: its children are inline runs, not elements.
    pub text: bool,
}

pub(crate) fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

impl RuntimeCore {
    /// The visible scene as `ExportNode`s: what `--html` writes for one width.
    pub(crate) fn export_nodes(&self) -> Vec<ExportNode> {
        let mut out = Vec::new();
        // Ancestors' tags, to keep inline content inside links, headings and buttons.
        let mut stack: Vec<(usize, &'static str)> = Vec::new();
        let selects = self.page_text_selects();
        let mut i = 0;
        while i < self.tree.nodes.len() {
            let n = &self.tree.nodes[i];
            let end = self.subtree_end(i);
            while stack.last().is_some_and(|&(e, _)| i >= e) { stack.pop(); }
            if !self.allowed(i) || self.subtree_exiting(i) || n.vis.opacity <= 0.01 || n.input.is_some() || n.is_secure() {
                i = end;
                continue;
            }
            let inline = stack.iter().any(|&(_, t)| matches!(t, "a" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "span" | "p"));
            let mut node = self.export_node(i, stack.len(), inline);
            // A page's text selects as the live mirror's will, once the wasm takes over: not
            // inside a control, unless it's `El::selectable`.
            if let Some(&on) = selects.get(i) {
                let parent = n.parent.is_none_or(|p| selects[p]);
                let explicit = n.text.as_deref().is_some_and(|t| t.selectable);
                let value = match (on, parent, explicit) {
                    (false, true, false) => Some("none"),
                    (false, false, true) => Some("text"),
                    _ => None,
                };
                if let Some(v) = value {
                    node.decls.push(("-webkit-user-select", v.into()));
                    node.decls.push(("user-select", v.into()));
                }
            }
            stack.push((end, node.tag));
            out.push(node);
            // Shapes and text carry their content; an image has none.
            if out.last().is_some_and(|e| e.tag == "img") { i = end; stack.pop(); continue; }
            i += 1;
        }
        out
    }

    fn export_node(&self, i: usize, depth: usize, inline: bool) -> ExportNode {
        let n = &self.tree.nodes[i];
        let mut attrs = String::new();
        let mut inner = String::new();
        let href = n.a11y.more.as_ref().and_then(|m| m.href.as_deref()).and_then(|u| crate::open_url::check_href(u).ok());
        let label = n.label.as_deref().filter(|s| !s.trim().is_empty());
        let pixels = n.vis.cold.pixels.as_ref().filter(|_| n.vis.cold.pixels_wrap.is_none() && i + 1 == self.subtree_end(i));
        let pixels_url = pixels.and_then(|p| p.source_url());
        let tag: &'static str = if let Some(t) = roles::container_tag(n.role) {
            t
        } else if href.is_some() {
            "a"
        } else if n.role == "heading" {
            ["h1", "h2", "h3", "h4", "h5", "h6"][n.a11y.level.unwrap_or(2).clamp(1, 6) as usize - 1]
        } else if pixels_url.is_some() {
            "img"
        } else if n.text.is_some() {
            if inline { "span" } else { "p" }
        } else if inline {
            "span"
        } else {
            "div"
        };
        if matches!(tag, "a" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6") || roles::container_tag(n.role).is_some() {
            if let Some(id) = anchor_id(n.key.as_deref()) { write!(attrs, " id=\"{id}\"").ok(); }
        }
        if let Some(role) = roles::container_aria(n.role) { write!(attrs, " role=\"{role}\"").ok(); }
        if let Some(h) = &href { write!(attrs, " href=\"{}\"", escape(h)).ok(); }
        if let Some(url) = pixels_url {
            let [x0, y0, x1, y1] = n.target;
            write!(attrs, " src=\"{}\" alt=\"{}\" width=\"{}\" height=\"{}\" decoding=\"async\"",
                escape(url), escape(label.unwrap_or("")), (x1 - x0).round(), (y1 - y0).round()).ok();
            // `page` adds `sizes`, which needs every width.
            if let Some(set) = pixels.and_then(|p| p.source_set()) { write!(attrs, " srcset=\"{}\"", escape(set)).ok(); }
            // Images below the first screen wait until the reader scrolls near them.
            if y0 > 900.0 { attrs.push_str(" loading=\"lazy\""); }
        } else if let Some(l) = label.filter(|_| n.text.is_none() && tag != "a") {
            let role = matches!(n.role, "image" | "img").then_some(" role=\"img\"").unwrap_or("");
            write!(attrs, "{role} aria-label=\"{}\"", escape(l)).ok();
        }
        if n.a11y.hidden { attrs.push_str(" aria-hidden=\"true\""); }
        let mut decls = css::layout(&self.commit.layout.style(n.taffy).to_style(), i == 0);
        let [x0, y0, x1, y1] = n.target;
        css::paint(&n.vis, (x1 - x0, y1 - y0), &mut decls);
        if let Some(spec) = n.text.as_deref().filter(|t| !t.redacted && !n.is_secure()) {
            css::text(spec, &mut decls);
            inner = css::text_html(spec);
        } else if let Some(shape) = n.vis.cold.shape.as_deref() {
            inner = css::svg(shape, (x1 - x0, y1 - y0));
        }
        if tag == "img" {
            let fit = match n.vis.cold.pixels_fit {
                None => "fill",
                Some(crate::SurfaceFit::Fit) => "contain",
                Some(crate::SurfaceFit::Fill) => "cover",
            };
            decls.push(("object-fit", fit.into()));
            if n.vis.cold.image_filter == crate::ImageFilter::Nearest {
                decls.push(("image-rendering", "pixelated".into()));
            }
        }
        ExportNode { path: n.path.clone(), depth, tag, attrs, inner, decls, width: x1 - x0, text: n.text.is_some() }
    }
}

#[cfg(test)]
mod tests;
