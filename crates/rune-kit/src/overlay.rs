//! Anchored overlays: Popover, Tooltip, HoverCard (`@opencode/ui` components/popover,
//! overlays/tooltip, components/hover-card), plus the shared placement and outside-press scrim.
//!
//! Overlays render in two parts: the trigger stays where the view puts it (wrapped in a keyed
//! anchor), and the content goes into the `layer` vector the view mounts at the root with
//! [`crate::root`]. Content is `El::anchored` to the trigger, so it follows layout, flips when
//! it doesn't fit, and paints above everything mounted before it.

use rune::{Align, Color, El, ScrollSpec, Shadow, Side, TextStyle, Transition, col, el, row, text};
use rune_motion::{Effect, Spring, SpringPair, Vec2, tokens};

use crate::{Acts, ControlSize, IconButton, IconButtonVariant, Kit, shadows};

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
}

fn anchor_origin(side: Side, align: Align) -> Vec2 {
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

/// Positions `content` next to the element keyed `anchor` and gives it the overlay entrance:
/// fade plus 0.96 scale from the trigger side.
pub(crate) fn anchor_at(k: &Kit, content: El, anchor: &str, side: Side, align: Align, gap: f32) -> El {
    fit(content, anchor, side, align, gap)
        .transition(Transition::scale_anchor(0.96, anchor_origin(side, align)))
        .motion(k.motion.overlay)
}

fn anchor_with(motion: Spring, content: El, anchor: &str, placement: Placement, gap: f32) -> El {
    let (side, align) = placement.split();
    content
        .anchored(anchor, side, align, gap)
        .transition(Transition::scale_anchor(0.96, anchor_origin(side, align)))
        .motion(motion)
}

/// A dropdown surface above or below its anchor is at most as tall as the window leaves it
/// (`El::anchored_fit`); one beside it (a submenu) only flips.
fn fit(content: El, anchor: &str, side: Side, align: Align, gap: f32) -> El {
    if matches!(side, Side::Top | Side::Bottom) {
        content.anchored_fit(anchor, side, align, gap)
    } else {
        content.anchored(anchor, side, align, gap)
    }
}

/// A dropdown's rows in a scroll viewport that shrinks with its fitted surface, so a list
/// longer than the window has room for scrolls, its edges fading while rows pass under them.
/// Put it in a column with `.min_h(0.0)` up to the surface.
pub(crate) fn fit_scroll(rows: El) -> El {
    col().key("scroll").w_full().shrink().min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).scroll_edge_fade(8.0).child(rows)
}

/// Positions a menu surface next to `anchor` with no entrance: it is there on the next frame,
/// and only its close fades (`.25 / 0`).
/// The 0.1 s enter spring only plays when a menu reopens mid-fade, from wherever the fade was.
pub(crate) fn menu_anchor(k: &Kit, content: El, anchor: &str, placement: Placement, gap: f32) -> El {
    let (side, align) = placement.split();
    let fade = Transition {
        insertion: Effect::IDENTITY,
        removal: Effect::opacity(0.0),
        springs: SpringPair::new(tokens::INSTANT, Spring::new(0.25, 0.0)),
        ..Transition::identity()
    };
    fit(content, anchor, side, align, gap).transition(fade).motion(k.motion.overlay)
}

/// A transparent full-window press target under an open overlay: pressing outside closes it.
pub fn scrim<S: 'static>(key: impl Into<String>, close: impl Fn(&mut S) + 'static) -> El {
    el().key(key).role("none").absolute(0.0, 0.0).w_full().h_full().on_press(close)
}

/// The older raised surface (popover, hover card): raised fill, half-strength border, md shadow.
pub(crate) fn raised(k: &Kit, e: El, radius: f32) -> El {
    shadows(k, e.radius(radius).fill(k.c.surface_raised).border(1.0, k.alpha(k.c.std_border, 0.5)), &k.elevation.md)
}

/// `Popover::new("share").trigger(button).title("Share").open(s.open).build(&kit, &mut layer)`.
///
/// Above or below its trigger it is at most as tall as the window leaves it, and its body
/// shrinks: give a long body a scroll viewport (`.shrink().min_h(0.0)`) to scroll in.
pub struct Popover {
    key: String,
    trigger: Option<El>,
    title: Option<String>,
    description: Option<String>,
    body: Vec<El>,
    open: bool,
    placement: Placement,
    gutter: f32,
    padding: f32,
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
            padding: 12.0,
            close: Acts::default(),
            dismiss: None,
        }
    }
    pub fn trigger(mut self, t: El) -> Self {
        self.trigger = Some(t);
        self
    }
    /// The body's inset on every side, 12 by default; 0 lets a body draw edge to edge, such as a
    /// search field over a divider.
    pub fn padding(mut self, p: f32) -> Self {
        self.padding = p;
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
                .control_size(ControlSize::Regular)
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
        content = content.child(col().key("body").padding(self.padding).shrink().min_h(0.0).children(self.body));
        if let Some(s) = self.dismiss {
            layer.push(s);
        }
        let (side, align) = self.placement.split();
        layer.push(anchor_at(k, raised(k, content, k.radius.md), &anchor_key, side, align, self.gutter));
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

/// How a tooltip looks: its surface, padding, type and entrance spring. Built from a kit by
/// [`TooltipStyle::from_kit`], or without one by [`TooltipStyle::neutral`]; each builder
/// overrides one thing.
#[derive(Clone, Debug)]
pub struct TooltipStyle {
    fill: Color,
    border: Option<(f32, Color)>,
    radius: f32,
    padding: (f32, f32),
    max_w: Option<f32>,
    font: TextStyle,
    ink: Color,
    shadows: Vec<Shadow>,
    motion: Spring,
}

impl TooltipStyle {
    /// The kit's look for `appearance`.
    pub fn from_kit(k: &Kit, appearance: TooltipAppearance) -> TooltipStyle {
        let c = &k.c;
        let base = TooltipStyle {
            fill: c.layer_01,
            border: None,
            radius: 4.0,
            padding: (6.0, 5.0),
            max_w: None,
            font: k.ty.small_medium,
            ink: c.text_base,
            shadows: k.shadow_layers(&k.elevation.floating).collect(),
            motion: k.motion.overlay,
        };
        match appearance {
            TooltipAppearance::Compact => base,
            TooltipAppearance::Large => {
                TooltipStyle { padding: (12.0, 12.0), font: TextStyle { weight: 530.0, ..k.ty.ui_compact }, ..base }
            }
            TooltipAppearance::Standard => TooltipStyle {
                fill: c.surface_float,
                border: Some((1.0, c.border_weak)),
                radius: k.radius.sm,
                padding: (8.0, 2.0),
                max_w: Some(320.0),
                font: k.ty.text_small_medium,
                ink: c.text_invert_strong,
                shadows: k.shadow_layers(&k.elevation.md).collect(),
                ..base
            },
        }
    }
    /// The compact tooltip on Rune's neutral light or dark tokens, for apps without a kit.
    pub fn neutral(dark: bool) -> TooltipStyle {
        TooltipStyle::from_kit(&Kit::neutral(dark), TooltipAppearance::Compact)
    }
    pub fn fill(mut self, c: Color) -> Self {
        self.fill = c;
        self
    }
    /// A ring inside the edge; `width` 0 removes it.
    pub fn border(mut self, width: f32, c: Color) -> Self {
        self.border = (width > 0.0).then_some((width, c));
        self
    }
    pub fn radius(mut self, r: f32) -> Self {
        self.radius = r;
        self
    }
    /// Horizontal and vertical padding.
    pub fn padding(mut self, x: f32, y: f32) -> Self {
        self.padding = (x, y);
        self
    }
    /// The widest the tooltip grows: [`Tooltip::label`], [`Tooltip::text`] and a
    /// `.wrap()` text [`Tooltip::child`] wrap at it. Wrapping text nested deeper in a child
    /// needs `.shrink()` on the child as well.
    pub fn max_width(mut self, w: f32) -> Self {
        self.max_w = Some(w);
        self
    }
    /// The type and colour of [`Tooltip::label`] text.
    pub fn font(mut self, f: TextStyle) -> Self {
        self.font = f;
        self
    }
    pub fn ink(mut self, c: Color) -> Self {
        self.ink = c;
        self
    }
    /// Box-shadow layers, painted in order; empty for none.
    pub fn shadows(mut self, layers: impl Into<Vec<Shadow>>) -> Self {
        self.shadows = layers.into();
        self
    }
    /// The spring the tooltip fades and scales in on.
    pub fn motion(mut self, s: Spring) -> Self {
        self.motion = s;
        self
    }
    pub fn text_style(&self) -> TextStyle {
        self.font
    }
    pub fn text_color(&self) -> Color {
        self.ink
    }
}

/// A tooltip: `Tooltip::new("save-tip").trigger(btn).text(&kit, "Save").open(hovered).build(&kit, &mut layer)`,
/// or without a kit `Tooltip::new("save-tip").trigger(btn).label("Save").open(hovered).render(&TooltipStyle::neutral(dark), &mut layer)`.
pub struct Tooltip {
    key: String,
    trigger: Option<El>,
    value: Vec<Content>,
    appearance: TooltipAppearance,
    open: bool,
    placement: Placement,
}

enum Content {
    El(El),
    /// Text, in the given type and colour or else the building style's. It wraps at the
    /// style's max width.
    Text(String, Option<(TextStyle, Color)>),
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
    /// Plain text content. It wraps at the style's [`max_width`](TooltipStyle::max_width).
    pub fn text(mut self, k: &Kit, s: impl Into<String>) -> Self {
        let look = (self.font(k), self.ink(k));
        self.value.push(Content::Text(s.into(), Some(look)));
        self
    }
    /// Plain text content in the style's font and ink, whichever style builds the tooltip. It
    /// wraps at the style's [`max_width`](TooltipStyle::max_width).
    pub fn label(mut self, s: impl Into<String>) -> Self {
        self.value.push(Content::Text(s.into(), None));
        self
    }
    /// Any content (keybinds, spans in other colours).
    pub fn child(mut self, c: El) -> Self {
        self.value.push(Content::El(c));
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
        let style = TooltipStyle::from_kit(k, self.appearance);
        self.render(&style, layer)
    }

    /// Builds with `style` instead of a kit's tokens (the appearance is then up to the style).
    pub fn render(self, style: &TooltipStyle, layer: &mut Vec<El>) -> El {
        let anchor_key = format!("{}-trigger", self.key);
        let trigger = row().key(anchor_key.clone()).children(self.trigger);
        if !self.open {
            return trigger;
        }
        let value = self.value.into_iter().map(|c| match c {
            // Wrapping text may shrink below its one-line width, so it wraps inside the box.
            Content::El(e) if style.max_w.is_some() && e.wraps() => e.shrink(),
            Content::El(e) => e,
            Content::Text(s, look) => {
                let (font, ink) = look.unwrap_or((style.font, style.ink));
                let t = text(s).font(font).color(ink);
                if style.max_w.is_some() { t.wrap().shrink() } else { t }
            }
        });
        let mut body = row().key(format!("{}-tooltip", self.key)).role("tooltip").center_y().gap(6.0).children(value);
        if let Some(w) = style.max_w {
            body = body.max_w(w);
        }
        body = body.px(style.padding.0).py(style.padding.1).radius(style.radius).fill(style.fill);
        if let Some((w, c)) = style.border {
            body = body.border(w, c);
        }
        for s in &style.shadows {
            body = body.box_shadow(*s);
        }
        layer.push(anchor_with(style.motion, body, &anchor_key, self.placement, 4.0));
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
        layer.push(anchor_with(k.motion.overlay, raised(k, content, k.radius.lg), &anchor_key, self.placement, 4.0));
        trigger
    }
}
