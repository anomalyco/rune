//! Anchored overlays: Popover, Tooltip, HoverCard (`@opencode/ui` components/popover,
//! overlays/tooltip, components/hover-card), plus the shared placement and outside-press scrim.
//!
//! Overlays render in two parts: the trigger stays where the view puts it (wrapped in a keyed
//! anchor), and the content goes into the `layer` vector the view mounts at the root with
//! [`crate::root`]. Content is `El::anchored` to the trigger, so it follows layout, flips when
//! it doesn't fit, and paints above everything mounted before it.

use std::rc::Rc;

use rune::{Align, Color, El, Shadow, Side, TextStyle, Transition, Tx, col, el, row, text};
use rune_motion::{Spring, Vec2};

use super::popover::{fit, raised_with};
use crate::{Acts, ControlSize, IconButton, IconButtonVariant, Kit};

/// Where an overlay sits relative to its trigger (Kobalte/Floating UI's 12 placements).
///
/// ```
/// use rune_kit::{HoverCard, Placement};
///
/// let _card = HoverCard::new("preview").placement(Placement::LeftStart);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Top,
    TopStart,
    TopEnd,
    Bottom,
    BottomStart,
    BottomEnd,
    Left,
    /// To the left of its trigger, top-aligned with the trigger's start edge.
    ///
    /// ```
    /// use rune_kit::{Placement, popover};
    ///
    /// let r = popover::anchor_rect([200.0, 100.0, 260.0, 130.0], (80.0, 40.0), Placement::LeftStart, 4.0, (400.0, 300.0));
    /// assert_eq!(r, [116.0, 100.0, 196.0, 140.0]);
    /// ```
    LeftStart,
    /// To the left of its trigger, bottom-aligned with the trigger's end edge.
    ///
    /// ```
    /// use rune_kit::{Placement, popover};
    ///
    /// let r = popover::anchor_rect([200.0, 100.0, 260.0, 130.0], (80.0, 40.0), Placement::LeftEnd, 4.0, (400.0, 300.0));
    /// assert_eq!(r, [116.0, 90.0, 196.0, 130.0]);
    /// ```
    LeftEnd,
    Right,
    RightStart,
    /// To the right of its trigger, bottom-aligned with the trigger's end edge.
    ///
    /// ```
    /// use rune_kit::{Placement, popover};
    ///
    /// let r = popover::anchor_rect([40.0, 100.0, 100.0, 130.0], (80.0, 40.0), Placement::RightEnd, 4.0, (400.0, 300.0));
    /// assert_eq!(r, [104.0, 90.0, 184.0, 130.0]);
    /// ```
    RightEnd,
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
            Placement::LeftStart => (Side::Left, Align::Start),
            Placement::LeftEnd => (Side::Left, Align::End),
            Placement::Right => (Side::Right, Align::Center),
            Placement::RightStart => (Side::Right, Align::Start),
            Placement::RightEnd => (Side::Right, Align::End),
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
        .token_motion(k.motion.overlay)
}

pub(crate) fn anchor_with(motion: Spring, content: El, anchor: &str, placement: Placement, gap: f32) -> El {
    let (side, align) = placement.split();
    content
        .anchored(anchor, side, align, gap)
        .transition(Transition::scale_anchor(0.96, anchor_origin(side, align)))
        .token_motion(motion)
}

/// A transparent full-window press target under an open overlay: pressing outside closes it.
pub fn scrim<S: 'static>(key: impl Into<String>, close: impl Fn(&mut S) + 'static) -> El {
    scrim_tx(key, move |s: &mut S, _| close(s))
}

/// [`scrim`] whose close callback receives the change's [`Tx`], so an outside press can focus,
/// scroll, or announce in the same commit.
///
/// ```
/// use rune::Tx;
/// use rune_kit::scrim_tx;
///
/// struct App { open: bool }
/// let _scrim = scrim_tx("popover-scrim", |s: &mut App, tx: &mut Tx| {
///     s.open = false;
///     tx.focus("after", true);
/// });
/// ```
pub fn scrim_tx<S: 'static>(key: impl Into<String>, close: impl Fn(&mut S, &mut Tx) + 'static) -> El {
    let close = Rc::new(close);
    let secondary = close.clone();
    el().key(key).role("none").absolute(0.0, 0.0).w_full().h_full()
        .on_press_tx(move |s: &mut S, tx: &mut Tx| close(s, tx))
        .on_secondary_press(move |s: &mut S, _, tx: &mut Tx| secondary(s, tx))
}

/// Makes an open surface modal: it takes focus, Escape and outside presses dismiss it, and
/// focus returns to the trigger when it closes.
pub(crate) type Modal = Box<dyn FnOnce(El) -> El>;

/// Standard overlay sub-element keys: `{key}-trigger`, `{key}-scrim`, `{key}-content`.
pub(crate) fn trigger_key(key: &str) -> String {
    format!("{key}-trigger")
}

pub(crate) fn scrim_key(key: &str) -> String {
    format!("{key}-scrim")
}

pub(crate) fn content_key(key: &str) -> String {
    format!("{key}-content")
}

/// Wraps an optional trigger element in its keyed anchor row (`{key}-trigger`).
pub(crate) fn anchor_trigger(anchor_key: String, trigger: Option<El>) -> El {
    row().key(anchor_key).children(trigger)
}

/// The outside-press scrim and the modal wrapper for a close request.
pub(crate) fn dismissal<S: 'static>(key: String, close: impl Fn(&mut S) + 'static) -> (El, Modal) {
    dismissal_tx(key, move |s: &mut S, _| close(s))
}

/// [`dismissal`] whose close callback receives the change's [`Tx`].
pub(crate) fn dismissal_tx<S: 'static>(key: String, close: impl Fn(&mut S, &mut Tx) + 'static) -> (El, Modal) {
    let close = Rc::new(close);
    let outside = close.clone();
    (
        scrim_tx(key, move |s: &mut S, tx: &mut Tx| outside(s, tx)),
        Box::new(move |e: El| e.modal_tx(true, move |s: &mut S, tx: &mut Tx| close(s, tx))),
    )
}

/// Mounts a dismissal's scrim into `layer` and hands back its modal wrapper.
fn mount_dismiss(dismiss: Option<(El, Modal)>, layer: &mut Vec<El>) -> Option<Modal> {
    dismiss.map(|(scrim, modal)| {
        layer.push(scrim);
        modal
    })
}

/// Mounts a modal dismissal's scrim, applies its modal trap to `surface`, and pushes the surface
/// anchored with [`super::popover::anchored`] (instant open, fade close).
pub(crate) fn push_anchored_modal(
    k: &Kit,
    layer: &mut Vec<El>,
    dismiss: Option<(El, Modal)>,
    mut surface: El,
    anchor: &str,
    placement: Placement,
    gutter: f32,
) {
    if let Some(modal) = mount_dismiss(dismiss, layer) {
        surface = modal(surface);
    }
    layer.push(super::popover::anchored(k, surface, anchor, placement, gutter));
}

/// Pushes an optional outside-press scrim and a raised surface anchored with [`anchor_at`]
/// (scale-and-fade entrance), with optional per-instance `(backdrop_blur, surface_fill)`
/// overrides for [`super::popover::raised_with`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn push_raised_anchored(
    k: &Kit,
    layer: &mut Vec<El>,
    dismiss: Option<El>,
    content: El,
    anchor: &str,
    side: Side,
    align: Align,
    gutter: f32,
    backdrop: (Option<f32>, Option<Color>),
) {
    if let Some(s) = dismiss {
        layer.push(s);
    }
    let surface = raised_with(k, content, k.radius.md, backdrop.0, backdrop.1);
    layer.push(anchor_at(k, surface, anchor, side, align, gutter));
}

/// `Popover::new("share").trigger(button).title("Share").open(s.open).build(&kit, &mut layer)`.
///
/// It stays inside the window on both axes when overflowing, and its body shrinks: give a long
/// body a scroll viewport (`.shrink().min_h(0.0)`) to scroll in.
pub struct Popover {
    key: String,
    trigger: Option<El>,
    title: Option<String>,
    label: Option<String>,
    description: Option<String>,
    body: Vec<El>,
    open: bool,
    placement: Placement,
    gutter: f32,
    padding: f32,
    width: Option<f32>,
    backdrop_blur: Option<f32>,
    surface_fill: Option<Color>,
    close: Acts,
    dismiss: Option<El>,
}

impl Popover {
    pub fn new(key: impl Into<String>) -> Popover {
        Popover {
            key: key.into(),
            trigger: None,
            title: None,
            label: None,
            description: None,
            body: Vec::new(),
            open: false,
            placement: Placement::Bottom,
            gutter: 4.0,
            padding: 12.0,
            width: None,
            backdrop_blur: None,
            surface_fill: None,
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
    /// The accessible name of a popover without a [`Self::title`] ("Emoji", "Search models").
    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.label = Some(l.into());
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
    /// Blurs what the window draws behind the popover surface ([`El::backdrop_blur`](rune::El::backdrop_blur)),
    /// overriding [`Kit::metrics`](crate::Kit::metrics)'s `popup_backdrop`. `0.0` turns blur off;
    /// negative or non-finite values clamp to `0.0`.
    pub fn backdrop_blur(mut self, sigma: f32) -> Self {
        self.backdrop_blur = Some(crate::tokens::metrics::non_neg(sigma));
        self
    }
    /// Replaces the popover's surface fill colour (for example a translucent tint over
    /// [`Self::backdrop_blur`]), overriding the theme fill and `Kit::metrics.popup_backdrop`.
    pub fn surface_fill(mut self, fill: Color) -> Self {
        self.surface_fill = Some(fill);
        self
    }
    /// Fixes the surface width in points in place of the default 200..320 clamp.
    pub(crate) fn width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }
    /// Close requests: the close button and presses outside.
    pub fn on_close<S: 'static>(mut self, f: impl Fn(&mut S) + Clone + 'static) -> Self {
        self.dismiss = Some(scrim(scrim_key(&self.key), f.clone()));
        self.close.press(f);
        self
    }
    /// [`Self::on_close`] whose callback receives the change's [`Tx`], so closing from the
    /// close button or an outside press can focus, scroll, or announce in the same commit.
    ///
    /// ```
    /// use rune::Tx;
    /// use rune_kit::Popover;
    ///
    /// struct App { open: bool }
    /// let _popover = Popover::new("share").on_close_tx(|s: &mut App, tx: &mut Tx| {
    ///     s.open = false;
    ///     tx.focus("after", true);
    /// });
    /// ```
    pub fn on_close_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        let f = Rc::new(f);
        let outside = f.clone();
        self.dismiss = Some(scrim_tx(scrim_key(&self.key), move |s: &mut S, tx: &mut Tx| outside(s, tx)));
        self.close.press_tx(move |s: &mut S, tx: &mut Tx| f(s, tx));
        self
    }

    pub fn build(self, k: &Kit, layer: &mut Vec<El>) -> El {
        let c = &k.c;
        let anchor_key = trigger_key(&self.key);
        // The trigger (a button) reads as open or closed; the wrapper only anchors the content.
        let open = self.open;
        let trigger = anchor_trigger(anchor_key.clone(), self.trigger.map(|t| t.expanded(open)));
        if !self.open {
            return trigger;
        }
        let mut content = col().key(content_key(&self.key)).role("dialog")
            .description(self.description.clone().unwrap_or_default())
            .focus_return(&anchor_key);
        let pm = k.popover_metrics();
        content = match self.width {
            Some(w) => content.w(w),
            None => content.min_w(pm.min_width).max_w(pm.max_width),
        };
        if let Some(l) = &self.label {
            content = content.label(l.clone());
        }
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
                    .child(text(t.clone()).key("title").font(k.ty.body_medium).color(c.text_strong).truncate())
                    .child(close),
            );
        }
        if let Some(d) = &self.description {
            content = content.child(
                row().key("description").px(12.0).child(text(d.clone()).font(k.ty.text_small).color(c.std_text).wrap().shrink()),
            );
        }
        content = content.child(col().key("body").padding(self.padding).shrink().min_h(0.0).children(self.body));
        let (side, align) = self.placement.split();
        push_raised_anchored(
            k,
            layer,
            self.dismiss,
            content,
            &anchor_key,
            side,
            align,
            self.gutter,
            (self.backdrop_blur, self.surface_fill),
        );
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
    /// A keybind's keys (`keybind-v2` neutral): their fill and label colour.
    keys: (Color, Color),
    /// The positioner's line it sits on, for the `inline-flex` appearances (`tooltip.css`).
    line: Option<TextStyle>,
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
            font: tabular(k.ty.small_medium),
            ink: c.text_base,
            shadows: k.shadow_layers(&k.elevation.floating).collect(),
            motion: k.motion.overlay,
            keys: (c.layer_03, c.text_muted),
            // The page's 13/24 line (`base.css` `html`, measured on the positioner).
            line: Some(TextStyle { family: k.ty.small_medium.family, ..TextStyle::new(13.0, 400.0).lh(24.0) }),
        };
        match appearance {
            TooltipAppearance::Compact => base,
            TooltipAppearance::Large => {
                TooltipStyle { padding: (12.0, 12.0), font: tabular(TextStyle { weight: 530.0, ..k.ty.ui_compact }), ..base }
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
                // `display: block`: it doesn't sit on the positioner's line.
                line: None,
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
    /// This look for Rune's own [`El::help`](rune::El::help) tooltips
    /// ([`Cx::help_look`](rune::Cx::help_look)), placed and timed as `@opencode/ui`'s
    /// `Tooltip` sets Kobalte's: 4 pt from its element, 8 pt inside the window, after 600 ms,
    /// at once within 300 ms of the last one hiding, and at once when the keyboard focuses its
    /// element (opencode#63834); it fades and scales in from 0.96 over
    /// 120 ms ease-out and out over 80 ms ease-in (`tooltip.css`). Its keycaps are `keybind-v2`'s
    /// neutral keys: 14 pt, 4 pt in, 2 pt corners and 2 pt apart, 6 pt after the text, their
    /// labels in uppercase 11/530. Rows ([`El::help_row`](rune::El::help_row)) stack 6 pt apart,
    /// as a tooltip listing two commands does (`line-comment.tsx`, `flex-col gap-1.5`). Compact
    /// and large sit on their positioner's 13/24 line (`display: inline-flex` in a block,
    /// `tooltip.css`), so under its element one shows 1 or 2 pt lower than the 4 pt gap puts it.
    pub fn help_look(&self) -> rune::HelpLook {
        let (ring_w, ring) = self.border.unwrap_or((0.0, Color::TRANSPARENT));
        let key = TextStyle::new(11.0, 530.0).lh(11.0).tracking(0.05).features("'tnum' 1, 'lnum' 1");
        let look = rune::HelpLook::system(false)
            .fill(self.fill)
            .ink(self.ink)
            .ring(ring_w, ring)
            .radius(self.radius)
            .padding(self.padding.0, self.padding.1)
            .font(self.font)
            .max_width(self.max_w)
            .shadows(self.shadows.iter().copied())
            .gap(4.0)
            .margin(8.0)
            .delay(0.6)
            .skip_delay(0.3)
            .on_focus(true)
            .fade(0.12, 0.08, 0.96)
            .keycaps(self.keys.0, self.keys.1, TextStyle { family: self.font.family, ..key }, true)
            .keycap_shape(14.0, 4.0, 2.0, 2.0, 6.0)
            .row_gap(6.0);
        match self.line {
            Some(line) => look.sit_on_line(line),
            None => look,
        }
    }
}

/// Compact and large tooltips set their figures tabular (`tooltip.css`), unless the type
/// already sets its own features.
fn tabular(t: TextStyle) -> TextStyle {
    if t.features.is_empty() { t.features("'tnum' 1") } else { t }
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
        TooltipStyle::from_kit(k, self.appearance).font
    }
    pub fn ink(&self, k: &Kit) -> Color {
        TooltipStyle::from_kit(k, self.appearance).ink
    }

    pub fn build(self, k: &Kit, layer: &mut Vec<El>) -> El {
        let style = TooltipStyle::from_kit(k, self.appearance);
        self.render(&style, layer)
    }

    /// Builds with `style` instead of a kit's tokens (the appearance is then up to the style).
    pub fn render(self, style: &TooltipStyle, layer: &mut Vec<El>) -> El {
        let anchor_key = trigger_key(&self.key);
        if !self.open {
            return anchor_trigger(anchor_key, self.trigger);
        }
        // While it shows, the tooltip's text describes its trigger (ARIA `aria-describedby`).
        let words: Vec<&str> = self.value.iter().filter_map(|c| match c {
            Content::Text(s, _) => Some(s.as_str()),
            Content::El(_) => None,
        }).collect();
        let described = words.join(" ");
        let named = described.clone();
        let trigger = anchor_trigger(
            anchor_key.clone(),
            self.trigger.map(|t| if described.is_empty() { t } else { t.description(described) }),
        );
        let value = self.value.into_iter().map(|c| match c {
            // Wrapping text may shrink below its one-line width, so it wraps inside the box.
            Content::El(e) if style.max_w.is_some() && e.wraps() => e.shrink(),
            Content::El(e) => e,
            Content::Text(s, look) => {
                let (font, ink) = look.unwrap_or((style.font, style.ink));
                // The tooltip's name reads it.
                let t = text(s).font(font).color(ink).a11y_hidden();
                if style.max_w.is_some() { t.wrap().shrink() } else { t }
            }
        });
        let mut body = row().key(format!("{}-tooltip", self.key)).role("tooltip").center_y().gap(6.0).children(value);
        if !named.is_empty() {
            body = body.label(named);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_twelve_placements_split_to_side_and_align() {
        let cases = [
            (Placement::Top, (Side::Top, Align::Center), Vec2::new(0.5, 1.0)),
            (Placement::TopStart, (Side::Top, Align::Start), Vec2::new(0.0, 1.0)),
            (Placement::TopEnd, (Side::Top, Align::End), Vec2::new(1.0, 1.0)),
            (Placement::Bottom, (Side::Bottom, Align::Center), Vec2::new(0.5, 0.0)),
            (Placement::BottomStart, (Side::Bottom, Align::Start), Vec2::new(0.0, 0.0)),
            (Placement::BottomEnd, (Side::Bottom, Align::End), Vec2::new(1.0, 0.0)),
            (Placement::Left, (Side::Left, Align::Center), Vec2::new(1.0, 0.5)),
            (Placement::LeftStart, (Side::Left, Align::Start), Vec2::new(1.0, 0.0)),
            (Placement::LeftEnd, (Side::Left, Align::End), Vec2::new(1.0, 1.0)),
            (Placement::Right, (Side::Right, Align::Center), Vec2::new(0.0, 0.5)),
            (Placement::RightStart, (Side::Right, Align::Start), Vec2::new(0.0, 0.0)),
            (Placement::RightEnd, (Side::Right, Align::End), Vec2::new(0.0, 1.0)),
        ];
        for (placement, expected_split, expected_origin) in cases {
            let (side, align) = placement.split();
            assert_eq!((side, align), expected_split, "{placement:?}");
            assert_eq!(anchor_origin(side, align), expected_origin, "{placement:?}");
        }
    }

    #[test]
    fn overlay_key_helpers_and_layer_order_stay_canonical() {
        assert_eq!(trigger_key("menu"), "menu-trigger");
        assert_eq!(scrim_key("menu"), "menu-scrim");
        assert_eq!(content_key("menu"), "menu-content");

        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        let s = scrim(scrim_key("p"), |_: &mut ()| {});
        push_raised_anchored(&k, &mut layer, Some(s), col().key(content_key("p")), "p-trigger", Side::Bottom, Align::Start, 4.0, (None, None));
        assert_eq!(layer.len(), 2, "scrim mounts before anchored surface");

        let mut tx_layer = Vec::new();
        let s_tx = scrim_tx(scrim_key("ptx"), |_: &mut (), _: &mut Tx| {});
        push_raised_anchored(&k, &mut tx_layer, Some(s_tx), col().key(content_key("ptx")), "ptx-trigger", Side::Bottom, Align::Start, 4.0, (None, None));
        assert_eq!(tx_layer.len(), 2, "scrim_tx mounts before anchored surface");

        let mut modal_layer = Vec::new();
        let d = dismissal(scrim_key("m"), |_: &mut ()| {});
        push_anchored_modal(&k, &mut modal_layer, Some(d), col().key("m-menu"), "m-trigger", Placement::BottomStart, 6.0);
        assert_eq!(modal_layer.len(), 2, "modal scrim mounts before anchored modal surface");

        let mut modal_tx_layer = Vec::new();
        let d_tx = dismissal_tx(scrim_key("mtx"), |_: &mut (), _: &mut Tx| {});
        push_anchored_modal(&k, &mut modal_tx_layer, Some(d_tx), col().key("mtx-menu"), "mtx-trigger", Placement::LeftStart, 6.0);
        assert_eq!(modal_tx_layer.len(), 2, "modal dismissal_tx scrim mounts before anchored modal surface");
    }
}
