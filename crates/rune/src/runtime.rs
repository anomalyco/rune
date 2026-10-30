//! The retained tree: view → keyed nodes → Taffy layout at the target state → `rune-motion` Scene.
//! Animation samples tracks at frame time; it never re-runs view functions.
//!
//! Beside Scene's tracks the runtime owns four render-side behaviours that never touch app code:
//! scroll viewports (offset, momentum, follow-the-end), growth stacks (row heights on a capped
//! spring, positions derived), streaming word reveals, and text-field carets. Each frame:
//! queued input → (commit if state changed) → Scene → growth → scroll → reveals → build.

use std::any::Any;
// The runtime's maps hash node ids and paths every commit; SipHash's DoS resistance buys nothing here.
pub(crate) type HashMap<K, V> = std::collections::HashMap<K, V, rustc_hash::FxBuildHasher>;
pub(crate) type HashSet<K> = std::collections::HashSet<K, rustc_hash::FxBuildHasher>;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::Arc;

use parley::Layout;
use parley::editing::Cursor;
use parley::layout::Affinity;
use rune_motion::{
    Anim, Corners, LayoutAnim, Mount, NodeId, NodeSpec, Phase as PresencePhase, Prop, PropClass, Rect as MRect, Scene,
    Spring, Transition, prop, snap::grid,
};
use taffy::prelude::*;
use taffy::TraversePartialTree;

use crate::color::{Color, over, srgb_encode};
use crate::edit::{EditOutcome, Ime, KeyEvent};
use crate::el::{Align, Drag, DragEndFn, DragFn, DragOutFn, El, GrowthSpec, InputSpec, Phase, PointerEvent, PointerFn, PressFn, ScrollSpec, Shell, TextSpec, Tx, Visual};
use crate::growth::{GrowTrack, SUBSTEP};
// Half-open, like overlays, NSPointInRect and PtInRect: a shared edge belongs to one side.
use crate::overlay_options::contains;
use crate::flight::{TextFlight, transformed};
use crate::render::{Instance, KIND_DASHED, KIND_EFFECT, KIND_GLYPH, KIND_GRADIENT, KIND_IMAGE, KIND_INSET, KIND_RADIAL, KIND_RECT, KIND_SEGMENT, KIND_SHADOW, KIND_TRIANGLE, NO_CLIP};
use crate::shape::{LineCap, Op, Shape};
use crate::text::{Deco, PaintedDeco, GlyphQuad, Ink, TextSystem, layout_height};
use crate::tok::motion;
use event_log::log_event;

/// The states a handler call can reach: the app's (`$self.state`), or while the playground is
/// attached, its stage's beside its own controls.
macro_rules! states {
    ($self:ident) => {
        crate::runtime::playground_host::handler_states(&mut $self.state, &mut $self.playground)
    };
}

#[inline(always)]
pub(crate) fn handler_state<S: 'static>(state: &mut S) -> &mut dyn Any {
    #[cfg(feature = "hot")]
    if std::any::TypeId::of::<S>() == std::any::TypeId::of::<crate::hot::HostState>() {
        return (state as &mut dyn Any).downcast_mut::<crate::hot::HostState>().unwrap().value.as_mut();
    }
    state
}

/// Calls an app's `poll` or `on_key`. With `rune/hot` it goes through subsecond's jump table,
/// so a patched body runs instead of the pointer captured at startup, and a panicking patch
/// falls back to that pointer.
macro_rules! app_call {
    ($f:expr, $($arg:expr),* $(,)?) => {{
        #[cfg(feature = "hot")]
        let called = crate::hot::patched_or_original!("callback", $f, $($arg),*);
        #[cfg(not(feature = "hot"))]
        let called = ($f)($($arg),*);
        called
    }};
}

#[inline(always)]
pub(crate) fn handler_state_ref<S: 'static>(state: &S) -> &dyn Any {
    #[cfg(feature = "hot")]
    if std::any::TypeId::of::<S>() == std::any::TypeId::of::<crate::hot::HostState>() {
        return (state as &dyn Any).downcast_ref::<crate::hot::HostState>().unwrap().value.as_ref();
    }
    state
}

#[cfg(debug_assertions)]
#[path = "runtime/layout_diagnostics.rs"]
mod layout_diagnostics;
#[path = "runtime/blend.rs"]
mod blend;
#[path = "runtime/drag_out_host.rs"]
mod drag_out_host;
#[path = "runtime/system_settings.rs"]
mod system_settings;
#[path = "runtime/prune.rs"]
mod prune;
pub(crate) use blend::{glyph, glyph_uv_page, premul};

#[path = "focus.rs"]
mod focus;
#[path = "physical_dispatch.rs"]
mod physical_dispatch;
#[cfg_attr(not(target_os = "macos"), allow(unused_imports))]
pub(crate) use physical_dispatch::Unhandled;
#[path = "modifier_keys.rs"]
mod modifier_keys;
#[path = "popup_dispatch.rs"]
mod popup_dispatch;
#[path = "file_dialog_dispatch.rs"]
mod file_dialog_dispatch;
#[path = "drop_dispatch.rs"]
mod drop_dispatch;
#[path = "runtime/playground.rs"]
mod playground_host;
#[path = "runtime/scroll_bottom.rs"]
mod scroll_bottom;
#[path = "runtime/caret.rs"]
mod caret;
#[path = "runtime/event_log.rs"]
pub(crate) mod event_log;
#[path = "runtime/layout_read.rs"]
pub(crate) mod layout_read;
#[path = "links.rs"]
mod links;
#[path = "selectable.rs"]
pub(crate) mod selectable;
mod input_click;
#[path = "help.rs"]
mod help;
#[path = "runtime/scroll_edge.rs"]
mod scroll_edge;
#[path = "runtime/scroll_x.rs"]
mod scroll_x;
#[path = "runtime/scroll_offset.rs"]
mod scroll_offset;
#[path = "runtime/reveal.rs"]
mod reveal;
#[path = "runtime/sticky.rs"]
mod sticky;
#[path = "runtime/anchor.rs"]
mod anchor;
use anchor::{anchor_index, anchor_position, step_up};
#[path = "runtime/motion_elements.rs"]
mod motion_elements;
pub use motion_elements::MotionElement;
#[path = "runtime/live_latch.rs"]
mod live_latch;
#[path = "runtime/loop_clock.rs"]
mod loop_clock;
#[path = "runtime/scroll_watch.rs"]
mod scroll_watch;
#[path = "gesture_dispatch.rs"]
mod gesture_dispatch;
#[path = "runtime/flights.rs"]
mod flights;
#[path = "runtime/text_flights.rs"]
mod text_flights;
#[path = "runtime/hover.rs"]
mod hover;
#[path = "runtime/timer_wake.rs"]
mod timer_wake;
#[path = "runtime/idle_meter.rs"]
mod idle_meter;
#[cfg(feature = "hot")]
#[path = "runtime/hot_state.rs"]
mod hot_state;
#[path = "runtime/props.rs"]
mod props;
#[path = "runtime/secondary.rs"]
mod secondary;
#[path = "runtime/app_ime.rs"]
mod app_ime;
#[path = "runtime/press_clicks.rs"]
mod press_clicks;
#[path = "runtime/window_requests.rs"]
mod window_requests;
#[path = "runtime/paint_tracks.rs"]
mod paint_tracks;
#[path = "runtime/action.rs"]
mod action;
use action::*;
#[path = "runtime/bands.rs"]
mod bands;
#[path = "runtime/paint_slots.rs"]
mod paint_slots;
mod pixel_snap;
mod soft_edges;
use soft_edges::{in_fade, inherit_fade};
#[path = "runtime/shadow_paint.rs"]
mod shadow_paint;
#[path = "runtime/shape_paint.rs"]
mod shape_paint;
use shape_paint::push_shape;
use props::*;
pub(crate) use scroll_x::{SHIFT_TURNS_WHEEL, wheel_delta};
pub(crate) use scroll_offset::ScrollRequest;
pub(crate) use reveal::ScrollReveal;
pub use selectable::{SelectedText, SelectionChange, TextSelection};
#[cfg(test)]
#[path = "runtime/timer_wake_tests.rs"]
mod timer_wake_tests;
#[cfg(test)]
#[path = "runtime/truncate_tests.rs"]
mod truncate_tests;
#[cfg(test)]
#[path = "runtime/line_clamp_tests.rs"]
mod line_clamp_tests;
#[cfg(test)]
#[path = "runtime/aspect_fit_tests.rs"]
mod aspect_fit_tests;
#[cfg(test)]
#[path = "runtime/anchor_fit_tests.rs"]
mod anchor_fit_tests;
#[cfg(test)]
#[path = "drop_tests.rs"]
mod drop_tests;
#[cfg(test)]
#[path = "runtime/on_key_tests.rs"]
mod on_key_tests;
#[cfg(test)]
#[path = "live_only_tests.rs"]
mod live_only_tests;
#[path = "runtime/select_all.rs"] mod select_all;
#[cfg(test)]
#[path = "runtime/select_all_tests.rs"]
mod select_all_tests;
#[cfg(test)]
#[path = "runtime/selection_api_tests.rs"]
mod selection_api_tests;
#[cfg(test)]
#[path = "runtime/anchored_grow_tests.rs"]
mod anchored_grow_tests;
#[cfg(test)]
mod pixel_snap_tests;
#[cfg(test)]
mod size_tests;

/// Seconds after a commit before debug builds check a `Tx::live_only` event (`skip_view`).
#[cfg(debug_assertions)]
const LIVE_ONLY_CHECK: f64 = 0.25;

/// How far (points, per axis) a click may travel and still pair into a double click.
const CLICK_SLOP: f32 = 4.0;

/// Scroll momentum decay (iOS normal: 0.998 per ms).
const SCROLL_DECAY: f32 = 2.002;

#[cfg(any(test, target_arch = "wasm32"))]
#[path = "runtime/web_runtime.rs"]
pub(crate) mod web_runtime;
#[path = "runtime/web_a11y.rs"]
pub(crate) mod web_a11y;

#[path = "runtime/text_layout.rs"]
mod text_layout;
use text_layout::*;
#[path = "runtime/flatten.rs"]
mod flatten;
use flatten::*;
#[path = "runtime/memo_reuse.rs"]
pub(crate) mod memo_reuse;
#[path = "runtime/measure.rs"]
mod measure;
#[path = "runtime/apply.rs"]
mod apply;
#[path = "runtime/relayout.rs"]
mod relayout;
#[path = "runtime/virtual_window.rs"]
mod virtual_window;
#[path = "runtime/unseen_presence.rs"]
mod unseen_presence;
#[path = "runtime/row_reorder.rs"]
mod row_reorder;
#[path = "runtime/growth_rows.rs"]
mod growth_rows;
mod elements;
pub use elements::{ElementInfo, Pick, Picked, Pose};
mod cache_layer;
use cache_layer::{ActiveCache, NodeCache};
mod paint_helpers;
use paint_helpers::*;
mod hit;
mod build;

#[derive(Clone, Copy, Debug)]
pub struct Env {
    pub system_dark: bool,
    pub scale: f32,
}
impl Env {
    pub const fn new(system_dark: bool, scale: f32) -> Self {
        Self { system_dark, scale }
    }
}
impl Default for Env {
    fn default() -> Self { Self::new(true, 1.0) }
}

/// Wakes the shell from another thread (new data arrived); a no-op headless.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

thread_local! {
    static COMMITS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// How many commits (runs of a view that go on to the screen) have started on this thread,
/// across all its windows; 0 before the first. A cache of what views draw can keep whatever
/// the last commit or two used and evict the rest.
pub fn commit_generation() -> u64 {
    COMMITS.get()
}

/// Passed to view functions: the environment, plus runtime settings a view may declare.
pub struct Cx<S> {
    env: Env,
    titlebar_inset: f32,
    system_accent: crate::SystemAccent,
    system_reduced_motion: bool,
    reduced_motion: bool,
    time_scale: f32,
    caret: Option<(String, [f32; 4])>,
    caret_read: std::cell::Cell<bool>,
    layout: layout_read::LayoutRects,
    layout_reads: std::cell::RefCell<layout_read::LayoutReads>,
    _s: PhantomData<fn(&S)>,
}

impl<S> Cx<S> {
    pub(crate) fn retype<T>(&self) -> Cx<T> {
        Cx {
            env: self.env, titlebar_inset: self.titlebar_inset, system_accent: self.system_accent,
            system_reduced_motion: self.system_reduced_motion, reduced_motion: self.reduced_motion, time_scale: self.time_scale,
            caret: self.caret.clone(), caret_read: self.caret_read.clone(),
            layout: self.layout.clone(), layout_reads: self.layout_reads.clone(), _s: PhantomData,
        }
    }

    /// Takes back a [`Self::retype`]d context a nested view ran with: its overrides, and the
    /// caret and layout reads it added.
    pub(crate) fn absorb<T>(&mut self, other: Cx<T>) {
        self.reduced_motion = other.reduced_motion;
        self.time_scale = other.time_scale;
        self.caret_read.set(self.caret_read.get() || other.caret_read.get());
        let mut reads = self.layout_reads.borrow_mut();
        for read in other.layout_reads.into_inner() {
            layout_read::note_read(&mut reads, &read.key, read.same);
        }
    }

    pub(crate) fn set_env(&mut self, env: Env) {
        self.env = env;
    }

    pub(crate) fn set_titlebar_inset(&mut self, inset: f32) {
        self.titlebar_inset = inset;
    }

    #[cfg(feature = "hot")]
    pub(crate) fn settings_from<T>(&mut self, other: Cx<T>) {
        self.reduced_motion = other.reduced_motion;
        self.time_scale = other.time_scale;
        self.caret_read = other.caret_read;
        self.layout_reads = other.layout_reads;
    }

    pub fn env(&self) -> Env {
        self.env
    }
    pub fn system_dark(&self) -> bool {
        self.env.system_dark
    }
    /// Points from the window's leading edge to the trailing edge of the macOS traffic lights
    /// drawn over the content, for a top bar's leading inset: 0 when nothing is drawn there (full
    /// screen, a titled window whose content starts below its titlebar, other platforms). The
    /// view re-runs when it changes, as entering and leaving full screen do. Headless runs report
    /// [`Chrome::titlebar_inset`](crate::Chrome::titlebar_inset). A `memo` closure that reads
    /// it needs it in its version.
    pub fn titlebar_inset(&self) -> f32 {
        self.titlebar_inset
    }
    /// The user's accent colour (System Settings › Appearance on macOS) for this window's
    /// appearance, [`SystemAccent::DEFAULT`](crate::SystemAccent::DEFAULT)'s blue elsewhere and
    /// headless. The view re-runs, memos included, when the user picks another accent.
    pub fn accent(&self) -> Color {
        self.system_accent.accent(self.env.system_dark)
    }
    /// The keyboard focus ring colour for this window's appearance: AppKit's
    /// `keyboardFocusIndicatorColor`, the accent at 50% elsewhere. Elements with no ring of
    /// their own draw their default focus ring in it.
    pub fn focus_color(&self) -> Color {
        self.system_accent.focus(self.env.system_dark)
    }
    /// Browser safe-area insets in points, in top/right/bottom/left order. Zero on native.
    /// Apply these inside a full-canvas root so backgrounds can extend to the screen edges.
    pub fn safe_area(&self) -> [f32; 4] {
        #[cfg(target_arch = "wasm32")]
        { crate::web::safe_area() }
        #[cfg(not(target_arch = "wasm32"))]
        { [0.0; 4] }
    }
    /// The host's OS accessibility preference, independent of this commit's override.
    pub fn system_reduced_motion(&self) -> bool {
        self.system_reduced_motion
    }
    /// Whether this commit reduces motion: the host preference, or the view's override from
    /// [`Self::reduced_motion`]. Pass it to builders that pick a different motion under it.
    pub fn is_reduced_motion(&self) -> bool {
        self.reduced_motion
    }
    /// Override the host preference for this commit: travel jumps, opacity/colour briefly fade.
    pub fn reduced_motion(&mut self, on: bool) {
        self.reduced_motion = on;
    }
    /// Scale the window's animation clock (slow motion below 1, paused at 0). To slow or freeze
    /// only part of the window, use [`El::time_scale`](crate::El::time_scale), which multiplies
    /// with this.
    pub fn time_scale(&mut self, scale: f32) {
        self.time_scale = scale.clamp(0.0, 8.0);
    }
    /// The focused text field's caret line in window points `[x, y, w, h]`, as last drawn (after
    /// scrolling, at the cursor inside an IME composition), when that field is keyed `key` or sits
    /// inside an element keyed `key`. A view that reads it re-runs when the caret moves, so a
    /// popup placed from it lands the frame after each edit.
    pub fn caret(&self, key: &str) -> Option<[f32; 4]> {
        self.caret_read.set(true);
        let rect = caret_of(&self.caret, key);
        crate::memo::read_caret(key, rect);
        rect
    }
}

/// [`Cx::caret`] for the caret `caret` (the focused field's path and rect).
pub(crate) fn caret_of(caret: &Option<(String, [f32; 4])>, key: &str) -> Option<[f32; 4]> {
    let (path, rect) = caret.as_ref()?;
    path.split('/').any(|seg| seg == key).then_some(*rect)
}

/// A node's place under a growth stack's row. The row's height carries vertical motion there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RowLayout {
    /// Not inside a growth row.
    Free,
    /// Inside one: layout snaps.
    Snaps,
    /// Inside one under `El::animate_layout`: x and width spring, y and height snap.
    Horizontal,
}

#[derive(Clone)]
pub(crate) struct Node {
    on_paste: Option<crate::el::PasteFn>,
    on_window_focus: Option<crate::el::WindowFocusFn>,
    focus: crate::cold::Cold<crate::el::FocusSpec>,
    modal: Option<crate::el::ModalSpec>,
    looping: Option<Box<rune_motion::LoopSpec>>,
    pub id: NodeId,
    pub path: String,
    pub key: Option<String>,
    pub trace_name: Option<&'static str>,
    pub trace_namespace: Option<(&'static str, &'static str)>,
    pub role: &'static str,
    pub label: Option<String>,
    pub vis: Visual,
    live: Vec<crate::live::Bound>,
    pub text: Option<crate::cold::SharedBox<TextSpec>>,
    pub text_layout: Option<Rc<Layout<Ink>>>,
    /// Where `text_layout` cut the text for `El::truncate` and its kin.
    pub(crate) elided: Option<Elision>,
    /// The run a cell measures and draws from (see `El::advance_in`).
    run_layout: Option<Rc<crate::text::RunLayout>>,
    live_text_seen: Option<u128>,
    on_press: Option<PressFn>,
    on_double_press: Option<PressFn>,
    on_secondary_press: Option<PointerFn>,
    on_hover: Option<PressFn>,
    on_hover_change: Option<crate::el::WindowFocusFn>,
    hover_delay: [f32; 2],
    on_pointer_move: Option<PointerFn>,
    gestures: Option<Box<crate::canvas_input::CanvasInput>>,
    above: Option<crate::el::Anchor>,
    on_drag: Option<DragFn>,
    drag_out: Option<DragOutFn>,
    drag_threshold: Option<f32>,
    on_drag_start: Option<PressFn>,
    on_drag_end: Option<DragEndFn>,
    drop: Option<Box<crate::el::DropSpec>>,
    motion: Spring,
    opacity_motion: Option<Box<rune_motion::Anim>>,
    lead: Option<(Spring, Spring)>,
    width_leads: bool,
    /// Inside an `El::instant_changes` element, or one itself.
    instant_changes: bool,
    transition: Option<Box<Transition>>,
    /// `El::matched` id, with the element's own (not inherited) layout spring.
    matched: Option<(Rc<str>, Option<Spring>)>,
    scroll: Option<ScrollSpec>,
    scroll_axes: crate::el::ScrollAxes,
    on_at_bottom: Option<crate::el::AtBottomFn>,
    on_far_from_bottom: Option<(f32, crate::el::AtBottomFn)>,
    on_visible_rows: Option<crate::el::VisibleRowsFn>,
    on_layout: Option<crate::el::LayoutFn>,
    on_screen_layout: Option<crate::el::LayoutFn>,
    on_drawn_layout: Option<crate::el::DrawnLayoutFn>,
    on_scroll_offset: Option<crate::el::ScrollOffsetFn>,
    /// `El::sticky_top`, `El::sticky_bottom` and `El::stuck_shadow`.
    sticky: Option<Box<crate::el::Sticky>>,
    scroll_edges: crate::el::ScrollEdges,
    growth: Option<Box<GrowthSpec>>,
    streaming: bool,
    whole: bool,
    glide: Option<rune_motion::Spring>,
    input: Option<Box<InputSpec>>,
    cursor: Option<crate::el::Cursor>,
    window_drag: bool,
    ime_cursor: bool,
    /// `El::selection_group`.
    selection_group: bool,
    /// `El::selection_copy`.
    selection_copy: Option<crate::el::SelectionCopy>,
    /// `El::on_selection_change`.
    on_selection_change: Option<crate::el::SelectionChangeFn>,
    a11y: crate::el::A11y,
    /// Inside an `El::inert` subtree (this element or an ancestor).
    pub(crate) inert: bool,
    press_scale: Option<f32>,
    /// `El::scale`: a factor on the element's own scale, springing on `P_SCALE`.
    static_scale: f32,
    /// `P_SCALE` has a track: paint reads the sprung factor.
    scale_track: bool,
    /// `P_GROUP_REST` fades a dropped `El::group_opacity` to full: paint reads it until it lands.
    group_fade: bool,
    /// `El::time_scale`: this element's own factor; the scene multiplies ancestors' in.
    time_scale: f32,
    /// The image ratio this picture's layout took, when it sizes one side from its pixels.
    pixel_aspect: Option<f32>,
    /// `El::aspect_fit`: the ratio and the element's own style, which the fitted size replaces.
    aspect_fit: Option<Box<(f32, Style)>>,
    material: Option<crate::SystemMaterial>,
    /// Where it sits relative to a growth row, which carries its contents' motion.
    in_row: RowLayout,
    virtual_mount: bool,
    pub parent: Option<usize>,
    taffy: taffy::NodeId,
    /// Layout target in root space: x0, y0, x1, y1 in points.
    pub target: [f32; 4],
    /// Layout target in parent space.
    pub local: MRect,
    /// Padding plus border (left, top, right, bottom): text draws inside it.
    inset: [f32; 4],
    /// `El::help`.
    help: Option<String>,
    /// `El::help_when_truncated`.
    help_cut: bool,
    #[cfg(debug_assertions)]
    scroll_source: Option<&'static std::panic::Location<'static>>,
}

impl Node {
    /// Whether its text is a secret: marked secure, redacted, or a secure field.
    fn is_secure(&self) -> bool {
        self.a11y.secure || self.text.as_ref().is_some_and(|t| t.redacted) || self.input.as_ref().is_some_and(|i| i.edit.is_secure())
    }
    /// Whether a press, hover or drag can go to this node itself.
    fn has_primary_handler(&self) -> bool {
        self.on_press.is_some() || self.on_double_press.is_some() || self.on_hover.is_some() || self.on_drag.is_some() || self.drag_out.is_some()
    }
    /// Whether `selector` names this node, as `Cx::layout` reads it: its key, its full path, or
    /// a key path such as `msg-42/body` (see `flight::selects`).
    pub(crate) fn named(&self, selector: &str) -> bool {
        self.path == selector || crate::flight::selects(selector, &self.path, self.key.as_deref())
    }
}

/// What kept a frame animating, as bits in traces (`quality.why`).
pub(crate) struct AnimSource;

impl AnimSource {
    pub const TRACKS: u16 = 1;
    pub const GROWTH: u16 = 2;
    pub const SCROLL: u16 = 4;
    pub const REVEAL: u16 = 8;
    pub const LOOP: u16 = 16;
    pub const SCALE_SETTLE: u16 = 32;
    pub const VIDEO: u16 = 64;
    pub const TEXT_FLIGHT: u16 = 128;
    pub const LIVE: u16 = 256;
    pub const VIRTUAL_FILL: u16 = 512;
    pub const NAMES: [&'static str; 10] =
        ["tracks", "growth", "scroll", "reveal", "loop", "scale-settle", "video", "text-flight", "live", "virtual-fill"];

    pub fn names(bits: u16) -> Vec<&'static str> {
        Self::NAMES.iter().enumerate().filter(|(i, _)| bits & (1 << i) != 0).map(|(_, n)| *n).collect()
    }
}

#[derive(Clone, Copy)]
struct Inh {
    opacity: f32,
    paint_opacity: f32,
    clip: [f32; 4],
    fade: [f32; 4],
    fade_origin: [f32; 4],
    /// Paint clips are consumed by isolation; logical clips remain for hit tests/inspection.
    paint_clip: [f32; 4],
    paint_fade: [f32; 4],
    paint_fade_origin: [f32; 4],
    rounded: Option<([f32; 4], [f32; 4])>,
    /// Ancestors' scales, composed: a point `x` (points, with `dx`/`dy` applied) draws at
    /// `anchor_to + (x - anchor) * scale`. The anchors differ when scales nest.
    scale: f32,
    anchor: [f32; 2],
    anchor_to: [f32; 2],
    blur: f32,
    /// Vertical offset in points from scroll viewports and growth rows above.
    dy: f32,
    /// Horizontal offset in points: a matched flight centring its scaled children.
    dx: f32,
    /// The part of `dx`/`dy` from flights' covers above. A flight nested in another travels its
    /// own root-space path, so it drops this.
    cover: [f32; 2],
    /// The clip, fade and fade origin outside the nearest flight above, for a flight nested in it.
    outer: Option<[[f32; 4]; 3]>,
    mark: u32,
    /// The nearest hover group's hover amount (0–1), for `group_*` styles.
    group: f32,
    /// How `scale` is changing, which decides how text rasterizes under it.
    scale_motion: ScaleMotion,
    /// `(scale, anchor, anchor_to)` of the ancestors whose scales hold still from the root down,
    /// as an app-wide zoom does, stopping at the first that moves. A flight keeps this part.
    still: (f32, [f32; 2], [f32; 2]),
    /// Inside an active `El::cache` scope: do not cull children against the window viewport.
    in_cache: bool,
    /// Inside an elided or off-screen `El::cache` scope: skip primitive emission.
    skip_paint: bool,
    /// Ancestors' `El::rotate` layers, composed, as an index into `turn_scratch` plus one
    /// (0: none): where this subtree's device pixels land. An index keeps `Inh` small.
    turn: u32,
    snap: pixel_snap::Inherit,
}

/// A 2×3 affine in device pixels, `[a, b, c, d, tx, ty]`: `(x, y)` lands at
/// `(a·x + b·y + tx, c·x + d·y + ty)`. The composed `El::rotate` layers above a node.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Turn {
    mat: [f32; 6],
    rect: [f32; 4],
    angle: f32,
    pivot: [f32; 2],
    outer_clip: [f32; 4],
}

/// The composed turn for a node under `Inh::turn` index `inherited` turning `own` radians about
/// `pivot`: pushed onto `turns` when it turns itself. Off the paint loop's common path.
#[cold]
#[inline(never)]
fn compose_turn(
    turns: &mut Vec<Turn>,
    inherited: u32,
    own: Option<f32>,
    (rect, pivot, outer_clip): ([f32; 4], [f32; 2], [f32; 4]),
) -> (Option<Turn>, u32) {
    let above = inherited.checked_sub(1).map(|k| turns[k as usize]);
    match own {
        None => (above, inherited),
        Some(a) => {
            let own = Turn::about(a, pivot, rect, outer_clip);
            let t = above.map_or(own, |t| t.after(own));
            turns.push(t);
            (Some(t), turns.len() as u32)
        }
    }
}

/// Open the layer a turned node draws into, upright, at `rect` (x, y, w, h in device pixels).
/// The layer takes the clips from above, so the node's own clips start fresh inside it; a
/// rectangular cut through the inherited rounded mask gets an outer scope of its own.
#[cold]
#[inline(never)]
fn open_turned_layer(instances: &mut Vec<Instance>, layers: &mut Vec<(usize, usize, usize)>, (i, surfaces): (usize, usize),
    rect: [f32; 4], (angle, pivot): (f32, [f32; 2]),
    (clip, fade, fade_origin, rounded): (&mut [f32; 4], &mut [f32; 4], &mut [f32; 4], &mut Option<([f32; 4], [f32; 4])>))
{
    if let Some((mask, _)) = *rounded && intersect(*clip, mask) != mask {
        let mut marker = Instance::begin_layer(rect, *clip, Default::default());
        marker.fade = crate::render::pack_fade(*fade, *fade_origin, *clip);
        layers.push((i, instances.len(), surfaces));
        instances.push(marker);
        (*clip, *fade, *fade_origin) = (NO_CLIP, [0.0; 4], NO_CLIP);
    }
    if let Some((mask, _)) = *rounded { *clip = intersect(*clip, mask); }
    let mut marker = Instance::begin_layer(rect, *clip, Default::default()).turned(angle, pivot);
    marker.fade = crate::render::pack_fade(*fade, *fade_origin, *clip);
    if let Some((_, radii)) = *rounded { marker.round_clip_corners(radii); }
    layers.push((i, instances.len(), surfaces));
    instances.push(marker);
    (*clip, *fade, *fade_origin, *rounded) = (NO_CLIP, [0.0; 4], NO_CLIP, None);
}

/// The overlap of two `[x0, y0, x1, y1]` rects (inverted when they don't meet).
pub(crate) fn intersect(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])]
}

fn screen_clip_of(parent: Option<usize>, inh: &[Inh], turns: &[Turn]) -> [f32; 4] {
    let Some(pi) = parent else { return NO_CLIP };
    let Some(h) = inh.get(pi) else { return NO_CLIP };
    if let Some(&t) = h.turn.checked_sub(1).and_then(|k| turns.get(k as usize)) {
        let ic = if h.clip == NO_CLIP { NO_CLIP } else { t.bbox(h.clip) };
        let oc = t.outer_clip;
        intersect(oc, ic)
    } else {
        h.clip
    }
}

impl Turn {
    /// `angle` radians clockwise about `p`.
    fn about(angle: f32, p: [f32; 2], rect: [f32; 4], outer_clip: [f32; 4]) -> Turn {
        let (sn, c) = angle.sin_cos();
        Turn {
            mat: [c, -sn, sn, c, p[0] - c * p[0] + sn * p[1], p[1] - sn * p[0] - c * p[1]],
            rect,
            angle,
            pivot: p,
            outer_clip,
        }
    }

    /// `inner`, then `self`.
    fn after(self, inner: Turn) -> Turn {
        let [a, b, c, d, x, y] = self.mat;
        let [e, f, g, h, u, v] = inner.mat;
        let ic = if inner.outer_clip == NO_CLIP { NO_CLIP } else { self.bbox(inner.outer_clip) };
        let oc = self.outer_clip;
        Turn {
            mat: [a * e + b * g, a * f + b * h, c * e + d * g, c * f + d * h, a * u + b * v + x, c * u + d * v + y],
            outer_clip: intersect(oc, ic),
            ..inner
        }
    }

    /// The upright bounding box of edges `r` once turned.
    fn bbox(self, r: [f32; 4]) -> [f32; 4] {
        let [a, b, c, d, x, y] = self.mat;
        let mut out = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
        for (px, py) in [(r[0], r[1]), (r[2], r[1]), (r[0], r[3]), (r[2], r[3])] {
            let (qx, qy) = (a * px + b * py + x, c * px + d * py + y);
            out = [out[0].min(qx), out[1].min(qy), out[2].max(qx), out[3].max(qy)];
        }
        out
    }

    /// Unturns a screen-space point `q` back into the layer's upright device pixels.
    fn unturn(self, [qx, qy]: [f32; 2]) -> [f32; 2] {
        let [a, b, c, d, tx, ty] = self.mat;
        let (dx, dy) = (qx - tx, qy - ty);
        [a * dx + c * dy, b * dx + d * dy]
    }
}

/// How a subtree's scale is changing. Text under a still scale rasterizes at its scaled size.
/// Under a live zoom gesture it steps through logarithmic $2^{1/4}$ buckets and lands on the
/// exact resting scale once still; under a short spring it samples its rest-size glyphs.
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
enum ScaleMotion {
    #[default]
    Still,
    /// Bound to a live scale that is actively changing: rasterize at $2^{1/4}$ octave buckets.
    Live,
    Moving,
}

impl ScaleMotion {
    /// The scale text under `sc` rasterizes at. It draws at `sc`, filtered, when they differ.
    fn raster(self, sc: f32) -> f32 {
        let k = match self {
            ScaleMotion::Still => sc,
            ScaleMotion::Live => 2.0f32.powf((sc.max(1e-4).log2() * 4.0).round() * 0.25),
            ScaleMotion::Moving => 1.0,
        };
        // A tiny or huge raster isn't worth its atlas space; sample the rest size instead.
        if (1.0 / 16.0..=4.0).contains(&k) { k } else { 1.0 }
    }
}

impl Default for Inh {
    fn default() -> Self {
        Inh { opacity: 1.0, paint_opacity: 1.0, clip: NO_CLIP, fade: [0.0; 4], fade_origin: NO_CLIP,
            paint_clip: NO_CLIP, paint_fade: [0.0; 4], paint_fade_origin: NO_CLIP, rounded: None,
            scale: 1.0, anchor: [0.0; 2], anchor_to: [0.0; 2], blur: 0.0, dy: 0.0, dx: 0.0, cover: [0.0; 2], outer: None, mark: 0, group: 0.0,
            scale_motion: ScaleMotion::Still, still: (1.0, [0.0; 2], [0.0; 2]), in_cache: false, skip_paint: false, turn: 0,
            snap: Default::default() }
    }
}

#[derive(Clone, Copy, Debug)]
struct ScrollState {
    offset: f64,
    v: f32,
    pinned: bool,
    max: f64,
    direct: bool,
    scrolling: bool,
    /// Velocity of a glide to the bottom (`Tx::scroll_to_bottom`) or to `glide_to`
    /// (`Tx::scroll_to`) while one runs.
    glide: Option<f32>,
    /// The glide is `ease_to_shrunk_end`'s, which retargets or ends it as the rows move.
    shrink_glide: bool,
    /// A hand scrolled while the growth stack moved: no shrink glide until it settles.
    shrink_cut: bool,
    /// Where a glide goes, when not to the bottom.
    glide_to: Option<f64>,
    /// The growth stack's layout end while its rows still move (see `ease_to_shrunk_end`).
    laid: Option<f64>,
    /// At-bottom state last sent to `El::on_at_bottom`.
    at_bottom: Option<bool>,
    /// State last sent to `El::on_far_from_bottom`.
    far_from_bottom: Option<bool>,
    /// Whether the last frame drew the content's bottom on the viewport's (see `stuck_offset`).
    held: bool,
    /// Room freed under a pinned growth stack, held under its last row (see `step_slack`).
    slack: scroll_bottom::Slack,
    /// The horizontal axis of `El::scroll_x` and `El::scroll_xy`; all zero on a vertical one.
    x: AxisScroll,
    /// What `El::on_scroll_offset` was last told (`runtime/scroll_watch.rs`).
    told: Option<crate::ScrollPos>,
}

/// A scroll viewport's horizontal offset, momentum velocity and limit (points).
#[derive(Clone, Copy, Debug, Default)]
struct AxisScroll {
    offset: f64,
    v: f32,
    max: f64,
    /// Velocity of a glide to `aim` (`Tx::scroll_to`) while one runs.
    glide: Option<f32>,
    aim: f64,
}

#[derive(Clone, Debug, Default)]
struct RevealState {
    len: usize,
    /// (word start byte, start time) for words still fading.
    words: Vec<(u32, f64)>,
    fade: f32,
}

#[derive(Clone, Debug, Default)]
struct Stack {
    /// Row ids in presented order, including rows still closing after removal.
    order: Vec<NodeId>,
    /// Removed rows: when their space starts closing.
    closing: HashMap<NodeId, f64>,
    /// Last known target top (container-relative) per row, for rows that left.
    tops: HashMap<NodeId, f32>,
    pad_top: f32,
    pad_bottom: f32,
    gap: f32,
    width: f32,
    /// Measure at the landing width immediately; defer height targets until it is stable.
    width_hold_until: f64,
    width_shrink_until: f64,
    pending_heights: HashMap<NodeId, f32>,
    /// The window size at the last commit.
    window: (f32, f32),
    /// How far its rows grew (positive steps only) this frame.
    grown: f32,
    /// Whether any row in the stack still has height left to grow.
    pub(crate) growing: bool,
    /// Whether any row in the stack is still animating.
    pub(crate) moving: bool,
    reorder: row_reorder::Reorder,
}

struct LayoutEntry {
    id: taffy::NodeId,
    measure: Option<u64>,
    used: bool,
}

pub struct Runtime<S: 'static> {
    pub state: S,
    pub(crate) on_focus: Option<fn(&mut S, bool, &mut Tx)>,
    /// `App::on_ime` and the composition it has open.
    pub(crate) app_ime: app_ime::AppIme<S>,
    view: fn(&S, &mut Cx<S>) -> El,
    /// `App::playground`, when this run shows it.
    pub(crate) playground: Option<Box<crate::playground::Live<S>>>,
    #[cfg(feature = "hot")]
    pub(crate) hot: Option<crate::hot::Session>,
    pub text: TextSystem,
    /// RGBA images elements show (`El::pixels`), uploaded by the renderer.
    pub images: crate::pixels::ImageAtlas,
    pub scene: Scene,
    pub env: Env,
    /// Viewport in points.
    pub viewport: (f32, f32),
    /// Transparent native surfaces/headless exports clear to transparent black.
    pub transparent: bool,
    /// Opt-in gamma-space (sRGB) blending for translucent fills, shadows and blurs.
    pub(crate) srgb_blend: bool,
    pub(crate) native_overlay: bool,
    /// The window or overlay surface's frame counts, published by its host.
    pub(crate) surface_stats: Option<std::sync::Arc<std::sync::Mutex<crate::SurfaceStats>>>,
    /// `App::dev_stats`.
    pub(crate) app_stats: Option<fn() -> String>,
    /// Animation time (seconds, scaled by the time scale).
    pub t: f64,
    pub reduced_motion: bool,
    system_reduced_motion: bool,
    titlebar_inset: f32,
    system_accent: crate::SystemAccent,
    pub time_scale: f32,
    /// Called at the start of every frame with the clock; return true when state changed (new data).
    pub poll: Option<fn(&mut S, f64, &mut Tx) -> bool>,
    /// Seconds since the first frame, unscaled (virtual when headless).
    pub clock: f64,
    wake_deadlines: Vec<f64>,
    timer_wake: timer_wake::TimerWake,
    idle_meter: crate::perf_warn::IdleMeter,
    /// Windowed runs read the clock from the wall instead of summing frame steps.
    pub wall_start: Option<crate::time::Instant>,
    /// Called for keys unhandled by the focused control; return true to consume it.
    pub on_key: Option<fn(&mut S, &KeyEvent, &mut Tx) -> bool>,
    pub(crate) nodes: Vec<Node>,
    /// The last commit's node buffer, emptied, for the next flatten to fill: an app of a few
    /// thousand elements otherwise maps and faults in megabytes every commit.
    spare_nodes: Vec<Node>,
    layout: TaffyTree<usize>,
    layout_entries: HashMap<String, LayoutEntry>,
    node_index: HashMap<NodeId, usize>,
    path_ids: HashMap<String, NodeId>,
    pub(crate) memo: crate::memo::MemoCache,
    next_id: u64,
    commits: u64,
    dirty: bool,
    relayout: bool,
    /// Every viewport change since the last commit came from the window's own resize
    /// animation (`step_viewport`), not the user or the system.
    viewport_stepped: bool,
    settle: bool,
    queue: ActionQueue,
    pointer: Option<(f32, f32)>,
    capture: Option<(String, Drag)>,
    /// The button holding `capture` (`El::pointer_buttons`).
    capture_button: crate::PointerButton,
    secondary_capture: Option<String>,
    capture_inside: bool,
    /// The pointer left while a press held it; the leave lands on release.
    left_in_capture: bool,
    drag_data: f32,
    pub instances: Vec<Instance>,
    /// Per instance, the `El::mark` of its nearest marked node (0 for none). Filled only while
    /// `track_marks` is set (a compositor is attached).
    pub marks: Vec<u32>,
    pub track_marks: bool,
    pub surfaces: Vec<crate::native_surface::SurfaceDraw>,
    /// `El::system_material` elements built this frame, in paint order.
    pub(crate) materials: Vec<crate::system_material::MaterialDraw>,
    /// The shell shows native views for the first `MAX_NATIVE_MATERIALS` of `materials`; their
    /// elements skip their fill and cut their shadows out of their interior.
    pub(crate) native_materials: bool,
    hits: Vec<(NodeId, [f32; 4])>,
    overlay_hits: Vec<[f32; 4]>,
    glyph_scratch: Vec<GlyphQuad>,
    deco_scratch: Vec<PaintedDeco>,
    inh_scratch: Vec<Inh>,
    /// `motion_elements`' per-node scratch, kept so a frame doesn't allocate.
    motion_scratch: std::cell::RefCell<Vec<(u32, u16, bool)>>,
    /// The composed turns `Inh::turn` points into, rebuilt each paint.
    turn_scratch: Vec<Turn>,
    /// For nodes inside a turned layer: `(unturned_rect_pt, unturned_clip_px)`. Empty unless a
    /// node turns.
    unturned_drawn: Vec<([f32; 4], [f32; 4])>,
    /// Only nonempty when retained exit nodes or matched flights make storage order differ
    /// from paint order.
    layer_paint_order: Vec<usize>,
    /// The last commit's `El::matched` ids, one present element each (the last in tree order).
    matched: Vec<(Rc<str>, NodeId)>,
    /// Elements flying from a matched one, with their scene target. They paint last.
    flying: Vec<(NodeId, MRect)>,
    /// `Tx::launch` flights, waiting for their commit or in the air.
    launches: flights::Launches,
    pixel_snap: pixel_snap::Glides,
    /// `inh_scratch` and `row_dy` index `nodes`: no commit since the last build.
    built: bool,
    /// The node ids the last build indexed its scratch by.
    built_ids: Vec<NodeId>,
    #[cfg(debug_assertions)]
    matched_warned: HashSet<Rc<str>>,
    layer_scratch: Vec<(usize, usize, usize)>,
    /// Sideways scroll of focused single-line fields, in points, keyed by node path.
    input_scroll: HashMap<String, f32>,
    dissolve_scratch: Vec<bool>,
    has_dissolve: bool,
    cache_live_scratch: Vec<bool>,
    cache_check_scratch: Vec<bool>,
    cache_paint_scratch: Vec<u64>,
    cache_inside_scratch: Vec<bool>,
    node_caches: HashMap<NodeId, NodeCache>,
    has_cache: bool,
    commit_gen: u64,
    last_dt: f64,
    /// Some picture sizes a side from its image's ratio (`pixel_aspect`).
    has_pixel_aspect: bool,
    has_drawn_layout: bool,
    has_key_up: bool,
    /// Some element is `El::sticky_top` or `El::sticky_bottom` (see `sticky.rs`).
    has_sticky: bool,
    /// Each node's sticky shift this frame (points), while `has_sticky`.
    stuck: Vec<f32>,
    /// Per node, what its children stick within: the nearest vertical viewport's top and
    /// bottom, then the node's own content box top and bottom (unscaled points), while
    /// `has_sticky`.
    sticky_frame: Vec<[f32; 8]>,
    /// `Tx::keep_on_screen` across the commit underway.
    keeps: Vec<sticky::Keep>,
    /// `Tx::scroll_into_view` requests that follow their target while growth/layout springs run.
    scroll_reveals: Vec<sticky::Reveal>,
    /// Some element is `El::raise`d: paint order isn't tree order.
    has_raise: bool,
    /// The focus where the held non-IME key(s) went down, so a release after focus moves
    /// still reaches that element first.
    key_focus: Option<String>,
    keys_held: u32,
    ime_keys_down: u32,
    ime_eat_key_up: bool,
    /// Pictures whose image changed shape since the last commit, which lays them out at once.
    aspect_snap: Vec<NodeId>,
    /// Drawn rect per node (points, root space), for tools and hit tests.
    drawn: Vec<[f32; 4]>,
    /// The scale each node's text drew at (its own and its ancestors'), for text hit tests.
    drawn_scale: Vec<f32>,
    /// Per node: it drew inside its clip and the window in the last build.
    seen: Vec<bool>,
    scrolls: HashMap<NodeId, ScrollState>,
    /// The axis the current wheel or trackpad gesture scrolls (`scroll_x.rs`).
    wheel_lock: scroll_x::WheelLock,
    #[cfg(any(test, target_arch = "wasm32"))]
    web_scroll: Option<web_runtime::TouchScroll>,
    grows: HashMap<NodeId, GrowTrack>,
    stacks: HashMap<NodeId, Stack>,
    pub(crate) virtuals: HashMap<String, crate::virtual_list::VirtualList>,
    /// The memo pass the current commit's first view build was. Memos none of its builds used
    /// go once it applies.
    memo_since: u64,
    /// Frames in a row an index-built list has asked for to fill its overscan.
    virtual_fill: u32,
    /// True when the commit in flight is for scroll/layout only (`self.state` wasn't mutated).
    state_unchanged: bool,
    reuse: relayout::Reuse,
    pub(crate) virtual_culled: HashSet<String>,
    /// Fresh view accessors for focused text fields omitted by virtualization.
    virtual_inputs: HashMap<String, Box<InputSpec>>,
    reveals: HashMap<NodeId, RevealState>,
    flights: Vec<TextFlight>,
    /// Per node index this frame: presented row top dy, row height override, and bottom feather mask.
    row_dy: Vec<f32>,
    row_h: Vec<f32>,
    row_mask: Vec<f32>,
    grow_acc: f32,
    caught_up: f32,
    slack_dt: f32,
    focused: Option<String>,
    keyboard_focus: bool,
    /// The pointer focused a `focus_highlight` row, so its highlight shows without a key.
    hover_focus: bool,
    /// Type-select's typed prefix and when its last key arrived (`now_clock`).
    typeahead: (String, f64),
    /// The activation key (Enter, Escape, Space) held down and the focus it was first pressed
    /// on. Its repeats are dropped once focus has moved, so a held key that closed something
    /// can't act again on whatever took focus next.
    held: Option<physical_dispatch::HeldKey>,
    /// Modifier keys down, for `Key::Modifier` (off unless the app opts in).
    modifier_keys: modifier_keys::ModifierKeys,
    /// Tab visits every control; off, only text fields and lists (macOS Full Keyboard
    /// Access off). Native hosts sample the system setting; headless runtimes default on.
    full_keyboard_access: bool,
    /// `App::tab_visits_controls`: Tab visits every control whatever `full_keyboard_access` says.
    tab_visits_controls: bool,
    modal_stack: Vec<(String, Option<String>)>,
    /// Focus when the running commit began, before a new view's focus request applied: what a
    /// modal that opens in this commit returns focus to.
    focus_before_commit: Option<String>,
    /// Focus before the current gesture/commit blurred it, so a scrim press that blurs on
    /// pointer-down and closes its overlay on pointer-up still honours `focus_return`.
    last_focused: Option<String>,
    /// Focused text fields that unmounted without being blurred: while another field holds focus,
    /// remounting one with its old `focused = true` flag does not steal focus back.
    unmounted_focused: Vec<std::sync::Weak<crate::edit::History>>,
    modal_index: Option<usize>,
    modal_members: Vec<bool>,
    loops: HashMap<NodeId, rune_motion::LoopTrack>,
    loop_clock: loop_clock::LoopClock,
    loops_active: bool,
    /// `El::scale` per node that has one or had one: (factor, animation time it last changed,
    /// whether it has ever changed). See [`ScaleMotion`].
    static_scales: HashMap<NodeId, (f32, f64, bool)>,
    /// A changed `El::scale` hasn't held still long enough for its text to rasterize.
    static_scale_settling: bool,
    /// Some node ran on a scaled clock (`El::time_scale`) at the last commit.
    time_scaled: bool,
    /// Shared host deadline: unscaled wake time and corresponding animation time.
    loop_wake: Option<(f64, f64)>,
    surface_visible: bool,
    hovered: Option<String>,
    /// Elements with `on_hover_change` that the pointer is inside.
    hover_within: Vec<String>,
    /// `on_hover_change` handlers of elements that left the tree, owed their queued leave.
    hover_gone: Vec<(String, crate::el::WindowFocusFn)>,
    /// Elements with `hover_delays` whose `on_hover_change` last heard `true`.
    hover_shown: Vec<String>,
    /// Delayed hover changes waiting out their delay: path, inside, due on the clock.
    hover_pending: Vec<(String, bool, f64)>,
    /// A press stopped the waits to report an enter; the pointer's next move restarts them.
    hover_paused: bool,
    /// Hover waiting out layout motion under a still pointer.
    hover_hold: hover::HoverHold,
    /// Files dragged over the window from another app (`drop_dispatch`).
    file_drag: drop_dispatch::FileDrag,
    pub ime_area: Option<[f32; 4]>,
    /// The focused field's caret (path, window rect) the next view sees through `Cx::caret`.
    caret: Option<(String, [f32; 4])>,
    /// The last view read the caret, so it re-runs when the caret moves.
    caret_watch: bool,
    /// Keys the last view read with `Cx::layout` or `Cx::layout_size`, and what each resolved to.
    layout_seen: layout_read::LayoutSeen,
    /// Rect last sent to each element's `El::on_layout`.
    layout_fired: HashMap<NodeId, [f32; 4]>,
    /// Rect last sent to each element's `El::on_screen_layout`.
    screen_fired: HashMap<NodeId, [f32; 4]>,
    /// (Drawn rect, laid-out rect) last sent to each element's `El::on_drawn_layout`.
    drawn_fired: HashMap<NodeId, ([f32; 4], [f32; 4])>,
    /// The window's content top-left in global screen points, as its host last saw it.
    window_origin: (f32, f32),
    /// Commits in a row caused only by layout feedback (see `layout_read`).
    layout_echoes: u32,
    /// An edit ran this frame: keep the caret in its scroll viewport after the commit.
    caret_follow: bool,
    links: links::LinkState,
    /// The selection in `El::selectable` text.
    text_sel: selectable::Held,
    /// `Tx::text_selection`'s snapshot, kept while neither a commit nor the selection changed.
    sel_snapshot: std::cell::RefCell<Option<selectable::Snapshot>>,
    /// What each `El::on_selection_change` element (by path) last heard.
    sel_heard: selectable::Heard,
    /// Multi-click and drag selection state for `text_input`.
    input_click: input_click::InputClick,
    /// Modifier keys held now, as the window last reported them (`Tx::mods`).
    held_mods: crate::Mods,
    /// `El::help` tooltips.
    help: help::HelpState,
    /// The shell cancels native composition before enabling the new text focus.
    pub(crate) ime_reset: bool,
    /// Text the focused field asked to copy (the shell writes it to the clipboard).
    pub copy_request: Option<crate::ClipboardWrite>,
    // Outer None: no read; inner None: app-level paste rather than a focused editor.
    pub(crate) paste_request: Option<Option<String>>,
    /// `Tx::read_clipboard` reads for the shell to start, by id.
    pub(crate) read_requests: Vec<u64>,
    /// Their callbacks, until the clipboard answers.
    clipboard_reads: Vec<(u64, crate::el::ReadFn)>,
    /// The plain text and pill ranges of this runtime's last copy that held pills, for web
    /// clipboard pastes where the OS worker does not own the pasteboard.
    pill_copy: Option<(zeroize::Zeroizing<String>, Vec<(std::ops::Range<usize>, String)>)>,
    next_clipboard_read: u64,
    pub(crate) clear_secret_request: bool,
    /// `Runtime::use_clipboard`: a test clipboard that replaces the OS one.
    pub(crate) clipboard: Option<crate::clipboard::MemoryClipboard>,
    /// A handler or `update` asked to close the window or quit (the shell takes it).
    pub shell_request: Option<Shell>,
    /// A handler or `update` asked to minimize the window (`Tx::minimize`); the shell takes it.
    pub(crate) minimize_request: bool,
    /// Links handlers asked to open that this runtime left unopened (`Runtime::take_url_requests`).
    pub(crate) url_requests: Vec<String>,
    /// True when attached to a live window, overlay or web canvas, not a headless or scripted run.
    pub(crate) open_urls_natively: bool,
    /// Pop-up menus asked for (`Tx::popup_menu`) and not yet answered, oldest first.
    pub(crate) popups: std::collections::VecDeque<crate::popup_menu::Request>,
    /// The first of `popups` is showing.
    pub(crate) popup_open: bool,
    /// The file dialog asked for (`Tx::open_file`, `Tx::save_file`) and not yet answered.
    pub(crate) file_dialog: Option<crate::file_dialog::Pending>,
    /// File dialog callbacks, by id, until their answer runs.
    file_dialog_answers: Vec<(u64, crate::file_dialog::AnswerFn)>,
    next_file_dialog: u64,
    /// A frame change for the window (`Tx::resize_window`), with its spring resolved.
    pub(crate) window_frame: Option<crate::window_frame::FrameRequest>,
    /// The window's title (`Tx::set_title`), from `AppSpec`'s.
    pub(crate) window_title: window_requests::Title,
    /// The window's height follows the root's natural height (`App::fit_content_height`).
    pub(crate) fit_height: bool,
    /// The content height last asked of the window.
    fit_asked: Option<f32>,
    /// A fitting window at rest asked to move: this frame shows the commit's motion at its
    /// start, as the window can't move until the next frame.
    fit_hold: bool,
    /// The last frame held its motion for the window (`fit_hold`). The host reads it.
    pub(crate) fit_held: bool,
    /// The host's window frame isn't moving or about to: set by the host before each frame.
    pub(crate) window_resting: bool,
    /// What `update` asked of the next commit.
    pending: Tx,
    pub animating: bool,
    /// Why the last frame was animating, as [`AnimSource`] bits, for traces.
    pub(crate) anim_sources: u16,
    /// This frame's commit landed every track (`Tx::settle`): a deliberate cut.
    pub settled: bool,
    /// Input markers for traces and headless tests, at most `event_log::CAP` of them unread.
    pub events: Vec<String>,
    /// Record `events` without a trace: headless runs. Live hosts turn it off.
    pub(crate) record_events: bool,
    /// Tooling opts in before any callbacks. Absent in ordinary app runs.
    pub(crate) trace_channels: Option<HashMap<&'static str, (&'static str, f32)>>,
    /// Counters for perf gates.
    pub stats: FrameStats,
    live_wake: Arc<crate::live::Wake>,
    live_clock: Arc<()>,
    window_focused: Option<bool>,
    /// `keys_lost` queued its `FocusLost` since the window last had focus.
    keys_lost: bool,
    live_subscriptions: Vec<Arc<crate::live::Subscription>>,
    live_dirty: bool,
    /// Events whose handler said `Tx::live_only`, and so didn't commit.
    live_only_skips: u64,
    /// The clock at the last commit.
    last_commit_clock: f64,
    /// Debug builds check `Tx::live_only` events (`skip_view`).
    #[cfg(debug_assertions)]
    live_only_check: bool,
    /// The live-only event this frame's commit checks: its handler, and what the view had built.
    #[cfg(debug_assertions)]
    live_check: Option<(String, u64)>,
    /// Live-only events checked (and so committed) so far.
    #[cfg(debug_assertions)]
    live_checks: u64,
    /// Stale-view warnings printed for `Tx::live_only` handlers, each once.
    #[cfg(debug_assertions)]
    pub(crate) live_only_warnings: Vec<String>,
    /// When the frame being built will present; videos pull the picture for it.
    pub(crate) frame_target: Option<crate::time::Instant>,
    /// A drawn video is playing or seeking: keep drawing frames.
    videos_active: bool,
    /// Drawn videos' state generations; a change re-runs the view.
    video_signature: u64,
    /// Opt-in tooling checks. None keeps release commits free of key traversal/allocation.
    pub(crate) key_warnings: Option<Vec<String>>,
    /// Seconds until the next frame while only [`Cycle`](crate::el::Cycle)s move on screen
    /// (the shell sleeps until then instead of drawing every display frame).
    pub ambient: Option<f32>,
    /// Where (window points) the current press started.
    press_at: (f32, f32),
    /// The last press on a window drag region: when (clock seconds) and where.
    last_drag_press: Option<(f64, f32, f32)>,
    /// The first of a possible double press: the element, its down time and point.
    last_press: Option<(String, f64, f32, f32)>,
    /// The press now held is the second of a double press.
    double_armed: bool,
    /// The presses counted into `Drag::clicks`.
    press_clicks: press_clicks::PressClicks,
    press_down_t: f64,
    /// The held press has travelled past the click slop: it isn't a click of a double press.
    press_moved: bool,
    /// A scroll under the held press cancelled it ([`Runtime::scroll_cancels_press`]).
    press_cancelled: bool,
    titlebar_action: Option<TitlebarAction>,
    /// Two presses closer than this (seconds) are a double press: the system setting on macOS
    /// and Windows, 0.5 elsewhere and headless.
    pub double_click_interval: f64,
    /// How long (seconds) the pointer rests on an `El::help` element before its tooltip shows:
    /// the system's hover time on Windows, 0.5 elsewhere and headless. macOS windows use
    /// AppKit's own tooltips.
    pub help_delay: f64,
    /// The host starts OS drags, so `drag_out` elements start one (headless tools act as one).
    os_drag: bool,
    /// The held press is on a `drag_out` element that hasn't crossed the threshold yet.
    drag_out_armed: bool,
    /// The armed element's drag threshold (points).
    drag_out_threshold: f32,
    drag_out_request: Option<crate::drag_out::DragOutStart>,
    /// How OS drags ended, filled by the host's session callbacks between frames.
    drag_ends: crate::drag_out::DragEnds,
    /// Drag-outs whose `on_drag_start` ran and whose `on_drag_end` hasn't yet.
    drags_open: usize,
}

/// What a press on a window drag region asks the shell to do, synchronously in that press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitlebarAction {
    /// Move the window with the pointer (the system tracks the drag).
    Drag,
    /// The titlebar double-click action (zoom, minimise or nothing, per system setting).
    DoubleClick,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FrameStats {
    pub commits: u64,
    pub layout_nodes: usize,
    pub glyphs: usize,
    pub shaped: u32,
}

fn ease_out(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    1.0 - (1.0 - x) * (1.0 - x) * (1.0 - x)
}

fn word_starts(s: &str, from: usize, out: &mut Vec<u32>) {
    let mut prev_space = true;
    for (i, c) in s[from..].char_indices() {
        let space = c.is_whitespace();
        if !space && prev_space {
            out.push((from + i) as u32);
        }
        prev_space = space;
    }
}

impl<S: 'static> Runtime<S> {
    pub fn new(state: S, view: fn(&S, &mut Cx<S>) -> El, viewport: (f32, f32), env: Env) -> Self {
        Self::with_text(state, view, viewport, env, TextSystem::default())
    }

    pub fn with_text(state: S, view: fn(&S, &mut Cx<S>) -> El, viewport: (f32, f32), env: Env, text: TextSystem) -> Self {
        let mut scene = Scene::new();
        scene.set_pixel_scale(env.scale);
        crate::recorder::headless_reset();
        crate::video::headless_reset();
        Runtime {
            state,
            view,
            playground: None,
            #[cfg(feature = "hot")]
            hot: None,
            text,
            images: Default::default(),
            scene,
            env,
            viewport,
            transparent: false,
            srgb_blend: false,
            native_overlay: false,
            surface_stats: None,
            app_stats: None,
            t: 0.0,
            reduced_motion: false,
            system_reduced_motion: false,
            titlebar_inset: 0.0,
            system_accent: crate::SystemAccent::DEFAULT,
            time_scale: 1.0,
            poll: None,
            clock: 0.0,
            wake_deadlines: Vec::new(),
            timer_wake: Default::default(),
            idle_meter: Default::default(),
            wall_start: None,
            on_key: None,
            nodes: Vec::new(),
            spare_nodes: Vec::new(),
            layout: {
                let mut tree = TaffyTree::new();
                tree.disable_rounding();
                tree
            },
            layout_entries: HashMap::default(),
            node_index: HashMap::default(),
            path_ids: HashMap::default(),
            memo: Default::default(),
            next_id: 1,
            commits: 0,
            dirty: true,
            relayout: false,
            viewport_stepped: false,
            settle: false,
            queue: ActionQueue::default(),
            pointer: None,
            capture: None,
            capture_button: crate::PointerButton::Primary,
            secondary_capture: None,
            capture_inside: false,
            left_in_capture: false,
            drag_data: 0.0,
            instances: Vec::new(),
            marks: Vec::new(),
            track_marks: false,
            surfaces: Vec::new(),
            materials: Vec::new(),
            native_materials: false,
            hits: Vec::new(),
            overlay_hits: Vec::new(),
            glyph_scratch: Vec::new(),
            deco_scratch: Vec::new(),
            inh_scratch: Vec::new(),
            motion_scratch: Default::default(),
            turn_scratch: Vec::new(),
            unturned_drawn: Vec::new(),
            layer_paint_order: Vec::new(),
            matched: Vec::new(),
            flying: Vec::new(),
            built: false,
            built_ids: Vec::new(),
            #[cfg(debug_assertions)]
            matched_warned: HashSet::default(),
            layer_scratch: Vec::new(),
            input_scroll: HashMap::default(),
            dissolve_scratch: Vec::new(),
            has_dissolve: false,
            cache_live_scratch: Vec::new(),
            cache_check_scratch: Vec::new(),
            cache_paint_scratch: Vec::new(),
            cache_inside_scratch: Vec::new(),
            node_caches: HashMap::default(),
            has_cache: false,
            commit_gen: 0,
            last_dt: 0.0,
            has_pixel_aspect: false,
            has_drawn_layout: false,
            has_key_up: false,
            has_sticky: false,
            stuck: Vec::new(),
            sticky_frame: Vec::new(),
            keeps: Vec::new(),
            scroll_reveals: Vec::new(),
            has_raise: false,
            key_focus: None,
            keys_held: 0,
            ime_keys_down: 0,
            ime_eat_key_up: false,
            aspect_snap: Vec::new(),
            drawn: Vec::new(),
            drawn_scale: Vec::new(),
            seen: Vec::new(),
            scrolls: HashMap::default(),
            wheel_lock: Default::default(),
            #[cfg(any(test, target_arch = "wasm32"))]
            web_scroll: None,
            grows: HashMap::default(),
            stacks: HashMap::default(),
            virtuals: HashMap::default(),
            virtual_culled: HashSet::default(),
            memo_since: 0,
            virtual_fill: 0,
            state_unchanged: false,
            reuse: Default::default(),
            virtual_inputs: HashMap::default(),
            reveals: HashMap::default(),
            flights: Vec::new(),
            launches: Default::default(),
            pixel_snap: Default::default(),
            row_dy: Vec::new(),
            row_h: Vec::new(),
            row_mask: Vec::new(),
            grow_acc: 0.0,
            caught_up: 0.0,
            slack_dt: 0.0,
            focused: None,
            keyboard_focus: false,
            hover_focus: false,
            typeahead: (String::new(), f64::NEG_INFINITY),
            held: None,
            modifier_keys: Default::default(),
            full_keyboard_access: true,
            tab_visits_controls: false,
            modal_stack: Vec::new(),
            focus_before_commit: None,
            last_focused: None,
            unmounted_focused: Vec::new(),
            modal_index: None,
            modal_members: Vec::new(),
            loops: HashMap::default(),
            loop_clock: Default::default(),
            loops_active: false,
            static_scales: HashMap::default(),
            static_scale_settling: false,
            time_scaled: false,
            loop_wake: None,
            surface_visible: true,
            hovered: None,
            hover_within: Vec::new(),
            hover_gone: Vec::new(),
            hover_shown: Vec::new(),
            hover_pending: Vec::new(),
            hover_paused: false,
            hover_hold: Default::default(),
            file_drag: Default::default(),
            ime_area: None,
            caret: None,
            caret_watch: false,
            layout_seen: Default::default(),
            layout_fired: HashMap::default(),
            screen_fired: HashMap::default(),
            drawn_fired: HashMap::default(),
            window_origin: (0.0, 0.0),
            layout_echoes: 0,
            caret_follow: false,
            links: Default::default(),
            text_sel: Default::default(),
            sel_snapshot: Default::default(),
            sel_heard: Default::default(),
            input_click: Default::default(),
            held_mods: Default::default(),
            help: Default::default(),
            help_delay: 0.5,
            ime_reset: false,
            copy_request: None,
            paste_request: None,
            read_requests: Vec::new(),
            clipboard_reads: Vec::new(),
            pill_copy: None,
            next_clipboard_read: 0,
            clear_secret_request: false,
            clipboard: None,
            shell_request: None,
            minimize_request: false,
            url_requests: Vec::new(),
            open_urls_natively: false,
            popups: Default::default(),
            file_dialog: None,
            file_dialog_answers: Vec::new(),
            next_file_dialog: 0,
            window_frame: None,
            window_title: Default::default(),
            fit_height: false,
            fit_asked: None,
            fit_hold: false,
            fit_held: false,
            window_resting: false,
            popup_open: false,
            pending: Tx::default(),
            animating: false,
            anim_sources: 0,
            settled: false,
            events: Vec::new(),
            record_events: true,
            trace_channels: None,
            stats: FrameStats::default(),
            live_wake: crate::live::Wake::new(Arc::new(|| {})),
            live_clock: Arc::new(()),
            frame_target: None,
            videos_active: false,
            video_signature: 0,
            window_focused: None,
            keys_lost: false,
            on_focus: None,
            app_ime: Default::default(),
            live_subscriptions: Vec::new(),
            live_dirty: false,
            live_only_skips: 0,
            last_commit_clock: f64::NEG_INFINITY,
            #[cfg(debug_assertions)]
            live_only_check: true,
            #[cfg(debug_assertions)]
            live_check: None,
            #[cfg(debug_assertions)]
            live_checks: 0,
            #[cfg(debug_assertions)]
            live_only_warnings: Vec::new(),
            key_warnings: None,
            ambient: None,
            press_at: (0.0, 0.0),
            last_drag_press: None,
            last_press: None,
            double_armed: false,
            press_clicks: Default::default(),
            press_down_t: 0.0,
            press_moved: false,
            press_cancelled: false,
            titlebar_action: None,
            double_click_interval: 0.5,
            os_drag: true,
            drag_out_armed: false,
            drag_out_threshold: crate::drag_out::DRAG_THRESHOLD,
            drag_out_request: None,
            drag_ends: Default::default(),
            drags_open: 0,
        }
    }

    /// A window drag or titlebar double-click requested by the press just delivered. The shell
    /// must act on it before returning from that press event.
    pub fn take_titlebar_action(&mut self) -> Option<TitlebarAction> {
        self.titlebar_action.take()
    }

    /// Clock seconds now: the wall clock in a window (input arrives between frames), the
    /// virtual clock headless.
    fn now_clock(&self) -> f64 {
        self.wall_start.map_or(self.clock, |t| t.elapsed().as_secs_f64())
    }

    pub fn set_viewport(&mut self, viewport: (f32, f32), scale: f32) {
        // Someone resized a window that fits its content: it goes back to the content's height.
        if self.fit_height && self.window_resting && (self.fit_asked.unwrap_or(self.viewport.1) - viewport.1).abs() > 0.5 {
            self.fit_asked = None;
            self.relayout = true;
            self.viewport_stepped = false;
        }
        if self.change_viewport(viewport, scale) { self.viewport_stepped = false; }
    }

    /// The viewport one frame of the window's own resize animation moved it to. Unlike a
    /// user's resize it doesn't land running motion: content at rest follows the window and
    /// springs keep going.
    pub(crate) fn step_viewport(&mut self, viewport: (f32, f32), scale: f32) {
        let fresh = !self.relayout;
        if self.change_viewport(viewport, scale) && fresh { self.viewport_stepped = true; }
    }

    /// Whether the change needs a layout.
    fn change_viewport(&mut self, viewport: (f32, f32), scale: f32) -> bool {
        // A window that fits its content lays out the same at any height: its own resize
        // mustn't cut short the layout springs it is following.
        if self.fit_height && viewport.0 == self.viewport.0 && scale == self.env.scale {
            self.viewport = viewport;
            return false;
        }
        let changed = viewport != self.viewport || scale != self.env.scale;
        if changed {
            if scale != self.env.scale { self.flights.clear(); }
            self.viewport = viewport;
            self.env.scale = scale;
            self.scene.set_pixel_scale(scale);
            self.relayout = true;
        }
        changed
    }

    /// Queue a native focus transition. Repeated notifications do not repeat the callback.
    /// Losing focus is also `keys_lost`.
    pub fn window_focus(&mut self, focused: bool) {
        // Every focus gain, even one a hide without blur left unreported.
        if focused { self.keys_lost = false; }
        if self.window_focused != Some(focused) {
            self.window_focused = Some(focused);
            if let Some(f) = self.on_focus { self.update(|s, tx| f(s, focused, tx)); }
            // Deliver the cancelled Up before FocusLost can commit a view that removes its
            // grip (or the focus callback clears the app's drag state).
            if !focused {
                self.cancel_pointer();
                self.held = None;
            }
            if self.commits == 0 || self.dirty || self.nodes.get(self.app_root()).is_some_and(|n| n.on_window_focus.is_some()) {
                self.queue.push(Action::WindowFocus(focused));
            }
            if !focused {
                self.keys_lost();
                // Losing the window ends hover, rather than waiting out an exit grace.
                self.hover_pending.clear();
                for path in std::mem::take(&mut self.hover_shown) {
                    self.queue.push(Action::HoverChange(path, false));
                }
            }
        }
        if !focused { self.input_click.clear(); self.wheel_lost(); }
    }

    /// The window gets no more keys: it lost focus or hid, which may never report a blur. The
    /// focused physical handler hears `FocusLost` and held modifier keys are released, once
    /// until the window has focus again. A hidden host steps `hidden_timer_frame` while
    /// `keys_queued` to deliver them.
    pub(crate) fn keys_lost(&mut self) {
        if std::mem::replace(&mut self.keys_lost, true) { return; }
        self.physical_input(crate::PhysicalInput::FocusLost);
    }

    /// Keyboard input, a focus change, a file drag step or a file dialog answer is queued for the
    /// next frame.
    pub(crate) fn keys_queued(&self) -> bool {
        self.queue.iter().any(|a| matches!(a, Action::Physical(..) | Action::Key { .. } | Action::KeyUp(_) | Action::WindowFocus(_) | Action::Files(_) | Action::FileDialog(..)))
    }

    /// The last committed frame is out of date: state changed (an update, poll or handler), or
    /// one asked for a scroll or text flight that waits for the next frame.
    pub(crate) fn needs_commit(&self) -> bool {
        self.dirty || self.pending.frame_work() || self.patch_pending()
    }

    /// The last frame no longer shows the window: state, its size or queued input changed since.
    pub(crate) fn stale(&self) -> bool {
        self.needs_commit() || self.relayout || !self.queue.is_empty()
    }

    /// A live-edit patch arrived that the next frame hasn't rebuilt the view with yet.
    pub(crate) fn patch_pending(&self) -> bool {
        #[cfg(feature = "hot")]
        return self.hot.as_ref().is_some_and(crate::hot::Session::pending);
        #[cfg(not(feature = "hot"))]
        false
    }

    /// Marks app state changed outside a handler (the next frame re-runs the view).
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    /// Changes the state from outside any handler, for data another thread delivered
    /// (`AppHandle::update`). Runs now, so it works while no window shows; the next frame commits
    /// it with the motion `f` asked for.
    pub fn update(&mut self, f: impl FnOnce(&mut S, &mut Tx)) {
        self.update_if(|s, tx| { f(s, tx); true });
    }

    /// `update` for a change that may be no change: `f` returns whether it changed state the
    /// view reads, as `poll` does. On false the view doesn't re-run and no frame is asked for,
    /// while what `f` asked of its `Tx` still happens: clipboard, close and quit, menus and
    /// window frames reach the shell, a scroll or text flight still takes a frame, and a
    /// `wake_after` waits, like `update`'s, for the next frame or hidden poll to schedule it
    /// (a host that runs neither calls `schedule_pending_wakes`). Returns whether the last frame
    /// went out of date: `f` reported a change or asked for work only a frame does.
    ///
    /// Returning false after changing state the view reads leaves the screen stale until the
    /// next change.
    pub fn update_if(&mut self, f: impl FnOnce(&mut S, &mut Tx) -> bool) -> bool {
        let mut one = self.transaction();
        let changed = crate::perf_warn::timed("an `update` closure", || f(&mut self.state, &mut one));
        let stale = changed || one.frame_work() || one.window_frame.is_some() || one.srgb_blend.is_some_and(|b| b != self.srgb_blend) || one.font_smoothing.is_some_and(|m| m != self.text.smoothing)
            || (one.blur && self.focused.is_some());
        self.latch_requests(&mut one);
        merge(&mut self.pending, one);
        self.dirty |= changed;
        stale
    }

    /// Events whose handler called [`Tx::live_only`] and so didn't re-run the view, since the
    /// runtime started.
    pub fn live_only_events(&self) -> u64 {
        self.live_only_skips
    }

    /// Whether debug builds check `Tx::live_only` handlers (on by default): a live-only event
    /// a quarter second or more after the last commit still commits, and Rune warns when the
    /// view built something else. A test counting commits turns it off. Release builds never
    /// check.
    pub fn check_live_only(&mut self, on: bool) {
        #[cfg(debug_assertions)]
        { self.live_only_check = on; }
        let _ = on;
    }

    /// Takes a `Tx::minimize` that `what` can't carry out, saying so in debug builds.
    pub(crate) fn ignore_minimize(&mut self, what: &str) {
        if std::mem::take(&mut self.minimize_request) && cfg!(debug_assertions) {
            eprintln!("rune: minimize ignored: {what} can't minimize");
        }
    }

    /// Latches a Tx's shell and clipboard requests for the shell, so `update`, a hidden poll
    /// and `on_close` reach the clipboard without a frame, and applies clipboard requests to a
    /// memory clipboard at once. A secret clear drops an earlier secret copy not yet written.
    fn latch_requests(&mut self, tx: &mut Tx) {
        self.run_pick_reads(tx);
        self.shell_request = self.shell_request.max(tx.shell.take());
        self.minimize_request |= std::mem::take(&mut tx.minimize);
        self.latch_playground(tx);
        if !tx.open_urls.is_empty() { self.open_urls(std::mem::take(&mut tx.open_urls)); }
        if std::mem::take(&mut tx.clear_secret) {
            if self.copy_request.as_ref().is_some_and(crate::ClipboardWrite::is_secret) { self.copy_request = None; }
            self.clear_secret_request = true;
            self.pill_copy = None;
        }
        if let Some(copy) = tx.copy.take() {
            self.pill_copy = (!copy.is_secret())
                .then(|| copy.content().html())
                .flatten()
                .filter(|h| h.contains("<span data-pill="))
                .map(|h| {
                    let frag = crate::TextEdit::from_html(h);
                    (
                        zeroize::Zeroizing::new(copy.content().text().to_owned()),
                        frag.pills().map(|(r, id)| (r, id.to_owned())).collect::<Vec<_>>(),
                    )
                })
                .filter(|(_, pills)| !pills.is_empty());
            self.copy_request = Some(copy);
        }
        if std::mem::take(&mut tx.paste) { self.paste_request = Some(self.paste_target()); }
        for read in tx.clipboard_reads.drain(..).filter_map(|read| read.take()) {
            self.next_clipboard_read += 1;
            self.clipboard_reads.push((self.next_clipboard_read, read));
            self.read_requests.push(self.next_clipboard_read);
        }
        self.popups.extend(tx.popups.drain(..).filter_map(|call| call.take()));
        for request in tx.file_dialogs.drain(..).filter_map(|call| call.take()) {
            self.request_file_dialog(request);
        }
        if let Some(mode) = tx.font_smoothing.take() { self.set_font_smoothing(mode); }
        if let Some(srgb) = tx.srgb_blend.take() { self.set_srgb_blend(srgb); }
        if std::mem::take(&mut tx.blur) {
            if self.focused.is_some() {
                self.set_focus(None, self.keyboard_focus);
                self.dirty = true;
            }
            // An explicit blur is not the temporary pointer-down blur of a scrim.
            self.last_focused = None;
        }
        if let Some(title) = tx.title.take() { self.window_title.set(title); }
        if let Some(mut request) = tx.window_frame.take() {
            request.spring = (request.animate && !tx.instant && !self.reduced_motion).then(|| tx.motion.unwrap_or(motion::DEFAULT));
            self.window_frame = Some(self.window_frame.unwrap_or_default().merge(request));
        }
        crate::clipboard::flush_memory(self);
    }

    /// Host time on the logical clock.
    fn wall_clock(&self) -> Option<f64> {
        self.wall_start.map(|t0| t0.elapsed().as_secs_f64())
    }

    /// Schedules the `wake_after`s that updates left for a frame, counting from now, when no
    /// frame or hidden poll comes to. The clock stays at the last poll, so the frame-interval
    /// floor on timers (`window::timer_deadline`) still counts from that poll.
    pub(crate) fn schedule_pending_wakes(&mut self) {
        let wakes = std::mem::take(&mut self.pending.wakes);
        self.schedule_wakes_from(self.now_clock(), wakes);
    }

    /// Sorted, de-duplicated deadlines on the logical clock.
    fn schedule_wakes(&mut self, wakes: impl IntoIterator<Item = std::time::Duration>) {
        self.schedule_wakes_from(self.clock, wakes);
    }

    fn schedule_wakes_from(&mut self, now: f64, wakes: impl IntoIterator<Item = std::time::Duration>) {
        for delay in wakes {
            let deadline = now + delay.as_secs_f64();
            if let Err(index) = self.wake_deadlines.binary_search_by(|t| t.total_cmp(&deadline)) {
                self.wake_deadlines.insert(index, deadline);
            }
        }
    }

    pub fn tracing(&self) -> bool { self.trace_channels.is_some() || self.scene.tracing() }

    fn transaction(&self) -> Tx {
        Tx { trace: self.tracing().then(crate::el::TraceCalls::default), mods: self.held_mods, selection: self.selection_snapshot(),
            input_time: crate::input_time::current(), ..Tx::default() }
    }

    /// A scalar from the actual draw path (e.g. `shell.width`). Values hold until resampled.
    /// Call before the tooling writer records the frame; scene traces sample in `frame`.
    pub fn trace_channel(&mut self, name: &'static str, kind: &'static str, value: f32) {
        if let Some(channels) = &mut self.trace_channels { channels.insert(name, (kind, value)); }
        if let Some(trace) = self.scene.trace_mut() { trace.channel(name, kind, value); }
    }

    /// Queues an app marker once for each attached writer. Formatting is skipped when off.
    pub fn trace_event(&mut self, event: impl std::fmt::Display) {
        if self.trace_channels.is_some() { event_log::push(&mut self.events, event.to_string()); }
        if let Some(trace) = self.scene.trace_mut() { trace.event(event); }
    }

    /// Next requested wake on the unscaled app clock. None lets the shell sleep indefinitely.
    pub fn wake_deadline(&self) -> Option<f64> {
        self.logical_wake_deadline()
    }

    /// The native window or overlay's frame counts; `None` headless.
    pub fn surface_stats(&self) -> Option<crate::SurfaceStats> {
        self.surface_stats.as_deref().map(crate::surface_stats::read)
    }

    /// When `poll` last ran, on `wake_deadline`'s clock.
    pub(crate) fn polled_at(&self) -> f64 {
        self.timer_wake.polled_at().unwrap_or(self.clock)
    }

    fn logical_wake_deadline(&self) -> Option<f64> {
        self.wake_deadlines.first().copied().into_iter().chain(self.loop_wake.map(|w| w.0)).min_by(f64::total_cmp)
    }

    /// Lands every track before the next frame: growth, reveals and follow-the-end (guarantee 8,
    /// for returning to a window or session).
    pub fn settle_all(&mut self) {
        self.settle = true;
        self.dirty = true;
    }

    /// Advances animation time by `dt` real seconds, runs queued handlers, re-runs the view only
    /// if state changed, steps `Scene`, growth, scroll and reveals, and builds this frame's
    /// instances.
    pub fn frame(&mut self, dt: f64) -> bool {
        self.frame_split(0.0, dt)
    }

    /// A hidden or windowless host's due timer.
    pub(crate) fn hidden_timer_frame(&mut self) {
        self.frame(0.0);
    }

    /// Builds what changed while a window that fits its content was hidden, before the host
    /// shows it, so the content's height (`window_frame`, a jump) is known before the window
    /// is ordered in and its first drawable taken. Lands every track, as showing does.
    /// Returns whether it built; the first frame then has nothing left to build.
    pub(crate) fn build_for_show(&mut self) -> bool {
        if !self.fit_height || self.commits == 0 || !self.stale() { return false; }
        self.settle_all();
        self.hidden_timer_frame();
        true
    }

    /// A hidden or covered host's due timer or wake. `poll` and due timers run now; the view
    /// and layout wait, dirty, for the frame that shows the host. Queued input and drag ends take a frame
    /// without drawing (`hidden_timer_frame`) so their handlers hear them now. Returns whether
    /// the last drawn frame went out of date.
    pub(crate) fn hidden_wake(&mut self) -> bool {
        if self.has_pending_input() || self.drag_ends_pending() {
            let commits = self.stats.commits;
            self.hidden_timer_frame();
            // File steps held for that frame's build land in another.
            if self.files_held() { self.hidden_timer_frame(); }
            return self.stats.commits != commits;
        }
        self.poll_hidden()
    }

    /// One frame in one pass that steps `catch_up + dt` seconds. Motion already running advances
    /// `catch_up` before this frame's handlers and commit, so they retarget it from its pose at
    /// the frame's time. Then everything, including what the commit started, steps `dt`. A late
    /// frame or a sleeping host's gap belongs to old tracks, never to targets first observed
    /// on this frame.
    pub(crate) fn frame_split(&mut self, catch_up: f64, dt: f64) -> bool {
        // Like a native shell: the logical clock follows host time (timers, poll), while
        // animations step by the frame's own dt, so idle time never lands on a fresh commit.
        let elapsed = catch_up + dt;
        // A coarse headless step still polls at its timer deadlines, independent of FPS, as an
        // idle window wakes for them. Like a window's timer floor (`window::timer_deadline`),
        // a poll comes at most once a frame interval and takes every deadline due by then, so
        // wakes that polls keep asking for collapse instead of each adding a poll per frame.
        // Input queued for the requested frame stays at that frame's timestamp. The catch-up
        // is the earliest part of the step.
        let target = self.clock + elapsed;
        if self.wall_start.is_none() && self.timer_step(target).is_some() {
            let input = std::mem::take(&mut self.queue);
            let mut catch_up = catch_up;
            while let Some(at) = self.timer_step(target) {
                let step = at - self.clock;
                let old = step.min(catch_up);
                catch_up -= old;
                self.frame_step(step, old, step - old);
            }
            self.queue.extend(input);
            let step = target - self.clock;
            let old = step.min(catch_up);
            return self.frame_step(step, old, step - old);
        }
        self.frame_step(elapsed, catch_up, dt)
    }

    /// When a headless step toward `target` stops early for a timer: the next deadline, but no
    /// sooner than a frame interval after the last poll. None when the step's own poll at
    /// `target` takes everything due.
    fn timer_step(&self, target: f64) -> Option<f64> {
        let deadline = self.logical_wake_deadline()?;
        let at = deadline.max(self.clock + crate::window::TIMER_FRAME.as_secs_f64());
        (at < target).then_some(at)
    }

    /// A user or explicit scroll ends the holds and reveals that would pull the view back.
    fn drop_scroll_holds(&mut self) {
        self.keeps.clear();
        self.scroll_reveals.clear();
    }

    /// Starts the text flights and launches handlers asked for this frame.
    fn capture_flights(&mut self, tx: &mut Tx) {
        for (source, target) in std::mem::take(&mut tx.text_flights) {
            self.capture_text_flight(source, target);
        }
        for (source, target) in std::mem::take(&mut tx.launches) {
            self.capture_launch(source, target);
        }
    }

    /// `elapsed` advances the logical clock when no wall clock drives it; `catch_up` steps
    /// running motion before the commit and `dt` steps all motion after it.
    fn frame_step(&mut self, elapsed: f64, catch_up: f64, dt: f64) -> bool {
        // Each queued action runs with its own input's arrival time; nothing else has one.
        let _input = crate::input_time::scope(None);
        let mut live_woke = self.take_live_wake();
        for v in self.virtuals.values_mut() { v.frame_begins(); }
        #[cfg(feature = "hot")]
        if self.hot.as_mut().is_some_and(crate::hot::Session::changed) {
            // Replace handler trait objects before queued input can call an old vtable.
            // A regular commit preserves keyed nodes, scrolls and spring velocities.
            self.memo.clear();
            self.commit(&Tx::default());
        }
        let mut tx = std::mem::take(&mut self.pending);
        let mut ran = false;
        let shaped0 = self.text.shaped;
        self.settled = false;
        self.clock = self.wall_clock().unwrap_or(self.clock + elapsed);
        if self.wall_start.is_none() {
            crate::video::headless_frame(self.clock);
            // Fake recorders publish this frame's level to bound meters, drawn this frame.
            crate::recorder::headless_frame(self.clock);
            live_woke |= self.take_live_wake();
        }
        if let Some((deadline, animation_time)) = self.loop_wake
            && !self.animating
        {
            // Native shells cap dt after idle. Advance the sleeping part of the
            // delay before new handlers retarget, without double-stepping manual time.
            let elapsed = animation_time - (deadline - self.clock).max(0.0) * self.time_scale as f64;
            self.t += (elapsed - self.t - (catch_up + dt) * self.time_scale as f64).max(0.0);
            self.scene.set_time(self.t);
            if self.clock >= deadline { self.loop_wake = None; }
        }
        // Bring old tracks, growth and scroll to this frame's time before a poll or update
        // retargets them. Presenting waits for `step_motion`, so the frame is sampled and
        // built once.
        if catch_up > 0.0 { self.catch_up_motion(catch_up); }
        self.keep_loops_on_clock(dt);
        let polled = self.timer_wake.frame_polled(self.wake_deadlines.first().is_some_and(|t| *t <= self.clock));
        let due = self.wake_deadlines.partition_point(|t| *t <= self.clock);
        self.idle_frame_begins(polled || due > 0, live_woke);
        self.wake_deadlines.drain(..due);
        self.fire_hover_delays();
        if let Some(poll) = self.poll.filter(|_| !polled) {
            let mut one = self.transaction();
            // Scheduling does not imply a model change.
            if self.idle_meter.polled(crate::perf_warn::timed("poll", || app_call!(poll, &mut self.state, self.clock, &mut one))) {
                ran = true;
            }
            merge(&mut tx, one);
        }
        if self.playground.as_ref().is_some_and(|p| p.polls()) {
            let mut one = self.transaction();
            let clock = self.clock;
            if self.playground.as_mut().is_some_and(|p| p.poll_stage(clock, &mut one)) {
                ran = true;
            }
            merge(&mut tx, one);
        }
        if self.commits == 0 && self.queue.iter().any(|a| matches!(a, Action::WindowFocus(_))) {
            self.commit(&Tx::default());
        }
        let mut ran_layout = false;
        self.report_visible_rows();
        // Hosts take a drag-out inside the move that asked for it; by now nothing will.
        self.drop_drag_out_request();
        let ended: Vec<_> = self.drag_ends.borrow_mut().drain(..).collect();
        let queue = order_drag_ends(std::mem::take(&mut self.queue), ended, &mut self.drags_open);
        let queue = self.hold_files(queue, ran || self.dirty);
        // Something besides a checked live-only event wants this frame's commit, so a change
        // it makes isn't the live-only handler's.
        #[cfg(debug_assertions)]
        let mut ran_other = ran;
        let mut pointer_hover = false;
        for Queued { action, at } in queue {
            let _input = crate::input_time::scope(at);
            pointer_hover |= matches!(action, Action::Hover(_));
            // State may have requested focus on a newly mounted field. Resolve the
            // new tree before routing this frame's text, not after it has gone to
            // the previous field. The same boundary covers an earlier handler, and lets a
            // file drag find only what is mounted and reachable now (a covered window builds
            // nothing until this).
            let routed = matches!(&action, Action::Key { .. } | Action::KeyUp(_) | Action::Physical(..) | Action::Ime(_) | Action::Paste(..) | Action::Files(_));
            // So is a playground an earlier handler opened or moved.
            if routed { self.latch_playground(&mut tx); }
            if (ran || self.dirty || !tx.scroll_to.is_empty()) && routed {
                if ran || self.dirty {
                    self.capture_flights(&mut tx);
                    if self.settle { tx.settle(); }
                    self.settled |= tx.settle;
                    #[cfg(debug_assertions)]
                    let check = self.live_check.take().filter(|_| !ran_other && !self.dirty && !ran_layout);
                    self.capture_keeps(&mut tx);
                    self.commit(&tx);
                    #[cfg(debug_assertions)]
                    { self.compare_live_check(check); ran_other = false; }
                    self.apply_keeps();
                    ran = false;
                }
                if !tx.scroll_to.is_empty() {
                    self.drop_scroll_holds();
                    for r in std::mem::take(&mut tx.scroll_to) {
                        self.reveal_row(&r, tx.instant);
                    }
                }
            }
            // A selection an earlier handler asked for is what this key copies.
            if matches!(action, Action::Key { .. } | Action::Physical(..)) && let Some(request) = tx.select.take() {
                let from_change = std::mem::take(&mut tx.select_from_change);
                self.apply_select_request(request, from_change);
            }
            if matches!(action, Action::Layout(..) | Action::ScreenLayout(..) | Action::DrawnLayout(_, _, false) | Action::VisibleRows(..)) {
                ran_layout |= self.run_action(action, &mut tx);
            } else {
                #[cfg(debug_assertions)]
                let checks = self.live_checks;
                let r = self.run_action(action, &mut tx);
                #[cfg(debug_assertions)]
                { ran_other |= r && checks == self.live_checks; }
                ran |= r;
            }
            self.run_pick_reads(&mut tx);
        }
        // Once a frame, however often a drag moved the selection, and in time for this commit.
        for action in self.selection_changes() { ran |= self.run_action(action, &mut tx); }
        let mut open_urls = std::mem::take(&mut tx.open_urls);
        self.latch_requests(&mut tx);
        // A handler asked for a menu: the hover it takes ends in this commit, not a frame later.
        if self.release_pointer_for_popup() {
            for queued in std::mem::take(&mut self.queue) {
                if matches!(queued.action, Action::HoverChange(..)) { ran |= self.run_action(queued.action, &mut tx); } else { self.queue.push_queued(queued); }
            }
            open_urls.extend(std::mem::take(&mut tx.open_urls));
            self.latch_requests(&mut tx);
        }
        self.release_files();
        self.schedule_wakes(tx.wakes.drain(..));
        self.capture_flights(&mut tx);
        if self.caret_watch && self.caret_moved(None) { self.dirty = true; }
        let read_moved = self.layout_reads_moved();
        self.reuse.read_moved = read_moved;
        if self.has_pixel_aspect && self.pixel_aspect_changed() { self.relayout = true; }
        let besides = self.dirty || self.relayout || self.settle || self.virtual_window_changed();
        let other = ran || besides;
        self.count_layout_echo(ran_layout || read_moved, other);
        if other || ran_layout || read_moved {
            if self.relayout && !ran && !self.dirty {
                if self.viewport_stepped { tx.window_step = true } else { tx.relayout = true }
            }
            if self.settle {
                tx.instant = true;
                tx.settle = true;
            }
            self.settled = tx.settle;
            self.state_unchanged = !ran && !self.dirty && !ran_layout && !self.settle && self.commits > 0;
            #[cfg(debug_assertions)]
            let check = self.live_check.take().filter(|_| !ran_other && !besides && !ran_layout && !read_moved);
            self.capture_keeps(&mut tx);
            self.commit(&tx);
            #[cfg(debug_assertions)]
            self.compare_live_check(check);
            self.apply_keeps();
        }
        #[cfg(debug_assertions)]
        { self.live_check = None; }
        tx.keep_on_screen.clear();
        let mut scroll_to = std::mem::take(&mut tx.scroll_to);
        for (path, v) in &mut self.virtuals {
            if let Some(key) = v.reveal_selected.take() && !pointer_hover {
                // Use the keyed ancestor path so identical row keys in other lists do not
                // intercept the reveal, including when this row is still virtual.
                let scope = path.split('/').filter(|seg| !seg.starts_with('#')).collect::<Vec<_>>().join("/");
                let key = if scope.is_empty() { key } else { format!("{scope}/{key}") };
                scroll_to.push(ScrollReveal::into_view(None, key));
            }
        }
        let mut scroll_bottom = std::mem::take(&mut tx.scroll_bottom);
        let scroll_pages = std::mem::take(&mut tx.scroll_pages);
        let mut scroll_offsets = std::mem::take(&mut tx.scroll_offsets);
        if tx.instant {
            for (_, animated) in &mut scroll_bottom { *animated = false; }
            for r in &mut scroll_offsets { r.animated = false; }
        }
        let follow = std::mem::take(&mut self.caret_follow);
        let destination = !scroll_bottom.is_empty() || !scroll_pages.is_empty() || !scroll_offsets.is_empty();
        if !scroll_to.is_empty() || destination || follow {
            self.drop_scroll_holds();
        }
        for r in &scroll_to { self.reveal_row(r, tx.instant); }
        if destination || follow {
            self.step_scroll(0.0);
        }
        if follow { self.follow_caret(); }
        if !scroll_bottom.is_empty() { self.scroll_to_bottom(scroll_bottom); }
        if !scroll_pages.is_empty() { self.scroll_pages(scroll_pages); }
        if !scroll_offsets.is_empty() { self.scroll_to_offsets(scroll_offsets); }
        // Explicit destinations processed after a reveal win for its entire lifetime.
        if destination { self.scroll_reveals.clear(); }
        // Those scrolls moved the caret after the commit a view placed from it saw.
        if self.caret_watch && self.caret_moved(None) { self.commit(&Tx { instant: true, ..Tx::default() }); }
        if let Some(request) = tx.select.take() {
            let from_change = std::mem::take(&mut tx.select_from_change);
            self.apply_select_request(request, from_change);
        }
        // What the commit or a `Tx::select_text` changed is heard next frame.
        let changes = self.selection_changes();
        self.queue.extend(changes);
        // Hit tests read last frame's drawn rects and clips by index. When a commit moved the
        // indices, the build re-evaluates hover from fresh geometry instead.
        if self.built || self.nodes.iter().map(|n| n.id).eq(self.built_ids.iter().copied()) {
            self.update_interaction(None);
        }
        #[cfg(test)]
        if let Some(publish) = tests::MID_FRAME.with(|hook| hook.borrow_mut().take()) { publish(); }
        let live_animating = self.latch_live(live_woke);
        if let Some(trace) = tx.trace.take() {
            for event in trace.events { self.trace_event(event); }
            for (name, kind, value) in trace.channels { self.trace_channel(name, kind, value); }
        }
        self.fit_held = std::mem::take(&mut self.fit_hold);
        let dt = if self.fit_held { 0.0 } else { dt };
        let animating = self.step_motion(dt) || live_animating;
        self.stats.shaped = self.text.shaped - shaped0;
        let filling = self.virtual_filling();
        if live_animating { self.anim_sources |= AnimSource::LIVE; }
        if self.time_scale <= 0.0 { self.anim_sources = 0; }
        if filling { self.anim_sources |= AnimSource::VIRTUAL_FILL; }
        self.animating = (animating && self.time_scale > 0.0) || filling;
        self.idle_frame_ends();
        if !open_urls.is_empty() { self.open_urls(open_urls); }
        self.animating
    }

    fn catch_up_motion(&mut self, dt: f64) {
        let step = dt * self.time_scale as f64;
        self.t += step;
        self.scene.set_time(self.t);
        self.caught_up += step as f32;
        self.step_growth(step as f32);
        self.step_scroll(step as f32);
    }

    fn step_motion(&mut self, dt: f64) -> bool {
        self.loops_stepped();
        let step = dt * self.time_scale as f64;
        self.slack_dt = std::mem::take(&mut self.caught_up) + step as f32;
        self.last_dt = step;
        self.t += step;
        self.scene.set_time(self.t);
        if self.scene.tracing() {
            for node in &self.nodes {
                self.scene.trace_name(node.id, node.trace_name);
                self.scene.trace_namespace(node.id, node.trace_namespace);
            }
        }
        for (id, track) in &self.loops {
            if let Some([x, y]) = track.offset(self.scene.store().node_time(*id), self.reduced_motion)
                && let Some(n) = self.node_index.get(id).map(|&i| &self.nodes[i])
            {
                let offset = rune_motion::Vec2::new(x * n.local.w, y * n.local.h);
                if self.scene.get(*id, prop::OFFSET) != Some(offset) {
                    self.scene.set(*id, prop::OFFSET, offset);
                }
            }
        }
        self.end_unseen_exits();
        let out = self.scene.frame();
        let mut animating = out.animating;
        let mut sources = if out.animating { AnimSource::TRACKS } else { 0 };
        if !out.exited.is_empty() {
            self.prune_exited();
        }
        self.land_group_fades();
        if !self.flying.is_empty() {
            let scene = &self.scene;
            let before = self.flying.len();
            // Screen travel ends once it lands and no ancestor still moves. An exiting flight
            // stays lifted until it's gone.
            self.flying.retain(|(id, _)| scene.store().has(*id, prop::TRAVEL)
                || matches!(scene.phase(*id), Some(PresencePhase::Exiting { .. })));
            if self.flying.len() != before {
                self.launches.keep(&self.flying);
                self.update_layer_paint_order();
            }
        }
        let mut source = |on: bool, bit: u16| { if on { sources |= bit; } on };
        animating |= source(self.step_growth(step as f32), AnimSource::GROWTH);
        animating |= source(self.step_scroll(step as f32), AnimSource::SCROLL);
        for stack in self.stacks.values_mut() { stack.grown = 0.0; }
        animating |= source(self.reveals_active(), AnimSource::REVEAL);
        // Values set from `on_scroll_offset` show in the frame that scrolled.
        if self.tell_scroll_offsets() {
            animating |= self.latch_live(self.take_live_wake());
            self.scene.present();
        }
        animating |= self.build();
        // Compensate drawn motion, leaving unrelated layout and growth springs alone.
        for _ in 0..3 {
            if !self.follow_keeps() { break; }
            animating |= self.build();
        }
        self.finish_keeps();
        self.report_at_bottom();
        animating |= self.report_visible_rows();
        self.report_layout();
        // A report waits for the next frame: draw one to deliver it.
        animating |= self.report_drawn();
        animating |= source(self.loops_active, AnimSource::LOOP);
        animating |= source(self.static_scale_settling || self.pixel_snap.easing(), AnimSource::SCALE_SETTLE);
        animating |= source(self.videos_active, AnimSource::VIDEO);
        animating |= source(self.draw_text_flights(), AnimSource::TEXT_FLIGHT);
        self.images.end_frame();
        if self.track_marks {
            self.marks.resize(self.instances.len(), 0);
        }
        self.anim_sources = sources;
        animating
    }

    /// A picture sizing a side from its image changed shape (decoded, or a stream resized).
    /// The next commit lays those pictures out at once, whatever else it animates.
    fn pixel_aspect_changed(&mut self) -> bool {
        for n in &mut self.nodes {
            if let (Some(ratio), Some(pixels)) = (&mut n.pixel_aspect, &n.vis.pixels) {
                let now = pixel_ratio(pixels);
                if now != *ratio { self.aspect_snap.push(n.id); }
                *ratio = now;
            }
        }
        !self.aspect_snap.is_empty()
    }

    /// Subtree (preorder) paint order, then each flying element's subtree on top. A sticky
    /// element paints after its normal siblings (and before overlay siblings), so it covers the
    /// rows it's stuck over.
    fn update_layer_paint_order(&mut self) {
        let n = self.nodes.len();
        let mut first = vec![n; n + 1];
        let mut next = vec![n; n];
        let rank = |i: usize| {
            let nd = &self.nodes[i];
            if nd.above.is_some() || nd.modal.is_some() { 2 }
            else if self.sticky_of(i).is_some() { 1 }
            else { 0 }
        };
        if self.has_sticky {
            for r in (0..=2).rev() {
                for i in (0..n).rev().filter(|&i| rank(i) == r) {
                    let parent = self.nodes[i].parent.map_or(0, |p| p + 1);
                    next[i] = first[parent]; first[parent] = i;
                }
            }
        } else {
            for i in (0..n).rev() {
                let parent = self.nodes[i].parent.map_or(0, |p| p + 1);
                next[i] = first[parent]; first[parent] = i;
            }
        }
        let mut lifted: Vec<usize> = self.flying.iter().filter_map(|(id, _)| self.node_index.get(id).copied()).collect();
        if self.has_raise { lifted.extend((0..n).filter(|&i| self.nodes[i].vis.raise)); }
        lifted.sort_unstable();
        lifted.dedup();
        self.layer_paint_order.clear();
        for top in std::iter::once(None).chain(lifted.iter().map(|&r| Some(r))) {
            let mut i = top.unwrap_or(first[0]);
            while i < n {
                // A flying subtree paints in its own pass, not where the tree reaches it.
                if top == Some(i) || lifted.binary_search(&i).is_err() {
                    self.layer_paint_order.push(i);
                    if first[i + 1] < n { i = first[i + 1]; continue; }
                }
                loop {
                    if top == Some(i) { i = n; break; }
                    if next[i] < n { i = next[i]; break; }
                    match self.nodes[i].parent { Some(parent) => i = parent, None => { i = n; break; } }
                }
            }
        }
        if self.layer_paint_order.iter().copied().eq(0..n) { self.layer_paint_order.clear(); }
    }

    /// An event whose handler called `Tx::live_only`: false, so it doesn't commit, while this
    /// frame still latches the live values it set. Debug builds check one such event a quarter
    /// second after the last commit by committing anyway (true) and comparing what it built.
    fn skip_view(&mut self, label: impl std::fmt::Display) -> bool {
        #[cfg(debug_assertions)]
        if self.live_only_check && self.clock - self.last_commit_clock >= LIVE_ONLY_CHECK {
            if self.live_check.is_none() { self.live_check = Some((label.to_string(), self.view_fingerprint())); }
            self.live_checks += 1;
            self.live_dirty = true;
            return true;
        }
        let _ = label;
        self.live_only_skips += 1;
        self.live_dirty = true;
        false
    }

    /// What the last commit built: each element's path, layout, colours and text. Live values
    /// are left out, since a commit rebuilds them from the view and the next latch sets them.
    #[cfg(debug_assertions)]
    fn view_fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::hash::DefaultHasher::new();
        let floats = |h: &mut std::hash::DefaultHasher, xs: &[f32]| for x in xs { x.to_bits().hash(h) };
        for n in &self.nodes {
            // Retained exits can disappear between commits without a model change.
            if self.is_exiting(n.id) { continue; }
            n.path.hash(&mut h);
            floats(&mut h, &n.target);
            let v = &n.vis;
            if !n.live.iter().any(|b| b.property == crate::live::Property::Fill) {
                floats(&mut h, &[v.fill.r, v.fill.g, v.fill.b, v.fill.a]);
            }
            floats(&mut h, &[v.border.r, v.border.g, v.border.b, v.border.a, v.border_width, v.opacity]);
            floats(&mut h, &v.radius);
            if let Some(t) = &n.text {
                if t.live.is_none() { t.content.hash(&mut h); }
                floats(&mut h, &[t.color.r, t.color.g, t.color.b, t.color.a, t.style.size, t.style.weight]);
                for p in &t.pills {
                    (p.start, p.end, p.icon.map(|i| i.name)).hash(&mut h);
                    for c in [p.bg, p.fg].into_iter().flatten() {
                        floats(&mut h, &[c.r, c.g, c.b, c.a]);
                    }
                }
                for &(a, b, c, w) in &t.rings {
                    (a, b).hash(&mut h);
                    floats(&mut h, &[c.r, c.g, c.b, c.a, w]);
                }
                for &(a, b, c) in &t.strikes {
                    (a, b, c).hash(&mut h);
                }
                for hl in &t.highlights {
                    (hl.start, hl.end, hl.bg, hl.fg, hl.pad.to_bits(), &hl.key).hash(&mut h);
                    if let Some((rc, rw)) = hl.ring {
                        (rc, rw.to_bits()).hash(&mut h);
                    }
                }
            }
        }
        h.finish()
    }

    /// After the commit a live-only check forced: warn when the view built something else, as
    /// it does when the handler changed state the view reads.
    #[cfg(debug_assertions)]
    fn compare_live_check(&mut self, check: Option<(String, u64)>) {
        let Some((handler, before)) = check else { return };
        if self.view_fingerprint() == before { return; }
        let warning = format!(
            "rune: {handler} called Tx::live_only, but the view built something else after it. The handler may have changed \
             state the view reads, which stays stale until another event commits. Set only Live values in a \
             live-only handler; for a time-dependent view, disable this heuristic with check_live_only(false)."
        );
        if !self.live_only_warnings.contains(&warning) {
            eprintln!("{warning}");
            self.live_only_warnings.push(warning);
        }
    }

    /// Offer a logical key that already passed `held_gate` to the app, the focused
    /// input's handler, focus traversal and the focused editor, in that order.
    fn route_key(&mut self, ev: &KeyEvent, repeat: bool, one: &mut Tx) -> bool {
        let physical_key = one.physical_key;
        one.focused = self.focused_key();
        if matches!(ev.key, crate::Key::Modifier { .. }) { return !repeat && self.route_modifier(ev, one); }
        one.key_repeat = repeat;
        let shown = self.keyboard_focus || self.hover_focus;
        self.keyboard_focus |= focus::navigates(ev);
        // With no focused element, app-owned navigation (e.g. a toolbar)
        // gets first refusal before Rune starts a new Tab traversal.
        let app_first = self.focused.is_none();
        // Return and Escape that reach a field mid-composition belong to the input method: they
        // confirm or cancel it, and submit, blur and press nothing.
        if matches!(ev.key, crate::Key::Enter | crate::Key::Escape) && self.composing() { return true; }
        if self.element_key(ev, one) { return true; }
        one.physical_key = None;
        if self.default_button_key(ev, one) { return true; }
        one.physical_key = physical_key;
        if self.playground_nav(ev, one) { return true; }
        if app_first && !self.playground_typing() && let Some(f) = self.on_key && Tx::took(self.app_key(f, ev, one), one) { return true; }
        if self.focused_input().is_some() {
            self.input_click.clear_last();
        }
        if let Some(f) = self.focused_input().and_then(|input| input.key.clone())
            && Tx::took(f(states!(self), ev, one), one)
        {
            return true;
        }
        one.physical_key = None;
        // Inside a list item or code block, Tab (and Shift+Tab in lists) edits the document by default.
        if ev.key == crate::edit::Key::Tab
            && !ev.mods.cmd
            && !ev.mods.alt
            && !ev.mods.ctrl
            && self.focused_input().is_some_and(|i| {
                !i.single_line
                    && !i.edit.is_secure()
                    && (i.edit.selection_in_list() || (!ev.mods.shift && i.edit.active_block() == crate::edit::Block::Code))
            })
        {
            let layout = self.focused_layout();
            if self.with_focused(|e| e.key(ev, layout.as_deref()), one) {
                return true;
            }
        }
        if self.focus_key(ev, shown, one) { return true; }
        let input_handles = match &ev.key {
            crate::edit::Key::Function(_) | crate::edit::Key::ContextMenu | crate::edit::Key::Tab | crate::edit::Key::PageUp | crate::edit::Key::PageDown => false,
            crate::edit::Key::Command(c) => {
                matches!(c, 'a' | 'c' | 'x' | 'z')
                    || (matches!(c, 'b' | 'i' | 'u') && self.focused_input().is_some_and(|i| i.edit.format_chord(ev).is_some()))
            }
            _ => !ev.is_command_chord() || ev.control_binding().is_some(),
        };
        if input_handles {
            let layout = self.focused_layout();
            // A rich field copies its selection's formatting too, read before a cut removes it.
            let mut html = None;
            let copies = matches!(ev.key, crate::edit::Key::Command('c' | 'x'));
            let handled = self.with_focused(|e| {
                if copies && e.is_rich() && !e.is_secure() {
                    html = Some(e.selected_html()).filter(|h| !h.is_empty());
                }
                e.key(ev, layout.as_deref())
            }, one);
            if let Some(html) = html
                && let Some(text) = one.copy.as_ref().filter(|w| !w.is_secret()).map(|w| w.content().text().to_owned())
            {
                one.copy_rich(crate::ClipboardContent::new(text).with_html(html));
            }
            if handled { return true; }
        }
        one.physical_key = physical_key;
        if !app_first && !self.playground_typing() && let Some(f) = self.on_key && Tx::took(self.app_key(f, ev, one), one) {
            log_event!(self, "key {:?}", ev.key);
            return true;
        }
        one.physical_key = None;
        // Selected text takes the Cmd+A and Cmd+C nothing else took, as AppKit's responder chain
        // gives a key equivalent to the menu bar before the text view: an app's shortcut wins.
        if !repeat && self.selection_key(ev, one) { return true; }
        if !repeat && self.context_menu_key(ev, one) { return true; }
        self.page_scroll(ev) || self.playground_key(ev, one)
    }

    /// `AppSpec::on_key`, given the stage's state while the playground shows one of the app's.
    /// The playground's list and side column have already taken the keys they use.
    fn app_key(&mut self, f: fn(&mut S, &KeyEvent, &mut Tx) -> bool, ev: &KeyEvent, one: &mut Tx) -> bool {
        let state = match &mut self.playground {
            Some(playground) => playground.key_state(&mut self.state),
            None => Some(&mut self.state),
        };
        state.is_some_and(|state| app_call!(f, state, ev, one))
    }

    /// Page Up and Page Down scroll by most of a viewport, and Home and End to the top and
    /// bottom, as an NSScrollView does, when nothing else took the key. The scroll view is the
    /// nearest one around the focused element, else around the pointer, else the first in the
    /// tree, skipping any whose content fits (nothing to scroll): a chat's short composer passes
    /// the keys to the message list beside it. A view that overflows keeps them even at the edge
    /// a key moves toward, where it does nothing, as an NSTextView does; an app that wants
    /// another view paged there routes the key itself with `Tx::scroll_page`.
    fn page_scroll(&mut self, ev: &KeyEvent) -> bool {
        use crate::Key::*;
        if !matches!(ev.key, PageUp | PageDown | Home | End) || ev.mods.cmd || ev.mods.alt || ev.mods.ctrl { return false; }
        // A focused field in a virtual row that scrolled out has no node: page its list.
        let focused = self.focused.as_deref().and_then(|path| {
            std::iter::successors(Some(path), |p| p.rsplit_once('/').map(|(head, _)| head))
                .find_map(|p| self.nodes.iter().position(|n| n.path == p))
        });
        let around = |from: usize| std::iter::successors(Some(from), |&i| self.nodes[i].parent).find(|&i| self.pageable(i));
        let i = focused.and_then(around)
            .or_else(|| self.pointer.and_then(|(x, y)| self.scroll_at(x, y)).and_then(|id| self.node_index.get(&id).copied()).and_then(around))
            .or_else(|| (0..self.nodes.len()).find(|&i| self.pageable(i)));
        let Some(i) = i else { return false };
        if !self.page_to(i, |offset, max, page| match ev.key { PageUp => offset - page, PageDown => offset + page, Home => 0.0, _ => max }) { return false; }
        log_event!(self, "page {:?}", ev.key);
        true
    }

    /// Whether node `i` is a live scroll view whose content overflows it.
    fn pageable(&self, i: usize) -> bool {
        self.nodes[i].scroll.is_some() && !self.subtree_exiting(i) && self.allowed(i)
            && self.scrolls.get(&self.nodes[i].id).is_some_and(|st| st.max > 0.5)
    }

    /// Moves scroll view `i` to `to(offset, max, page)`, clamped, a page being 90% of its
    /// height. The hand moved it: any fling or glide stops, and a `stick_bottom` view that
    /// lands on its end follows it again.
    fn page_to(&mut self, i: usize, to: impl FnOnce(f64, f64, f64) -> f64) -> bool {
        self.release_hover_hold();
        self.drop_scroll_holds();
        let (id, page, stick) = (self.nodes[i].id, f64::from(self.nodes[i].local.h * 0.9), self.nodes[i].scroll.is_some_and(|s| s.stick_bottom));
        let Some(st) = self.scrolls.get_mut(&id) else { return false };
        st.offset = to(st.offset, st.max, page).clamp(0.0, st.max);
        st.v = 0.0;
        st.glide = None;
        st.direct = true;
        st.pinned = stick && st.offset >= st.max - 0.5;
        true
    }

    fn focused_layout(&mut self) -> Option<Rc<Layout<Ink>>> {
        let path = self.focused.clone()?;
        let s = self.env.scale;
        let n = self.node(&path)?;
        let (mut spec, w) = (n.text.clone()?, n.target[2] - n.target[0] - n.inset[0] - n.inset[2]);
        spec.replace_content(n.input.as_ref()?.edit.display().0);
        Some(self.text.layout_for_spec(&spec, s, spec.wrap.then_some(w * s), line_alignment(&spec)))
    }

    fn with_focused(&mut self, f: impl FnOnce(&mut crate::edit::TextEdit) -> EditOutcome, tx: &mut Tx) -> bool {
        self.with_focused_state(|state, access, _| f(access(state)), tx)
    }

    /// [`Self::with_focused`] for an edit that needs the app state (an app's paste hook).
    fn with_focused_state(&mut self, f: impl FnOnce(crate::states::States<'_>, &crate::el::EditFn, &mut Tx) -> EditOutcome, tx: &mut Tx) -> bool {
        let Some(path) = self.focused.clone() else { return false };
        let Some((access, submit, change, animate_layout, single_line, secure, revealed_override)) =
            self.focused_input().map(|i| (i.access.clone(), i.submit.clone(), i.change.clone(), i.animate_layout, i.single_line, i.edit.is_secure(), i.revealed_override))
        else {
            return false;
        };
        let Some(access) = access else { return false };
        self.input_click.clear_last();
        {
            let edit = access(states!(self));
            edit.set_single_line(single_line);
            if secure && !edit.is_secure() { edit.set_secure(true); }
            if let Some(rev) = revealed_override { edit.set_revealed(rev); }
        }
        let before = change.as_ref().map(|_| zeroize::Zeroizing::new(access(states!(self)).text.clone()));
        let mut out = f(states!(self), &access, tx);
        // Submit and Escape's blur act once per press; held keys still type, move and delete.
        if tx.key_repeat { out.submit = false; out.blur = false; }
        // Typing is the user's own hand: no animation.
        tx.instant = !animate_layout;
        self.caret_follow = true;
        if let Some(change) = change
            && before.as_ref().map(|s| s.as_str()) != Some(access(states!(self)).text.as_str())
        {
            change(states!(self), tx);
            access(states!(self)).mark_edited();
        }
        if let Some(c) = out.copy {
            tx.copy(c);
        }
        if out.blur {
            self.set_focus(None, self.keyboard_focus);
        }
        if out.submit
            && let Some(sub) = submit
        {
            log_event!(self, "submit {path}");
            tx.instant = false;
            sub(states!(self), tx);
        } else if out.submit {
            // A field with nothing to submit passes Return to the default button, as in a sheet.
            self.press_default_button(tx);
        }
        true
    }

    /// Whether the focused field has an input method composition (preedit) open.
    fn composing(&mut self) -> bool {
        let Some(access) = self.focused_input().map(|i| i.access.clone()) else { return self.app_ime.composing() };
        match access {
            Some(access) => access(states!(self)).preedit.is_some(),
            None => self.focused_input().is_some_and(|i| i.edit.preedit.is_some()),
        }
    }

    fn node(&self, path: &str) -> Option<&Node> {
        self.nodes.iter().enumerate().find(|(i, n)| n.path == path && !self.subtree_exiting(*i) && !self.subtree_unmounted(*i)).map(|(_, n)| n)
    }

    /// `El::on_key` handlers on the focused element and its ancestors, innermost first. A
    /// focused text field's keys stay its own.
    fn element_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let Some(i) = self.focused_index() else { return false };
        if !self.can_focus(i) || self.focused_input().is_some() { return false; }
        let handlers: Vec<_> = std::iter::successors(Some(i), |&j| self.nodes[j].parent)
            .take_while(|&j| self.allowed(j) && !self.subtree_exiting(j))
            .filter_map(|j| self.nodes[j].focus.key.clone())
            .collect();
        handlers.into_iter().any(|f| Tx::took(f(states!(self), ev, tx), tx))
    }

    fn focused_input(&self) -> Option<&InputSpec> {
        let path = self.focused.as_deref()?;
        self.node(path).and_then(|n| n.input.as_deref()).or_else(|| self.virtual_inputs.get(path).map(Box::as_ref))
    }

    /// What [`Tx::focused`] reports: the focused link's id, else the focused element's key,
    /// else its path's last segment (a field in a virtual row that scrolled out has no node).
    fn focused_key(&self) -> Option<String> {
        let path = self.focused.as_deref()?;
        if let Some(id) = self.focused_index().and_then(|i| self.focused_link_id(i)) { return Some(id); }
        Some(self.focused_index().and_then(|i| self.nodes[i].key.clone())
            .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path).to_owned()))
    }

    /// The focused element's index. Unlike [`Runtime::node`], this finds it while it exits.
    fn focused_index(&self) -> Option<usize> {
        let path = self.focused.as_deref()?;
        self.nodes.iter().position(|n| n.path == path)
    }

    fn set_or_anim<T: rune_motion::Animatable>(&mut self, id: NodeId, p: Prop<T>, val: T, s: Spring, instant: bool) {
        if instant {
            self.scene.set(id, p, val);
        } else {
            self.scene.animate(id, p, val, s);
        }
    }

    fn color(&self, id: NodeId, prop: Prop<Color>, rest: Color) -> Color {
        if self.scene.store().is_animating(id, prop) { self.scene.get(id, prop).unwrap_or(rest) } else { rest }
    }

    /// Shadow `k` of node `id` with its geometry where its spring has it.
    fn shadow_at(&self, id: NodeId, k: usize, sh: &crate::el::Shadow) -> crate::el::Shadow {
        match P_SHADOW_GEO.get(k) {
            Some(&prop) if self.scene.store().active_len() > 0 && self.scene.store().is_animating(id, prop) => match self.scene.get(id, prop) {
                Some(g) => crate::el::Shadow { x: g.x, y: g.y, blur: g.w.max(0.0), spread: g.h, ..*sh },
                None => *sh,
            },
            _ => *sh,
        }
    }
}

// -------------------------------------------------------------------------------------------
// Commit: view → nodes → layout → Scene retarget

/// Measurement capabilities: text/layout caches and virtual geometry only. In particular this
/// type has no Scene, growth tracks, reveal tracks, draw instances, or mutable app state.
struct Measurement<'a, S: 'static> {
    state: &'a S,
    view: fn(&S, &mut Cx<S>) -> El,
    playground: Option<&'a crate::playground::Live<S>>,
    env: Env,
    viewport: (f32, f32),
    fit_height: bool,
    system_reduced_motion: bool,
    titlebar_inset: f32,
    system_accent: crate::SystemAccent,
    reduced_motion: bool,
    time_scale: f32,
    commits: u64,
    /// The last commit's nodes. A pass that commits drops the text layouts of the ones it keeps.
    nodes: &'a mut [Node],
    committing: bool,
    spare_nodes: Vec<Node>,
    text: &'a mut TextSystem,
    layout: &'a mut TaffyTree<usize>,
    layout_entries: &'a mut HashMap<String, LayoutEntry>,
    node_index: &'a mut HashMap<NodeId, usize>,
    path_ids: &'a mut HashMap<String, NodeId>,
    memo: &'a mut crate::memo::MemoCache,
    next_id: &'a mut u64,
    scrolls: &'a mut HashMap<NodeId, ScrollState>,
    virtuals: &'a mut HashMap<String, crate::virtual_list::VirtualList>,
    virtual_culled: &'a mut HashSet<String>,
    virtual_inputs: &'a mut HashMap<String, Box<InputSpec>>,
    stats: &'a mut FrameStats,
    key_warnings: &'a mut Option<Vec<String>>,
    /// The focused element's path, so an index-built row holding it is still built.
    focused: Option<&'a str>,
    /// True when `self.state` was not mutated since the previous commit.
    state_unchanged: bool,
    /// Whether this pass measures again because estimates left part of a viewport blank.
    retry: bool,
    caret: Option<(String, [f32; 4])>,
    caret_read: bool,
    layout_seen: layout_read::LayoutRects,
    layout_reads: layout_read::LayoutReads,
}

/// Owned result of completed measurement. Scene mutation consumes this once; retries never
/// return it. Keeping this separate from El makes unmeasured views invalid commit inputs.
struct Measured {
    layout: TaffyTree<usize>,
    nodes: Vec<Node>,
    reduced_motion: bool,
    time_scale: f32,
    caret_read: bool,
    layout_reads: layout_read::LayoutReads,
    memo_flattened: Option<Vec<memo_reuse::PendingFlat>>,
    memo_reused: Vec<memo_reuse::ReusedSpan>,
}

impl<S: 'static> Runtime<S> {
    fn commit(&mut self, tx: &Tx) {
        let _commit_time = crate::frame_profile::Scope::new(crate::frame_profile::Phase::Commit);
        let _first_view = crate::perf_warn::launch::FirstView::start(self.commits == 0);
        self.apply_playground();
        COMMITS.set(COMMITS.get() + 1);
        self.last_commit_clock = self.clock;
        self.memo_since = self.memo.next_pass();
        for r in &self.scroll_reveals {
            if r.at_end && let Some(&id) = self.path_ids.get(&r.viewport)
                && let Some(scroll) = self.scrolls.get_mut(&id)
            {
                scroll.pinned = true;
            }
        }
        let mut measured = self.measure_commit(tx);
        self.focus_before_commit = self.focused.clone();
        // Settle layout reads first so a field that mounts once a rect is known is in `nodes`.
        measured = self.settle_layout_reads(measured, tx);
        if self.sync_requested_focus(&mut measured.nodes) {
            self.layout = measured.layout;
            measured = self.measure(tx);
            measured = self.settle_layout_reads(measured, tx);
        }
        if measured.caret_read && self.caret_moved(Some(&measured.nodes)) {
            self.layout = measured.layout;
            measured = self.measure(tx);
            measured = self.settle_layout_reads(measured, tx);
        }
        self.apply(measured, tx);
        self.scene.keep_flights(false);
        self.memo.keep_since(self.memo_since);
        self.track_focused_rows();
        self.prune_text_selection();
    }

    // ---------------------------------------------------------------------------------------
    // Growth, scroll and reveals: render-side tracks.

    fn step_growth(&mut self, dt: f32) -> bool {
        if self.stacks.is_empty() {
            return false;
        }
        self.grow_acc += dt;
        let steps = (self.grow_acc / SUBSTEP).floor() as usize;
        self.grow_acc -= steps as f32 * SUBSTEP;
        let now = self.t;
        let mut moving = false;
        for (cid, stack) in self.stacks.iter_mut() {
            let Some(&ci) = self.node_index.get(cid) else { continue };
            let Some(spec) = self.nodes[ci].growth.as_deref().copied() else { continue };
            let n_rows = stack.order.len();
            let mut st_growing = !stack.pending_heights.is_empty();
            let mut st_moving = st_growing;
            for (k, id) in stack.order.iter().enumerate() {
                let Some(g) = self.grows.get_mut(id) else { continue };
                let h0 = g.h;
                if let Some(&at) = stack.closing.get(id)
                    && now >= at
                {
                    if self.reduced_motion {
                        *g = GrowTrack::settled(0.0);
                    } else {
                        g.retarget(0.0);
                    }
                }
                // A row above the tail (the last row) carries it.
                let carrying = k + 2 == n_rows;
                for substep in 0..steps.min(240) {
                    if !stack.pending_heights.is_empty() {
                        let at = now - f64::from(self.grow_acc) - f64::from((steps.min(240) - substep) as f32 * SUBSTEP);
                        if at >= stack.width_hold_until && let Some(height) = stack.pending_heights.remove(id) {
                            g.retarget(height);
                        }
                    }
                    g.step(spec.spring, &spec.edge, carrying);
                }
                stack.grown += (g.h - h0).max(0.0);
                st_growing |= g.target > g.h + 1.0 / 128.0;
                st_moving |= g.moving() || (g.h - h0).abs() > 1e-5;
            }
            stack.order.retain(|id| {
                let keep = !stack.closing.contains_key(id) || self.grows.get(id).is_some_and(|g| g.h > 0.01 || g.target > 0.0);
                if !keep {
                    self.grows.remove(id);
                }
                keep
            });
            stack.closing.retain(|id, _| stack.order.contains(id));
            stack.growing = st_growing;
            stack.moving = st_moving;
            // Keep the shared clock awake through the hold. No timer thread or remeasurement.
            moving |= st_moving;
        }
        moving
    }

    /// Presented content extent of node `c` if it is a growth stack (points).
    fn stack_extent(&self, c: usize) -> Option<f64> {
        let n = &self.nodes[c];
        n.growth.as_ref()?;
        let stack = self.stacks.get(&n.id)?;
        let mut count = 0usize;
        let rows: f64 = stack.order.iter().filter_map(|id| self.grows.get(id)).map(|g| { count += 1; f64::from(g.h) }).sum();
        let gaps = f64::from(stack.gap) * count.saturating_sub(1) as f64;
        Some(f64::from(stack.pad_top) + rows + gaps + f64::from(stack.pad_bottom))
    }

    /// How far scroll viewport `i`'s content reaches past its bottom, negative while it fits.
    fn scroll_overflow(&self, i: usize) -> f64 {
        let view = self.nodes[i].target;
        let content_h = self.scroll_children(i).map(|c| {
            let n = &self.nodes[c];
            f64::from(n.target[1] - view[1]) + self.stack_extent(c).unwrap_or(f64::from(n.target[3] - n.target[1]))
        }).reduce(f64::max).unwrap_or(0.0);
        content_h - f64::from(view[3] - view[1])
    }

    fn step_scroll(&mut self, dt: f32) -> bool {
        let slack_dt = std::mem::take(&mut self.slack_dt);
        let mut moving = false;
        for i in 0..self.nodes.len() {
            let Some(sp) = self.nodes[i].scroll else { continue };
            if self.subtree_exiting(i) { continue; }
            let id = self.nodes[i].id;
            let axes = self.nodes[i].scroll_axes;
            if axes.x() { moving |= self.step_scroll_x(i, dt); }
            // A horizontal viewport doesn't move vertically, even with content taller than it.
            let overflow = if axes.y() { self.scroll_overflow(i) } else { 0.0 };
            #[cfg(target_arch = "wasm32")]
            if let Some(active) = self.web_step_scroll(id, overflow, dt, false) {
                moving |= active;
                continue;
            }
            let held = (!self.keeps.is_empty() || !self.scroll_reveals.is_empty()) && {
                let path = &self.nodes[i].path;
                (self.keeps.iter().any(|k| k.viewport == *path) || self.scroll_reveals.iter().any(|r| r.viewport == *path))
                    && self.scroll_stack(i).and_then(|c| self.stacks.get(&self.nodes[c].id)).is_some_and(|s| s.growing)
            };
            let laid = if axes.y() { self.ease_to_shrunk_end(i, overflow) } else { None };
            let Some(st) = self.scrolls.get_mut(&id) else { continue };
            st.scrolling = st.direct || st.v.abs() > 1.0 || st.x.v.abs() > 1.0;
            st.direct = false;
            st.max = overflow.max(0.0);
            if st.pinned && sp.stick_bottom {
                // A reader stuck on a shrinking stack's end doesn't rise with its rows' last fraction.
                st.offset = laid.filter(|&end| st.offset <= end + 1e-6).map_or(st.max, |end| st.max.min(end));
                st.v = 0.0;
                st.glide = None;
            } else if st.glide.is_some() {
                moving |= scroll_bottom::glide(st, dt);
            } else if st.v.abs() > 1.0 {
                // Exact for exponential decay, so a long frame carries the same distance as
                // the short frames it stands for.
                let decay = (-SCROLL_DECAY * dt).exp();
                st.offset += f64::from(st.v * (1.0 - decay) / SCROLL_DECAY);
                st.v *= decay;
                if st.offset <= 0.0 || st.offset >= st.max {
                    st.v = 0.0;
                }
                moving = true;
            } else {
                st.v = 0.0;
            }
            st.offset = st.offset.clamp(0.0, st.max);
            // Landed on a shrinking stack's new end: stick once its rows are within half a point
            // of it, not while they're still short of it.
            let shrinking = laid.is_some_and(|end| end < st.max - 0.5 && st.offset < st.max - 1e-3);
            if sp.stick_bottom && !held && !shrinking && st.offset >= st.max - 0.5 && st.v.abs() <= 1.0 {
                st.pinned = true;
            }
            st.laid = laid;
            if sp.stick_bottom && axes.y() && slack_dt > 0.0 { moving |= self.step_slack(i, slack_dt); }
        }
        moving
    }

    fn reveals_active(&mut self) -> bool {
        let store = self.scene.store();
        let mut any = false;
        for (id, st) in self.reveals.iter_mut() {
            let now = store.node_time(*id);
            let fade = st.fade as f64;
            // Words fully shown leave the list (only from the front, which keeps lookups valid).
            let done = st.words.iter().take_while(|w| now - w.1 >= fade).count();
            if done > 0 && done == st.words.len() {
                st.words.clear();
            }
            any |= !st.words.is_empty() && store.node_time_scale(*id) > 0.0;
        }
        any
    }

    fn reveal_alpha(st: &RevealState, byte: u32, now: f64) -> f32 {
        let i = st.words.partition_point(|w| w.0 <= byte);
        if i == 0 {
            return 1.0;
        }
        let t0 = st.words[i - 1].1;
        if st.fade <= 0.0 {
            return if now >= t0 { 1.0 } else { 0.0 };
        }
        ease_out(((now - t0) / st.fade as f64) as f32)
    }

    // ---------------------------------------------------------------------------------------
    // Input: queued, handled at the next frame.

    pub fn pointer_move(&mut self, x: f32, y: f32) { self.move_pointer(x, y, true); }

    /// `drag_move` false keeps a held drag from getting a `Phase::Move`: a release at the
    /// point the drag last saw has nothing new to report, and its instant transaction would
    /// cut short whatever the press started (a tapped slider's knob mid-spring).
    fn move_pointer(&mut self, x: f32, y: f32, drag_move: bool) {
        if self.pointer != Some((x, y)) { self.hover_paused = false; self.release_hover_hold(); }
        self.pointer = Some((x, y));
        if self.in_viewport(x, y) { self.left_in_capture = false; }
        for &(id, rect) in &self.hits {
            if self.hit_contains(id, rect, x, y)
                && let Some(&i) = self.node_index.get(&id)
                && self.nodes[i].on_pointer_move.is_some()
            {
                let point = self.pointer_event(i, x, y);
                self.queue.push(Action::PointerMove(self.nodes[i].path.clone(), point));
            }
        }
        self.refresh_hover(x, y);
        let Some((path, d)) = self.capture.clone() else { return };
        if self.drag_out_armed && crate::drag_out::past_threshold(self.press_at, (x, y), self.drag_out_threshold) && self.start_drag_out(path.clone()) {
            return;
        }
        self.press_moved |= (x - self.press_at.0).abs() > CLICK_SLOP || (y - self.press_at.1).abs() > CLICK_SLOP;
        self.selection_drag(x, y);
        if self.input_click.captured {
            self.input_drag(x, y, drag_move);
            return;
        }
        let Some(i) = self.live_index(&path) else { return };
        let (id, r, has_drag) = (self.nodes[i].id, self.nodes[i].target, self.nodes[i].on_drag.is_some() && !self.drags_out(&self.nodes[i]));
        // A hit rect is where the element was drawn (after scrolling); `target` is its
        // unscrolled layout rect, kept only for elements that were never drawn.
        let mut hits = self.hits.iter().filter(|(hit, _)| *hit == id).peekable();
        self.capture_inside = !self.press_cancelled && if self.native_overlay || hits.peek().is_some() {
            hits.any(|&(id, r)| self.hit_contains(id, r, x, y))
        } else { contains(r, x, y) };
        if has_drag && drag_move {
            let (dx, dy) = (x - self.press_at.0, y - self.press_at.1);
            let [lx, ly] = self.to_local(i, x, y);
            let d = Drag { phase: Phase::Move, x: lx, y: ly, dx, dy, ..d };
            self.queue.push(Action::Drag(path, d, self.capture_button));
        }
    }

    /// Overlay observation updates coordinates without presenting unchanged hover pixels.
    pub(crate) fn observe_pointer(&mut self, x: f32, y: f32) -> bool {
        if self.pointer == Some((x, y)) { return false; }
        let visual_changed = self.nodes.iter().enumerate().any(|(i, n)| {
            if n.vis.hover.a <= 0.0 && n.vis.press.a <= 0.0 && n.press_scale.is_none() { return false; }
            let inside = |p: Option<(f32, f32)>| p.is_some_and(|(x, y)| self.node_contains(i, x, y));
            inside(self.pointer) != inside(Some((x, y)))
        });
        // Reaching or leaving a help element starts or ends its tooltip's wait.
        let help_changed = self.help_target_changes(x, y);
        let (queued, capture_inside) = (self.queue.len(), self.capture_inside);
        self.pointer_move(x, y);
        visual_changed || help_changed || self.queue.len() != queued || self.capture_inside != capture_inside
    }

    pub(crate) fn pointer_captured(&self) -> bool { self.capture.is_some() || self.secondary_capture.is_some() }

    /// The host moved its window by (dx, dy) points under a still pointer. Keep the pointer and
    /// a held press's origin in window points, so a drag that moves its own window still reports
    /// pointer travel in `Drag::dx`/`dy`.
    pub(crate) fn window_moved(&mut self, dx: f32, dy: f32) {
        if let Some((x, y)) = self.pointer { self.pointer = Some((x - dx, y - dy)); }
        self.press_at = (self.press_at.0 - dx, self.press_at.1 - dy);
    }

    pub(crate) fn cancel_pointer(&mut self) {
        self.cancel_drag();
        self.drop_drag_out_request();
        self.drag_out_armed = false;
        self.text_sel.dragging = false;
        self.input_click.clear();
        self.capture = None;
        self.secondary_capture = None;
        self.capture_inside = false;
        self.pointer_leave();
        self.drag_files_leave();
    }

    /// End a held drag without a drop: its handler hears `Phase::Up` with [`Drag::cancelled`]
    /// and the release that follows does nothing. False when no drag is held.
    fn cancel_drag(&mut self) -> bool {
        if self.input_click.captured { return false; }
        let Some((path, d)) = &self.capture else { return false };
        let Some(i) = self.live_index(&path) else { return false };
        if self.nodes[i].on_drag.is_none() || self.drags_out(&self.nodes[i]) { return false; }
        let (x, y) = self.pointer.unwrap_or(self.press_at);
        let [lx, ly] = self.to_local(i, x, y);
        let (dx, dy) = (x - self.press_at.0, y - self.press_at.1);
        let (path, d) = (path.clone(), Drag { phase: Phase::Up, x: lx, y: ly, dx, dy, cancelled: true, ..*d });
        let button = std::mem::take(&mut self.capture_button);
        self.queue.push(Action::Drag(path, d, button));
        self.capture = None;
        self.capture_inside = false;
        self.double_armed = false;
        self.drag_out_armed = false;
        self.text_sel.dragging = false;
        true
    }

    /// The held press crossed the drag threshold on a `drag_out` element: ask the host for an
    /// OS drag and hand it the pointer, so nothing stays pressed or hovered after the drop.
    /// False when the element offers nothing this time and the press stays a press.
    fn start_drag_out(&mut self, path: String) -> bool {
        self.drag_out_armed = false;
        let Some(i) = self.live_index(&path) else { return false };
        let Some(f) = self.nodes[i].drag_out.clone() else { return false };
        let Some(item) = f(playground_host::handler_states_ref(&self.state, self.playground.as_deref())).filter(|item| !item.is_empty()) else { return false };
        let n = &self.nodes[i];
        let r = self.drawn.get(i).copied().unwrap_or([n.target[0], n.target[1], n.target[2] - n.target[0], n.target[3] - n.target[1]]);
        let clip = self.screen_clip_of_parent(n.parent).map(|v| v / self.env.scale);
        let (x0, y0) = (r[0].max(clip[0]), r[1].max(clip[1]));
        let rect = [x0, y0, ((r[0] + r[2]).min(clip[2]) - x0).max(0.0), ((r[1] + r[3]).min(clip[3]) - y0).max(0.0)];
        log_event!(self, "drag-out {path}");
        self.drop_drag_out_request();
        self.queue.push(Action::DragStart(path.clone()));
        self.drag_out_request = Some(crate::drag_out::DragOutStart { path, item, rect, bounds: r, press: self.press_at });
        self.capture = None;
        self.capture_inside = false;
        self.pointer_leave();
        true
    }

    /// A request no host took (headless tools, a cancelled press) ends as `Cancelled`, queued
    /// behind its start, so `on_drag_start` is always followed by `on_drag_end`.
    fn drop_drag_out_request(&mut self) {
        if let Some(start) = self.drag_out_request.take() {
            self.queue.push(Action::DragEnd(start.path, crate::DragOutcome::Cancelled));
        }
    }

    /// A release outside the window, or after the pointer left during the press, ends the hover.
    fn leave_after_release(&mut self) {
        if self.pointer_captured() { return; }
        if std::mem::take(&mut self.left_in_capture) || self.pointer.is_some_and(|(x, y)| !self.in_viewport(x, y)) {
            self.pointer_leave();
        }
    }

    pub fn pointer_down(&mut self, x: f32, y: f32) {
        if self.capture.is_some() && self.capture_button != crate::PointerButton::Primary {
            self.pointer = Some((x, y));
            return;
        }
        self.drop_drag_out_request();
        self.pointer = Some((x, y));
        // A press is the hand's: hover follows the pointer through moving layout.
        self.release_hover_hold();
        self.keyboard_focus = false;
        self.dismiss_help();
        // A press cancels the waits to report an enter; the next move after it starts them again.
        self.hover_pending.retain(|p| !p.1);
        self.hover_paused = true;
        let target = self.primary_target(x, y);
        if target.and_then(|t| self.textbox_input(t)).is_none() {
            self.input_click.clear();
        }
        if let Some(i) = self.active_modal()
            && target.is_none_or(|t| !self.allowed(t))
        {
            if self.nodes[i].modal.as_ref().is_some_and(|m| m.outside) {
                self.queue.push(Action::Dismiss(self.nodes[i].path.clone()));
            }
            return;
        }
        self.playground_pointer_down(x);
        self.press_selection(target, x, y);
        // Inside a draggable row, dragging drags the row; clicks still select words and lines.
        if target.is_some_and(|t| self.nodes[t].on_drag.is_some() || self.nodes[t].drag_out.is_some()) {
            self.text_sel.dragging = false;
        }
        let Some(ti) = target else {
            if self.focused.is_some() {
                self.queue.push(Action::Blur);
            }
            return;
        };
        let on_link = self.link_down(ti, x, y);
        // Any press breaks a pending pair unless it's the second press of it.
        let pending = self.last_press.take();
        let now = self.now_clock();
        let n = &self.nodes[ti];
        if n.window_drag && n.on_press.is_none() && n.on_double_press.is_none() && n.on_drag.is_none() && n.drag_out.is_none() && self.textbox_input(ti).is_none() {
            let double = self.last_drag_press.is_some_and(|(t, px, py)| {
                now - t <= self.double_click_interval && (x - px).abs() <= CLICK_SLOP && (y - py).abs() <= CLICK_SLOP
            });
            self.last_drag_press = if double { None } else { Some((now, x, y)) };
            self.titlebar_action = Some(if double { TitlebarAction::DoubleClick } else { TitlebarAction::Drag });
            if self.focused.is_some() {
                self.queue.push(Action::Blur);
            }
            return;
        }
        self.press_at = (x, y);
        let clicks = self.press_clicks.press(&n.path, now, (x, y), self.double_click_interval);
        self.double_armed = n.on_double_press.is_some() && pending.is_some_and(|(p, t, px, py)| {
            p == n.path && now - t <= self.double_click_interval && (x - px).abs() <= CLICK_SLOP && (y - py).abs() <= CLICK_SLOP
        });
        self.press_down_t = now;
        self.press_moved = false;
        self.press_cancelled = false;
        if let Some(ji) = self.textbox_input(ti) {
            self.input_down(ji, x, y);
            return;
        }
        // Clicking a link leaves focus where it is (a composer keeps typing). Selectable text
        // off its links isn't a control: the press blurs, as a click on plain content does.
        // Inside a control that owns its keyboard (`El::on_key`), a click on any part of it
        // focuses it, as a click on a table view's row focuses the table.
        let keyed = (!n.focus.enabled && !self.is_selectable(ti)).then(|| std::iter::successors(n.parent, |&p| self.nodes[p].parent)
            .take_while(|&p| self.allowed(p))
            .find(|&p| self.nodes[p].focus.key.is_some() && self.can_focus(p))).flatten();
        let in_textbox = std::iter::successors(n.parent, |&p| self.nodes[p].parent).any(|p| self.textbox_input(p).is_some());
        let (preserve_focus, keeps) = (self.press_keeps_focus(ti), self.press_keeps_selection(ti));
        if !on_link && !keeps && n.focus.enabled && !self.is_selectable(ti) {
            self.queue.push(Action::FocusControl(n.path.clone()));
        } else if !on_link && !keeps && let Some(p) = keyed {
            self.queue.push(Action::FocusControl(self.nodes[p].path.clone()));
        } else if !on_link && !in_textbox && !preserve_focus && self.focused.is_some() {
            self.queue.push(Action::Blur);
        }
        let r = n.target;
        let [lx, ly] = self.to_local(ti, x, y);
        let d = Drag {
            phase: Phase::Down,
            x: lx,
            y: ly,
            w: r[2] - r[0],
            h: r[3] - r[1],
            start_x: lx,
            start_y: ly,
            dx: 0.0,
            dy: 0.0,
            data: 0.0,
            clicks,
            cancelled: false,
        };
        let path = n.path.clone();
        self.drag_out_armed = self.drags_out(n);
        self.drag_out_threshold = n.drag_threshold.unwrap_or(crate::drag_out::DRAG_THRESHOLD);
        if n.on_drag.is_some() && !self.drag_out_armed {
            self.queue.push(Action::Drag(path.clone(), d, crate::PointerButton::Primary));
        }
        self.capture = Some((path, d));
        self.capture_button = crate::PointerButton::Primary;
        self.capture_inside = true;
    }

    pub fn pointer_up(&mut self, x: f32, y: f32) {
        if self.capture.is_some() && self.capture_button != crate::PointerButton::Primary {
            self.pointer = Some((x, y));
            return;
        }
        self.release_capture(x, y);
    }

    pub(super) fn release_capture(&mut self, x: f32, y: f32) {
        // Hosts release at the last position they saw, which a leave doesn't move.
        let left = self.left_in_capture;
        self.release(x, y);
        self.left_in_capture = left;
        self.leave_after_release();
    }

    fn release(&mut self, x: f32, y: f32) {
        // A release never starts a drag-out: the system session needs the button held.
        self.drag_out_armed = false;
        let moved = self.pointer != Some((x, y));
        self.move_pointer(x, y, moved);
        // A drag that selected text is a selection, not a press.
        let selected = self.selected_by_drag();
        self.text_sel.dragging = false;
        let input_capture = self.input_click.end_press();
        let Some((path, d)) = self.capture.take() else { return };
        let button = std::mem::take(&mut self.capture_button);
        self.refresh_hover_within();
        let double = std::mem::take(&mut self.double_armed);
        if self.press_moved { self.press_clicks.clear(); }
        if input_capture {
            self.capture_inside = false;
            return;
        }
        let Some(i) = self.live_index(&path) else { return };
        let n = &self.nodes[i];
        let (has_drag, has_press, has_double) = (n.on_drag.is_some() && !self.drags_out(n), n.on_press.is_some(), n.on_double_press.is_some());
        if has_drag {
            let (dx, dy) = (x - self.press_at.0, y - self.press_at.1);
            let [lx, ly] = self.to_local(i, x, y);
            self.queue.push(Action::Drag(path.clone(), Drag { phase: Phase::Up, x: lx, y: ly, dx, dy, ..d }, button));
        }
        self.link_up(&path, x, y);
        if has_press && self.capture_inside && !selected {
            self.queue.push(Action::Press(path.clone(), button));
        }
        // Only clicks pair: a press that dragged or was held past the interval is neither half.
        let click = !self.press_moved && self.now_clock() - self.press_down_t <= self.double_click_interval;
        if has_double && self.capture_inside && click && button == crate::PointerButton::Primary {
            if double {
                self.queue.push(Action::DoublePress(path));
            } else {
                self.last_press = Some((path, self.press_down_t, self.press_at.0, self.press_at.1));
            }
        }
        self.capture_inside = false;
    }

    pub fn pointer_leave(&mut self) {
        self.wheel_lost();
        if self.capture.is_none() && self.secondary_capture.is_none() {
            self.pointer = None;
            self.hovered = None;
            self.left_in_capture = false;
            self.refresh_hover_within();
        } else {
            self.left_in_capture = true;
        }
    }

    /// A press on an element is held (the hand drives whatever follows it).
    pub fn pressing(&self) -> bool {
        self.capture.is_some()
    }
    pub fn pointer(&self) -> Option<(f32, f32)> {
        self.pointer
    }

    /// Native overlay regions from the most recent frame, in logical window points (x0,y0,x1,y1).
    pub fn hit_regions(&self) -> &[[f32; 4]] { &self.overlay_hits }

    /// The pointer shape the shell should show: the pressed element's while a gesture holds
    /// the pointer, else the one under it in the current frame (the held hover while moving
    /// layout holds it), from the nearest ancestor that sets one. The arrow while the pointer is outside the window.
    pub fn cursor(&self) -> crate::el::Cursor {
        let Some((x, y)) = self.pointer.filter(|&(x, y)| self.in_viewport(x, y)) else { return crate::el::Cursor::Default };
        // An element on its way out has no cursor: `Cursor::Hidden` ends when it's removed.
        // Selectable text shows its I-beam even where its presses go to a row around it.
        let target = match &self.capture {
            Some(_) if self.text_sel.dragging => self.selection_holder(),
            Some((p, _)) => self.live_index(p).filter(|&i| self.allowed(i)),
            // Moving layout holds hover: the cursor stays with what the pointer was over.
            None => self.held_hover().flatten().or_else(|| self.selectable_at(x, y).or_else(|| self.primary_target(x, y))),
        };
        let Some(mut i) = target else {
            return crate::el::Cursor::Default;
        };
        if self.nodes[i].cursor.is_none() && (self.nodes[i].input.is_some() || self.nodes[i].role == "textbox") {
            return crate::el::Cursor::Text;
        }
        if self.is_selectable(i) && !self.text_sel.dragging && self.link_at(i, x, y).is_some() {
            return crate::el::Cursor::Pointer;
        }
        loop {
            if let Some(c) = self.nodes[i].cursor {
                return c;
            }
            let Some(parent) = self.nodes[i].parent else { return crate::el::Cursor::Default };
            i = parent;
        }
    }

    pub fn key(&mut self, ev: KeyEvent) {
        self.dismiss_help();
        // Escape during a drag cancels it and goes no further.
        if ev.key == crate::Key::Escape && self.cancel_drag() { return; }
        self.queue.push(Action::Key { ev, repeat: false });
    }

    /// Queue an auto-repeat of a held key, as the OS sends after the first press. Handlers see
    /// `Tx::key_repeat` true for it.
    pub fn repeat_key(&mut self, ev: KeyEvent) {
        self.queue.push(Action::Key { ev, repeat: true });
    }

    pub fn ime(&mut self, ev: Ime) {
        self.queue.push(Action::Ime(ev));
    }

    /// End native composition on window deactivation without discarding text or field focus.
    pub fn cancel_composition(&mut self) {
        self.ime(Ime::Preedit(String::new(), None));
        self.ime_reset = true;
    }

    pub fn paste(&mut self, text: impl Into<String>) {
        self.queue.push(Action::Paste(crate::ClipboardContent::new(text), None));
    }
    /// [`Runtime::paste`] with several forms, as from another app's clipboard: its HTML is
    /// sanitised as a real paste's is. For tests and scripts.
    pub fn paste_content(&mut self, content: crate::ClipboardContent) {
        self.queue.push(Action::Paste(content.untrusted(), None));
    }

    fn app_paste_handler(&self) -> Option<crate::el::PasteFn> {
        self.nodes.get(self.active_modal().unwrap_or_else(|| self.app_root()))?.on_paste.clone()
    }
    pub(crate) fn accepts_paste(&self) -> bool {
        self.paste_target().is_some() || self.app_paste_handler().is_some()
    }
    pub(crate) fn paste_target(&self) -> Option<String> {
        self.focused_input().filter(|i| i.access.is_some()).and(self.focused.clone())
    }
    /// What the paste target hears: every form, images too, for a field with
    /// `on_input_paste` or a root `on_paste_content` handler; text, HTML and RTF for a rich
    /// [`TextEdit`](crate::TextEdit); only the clipboard's text for anything else (including a
    /// plain field or a plain `on_paste`).
    pub(crate) fn paste_forms(&self, target: Option<&str>) -> crate::clipboard::Forms {
        use crate::clipboard::Forms;
        match target {
            Some(path) => match self.node(path).and_then(|n| n.input.as_deref()).or_else(|| self.virtual_inputs.get(path).map(Box::as_ref)) {
                Some(input) if input.paste.is_some() => Forms::All,
                Some(input) if input.edit.is_rich() && !input.edit.is_secure() => Forms::Rich,
                _ => Forms::Text,
            },
            None if self.app_paste_handler().is_some_and(|p| p.rich) => Forms::All,
            None => Forms::Text,
        }
    }
    pub(crate) fn paste_result(&mut self, target: Option<String>, content: crate::ClipboardContent) {
        self.queue.push(Action::Paste(content, Some(target)));
    }
    pub(crate) fn read_result(&mut self, id: u64, content: crate::ClipboardContent) {
        self.queue.push(Action::ClipboardRead(id, content));
    }

    /// Whether the focused editor requires secure native keyboard input.
    pub fn has_secure_focus(&self) -> bool {
        self.focused_input().is_some_and(|i| i.edit.is_secure())
    }

    pub fn has_focus(&self) -> bool {
        self.focused.is_some()
    }

    /// The innermost vertical scroll viewport at (x, y).
    fn scroll_at(&self, x: f32, y: f32) -> Option<NodeId> {
        self.scroll_at_axis(x, y, false)
    }

    /// The innermost scroll viewport at (x, y) that moves on the horizontal axis or the vertical.
    fn scroll_at_axis(&self, x: f32, y: f32, horizontal: bool) -> Option<NodeId> {
        self.nodes
            .iter()
            .enumerate()
            .rev()
            .filter(|(i, n)| n.scroll.is_some() && n.scroll_axes.has(horizontal) && !self.subtree_exiting(*i) && self.allowed(*i))
            .find(|(i, _)| self.node_contains(*i, x, y))
            .map(|(_, n)| n.id)
    }

    /// Wheel or trackpad delta in points (positive `dy` moves content down, like a browser's
    /// negative deltaY). Applied 1:1: the platform supplies its own momentum phases.
    pub fn wheel(&mut self, x: f32, y: f32, dy: f32) {
        self.release_hover_hold();
        self.drop_scroll_holds();
        self.dismiss_help();
        let Some(id) = self.scroll_at(x, y) else { return };
        let stick = self.node_index.get(&id).and_then(|&i| self.nodes[i].scroll).is_some_and(|s| s.stick_bottom);
        if let Some(st) = self.scrolls.get_mut(&id) {
            let before = st.offset;
            st.offset = (st.offset - f64::from(dy)).clamp(0.0, st.max);
            st.v = 0.0;
            st.glide = None;
            st.direct = true;
            st.pinned = stick && st.offset >= st.max - 0.5;
            log_event!(self, "wheel {dy:.1}");
            if st.offset != before { self.scroll_cancels_press(id); }
        }
    }

    /// Releases a scroll with `velocity` (points per second, positive moves content down);
    /// the runtime decays it (for platforms without OS momentum and headless scripts).
    pub fn fling(&mut self, x: f32, y: f32, velocity: f32) {
        self.release_hover_hold();
        self.drop_scroll_holds();
        self.dismiss_help();
        let Some(id) = self.scroll_at(x, y) else { return };
        if let Some(st) = self.scrolls.get_mut(&id) {
            st.v = -velocity;
            st.pinned = false;
            st.glide = None;
            log_event!(self, "fling {velocity:.0}");
            if velocity != 0.0 { self.scroll_cancels_press(id); }
        }
    }

    pub fn has_pending_input(&self) -> bool {
        !self.queue.is_empty()
    }

    /// Scrolling viewport `id` under a held press inside it cancels the press, as a UIScrollView
    /// cancels its content's touches: the row lets go and the release doesn't press. A drag
    /// keeps its capture.
    fn scroll_cancels_press(&mut self, id: NodeId) {
        let Some((path, _)) = &self.capture else { return };
        let Some(i) = self.nodes.iter().position(|n| &n.path == path) else { return };
        let inside = std::iter::successors(self.nodes[i].parent, |&p| self.nodes[p].parent).any(|p| self.nodes[p].id == id);
        if inside && self.nodes[i].on_drag.is_none() {
            self.press_cancelled = true;
            self.capture_inside = false;
            self.double_armed = false;
        }
    }

}

/// A horizontal scroll viewport's content without a width of its own, or one filling the
/// viewport (`w_full`), takes its natural one, unwrapped, and at least the viewport's (CSS
/// `width: max-content; min-width: 100%`). Any other width stays as set, narrower than the
/// viewport too. The content never shrinks. The viewport lays it out as a row, where taffy
/// measures a max-content main size unwrapped (a column measures its height wrapped at the
/// viewport's width). With vertical scrolling too it keeps its natural height instead of
/// stretching to the viewport's.
fn unwrapped_width(style: &mut Style, vertical: bool) {
    if style.size.width.is_auto() || style.size.width == Dimension::percent(1.0) {
        style.size.width = Dimension::max_content();
        if style.min_size.width.is_auto() { style.min_size.width = percent(1.0); }
    }
    style.flex_shrink = 0.0;
    if vertical && style.align_self.is_none() { style.align_self = Some(AlignSelf::FLEX_START); }
}

/// The (x, y) scroll offsets of node `i`'s scroll viewports, innermost first.
fn scroll_offsets<'a>(scrolls: &'a HashMap<NodeId, ScrollState>, nodes: &'a [Node], i: usize) -> impl Iterator<Item = [f32; 2]> + 'a {
    std::iter::successors(Some(i), |&p| nodes[p].parent.filter(|_| nodes[p].above.is_none()))
        .skip(1)
        .filter_map(|p| scrolls.get(&nodes[p].id).map(|st| [st.x.offset as f32, st.offset as f32]))
}

/// Marks every ancestor of a marked node.
fn mark_ancestors(nodes: &[Node], marked: &mut [bool]) {
    for i in 0..nodes.len() {
        if !marked[i] { continue; }
        let mut p = nodes[i].parent;
        while let Some(j) = p {
            if marked[j] { break; }
            marked[j] = true;
            p = nodes[j].parent;
        }
    }
}

/// Folds one handler's `Tx::live_only` into its event's: live-only when every handler that
/// took the event said so.
pub(crate) fn fold_live(live: &mut Option<bool>, took: bool, one: &Tx) {
    if took { *live = Some(live.unwrap_or(true) && one.live_only); }
}

#[cfg(test)]
#[path = "runtime/tests/mod.rs"]
mod tests;
