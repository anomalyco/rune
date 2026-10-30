//! Card (`@opencode/ui` components/card): a left accent rule, a title with an icon, a
//! description and actions.

use rune::{Color, El, Icon, col, el, icon, row, text};

use crate::Kit;

pub struct Card {
    accent: Option<Color>,
    title: Option<(String, Option<&'static Icon>)>,
    description: Option<String>,
    actions: Vec<El>,
    children: Vec<El>,
}

impl Card {
    pub fn new() -> Card {
        Card { accent: None, title: None, description: None, actions: Vec::new(), children: Vec::new() }
    }
    /// The accent rule and title icon colour (default `icon_base`); variants pass a state colour.
    pub fn accent(mut self, c: Color) -> Self {
        self.accent = Some(c);
        self
    }
    /// A title with a leading 14 point icon (`None` draws a faint dash placeholder).
    pub fn title(mut self, t: impl Into<String>, i: Option<&'static Icon>) -> Self {
        self.title = Some((t.into(), i));
        self
    }
    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = Some(d.into());
        self
    }
    pub fn action(mut self, a: El) -> Self {
        self.actions.push(a);
        self
    }
    pub fn child(mut self, c: El) -> Self {
        self.children.push(c);
        self
    }

    /// The root is unkeyed. Its inner keys (`rule`, `body`, ...) are scoped under it, so sibling
    /// cards never collide; key the result (`.build(k).key(id)`) when cards can reorder.
    pub fn build(self, k: &Kit) -> El {
        let c = &k.c;
        let accent = self.accent.unwrap_or(c.icon_base);
        let has_title = self.title.is_some();
        // The 2 point accent rule runs 8 points in from the card's top and bottom edges; the
        // copy sits in the card's 10/12/10/10 padding beside it.
        let rule = el().key("rule").w(2.0).mt(8.0).mb(8.0).radius(2.0).fill(accent);
        let mut root = col().key("body").grow(1.0).shrink().pt(10.0).pb(10.0).pl(8.0).pr(12.0);
        if has_title {
            root = root.gap(8.0);
        }
        let mut indent = 0.0;
        let mut label = None;
        if let Some((t, i)) = self.title {
            let (glyph, color) = match i {
                Some(i) => (i, accent),
                None => (k.glyph.dash, c.text_faint),
            };
            indent = 16.0 + 8.0;
            label = Some(t.clone());
            root = root.child(
                row()
                    .key("title")
                    .gap(8.0)
                    .center_y()
                    .child(el().size(16.0, 16.0).center().child(icon(glyph, 14.0).color(color)))
                    .child(text(t).font(k.ty.body_medium).color(c.text_base)),
            );
        }
        if let Some(d) = self.description {
            root = root.child(text(d).key("description").font(k.ty.body).color(c.text_muted).wrap().w_full());
        }
        root = root.children(self.children);
        if !self.actions.is_empty() {
            root = root.child(row().key("actions").pl(indent).gap(8.0).children(self.actions));
        }
        let mut card = row().role("group").w_full().items_stretch().radius(k.radius.md).child(rule).child(root);
        if let Some(l) = label {
            card = card.label(l);
        }
        card
    }
}

impl Default for Card {
    fn default() -> Self {
        Self::new()
    }
}
