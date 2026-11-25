//! Links in Markdown: their handlers, looks and rings, and wiring them onto the text.
use super::*;
use rune::PointerPress;

pub(super) type LinkWire = (Rc<dyn Fn(El) -> El>, Option<std::any::TypeId>);
pub(super) type LinkLook = Rc<dyn Fn(&str) -> Option<(Span, LinkPaint)>>;
pub(super) type LinkRing = Rc<dyn Fn(&str) -> Option<(Color, f32)>>;
pub(super) type LinkHelp = Rc<dyn Fn(&str) -> Option<String>>;
pub(super) type CodeLink = (Rc<dyn Fn(&str) -> Option<String>>, Color);

fn fn_tag<F: 'static>(_: &F) -> Option<std::any::TypeId> {
    (std::mem::size_of::<F>() == 0).then(std::any::TypeId::of::<F>)
}

impl Markdown {
    /// Called with a link's destination when it is clicked, or pressed with Enter or Space
    /// while focused. With a handler, links show a hand and a solid underline under the
    /// pointer and are Tab stops; without one they are styled text only. Shares its slot with
    /// [`Self::on_link_tx`] (the later call wins).
    pub fn on_link<S: 'static>(self, f: impl Fn(&mut S, &str) + 'static) -> Self {
        self.on_link_tx(move |s: &mut S, url, _| f(s, url))
    }
    /// [`Self::on_link`] with the [`Tx`] of the press, for a handler that opens a screen, scrolls,
    /// copies, or anchors a popover beside the activated link via [`Tx::link_rect`] in the same
    /// commit (an app's own `scheme:id` links, say). Replaces any earlier `on_link` or `on_link_tx`
    /// handler (the later call wins).
    pub fn on_link_tx<S: 'static>(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self {
        let tag = fn_tag(&f);
        let f = Rc::new(f);
        self.on_link = Some((
            Rc::new(move |e: El| {
                let f = f.clone();
                e.on_link(move |s: &mut S, url, tx| f(s, url, tx))
            }),
            tag,
        ));
        self
    }
    /// Whether right-clicking (or pressing the context-menu key on) a Markdown link with a real
    /// address (`http:`, `https:`, `mailto:`) opens Rune's default Open Link / Copy Link
    /// context menu (`true` by default; see [`El::link_menu`](rune::El::link_menu)).
    ///
    /// Yields to [`Self::on_link_secondary`] and any ancestor [`El::on_secondary_press`](rune::El::on_secondary_press).
    /// Pass `false` to opt out while keeping plain selectable-text Copy.
    ///
    /// ```
    /// # use rune_kit::{Kit, Markdown};
    /// # struct App;
    /// # impl App { fn open(&mut self, _: &str) {} }
    /// # let kit = Kit::neutral(true);
    /// let _view = Markdown::new("See [Rune](https://example.com/rune).")
    ///     .on_link(|s: &mut App, url| s.open(url))
    ///     .link_menu(false)
    ///     .build(&kit);
    /// ```
    pub fn link_menu(mut self, on: bool) -> Self {
        self.link_menu = on;
        self
    }

    /// Called with a link's destination and the point (window points) when a right-click (or
    /// macOS control-click) lands on it: a menu for that link (Open, Copy Link). It takes the
    /// press from the menus of the elements around the Markdown, which still get right-clicks
    /// elsewhere in the text. Links show a hand and are Tab stops with this alone, too.
    pub fn on_link_secondary<S: 'static>(mut self, f: impl Fn(&mut S, &str, PointerPress, &mut Tx) + 'static) -> Self {
        let tag = fn_tag(&f);
        let f = Rc::new(f);
        self.on_link_secondary = Some((
            Rc::new(move |e: El| {
                let f = f.clone();
                e.on_link_secondary(move |s: &mut S, url, p, tx| f(s, url, p, tx))
            }),
            tag,
        ));
        self
    }
    /// Called when the pointer enters, switches between, or leaves any rendered link (mirroring
    /// [`El::on_link_hover`]): `(Some(url), Some([left, top, width, height]))` in window points
    /// for the hit line box on enter or switch, and `(None, None)` when hover ends. Links show a
    /// hand, hover underline, and Tab stops with this alone, even without [`Self::on_link`] or
    /// [`Self::on_link_secondary`]. When moving directly between links in different blocks, the
    /// old block's leave runs before the new block's enter so a single state slot settles on the
    /// newly hovered link.
    ///
    /// ```
    /// # use rune::{Cx, El, Env, Runtime, col};
    /// # use rune_kit::{Kit, Markdown};
    /// # #[derive(Default)] struct S { hovered: Option<(String, [f32; 4])> }
    /// # fn view(_: &S, _: &mut Cx<S>) -> El {
    /// # let k = Kit::neutral(true);
    /// col().size(280.0, 80.0).child(
    ///     Markdown::new("Read the [docs](https://example.com/docs).")
    ///         .on_link_hover(|s: &mut S, url, rect, _| {
    ///             s.hovered = url.zip(rect).map(|(u, r)| (u.to_owned(), r));
    ///         })
    ///         .build(&k),
    /// )
    /// # }
    /// # let mut rt = Runtime::new(S::default(), view, (280.0, 80.0), Env::new(true, 1.0));
    /// # rt.frame(0.0);
    /// # let (x, y) = rt.find("https://example.com/docs").unwrap();
    /// # rt.pointer_move(x, y);
    /// # rt.frame(0.0);
    /// # assert_eq!(rt.state.hovered.as_ref().map(|(u, _)| u.as_str()), Some("https://example.com/docs"));
    /// ```
    pub fn on_link_hover<S: 'static>(
        mut self,
        f: impl Fn(&mut S, Option<&str>, Option<[f32; 4]>, &mut Tx) + 'static,
    ) -> Self {
        let tag = fn_tag(&f);
        let f = Rc::new(f);
        self.on_link_hover = Some((
            Rc::new(move |e: El| {
                let f = f.clone();
                e.on_link_hover(move |s: &mut S, url, rect, tx| f(s, url, rect, tx))
            }),
            tag,
        ));
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
    /// with a handler ([`Self::on_link`], [`Self::on_link_secondary`], or [`Self::on_link_hover`]).
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
        if inl.links.is_empty()
            || (self.on_link.is_none() && self.on_link_secondary.is_none() && self.on_link_hover.is_none())
        {
            return e;
        }
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
        let e = if self.link_menu { e } else { e.link_menu(false) };
        let e = match &self.on_link { Some((f, _)) => f(e), None => e };
        let e = match &self.on_link_secondary { Some((f, _)) => f(e), None => e };
        match &self.on_link_hover { Some((f, _)) => f(e), None => e }
    }
}
