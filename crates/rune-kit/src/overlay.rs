//! Anchored overlays: Popover, Tooltip, HoverCard (`@opencode/ui` components/popover,
//! overlays/tooltip, components/hover-card), plus the shared placement and outside-press scrim.
//!
//! Overlays render in two parts: the trigger stays where the view puts it (wrapped in a keyed
//! anchor), and the content goes into the `layer` vector the view mounts at the root with
//! [`crate::root`]. Content is `El::anchored` to the trigger, so it follows layout, flips when
//! it doesn't fit, and paints above everything mounted before it.

use rune::{Align, Color, El, Side, TextStyle, Transition, col, el, row, text};
use rune_motion::{Effect, Spring, SpringPair, Vec2, tokens};

use crate::{Acts, ButtonSize, IconButton, IconButtonVariant, Kit, shadows};

/// Where an overlay sits relative to its trigger (Kobalte/Floating UI placements).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Top,
    TopStart,
    TopEnd,
    Bottom,
    BottomStart,
    BottomEnd,
    Left,
    Right,
    RightStart,
}

impl Placement {
    pub(crate) fn split(self) -> (Side, Align) {
        match self {
            Placement::Top => (Side::Top, Align::Center),
            Placement::TopStart => (Side::Top, Align::Start),
            Placement::TopEnd => (Side::Top, Align::End),
            Placement::Bottom => (Side::Bottom, Align::Center),
            Placement::BottomStart => (Side::Bottom, Align::Start),
            Placement::BottomEnd => (Side::Bottom, Align::End),
            Placement::Left => (Side::Left, Align::Center),
            Placement::Right => (Side::Right, Align::Center),
            Placement::RightStart => (Side::Right, Align::Start),
        }
    }

    /// The point content grows from (the trigger side), for the 0.96 scale-in.
    fn origin(self) -> Vec2 {
        let (side, align) = self.split();
        let along = match align {
            Align::Start => 0.0,
            Align::Center => 0.5,
            Align::End => 1.0,
        };
        match side {
            Side::Top => Vec2::new(along, 1.0),
            Side::Bottom => Vec2::new(along, 0.0),
            Side::Left => Vec2::new(1.0, along),
            Side::Right => Vec2::new(0.0, along),
        }
    }
}

/// Positions `content` next to the element keyed `anchor` and gives it the overlay entrance:
/// fade plus 0.96 scale from the trigger side.
pub(crate) fn anchor(k: &Kit, content: El, anchor: &str, placement: Placement, gap: f32) -> El {
    let (side, align) = placement.split();
    content
        .anchored(anchor, side, align, gap)
        .transition(Transition::scale_anchor(0.96, placement.origin()))
        .motion(k.motion.overlay)
}

/// Positions a menu surface next to `anchor` with no entrance: it is there on the next frame,
/// and only its close fades (`.25 / 0`). Abattoir: "Open menus instantly, fade only the close".
/// The 0.1 s enter spring only plays when a menu reopens mid-fade, from wherever the fade was.
pub(crate) fn menu_anchor(k: &Kit, content: El, anchor: &str, placement: Placement, gap: f32) -> El {
    let (side, align) = placement.split();
    let fade = Transition {
        insertion: Effect::IDENTITY,
        removal: Effect::opacity(0.0),
        springs: SpringPair::new(tokens::INSTANT, Spring::new(0.25, 0.0)),
        ..Transition::identity()
    };
    content.anchored(anchor, side, align, gap).transition(fade).motion(k.motion.overlay)
}

/// A transparent full-window press target under an open overlay: pressing outside closes it.
pub fn scrim<S: 'static>(key: impl Into<String>, close: impl Fn(&mut S) + 'static) -> El {
    el().key(key).role("none").absolute(0.0, 0.0).w_full().h_full().on_press(close)
}

/// The older raised surface (popover, hover card): raised fill, half-strength border, md shadow.
fn raised(k: &Kit, e: El, radius: f32) -> El {
    shadows(k, e.radius(radius).fill(k.c.surface_raised).border(1.0, k.alpha(k.c.std_border, 0.5)), &k.elevation.md)
}

/// `Popover::new("share").trigger(button).title("Share").open(s.open).build(&kit, &mut layer)`.
pub struct Popover {
    key: String,
    trigger: Option<El>,
    title: Option<String>,
    description: Option<String>,
    body: Vec<El>,
    open: bool,
    placement: Placement,
    gutter: f32,
    close: Acts,
    dismiss: Option<El>,
}

impl Popover {
    pub fn new(key: impl Into<String>) -> Popover {
        Popover {
            key: key.into(),
            trigger: None,
            title: None,
            description: None,
            body: Vec::new(),
            open: false,
            placement: Placement::Bottom,
            gutter: 4.0,
            close: Acts::default(),
            dismiss: None,
        }
    }
    pub fn trigger(mut self, t: El) -> Self {
        self.trigger = Some(t);
        self
    }
    pub fn title(mut self, t: impl Into<String>) -> Self {
        self.title = Some(t.into());
        self
    }
    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = Some(d.into());
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
    pub fn gutter(mut self, g: f32) -> Self {
        self.gutter = g;
        self
    }
    /// Close requests: the close button and presses outside.
    pub fn on_close<S: 'static>(mut self, f: impl Fn(&mut S) + Clone + 'static) -> Self {
        self.dismiss = Some(scrim(format!("{}-scrim", self.key), f.clone()));
        self.close.press(f);
        self
    }

    pub fn build(self, k: &Kit, layer: &mut Vec<El>) -> El {
        let c = &k.c;
        let anchor_key = format!("{}-trigger", self.key);
        let trigger = row().key(anchor_key.clone()).role("button").children(self.trigger);
        if !self.open {
            return trigger;
        }
        let mut content = col().key(format!("{}-content", self.key)).role("dialog")
            .focus_return(&anchor_key).min_w(200.0).max_w(320.0);
        if let Some(t) = &self.title {
            let close = IconButton::new(k.glyph.close)
                .variant(IconButtonVariant::Ghost)
                .size(ButtonSize::Normal)
                .label("Close");
            let close = self.close.apply(close.build(k).key("close"), true);
            content = content.label(t.clone()).child(
                row()
                    .key("header")
                    .padding(12.0)
                    .pb(0.0)
                    .gap(8.0)
                    .center_y()
                    .justify_between()
                    .child(text(t.clone()).key("title").font(k.ty.body_medium).color(c.text_strong).grow(1.0))
                    .child(close),
            );
        }
        if let Some(d) = &self.description {
            content = content.child(
                row().key("description").px(12.0).child(text(d.clone()).font(k.ty.text_small).color(c.std_text)),
            );
        }
        content = content.child(col().key("body").padding(12.0).children(self.body));
        if let Some(s) = self.dismiss {
            layer.push(s);
        }
        layer.push(anchor(k, raised(k, content, k.radius.md), &anchor_key, self.placement, self.gutter));
        trigger
    }
}

/// Text that inherits the page's type (13 / 440 on a 24 line, `--text-base`).
pub fn body_text(k: &Kit, s: impl Into<String>) -> El {
    text(s).font(TextStyle { tracking: 0.0, ..k.ty.ui }.lh(24.0)).color(k.c.std_text)
}

/// Text inside overlay surfaces, which inherit the document's strong text colour.
pub fn overlay_text(k: &Kit, s: impl Into<String>) -> El {
    text(s).font(TextStyle { tracking: 0.0, ..k.ty.ui }.lh(24.0)).color(k.c.text_strong)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TooltipAppearance {
    #[default]
    Compact,
    Standard,
    Large,
}

/// A tooltip: `Tooltip::new("save-tip").trigger(btn).value("Save").open(hovered).build(&kit, &mut layer)`.
pub struct Tooltip {
    key: String,
    trigger: Option<El>,
    value: Vec<El>,
    appearance: TooltipAppearance,
    open: bool,
    placement: Placement,
}

impl Tooltip {
    pub fn new(key: impl Into<String>) -> Tooltip {
        Tooltip {
            key: key.into(),
            trigger: None,
            value: Vec::new(),
            appearance: TooltipAppearance::Compact,
            open: false,
            // Kobalte's tooltips in the kit open below their trigger (observed in Storybook).
            placement: Placement::Bottom,
        }
    }
    pub fn trigger(mut self, t: El) -> Self {
        self.trigger = Some(t);
        self
    }
    /// Plain text content.
    pub fn text(mut self, k: &Kit, s: impl Into<String>) -> Self {
        let font = self.font(k);
        let color = self.ink(k);
        self.value.push(text(s).font(font).color(color));
        self
    }
    /// Any content (keybinds, spans in other colours).
    pub fn child(mut self, c: El) -> Self {
        self.value.push(c);
        self
    }
    pub fn appearance(mut self, a: TooltipAppearance) -> Self {
        self.appearance = a;
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

    /// The tooltip's type for its appearance (use it for extra spans).
    pub fn font(&self, k: &Kit) -> TextStyle {
        match self.appearance {
            TooltipAppearance::Compact => k.ty.small_medium,
            TooltipAppearance::Standard => k.ty.text_small_medium,
            TooltipAppearance::Large => TextStyle { weight: 530.0, ..k.ty.ui_compact },
        }
    }
    pub fn ink(&self, k: &Kit) -> Color {
        if self.appearance == TooltipAppearance::Standard { k.c.text_invert_strong } else { k.c.text_base }
    }

    pub fn build(self, k: &Kit, layer: &mut Vec<El>) -> El {
        let c = &k.c;
        let anchor_key = format!("{}-trigger", self.key);
        let trigger = row().key(anchor_key.clone()).children(self.trigger);
        if !self.open {
            return trigger;
        }
        let body = row().key(format!("{}-tooltip", self.key)).role("tooltip").center_y().gap(6.0).children(self.value);
        let body = match self.appearance {
            TooltipAppearance::Compact => {
                shadows(k, body.px(6.0).py(5.0).radius(4.0).fill(c.layer_01), &k.elevation.floating)
            }
            TooltipAppearance::Large => {
                shadows(k, body.padding(12.0).radius(4.0).fill(c.layer_01), &k.elevation.floating)
            }
            TooltipAppearance::Standard => shadows(
                k,
                body.max_w(320.0).px(8.0).py(2.0).radius(k.radius.sm).fill(c.surface_float).border(1.0, c.border_weak),
                &k.elevation.md,
            ),
        };
        layer.push(anchor(k, body, &anchor_key, self.placement, 4.0));
        trigger
    }
}

/// A hover card: rich preview content anchored under its trigger.
pub struct HoverCard {
    key: String,
    trigger: Option<El>,
    body: Vec<El>,
    open: bool,
    placement: Placement,
}

impl HoverCard {
    pub fn new(key: impl Into<String>) -> HoverCard {
        HoverCard { key: key.into(), trigger: None, body: Vec::new(), open: false, placement: Placement::Bottom }
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
    /// Open on hover: the trigger reports entering (the app closes it on leaving or elsewhere).
    pub fn build(self, k: &Kit, layer: &mut Vec<El>) -> El {
        let anchor_key = format!("{}-trigger", self.key);
        let trigger = row().key(anchor_key.clone()).w_full().children(self.trigger);
        if !self.open {
            return trigger;
        }
        let content = col()
            .key(format!("{}-card", self.key))
            .min_w(200.0)
            .max_w(320.0)
            .child(col().padding(4.0).children(self.body));
        layer.push(anchor(k, raised(k, content, k.radius.lg), &anchor_key, self.placement, 4.0));
        trigger
    }
}
