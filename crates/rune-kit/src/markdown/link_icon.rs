//! The icon before an external link: a globe, GitHub's mark for github.com, or the site's own
//! favicon once an app has it (`markdown.css`'s `.markdown-link-favicon`).

use rune::{Icon, InlineIcon, Pixels};

use super::*;

/// `markdown-globe.svg`: a globe stroked in the link's colour.
pub static GLOBE: Icon = Icon {
    name: "md-link-globe",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="6" stroke="currentColor"/><ellipse cx="8" cy="8" rx="2.5" ry="6" stroke="currentColor"/><path d="M2 8h12" stroke="currentColor"/>"#,
};

/// `markdown-github.svg`: GitHub's mark, filled in the link's colour.
pub static GITHUB: Icon = Icon {
    name: "md-link-github",
    view: 16.0,
    body: r#"<path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.23.49-2.69-.94-2.69-.94-.36-.92-.89-1.17-.89-1.17-.73-.5.06-.49.06-.49.8.06 1.22.82 1.22.82.71 1.21 1.87.86 2.33.66.07-.51.28-.86.51-1.06-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27s1.36.09 2 .27c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.19 0 .21.15.46.55.38A8.013 8.013 0 0 0 16 8c0-4.42-3.58-8-8-8z" fill="currentColor"/>"#,
};

/// What an app draws before an external link: the site's favicon, once it has it.
pub(super) type FaviconWire = Rc<dyn Fn(&str) -> Option<Pixels>>;

/// The host of an `http` or `https` URL, lowercased, without its user, port or path.
pub(super) fn link_host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).or_else(|| {
        let (scheme, rest) = url.split_once("://")?;
        (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")).then_some(rest)
    })?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if host.starts_with('[') { host.split_inclusive(']').next().unwrap_or(host) } else { host.split(':').next().unwrap_or(host) };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

impl MarkdownStyle {
    /// An icon `size` points square, `gap` points before the text, ahead of every link to an
    /// `http` or `https` page: a globe in the link's colour, GitHub's mark for github.com, or the
    /// site's favicon from [`Markdown::link_favicon`]. OpenCode's markdown sets them 14 and 4.
    /// None by default.
    pub fn link_icons(mut self, size: f32, gap: f32) -> Self {
        self.link_icon = Some([size.max(0.0), gap.max(0.0)]);
        self
    }
}

impl Markdown {
    /// The image to draw before a link to `url` in place of [`MarkdownStyle::link_icons`]'
    /// globe: the site's favicon, once the app has fetched and decoded it (`None` until then,
    /// and for sites it won't ask). GitHub links keep GitHub's mark.
    pub fn link_favicon(mut self, f: impl Fn(&str) -> Option<Pixels> + 'static) -> Self {
        self.link_favicon = Some(Rc::new(f));
        self
    }
}

impl Renderer {
    /// `e`, the text of `inl`, with an icon before each of its links to a web page.
    pub(super) fn link_icons(&self, mut e: El, inl: &Inline) -> El {
        let Some([size, gap]) = self.s.link_icon else { return e };
        for (a, b, url) in &inl.links {
            let Some(host) = link_host(url) else { continue };
            if inl.text.get(*a..*b).is_none_or(|t| t.trim().is_empty()) {
                continue;
            }
            let icon = match self.link_favicon.as_ref().filter(|_| host != "github.com").and_then(|f| f(url)) {
                Some(pixels) => InlineIcon::image(pixels).radius(2.0),
                None => InlineIcon::glyph(if host == "github.com" { &GITHUB } else { &GLOBE }).color(self.s.accent),
            };
            e = e.inline_icon(*a, icon.size(size).gap(gap));
        }
        e
    }
}
