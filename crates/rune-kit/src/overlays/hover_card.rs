//! Hover card (`@opencode/ui` components/hover-card): rich preview content anchored beside
//! its trigger, with configurable placement, gutter, width, surface hover and ARIA attributes.

use rune::{El, Tx, col};

use super::overlay::{Placement, anchor_trigger, anchor_with, trigger_key};
use super::popover::raised;
use crate::Kit;

/// A hover card: rich preview content anchored beside its trigger.
pub struct HoverCard {
    key: String,
    trigger: Option<El>,
    body: Vec<El>,
    open: bool,
    placement: Placement,
    gutter: f32,
    min_width: f32,
    max_width: f32,
    width: Option<f32>,
    hover: Option<Box<dyn FnOnce(El) -> El>>,
    role: Option<&'static str>,
    label: Option<String>,
}

impl HoverCard {
    pub fn new(key: impl Into<String>) -> HoverCard {
        HoverCard {
            key: key.into(),
            trigger: None,
            body: Vec::new(),
            open: false,
            placement: Placement::Bottom,
            gutter: 4.0,
            min_width: 200.0,
            max_width: 320.0,
            width: None,
            hover: None,
            role: None,
            label: None,
        }
    }
    pub fn trigger(mut self, t: El) -> Self {
        self.trigger = Some(t);
        self
    }
    pub fn child(mut self, c: El) -> Self {
        self.body.push(c);
        self
    }
    pub fn open(mut self, o: bool) -> Self {
        self.open = o;
        self
    }
    pub fn placement(mut self, p: Placement) -> Self {
        self.placement = p;
        self
    }
    /// Gap in points between the trigger and the card surface (default 4).
    ///
    /// ```
    /// use rune_kit::HoverCard;
    ///
    /// let _card = HoverCard::new("preview").gutter(8.0);
    /// ```
    pub fn gutter(mut self, g: f32) -> Self {
        self.gutter = g;
        self
    }
    /// Minimum width of the card surface in points (default 200). Overridden by [`Self::width`].
    ///
    /// ```
    /// use rune_kit::HoverCard;
    ///
    /// let _card = HoverCard::new("preview").min_width(140.0);
    /// ```
    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = w;
        self
    }
    /// Maximum width of the card surface in points (default 320; lower [`Self::min_width`] as
    /// well to allow cards narrower than 200). Overridden by [`Self::width`].
    ///
    /// ```
    /// use rune_kit::HoverCard;
    ///
    /// let _card = HoverCard::new("preview").max_width(420.0);
    /// ```
    pub fn max_width(mut self, w: f32) -> Self {
        self.max_width = w;
        self
    }
    /// Fixes the card surface width in points, overriding [`Self::min_width`] and
    /// [`Self::max_width`].
    ///
    /// ```
    /// use rune_kit::HoverCard;
    ///
    /// let _card = HoverCard::new("preview").width(260.0);
    /// ```
    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }
    /// Called with `true` when the pointer enters the open card surface and `false` when it
    /// leaves (descendants included, so hovering controls inside the card does not leave).
    ///
    /// ```
    /// use rune_kit::HoverCard;
    ///
    /// struct App { card_hovered: bool }
    /// let _card = HoverCard::new("preview").on_hover(|s: &mut App, over| s.card_hovered = over);
    /// ```
    pub fn on_hover<S: 'static>(mut self, f: impl Fn(&mut S, bool) + 'static) -> Self {
        self.hover = Some(Box::new(move |e: El| e.on_hover_change(move |s: &mut S, over, _| f(s, over))));
        self
    }
    /// [`Self::on_hover`] with the change's [`Tx`], for cancelling or scheduling close timers
    /// while the pointer moves between the trigger and the card surface.
    ///
    /// ```
    /// use rune::Tx;
    /// use rune_kit::HoverCard;
    ///
    /// struct App { card_hovered: bool }
    /// let _card = HoverCard::new("preview").on_hover_tx(|s: &mut App, over, _tx: &mut Tx| {
    ///     s.card_hovered = over;
    /// });
    /// ```
    pub fn on_hover_tx<S: 'static>(mut self, f: impl Fn(&mut S, bool, &mut Tx) + 'static) -> Self {
        self.hover = Some(Box::new(move |e: El| e.on_hover_change(f)));
        self
    }
    /// Accessible role for the open card surface (for example `"dialog"`, `"region"`, or
    /// `"note"`). Without it the surface keeps the default `"group"` role.
    ///
    /// ```
    /// use rune_kit::HoverCard;
    ///
    /// let _card = HoverCard::new("preview").role("dialog").label("File preview");
    /// ```
    pub fn role(mut self, r: &'static str) -> Self {
        self.role = (!r.trim().is_empty()).then_some(r);
        self
    }
    /// Accessible name for the open card surface (ARIA `aria-label`).
    ///
    /// ```
    /// use rune_kit::HoverCard;
    ///
    /// let _card = HoverCard::new("preview").label("File preview");
    /// ```
    pub fn label(mut self, l: impl Into<String>) -> Self {
        let l = l.into();
        self.label = (!l.trim().is_empty()).then_some(l);
        self
    }
    /// Open on hover: the trigger reports entering (the app closes it on leaving or elsewhere).
    pub fn build(self, k: &Kit, layer: &mut Vec<El>) -> El {
        let anchor_key = trigger_key(&self.key);
        let trigger = anchor_trigger(anchor_key.clone(), self.trigger).w_full();
        if !self.open {
            return trigger;
        }
        let mut content = col().key(format!("{}-card", self.key));
        content = match self.width {
            Some(w) => content.w(w.max(0.0)),
            None => content.min_w(self.min_width.max(0.0)).max_w(self.max_width.max(0.0)),
        };
        if let Some(r) = self.role {
            content = content.role(r);
        }
        if let Some(l) = self.label {
            content = content.label(l);
        }
        content = content.child(col().padding(4.0).children(self.body));
        if let Some(hover) = self.hover {
            content = hover(content);
        }
        layer.push(anchor_with(k.motion.overlay, raised(k, content, k.radius.lg), &anchor_key, self.placement, self.gutter));
        trigger
    }
}
