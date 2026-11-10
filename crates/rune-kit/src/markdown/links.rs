//! Links in Markdown: their handlers, looks and rings, and wiring them onto the text.
use super::*;
use rune::PointerPress;

pub(super) type LinkWire = Rc<dyn Fn(El) -> El>;
pub(super) type LinkLook = Rc<dyn Fn(&str) -> Option<(Span, LinkPaint)>>;
pub(super) type LinkRing = Rc<dyn Fn(&str) -> Option<(Color, f32)>>;
pub(super) type LinkHelp = Rc<dyn Fn(&str) -> Option<String>>;
pub(super) type CodeLink = (Rc<dyn Fn(&str) -> Option<String>>, Color);

impl Markdown {
    /// Called with a link's destination when it is clicked, or pressed with Enter or Space
    /// while focused. With a handler, links show a hand and a solid underline under the
    /// pointer and are Tab stops; without one they are styled text only. Shares its slot with
    /// [`Self::on_link_tx`] (the later call wins).
    pub fn on_link<S: 'static>(self, f: impl Fn(&mut S, &str) + 'static) -> Self {
        self.on_link_tx(move |s: &mut S, url, _| f(s, url))
    }
    /// [`Self::on_link`] with the [`Tx`] of the press, for a handler that opens a screen, scrolls
    /// or copies in the same commit (an app's own `scheme:id` links, say). Replaces any earlier
    /// `on_link` or `on_link_tx` handler (the later call wins).
    pub fn on_link_tx<S: 'static>(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self {
        let f = Rc::new(f);
        self.on_link = Some(Rc::new(move |e: El| {
            let f = f.clone();
            e.on_link(move |s: &mut S, url, tx| f(s, url, tx))
        }));
        self
    }
    /// Called with a link's destination and the point (window points) when a right-click (or
    /// macOS control-click) lands on it: a menu for that link (Open, Copy Link). It takes the
    /// press from the menus of the elements around the Markdown, which still get right-clicks
    /// elsewhere in the text. Links show a hand and are Tab stops with this alone, too.
    pub fn on_link_secondary<S: 'static>(mut self, f: impl Fn(&mut S, &str, PointerPress, &mut Tx) + 'static) -> Self {
        let f = Rc::new(f);
        self.on_link_secondary = Some(Rc::new(move |e: El| {
            let f = f.clone();
            e.on_link_secondary(move |s: &mut S, url, p, tx| f(s, url, p, tx))
        }));
        self
    }
    /// A look for some links by destination. `f` returns `None` for the default look
    /// ([`MarkdownStyle::link`] with its underline, and an accent underline on hover), or
    /// `Some((resting, hover))`: the [`Span`] drawn over the link's text (colour, weight,
    /// `bg` with its padding, underline; its range is ignored, and emphasis, strong and inline
    /// code inside the link keep their styles over it) and the [`LinkPaint`] over it while
    /// hovered. Applies with or without a handler.
    pub fn link_style(mut self, f: impl Fn(&str) -> Option<(Span, LinkPaint)> + 'static) -> Self {
        self.link_look = Some(Rc::new(f));
        self
    }
    /// An inset border ring `(color, width_pt)` inside the rounded [`Span::bg`] of chosen links
    /// by destination (`None`: no ring). Stays on hover unless the link's hover [`LinkPaint`]
    /// sets its own [`LinkPaint::ring`]. Applies with or without a handler.
    /// Links without a background draw no resting ring; use [`Self::link_style`] to set one.
    pub fn link_ring(mut self, f: impl Fn(&str) -> Option<(Color, f32)> + 'static) -> Self {
        self.link_ring = Some(Rc::new(f));
        self
    }
    /// A tooltip for links by destination (`None`: none), shown after the hover delay while
    /// the pointer rests on the link ([`Link::help`]): where it goes, say. Links take it only
    /// with a handler ([`Self::on_link`] or [`Self::on_link_secondary`]).
    pub fn link_help(mut self, f: impl Fn(&str) -> Option<String> + 'static) -> Self {
        self.link_help = Some(Rc::new(f));
        self
    }
    /// Inline code that names something to open, as Desktop's Markdown draws a path to a file
    /// that exists (markdown.tsx markInlineCode, markdown.css
    /// code[data-inline-code-kind="path"]):  maps the code's text to a link destination, or
    /// None to leave it code. Such code keeps its font but draws in color without its chip,
    /// and is a link, underlined in color at 40% under the pointer. Code already in a link
    /// stays the link's.
    pub fn code_link(mut self, color: Color, f: impl Fn(&str) -> Option<String> + 'static) -> Self {
        self.code_link = Some((Rc::new(f), color));
        self
    }
}

impl Renderer {
    /// inl with the inline code [Markdown::code_link] names as links among its links.
    pub(super) fn with_code_links<'a>(&self, inl: &'a Inline) -> std::borrow::Cow<'a, Inline> {
        let Some((f, _)) = &self.code_link else { return std::borrow::Cow::Borrowed(inl) };
        let found: Vec<(usize, usize, String)> = inl
            .spans
            .iter()
            .filter(|&&(a, b, st)| matches!(st, Style::Code) && !inl.links.iter().any(|l| l.0 <= a && b <= l.1))
            .filter_map(|&(a, b, _)| f(inl.text[a..b].trim()).map(|url| (a, b, url)))
            .collect();
        if found.is_empty() {
            return std::borrow::Cow::Borrowed(inl);
        }
        let mut out = inl.clone();
        for (a, b, url) in found {
            out.spans.push((a, b, Style::Link));
            out.links.push((a, b, url));
        }
        out.links.sort_by_key(|l| l.0);
        std::borrow::Cow::Owned(out)
    }

    /// The colour of the code link over ..b, when that range is one.
    pub(super) fn code_link_color(&self, a: usize, b: usize, inl: &Inline) -> Option<Color> {
        let (_, color) = self.code_link.as_ref()?;
        let code = inl.spans.iter().any(|&(x, y, st)| matches!(st, Style::Code) && (x, y) == (a, b));
        (code && inl.links.iter().any(|l| (l.0, l.1) == (a, b))).then_some(*color)
    }

    pub(super) fn link_span(&self, a: usize, b: usize, inl: &Inline) -> Span {
        if let Some(color) = self.code_link_color(a, b, inl) {
            return Span { color: Some(color), ..Span::new(a, b) };
        }
        let s = &self.s;
        let url = inl.links.iter().find(|l| (l.0, l.1) == (a, b)).map(|l| l.2.as_str());
        if let Some((own, _)) = self.link_look.as_ref().and_then(|look| url.and_then(|u| look(u))) {
            Span { start: a as u32, end: b as u32, ..own }
        } else {
            Span { color: Some(s.accent), underline: Some(s.link_underline), ..Span::new(a, b) }
        }
    }

    /// The single place a link press is connected: each of `inl.links` becomes a pressable
    /// span of `e` (the text element holding it) reporting its destination.
    pub(super) fn wire_link(&self, mut e: El, inl: &Inline) -> El {
        let inl = &*self.with_code_links(inl);
        if let Some(ring) = self.link_ring.as_ref().filter(|_| !inl.links.is_empty()) {
            for (a, b, url) in &inl.links {
                if let Some((color, width)) = ring(url) {
                    e = e.span_ring(*a, *b, color, width);
                }
            }
        }
        if inl.links.is_empty() || (self.on_link.is_none() && self.on_link_secondary.is_none()) { return e; }
        let default = match self.s.link_hover {
            Some(c) => LinkPaint::new().color(c).underline(c),
            None => LinkPaint::new().underline(self.s.accent),
        };
        let hover = |url: &str| self.link_look.as_ref().and_then(|f| f(url)).map_or(default, |l| l.1);
        let help = |url: &str| self.link_help.as_ref().and_then(|f| f(url)).unwrap_or_default();
        let hover_at = |a: usize, b: usize, url: &str| match self.code_link_color(a, b, inl) {
            Some(c) => LinkPaint::new().underline(c.alpha(0.4)),
            None => hover(url),
        };
        let e = inl.links.iter().fold(e, |e, (a, b, url)| e.link(Link::new(*a..*b, url.clone()).hover(hover_at(*a, *b, url)).help(help(url))));
        let e = match &self.on_link { Some(f) => f(e), None => e };
        match &self.on_link_secondary { Some(f) => f(e), None => e }
    }
}
