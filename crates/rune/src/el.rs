//! The element tree that view functions return. Plain data plus type-erased handlers.

use std::rc::Rc;

use rune_motion::{Anim, Spring, Transition};
use taffy::prelude::*;

use crate::color::Color;
pub(crate) use crate::el_drop::{DropHoverFn, DropSpec};
pub use crate::cursor::{Cursor, CursorImage};
pub(crate) use crate::el_sticky::{Sticky, StickyEdge};
use crate::edit::TextEdit;
use crate::growth::{GROWTH, StreamEdge, edge};
use crate::shape::Shape;
use crate::states::{downcast, edit_fn};
pub(crate) use crate::clipboard::tx::{ClipboardRead, ReadFn};
use crate::text::{Icon, Link, Span};
use crate::tok::TextStyle;

pub use crate::el_tx::{Shell, Tx};
pub(crate) use crate::el_tx::{SelectRequest, TraceCalls};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Down,
    Move,
    Up,
}

/// A pointer gesture on an element. `x`, `y`, `start_x`, `start_y`, `w` and `h` are in the
/// element's own layout points: from its top-left as drawn (after scrolling and live
/// offsets), divided by any scale it draws at, so they hold on a zoomed canvas.
#[derive(Clone, Copy, Debug)]
pub struct Drag {
    pub phase: Phase,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub start_x: f32,
    pub start_y: f32,
    /// Pointer travel since the press, in window points (independent of the element moving).
    pub dx: f32,
    pub dy: f32,
    /// Scratch kept across one gesture's phases (e.g. a grab offset).
    pub data: f32,
    /// Which click of a run the press is, the same on every phase: 1, then 2 and 3 (and on, as
    /// AppKit's `clickCount`) for presses on this element each within the double-click
    /// interval and 4 pt of the one before. A press that drags past 4 pt ends the run. A text
    /// view selects by words on 2 and lines on 3, and extends by them as the drag moves.
    pub clicks: u32,
    pub(crate) cancelled: bool,
}

impl Drag {
    /// On `Phase::Up`: the gesture ended without a drop (Escape, the window lost focus, or the
    /// pointer was taken away). Put back whatever the drag moved.
    pub fn cancelled(&self) -> bool {
        self.cancelled
    }
}

pub(crate) type PressFn = Rc<dyn Fn(crate::states::States<'_>, &mut Tx)>;
pub(crate) type InputKeyFn = Rc<dyn Fn(crate::states::States<'_>, &crate::KeyEvent, &mut Tx) -> bool>;
pub(crate) type StepFn = Rc<dyn Fn(crate::states::States<'_>, i32, &mut Tx)>;
pub(crate) type StepLimitFn = Rc<dyn Fn(crate::states::States<'_>, bool, &mut Tx)>;

/// A press position in window points, suitable for a context-menu anchor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerPress {
    pub x: f32,
    pub y: f32,
}

/// Where the pointer is, for [`El::on_pointer_move_local`] and
/// [`El::on_secondary_press_local`]: `x` and `y` in the element's own layout points, as
/// [`Drag`] and [`crate::WheelEvent`] give them (from its top-left as drawn, divided by any
/// scale it draws at, so they hold on a zoomed canvas), and the same point in window points
/// for a context menu's anchor.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PointerEvent {
    pub x: f32,
    pub y: f32,
    pub window_x: f32,
    pub window_y: f32,
}

impl PointerEvent {
    /// The point in window points, as `on_pointer_move` and `on_secondary_press` give it: a
    /// popup or context menu's anchor.
    pub fn window(&self) -> PointerPress {
        PointerPress { x: self.window_x, y: self.window_y }
    }
}

pub(crate) type PointerFn = Rc<dyn Fn(crate::states::States<'_>, PointerEvent, &mut Tx)>;
pub(crate) type DragFn = Rc<dyn Fn(crate::states::States<'_>, &mut Drag, &mut Tx)>;
pub(crate) type DragOutFn = Rc<dyn Fn(crate::states::StatesRef<'_>) -> Option<crate::DragItem>>;
pub(crate) type DragEndFn = Rc<dyn Fn(crate::states::States<'_>, crate::DragOutcome, &mut Tx)>;
pub(crate) type EditFn = Rc<dyn for<'a> Fn(crate::states::States<'a>) -> &'a mut TextEdit>;

/// Accessibility state beyond role and label (`El::level`, `El::expanded`, `El::secure`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct A11y {
    pub level: Option<u32>,
    pub expanded: Option<bool>,
    pub secure: bool,
}

pub(crate) use crate::el_focus::FocusSpec;

#[derive(Clone)]
pub(crate) struct ModalSpec {
    pub dismiss: PressFn,
    pub outside: bool,
}

/// A vertical scroll viewport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollSpec {
    /// Follow the end: while scrolled to the bottom, growth keeps the bottom in view, and a new
    /// viewport opens at the bottom on its first frame. When the content is a growth stack, room
    /// the viewport gains at its bottom (a composer shrinking under it) is held under the last
    /// row and closes on the stack's spring once its rows stop growing, so a send only moves
    /// the rows up.
    pub stick_bottom: bool,
}

/// A growth stack (the session timeline): children are rows whose heights ease on `spring`
/// with per-second speed limits; positions derive from the presented heights above.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GrowthSpec {
    pub spring: Spring,
    pub edge: StreamEdge,
}

impl Default for GrowthSpec {
    fn default() -> Self {
        GrowthSpec { spring: GROWTH, edge: edge::FEATHER }
    }
}

/// The selection highlight until [`El::selection_color`] or [`El::caret`] sets one.
const DEFAULT_SELECTION: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.2 };

/// A text field bound to a [`TextEdit`] in app state.
#[derive(Clone)]
pub struct InputSpec {
    /// Text edits are immediate, but dependent layout (e.g. filtered results) may animate.
    pub animate_layout: bool,
    pub edit: TextEdit,
    pub placeholder: String,
    pub placeholder_color: Color,
    pub caret: Color,
    pub selection: Color,
    /// One unwrapped line that scrolls sideways to keep the caret visible, like `<input>`.
    pub single_line: bool,
    pub(crate) revealed_override: Option<bool>,
    pub(crate) decorations: Vec<Span>,
    pub(crate) rich_style: crate::edit::RichStyle,
    pub(crate) pill_icon: Option<PillIconFn>,
    pub(crate) pill_style: Option<PillStyleFn>,
    pub(crate) access: Option<EditFn>,
    pub(crate) submit: Option<PressFn>,
    pub(crate) key: Option<InputKeyFn>,
    pub(crate) change: Option<PressFn>,
    pub(crate) paste: Option<InputPasteFn>,
}

impl std::fmt::Debug for InputSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InputSpec")
            .field("animate_layout", &self.animate_layout)
            .field("edit", &self.edit)
            .field("placeholder", &self.placeholder)
            .field("placeholder_color", &self.placeholder_color)
            .field("caret", &self.caret)
            .field("selection", &self.selection)
            .field("single_line", &self.single_line)
            .finish_non_exhaustive()
    }
}

/// A CSS-style box shadow. Outer shadows draw behind the element; `inset` ones draw inside its
/// shape, over the fill. A zero-blur, zero-offset outer shadow with a spread is a ring outside
/// the border box (drawn as a ring, so translucent fills don't show it through).
///
/// `blur` is the CSS `box-shadow` blur radius (`blur = 2σ`, so Gaussian standard deviation
/// `σ = blur * 0.5`). `CALayer.shadowRadius` and Sketch / Figma shadow blur specify `r ≈ σ`,
/// so pass `blur = 2.0 * r` to match a `CALayer` shadow radius (whereas [`El::blur`] takes `σ` directly).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Shadow {
    pub x: f32,
    pub y: f32,
    /// Twice the Gaussian's standard deviation, in points (CSS blur radius).
    pub blur: f32,
    pub spread: f32,
    pub color: Color,
    pub inset: bool,
}

/// Radius of [`El::radial_gradient`], in points: a circle (`f32`) or an `[rx, ry]` ellipse.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RadialRadius(pub [f32; 2]);

impl From<f32> for RadialRadius {
    fn from(r: f32) -> Self {
        Self([r, r])
    }
}

impl From<[f32; 2]> for RadialRadius {
    fn from(r: [f32; 2]) -> Self {
        Self(r)
    }
}

impl From<(f32, f32)> for RadialRadius {
    fn from((rx, ry): (f32, f32)) -> Self {
        Self([rx, ry])
    }
}

pub(crate) const MAX_GRADIENT_STOPS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum GradientGeometry {
    Radial { center: [f32; 2], radius: [f32; 2] },
    Linear { angle: f32 },
}

/// A radial or angled linear gradient painted over an element's fill.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RadialGradient {
    pub(crate) geometry: GradientGeometry,
    /// Sorted `(t, color)` stops in `0.0..=1.0`, capped at [`MAX_GRADIENT_STOPS`].
    pub(crate) stops: Vec<(f32, Color)>,
}

fn normalize_gradient_stops(geometry: GradientGeometry, raw: &[(f32, Color)]) -> Option<RadialGradient> {
    let mut stops: Vec<(f32, Color)> = raw
        .iter()
        .copied()
        .filter(|(t, _)| t.is_finite())
        .map(|(t, c)| (t.clamp(0.0, 1.0), c))
        .collect();
    if stops.is_empty() {
        return None;
    }
    stops.sort_by(|a, b| a.0.total_cmp(&b.0));
    stops.truncate(MAX_GRADIENT_STOPS);
    if stops.len() == 1 {
        let c = stops[0].1;
        stops = vec![(0.0, c), (1.0, c)];
    }
    Some(RadialGradient { geometry, stops })
}

impl Shadow {
    /// A CALayer shadow: `shadowRadius` `radius`, `shadowOffset` (`x`, `y`) with y down as in a
    /// flipped view, and `shadowColor` at `shadowOpacity` folded into `color`. Core Animation's
    /// blur has a standard deviation of about `shadowRadius` (0.97 to 1.02 times it, measured
    /// from 1.5 to 32 pt), so `blur` is twice the radius. Corners stay circular, as with a
    /// `CGPath(roundedRect:)` shadow path. With `srgb_blend(true)` it also composites as one.
    pub fn core_animation(x: f32, y: f32, radius: f32, color: Color) -> Shadow {
        Shadow { x, y, blur: 2.0 * radius, color, ..Shadow::default() }
    }
}

/// A ring drawn `offset` points outside the border box, `width` points wide (CSS `outline`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Outline {
    pub width: f32,
    pub offset: f32,
    pub color: Color,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
}

pub use crate::el_anchor::{Anchor, Side};
pub use crate::el_text::TextSpec;
pub(crate) use crate::el_text::Elide;

/// [`El::on_link`]'s handler.
#[derive(Clone)]
pub(crate) struct LinkFn(pub LinkHandler);
pub(crate) type LinkHandler = Rc<dyn Fn(crate::states::States<'_>, &str, &mut Tx)>;

impl std::fmt::Debug for LinkFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LinkFn")
    }
}
/// [`El::on_link_secondary`]'s handler.
#[derive(Clone)]
pub(crate) struct LinkSecondaryFn(pub Rc<dyn Fn(crate::states::States<'_>, &str, PointerPress, &mut Tx)>);
impl std::fmt::Debug for LinkSecondaryFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("LinkSecondaryFn") }
}

pub use rune_motion::Cycle;

/// Default `truncate_fade` edge, in ems.
const TRUNCATE_FADE_EM: f32 = 1.5;

#[derive(Clone, Debug)]
pub struct Visual {
    /// Rectangular native input region for overlays using `OverlayInput::Regions`.
    pub hit: bool,
    pub surface: Option<(crate::NativeSurface, crate::SurfaceFit)>,
    pub fill: Color,
    pub radius: [f32; 4],
    /// Corners follow Apple's continuous curve instead of circular arcs (`El::continuous_corners`).
    pub continuous_corners: bool,
    pub border_width: f32,
    pub border: Color,
    /// Bottom colour of a vertical border gradient whose top is `border` (`El::border_gradient`).
    pub border_bottom: Option<Color>,
    /// Dash and gap lengths of a dashed border, in points (`El::border_dash`).
    pub border_dash: Option<[f32; 2]>,
    /// Points into the dash pattern where it starts (`El::border_dash_phase`).
    pub border_dash_phase: f32,
    pub shadows: Vec<Shadow>,
    pub opacity: f32,
    pub blur: f32,
    pub blur_motion: Option<Box<Anim>>,
    /// Static alpha of this element's own paint; never inherited or isolated.
    pub paint_alpha: f32,
    pub blend: crate::render::Blend,
    pub hover: Color,
    pub press: Color,
    /// Soft clip of children to this rect: fade widths left, top, right, bottom.
    pub clip: Option<[f32; 4]>,
    /// Rounded clip of this element and its subtree, applied after subtree filters.
    pub clip_radius: Option<f32>,
    /// Per-corner clip radii override (`[tl, tr, br, bl]`, in points).
    pub(crate) clip_corners: Option<[f32; 4]>,
    pub icon: Option<(&'static Icon, Color)>,
    /// Vertical linear gradient (top, bottom) painted over the fill, under hover/press overlays.
    pub gradient: Option<Box<[Color; 2]>>,
    /// Radial or angled linear gradient painted over the fill, under hover/press overlays.
    pub(crate) radial: Option<Box<RadialGradient>>,
    pub outline: Option<Outline>,
    /// Vector strokes and fills in the element's own points, over the fill.
    pub shape: Option<Rc<Shape>>,
    /// An RGBA image scaled into the element's rect, over the fill and shape.
    pub pixels: Option<crate::pixels::Pixels>,
    /// How `pixels` keeps its aspect ratio; `None` stretches it to the rect.
    pub pixels_fit: Option<crate::SurfaceFit>,
    /// App-defined tag a compositor reads per instance (`Runtime::marks`); 0 inherits the parent's.
    pub mark: u32,
    /// An effect slot (`El::effect`): `(bleed_pt, needs_backdrop)`.
    pub(crate) effect: Option<(f32, bool)>,
    /// Backdrop blur standard deviation in points (`El::backdrop_blur`); 0 is off.
    pub(crate) backdrop_blur: f32,
    /// Tracks hover for its descendants' `group_*` styles even without a hover colour. Any
    /// element with a hover colour is a group too.
    pub hover_group: bool,
    /// The hover track's enter and exit curves (default: snappy spring both ways).
    pub hover_motion: Option<Rc<(Anim, Anim)>>,
    /// Hover curves for this element and the hover tracks below it without their own.
    pub(crate) hover_within: Option<Rc<(Anim, Anim)>>,
    /// Opacity factor (rest, hovered) following the nearest ancestor group's hover.
    pub group_opacity: Option<[f32; 2]>,
    /// Text or icon colour while the nearest ancestor group is hovered.
    pub group_color: Option<Color>,
    /// `El::icon_rotation`: the icon's turn in radians, clockwise.
    pub(crate) icon_turn: f32,
    /// Rasterize this subtree once into an offscreen texture when settled (`El::cache`).
    pub(crate) cache: bool,
    /// Optional app-supplied key mixed into the cached subtree's signature (`El::cache_key`).
    pub(crate) cache_key: Option<u64>,
    /// `El::offset`: a visual shift in points, and the curve it moves on.
    pub(crate) offset: [f32; 2],
    pub(crate) offset_motion: Option<Box<Anim>>,
    /// `El::raise`: paints and takes input above the rest of the window.
    pub(crate) raise: bool,
    /// `El::pixel_snap(false)`: this subtree paints at its exact fractional rects.
    pub(crate) no_pixel_snap: bool,
    /// The curve `El::scale` changes on.
    pub(crate) scale_motion: Option<Box<Anim>>,
    /// `El::origin`: the unit point scale and rotation pivot about; `None` is the centre.
    pub(crate) origin: Option<[f32; 2]>,
    /// `El::rotate`, in radians clockwise.
    pub(crate) rotate: f32,
    /// The curve `El::rotate` changes on.
    pub(crate) rotate_motion: Option<Box<Anim>>,
    /// `El::child_exit`: how children removed in a commit leave, in place of their own.
    pub(crate) child_exit: Option<Box<Transition>>,
    /// `El::ride_trailing`: x holds against the parent's trailing edge while its width moves.
    pub(crate) ride_trailing: bool,
}

impl Default for Visual {
    fn default() -> Self {
        Visual {
            hit: false,
            surface: None,
            fill: Color::TRANSPARENT,
            radius: [0.0; 4],
            continuous_corners: false,
            border_width: 0.0,
            border: Color::TRANSPARENT,
            border_bottom: None,
            border_dash: None,
            border_dash_phase: 0.0,
            shadows: Vec::new(),
            opacity: 1.0,
            blur: 0.0,
            blur_motion: None,
            paint_alpha: 1.0,
            blend: crate::render::Blend::SourceOver,
            hover: Color::TRANSPARENT,
            press: Color::TRANSPARENT,
            clip: None,
            clip_radius: None,
            clip_corners: None,
            icon: None,
            gradient: None,
            radial: None,
            outline: None,
            shape: None,
            pixels: None,
            pixels_fit: None,
            mark: 0,
            effect: None,
            backdrop_blur: 0.0,
            hover_group: false,
            hover_motion: None,
            hover_within: None,
            group_opacity: None,
            group_color: None,
            icon_turn: 0.0,
            cache: false,
            cache_key: None,
            offset: [0.0; 2],
            offset_motion: None,
            raise: false,
            no_pixel_snap: false,
            scale_motion: None,
            origin: None,
            rotate: 0.0,
            rotate_motion: None,
            child_exit: None,
            ride_trailing: false,
        }
    }
}

/// A root paste handler; `rich` is true only for [`El::on_paste_content`], so a plain
/// [`El::on_paste`] never asks the OS clipboard for HTML or RTF.
#[derive(Clone)]
pub(crate) struct PasteFn {
    pub(crate) rich: bool,
    pub(crate) call: InputPasteFn,
}
/// A pill's leading icon by its id; see [`El::pill_icon`].
pub(crate) type PillIconFn = Rc<dyn Fn(&str) -> Option<&'static crate::Icon>>;
/// A pill's look by its id; see [`El::pill_style`].
pub(crate) type PillStyleFn = Rc<dyn Fn(&str) -> PillStyle>;

/// How one [`Mark::Pill`](crate::Mark::Pill) chip looks: any part left unset falls back to
/// [`El::pill_colors`] and [`El::pill_icon`], then the field's defaults. Build one with
/// `PillStyle::default().bg(..).icon(..)`.
#[derive(Clone, Copy, Debug, Default)]
pub struct PillStyle {
    pub(crate) bg: Option<Color>,
    pub(crate) fg: Option<Color>,
    pub(crate) icon: Option<&'static crate::Icon>,
}

impl PillStyle {
    pub const fn new() -> Self {
        Self { bg: None, fg: None, icon: None }
    }
    /// The chip's colour.
    pub const fn bg(mut self, bg: Color) -> Self {
        self.bg = Some(bg);
        self
    }
    /// The label and icon colour.
    pub const fn fg(mut self, fg: Color) -> Self {
        self.fg = Some(fg);
        self
    }
    /// The leading icon, in the label's colour.
    pub const fn icon(mut self, icon: &'static crate::Icon) -> Self {
        self.icon = Some(icon);
        self
    }
}
pub(crate) type InputPasteFn = Rc<dyn Fn(crate::states::States<'_>, &crate::ClipboardContent, &mut Tx) -> bool>;
pub(crate) type PickFn = Box<dyn FnOnce(crate::states::States<'_>, Option<crate::Picked>, &mut Tx)>;

#[derive(Clone)]
pub(crate) struct PickRead {
    pub within: Option<String>,
    pub x: f32,
    pub y: f32,
    pub pick: crate::Pick,
    pub(crate) f: Rc<std::cell::Cell<Option<PickFn>>>,
}
impl PickRead {
    pub fn take(&self) -> Option<PickFn> { self.f.take() }
}
impl std::fmt::Debug for PickRead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("PickRead") }
}

/// What an `El::selection_copy` element copies for a selection inside it.
pub(crate) type SelectionCopy = Rc<dyn Fn(&crate::TextSelection) -> Option<String>>;
/// `El::on_selection_change`.
pub(crate) type SelectionChangeFn = Rc<dyn Fn(crate::states::States<'_>, &crate::SelectionChange, &mut Tx)>;

/// A scroll viewport's content-aware edges: `El::scroll_edge_fade` and `El::scroll_margin`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ScrollEdges {
    /// Fade width (points) for the top and bottom edges; 0 for none.
    pub(crate) fade: f32,
    /// Space `Tx::scroll_into_view` leaves between a revealed row and the viewport's edge.
    pub(crate) margin: f32,
}

/// The axes a scroll viewport moves on: `El::scroll` (vertical), `El::scroll_x`, `El::scroll_xy`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ScrollAxes {
    #[default]
    Y,
    X,
    Both,
}

impl ScrollAxes {
    pub(crate) fn x(self) -> bool { self != ScrollAxes::Y }
    pub(crate) fn y(self) -> bool { self != ScrollAxes::X }
    /// Whether it moves on the horizontal axis (`true`) or the vertical one.
    pub(crate) fn has(self, horizontal: bool) -> bool { if horizontal { self.x() } else { self.y() } }
}

pub(crate) type AtBottomFn = Rc<dyn Fn(crate::states::States<'_>, bool)>;
pub(crate) type VisibleRowsFn = Rc<dyn Fn(crate::states::States<'_>, std::ops::Range<usize>) -> bool>;
pub(crate) type LayoutFn = Rc<dyn Fn(crate::states::States<'_>, [f32; 4])>;
pub(crate) type DrawnLayoutFn = Rc<dyn Fn(crate::states::States<'_>, [f32; 4], &mut Tx)>;
pub(crate) type ScrollOffsetFn = Rc<dyn Fn(crate::ScrollPos)>;
pub(crate) type WindowFocusFn = Rc<dyn Fn(crate::states::States<'_>, bool, &mut Tx)>;

#[derive(Clone)]
pub struct El {
    pub(crate) on_paste: Option<PasteFn>,
    pub(crate) on_window_focus: Option<WindowFocusFn>,
    pub(crate) focus: crate::cold::Cold<FocusSpec>,
    pub(crate) modal: Option<ModalSpec>,
    pub(crate) looping: Option<Box<rune_motion::LoopSpec>>,
    pub(crate) key: Option<String>,
    pub(crate) trace_name: Option<&'static str>,
    pub(crate) trace_namespace: Option<(&'static str, &'static str)>,
    #[cfg(debug_assertions)]
    key_source: Option<&'static std::panic::Location<'static>>,
    #[cfg(debug_assertions)]
    pub(crate) scroll_source: Option<&'static std::panic::Location<'static>>,
    #[cfg(debug_assertions)]
    pub(crate) virtual_source: Option<&'static std::panic::Location<'static>>,
    pub(crate) role: &'static str,
    pub(crate) label: Option<String>,
    pub(crate) style: Style,
    pub(crate) vis: Visual,
    pub(crate) live: Vec<crate::live::Bound>,
    pub(crate) text: Option<Box<TextSpec>>,
    pub(crate) children: Vec<El>,
    /// Children a [`memo`](crate::memo) shares with its cache, before `children`. Copied in
    /// by [`El::expand_memo`] only when the element mounts.
    pub(crate) memo_children: Option<Rc<crate::memo::Shared>>,
    pub(crate) on_press: Option<PressFn>,
    pub(crate) on_double_press: Option<PressFn>,
    pub(crate) on_secondary_press: Option<PointerFn>,
    pub(crate) on_hover: Option<PressFn>,
    pub(crate) on_hover_change: Option<WindowFocusFn>,
    /// Seconds the pointer must stay inside, then outside, before `on_hover_change` hears it.
    pub(crate) hover_delay: [f32; 2],
    pub(crate) on_pointer_move: Option<PointerFn>,
    /// `on_wheel`, `on_magnify` and the other canvas handlers (`canvas_input.rs`).
    pub(crate) gestures: Option<Box<crate::canvas_input::CanvasInput>>,
    pub(crate) above: Option<Anchor>,
    pub(crate) on_drag: Option<DragFn>,
    pub(crate) drag_out: Option<DragOutFn>,
    pub(crate) drag_threshold: Option<f32>,
    pub(crate) on_drag_start: Option<PressFn>,
    pub(crate) on_drag_end: Option<DragEndFn>,
    pub(crate) drop: Option<Box<DropSpec>>,
    pub(crate) motion: Option<Spring>,
    pub(crate) opacity_motion: Option<Box<Anim>>,
    pub(crate) lead: Option<(Spring, Spring)>,
    pub(crate) width_leads: bool,
    pub(crate) animate_layout: bool,
    pub(crate) instant_changes: bool,
    pub(crate) transition: Option<Box<Transition>>,
    pub(crate) matched: Option<Rc<str>>,
    pub(crate) scroll: Option<ScrollSpec>,
    pub(crate) scroll_axes: ScrollAxes,
    pub(crate) on_at_bottom: Option<AtBottomFn>,
    pub(crate) on_far_from_bottom: Option<(f32, AtBottomFn)>,
    pub(crate) on_visible_rows: Option<VisibleRowsFn>,
    pub(crate) on_layout: Option<LayoutFn>,
    pub(crate) on_screen_layout: Option<LayoutFn>,
    pub(crate) on_drawn_layout: Option<DrawnLayoutFn>,
    pub(crate) on_scroll_offset: Option<ScrollOffsetFn>,
    pub(crate) sticky: Option<Box<Sticky>>,
    pub(crate) scroll_edges: ScrollEdges,
    pub(crate) growth: Option<Box<GrowthSpec>>,
    pub(crate) virtual_estimate: Option<f32>,
    /// Rows `El::virtual_rows` builds at layout time, in place of `children`.
    pub(crate) virtual_rows: Option<Rc<crate::virtual_rows::RowSource>>,
    /// [`El::virtual_rows_version`], hashed.
    pub(crate) virtual_rows_version: Option<u64>,
    pub(crate) virtual_mount: bool,
    pub(crate) streaming: bool,
    pub(crate) whole: bool,
    pub(crate) input: Option<Box<InputSpec>>,
    pub(crate) cursor: Option<Cursor>,
    pub(crate) window_drag: bool,
    /// `El::ime_cursor`.
    pub(crate) ime_cursor: bool,
    pub(crate) press_scale: Option<f32>,
    pub(crate) scale: f32,
    pub(crate) time_scale: f32,
    pub(crate) help: Option<String>,
    /// `El::help_when_truncated`: the tooltip shows only while the text is cut short.
    pub(crate) help_cut: bool,
    pub(crate) material: Option<crate::SystemMaterial>,
    /// `El::aspect_fit`'s width over height; 0 when unset.
    pub(crate) aspect_fit: f32,
    pub(crate) selection_group: bool,
    /// `El::selection_copy`.
    pub(crate) selection_copy: Option<SelectionCopy>,
    /// `El::on_selection_change`.
    pub(crate) on_selection_change: Option<SelectionChangeFn>,
    /// `El::level` and `El::expanded`.
    pub(crate) a11y: A11y,
    pub(crate) inert: bool,
}

fn base(dir: FlexDirection) -> El {
    El {
        on_paste: None,
        on_window_focus: None,
        focus: Default::default(),
        modal: None,
        looping: None,
        key: None,
        trace_name: None,
        trace_namespace: None,
        #[cfg(debug_assertions)]
        key_source: None,
        #[cfg(debug_assertions)]
        scroll_source: None,
        #[cfg(debug_assertions)]
        virtual_source: None,
        role: "group",
        label: None,
        // Relative makes every element the containing block for its absolute children, so they
        // size and pin against their parent's padding box. Taffy's default (static) hands them
        // to the window instead, as CSS does.
        style: Style { display: Display::Flex, flex_direction: dir, flex_shrink: 0.0, position: Position::Relative, ..Default::default() },
        vis: Visual::default(),
        live: Vec::new(),
        text: None,
        children: Vec::new(),
        memo_children: None,
        on_press: None,
        on_double_press: None,
        on_secondary_press: None,
        on_hover: None,
        on_hover_change: None,
        hover_delay: [0.0; 2],
        on_pointer_move: None,
        gestures: None,
        above: None,
        on_drag: None,
        drag_out: None,
        drag_threshold: None,
        on_drag_start: None,
        on_drag_end: None,
        drop: None,
        motion: None,
        opacity_motion: None,
        lead: None,
        width_leads: false,
        animate_layout: false,
        instant_changes: false,
        transition: None,
        matched: None,
        scroll: None,
        scroll_axes: ScrollAxes::Y,
        on_at_bottom: None,
        on_far_from_bottom: None,
        on_visible_rows: None,
        on_layout: None,
        on_screen_layout: None,
        on_drawn_layout: None,
        on_scroll_offset: None,
        sticky: None,
        scroll_edges: ScrollEdges::default(),
        growth: None,
        virtual_estimate: None,
        virtual_rows: None,
        virtual_rows_version: None,
        virtual_mount: false,
        streaming: false,
        whole: false,
        input: None,
        cursor: None,
        window_drag: false,
        ime_cursor: false,
        press_scale: None,
        scale: 1.0,
        time_scale: 1.0,
        help: None,
        help_cut: false,
        material: None,
        aspect_fit: 0.0,
        selection_group: false,
        selection_copy: None,
        on_selection_change: None,
        a11y: A11y::default(),
        inert: false,
    }
}

pub fn row() -> El {
    base(FlexDirection::Row)
}

pub fn col() -> El {
    base(FlexDirection::Column)
}

/// A plain box.
pub fn el() -> El {
    base(FlexDirection::Row)
}

/// An effect slot (`El::effect`): size it like any box.
pub fn effect(mark: u32) -> El {
    base(FlexDirection::Row).effect(mark)
}

/// Takes the free space along its parent's main axis.
pub fn spacer() -> El {
    base(FlexDirection::Row).grow(1.0)
}

pub fn text(content: impl Into<String>) -> El {
    let mut e = base(FlexDirection::Row);
    e.role = "text";
    e.text = Some(Box::new(TextSpec {
        content: content.into(),
        style: crate::tok::type_::BODY,
        color: Color::WHITE,
        align: Align::Start,
        spans: Vec::new(),
        wrap: false,
        reveal: -1.0,
        truncate: false,
        elide: Elide::End,
        max_lines: 0,
        truncate_fade: false,
        fade_em: TRUNCATE_FADE_EM,
        justify: false,
        live: None,
        links: Vec::new(),
        on_link: None,
        on_link_secondary: None,
        advance_in: None,
        case: None,
        source: None,
        selectable: false,
        selection: DEFAULT_SELECTION,
        uncopied: Vec::new(),
        rings: Vec::new(),
        strikes: Vec::new(),
        highlights: Vec::new(),
        blocks: Vec::new(),
        soft_breaks: Vec::new(),
        pills: Vec::new(),
        pill_colors: None,
        rich_style: crate::edit::RichStyle::default(),
        redacted: false,
        secure_revealed: false,
        wrap_anywhere: false,
        shape: None,
    }));
    e
}

/// Text with styled byte ranges; wraps at its width.
pub fn rich(content: impl Into<String>, spans: Vec<Span>) -> El {
    let mut e = text(content);
    if let Some(t) = &mut e.text {
        t.spans = if spans.len() <= 1 || spans.windows(2).all(|w| w[0].start < w[0].end && w[0].end <= w[1].start) {
            spans
        } else {
            crate::text::flatten_spans(&spans)
        };
        t.wrap = true;
    }
    e
}

/// An SVG icon from a catalog, `size` points square, tinted with `.color()`.
pub fn icon(i: &'static Icon, size: f32) -> El {
    let mut e = base(FlexDirection::Row).size(size, size);
    e.role = "icon";
    e.vis.icon = Some((i, Color::WHITE));
    e
}

/// An element that paints `shape`; size it like any element.
pub fn canvas(shape: Shape) -> El {
    let mut e = base(FlexDirection::Row).shape(shape);
    e.role = "image";
    e
}

/// A text field showing `edit`; route edits back with `.on_edit(|s| &mut s.field)`.
///
/// Multi-line by default (see [`El::single_line`]). Plain Return in a paragraph submits (calling
/// [`El::on_submit`], or pressing the [`El::default_button`] when none is set) while Shift+Return
/// and Option+Return insert a line break; a document editor that wants plain Return to break
/// paragraphs attaches `.on_submit(|s, _| s.field.insert("\n"))`.
pub fn text_input(edit: &TextEdit, style: TextStyle) -> El {
    // An app may have assigned `edit.text` directly: clamp stale marks, blocks and caret to it.
    let mut edit = edit.clone();
    edit.set_single_line(false);
    edit.sync_invariants();
    let (content, _) = edit.display();
    let mut e = text(content).font(style);
    e.role = "textbox";
    e.a11y.secure = edit.is_secure();
    if let Some(t) = &mut e.text {
        t.wrap = true;
        t.wrap_anywhere = edit.is_secure();
        t.redacted = edit.is_secure();
    }
    e.input = Some(Box::new(InputSpec {
        animate_layout: false,
        edit,
        placeholder: String::new(),
        placeholder_color: Color::WHITE.alpha(0.4),
        caret: Color::WHITE,
        selection: DEFAULT_SELECTION,
        single_line: false,
        revealed_override: None,
        decorations: Vec::new(),
        rich_style: crate::edit::RichStyle::default(),
        pill_icon: None,
        pill_style: None,
        access: None,
        submit: None,
        key: None,
        change: None,
        paste: None,
    }));
    e.sync_input_pills();
    e.sync_input_rich();
    e
}

/// Fits a multi-line [`text_input`] to its text: `min_lines` tall at least, growing with each
/// wrapped line up to `max_lines`, then scrolling inside with the caret kept in view. The height
/// springs on the element's motion. Returns `field` inside a scroll viewport keyed `"scroll"`, so
/// key the field and attach its handlers first. A smaller `.max_h(..)` caps it in points.
#[cfg_attr(debug_assertions, track_caller)]
pub fn auto_grow(field: El, min_lines: usize, max_lines: usize) -> El {
    let line = field.text.as_ref().map_or(0.0, |t| t.style.line_px());
    let (min, max) = (min_lines.max(1), max_lines.max(min_lines.max(1)));
    col().key("scroll").w_full().shrink().min_h(min as f32 * line).max_h(max as f32 * line)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(field.animate_edit_layout())
}

impl El {
    /// On the root, receive native window activation/deactivation (not text-field focus).
    /// Also works on interactive overlays. Headless hosts may call Runtime::window_focus.
    pub fn on_window_focus<S: 'static>(mut self, f: impl Fn(&mut S, bool, &mut Tx) + 'static) -> Self {
        self.on_window_focus = Some(Rc::new(move |s, focused, tx| f(s.cast::<S>().unwrap(), focused, tx)));
        self
    }
    /// Paint the latest text in this element's fixed layout box. `text("00:00")` reserves
    /// that placeholder's size; explicit `.size(w, h)` also works. Updates never resize the
    /// box or reflow siblings, and overflow is clipped. Plain single-line text only.
    pub fn live_text(mut self, value: &crate::LiveText) -> Self {
        assert!(self.input.is_none(), "live_text cannot bind a text editor");
        let spec = self.text.as_mut().expect("live_text requires a text element");
        spec.live = Some(value.clone());
        self
    }
    /// On the root (or modal root), handle paste when no text field is focused.
    /// Native clipboard reads are asynchronous. Script `paste` uses the same handler.
    /// It hears the plain text; [`El::on_paste_content`] hears every form.
    pub fn on_paste<S: 'static>(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self {
        self.on_paste = Some(PasteFn {
            rich: false,
            call: Rc::new(move |s, content, tx| {
                content.has_plain() && { f(s.cast::<S>().unwrap(), content.text(), tx); true }
            }),
        });
        self
    }
    /// [`El::on_paste`] with everything the clipboard held: plain text and, when the source
    /// offered them, sanitised HTML and RTF. Replaces an `on_paste` on the same element.
    pub fn on_paste_content<S: 'static>(mut self, f: impl Fn(&mut S, &crate::ClipboardContent, &mut Tx) + 'static) -> Self {
        self.on_paste = Some(PasteFn {
            rich: true,
            call: Rc::new(move |s, content, tx| { f(s.cast::<S>().unwrap(), content, tx); true }),
        });
        self
    }
    /// Mask a text element or text input before layout; disable undo, copy, cut, selection and
    /// IME composition, omit accessibility text, redact [`Debug`], bypass layout/raster caches,
    /// and zeroize owned buffers on conceal and drop.
    ///
    /// Non-empty read-only [`text`] and [`rich`] labels show a fixed twelve-bullet mask on
    /// one line, hiding their length as well as their content. Explicit labels and help must
    /// describe the field, not contain its secret value. Links and rolling text are disabled.
    /// Pair with [`revealed`](Self::revealed) to toggle plaintext display.
    pub fn secure(mut self) -> Self {
        self.a11y.secure = true;
        if let Some(input) = &mut self.input {
            input.edit.set_secure(true);
            let single = input.single_line;
            if let Some(text) = &mut self.text {
                text.replace_content(input.edit.display().0);
                text.redacted = true;
                if !single {
                    text.wrap_anywhere = true;
                }
            }
            self.sync_input_pills();
            self.sync_input_rich();
        } else if let Some(text) = &mut self.text {
            text.redacted = true;
            if text.selectable {
                text.selectable = false;
                if self.cursor == Some(Cursor::Text) {
                    self.cursor = None;
                }
            }
        }
        self
    }

    /// Show the plain characters of a [`secure`](Self::secure) text element or text input while
    /// keeping all other secure protections (no undo, copy, cut, selection, IME or accessibility
    /// text, redacted [`Debug`], uncached shaping/raster, and zeroized buffers). Prefer
    /// [`TextEdit::revealed`] or [`TextEdit::set_revealed`] when an input model owns the state.
    pub fn revealed(mut self, revealed: bool) -> Self {
        if let Some(input) = &mut self.input {
            input.edit.set_revealed(revealed);
            input.revealed_override = Some(revealed);
            if let Some(text) = &mut self.text {
                text.replace_content(input.edit.display().0);
            }
            self.sync_input_pills();
            self.sync_input_rich();
        } else if let Some(text) = &mut self.text {
            text.secure_revealed = revealed;
        }
        self
    }

    /// A stable motion-lint name, independent of keys and generations. Defaults to the NodeId
    /// in MotionTrace; tooling retains its keyed path fallback. Static metadata allocates nothing.
    /// Give simultaneous generations distinct names when comparing their motion separately.
    pub fn trace_name(mut self, name: &'static str) -> Self {
        self.trace_name = Some(name);
        self
    }

    /// Class and host for this subtree's trace metadata (descendants may override them).
    /// motion-lint pairs names by class too: use `("gpui", "pill")` to match a native Metal trace.
    /// Static metadata performs no allocation, including when tracing is disabled.
    pub fn trace_namespace(mut self, class: &'static str, host: &'static str) -> Self {
        self.trace_namespace = Some((class, host));
        self
    }

    /// Bind opacity directly to a latest-wins value, without rebuilding or laying out the view.
    pub fn live_opacity(mut self, value: impl Into<crate::live::Binding<f32>>) -> Self {
        self.bind_live(crate::live::Bound::new(value, crate::live::Property::Opacity)); self
    }

    /// Bind a subtree's Gaussian blur (sigma in points), optionally smoothed with `Live::smooth`.
    pub fn live_blur(mut self, value: impl Into<crate::live::Binding<f32>>) -> Self {
        self.bind_live(crate::live::Bound::new(value, crate::live::Property::Blur));
        self
    }
    /// Bind an additive parent-space offset in points. Descendants and hit tests follow it.
    pub fn live_offset(mut self, value: impl Into<crate::live::Binding<[f32; 2]>>) -> Self {
        self.bind_live(crate::live::Bound::new(value, crate::live::Property::Offset)); self
    }
    /// Bind uniform scale around the node's centre; layout remains at its target size. Text,
    /// icons and images in the subtree scale with it, as under [`El::scale`]. Text rasterizes
    /// at the scale rounded to 1/32, so a dragged value doesn't fill the glyph atlas.
    pub fn live_scale(mut self, value: impl Into<crate::live::Binding<f32>>) -> Self {
        self.bind_live(crate::live::Bound::new(value, crate::live::Property::Scale)); self
    }
    pub fn live_fill(mut self, value: impl Into<crate::live::Binding<Color>>) -> Self {
        self.bind_live(crate::live::Bound::new(value, crate::live::Property::Fill)); self
    }
    /// Bind presentation [x, y, width, height] in parent points. Does not reflow siblings or
    /// rewrap text; use ordinary layout for those changes. Width/height clamp at zero.
    pub fn live_rect(mut self, value: impl Into<crate::live::Binding<[f32; 4]>>) -> Self {
        self.bind_live(crate::live::Bound::new(value, crate::live::Property::Rect)); self
    }
    fn bind_live(&mut self, bound: crate::live::Bound) {
        if let Some(old) = self.live.iter_mut().find(|b| b.property == bound.property) { *old = bound; }
        else { self.live.push(bound); }
    }
    /// Called after a focused field's text changes, with the updated model available.
    pub fn on_input_change<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        if let Some(input) = &mut self.input {
            input.change = Some(Rc::new(move |s, tx| f(s.cast::<S>().expect("input change state type"), tx)));
        }
        self
    }
    /// Handle a paste into this focused text field before the editor, with every form the
    /// clipboard held ([`ClipboardContent`](crate::ClipboardContent): plain text, and sanitised
    /// HTML and RTF when the source offered them). Return true when the app took it; false lets
    /// the editor paste the plain text as usual. Without it a field pastes plain text only.
    ///
    /// ```ignore
    /// text_input(&s.draft).on_input_paste(|s: &mut Chat, content, _| match content.html() {
    ///     Some(html) => { s.draft_rich.paste_html(html, content.text()); true }
    ///     None => false,
    /// })
    /// ```
    pub fn on_input_paste<S: 'static>(mut self, f: impl Fn(&mut S, &crate::ClipboardContent, &mut Tx) -> bool + 'static) -> Self {
        if let Some(input) = &mut self.input {
            input.paste = Some(Rc::new(move |s, content, tx| f(s.cast::<S>().expect("input paste state type"), content, tx)));
        }
        self
    }
    /// Handle a focused text field's key before the editor. Return true to consume it.
    pub fn on_input_key<S: 'static>(mut self, f: impl Fn(&mut S, &crate::KeyEvent, &mut Tx) -> bool + 'static) -> Self {
        if let Some(input) = &mut self.input {
            input.key = Some(Rc::new(move |s, key, tx| f(s.cast::<S>().expect("input key state type"), key, tx)));
        }
        self
    }
    /// Keys while this element, or a control inside it, has focus: `f` sees each key before
    /// focus navigation (arrows in a group, Return and Space presses), the default button and
    /// the window's `on_key`, and returns true to consume it. Keys typed into a focused text
    /// field stay the field's. Nested handlers run innermost first.
    ///
    /// With `focusable(true)`, this is a control that owns its keyboard, like a virtualised
    /// list whose rows aren't all built: it keeps focus while its rows scroll in and out, and a
    /// click anywhere inside it focuses it (an NSTableView).
    pub fn on_key<S: 'static>(mut self, f: impl Fn(&mut S, &crate::KeyEvent, &mut Tx) -> bool + 'static) -> Self {
        self.focus.key = Some(Rc::new(move |s, key, tx| f(downcast::<S>(s), key, tx)));
        self
    }
    /// Secondary release inside the captured element (right-click or macOS control-click), at a
    /// point in window points: a context menu's anchor. [`El::on_secondary_press_local`] gives
    /// the element's own points as well. The Menu key and Shift+F10 call it too, while this
    /// element or one inside it is focused and no key handler took the key, at the focused
    /// element's bottom-left.
    pub fn on_secondary_press<S: 'static>(mut self, f: impl Fn(&mut S, PointerPress, &mut Tx) + 'static) -> Self {
        self.on_secondary_press = Some(Rc::new(move |s, p, tx| {
            f(s.cast::<S>().expect("secondary press state type"), p.window(), tx);
        }));
        self
    }
    /// [`El::on_secondary_press`] with the point in this element's own layout points too, as
    /// [`Drag`] gives them, so a canvas finds what was right-clicked at any zoom. Open the menu
    /// at [`PointerEvent::window`].
    ///
    /// ```ignore
    /// .on_secondary_press_local(|s: &mut Canvas, p, tx| {
    ///     s.menu = Some((s.shape_at(p.x, p.y), p.window()));
    /// })
    /// ```
    pub fn on_secondary_press_local<S: 'static>(mut self, f: impl Fn(&mut S, PointerEvent, &mut Tx) + 'static) -> Self {
        self.on_secondary_press = Some(Rc::new(move |s, p, tx| f(downcast::<S>(s), p, tx)));
        self
    }

    pub(crate) fn key_location(&self) -> String {
        #[cfg(debug_assertions)]
        if let Some(source) = self.key_source { return source.to_string(); }
        "unknown (build with debug assertions for source locations)".into()
    }

    /// Paint the newest video frame inside this node. Node dimensions are in points.
    pub fn native_surface(mut self, surface: &crate::NativeSurface, fit: crate::SurfaceFit) -> Self {
        self.vis.surface = Some((surface.clone(), fit));
        self
    }
    /// Play `video` inside this node: `Fit` letterboxes, `Fill` crops. Frames come from the
    /// display link while it plays; a paused video draws no frames of its own. Audio players
    /// need no element.
    pub fn video(self, video: &crate::Player, fit: crate::SurfaceFit) -> Self {
        self.native_surface(video.surface(), fit)
    }
    /// Opt into keyboard and pointer focus. Disabled controls must set this false.
    pub fn focusable(mut self, enabled: bool) -> Self {
        self.focus.enabled = enabled;
        self
    }
    /// Positive overrides precede document order (0); negative values skip Tab only.
    pub fn tab_index(mut self, order: i32) -> Self {
        self.focus.order = order;
        self
    }
    /// Arrow keys rove among focusable descendants; Tab visits only the current/selected item.
    /// Groups with role `menu`, `listbox`, `list` or `tree` move on Up and Down only; other
    /// groups (segments, tabs, radios, toolbars) move on Left and Right as well. Home and End
    /// jump to the ends. `list`, `tree` and `grid` stop at their ends, as NSTableView does;
    /// menus, popups and segments wrap.
    pub fn focus_group(mut self) -> Self {
        self.focus.group = true;
        self
    }
    pub fn focus_selected(mut self, selected: bool) -> Self {
        self.focus.selected = selected;
        self
    }
    /// On a virtualized stack, reveal its selected row when the selected key changes,
    /// even while another control owns keyboard focus. Frames dispatching `on_hover`
    /// do not scroll, so hover-driven selection does not move the row under the pointer.
    /// Off by default; useful for suggestions controlled by an outside text field.
    pub fn reveal_selected(mut self, enabled: bool) -> Self {
        self.focus.reveal_selected = enabled;
        self
    }
    /// A press on this element or its unfocusable descendants leaves the current field
    /// focused instead of blurring it. Works on any element, not only popups: a list row, a
    /// button, a toolbar. The press still reaches `on_press` and the other handlers, and a
    /// scroll still cancels it as usual. Focusable descendants still take focus normally.
    pub fn preserve_focus_on_press(mut self, enabled: bool) -> Self {
        self.focus.preserve_on_press.focus = enabled;
        self
    }
    /// On a focus group: arrow keys and type-select also press the item they focus, so
    /// selection follows focus (a source list). Radio and tab items always do.
    pub fn focus_follows(mut self) -> Self {
        self.focus.follows = true;
        self
    }
    /// A menu or list row's highlight: painted while focused from the keyboard or pointer,
    /// and moving the pointer over the row focuses it, so keys and mouse share one
    /// highlight (NSMenu). Use instead of `hover` and `focus_ring`.
    pub fn focus_highlight(mut self, color: Color) -> Self {
        self.focus.highlight = Some(color);
        self
    }
    /// The window's default button, which Return presses (AppKit's `keyEquivalent = "\r"`).
    /// Return goes to it from anywhere in the window, or the open modal, unless the focused
    /// element takes Return itself: a text field with `on_submit`, a link, or a row of a
    /// `menu`, `listbox`, `list`, `tree` or `grid` group or a `focus_highlight` row, which
    /// Return presses. With several, the last one shown wins (a
    /// sheet's over its window's). It needs an `on_press`, and stays out of reach while hidden
    /// or behind a modal. A focused button presses on Space, and passes Return on to this.
    pub fn default_button(mut self) -> Self {
        self.focus.default = true;
        self
    }
    /// A slider's keyboard, as NSSlider's: while this element is focused, Left and Down call
    /// `f` with -1, Right and Up with +1, Page Down with -10 and Page Up with +10, repeating
    /// while held. `f` moves the value by that many of its steps and clamps it. The element
    /// becomes focusable, and its role `slider` unless it has another. Home and End go to the
    /// ends with [`El::on_step_limit`].
    pub fn on_step<S: 'static>(mut self, f: impl Fn(&mut S, i32, &mut Tx) + 'static) -> Self {
        self.focus.step = Some(Rc::new(move |s, delta, tx| f(downcast::<S>(s), delta, tx)));
        self.focus.enabled = true;
        if self.role == "group" {
            self.role = "slider";
        }
        self
    }
    /// A slider's Home and End (see [`El::on_step`]): `f` gets `false` for Home, which goes to
    /// the minimum, and `true` for End, the maximum. Without it they scroll, as elsewhere.
    pub fn on_step_limit<S: 'static>(mut self, f: impl Fn(&mut S, bool, &mut Tx) + 'static) -> Self {
        self.focus.step_limit = Some(Rc::new(move |s, max, tx| f(downcast::<S>(s), max, tx)));
        self
    }
    /// Return focus to this keyed trigger (or its first focusable child) when removed.
    pub fn focus_return(mut self, trigger: impl Into<String>) -> Self {
        self.focus.return_to = Some(trigger.into());
        self
    }
    /// Keyboard-only ring, on this element or a visual child of the focused control. A
    /// control with no ring of its own draws the default one (see [`El::no_focus_ring`]).
    pub fn focus_ring(mut self, width: f32, offset: f32, color: Color) -> Self {
        self.focus.ring = Some(Outline { width, offset, color });
        self.focus.ring_off = false;
        self
    }
    /// No focus ring: not the default one, and not one set with [`El::focus_ring`]. A control
    /// that shows focus its own way (a fill, an outline it animates) sets it on itself or on
    /// the child that would draw the ring.
    ///
    /// Without it, a `focusable` element focused from the keyboard draws AppKit's ring when
    /// neither it nor a child (short of another focusable) sets `focus_ring`: 3 points wide
    /// in [`Cx::focus_color`](crate::Cx::focus_color), just outside its border box, following
    /// its corner radii. Text fields, focused links, `focus_highlight` rows and the items of a `menu`,
    /// `listbox`, `list`, `tree` or `grid` group draw none, as in AppKit: their caret,
    /// highlight or selection shows focus.
    pub fn no_focus_ring(mut self) -> Self {
        self.focus.ring = None;
        self.focus.ring_off = true;
        self
    }
    /// Trap focus and make the background inert while mounted. Escape requests dismissal.
    pub fn modal<S: 'static>(mut self, click_outside: bool, dismiss: impl Fn(&mut S) + 'static) -> Self {
        self.modal = Some(ModalSpec { outside: click_outside,
            dismiss: Rc::new(move |s, _| dismiss(s.cast::<S>().expect("modal state type"))) });
        self
    }
    pub fn modal_click_outside(mut self, enabled: bool) -> Self {
        if let Some(m) = &mut self.modal { m.outside = enabled; }
        self
    }
    /// A finite opacity/scale sequence on the shared render clock. The final pose holds;
    /// reduced motion and explicit settle land it immediately. A new key replays it.
    pub fn sequence(self, frames: rune_motion::Sequence) -> Self {
        self.looping(rune_motion::LoopSpec::sequence(frames))
    }

    /// A renderer-owned periodic or finite presentation track, on the shared clock.
    pub fn looping(mut self, spec: rune_motion::LoopSpec) -> Self {
        self.looping = Some(Box::new(spec));
        self
    }
    /// Window keyed rows of a growth stack directly inside a vertical scroll viewport.
    /// Unvisited rows use an estimated height; measurement preserves the visible anchor.
    /// Offscreen rows have no layout, text or scene nodes.
    ///
    /// Use [`El::virtual_rows`] unless the list is short: the view still builds every row
    /// handed to `virtualize` on every commit, mounted or not. Debug builds warn once per list
    /// that builds more than 200 rows and four times what it mounts.
    ///
    /// The same element must also be a growth stack: call `.growth(GrowthSpec)` on it as well
    /// as `.virtualize(..)`, and make it the first child of the `.scroll(..)` viewport. Layout
    /// panics with the element's path otherwise.
    ///
    /// ```ignore
    /// col().scroll(ScrollSpec { stick_bottom: false }).grow(1.0).child(
    ///     col().key("rows").growth(GrowthSpec::default()).virtualize(32.0).children(rows),
    /// )
    /// ```
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn virtualize(mut self, estimated_row_height: f32) -> Self {
        self.virtual_estimate = Some(estimated_row_height.max(1.0));
        #[cfg(debug_assertions)]
        { self.virtual_source = Some(std::panic::Location::caller()); }
        self
    }

    /// [`El::virtualize`] with rows built by index: `count` rows, row `i` keyed `key(s, i)` and
    /// built by `build(s, i)`. Layout calls `build` only for the rows in the mounted window (the
    /// viewport and up to two viewports either side), after it knows where that window is, so a
    /// commit costs one `key` call per row plus the rows it mounts, however long the list is.
    /// Rows new to the window beyond the viewport mount half a viewport per side per frame, so a
    /// jump or a page loaded above the reader doesn't lay out three viewports of text at once.
    /// Both closures get the window's state, so they capture nothing borrowed.
    ///
    /// Everything `virtualize` does holds: rows keep their measured heights by key while the key
    /// stays, unvisited rows use the estimate, inserting or prepending rows keeps the reader's
    /// row in place, and `Tx::scroll_into_view` reaches any row by key. A row's key is
    /// `key(s, i)`, whatever key its element had. The same element needs `.growth(GrowthSpec)`
    /// and must be the direct child of a `.scroll(..)` viewport, and holds no other children.
    ///
    /// A row outside the window isn't built, so it has no elements at all: the inspector,
    /// `Runtime::elements`, accessibility, Tab order, text selection and matched elements see the
    /// built rows only. A focused field in a row that scrolls out keeps its focus and typing (its
    /// row is built for that, not mounted); a field focused from state in a row that isn't built
    /// takes focus once the row is. A [`memo`](crate::memo) inside `build` keeps its subtree while
    /// the row stays built; a row built again after scrolling out builds afresh.
    ///
    /// Handlers in a row must find their item by its key or id, never by `i`. An index is only
    /// where the item sat when the row was built: a row kept by `memo` keeps its handlers while
    /// rows are inserted or removed above it, so a handler holding `i` would act on whatever
    /// item moved into that slot.
    ///
    /// ```ignore
    /// col().scroll(ScrollSpec { stick_bottom: true }).grow(1.0).child(
    ///     col().key("rows").growth(GrowthSpec::default()).virtual_rows(s.messages.len(), 72.0,
    ///         |s: &App, i| s.messages[i].id.as_str().into(),
    ///         |s: &App, i| {
    ///             let m = &s.messages[i];
    ///             let id = m.id.clone();
    ///             memo(&m.id, m.revision, || message(m).on_press(move |s: &mut App| s.open(&id)))
    ///         }),
    /// )
    /// ```
    pub fn virtual_rows<S: 'static>(
        mut self,
        count: usize,
        estimated_row_height: f32,
        key: impl for<'a> Fn(&'a S, usize) -> std::borrow::Cow<'a, str> + 'static,
        build: impl Fn(&S, usize) -> El + 'static,
    ) -> Self {
        self.virtual_rows = Some(Rc::new(crate::virtual_rows::RowSource::new(count, key, build)));
        self.virtualize(estimated_row_height)
    }

    /// Tells [`El::virtual_rows`] its keys are those of the last commit that gave the same
    /// `version`, so layout skips calling `key` for every row to find out. A list of tens of
    /// thousands of rows then commits in time independent of its length. Change `version`
    /// whenever rows are reordered, replaced or any row's key changes while the row count stays
    /// the same (a change in `count` is always detected without a bump); with a stale version on
    /// an in-place change the rows keep their old keys.
    pub fn virtual_rows_version(mut self, version: impl std::hash::Hash) -> Self {
        self.virtual_rows_version = Some(crate::memo::hash_of(&version));
        self
    }

    /// On a virtual list ([`El::virtualize`] or [`El::virtual_rows`]), called with the index
    /// range `start..end` of rows currently visible in its scroll viewport: once after its
    /// first layout, then whenever that range or the rows change. Return `true` when `f`
    /// changed state the view reads (so the view runs again), or `false` when it only updated
    /// bookkeeping (so scrolling through rows that don't change the view's output doesn't
    /// re-run the view).
    pub fn on_visible_rows<S: 'static>(
        mut self,
        f: impl Fn(&mut S, std::ops::Range<usize>) -> bool + 'static,
    ) -> Self {
        self.on_visible_rows = Some(Rc::new(move |s, r| f(downcast::<S>(s), r)));
        self
    }

    /// Identity among siblings. Siblings need unique keys: when two share one, the first keeps
    /// it, each later one is renamed `key#dup1`, `key#dup2`, ... and Rune prints a warning naming
    /// both call sites once per key.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        #[cfg(debug_assertions)]
        { self.key_source = Some(std::panic::Location::caller()); }
        self
    }
    /// The element's accessible role. `"textbox"` on a container also makes a press anywhere
    /// on it (padding, a leading icon, a trailing label) focus its one reachable text editor and
    /// place the caret at the nearer end, without blurring a focused field or dragging the
    /// window; buttons and other controls inside keep their own presses. With zero or more
    /// than one reachable editor inside, the container does nothing special.
    pub fn role(mut self, role: &'static str) -> Self {
        self.role = role;
        self
    }
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
    /// An outline row's depth for accessibility, 1 at the top (ARIA `aria-level`), as a
    /// `treeitem` carries it. The element tree reports it as `level`.
    pub fn level(mut self, level: u32) -> Self {
        self.a11y.level = Some(level);
        self
    }
    /// Whether a disclosure (a tree row with children, a collapsible header) is open, for
    /// accessibility (ARIA `aria-expanded`). Leave it unset on an element that can't open.
    /// The element tree reports it as `expanded`.
    pub fn expanded(mut self, open: bool) -> Self {
        self.a11y.expanded = Some(open);
        self
    }
    /// Turn this element's icon by `radians`, clockwise about its centre: a disclosure chevron
    /// pointing right when closed and down when open. A change springs on the element's
    /// [`motion`](Self::motion) and jumps under reduced motion. Only the glyph turns; layout and
    /// hit testing keep the upright box.
    pub fn icon_rotation(mut self, radians: f32) -> Self {
        self.vis.icon_turn = if radians.is_finite() { radians } else { 0.0 };
        self
    }
    /// A tooltip shown after the system's hover delay while the pointer rests on this element,
    /// as SwiftUI's `.help`. A press, scroll, key or the pointer leaving hides it; it takes no
    /// input itself. On macOS it is AppKit's own tooltip; elsewhere Rune draws it. The element
    /// tree (`Runtime::elements`, `--tree`) carries it as the element's help text.
    pub fn help(mut self, text: impl Into<String>) -> Self {
        let text = text.into();
        self.help = (!text.is_empty()).then_some(text);
        self.help_cut = false;
        self
    }
    /// [`El::help`] that shows only while this [`truncate`](Self::truncate)d text is cut short,
    /// as AppKit's expansion tooltips show a table cell's whole text. Text that fits shows none.
    pub fn help_when_truncated(self, text: impl Into<String>) -> Self {
        let mut e = self.help(text);
        e.help_cut = true;
        e
    }
    pub fn child(mut self, child: El) -> Self {
        self.children.push(child);
        self
    }
    /// Every child: those shared with a memo cache, then the element's own.
    pub(crate) fn all_children(&self) -> impl Iterator<Item = &El> {
        self.memo_children.iter().flat_map(|c| c.children.iter()).chain(&self.children)
    }
    /// Copies in the children shared with a memo cache, so the element owns all of them.
    pub(crate) fn expand_memo(&mut self) {
        if let Some(shared) = self.memo_children.take() {
            let shared = Rc::unwrap_or_clone(shared).children;
            let own = std::mem::replace(&mut self.children, shared);
            self.children.extend(own);
        }
    }
    pub fn children(mut self, children: impl IntoIterator<Item = El>) -> Self {
        self.children.extend(children);
        self
    }

    // Layout
    pub fn w(mut self, w: f32) -> Self {
        self.style.size.width = length(w);
        self
    }
    pub fn h(mut self, h: f32) -> Self {
        self.style.size.height = length(h);
        self
    }
    pub fn size(self, w: f32, h: f32) -> Self {
        self.w(w).h(h)
    }
    /// Width over height, as CSS `aspect-ratio`: with one side set (or stretched by the parent)
    /// the other follows; a box with both sides set ignores it. A picture that sets only one
    /// side already takes its image's ratio ([`pixels`](Self::pixels)); use this for video or
    /// any other box. Anything not positive removes it.
    pub fn aspect_ratio(mut self, ratio: f32) -> Self {
        self.style.aspect_ratio = (ratio.is_finite() && ratio > 0.0).then_some(ratio);
        self
    }
    /// The largest box at `ratio` (width over height) that fits the room the element is given,
    /// as SwiftUI's `.aspectRatio(ratio, contentMode: .fit)` or CSS `object-fit: contain` for
    /// the element itself. The room is the free space along the parent's main axis and its
    /// whole cross side, within `max_w` and `max_h`; siblings keep theirs, so a caption under
    /// a picture sits under the picture. The parent's alignment places the fitted box. Use it
    /// for a picture or video in a bounded cell: `el().aspect_fit(w / h).pixels_fit(p, Fill)`.
    /// In a column with no bounded height, bound it with `max_h`. It sets both sides, so `w`
    /// and `h` don't apply; anything not positive removes it.
    pub fn aspect_fit(mut self, ratio: f32) -> Self {
        self.aspect_fit = if ratio.is_finite() && ratio > 0.0 { ratio } else { 0.0 };
        self
    }
    pub fn w_full(mut self) -> Self {
        self.style.size.width = percent(1.0);
        self
    }
    /// Width as a fraction of the parent's (CSS `width: 40%` is `.w_frac(0.4)`).
    pub fn w_frac(mut self, f: f32) -> Self {
        self.style.size.width = percent(f);
        self
    }
    pub fn h_full(mut self) -> Self {
        self.style.size.height = percent(1.0);
        self
    }
    /// Take a `g` share of the free space along the parent's main axis (CSS `flex: g 1 0`).
    ///
    /// This also sets flex-basis to 0 and allows shrinking, so the element's own content does
    /// not size it: it is only as big as the space the parent has left over. Inside a parent
    /// with no bounded height (an auto-height column) there is no free space, and a grown
    /// `.scroll(..)` viewport collapses to 0 pt. Bound an ancestor's height (`.h(..)`,
    /// `.h_full()`, or a grown child of a bounded parent), or use `.min_h(..)` for a floor.
    /// A grown column that holds its own flex content also needs `.min_h(0.0)` (see
    /// [`El::min_h`]).
    pub fn grow(mut self, g: f32) -> Self {
        self.style.flex_grow = g;
        self.style.flex_shrink = 1.0;
        self.style.flex_basis = length(0.0);
        self
    }
    pub fn padding(mut self, p: f32) -> Self {
        self.style.padding = Rect { left: length(p), right: length(p), top: length(p), bottom: length(p) };
        self
    }
    pub fn px(mut self, p: f32) -> Self {
        self.style.padding.left = length(p);
        self.style.padding.right = length(p);
        self
    }
    pub fn py(mut self, p: f32) -> Self {
        self.style.padding.top = length(p);
        self.style.padding.bottom = length(p);
        self
    }
    pub fn pt(mut self, p: f32) -> Self {
        self.style.padding.top = length(p);
        self
    }
    pub fn pb(mut self, p: f32) -> Self {
        self.style.padding.bottom = length(p);
        self
    }
    pub fn pl(mut self, p: f32) -> Self {
        self.style.padding.left = length(p);
        self
    }
    pub fn ml(mut self, m: f32) -> Self {
        self.style.margin.left = length(m);
        self
    }
    pub fn mr(mut self, m: f32) -> Self {
        self.style.margin.right = length(m);
        self
    }
    pub fn gap(mut self, g: f32) -> Self {
        self.style.gap = Size { width: length(g), height: length(g) };
        self
    }
    /// Centre children on the cross axis.
    pub fn center_y(mut self) -> Self {
        self.style.align_items = AlignItems::CENTER;
        self
    }
    pub fn center(mut self) -> Self {
        self.style.align_items = AlignItems::CENTER;
        self.style.justify_content = JustifyContent::CENTER;
        self
    }
    pub fn justify_between(mut self) -> Self {
        self.style.justify_content = JustifyContent::SPACE_BETWEEN;
        self
    }
    pub fn justify_end(mut self) -> Self {
        self.style.justify_content = JustifyContent::FLEX_END;
        self
    }
    /// Wrap children onto new lines (CSS `flex-wrap: wrap`).
    pub fn flex_wrap(mut self) -> Self {
        self.style.flex_wrap = FlexWrap::Wrap;
        self
    }
    /// Align children to the start of the cross axis (CSS `align-items: flex-start`).
    pub fn items_start(mut self) -> Self {
        self.style.align_items = AlignItems::FLEX_START;
        self
    }
    /// Align children to the end of the cross axis (CSS `align-items: flex-end`).
    pub fn items_end(mut self) -> Self {
        self.style.align_items = AlignItems::FLEX_END;
        self
    }
    /// Centre children on the cross axis (CSS `align-items: center`; same as `center_y`).
    pub fn items_center(mut self) -> Self {
        self.style.align_items = AlignItems::CENTER;
        self
    }
    /// Stretch children across the cross axis (CSS `align-items: stretch`).
    pub fn items_stretch(mut self) -> Self {
        self.style.align_items = AlignItems::STRETCH;
        self
    }
    /// This element's own cross-axis alignment (CSS `align-self`).
    pub fn self_start(mut self) -> Self {
        self.style.align_self = Some(AlignSelf::FLEX_START);
        self
    }
    pub fn self_center(mut self) -> Self {
        self.style.align_self = Some(AlignSelf::CENTER);
        self
    }
    pub fn self_end(mut self) -> Self {
        self.style.align_self = Some(AlignSelf::FLEX_END);
        self
    }
    pub fn self_stretch(mut self) -> Self {
        self.style.align_self = Some(AlignSelf::STRETCH);
        self
    }
    /// Pack children at the start of the main axis (CSS `justify-content: flex-start`).
    pub fn justify_start(mut self) -> Self {
        self.style.justify_content = JustifyContent::FLEX_START;
        self
    }
    /// Centre children on the main axis (CSS `justify-content: center`).
    pub fn justify_center(mut self) -> Self {
        self.style.justify_content = JustifyContent::CENTER;
        self
    }
    /// CSS `justify-content: space-around`.
    pub fn justify_around(mut self) -> Self {
        self.style.justify_content = JustifyContent::SPACE_AROUND;
        self
    }
    /// CSS `justify-content: space-evenly`.
    pub fn justify_evenly(mut self) -> Self {
        self.style.justify_content = JustifyContent::SPACE_EVENLY;
        self
    }
    pub fn min_w(mut self, w: f32) -> Self {
        self.style.min_size.width = length(w);
        self
    }
    pub fn max_h(mut self, h: f32) -> Self {
        self.style.max_size.height = length(h);
        self
    }
    /// Separate row and column gaps (CSS `row-gap`, `column-gap`).
    pub fn gaps(mut self, row: f32, column: f32) -> Self {
        self.style.gap = Size { width: length(column), height: length(row) };
        self
    }
    pub fn max_w(mut self, w: f32) -> Self {
        self.style.max_size.width = length(w);
        self
    }
    /// Minimum height in points.
    ///
    /// Flex items default to a minimum of their content height (CSS `min-height: auto`), so a
    /// nested flex column with tall content overflows its parent instead of shrinking, and a
    /// scroll viewport inside it never gets to scroll. Put `.min_h(0.0)` on each flex column
    /// between the bounded ancestor and the viewport:
    ///
    /// ```ignore
    /// col().h_full().child(header).child(
    ///     col().grow(1.0).min_h(0.0).child(col().scroll(ScrollSpec { stick_bottom: false }).grow(1.0).child(list)),
    /// )
    /// ```
    pub fn min_h(mut self, h: f32) -> Self {
        self.style.min_size.height = length(h);
        self
    }
    /// Left and right margin together (CSS `margin-inline`).
    pub fn mx(mut self, m: f32) -> Self {
        self.style.margin.left = length(m);
        self.style.margin.right = length(m);
        self
    }
    /// Top and bottom margin together (CSS `margin-block`).
    pub fn my(mut self, m: f32) -> Self {
        self.style.margin.top = length(m);
        self.style.margin.bottom = length(m);
        self
    }
    pub fn mt(mut self, m: f32) -> Self {
        self.style.margin.top = length(m);
        self
    }
    pub fn mb(mut self, m: f32) -> Self {
        self.style.margin.bottom = length(m);
        self
    }
    pub fn pr(mut self, p: f32) -> Self {
        self.style.padding.right = length(p);
        self
    }
    /// Allow shrinking below content size along the parent's main axis.
    pub fn shrink(mut self) -> Self {
        self.style.flex_shrink = 1.0;
        self.style.min_size.width = length(0.0);
        self
    }
    /// Lay children out in `n` equal columns, each as wide as the widest child needs (CSS
    /// `grid-template-columns: repeat(n, 1fr)`), or sharing the width the element is given.
    /// Children fill the cells in order unless placed with `grid_cell`.
    pub fn equal_columns(mut self, n: usize) -> Self {
        self.style.display = Display::Grid;
        self.style.grid_template_columns = vec![fr(1.0); n.max(1)];
        self
    }
    /// Place this child of an `equal_columns` element in `column` (0-based) of the first row.
    /// Children in the same cell overlap in order, so a moving highlight can sit under a label.
    pub fn grid_cell(mut self, column: usize) -> Self {
        self.style.grid_column = Line { start: line(column as i16 + 1), end: span(1) };
        self.style.grid_row = Line { start: line(1), end: span(1) };
        self
    }
    /// Out of flow, positioned at `left, top` inside the parent's padding box.
    pub fn absolute(mut self, left: f32, top: f32) -> Self {
        self.style.position = Position::Absolute;
        self.style.inset = Rect { left: length(left), top: length(top), right: auto(), bottom: auto() };
        self
    }
    /// Out of flow, pinned to the given edges of the parent's padding box (`None` leaves an
    /// edge free); pinning both opposite edges stretches the element between them.
    pub fn pin(mut self, left: Option<f32>, top: Option<f32>, right: Option<f32>, bottom: Option<f32>) -> Self {
        let edge = |v: Option<f32>| v.map_or(auto(), length);
        self.style.position = Position::Absolute;
        self.style.inset = Rect { left: edge(left), top: edge(top), right: edge(right), bottom: edge(bottom) };
        self
    }
    /// The pointer's shape while it is over this element or a descendant that sets none.
    /// Applies while the pointer is over a hit target (a pressable, draggable, hoverable or
    /// text element) inside this one.
    pub fn cursor(mut self, cursor: Cursor) -> Self {
        self.cursor = Some(cursor);
        self
    }
    /// Pressing this element moves the window, as a title bar does, and a double press zooms or
    /// minimises it as the system's titlebar setting says. Hit targets inside it (buttons,
    /// fields) keep their presses.
    pub fn window_drag(mut self) -> Self {
        self.window_drag = true;
        self
    }
    /// The caret of an editor the app draws itself rather than with `text_input`: while this
    /// element is drawn and no text field has focus, the window turns its input method on (dead
    /// keys, CJK, the emoji picker, dictation), places the candidate window at this element's
    /// drawn rect, and hands what it composes to [`App::on_ime`](crate::App::on_ime). Put it on
    /// the caret you draw, only while the editor has the keyboard. A focused field's caret wins,
    /// and a focused secure field keeps the input method off.
    pub fn ime_cursor(mut self) -> Self {
        self.ime_cursor = true;
        self
    }
    /// Mark the drawn, ancestor-clipped rectangle as native overlay input. Does not add a handler.
    pub fn hit(mut self) -> Self {
        self.vis.hit = true;
        self
    }

    /// Observe the pointer moving over this element, in window points, without making a native
    /// overlay input region. [`El::on_pointer_move_local`] gives the element's own points.
    pub fn on_pointer_move<S: 'static>(mut self, f: impl Fn(&mut S, PointerPress) + 'static) -> Self {
        self.on_pointer_move = Some(Rc::new(move |s, p, _| f(downcast::<S>(s), p.window())));
        self
    }
    /// [`El::on_pointer_move`] with the point in this element's own layout points as well as
    /// the window's (see [`PointerEvent`]), and a [`Tx`]: a canvas tracks the pointer in its own
    /// coordinates at any zoom, and a handler that only moves a [`Live`](crate::Live) value
    /// (a crosshair) says so with [`Tx::live_only`].
    ///
    /// ```ignore
    /// .on_pointer_move_local(|s: &mut Canvas, p, tx| {
    ///     s.crosshair.set([p.x, p.y]);
    ///     tx.live_only();
    /// })
    /// ```
    pub fn on_pointer_move_local<S: 'static>(mut self, f: impl Fn(&mut S, PointerEvent, &mut Tx) + 'static) -> Self {
        self.on_pointer_move = Some(Rc::new(move |s, p, tx| f(downcast::<S>(s), p, tx)));
        self
    }

    /// Called once when the pointer enters this hit target, not on animation frames.
    pub fn on_hover<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.on_hover = Some(Rc::new(move |s, _| f(downcast::<S>(s))));
        self
    }

    /// Called with `true` when the pointer enters this element's visible rect and `false` when
    /// it leaves, descendants included (moving onto a child button is not a leave). It is not a
    /// hit target, so it never takes presses or hover from its children. A held press keeps the
    /// pointer inside until it releases.
    pub fn on_hover_change<S: 'static>(mut self, f: impl Fn(&mut S, bool, &mut Tx) + 'static) -> Self {
        self.on_hover_change = Some(Rc::new(move |s, inside, tx| f(downcast::<S>(s), inside, tx)));
        self
    }

    /// Delay [`Self::on_hover_change`]: `true` arrives once the pointer has rested inside for
    /// `enter`, and `false` once it has stayed out for `leave`. A pass shorter than the delay
    /// reports nothing, and a held button never starts the wait (a press during it cancels it;
    /// the wait starts again when the pointer next moves). A sidebar that peeks open when the pointer
    /// rests at the window's edge uses `hover_delays(40 ms, 0)`.
    pub fn hover_delays(mut self, enter: std::time::Duration, leave: std::time::Duration) -> Self {
        self.hover_delay = [enter.as_secs_f32(), leave.as_secs_f32()];
        self
    }

    // Visuals
    pub fn fill(mut self, c: Color) -> Self {
        self.vis.fill = c;
        self
    }
    pub fn radius(mut self, r: f32) -> Self {
        self.vis.radius = [r; 4];
        self
    }
    /// Round the corners with Apple's continuous curve (`CALayerCornerCurve.continuous`, SwiftUI's
    /// `.continuous` style), which eases into the straight edges over about 1.53 times the radius
    /// instead of meeting them at a circular arc. Applies to the fill, border, image, outline,
    /// `clip_rounded` and system material; shadows keep circular corners, like a CALayer
    /// shadow from a `CGPath(roundedRect:)`.
    pub fn continuous_corners(mut self) -> Self {
        self.vis.continuous_corners = true;
        self
    }
    /// Per-corner radius: top-left, top-right, bottom-right, bottom-left.
    pub fn corners(mut self, r: [f32; 4]) -> Self {
        self.vis.radius = r;
        self
    }
    /// A ring `width` points wide just inside the border box, following the corner radii. It
    /// takes no layout space and replaces the fill under it rather than drawing over it, so a
    /// translucent ring shows what's behind the element. For a stroke over the fill (SwiftUI's
    /// `strokeBorder`), put the border on an unfilled child the same size.
    pub fn border(mut self, width: f32, c: Color) -> Self {
        self.vis.border_width = width;
        self.vis.border = c;
        self.vis.border_bottom = None;
        self
    }
    /// A border whose colour runs from `top` at the top edge to `bottom` at the bottom edge,
    /// e.g. a top-lit hairline rim. Drawn like [`El::border`], inside the box in place of the
    /// fill. Replaces any `border`, and `border` replaces it.
    pub fn border_gradient(mut self, width: f32, top: Color, bottom: Color) -> Self {
        self.vis.border_width = width;
        self.vis.border = top;
        self.vis.border_bottom = Some(bottom);
        self
    }
    /// Dashes the border (`border` or `border_gradient`): `dash` points on, `gap` points off,
    /// with butt ends, along the ring's centre line, round the corners too. The pattern starts
    /// with a dash where the top edge leaves the top-left corner and runs clockwise. The period
    /// stretches or shrinks a little so a whole number of dashes fits, and the last gap meets the
    /// first dash cleanly. Unlike a solid border, the fill shows through the gaps and the dashes
    /// draw over it. A zero `dash` or `gap` keeps the border solid.
    pub fn border_dash(mut self, dash: f32, gap: f32) -> Self {
        let valid = dash.is_finite() && gap.is_finite() && dash > 0.0 && gap > 0.0;
        self.vis.border_dash = valid.then_some([dash, gap]);
        self
    }
    /// Starts the dash pattern `phase` points into itself (SwiftUI's `dashPhase`), so growing it
    /// marches the dashes anticlockwise. A phase of `dash + gap` is one whole period. It is
    /// applied as given every frame and never springs (a spring would run a wrapped phase
    /// backwards), so drive it from a clock for marching ants.
    pub fn border_dash_phase(mut self, phase: f32) -> Self {
        self.vis.border_dash_phase = if phase.is_finite() { phase } else { 0.0 };
        self
    }
    /// An outer drop shadow offset by `(0, y)` with CSS `blur` radius (`blur = 2σ`; pass
    /// `2.0 * r` to match `CALayer.shadowRadius`). Springs like [`Self::box_shadow`].
    pub fn shadow(mut self, y: f32, blur: f32, color: Color) -> Self {
        self.vis.shadows.push(Shadow { y, blur, color, ..Shadow::default() });
        self
    }
    /// A full box shadow (offset, spread, inset), e.g. one layer of a CSS elevation token.
    /// `s.blur` follows CSS (`blur = 2σ`, twice `CALayer.shadowRadius`). The first four
    /// shadows spring: a change to one's offset, blur or spread moves on the element's layout
    /// spring and its colour on the colour spring, so a pressed card's shadow tightens as it
    /// sinks. Under Reduce Motion, and in an instant transaction, the geometry lands at once.
    pub fn box_shadow(mut self, s: Shadow) -> Self {
        self.vis.shadows.push(s);
        self
    }
    /// A vertical gradient from `top` to `bottom` painted over the fill (CSS `linear-gradient(180deg, ..)`).
    pub fn gradient(mut self, top: Color, bottom: Color) -> Self {
        self.vis.gradient = Some(Box::new([top, bottom]));
        self
    }
    /// A radial gradient painted over the fill and clipped to the element's rounded corners.
    ///
    /// `center` is in normalized element coordinates (`[0.5, 0.5]` is the centre, `[0.0, 0.0]` top-left).
    /// `radius` is in points (`f32` for a circle or `[rx, ry]` for an ellipse).
    /// `stops` are up to 8 `(position, color)` pairs along `0.0..=1.0` from `center` (`0.0`) to
    /// `radius` (`1.0`). Stop colours spring in Oklab when changed and interpolate in premultiplied
    /// colour across the quad, matching [`El::gradient`].
    pub fn radial_gradient(
        mut self,
        center: [f32; 2],
        radius: impl Into<RadialRadius>,
        stops: impl AsRef<[(f32, Color)]>,
    ) -> Self {
        let [rx, ry] = radius.into().0;
        let valid = center[0].is_finite()
            && center[1].is_finite()
            && rx.is_finite()
            && ry.is_finite()
            && rx > 0.0
            && ry > 0.0;
        self.vis.radial = valid
            .then(|| normalize_gradient_stops(GradientGeometry::Radial { center, radius: [rx, ry] }, stops.as_ref()))
            .flatten()
            .map(Box::new);
        self
    }
    /// A linear gradient at `angle` radians (CSS convention: `0.0` is bottom-to-top, `PI / 2` is
    /// left-to-right, `PI` is top-to-bottom like [`El::gradient`]) with up to 8 stops, painted over
    /// the fill.
    pub fn linear_gradient(mut self, angle: f32, stops: impl AsRef<[(f32, Color)]>) -> Self {
        self.vis.radial = angle
            .is_finite()
            .then(|| normalize_gradient_stops(GradientGeometry::Linear { angle }, stops.as_ref()))
            .flatten()
            .map(Box::new);
        self
    }
    /// Paints `shape` (strokes and fills in this element's points, origin top-left) over the
    /// fill and under any text. It does not affect layout or hit testing.
    pub fn shape(mut self, shape: Shape) -> Self {
        self.vis.shape = (!shape.is_empty()).then(|| Rc::new(shape));
        self
    }
    /// Shows `pixels`, scaled into the element's rect and clipped to its corner radius. The image
    /// can change without the view re-running: the next frame draws its latest contents.
    /// An element that sets only its width (`w`, `w_full`) or only its height takes the other
    /// side from the image's aspect ratio, updating when the image decodes or changes shape;
    /// [`aspect_ratio`](Self::aspect_ratio) overrides it.
    pub fn pixels(mut self, pixels: &crate::pixels::Pixels) -> Self {
        self.vis.pixels = Some(pixels.clone());
        self.vis.pixels_fit = None;
        self
    }
    /// Shows `pixels` at its own aspect ratio, centred: `Fit` letterboxes the whole image inside
    /// the rect (the corner radius rounds the image), `Fill` covers the rect and crops the overflow.
    pub fn pixels_fit(mut self, pixels: &crate::pixels::Pixels, fit: crate::SurfaceFit) -> Self {
        self.vis.pixels = Some(pixels.clone());
        self.vis.pixels_fit = Some(fit);
        self
    }
    /// A ring `offset` points outside the border box (CSS `outline` + `outline-offset`).
    pub fn outline(mut self, width: f32, offset: f32, color: Color) -> Self {
        self.vis.outline = Some(Outline { width, offset, color });
        self
    }
    pub fn opacity(mut self, o: f32) -> Self {
        self.vis.opacity = o;
        self
    }
    /// Gaussian blur of the completed subtree, in points (standard deviation). Zero has no
    /// filter cost. The renderer caps sigma at 64 device pixels; bounds grow by three sigma.
    pub fn blur(mut self, sigma: f32) -> Self {
        self.vis.blur = sigma.max(0.0);
        self
    }
    /// Override the blur target's curve. Ordinary changes use the element's motion spring.
    pub fn blur_motion(mut self, anim: impl Into<Anim>) -> Self {
        self.vis.blur_motion = Some(Box::new(anim.into()));
        self
    }
    /// Dim each paint leaf in the currently built subtree, independently. Unlike `opacity`,
    /// overlapping draws retain their individual coverage and no group layer is introduced. A
    /// box with a solid fill still fades as `opacity` would: its outer shadows stay outside it
    /// and its fill and gradient fade as one.
    /// Call after adding children. Useful for static disabled styling; animation and hit-test
    /// opacity still belong to `opacity` / `live_opacity`.
    pub fn dim(mut self, alpha: f32) -> Self {
        fn apply(e: &mut El, alpha: f32) {
            e.expand_memo();
            e.vis.paint_alpha *= alpha;
            for child in &mut e.children { apply(child, alpha); }
        }
        apply(&mut self, alpha.clamp(0.0, 1.0));
        self
    }
    /// Add children's premultiplied colours while a direct child is between opacity 0 and 1.
    /// Give two overlaid states opacity `1-t` and `t` for a dissolve without a midpoint alpha dip.
    /// Put a shared background outside this scope; its own paint also participates in addition.
    /// At rest (all direct children at 0 or 1), draw normally without a dissolve layer. Overlapping
    /// opaque states therefore use source-over at rest; this API is for replacing states.
    pub fn dissolve(mut self) -> Self {
        self.vis.blend = crate::render::Blend::PlusLighter;
        self
    }
    /// Rasterize this subtree once into an offscreen texture when settled and composite that
    /// texture until the subtree's draw output changes, including while an ancestor's
    /// [`live_offset`](Self::live_offset) or [`live_scale`](Self::live_scale) moves it. While a
    /// spring, transition, or effect slot inside the subtree is active, it draws live and
    /// re-caches once settled.
    pub fn cache(mut self) -> Self {
        self.vis.cache = true;
        self
    }
    /// Cache this subtree like [`Self::cache`], also mixing `key` into its invalidation signature.
    pub fn cache_key(mut self, key: u64) -> Self {
        self.vis.cache = true;
        self.vis.cache_key = Some(key);
        self
    }
    /// Tags this element's instances (and its descendants', until one sets its own) for a
    /// compositor: `Runtime::marks` runs parallel to `Runtime::instances`.
    pub fn mark(mut self, mark: u32) -> Self {
        self.vis.mark = mark;
        self
    }
    /// Overlay drawn over the fill while hovered.
    pub fn hover(mut self, c: Color) -> Self {
        self.vis.hover = c;
        self
    }
    /// Overlay drawn over the fill while pressed.
    pub fn press(mut self, c: Color) -> Self {
        self.vis.press = c;
        self
    }
    /// Shrink to `scale` about the centre while a press on this element is held, as iOS tiles
    /// and buttons do (0.96 to 0.97), springing back on release or when the pointer leaves.
    /// Put it on the element with `on_press`. Layout and hit testing keep the resting size, and
    /// Reduce Motion turns it off. `1.0` (or anything not positive) removes it.
    pub fn press_scale(mut self, scale: f32) -> Self {
        self.press_scale = (scale.is_finite() && scale > 0.0 && scale != 1.0).then_some(scale);
        self
    }
    /// Draw this element and everything in it `k` times its size about its centre, like
    /// SwiftUI's `scaleEffect`: fills, borders, text, icons and images all scale, and text
    /// rasterizes at the scaled size once it rests. Layout keeps the unscaled size, so siblings
    /// don't move; hit testing follows the scaled rect. A change springs on the element's layout
    /// spring (or [`Self::scale_motion`]); under Reduce Motion, and in an instant transaction, it
    /// lands at once. Multiplies with transitions, `live_scale` and `press_scale`. Anything not
    /// finite or below zero is `1.0`.
    pub fn scale(mut self, k: f32) -> Self {
        self.scale = if k.is_finite() && k >= 0.0 { k } else { 1.0 };
        self
    }
    /// The curve [`Self::scale`] changes on.
    pub fn scale_motion(mut self, anim: impl Into<Anim>) -> Self {
        self.vis.scale_motion = Some(Box::new(anim.into()));
        self
    }
    /// Draw this element and everything in it turned `radians` clockwise about its
    /// [`origin`](Self::origin), like SwiftUI's `rotationEffect`: fills, borders, shadows,
    /// text, icons and images all turn. Layout keeps the upright box, so siblings don't move.
    /// A change springs on the element's layout spring (or [`Self::rotate_motion`]); under
    /// Reduce Motion, and in an instant transaction, it lands at once. While turned, the
    /// subtree draws into its own layer, and a native surface inside it (a video, a web view)
    /// stays upright. Hit testing follows the turned box; the reported rect uses its upright
    /// bounding box. Anything not finite is `0.0`.
    pub fn rotate(mut self, radians: f32) -> Self {
        self.vis.rotate = if radians.is_finite() { radians } else { 0.0 };
        self
    }
    /// The curve [`Self::rotate`] changes on.
    pub fn rotate_motion(mut self, anim: impl Into<Anim>) -> Self {
        self.vis.rotate_motion = Some(Box::new(anim.into()));
        self
    }
    /// The point this element scales and rotates about, in units of its own size: `(0.5, 0.5)`
    /// (the default) is its centre, `(0.5, 1.0)` the middle of its bottom edge, which stays put
    /// while a keycap sinks. Like SwiftUI's `anchor:`. [`Self::scale`], [`Self::rotate`],
    /// `live_scale`, `press_scale` and a transition's scale all pivot here once it's set;
    /// [`Self::offset`] ignores it. Values outside 0..1 pivot outside the element; anything not
    /// finite is the centre.
    pub fn origin(mut self, x: f32, y: f32) -> Self {
        self.vis.origin = (x.is_finite() && y.is_finite() && (x, y) != (0.5, 0.5)).then_some([x, y]);
        self
    }
    /// Run the animations of this element and everything in it at `k` times normal speed:
    /// springs, transitions (enter, exit, layout), implicit animations, hover and press
    /// fades, loops and word reveals. `0.0` freezes them exactly where they are, `0.5` is slow
    /// motion, and changing `k` carries on from the same place. Nested scales multiply, and
    /// all of them multiply with the window's [`Cx::time_scale`](crate::Cx::time_scale).
    ///
    /// Only time slows. State still updates at once: a hover or press is tracked as usual and
    /// only its fade waits. An exiting element drops its own factor and follows its ancestors
    /// still in the tree, so one under a frozen subtree stays mid-exit until that scale rises,
    /// then finishes and is removed. A frozen subtree requests no frames.
    /// Scrolling, growth stacks and `Tween` keep the window's clock: they follow input and
    /// layout, and a frozen scroll view would ignore the wheel. Anything not finite or below
    /// zero is `1.0`.
    pub fn time_scale(mut self, k: f32) -> Self {
        self.time_scale = if k.is_finite() && k >= 0.0 { k } else { 1.0 };
        self
    }
    /// Clip children to this box with soft gradient edges (never a hard crop).
    pub fn clip_soft(mut self, fade: f32) -> Self {
        self.vis.clip = Some([fade; 4]);
        self
    }
    /// Clip this element and its descendants to its presented rounded rectangle. Follows
    /// [`El::corners`] and [`El::continuous_corners`] when set on this element; otherwise rounds
    /// all four corners by `radius` points (capped at half the shorter edge). Filters run before
    /// this mask. Nested masks intersect without moving either rectangle's corners; complex
    /// intersections use a layer. Hit testing uses the rectangular clip bounds, including visually
    /// clipped corner areas.
    pub fn clip_rounded(mut self, radius: f32) -> Self {
        self.vis.clip_corners = None;
        self.vis.clip_radius = Some(radius.max(0.0));
        self
    }
    /// Clip this element and its descendants to its presented rectangle with per-corner radii
    /// `[top_left, top_right, bottom_right, bottom_left]` in points, capped at half the shorter
    /// edge and following [`El::continuous_corners`]. Unlike [`El::clip_rounded`], ignores the
    /// element's paint radii. Hit testing still uses the rectangular clip bounds.
    pub fn clip_corners(mut self, radii: [f32; 4]) -> Self {
        let clamped = radii.map(|r| r.max(0.0));
        self.vis.clip_corners = Some(clamped);
        self.vis.clip_radius = Some(clamped.into_iter().fold(0.0, f32::max));
        self
    }
    /// Soft clip with a fade width per side (left, top, right, bottom).
    pub fn clip_fade(mut self, left: f32, top: f32, right: f32, bottom: f32) -> Self {
        self.vis.clip = Some([left, top, right, bottom]);
        self
    }
    /// Track hover for descendants' `group_*` styles (see [`Visual::hover_group`]).
    pub fn hover_group(mut self) -> Self {
        self.vis.hover_group = true;
        self
    }
    /// Curves for this element's hover track: `enter` when the pointer arrives, `exit` when it
    /// leaves (e.g. a delayed reveal).
    pub fn hover_motion(mut self, enter: impl Into<Anim>, exit: impl Into<Anim>) -> Self {
        self.vis.hover_motion = Some(Rc::new((enter.into(), exit.into())));
        self
    }
    /// Default hover curves for this element and everything below it: any hover track (an
    /// element with [`Self::hover`] or [`Self::hover_group`]) without its own
    /// [`Self::hover_motion`] uses the nearest ancestor's pair, and [`Self::anchored`] overlays
    /// inherit from their anchor, so kit controls whose inner elements you can't reach follow it
    /// too. Set it on the root of the view (or on what you pass to `rune_kit::root`, which
    /// carries it to the overlays it layers beside the view). Without any, hover lands at once
    /// and fades out over 0.1 s. Rows highlighted by `focus_highlight` (menu items, select
    /// options) are never faded.
    pub fn hover_motion_within(mut self, enter: impl Into<Anim>, exit: impl Into<Anim>) -> Self {
        self.vis.hover_within = Some(Rc::new((enter.into(), exit.into())));
        self
    }
    /// The pair set by [`Self::hover_motion_within`].
    pub fn hover_within_curves(&self) -> Option<(Anim, Anim)> {
        self.vis.hover_within.as_deref().copied()
    }
    /// Opacity factor `rest` normally and `hovered` while the nearest ancestor group is hovered.
    pub fn group_opacity(mut self, rest: f32, hovered: f32) -> Self {
        self.vis.group_opacity = Some([rest, hovered]);
        self
    }
    /// Text or icon colour while the nearest ancestor group is hovered.
    pub fn group_color(mut self, c: Color) -> Self {
        self.vis.group_color = Some(c);
        self
    }
    /// Wall-clock opacity loop, using the shared loop engine and host wake deadline.
    pub fn cycle(self, c: Cycle) -> Self {
        self.looping(rune_motion::LoopSpec::cycle(c))
    }

    // Text
    pub fn font(mut self, s: TextStyle) -> Self {
        if let Some(t) = &mut self.text {
            t.style = s;
        }
        if self.input.is_some() {
            self.sync_input_rich();
        }
        self
    }
    /// Per-frame styled or coloured byte ranges (syntax highlights, mentions, inline styles)
    /// painted over a [`text`], [`rich`] or [`text_input`] element without mutating [`TextEdit`].
    /// On a [`text_input`], byte offsets are relative to `edit.text` and shift automatically
    /// across any active IME preedit.
    pub fn spans(mut self, spans: impl Into<Vec<Span>>) -> Self {
        let spans = spans.into();
        if let Some(inp) = &mut self.input {
            inp.decorations = spans;
            self.sync_input_rich();
        } else if let Some(t) = &mut self.text {
            t.spans = crate::el_pills::with_pill_colors(&spans, &t.pills);
        }
        self
    }
    /// Alias for [`El::spans`]: per-frame syntax or highlight [`Span`]s painted over text
    /// (for example `SyntaxTheme::from_kit(&kit).spans(&highlights, code_offset)`).
    pub fn decorations(self, spans: impl Into<Vec<Span>>) -> Self {
        self.spans(spans)
    }
    /// An inset `color` border ring `width` points wide inside the rounded [`Span::bg`] of bytes
    /// `start..end`. Touching runs of the range on one line share one ring; wrapped lines each
    /// get their own. No background means no ring. Later overlapping rings win.
    /// Ranges refer to authored UTF-8 bytes and follow text case and truncation. For static
    /// text only: editable text does not rebase these ranges after edits. Rebuild ranges when
    /// replacing the content. During streaming reveal a ringed chip fades with its last word
    /// on each line; selection is painted over the chip and below the glyphs.
    pub fn span_ring(mut self, start: usize, end: usize, color: Color, width: f32) -> Self {
        if let Some(t) = &mut self.text && start < end && width > 0.0 && width.is_finite() {
            t.rings.push((start as u32, end as u32, color, width));
        }
        self
    }
    /// Inset border rings `(start, end, color, width)` for multiple byte ranges (see [`El::span_ring`]).
    pub fn span_rings(mut self, rings: impl IntoIterator<Item = (usize, usize, Color, f32)>) -> Self {
        for (start, end, color, width) in rings {
            self = self.span_ring(start, end, color, width);
        }
        self
    }
    /// A strikethrough line across bytes `start..end` in `color` (`None`: the span's or text's
    /// colour). Wrapped lines each get their own segment.
    pub fn strike(mut self, start: usize, end: usize, color: Option<Color>) -> Self {
        if let Some(t) = &mut self.text && start < end {
            t.strikes.push((start as u32, end as u32, color));
        }
        self
    }
    /// Strikethrough lines `(start, end, color)` for multiple byte ranges (see [`El::strike`]).
    pub fn strikes(mut self, strikes: impl IntoIterator<Item = (usize, usize, Option<Color>)>) -> Self {
        for (start, end, color) in strikes {
            self = self.strike(start, end, color);
        }
        self
    }
    /// A paint-only [`TextHighlight`](crate::TextHighlight) over bytes of the text.
    ///
    /// Drawn from the shaped layout's glyph boxes without splitting runs, so adding, moving or
    /// clearing highlights never reshapes text or shifts line breaks.
    pub fn text_highlight(mut self, highlight: crate::text::TextHighlight) -> Self {
        if let Some(t) = &mut self.text && highlight.start < highlight.end {
            t.highlights.push(highlight);
        }
        self
    }
    /// Paint-only [`TextHighlight`](crate::TextHighlight)s over bytes of the text (see [`El::text_highlight`]).
    pub fn text_highlights(mut self, highlights: impl IntoIterator<Item = crate::text::TextHighlight>) -> Self {
        for h in highlights {
            self = self.text_highlight(h);
        }
        self
    }
    /// Set the visual palette for rich inline marks (`code`, `link`) and paragraph blocks
    /// (`quote`, `code block`, list markers) on a [`text_input`].
    pub fn rich_style(mut self, style: crate::edit::RichStyle) -> Self {
        if let Some(inp) = &mut self.input {
            inp.rich_style = style;
            self.sync_input_rich();
        } else if let Some(t) = &mut self.text {
            t.rich_style = style;
        }
        self
    }
    /// The chip colour of each [`Mark::Pill`](crate::Mark::Pill) in a [`text_input`], and its
    /// label and icon colour (`None`: the text's). The chip defaults to the rich style's link
    /// colour at 20% opacity.
    pub fn pill_colors(mut self, bg: Color, fg: Option<Color>) -> Self {
        if let Some(t) = &mut self.text {
            t.pill_colors = Some((bg, fg));
        }
        if self.input.is_some() {
            self.sync_input_rich();
        }
        self
    }
    /// The leading icon for each [`Mark::Pill`](crate::Mark::Pill) chip in a [`text_input`],
    /// by the pill's id (`None`: no icon). The icon takes the label's colour.
    ///
    /// ```ignore
    /// text_input(s.draft.clone(), body).pill_icon(|id| id.starts_with("file:").then_some(&FILE))
    /// ```
    ///
    /// The closure may capture state; it runs each time the field is built, so a pill follows
    /// whatever it captured. For colours too, see [`El::pill_style`].
    pub fn pill_icon(mut self, icon: impl Fn(&str) -> Option<&'static crate::Icon> + 'static) -> Self {
        if let Some(inp) = &mut self.input {
            inp.pill_icon = Some(Rc::new(icon));
            self.sync_input_pills();
            self.sync_input_rich();
        }
        self
    }
    /// The look of each [`Mark::Pill`](crate::Mark::Pill) chip in a [`text_input`] by the
    /// pill's id: chip colour, label colour and icon, so a person and an agent can differ.
    /// What the style leaves unset falls back to [`El::pill_colors`] and [`El::pill_icon`].
    /// The closure may capture app state; it runs each time the field is built, and what it
    /// returns is drawn, so a cached subtree restyles when that state changes.
    ///
    /// ```ignore
    /// text_input(s.draft.clone(), body).pill_style({
    ///     let agents = s.agents.clone();
    ///     move |id| if agents.contains(id) { PillStyle::default().bg(PURPLE).icon(&BOT) } else { PillStyle::default() }
    /// })
    /// ```
    pub fn pill_style(mut self, style: impl Fn(&str) -> PillStyle + 'static) -> Self {
        if let Some(inp) = &mut self.input {
            inp.pill_style = Some(Rc::new(style));
            self.sync_input_pills();
            self.sync_input_rich();
        }
        self
    }
    fn sync_input_pills(&mut self) {
        let (Some(inp), Some(t)) = (&self.input, &mut self.text) else { return };
        if inp.edit.is_plain() {
            t.pills.clear();
            return;
        }
        t.pills = inp
            .edit
            .display_pills()
            .into_iter()
            .map(|(start, end, id)| {
                let st = inp.pill_style.as_ref().map(|f| f(id)).unwrap_or_default();
                let icon = st.icon.or_else(|| inp.pill_icon.as_ref().and_then(|f| f(id)));
                crate::text::PillChip { start, end, icon, bg: st.bg, fg: st.fg }
            })
            .collect();
    }
    pub(crate) fn sync_input_rich(&mut self) {
        let (Some(inp), Some(t)) = (&self.input, &mut self.text) else { return };
        t.rich_style = inp.rich_style;
        if inp.edit.is_plain() && inp.decorations.is_empty() {
            // A plain field lays out and draws exactly as before rich editing existed.
            t.spans.clear();
            t.strikes.clear();
            t.blocks.clear();
            t.soft_breaks.clear();
            t.pills.clear();
            return;
        }
        let (mut base_spans, strikes, rich_links, blocks) =
            inp.edit.rich_display_specs(t.style, inp.rich_style);
        for d in &inp.decorations {
            for (ds, de) in inp.edit.map_range_to_display(d.start as usize, d.end as usize) {
                let mut sp = d.clone();
                sp.start = ds;
                sp.end = de;
                base_spans.push(sp);
            }
        }
        let pill_fg = t.pill_colors.and_then(|(_, fg)| fg);
        base_spans.extend(t.pills.iter().filter_map(|p| {
            Some(Span { start: p.start, end: p.end, color: Some(p.fg.or(pill_fg)?), ..Span::default() })
        }));
        let mut flat = crate::text::flatten_spans(&base_spans);
        for s in &mut flat {
            if s.underline == Some(Color::TRANSPARENT) {
                s.underline = Some(s.color.unwrap_or(t.color));
            }
        }
        t.spans = flat;
        t.strikes = strikes;
        t.blocks = blocks;
        t.soft_breaks = inp.edit.soft_breaks.clone();
        // Replace links generated from `Mark::Link` while keeping any explicit `.link(...)` calls.
        t.links = rich_links;
    }
    /// Wrap text at the element's width.
    pub fn wrap(mut self) -> Self {
        if let Some(t) = &mut self.text {
            t.wrap = true;
        }
        self
    }
    /// Wrap text at the element's width, breaking inside a word only when the word alone would
    /// overflow the line (`overflow-wrap: anywhere`) while breaking normal words at spaces, and
    /// lowering the text's min-content width. Does not change flex shrinking; use
    /// [`El::shrink`] to allow a flex row to narrow the element.
    pub fn wrap_anywhere(mut self) -> Self {
        if self.input.as_ref().is_some_and(|i| i.single_line) {
            return self;
        }
        if let Some(t) = &mut self.text {
            t.wrap = true;
            t.wrap_anywhere = true;
        }
        self
    }
    /// Whether this is text that wraps (`.wrap()`), for containers that let such text shrink
    /// to their width.
    pub fn wraps(&self) -> bool {
        self.text.as_ref().is_some_and(|t| t.wrap)
    }
    /// One line, cut with "…" when wider than the element; the element may shrink below its
    /// text's width. A [`text_input`] ignores it and the other truncations.
    pub fn truncate(self) -> Self {
        self.elide(Elide::End)
    }
    /// [`El::truncate`] keeping both ends: "…" replaces the middle, as AppKit's
    /// `NSLineBreakByTruncatingMiddle` does for paths and file names. Cuts fall between
    /// graphemes and refit when the element resizes; copying and the accessibility text keep
    /// the whole string.
    pub fn truncate_middle(self) -> Self {
        self.elide(Elide::Middle)
    }
    /// [`El::truncate`] keeping the end: "…" replaces the start.
    pub fn truncate_head(self) -> Self {
        self.elide(Elide::Head)
    }
    fn elide(mut self, elide: Elide) -> Self {
        // A field's caret and selection index its own text, which a cut would break.
        if self.input.is_some() { return self; }
        if let Some(t) = &mut self.text {
            t.truncate = true;
            t.elide = elide;
            t.truncate_fade = false;
            t.wrap = false;
            t.wrap_anywhere = false;
            t.max_lines = 0;
        }
        self.style.flex_shrink = 1.0;
        self.style.min_size.width = length(0.0);
        self
    }
    /// Wrapped text that stops after `lines` lines, "…" ending the last when more would follow,
    /// as CSS's `line-clamp`. The element measures at most `lines` lines tall. The cut falls
    /// between graphemes and refits when the element resizes; spans and links keep their
    /// looks up to it, and copying and the accessibility text keep the whole string. 0 shows
    /// every line. A [`text_input`] ignores it; use [`auto_grow`] there.
    pub fn max_lines(mut self, lines: usize) -> Self {
        if self.input.is_some() { return self; }
        if let Some(t) = &mut self.text {
            t.wrap = true;
            t.max_lines = lines;
            t.truncate = false;
            t.truncate_fade = false;
        }
        self
    }
    /// One line, clipped with a 1.5 em trailing fade only when it overflows. No ellipsis.
    pub fn truncate_fade(self) -> Self {
        self.truncate_fade_width(TRUNCATE_FADE_EM)
    }
    /// [`El::truncate_fade`] with a fade `em` ems wide.
    pub fn truncate_fade_width(mut self, em: f32) -> Self {
        if let Some(t) = &mut self.text {
            t.truncate = false;
            t.truncate_fade = true;
            t.fade_em = em.max(0.0);
            t.wrap = false;
            t.wrap_anywhere = false;
            t.max_lines = 0;
        }
        self.style.flex_shrink = 1.0;
        self.style.min_size.width = length(0.0);
        self
    }
    /// Streaming word reveal (see [`TextSpec::reveal`]).
    pub fn reveal(mut self, word_fade: f32) -> Self {
        if let Some(t) = &mut self.text {
            t.reveal = word_fade.max(0.0);
        }
        self
    }
    /// Keep a text field to one line: long text scrolls sideways under the caret instead of
    /// wrapping. Fields are multi-line by default.
    pub fn single_line(mut self) -> Self {
        if let Some(i) = &mut self.input {
            i.single_line = true;
            i.edit.set_single_line(true);
            if let Some(t) = &mut self.text {
                t.wrap = false;
                t.wrap_anywhere = false;
                if i.edit.is_secure() && !i.edit.is_revealed() {
                    t.replace_content(i.edit.display().0);
                }
            }
        }
        self
    }
    /// Make a byte range of this text pressable (see [`Link`]). Only the range's shaped glyph
    /// boxes take the pointer, which shows a hand over them; the rest of the text stays
    /// transparent to input. Each link is a Tab stop that Enter or Space presses.
    pub fn link(mut self, link: Link) -> Self {
        if let Some(t) = &mut self.text {
            t.links.push(link);
            self.focus.enabled = true;
            self.cursor.get_or_insert(Cursor::Pointer);
        }
        self
    }
    /// Called with a [`Link`]'s id when it is clicked, or pressed with Enter or Space while
    /// focused.
    pub fn on_link<S: 'static>(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self {
        if let Some(t) = &mut self.text {
            t.on_link = Some(LinkFn(Rc::new(move |s, id, tx| f(downcast::<S>(s), id, tx))));
        }
        self
    }
    /// Called with a [`Link`]'s id and the point (window points) when a secondary press
    /// (right-click, macOS control-click) lands on its glyphs: a menu for that link. It takes
    /// the press, so ancestors' [`El::on_secondary_press`] and the text's Copy menu don't see
    /// it; elsewhere in the text they still do.
    pub fn on_link_secondary<S: 'static>(mut self, f: impl Fn(&mut S, &str, PointerPress, &mut Tx) + 'static) -> Self {
        if let Some(t) = &mut self.text {
            t.on_link_secondary = Some(LinkSecondaryFn(Rc::new(move |s, id, p, tx| f(downcast::<S>(s), id, p, tx))));
        }
        self
    }
    pub fn placeholder(mut self, s: impl Into<String>, c: Color) -> Self {
        if let Some(i) = &mut self.input {
            i.placeholder = s.into();
            i.placeholder_color = c;
        }
        self
    }
    pub fn caret(mut self, caret: Color, selection: Color) -> Self {
        if let Some(i) = &mut self.input {
            i.caret = caret;
            i.selection = selection;
        }
        self
    }
    /// Lets people select this read-only text and copy it, as SwiftUI's
    /// `.textSelection(.enabled)`: drag across it, double-click a word, triple-click a paragraph,
    /// shift-click to extend; Cmd+C (Ctrl+C elsewhere) copies and Cmd+A selects all of it while it
    /// holds the selection; a right-click offers Copy on macOS. It shows the I-beam, isn't a Tab
    /// stop, and a press anywhere else clears the selection. Selecting never re-lays out the
    /// text. Set the highlight with [`El::selection_color`].
    ///
    /// Inside a row that handles input, the row keeps it: a click on the text presses the row,
    /// hover and press paint reach it, and its `on_secondary_press` wins over Copy. A drag that
    /// selects text doesn't press. Inside a draggable (`on_drag`, `drag_out`) row, dragging
    /// drags the row, and double- and triple-clicks still select.
    pub fn selectable(mut self, on: bool) -> Self {
        if let Some(t) = &mut self.text {
            if self.input.is_some() || t.redacted { return self; }
            t.selectable = on;
            if on && (self.cursor.is_none() || (!t.links.is_empty() && self.cursor == Some(Cursor::Pointer))) {
                self.cursor = Some(Cursor::Text);
            }
        }
        self
    }
    /// Makes the [`El::selectable`] texts inside this element one selection, in tree order, as
    /// the paragraphs of a web page are: a drag from one into another selects across them (the
    /// gaps between included), Shift-click extends into any of them, Cmd+A selects them all and
    /// Cmd+C copies their text joined by a newline, or a tab between texts side by side (table
    /// cells). Double- and triple-clicks still take a word or paragraph of one text. Text that
    /// isn't selectable (a list marker) is skipped. Outside a group each text is its own.
    pub fn selection_group(mut self, on: bool) -> Self {
        self.selection_group = on;
        self
    }
    /// What copying a text selection inside this element puts on the clipboard (Cmd+C and the
    /// Copy menu) instead of the selected text as drawn, so a document drawn from source can
    /// copy the source. `f` sees every [`El::selectable`] text inside this element that the
    /// selection could reach (those of its selection group), in tree order, each with its
    /// selected range (empty when it isn't selected) and its path relative to this element.
    /// Return `None` to copy the drawn text after all ([`TextSelection::text`]). The nearest
    /// such element around both ends of the selection answers. Kit's `Markdown::copy_source`
    /// is one.
    ///
    /// ```ignore
    /// col().selection_group(true)
    ///     .selection_copy(move |sel| sel.texts().iter().all(|t| t.is_whole()).then(|| note.clone()))
    /// ```
    ///
    /// [`TextSelection::text`]: crate::TextSelection::text
    pub fn selection_copy(mut self, f: impl Fn(&crate::TextSelection) -> Option<String> + 'static) -> Self {
        self.selection_copy = Some(Rc::new(f));
        self
    }
    /// Hears the [`El::selectable`] text selection change inside this element: when one starts
    /// (the first selected character of a drag, a double-click's word, Cmd+A,
    /// [`Tx::select_text`]), changes, leaves this element or clears. [`SelectionChange`] holds
    /// the whole selection while any of it is inside this element, `None` otherwise, and says
    /// whether it [`started`](crate::SelectionChange::started) or
    /// [`cleared`](crate::SelectionChange::cleared) here. It runs at most once a frame, however
    /// many times a drag moved the selection, before that frame's commit, and once more when the
    /// press making it releases ([`dragging`](crate::SelectionChange::dragging)). A press leaving
    /// only a caret isn't heard. Call [`Tx::live_only`] when the view reads nothing that changed.
    ///
    /// ```ignore
    /// col().selection_group(true).on_selection_change(|s: &mut Notes, change, _| {
    ///     if change.started() { s.whole_note_highlight = false; }
    /// })
    /// ```
    ///
    /// [`SelectionChange`]: crate::SelectionChange
    pub fn on_selection_change<S: 'static>(mut self, f: impl Fn(&mut S, &crate::SelectionChange, &mut Tx) + 'static) -> Self {
        self.on_selection_change = Some(Rc::new(move |s, change, tx| f(downcast::<S>(s), change, tx)));
        self
    }
    /// Draws and lays out, but the pointer and keyboard pass through this element and
    /// everything inside it, as with HTML's `inert`: a press, hover, scroll, drop or tooltip
    /// goes to whatever is under it (an ancestor or an element drawn below), along with that
    /// element's cursor. Handlers inside never run from input, nothing inside takes focus (Tab
    /// skips it, a field asking for focus doesn't get it, and focus already inside drops), and
    /// accessibility reports it inert. `on_layout` and `Cx::layout` still see its rects. For a
    /// document drawn under an editor that takes its presses itself.
    pub fn inert(mut self) -> Self {
        self.inert = true;
        self
    }
    /// Bytes of [`El::selectable`] text that draw but aren't copied, such as the spacing
    /// characters Markdown pads inline code with. A double-click's word stops at them.
    pub fn uncopied(mut self, range: std::ops::Range<usize>) -> Self {
        if let Some(t) = &mut self.text && range.start < range.end {
            let at = t.uncopied.partition_point(|r| (r.0 as usize) < range.start);
            t.uncopied.insert(at, (range.start as u32, range.end as u32));
        }
        self
    }
    /// The selection highlight of [`El::selectable`] text or a text field, drawn alike for both.
    pub fn selection_color(mut self, color: Color) -> Self {
        if let Some(i) = &mut self.input { i.selection = color; }
        if let Some(t) = &mut self.text { t.selection = color; }
        self
    }
    /// Binds the field to a `TextEdit` in app state. `f` may capture (a row index, say); wrap
    /// a closure in [`edit_access`] so its returned borrow is tied to the state.
    pub fn on_edit<S: 'static>(mut self, f: impl for<'a> Fn(&'a mut S) -> &'a mut TextEdit + 'static) -> Self {
        if let Some(i) = &mut self.input {
            i.access = Some(Rc::new(edit_fn(move |s| f(downcast::<S>(s)))));
        }
        self
    }
    /// Called when Return submits this text field. In a multi-line field (the default),
    /// Shift+Return and Option+Return insert a newline (as does plain Return inside a list, quote
    /// or code block), while plain Return in a paragraph calls `on_submit` — or presses the
    /// [`El::default_button`] when the field has none.
    pub fn on_submit<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        if let Some(i) = &mut self.input {
            i.submit = Some(Rc::new(move |s, tx| f(downcast::<S>(s), tx)));
        }
        self
    }
    /// Let dependent layout reflow when this input changes; the typed text remains immediate.
    pub fn animate_edit_layout(mut self) -> Self {
        if let Some(i) = &mut self.input { i.animate_layout = true; }
        self
    }
    /// A vertical scroll viewport over its children.
    ///
    /// The viewport needs a bounded height to scroll: a fixed `.h(..)`, or a `.grow(1.0)` share
    /// of a parent whose height is itself bounded. Every flex column between that bounded
    /// ancestor and the viewport needs `.min_h(0.0)`, or it grows to fit the content and the
    /// viewport never overflows. Debug builds warn once per call site when a viewport inside a
    /// `.grow()` container resolves to 0 pt tall while its content is not.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn scroll(mut self, spec: ScrollSpec) -> Self {
        self.scroll = Some(spec);
        self.scroll_axes = ScrollAxes::Y;
        #[cfg(debug_assertions)]
        { self.scroll_source = Some(std::panic::Location::caller()); }
        self.style.overflow = taffy::Point { x: taffy::Overflow::Hidden, y: taffy::Overflow::Scroll };
        self
    }
    /// A horizontal scroll viewport over its children: a filmstrip, a wide code block or
    /// table, a tab strip that overflows.
    ///
    /// The content is laid out at its natural width, unwrapped as SwiftUI's
    /// `ScrollView(.horizontal)` proposes, and at least as wide as the viewport (CSS
    /// `width: max-content; min-width: 100%`), and never shrinks. `w_full` content does the
    /// same; any other width it sets stays, narrower than the viewport too. The viewport takes its parent's width as any element does; its height is the
    /// content's unless set, and the content fills a set height unless it sets its own.
    ///
    /// The wheel, a trackpad and touch move it like a vertical viewport, with Rune's own
    /// momentum after a fling. A mostly-vertical trackpad swipe or touch pan that starts over
    /// it scrolls the vertical viewport around it instead, and the axis holds for the whole
    /// gesture, as in AppKit. Shift with a mouse wheel scrolls it on Windows, Linux and the web
    /// (macOS does that itself). Left and Right arrows don't scroll it (text fields own them),
    /// nor do Page Up and Page Down. [`El::scroll_edge_fade`] fades its left and right edges,
    /// and [`Tx::scroll_into_view`] scrolls it to show a row. `El::virtualize` and
    /// `El::virtual_rows` window vertical lists only.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn scroll_x(mut self) -> Self {
        self.scroll = Some(ScrollSpec { stick_bottom: false });
        self.scroll_axes = ScrollAxes::X;
        #[cfg(debug_assertions)]
        { self.scroll_source = Some(std::panic::Location::caller()); }
        self.style.overflow = taffy::Point { x: taffy::Overflow::Scroll, y: taffy::Overflow::Hidden };
        self
    }
    /// A viewport that scrolls both ways over its children (a wide timeline, a large
    /// table): [`El::scroll`] and [`El::scroll_x`] together. A trackpad gesture moves it along
    /// the axis it starts on, as AppKit's predominant-axis scrolling does. It keeps the
    /// `stick_bottom` of an earlier [`El::scroll`] call. Needs a bounded height like
    /// [`El::scroll`]. The content takes its natural width as in [`El::scroll_x`] and its
    /// natural height.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn scroll_xy(mut self) -> Self {
        self.scroll = Some(self.scroll.unwrap_or(ScrollSpec { stick_bottom: false }));
        self.scroll_axes = ScrollAxes::Both;
        #[cfg(debug_assertions)]
        { self.scroll_source = Some(std::panic::Location::caller()); }
        self.style.overflow = taffy::Point { x: taffy::Overflow::Scroll, y: taffy::Overflow::Scroll };
        self
    }
    /// On a scroll viewport, called with whether it shows its bottom (within a point): once
    /// after its first layout, then only when that changes, never per frame. Pair with
    /// [`Tx::scroll_to_bottom`] for a "jump to latest" button.
    pub fn on_at_bottom<S: 'static>(mut self, f: impl Fn(&mut S, bool) + 'static) -> Self {
        self.on_at_bottom = Some(Rc::new(move |s, at| f(downcast::<S>(s), at)));
        self
    }
    /// On a scroll viewport, called with whether it stands more than its own height, and at
    /// least `min` points, above its bottom: where a chat shows its "jump to latest" button and
    /// hides it again near the end. Once after its first layout, then only when that changes.
    pub fn on_far_from_bottom<S: 'static>(mut self, min: f32, f: impl Fn(&mut S, bool) + 'static) -> Self {
        self.on_far_from_bottom = Some((min, Rc::new(move |s, far| f(downcast::<S>(s), far))));
        self
    }
    /// On a scroll viewport, fade its top and bottom edges over `width` points (16–32 reads well),
    /// each only while content is hidden under it: the fade's strength is the hidden extent / 15
    /// points, so a viewport at its top (or bottom, or stuck to it) shows that edge hard. Replaces
    /// any top/bottom [`El::clip_fade`] widths; left/right ones still apply. On a horizontal
    /// viewport ([`El::scroll_x`]) the left and right edges fade the same way instead, and on
    /// [`El::scroll_xy`] all four. Sticky elements ([`El::sticky_top`], [`El::sticky_bottom`],
    /// [`El::sticky_left`]) are not faded on the edge they stick to, so a header's fill stays
    /// opaque while rows scrolling under it fade. This also applies before the element sticks;
    /// explicit [`El::clip_fade`] masks and outer viewports' fades still apply. Pair with
    /// [`El::scroll_margin`] so rows brought into view stop clear of the fade.
    pub fn scroll_edge_fade(mut self, width: f32) -> Self {
        self.scroll_edges.fade = width.max(0.0);
        self
    }
    /// On a scroll viewport, the space [`Tx::scroll_into_view`] leaves between a revealed row and
    /// the edge it scrolls to, like CSS `scroll-padding`. The content's own ends still stop flush.
    pub fn scroll_margin(mut self, pts: f32) -> Self {
        self.scroll_edges.margin = pts.max(0.0);
        self
    }
    /// Make this column a growth stack (see [`GrowthSpec`]). Required alongside
    /// [`El::virtualize`] on the same element.
    pub fn growth(mut self, spec: GrowthSpec) -> Self {
        self.growth = Some(Box::new(spec));
        self
    }
    /// A growth-stack row that is streaming: slows near its target and reveals through the
    /// feathered edge.
    pub fn streaming(mut self, on: bool) -> Self {
        self.streaming = on;
        self
    }
    /// A growth-stack row whose content arrives complete, like a sent message: its height
    /// changes glide on the growth spring alone, without the speed limits that pace streaming
    /// text, so a large insert and the pinned viewport following it move in one quick glide.
    pub fn arrives_whole(mut self) -> Self {
        self.whole = true;
        self
    }
    pub fn color(mut self, c: Color) -> Self {
        if let Some(t) = &mut self.text {
            t.color = c;
        }
        if let Some((_, ic)) = &mut self.vis.icon {
            *ic = c;
        }
        if self.input.is_some() {
            self.sync_input_rich();
        }
        self
    }
    pub fn align(mut self, a: Align) -> Self {
        if let Some(t) = &mut self.text {
            t.align = a;
        }
        self
    }
    /// Draw this text in capitals or lower case without changing its string, like SwiftUI's
    /// `textCase`: `text("Text channels").text_case(TextCase::Uppercase)` draws "TEXT CHANNELS".
    /// The dev tree, tools and link labels keep the authored text, and [`Span`] and [`Link`]
    /// ranges stay byte ranges of it. Casing follows `str::to_uppercase`, so "ß" draws "SS".
    /// Text fields draw what is typed and ignore it.
    pub fn text_case(mut self, case: crate::TextCase) -> Self {
        if let Some(t) = &mut self.text {
            t.case = Some(case);
        }
        self
    }
    /// Spread each wrapped line but the last to fill the width; the last line and unwrapped text
    /// keep [`El::align`].
    pub fn justify(mut self) -> Self {
        if let Some(t) = &mut self.text {
            t.justify = true;
        }
        self
    }
    /// Measure this text as the grapheme at byte `at` of `run`, shaped as one line in the same
    /// style, instead of on its own rounded up to a device pixel. Neighbouring cells of one run
    /// then sit at its fractional, kerned advances and together take its width, and each draws
    /// the run's own glyphs for its grapheme (contextual forms included).
    pub fn advance_in(mut self, run: &crate::text::Run, at: usize) -> Self {
        if let Some(t) = &mut self.text {
            t.advance_in = Some((run.clone(), at));
        }
        self
    }

    // Motion
    /// The spring for this subtree's layout and colour changes (inherited).
    /// Presence uses the transition's own enter/exit springs.
    pub fn motion(mut self, s: Spring) -> Self {
        self.motion = Some(s);
        self
    }
    /// Shift this element and its subtree by `(x, y)` points without moving it in layout:
    /// siblings keep their places, and hit tests follow the shift. A change springs on the
    /// element's layout spring (or [`Self::offset_motion`]); under Reduce Motion, and in an
    /// instant transaction, it lands at once. A reordered row slides aside this way while a
    /// dragged one passes it. When a commit both changes the offset and moves the element in a
    /// growth or virtual list (whose rows lay out at once), the offset starts from where the
    /// element was drawn, so a dropped row that clears its offset slides into its new place.
    /// Not for an element with a `live_offset` or a sliding loop, which own the same shift.
    pub fn offset(mut self, x: f32, y: f32) -> Self {
        let ok = |v: f32| if v.is_finite() { v } else { 0.0 };
        self.vis.offset = [ok(x), ok(y)];
        self
    }
    /// The curve [`Self::offset`] changes on.
    pub fn offset_motion(mut self, anim: impl Into<Anim>) -> Self {
        self.vis.offset_motion = Some(Box::new(anim.into()));
        self
    }
    /// Paint this element and its subtree above everything else in the window, and give it
    /// pointer input first, as if it came last in the tree. Its layout, focus order and
    /// accessibility order stay where it is, and its ancestors' clips, offsets and opacity
    /// still apply, though an ancestor that composites its children as a group (a partial
    /// opacity, a blur) no longer groups it. Raised elements stack in tree order. A floating
    /// sidebar sliding over the content beside it, or a dragged row over its neighbours.
    pub fn raise(mut self) -> Self {
        self.vis.raise = true;
        self
    }
    /// Override this element's opacity curve, including an optional entrance delay.
    /// The first paint and explicit instant/settle transactions still land immediately.
    pub fn opacity_motion(mut self, anim: impl Into<Anim>) -> Self {
        self.opacity_motion = Some(Box::new(anim.into()));
        self
    }
    /// When this element travels, its leading edge moves on `lead` and its trailing edge on `trail`
    /// (width leads).
    pub fn lead(mut self, lead: Spring, trail: Spring) -> Self {
        self.lead = Some((lead, trail));
        self
    }
    /// The X axis springs faster (`0.78×` duration) when this element resizes or travels.
    pub fn width_leads(mut self) -> Self {
        self.width_leads = true;
        self
    }
    /// Spring this subtree's horizontal layout (x and width) inside a growth-stack row, and
    /// fade in children it gains while the row is in place. A row's contents otherwise snap
    /// while the row's height carries the motion, so a label that changes width in place jumps
    /// its neighbours. Vertical place stays with the row's growth.
    pub fn animate_layout(mut self) -> Self {
        self.animate_layout = true;
        self
    }
    /// Land every change inside this element at once, as a [`Tx::instant`] commit would, even
    /// in a commit that animates the rest of the window: its own and its subtree's layout,
    /// opacity and transforms, the elements it gains or loses, and growth rows. Its own entrance
    /// and exit keep their transition, and colours (a hover highlight) keep their springs. A
    /// popup of search results under a field whose edits animate the layout around it.
    pub fn instant_changes(mut self) -> Self {
        self.instant_changes = true;
        self
    }
    /// How this element enters and exits when mounted or removed after the first frame
    /// (default: [`Transition::opacity()`] — a 0.2 s enter and 0.15 s exit fade). Because
    /// every element has this default fade, removing an element in an animated commit
    /// (`AppSpec::on_key`, `El::on_key`, `on_submit`, `on_press`) fades it out over 0.15 s,
    /// whereas commits that set [`Tx::instant`] or [`Tx::settle`] (such as typing in a
    /// focused input or clicking into a text field) unmount removed elements in the same
    /// frame. Call `tx.instant()` or `tx.settle()` in the handler when a key or button
    /// action should unmount elements at once. Rows inserted into a growth stack only grow
    /// open unless they set one, so `.transition(Transition::offset(0.0, 8.0).combined(Transition::opacity()))`
    /// on a row also slides and fades it in. Rows a virtual list remounts on scroll never replay it.
    pub fn transition(mut self, t: Transition) -> Self {
        self.transition = Some(Box::new(t));
        self
    }
    /// Shared-element motion (SwiftUI `matchedGeometryEffect`, Motion `layoutId`). When an
    /// element with `id` leaves the tree in the same commit that another with `id` mounts
    /// somewhere else, the new one starts exactly where the old one was seen, with its
    /// velocity, corner radius, fill and opacity, and springs to its own layout on one straight
    /// path. The old one disappears at once; neither plays its transition.
    ///
    /// Ids are per window. While it flies the element draws above the whole window at its own
    /// opacity, inside its destination's clips (a scroll viewport included), and its children
    /// keep their final layout, scaled uniformly to cover the moving frame and clipped to it,
    /// so a `pixels_fit(.., SurfaceFit::Fill)` picture zooms without distorting. Text moves
    /// with its box but keeps its size, as it does under any scale. The spring is
    /// `tok::motion::REFLOW` unless the commit's `Tx::motion` or this element's
    /// [`motion`](Self::motion) sets one. Under Reduce Motion nothing is matched: the old one
    /// exits and the new one enters by their own transitions, in place.
    /// When two elements share an id in one tree, the last one in tree order is matched.
    pub fn matched(mut self, id: impl Into<String>) -> Self {
        self.matched = Some(Rc::from(id.into()));
        self
    }

    // Handlers: queued, run between frames with `&mut S`.
    pub fn on_press<S: 'static>(self, f: impl Fn(&mut S) + 'static) -> Self {
        self.on_press_tx(move |s: &mut S, _tx: &mut Tx| f(s))
    }
    pub fn on_press_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.on_press = Some(Rc::new(move |s, tx| f(downcast::<S>(s), tx)));
        if self.role == "group" {
            self.role = "button";
        }
        self
    }
    /// Runs on the second of two presses on this element, as a double-click does: the second
    /// press lands within the system double-click interval of the first and within 4 pt of it,
    /// and releases inside. `on_press` still runs for each press, the first one right away, so a
    /// row selects on the first click and opens on the second, as in AppKit. A third press starts
    /// a new pair. Pointer only: the keyboard runs `on_press` (Space, and Return on a row).
    pub fn on_double_press<S: 'static>(self, f: impl Fn(&mut S) + 'static) -> Self {
        self.on_double_press_tx(move |s: &mut S, _tx: &mut Tx| f(s))
    }
    pub fn on_double_press_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.on_double_press = Some(Rc::new(move |s, tx| f(downcast::<S>(s), tx)));
        self
    }
    /// Receives Down, Move, then Up. Up also ends a cancelled gesture: always check
    /// [`Drag::cancelled`] before committing a drop or notifying an application callback.
    /// Save the original value on Down (in state, or [`Drag::data`]) and restore it on cancel.
    /// Move commits are instant unless the handler calls [`Tx::animated`].
    pub fn on_drag<S: 'static>(mut self, f: impl Fn(&mut S, &mut Drag, &mut Tx) + 'static) -> Self {
        self.on_drag = Some(Rc::new(move |s, d, tx| f(downcast::<S>(s), d, tx)));
        self
    }
    /// Drag this element out of the window into another app, as a file, image or text. Once
    /// the pointer travels [`crate::drag_out::DRAG_THRESHOLD`] from the press, `f` reads the
    /// state and returns what to carry (`None`: no drag this time, the press stays a press),
    /// and the system takes over the pointer until the drop. The drag shows a snapshot of the
    /// element unless the item sets [`crate::DragItem::image`].
    ///
    /// ```
    /// # use rune::{DragItem, el::el};
    /// # struct App { bundle: std::path::PathBuf }
    /// el().drag_out(|s: &App| Some(DragItem::new().file(&s.bundle)));
    /// ```
    ///
    /// A drag-out element never runs its `on_drag`; a press released before the threshold still
    /// runs `on_press`. macOS only for now: on other platforms the system drag doesn't start,
    /// and the element behaves as if it had no `drag_out`. Headless tools record `drag-out
    /// <path>` in the trace and `dragOut` in the tree instead. Overlay panels start the same
    /// session; see [`crate::drag_out`] for how far that path is tested.
    pub fn drag_out<S: 'static>(mut self, f: impl Fn(&S) -> Option<crate::DragItem> + 'static) -> Self {
        self.drag_out = Some(Rc::new(move |s| f(s.cast::<S>().expect("handler registered for a different state type than the app's"))));
        self
    }
    /// How far (points, straight-line) the pointer travels from the press before this
    /// element's [`El::drag_out`] starts, instead of [`crate::drag_out::DRAG_THRESHOLD`]. A
    /// port of a native control that used another distance keeps its feel this way.
    ///
    /// ```
    /// # use rune::{DragItem, el::el};
    /// # struct App;
    /// el().drag_out(|_: &App| Some(DragItem::new().text("row"))).drag_threshold(3.0);
    /// ```
    pub fn drag_threshold(mut self, points: f32) -> Self {
        self.drag_threshold = Some(points.max(0.0));
        self
    }
    /// Called when a [`El::drag_out`] drag starts, after the drag image was taken, so the app
    /// can style the source while it's dragged. Every start is followed by exactly one
    /// [`El::on_drag_end`], also when the system couldn't start the session.
    ///
    /// ```
    /// # use rune::{DragItem, el::el};
    /// # struct App { file: std::path::PathBuf, dragging: bool }
    /// # let s = App { file: "/tmp/a.txt".into(), dragging: false };
    /// el().opacity(if s.dragging { 0.28 } else { 1.0 })
    ///     .drag_out(|s: &App| Some(DragItem::new().file(&s.file)))
    ///     .on_drag_start(|s: &mut App, _| s.dragging = true)
    ///     .on_drag_end(|s: &mut App, _, _| s.dragging = false);
    /// ```
    pub fn on_drag_start<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.on_drag_start = Some(Rc::new(move |s, tx| f(downcast::<S>(s), tx)));
        self
    }
    /// Called once when a [`El::drag_out`] drag ends, with what the drop did.
    pub fn on_drag_end<S: 'static>(mut self, f: impl Fn(&mut S, crate::DragOutcome, &mut Tx) + 'static) -> Self {
        self.on_drag_end = Some(Rc::new(move |s, outcome, tx| f(downcast::<S>(s), outcome, tx)));
        self
    }
}

/// Pins a closure to the signature [`El::on_edit`] needs, so Rust ties the returned borrow to
/// the state: `.on_edit(edit_access(move |s: &mut App| &mut s.rows[i].name))`.
pub fn edit_access<S, F: for<'a> Fn(&'a mut S) -> &'a mut TextEdit>(f: F) -> F {
    f
}
