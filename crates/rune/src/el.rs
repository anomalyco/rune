//! The element tree that view functions return. Plain data plus type-erased handlers.

use std::any::Any;
use std::rc::Rc;

use rune_motion::{Anim, Spring, Transition};
use taffy::prelude::*;

use crate::color::Color;
use crate::edit::TextEdit;
use crate::growth::{GROWTH, StreamEdge, edge};
use crate::shape::Shape;
use crate::text::{Icon, Link, Span};
use crate::tok::TextStyle;

/// What a handler may say about the commit it causes.
#[derive(Clone, Debug, Default)]
pub struct Tx {
    pub(crate) shortcut_activation_token: Option<String>,
    pub(crate) instant: bool,
    pub(crate) motion: Option<Spring>,
    pub(crate) color_motion: Option<Spring>,
    pub(crate) settle: bool,
    pub(crate) relayout: bool,
    pub(crate) copy: Option<crate::ClipboardWrite>,
    pub(crate) paste: bool,
    pub(crate) clear_secret: bool,
    pub(crate) text_flights: Vec<(String, String)>,
    pub(crate) scroll_to: Vec<String>,
    /// Scroll viewports to send to their bottom: (key, animated).
    pub(crate) scroll_bottom: Vec<(String, bool)>,
    pub(crate) wakes: Vec<std::time::Duration>,
    pub(crate) shell: Option<Shell>,
    pub(crate) trace: Option<TraceCalls>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TraceCalls {
    pub events: Vec<String>,
    pub channels: Vec<(&'static str, &'static str, f32)>,
}

/// What a handler asks of the window and the app (`Tx::close_window`, `Tx::quit`); the later
/// variant wins when several handlers ask.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Shell {
    /// Close the window as if the user had: `App::keep_running` keeps the app and its state.
    Close,
    Quit,
}

impl Tx {
    /// Whether this runtime has an attached motion or tooling trace.
    pub fn tracing(&self) -> bool { self.trace.is_some() }

    /// An app marker on the next frame, e.g. `tx.trace_event(format_args!("lab:{step:?}"))`.
    /// Formatting is skipped when tracing is off. Avoid eagerly constructing a `String`.
    pub fn trace_event(&mut self, event: impl std::fmt::Display) {
        if let Some(trace) = &mut self.trace { trace.events.push(event.to_string()); }
    }

    /// A sample-and-hold scalar (`position`, `level`, or `phase`), e.g. `phase:recording`.
    /// No allocation when tracing is off. Use a compositor's `Runtime::trace_channel` for
    /// presented geometry that changes between commits.
    pub fn trace_channel(&mut self, name: &'static str, kind: &'static str, value: f32) {
        if let Some(trace) = &mut self.trace { trace.channels.push((name, kind, value)); }
    }

    /// Reveal a keyed row, including a currently unrealized virtual row.
    pub fn scroll_into_view(&mut self, key: impl Into<String>) {
        self.scroll_to.push(key.into());
    }
    /// Glide the scroll viewport keyed `key` (or the one containing it) to its bottom, as a
    /// "jump to latest" button does. A `stick_bottom` viewport follows the end again once it
    /// lands. Long distances cut to two viewports from the end first; reduced motion jumps.
    pub fn scroll_to_bottom(&mut self, key: impl Into<String>) {
        self.scroll_bottom.push((key.into(), true));
    }
    /// [`Tx::scroll_to_bottom`] without the glide.
    pub fn scroll_to_bottom_instant(&mut self, key: impl Into<String>) {
        self.scroll_bottom.push((key.into(), false));
    }
    /// Wake the app's poll hook after an unscaled duration. Equal deadlines coalesce.
    /// This schedules a frame, not a view commit; poll returns true if state changed.
    /// For debounce, keep the latest due time in app state and reschedule the remaining
    /// duration when an earlier wake arrives. No timer thread is created.
    pub fn wake_after(&mut self, duration: std::time::Duration) {
        self.wakes.push(duration);
    }

    /// Fly the visible text of an existing keyed field to a newly mounted keyed text node.
    /// The runtime captures the previous presentation and measures the live destination each
    /// frame, after row growth and scroll. Either side is a key unique in the view, or a key
    /// path such as `"msg-42/body"`: the `body` text whose nearest keyed ancestor is `msg-42`,
    /// so every row can reuse the same local keys.
    pub fn fly_text(&mut self, source: impl Into<String>, destination: impl Into<String>) {
        self.text_flights.push((source.into(), destination.into()));
    }

    /// Request a clipboard write from the shell after this handler finishes.
    pub fn copy(&mut self, text: impl Into<String>) {
        self.copy = Some(crate::ClipboardWrite::new(text.into(), None));
    }

    /// Exclude this text from clipboard histories and clear it after `clear_after` if unchanged.
    /// The handle reports whether the copy still holds the clipboard, and can clear it early.
    pub fn copy_secret(&mut self, text: impl Into<String>, clear_after: std::time::Duration) -> crate::SecretCopy {
        let (write, copy) = crate::ClipboardWrite::secret(text.into(), clear_after);
        self.copy = Some(write);
        copy
    }

    /// Clear the secret Rune copied last, from any window, if the clipboard still holds it
    /// (on lock, say). A secret copy asked for earlier in this handler is dropped.
    pub fn clear_secret_clipboard(&mut self) {
        if self.copy.as_ref().is_some_and(crate::ClipboardWrite::is_secret) { self.copy = None; }
        self.clear_secret = true;
    }

    /// Read the OS clipboard asynchronously and paste into the focused field or root `on_paste`.
    pub fn request_paste(&mut self) { self.paste = true; }

    /// Close the window after this handler, as the close button would (`App::on_close` runs).
    pub fn close_window(&mut self) {
        self.shell = self.shell.max(Some(Shell::Close));
    }

    /// Quit the app after this handler, whether or not it keeps running without a window.
    pub fn quit(&mut self) {
        self.shell = Some(Shell::Quit);
    }

    /// Apply this change with no animation (direct manipulation).
    pub fn instant(&mut self) {
        self.instant = true;
    }
    /// Animate this change's layout with `spring` instead of each element's own.
    pub fn motion(&mut self, spring: Spring) {
        self.motion = Some(spring);
    }
    /// Apply this change with no animation and land every running track: growth, reveals and
    /// follow-the-end (opening or returning to a session, guarantee 8).
    pub fn settle(&mut self) {
        self.instant = true;
        self.settle = true;
    }
    /// Animate this change's colours with `spring` (a theme change uses `tok::motion::CALM`).
    pub fn color_motion(&mut self, spring: Spring) {
        self.color_motion = Some(spring);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Down,
    Move,
    Up,
}

/// A pointer gesture on an element, in points relative to its layout rect.
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
}

pub(crate) type PressFn = Rc<dyn Fn(&mut dyn Any, &mut Tx)>;
pub(crate) type SecondaryFn = Rc<dyn Fn(&mut dyn Any, PointerPress, &mut Tx)>;
pub(crate) type InputKeyFn = Rc<dyn Fn(&mut dyn Any, &crate::KeyEvent, &mut Tx) -> bool>;

/// A press position in window points, suitable for a context-menu anchor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerPress {
    pub x: f32,
    pub y: f32,
}
pub(crate) type DragFn = Rc<dyn Fn(&mut dyn Any, &mut Drag, &mut Tx)>;
pub(crate) type EditFn = Rc<dyn for<'a> Fn(&'a mut dyn Any) -> &'a mut TextEdit>;

#[derive(Clone, Default)]
pub(crate) struct FocusSpec {
    pub physical: Option<crate::physical_input::Handler>,
    pub enabled: bool,
    pub order: i32,
    pub group: bool,
    pub selected: bool,
    pub ring: Option<Outline>,
    pub highlight: Option<Color>,
    pub follows: bool,
    pub return_to: Option<String>,
}

#[derive(Clone)]
pub(crate) struct ModalSpec {
    pub dismiss: PressFn,
    pub outside: bool,
}

/// A vertical scroll viewport. Its first child is the content.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollSpec {
    /// Follow the end: while scrolled to the bottom, growth keeps the bottom in view, and a new
    /// viewport opens at the bottom on its first frame.
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
    pub(crate) access: Option<EditFn>,
    pub(crate) submit: Option<PressFn>,
    pub(crate) key: Option<InputKeyFn>,
    pub(crate) change: Option<PressFn>,
}

/// A CSS-style box shadow. Outer shadows draw behind the element; `inset` ones draw inside its
/// shape, over the fill. A zero-blur, zero-offset outer shadow with a spread is a ring outside
/// the border box (drawn as a ring, so translucent fills don't show it through).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Shadow {
    pub x: f32,
    pub y: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: Color,
    pub inset: bool,
}

/// A ring drawn `offset` points outside the border box, `width` points wide (CSS `outline`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Outline {
    pub width: f32,
    pub offset: f32,
    pub color: Color,
}

/// The pointer's shape over an element (CSS `cursor`). Elements without one inherit their
/// nearest ancestor's; text fields default to [`Cursor::Text`], everything else to the arrow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Cursor {
    #[default]
    Default,
    Pointer,
    Text,
    Grab,
    Grabbing,
    NotAllowed,
    Move,
    ResizeColumn,
    ResizeRow,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
}

/// Which side of its anchor an [`El::anchored`] overlay sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Top,
    Bottom,
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Anchor {
    pub key: String,
    pub side: Side,
    pub align: Align,
    pub gap: f32,
}

#[derive(Clone, Debug)]
pub struct TextSpec {
    pub content: String,
    pub style: TextStyle,
    pub color: Color,
    pub align: Align,
    pub spans: Vec<Span>,
    /// Wrap at the element's width (its height follows the text).
    pub wrap: bool,
    /// Streaming word reveal: words that arrive after mount fade in over this many seconds
    /// (negative: no reveal tracking).
    pub reveal: f32,
    /// One line, cut with an ellipsis when wider than the element.
    pub truncate: bool,
    /// Fade the overflowing trailing edge rather than replacing text with an ellipsis.
    pub truncate_fade: bool,
    /// The `truncate_fade` edge's width in ems (see [`El::truncate_fade_width`]).
    pub(crate) fade_em: f32,
    /// Spread wrapped lines to fill the width (see [`El::justify`]).
    pub justify: bool,
    /// Plain live run. Layout is measured from the authored placeholder before latching it.
    pub live: Option<crate::LiveText>,
    pub(crate) links: Vec<Link>,
    pub(crate) on_link: Option<LinkFn>,
    /// Measured as its slice at this byte of the run (see [`El::advance_in`]).
    pub advance_in: Option<(crate::text::Run, usize)>,
}

/// [`El::on_link`]'s handler.
#[derive(Clone)]
pub(crate) struct LinkFn(pub LinkHandler);
pub(crate) type LinkHandler = Rc<dyn Fn(&mut dyn Any, &str, &mut Tx)>;

impl std::fmt::Debug for LinkFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LinkFn")
    }
}

pub use rune_motion::Cycle;

/// Default `truncate_fade` edge, in ems (Abattoir: 1.5 em on overflowing text).
const TRUNCATE_FADE_EM: f32 = 1.5;

#[derive(Clone, Debug)]
pub struct Visual {
    /// Rectangular native input region for overlays using `OverlayInput::Regions`.
    pub hit: bool,
    pub surface: Option<(crate::NativeSurface, crate::SurfaceFit)>,
    pub fill: Color,
    pub radius: [f32; 4],
    pub border_width: f32,
    pub border: Color,
    pub shadows: Vec<Shadow>,
    pub opacity: f32,
    pub blur: f32,
    pub blur_motion: Option<Anim>,
    /// Static alpha of this element's own paint; never inherited or isolated.
    pub paint_alpha: f32,
    pub blend: crate::render::Blend,
    pub hover: Color,
    pub press: Color,
    /// Soft clip of children to this rect: fade widths left, top, right, bottom.
    pub clip: Option<[f32; 4]>,
    /// Rounded clip of this element and its subtree, applied after subtree filters.
    pub clip_radius: Option<f32>,
    pub icon: Option<(&'static Icon, Color)>,
    /// Vertical linear gradient (top, bottom) painted over the fill, under hover/press overlays.
    pub gradient: Option<[Color; 2]>,
    pub outline: Option<Outline>,
    /// Vector strokes and fills in the element's own points, over the fill.
    pub shape: Option<Rc<Shape>>,
    /// An RGBA image scaled into the element's rect, over the fill and shape.
    pub pixels: Option<crate::pixels::Pixels>,
    /// How `pixels` keeps its aspect ratio; `None` stretches it to the rect.
    pub pixels_fit: Option<crate::SurfaceFit>,
    /// App-defined tag a compositor reads per instance (`Runtime::marks`); 0 inherits the parent's.
    pub mark: u32,
    /// Tracks hover for its descendants' `group_*` styles even without a hover colour. Any
    /// element with a hover colour is a group too.
    pub hover_group: bool,
    /// The hover track's enter and exit curves (default: snappy spring both ways).
    pub hover_motion: Option<(Anim, Anim)>,
    /// Opacity factor (rest, hovered) following the nearest ancestor group's hover.
    pub group_opacity: Option<[f32; 2]>,
    /// Text or icon colour while the nearest ancestor group is hovered.
    pub group_color: Option<Color>,
}

impl Default for Visual {
    fn default() -> Self {
        Visual {
            hit: false,
            surface: None,
            fill: Color::TRANSPARENT,
            radius: [0.0; 4],
            border_width: 0.0,
            border: Color::TRANSPARENT,
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
            icon: None,
            gradient: None,
            outline: None,
            shape: None,
            pixels: None,
            pixels_fit: None,
            mark: 0,
            hover_group: false,
            hover_motion: None,
            group_opacity: None,
            group_color: None,
        }
    }
}

pub(crate) type PasteFn = Rc<dyn Fn(&mut dyn Any, &str, &mut Tx)>;
/// A scroll viewport's content-aware edges: `El::scroll_edge_fade` and `El::scroll_margin`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ScrollEdges {
    /// Fade width (points) for the top and bottom edges; 0 for none.
    pub(crate) fade: f32,
    /// Space `Tx::scroll_into_view` leaves between a revealed row and the viewport's edge.
    pub(crate) margin: f32,
}

pub(crate) type AtBottomFn = Rc<dyn Fn(&mut dyn Any, bool)>;
pub(crate) type LayoutFn = Rc<dyn Fn(&mut dyn Any, [f32; 4])>;
pub(crate) type WindowFocusFn = Rc<dyn Fn(&mut dyn Any, bool, &mut Tx)>;

pub struct El {
    pub(crate) on_paste: Option<PasteFn>,
    pub(crate) on_window_focus: Option<WindowFocusFn>,
    pub(crate) focus: FocusSpec,
    pub(crate) modal: Option<ModalSpec>,
    pub(crate) looping: Option<rune_motion::LoopSpec>,
    pub(crate) key: Option<String>,
    pub(crate) trace_name: Option<&'static str>,
    pub(crate) trace_namespace: Option<(&'static str, &'static str)>,
    #[cfg(debug_assertions)]
    key_source: Option<&'static std::panic::Location<'static>>,
    #[cfg(debug_assertions)]
    pub(crate) scroll_source: Option<&'static std::panic::Location<'static>>,
    pub(crate) role: &'static str,
    pub(crate) label: Option<String>,
    pub(crate) style: Style,
    pub(crate) vis: Visual,
    pub(crate) live: Vec<crate::live::Bound>,
    pub(crate) text: Option<TextSpec>,
    pub(crate) children: Vec<El>,
    pub(crate) on_press: Option<PressFn>,
    pub(crate) on_secondary_press: Option<SecondaryFn>,
    pub(crate) on_hover: Option<PressFn>,
    pub(crate) on_pointer_move: Option<SecondaryFn>,
    pub(crate) above: Option<Anchor>,
    pub(crate) on_drag: Option<DragFn>,
    pub(crate) motion: Option<Spring>,
    pub(crate) opacity_motion: Option<Anim>,
    pub(crate) lead: Option<(Spring, Spring)>,
    pub(crate) width_leads: bool,
    pub(crate) transition: Option<Transition>,
    pub(crate) scroll: Option<ScrollSpec>,
    pub(crate) on_at_bottom: Option<AtBottomFn>,
    pub(crate) on_layout: Option<LayoutFn>,
    pub(crate) scroll_edges: ScrollEdges,
    pub(crate) growth: Option<GrowthSpec>,
    pub(crate) virtual_estimate: Option<f32>,
    pub(crate) virtual_mount: bool,
    pub(crate) streaming: bool,
    pub(crate) input: Option<Box<InputSpec>>,
    pub(crate) cursor: Option<Cursor>,
    pub(crate) window_drag: bool,
}

fn base(dir: FlexDirection) -> El {
    El {
        on_paste: None,
        on_window_focus: None,
        focus: FocusSpec::default(),
        modal: None,
        looping: None,
        key: None,
        trace_name: None,
        trace_namespace: None,
        #[cfg(debug_assertions)]
        key_source: None,
        #[cfg(debug_assertions)]
        scroll_source: None,
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
        on_press: None,
        on_secondary_press: None,
        on_hover: None,
        on_pointer_move: None,
        above: None,
        on_drag: None,
        motion: None,
        opacity_motion: None,
        lead: None,
        width_leads: false,
        transition: None,
        scroll: None,
        on_at_bottom: None,
        on_layout: None,
        scroll_edges: ScrollEdges::default(),
        growth: None,
        virtual_estimate: None,
        virtual_mount: false,
        streaming: false,
        input: None,
        cursor: None,
        window_drag: false,
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

/// Takes the free space along its parent's main axis.
pub fn spacer() -> El {
    base(FlexDirection::Row).grow(1.0)
}

pub fn text(content: impl Into<String>) -> El {
    let mut e = base(FlexDirection::Row);
    e.role = "text";
    e.text = Some(TextSpec {
        content: content.into(),
        style: crate::tok::type_::BODY,
        color: Color::WHITE,
        align: Align::Start,
        spans: Vec::new(),
        wrap: false,
        reveal: -1.0,
        truncate: false,
        truncate_fade: false,
        fade_em: TRUNCATE_FADE_EM,
        justify: false,
        live: None,
        links: Vec::new(),
        on_link: None,
        advance_in: None,
    });
    e
}

/// Text with styled byte ranges; wraps at its width.
pub fn rich(content: impl Into<String>, spans: Vec<Span>) -> El {
    let mut e = text(content);
    if let Some(t) = &mut e.text {
        t.spans = spans;
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
pub fn text_input(edit: &TextEdit, style: TextStyle) -> El {
    let (content, _) = edit.display();
    let mut e = text(content).font(style);
    e.role = "textbox";
    if let Some(t) = &mut e.text {
        t.wrap = true;
    }
    e.input = Some(Box::new(InputSpec {
        animate_layout: false,
        edit: edit.clone(),
        placeholder: String::new(),
        placeholder_color: Color::WHITE.alpha(0.4),
        caret: Color::WHITE,
        selection: Color::WHITE.alpha(0.2),
        single_line: false,
        access: None,
        submit: None,
        key: None,
        change: None,
    }));
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
        self.on_window_focus = Some(Rc::new(move |s, focused, tx| f(s.downcast_mut::<S>().unwrap(), focused, tx)));
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
    pub fn on_paste<S: 'static>(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self {
        self.on_paste = Some(Rc::new(move |s, text, tx| f(s.downcast_mut::<S>().unwrap(), text, tx)));
        self
    }
    /// Mask a text input before layout; disable undo, copy, cut and IME composition.
    /// Prefer `TextEdit::secure()` when the model can be configured at construction.
    pub fn secure(mut self) -> Self {
        if let Some(input) = &mut self.input {
            input.edit.set_secure(true);
            if let Some(text) = &mut self.text {
                use zeroize::Zeroize;
                text.content.zeroize();
                text.content = input.edit.display().0;
            }
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
    /// motion-lint pairs names by class too: use `("gpui", "pill")` to match Scrawl's Metal trace.
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
    /// Bind uniform scale around the node's centre; layout remains at its target size.
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
            input.change = Some(Rc::new(move |s, tx| f(s.downcast_mut::<S>().expect("input change state type"), tx)));
        }
        self
    }
    /// Handle a focused text field's key before the editor. Return true to consume it.
    pub fn on_input_key<S: 'static>(mut self, f: impl Fn(&mut S, &crate::KeyEvent, &mut Tx) -> bool + 'static) -> Self {
        if let Some(input) = &mut self.input {
            input.key = Some(Rc::new(move |s, key, tx| f(s.downcast_mut::<S>().expect("input key state type"), key, tx)));
        }
        self
    }
    /// Secondary release inside the captured element (right-click or macOS control-click).
    pub fn on_secondary_press<S: 'static>(mut self, f: impl Fn(&mut S, PointerPress, &mut Tx) + 'static) -> Self {
        self.on_secondary_press = Some(Rc::new(move |s, p, tx| {
            f(s.downcast_mut::<S>().expect("secondary press state type"), p, tx);
        }));
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
    pub fn focus_group(mut self) -> Self {
        self.focus.group = true;
        self
    }
    pub fn focus_selected(mut self, selected: bool) -> Self {
        self.focus.selected = selected;
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
    /// Return focus to this keyed trigger (or its first focusable child) when removed.
    pub fn focus_return(mut self, trigger: impl Into<String>) -> Self {
        self.focus.return_to = Some(trigger.into());
        self
    }
    /// Keyboard-only ring, on this element or a visual child of the focused control.
    pub fn focus_ring(mut self, width: f32, offset: f32, color: Color) -> Self {
        self.focus.ring = Some(Outline { width, offset, color });
        self
    }
    /// Trap focus and make the background inert while mounted. Escape requests dismissal.
    pub fn modal<S: 'static>(mut self, click_outside: bool, dismiss: impl Fn(&mut S) + 'static) -> Self {
        self.modal = Some(ModalSpec { outside: click_outside,
            dismiss: Rc::new(move |s, _| dismiss(s.downcast_mut::<S>().expect("modal state type"))) });
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
        self.looping = Some(spec);
        self
    }
    /// Window keyed rows of a growth stack directly inside a vertical scroll viewport.
    /// Unvisited rows use an estimated height; measurement preserves the visible anchor.
    /// Offscreen rows have no layout, text or scene nodes.
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
    pub fn virtualize(mut self, estimated_row_height: f32) -> Self {
        self.virtual_estimate = Some(estimated_row_height.max(1.0));
        self
    }

    #[cfg_attr(debug_assertions, track_caller)]
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        #[cfg(debug_assertions)]
        { self.key_source = Some(std::panic::Location::caller()); }
        self
    }
    pub fn role(mut self, role: &'static str) -> Self {
        self.role = role;
        self
    }
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
    pub fn child(mut self, child: El) -> Self {
        self.children.push(child);
        self
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
    /// Position an out-of-flow overlay above an earlier keyed element, clamped to the window.
    /// Attach the overlay at the root to escape the anchor's clipping ancestors.
    pub fn above(self, key: impl Into<String>, gutter: f32) -> Self {
        self.anchored(key, Side::Top, Align::Start, gutter)
    }
    /// Position an out-of-flow overlay on `side` of an earlier keyed element, aligned along that
    /// side, `gap` points away. It flips to the opposite side when it doesn't fit, and stays
    /// 8 points inside the window. Attach it at the root (after the anchor) so it draws above
    /// later siblings and escapes clipping ancestors.
    pub fn anchored(mut self, key: impl Into<String>, side: Side, align: Align, gap: f32) -> Self {
        self.style.position = Position::Absolute;
        self.above = Some(Anchor { key: key.into(), side, align, gap });
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
    /// Mark the drawn, ancestor-clipped rectangle as native overlay input. Does not add a handler.
    pub fn hit(mut self) -> Self {
        self.vis.hit = true;
        self
    }

    /// Observe movement in window-local points without making a native overlay input region.
    pub fn on_pointer_move<S: 'static>(mut self, f: impl Fn(&mut S, PointerPress) + 'static) -> Self {
        self.on_pointer_move = Some(Rc::new(move |s, p, _| f(downcast::<S>(s), p)));
        self
    }

    /// Called once when the pointer enters this hit target, not on animation frames.
    pub fn on_hover<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.on_hover = Some(Rc::new(move |s, _| f(downcast::<S>(s))));
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
    /// Per-corner radius: top-left, top-right, bottom-right, bottom-left.
    pub fn corners(mut self, r: [f32; 4]) -> Self {
        self.vis.radius = r;
        self
    }
    pub fn border(mut self, width: f32, c: Color) -> Self {
        self.vis.border_width = width;
        self.vis.border = c;
        self
    }
    pub fn shadow(mut self, y: f32, blur: f32, color: Color) -> Self {
        self.vis.shadows.push(Shadow { y, blur, color, ..Shadow::default() });
        self
    }
    /// A full box shadow (offset, spread, inset), e.g. one layer of a CSS elevation token.
    pub fn box_shadow(mut self, s: Shadow) -> Self {
        self.vis.shadows.push(s);
        self
    }
    /// A vertical gradient from `top` to `bottom` painted over the fill (CSS `linear-gradient(180deg, ..)`).
    pub fn gradient(mut self, top: Color, bottom: Color) -> Self {
        self.vis.gradient = Some([top, bottom]);
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
        self.vis.blur_motion = Some(anim.into());
        self
    }
    /// Dim each paint leaf in the currently built subtree, independently. Unlike `opacity`,
    /// overlapping draws retain their individual coverage and no group layer is introduced.
    /// Call after adding children. Useful for static disabled styling; animation and hit-test
    /// opacity still belong to `opacity` / `live_opacity`.
    pub fn dim(mut self, alpha: f32) -> Self {
        fn apply(e: &mut El, alpha: f32) {
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
    /// Clip children to this box with soft gradient edges (never a hard crop).
    pub fn clip_soft(mut self, fade: f32) -> Self {
        self.vis.clip = Some([fade; 4]);
        self
    }
    /// Clip this element and its descendants to its presented rounded rectangle. Radius is in
    /// points, capped at half the shorter edge. Filters run before this mask. Nested masks
    /// intersect without moving either rectangle's corners; complex intersections use a layer.
    /// Hit testing uses the rectangular clip bounds, including visually clipped corner areas.
    pub fn clip_rounded(mut self, radius: f32) -> Self {
        self.vis.clip_radius = Some(radius.max(0.0));
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
        self.vis.hover_motion = Some((enter.into(), exit.into()));
        self
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
        self
    }
    /// Wrap text at the element's width.
    pub fn wrap(mut self) -> Self {
        if let Some(t) = &mut self.text {
            t.wrap = true;
        }
        self
    }
    /// One line, cut with "…" when wider than the element; the element may shrink below its
    /// text's width.
    pub fn truncate(mut self) -> Self {
        if let Some(t) = &mut self.text {
            t.truncate = true;
            t.truncate_fade = false;
            t.wrap = false;
        }
        self.style.flex_shrink = 1.0;
        self.style.min_size.width = length(0.0);
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
            if let Some(t) = &mut self.text {
                t.wrap = false;
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
    /// Binds the field to a `TextEdit` in app state. `f` may capture (a row index, say); wrap
    /// a closure in [`edit_access`] so its returned borrow is tied to the state.
    pub fn on_edit<S: 'static>(mut self, f: impl for<'a> Fn(&'a mut S) -> &'a mut TextEdit + 'static) -> Self {
        if let Some(i) = &mut self.input {
            i.access = Some(Rc::new(move |s: &mut dyn Any| f(downcast::<S>(s))));
        }
        self
    }
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
    /// A vertical scroll viewport over its first child.
    ///
    /// The viewport needs a bounded height to scroll: a fixed `.h(..)`, or a `.grow(1.0)` share
    /// of a parent whose height is itself bounded. Every flex column between that bounded
    /// ancestor and the viewport needs `.min_h(0.0)`, or it grows to fit the content and the
    /// viewport never overflows. Debug builds warn once per call site when a viewport inside a
    /// `.grow()` container resolves to 0 pt tall while its content is not.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn scroll(mut self, spec: ScrollSpec) -> Self {
        self.scroll = Some(spec);
        #[cfg(debug_assertions)]
        { self.scroll_source = Some(std::panic::Location::caller()); }
        self.style.overflow = taffy::Point { x: taffy::Overflow::Hidden, y: taffy::Overflow::Scroll };
        self
    }
    /// On a scroll viewport, called with whether it shows its bottom (within a point): once
    /// after its first layout, then only when that changes, never per frame. Pair with
    /// [`Tx::scroll_to_bottom`] for a "jump to latest" button.
    pub fn on_at_bottom<S: 'static>(mut self, f: impl Fn(&mut S, bool) + 'static) -> Self {
        self.on_at_bottom = Some(Rc::new(move |s, at| f(downcast::<S>(s), at)));
        self
    }
    /// On a scroll viewport, fade its top and bottom edges over `width` points (16–32 reads well),
    /// each only while content is hidden under it: the fade's strength is the hidden extent / 15
    /// points, so a viewport at its top (or bottom, or stuck to it) shows that edge hard. Replaces
    /// any top/bottom [`El::clip_fade`] widths; left/right ones still apply. Pair with
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
    /// Called with this element's laid-out rect `[x, y, w, h]` in window points, after scrolling
    /// (as [`Cx::layout`](crate::Cx::layout) reads it): once after its first layout, then when it
    /// moves or resizes by more than half a point. Never per animation frame: the rect is where
    /// layout puts the element, not where its spring is on the way there.
    pub fn on_layout<S: 'static>(mut self, f: impl Fn(&mut S, [f32; 4]) + 'static) -> Self {
        self.on_layout = Some(Rc::new(move |s, r| f(downcast::<S>(s), r)));
        self
    }
    /// Make this column a growth stack (see [`GrowthSpec`]). Required alongside
    /// [`El::virtualize`] on the same element.
    pub fn growth(mut self, spec: GrowthSpec) -> Self {
        self.growth = Some(spec);
        self
    }
    /// A growth-stack row that is streaming: slows near its target and reveals through the
    /// feathered edge.
    pub fn streaming(mut self, on: bool) -> Self {
        self.streaming = on;
        self
    }
    pub fn color(mut self, c: Color) -> Self {
        if let Some(t) = &mut self.text {
            t.color = c;
        }
        if let Some((_, ic)) = &mut self.vis.icon {
            *ic = c;
        }
        self
    }
    pub fn align(mut self, a: Align) -> Self {
        if let Some(t) = &mut self.text {
            t.align = a;
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
    /// Override this element's opacity curve, including an optional entrance delay.
    /// The first paint and explicit instant/settle transactions still land immediately.
    pub fn opacity_motion(mut self, anim: impl Into<Anim>) -> Self {
        self.opacity_motion = Some(anim.into());
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
    /// How this element enters and exits when mounted or removed after the first frame
    /// (default: a fade). Rows inserted into a growth stack only grow open unless they set one,
    /// so `.transition(Transition::offset(0.0, 8.0).combined(Transition::opacity()))` on a row
    /// also slides and fades it in. Rows a virtual list remounts on scroll never replay it.
    pub fn transition(mut self, t: Transition) -> Self {
        self.transition = Some(t);
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
    pub fn on_drag<S: 'static>(mut self, f: impl Fn(&mut S, &mut Drag, &mut Tx) + 'static) -> Self {
        self.on_drag = Some(Rc::new(move |s, d, tx| f(downcast::<S>(s), d, tx)));
        self
    }
}

/// Pins a closure to the signature [`El::on_edit`] needs, so Rust ties the returned borrow to
/// the state: `.on_edit(edit_access(move |s: &mut App| &mut s.rows[i].name))`.
pub fn edit_access<S, F: for<'a> Fn(&'a mut S) -> &'a mut TextEdit>(f: F) -> F {
    f
}

fn downcast<S: 'static>(s: &mut dyn Any) -> &mut S {
    s.downcast_mut::<S>().expect("handler registered for a different state type than the app's")
}
