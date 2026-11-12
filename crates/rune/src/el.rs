//! The element tree that view functions return. Plain data plus type-erased handlers.

use std::rc::Rc;

use rune_motion::{Anim, Spring, Transition};
use rune_taffy::prelude::*;

use crate::color::Color;
use crate::edit::TextEdit;
use crate::growth::{GROWTH, StreamEdge, edge};
use crate::shape::Shape;
use crate::states::{downcast, edit_fn};
use crate::text::{Icon, Link, Span};
use crate::tok::TextStyle;

mod a11y;
mod anchor;
mod backdrop;
mod child_exit;
mod direction;
pub(crate) mod drop;
pub(crate) mod effect;
mod element_info;
mod enter_delay;
mod focus;
mod growth;
mod handler_set;
mod handlers;
mod help;
mod input;
mod isolates;
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
mod text_spec;
mod tx;
pub(crate) mod virtual_list;
pub(crate) mod virtual_rows;
mod visual;
mod visuals;

pub use a11y::LiveRegion;
pub(crate) use a11y::{A11y, Checked};
pub use anchor::{Anchor, Side, anchor_rect};
pub(crate) use crate::clipboard::tx::{ClipboardRead, ReadFn};
pub use crate::cursor::{Cursor, CursorImage};
pub use direction::Direction;
pub(crate) use direction::{DirEffect, DirEvent, Flow, Pin, mirror_anchor, mirror_visual, step as step_direction};
pub(crate) use drop::{DropHoverFn, DropSpec};
pub(crate) use focus::FocusSpec;
pub(crate) use focus::ModalSpec;
pub(crate) use handler_set::Handlers;
pub use handlers::{Drag, Phase, PointerEvent, PointerPress};
pub(crate) use handlers::{DragEndFn, DragFn, DragOutFn, InputKeyFn, PointerFn, PressFn, StepFn, StepLimitFn, WindowDrag};
pub(crate) use help::{HELP_KEY, HELP_ROW, help_words};
pub use input::{InputSpec, auto_grow, edit_access, text_input};
pub use pills::InlineIcon;
pub(crate) use rare::{ElCold, ElFlags, RoleTag};
pub use rune_motion::Cycle;
pub(crate) use scroll::FadeStyle;
pub use scroll::ScrollFade;
pub use scroll::ScrollSpec;
pub(crate) use scroll::{AtBottomFn, ScrollAxes, ScrollEdges, VisibleRowsFn};
pub use selection_style::SelectionStyle;
pub(crate) use sticky::{Sticky, StickyEdge};
pub(crate) use text_spec::{Elide, TextDir, TextDirEvent, step_text_dir};
pub use text_spec::TextSpec;
pub(crate) use tx::{Commit, FocusAsk, MotionSource, SelectRequest, TraceCalls};
pub use tx::{Shell, Tx};
pub(crate) use visual::VisualFlags;
pub use visual::{Visual, VisualCold};
pub(crate) use visuals::{GradientGeometry, RadialGradient};
pub use visuals::{Outline, RadialRadius, Shadow};

/// An effect slot (`El::effect`): size it like any box.
pub fn effect(mark: u32) -> El {
    base(FlexDirection::Row).effect(mark)
}

pub fn text(content: impl Into<String>) -> El {
    let mut e = base(FlexDirection::Row);
    e.role = RoleTag::TEXT;
    e.text = Some(crate::cold::SharedBox::new(TextSpec::new(content.into())));
    e
}

pub(crate) type EditFn = Rc<dyn for<'a> Fn(crate::states::States<'a>) -> &'a mut TextEdit>;

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

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
}

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

/// Default `truncate_fade` edge, in ems.
pub(crate) const TRUNCATE_FADE_EM: f32 = 1.5;

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

/// A horizontal rule as a CSS `border-top: <width>px` on an empty box draws and lays out: its
/// width snapped to device pixels as a border's is (one whole pixel under a pixel), and that
/// many pixels of room in its column. So `rule(0.5, c)` between two rows takes 1 point at 1x,
/// 0.8 at 1.25x and 0.667 at 1.5x, where `el().h(0.5)` takes 0.5 at every scale, and a list of
/// rows split by Desktop's `border-bottom: 0.5px` ends where Chromium's does. It stretches
/// across its column; give it `w_full` or margins as a `<div>` takes them.
pub fn rule(width: f32, c: Color) -> El {
    base(FlexDirection::Row).border_sides([width, 0.0, 0.0, 0.0], c).border_layout()
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
}
