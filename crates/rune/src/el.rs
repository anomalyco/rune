//! The element tree that view functions return. Plain data plus type-erased handlers.

use std::rc::Rc;

use rune_motion::{Anim, Spring, Transition};
use rune_taffy::prelude::*;

use crate::color::Color;
pub(crate) use crate::el::drop::{DropHoverFn, DropSpec};
pub use crate::cursor::{Cursor, CursorImage};
pub(crate) use crate::el::sticky::{Sticky, StickyEdge};
use crate::edit::TextEdit;
use crate::growth::{GROWTH, StreamEdge, edge};
use crate::shape::Shape;
use crate::states::{downcast, edit_fn};
pub(crate) use crate::clipboard::tx::{ClipboardRead, ReadFn};
use crate::text::{Icon, Link, Span};
use crate::tok::TextStyle;

pub use crate::el::tx::{Shell, Tx};
pub(crate) use crate::el::tx::{FocusAsk, SelectRequest, TraceCalls};

mod a11y;
mod anchor;
mod backdrop;
mod child_exit;
mod direction;
pub(crate) mod drop;
pub(crate) mod effect;

/// An effect slot (`El::effect`): size it like any box.
pub fn effect(mark: u32) -> El {
    base(FlexDirection::Row).effect(mark)
}
mod element_info;
mod enter_delay;
mod focus;
pub(crate) use focus::ModalSpec;
mod growth;
mod handler_set;
mod handlers;
pub use handlers::{Drag, Phase, PointerEvent, PointerPress};
pub(crate) use handlers::{DragEndFn, DragFn, DragOutFn, InputKeyFn, PointerFn, PressFn, StepFn, StepLimitFn, WindowDrag};
mod help;
pub(crate) use help::{HELP_KEY, HELP_ROW, help_words};
mod input;
pub use input::{InputSpec, auto_grow, edit_access, text_input};
mod layout;
pub(crate) mod layout_watch;
mod live_colors;
pub mod motion;
mod paragraphs;
mod pills;
mod pixel_snap;
mod rare;
mod ride_trailing;
mod scroll;
mod selection_style;
mod sticky;
mod text;

pub fn text(content: impl Into<String>) -> El {
    let mut e = base(FlexDirection::Row);
    e.role = RoleTag::TEXT;
    e.text = Some(crate::cold::SharedBox::new(TextSpec {
        content: content.into(),
        style: crate::tok::type_::BODY,
        color: Color::WHITE,
        inked: false,
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
        selection_style: None,
        uncopied: Vec::new(),
        copy_breaks: [1, 1],
        copy_prefix: None,
        rings: Vec::new(),
        span_radius: crate::text::SPAN_RADIUS,
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
        exact_lines: true,
        flow: None,
        plaintext: false,
        shape: None,
    }));
    e
}
mod text_spec;
mod tx;
pub(crate) mod virtual_list;
pub(crate) mod virtual_rows;
mod visual;
mod visuals;

pub(crate) type EditFn = Rc<dyn for<'a> Fn(crate::states::States<'a>) -> &'a mut TextEdit>;

pub(crate) use crate::el::a11y::{A11y, Checked};
pub use crate::el::a11y::LiveRegion;

pub(crate) use crate::el::focus::FocusSpec;
pub(crate) use crate::el::handler_set::Handlers;
pub(crate) use crate::el::rare::{ElCold, ElFlags, RoleTag};
pub use crate::el::visual::{Visual, VisualCold};
pub(crate) use crate::el::visual::VisualFlags;

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
/// with per-second speed limits; positions derive from the presented heights above. Rows an
/// animated commit puts in a new order slide from where they were drawn into their new places.
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
pub(crate) const DEFAULT_SELECTION: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.2 };

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

/// CSS writes an eased fade as a list of stops: 16 holds the eased gradients Desktop draws.
pub(crate) const MAX_GRADIENT_STOPS: usize = 16;

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

pub use crate::el::anchor::{Anchor, Side, anchor_rect};
pub use crate::el::direction::Direction;
pub(crate) use crate::el::direction::{DirEffect, DirEvent, Flow, Pin, mirror_anchor, mirror_visual, step as step_direction};
pub(crate) use crate::el::scroll::FadeStyle;
pub use crate::el::scroll::ScrollFade;
pub use crate::el::selection_style::SelectionStyle;
pub use crate::el::text_spec::TextSpec;
pub(crate) use crate::el::text_spec::Elide;

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
pub use pills::InlineIcon;

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

/// A scroll viewport's content-aware edges: `El::scroll_edge_fade`, `El::scroll_margin` and
/// `El::scroll_anchor`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ScrollEdges {
    /// Fade width (points) for the top and bottom edges; 0 for none.
    pub(crate) fade: f32,
    /// How the fade follows the content hidden under each edge (`El::scroll_fade`).
    pub(crate) fade_style: scroll::FadeStyle,
    /// Space `Tx::scroll_into_view` leaves between a revealed row and the viewport's edge.
    pub(crate) margin: f32,
    /// The viewport keeps its top row where it is when content above it changes.
    pub(crate) anchor: bool,
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
pub(crate) type LayoutTxFn = Rc<dyn Fn(crate::states::States<'_>, [f32; 4], &mut Tx)>;
pub(crate) type ScrollOffsetFn = Rc<dyn Fn(crate::ScrollPos)>;
pub(crate) type WindowFocusFn = Rc<dyn Fn(crate::states::States<'_>, bool, &mut Tx)>;
pub(crate) type LanguagesFn = Rc<dyn Fn(crate::states::States<'_>, Vec<String>, &mut Tx)>;

/// An element: layout, paint, children and handlers. Hot fields sit inline in three cache lines;
/// the rest wait behind `cold`, allocated on the first write.
#[derive(Clone)]
pub struct El {
    pub(crate) style: CompactStyle,
    pub(crate) vis: Visual,
    pub(crate) children: Vec<El>,
    pub(crate) key: Option<String>,
    pub(crate) text: Option<crate::cold::SharedBox<TextSpec>>,
    pub(crate) handlers: crate::cold::Cold<Handlers>,
    /// The cached [`memo`](crate::memo) subtree this placeholder came from. Its children come
    /// before `children`; once the placeholder is expanded or given extra children,
    /// `ElFlags::MEMO_EXTENDED` says it is no longer that subtree itself.
    pub(crate) memo: Option<Rc<crate::memo::Built>>,
    pub(crate) cold: crate::cold::Cold<ElCold>,
    pub(crate) scale: f32,
    pub(crate) flags: ElFlags,
    pub(crate) role: RoleTag,
    pub(crate) vis_flags: VisualFlags,
    #[cfg(debug_assertions)]
    pub(crate) key_source: Option<&'static std::panic::Location<'static>>,
}

#[cfg(all(target_pointer_width = "64", not(debug_assertions)))]
const _: () = assert!(std::mem::size_of::<El>() == 192);
#[cfg(all(target_pointer_width = "64", not(debug_assertions)))]
const _: () = assert!(std::mem::size_of::<Option<El>>() == 192);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<Visual>() == 40);

#[inline]
fn base(dir: FlexDirection) -> El {
    El {
        // Relative makes every element the containing block for its absolute children, so they
        // size and pin against their parent's padding box. Taffy's default (static) hands them
        // to the window instead, as CSS does.
        style: CompactStyle::rune_base(dir),
        vis: Visual::default(),
        children: Vec::new(),
        key: None,
        text: None,
        handlers: crate::cold::Cold::NONE,
        memo: None,
        cold: crate::cold::Cold::NONE,
        scale: 1.0,
        flags: ElFlags::default(),
        role: RoleTag::GROUP,
        vis_flags: VisualFlags::default(),
        #[cfg(debug_assertions)]
        key_source: None,
    }
}

#[inline]
pub fn row() -> El {
    base(FlexDirection::Row)
}

#[inline]
pub fn col() -> El {
    base(FlexDirection::Column)
}

/// A plain box.
#[inline]
pub fn el() -> El {
    base(FlexDirection::Row)
}

/// Takes the free space along its parent's main axis.
#[inline]
pub fn spacer() -> El {
    base(FlexDirection::Row).grow(1.0)
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

/// An SVG icon from a catalog, `size` points square (or what [`crate::text::set_icon_swap_size`]
/// gives a swapped icon), tinted with `.color()`.
pub fn icon(i: &'static Icon, size: f32) -> El {
    let size = crate::text::icon::swapped_size(i, size);
    let mut e = base(FlexDirection::Row).size(size, size);
    e.role = RoleTag::ICON;
    e.vis.cold_mut().icon = Some((i, Color::WHITE));
    e
}

/// An element that paints `shape`; size it like any element.
pub fn canvas(shape: Shape) -> El {
    let mut e = base(FlexDirection::Row).shape(shape);
    e.role = RoleTag::IMAGE;
    e
}

impl El {
    /// Paint the latest text in this element's fixed layout box. `text("00:00")` reserves
    /// that placeholder's size; explicit `.size(w, h)` also works. Updates never resize the
    /// box or reflow siblings, and overflow is clipped. Plain single-line text only.
    pub fn live_text(mut self, value: &crate::LiveText) -> Self {
        assert!(self.cold.input.is_none(), "live_text cannot bind a text editor");
        let spec = self.text.as_mut().expect("live_text requires a text element");
        spec.live = Some(value.clone());
        self
    }

    /// A stable motion-lint name, independent of keys and generations. Defaults to the NodeId
    /// in MotionTrace; tooling retains its keyed path fallback. Stored without copying the name.
    /// Give simultaneous generations distinct names when comparing their motion separately.
    pub fn trace_name(mut self, name: &'static str) -> Self {
        self.cold_mut().trace_name = Some(name);
        self
    }

    /// Class and host for this subtree's trace metadata (descendants may override them).
    /// motion-lint pairs names by class too: use `("gpui", "pill")` to match a native Metal trace.
    /// The labels share the element's lazily allocated storage with its other rare fields.
    pub fn trace_namespace(mut self, class: &'static str, host: &'static str) -> Self {
        self.cold_mut().trace_namespace = Some((class, host));
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
    /// Bind the [start, end] span of a shape's strokes that draws, as fractions (0 to 1) of
    /// their combined length, as [`Shape::trimmed`](crate::Shape::trimmed) cuts them. A
    /// [`Tween`](crate::Tween) runs a countdown ring or a travelling dash without rebuilding.
    pub fn live_trim(mut self, value: impl Into<crate::live::Binding<[f32; 2]>>) -> Self {
        self.bind_live(crate::live::Bound::new(value, crate::live::Property::Trim)); self
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
        if let Some(old) = self.cold_mut().live.iter_mut().find(|b| b.property == bound.property) { *old = bound; }
        else { self.cold_mut().live.push(bound); }
    }

    pub(crate) fn key_location(&self) -> String {
        #[cfg(debug_assertions)]
        if let Some(source) = self.key_source { return source.to_string(); }
        "unknown (build with debug assertions for source locations)".into()
    }

    /// Paint the newest video frame inside this node. Node dimensions are in points.
    pub fn native_surface(mut self, surface: &crate::NativeSurface, fit: crate::SurfaceFit) -> Self {
        self.vis.cold_mut().surface = Some((surface.clone(), fit));
        self
    }
    /// Which edges a surface or [`pixels_fit`](Self::pixels_fit) image sticks to inside this
    /// node, as (horizontal, vertical): `Start` is left and top, `End` is right and bottom. It
    /// positions the spare space `Fit` leaves and the overflow `Fill` crops; the default is
    /// `Center` on both axes. Applies to [`native_surface`](Self::native_surface),
    /// [`video`](Self::video) and `pixels_fit`.
    ///
    /// ```
    /// # use rune::{Align, SurfaceFit, el};
    /// # let stream = rune::NativeSurface::new();
    /// // A streamed page that keeps its top-left corner while the panel is resized.
    /// let view = el().native_surface(&stream, SurfaceFit::Fill).surface_anchor(Align::Start, Align::Start);
    /// ```
    pub fn surface_anchor(mut self, horizontal: Align, vertical: Align) -> Self {
        self.vis.cold_mut().surface_place.anchor = [horizontal, vertical];
        self
    }
    /// Draw a surface or `pixels_fit` image at one texel per device pixel instead of scaling it
    /// to the node, so `SurfaceFit` no longer applies. A node bigger than the image leaves the
    /// rest empty; a smaller one crops it. Combine with
    /// [`surface_anchor`](Self::surface_anchor) to choose which edges stay put. It suits a
    /// stream rendered at the window's scale, whose last frame should hold still, not stretch,
    /// until the next one arrives at the node's new size.
    ///
    /// ```
    /// # use rune::{Align, SurfaceFit, el};
    /// # let stream = rune::NativeSurface::new();
    /// let view = el()
    ///     .native_surface(&stream, SurfaceFit::Fill)
    ///     .surface_anchor(Align::Start, Align::Start)
    ///     .surface_unscaled(true);
    /// ```
    pub fn surface_unscaled(mut self, unscaled: bool) -> Self {
        self.vis.cold_mut().surface_place.unscaled = unscaled;
        self
    }
    /// Play `video` inside this node: `Fit` letterboxes, `Fill` crops. Frames come from the
    /// display link while it plays; a paused video draws no frames of its own. Audio players
    /// need no element.
    pub fn video(self, video: &crate::Player, fit: crate::SurfaceFit) -> Self {
        self.native_surface(video.surface(), fit)
    }
    /// A finite opacity/scale sequence on the shared render clock. The final pose holds;
    /// reduced motion and explicit settle land it immediately. A new key replays it.
    pub fn sequence(self, frames: rune_motion::Sequence) -> Self {
        self.looping(rune_motion::LoopSpec::sequence(frames))
    }

    /// A renderer-owned periodic or finite presentation track, on the shared clock.
    pub fn looping(mut self, spec: rune_motion::LoopSpec) -> Self {
        self.cold_mut().looping = Some(Box::new(spec));
        self
    }

    /// Identity among siblings. Siblings need unique keys: when two share one, the first keeps
    /// it, each later one is renamed `key#dup1`, `key#dup2`, ... and Rune prints a warning naming
    /// both call sites once per key.
    #[cfg_attr(debug_assertions, track_caller)]
    #[inline]
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.set_key(key);
        self
    }
    /// [`El::key`] in place.
    #[cfg_attr(debug_assertions, track_caller)]
    #[inline]
    pub fn set_key(&mut self, key: impl Into<String>) -> &mut Self {
        self.key = Some(key.into());
        #[cfg(debug_assertions)]
        { self.key_source = Some(std::panic::Location::caller()); }
        self
    }
    /// Turn this element's icon by `radians`, clockwise about its centre: a disclosure chevron
    /// pointing right when closed and down when open. A change springs on the element's
    /// [`motion`](Self::motion) and jumps under reduced motion. Only the glyph turns; layout and
    /// hit testing keep the upright box.
    pub fn icon_rotation(mut self, radians: f32) -> Self {
        let r = if radians.is_finite() { radians } else { 0.0 };
        if r != 0.0 || !self.vis.cold.is_empty() {
            self.vis.cold_mut().icon_turn = r;
        }
        self
    }
    #[inline]
    pub fn child(mut self, child: El) -> Self {
        self.push_child(child);
        self
    }
    /// [`El::child`] in place.
    #[inline]
    pub fn push_child(&mut self, child: El) -> &mut Self {
        self.flags.set(ElFlags::MEMO_EXTENDED, self.memo.is_some());
        if child.flags.has(ElFlags::HAS_VIRTUAL) {
            self.flags.set(ElFlags::HAS_VIRTUAL, true);
        }
        self.children.push(child);
        self
    }
    #[inline]
    pub fn children(mut self, children: impl IntoIterator<Item = El>) -> Self {
        self.extend_children(children);
        self
    }
    /// [`El::children`] in place.
    #[inline]
    pub fn extend_children(&mut self, children: impl IntoIterator<Item = El>) -> &mut Self {
        self.flags.set(ElFlags::MEMO_EXTENDED, self.memo.is_some());
        let start = self.children.len();
        self.children.extend(children);
        if self.children[start..].iter().any(|c| c.flags.has(ElFlags::HAS_VIRTUAL)) {
            self.flags.set(ElFlags::HAS_VIRTUAL, true);
        }
        self
    }

    // Text
    #[inline]
    pub fn color(mut self, c: Color) -> Self {
        self.set_color(c);
        self
    }
    /// [`El::color`] in place.
    #[inline]
    pub fn set_color(&mut self, c: Color) -> &mut Self {
        if let Some(t) = &mut self.text {
            t.color = c;
            t.inked = true;
        }
        if let Some(cold) = self.vis.cold.get_mut_if_some() && let Some((_, ic)) = &mut cold.icon {
            *ic = c;
            cold.icon_inked = true;
        }
        if self.cold.input.is_some() {
            self.sync_input_rich();
        }
        self
    }
    #[inline]
    pub fn align(mut self, a: Align) -> Self {
        self.set_align(a);
        self
    }
    /// [`El::align`] in place.
    #[inline]
    pub fn set_align(&mut self, a: Align) -> &mut Self {
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
}
