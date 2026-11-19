//! The retained tree: view → keyed nodes → Taffy layout at the target state → `rune-motion` Scene.
//! Animation samples tracks at frame time; it never re-runs view functions.
//!
//! Beside Scene's tracks the runtime owns four render-side behaviours that never touch app code:
//! scroll viewports (offset, momentum, follow-the-end), growth stacks (row heights on a capped
//! spring, positions derived), streaming word reveals, and text-field carets. Each frame:
//! queued input → (commit if state changed) → Scene → growth → scroll → reveals → build.

use std::any::Any;
// The runtime's maps hash node ids and paths every commit; SipHash's DoS resistance buys nothing here.
pub(crate) use rune_core::{HashMap, HashSet};
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
use rune_taffy::prelude::*;

use crate::color::{Color, over, srgb_encode};
use crate::edit::{EditOutcome, Ime, KeyEvent};
use crate::el::{Align, Commit, Drag, El, GrowthSpec, InputSpec, MotionSource, Phase, PointerEvent, ScrollSpec, Shell, TextSpec, Tx, Visual};
use crate::growth::GrowTrack;
// Half-open, like overlays, NSPointInRect and PtInRect: a shared edge belongs to one side.
use crate::overlay::options::contains;
use crate::flight::{TextFlight, transformed};
use crate::render::{Instance, KIND_DASHED, KIND_EFFECT, KIND_GLYPH, KIND_GRADIENT, KIND_IMAGE, KIND_INSET, KIND_RADIAL, KIND_RECT, KIND_SEGMENT, KIND_SHADOW, KIND_TRIANGLE, NO_CLIP};
use crate::shape::{LineCap, Op, Shape};
use crate::text::{Deco, PaintedDeco, GlyphQuad, Ink, TextFlow, TextLayout, TextSystem};
use crate::tok::motion;
use event_log::log_event;

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

pub(crate) mod driver;
pub(crate) use driver::{AppDriver, AppRef, AppRefMut, RuntimeHooks, ViewDriver};
mod init;
mod api;
#[path = "runtime/input/api.rs"]
mod input_api;

#[cfg(debug_assertions)]
#[path = "runtime/commit/layout_diagnostics.rs"]
mod layout_diagnostics;
#[path = "runtime/paint/blend.rs"]
mod blend;
#[path = "runtime/commit/watch.rs"]
mod commit_watch;
#[path = "runtime/commit/why.rs"]
mod commit_why;
#[path = "runtime/interaction.rs"]
mod interaction_timing;
mod drag_out_host;
#[path = "runtime/paint/exit_mask.rs"]
mod exit_mask;
#[path = "runtime/paint/raster_check.rs"]
mod raster_check;
mod font_families;
mod key_index;
mod system_settings;
mod zoom;
#[path = "runtime/commit/prune.rs"]
mod prune;
pub(crate) use blend::{apron_key, glyph, icon_inst, premul};

#[path = "runtime/input/focus.rs"]
mod focus;
#[path = "runtime/input/capture.rs"]
mod capture;
#[path = "runtime/input/focus_machine.rs"]
mod focus_machine;
#[path = "runtime/input/modal_focus.rs"]
mod modal_focus;
#[path = "runtime/input/physical.rs"]
mod physical_dispatch;
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(unused_imports))]
pub(crate) use physical_dispatch::Unhandled;
#[path = "runtime/input/modifier_keys.rs"]
mod modifier_keys;
#[path = "runtime/input/popup.rs"]
mod popup_dispatch;
#[path = "runtime/input/file_dialog.rs"]
mod file_dialog_dispatch;
#[path = "runtime/input/drop.rs"]
mod drop_dispatch;
#[path = "runtime/playground.rs"]
mod playground_host;
#[path = "runtime/scroll/bottom.rs"]
mod scroll_bottom;
#[path = "runtime/selection/caret.rs"]
mod caret;
pub(crate) mod event_log;
pub(crate) mod layout_read;
mod visible;
#[path = "runtime/selection/links.rs"]
mod links;
#[path = "runtime/selection/selectable.rs"]
pub(crate) mod selectable;
#[path = "runtime/selection/text_marks.rs"]
mod text_marks;
pub use text_marks::TextMark;
#[path = "runtime/selection/click.rs"]
mod input_click;
#[path = "runtime/input/help.rs"]
mod help;
#[path = "runtime/input/help_look.rs"]
mod help_look;
pub use help_look::HelpLook;
#[path = "runtime/scroll/edge.rs"]
mod scroll_edge;
#[path = "runtime/scroll/x.rs"]
mod scroll_x;
#[path = "runtime/scroll/offset.rs"]
mod scroll_offset;
#[path = "runtime/scroll/reveal.rs"]
mod reveal;
#[path = "runtime/scroll/requests.rs"]
mod requests;
#[path = "runtime/scroll/anchor.rs"]
mod scroll_anchor;
#[path = "runtime/scroll/sticky.rs"]
mod sticky;
mod anchor;
pub(crate) use anchor::anchor_position;
use anchor::{anchor_index, step_up};
#[path = "runtime/motion/motion_elements.rs"]
mod motion_elements;
pub use motion_elements::MotionElement;
#[path = "runtime/motion/live_latch.rs"]
mod live_latch;
#[path = "runtime/motion/loop_clock.rs"]
mod loop_clock;
#[path = "runtime/motion/reveal_phase.rs"]
mod reveal_phase;
use reveal_phase::{Arrival, RevealChain, RevealEffect, RevealEvent, RevealPhase};
#[path = "runtime/motion/rewind.rs"]
mod rewind_record;
#[path = "runtime/scroll/drawn.rs"]
mod scroll_drawn;
use scroll_drawn::PixelHold;
#[path = "runtime/scroll/nudge.rs"]
mod scroll_nudge;
#[path = "runtime/scroll/mode.rs"]
mod scroll_mode;
use scroll_mode::ScrollMode;
#[path = "runtime/scroll/watch.rs"]
mod scroll_watch;
#[path = "runtime/scroll/wheel.rs"]
mod scroll_wheel;
#[doc(hidden)]
pub use scroll_watch::scroll_watch_state;
#[path = "runtime/input/autoscroll.rs"]
mod autoscroll;
#[cfg(target_os = "linux")]
#[path = "runtime/input/primary_selection.rs"]
mod primary_selection;
#[path = "runtime/input/gesture.rs"]
mod gesture_dispatch;
#[path = "runtime/input/key_releases.rs"]
mod key_releases;
#[path = "runtime/input/touch.rs"]
mod touch_input;
pub use touch_input::TouchPhase;
#[path = "runtime/scroll/touch.rs"]
mod scroll_touch;
#[path = "runtime/motion/flights.rs"]
mod flights;
#[path = "runtime/paint/text_flights.rs"]
mod text_flights;
#[path = "runtime/input/hover.rs"]
mod hover;
#[path = "runtime/input/hover_delay.rs"]
mod hover_delay;
#[path = "runtime/motion/timer_wake.rs"]
mod timer_wake;
#[path = "runtime/motion/idle_meter.rs"]
mod idle_meter;
#[cfg(feature = "hot")]
mod hot_state;
#[path = "runtime/motion/props.rs"]
mod props;
#[path = "runtime/input/secondary.rs"]
mod secondary;
#[path = "runtime/input/field_menu.rs"]
pub(crate) mod field_menu;
#[path = "runtime/input/app_ime.rs"]
mod app_ime;
#[path = "runtime/input/press_run.rs"]
mod press_run;
#[path = "runtime/input/press_repeat.rs"]
mod press_repeat;
#[path = "runtime/input/titlebar.rs"]
mod titlebar;
pub(crate) use titlebar::WindowMenu;
mod window_requests;
#[path = "runtime/paint/tracks.rs"]
mod paint_tracks;
#[path = "runtime/input/action.rs"]
pub(crate) mod action;
use action::*;
#[path = "runtime/paint/bands.rs"]
mod bands;
#[path = "runtime/paint/border.rs"]
mod border_paint;
#[path = "runtime/paint/slots.rs"]
mod paint_slots;
#[path = "runtime/paint/pixel_snap.rs"]
mod pixel_snap;
#[path = "runtime/paint/soft_edges.rs"]
mod soft_edges;
use soft_edges::{in_fade, inherit_fade};
#[path = "runtime/paint/shadow.rs"]
mod shadow_paint;
#[path = "runtime/paint/shape.rs"]
mod shape_paint;
use shape_paint::push_shape;
use props::*;
#[cfg(not(target_os = "windows"))]
pub(crate) use scroll_x::LINE_POINTS;
pub(crate) use scroll_x::{SHIFT_TURNS_WHEEL, wheel_delta};
pub(crate) use scroll_offset::ScrollRequest;
pub(crate) use reveal::ScrollReveal;
pub use selectable::{SelectedText, SelectionChange, TextSelection};
#[cfg(test)]
mod timer_wake_tests;
#[cfg(test)]
mod grid_tests;
#[cfg(test)]
mod truncate_tests;
#[cfg(test)]
mod truncate_fade_tests;
#[cfg(test)]
mod line_clamp_tests;
#[cfg(test)]
mod aspect_fit_tests;
#[cfg(test)]
mod anchor_fit_tests;
#[cfg(test)]
mod drop_tests;
#[cfg(test)]
mod on_key_tests;
#[cfg(test)]
mod live_only_tests;
#[cfg(test)]
mod live_check_tests;
#[cfg(test)]
mod virtual_store_tests;
#[cfg(test)]
#[path = "runtime/virtual/fill_resume_tests.rs"]
mod fill_resume_tests;
#[path = "runtime/selection/select_all.rs"]
mod select_all;
#[cfg(test)]
mod select_all_tests;
#[cfg(test)]
mod selection_api_tests;
#[cfg(test)]
mod anchored_grow_tests;
#[cfg(test)]
mod pixel_snap_tests;
#[cfg(test)]
#[path = "runtime/recorder_tests.rs"]
mod recorder;
#[cfg(test)]
mod size_tests;
#[cfg(test)]
#[path = "runtime/video_tests.rs"]
mod video;

#[cfg(any(test, target_arch = "wasm32"))]
#[path = "runtime/web/runtime.rs"]
pub(crate) mod web_runtime;
#[path = "runtime/web/a11y.rs"]
pub(crate) mod web_a11y;
#[path = "runtime/web/page.rs"]
pub(crate) mod web_page;
#[path = "runtime/web/export.rs"]
pub(crate) mod web_export;

#[path = "runtime/commit/text_layout.rs"]
mod text_layout;
use text_layout::*;
pub(crate) use text_layout::text_width;
#[path = "runtime/commit/flatten.rs"]
mod flatten;
use flatten::*;
#[path = "runtime/commit/memo_reuse.rs"]
pub(crate) mod memo_reuse;
#[path = "runtime/commit/measure.rs"]
mod measure;
#[path = "runtime/commit/apply.rs"]
mod apply;
#[path = "runtime/commit/apply_scratch.rs"]
mod apply_scratch;
#[path = "runtime/commit/relayout.rs"]
mod relayout;
#[path = "runtime/virtual/window.rs"]
mod virtual_window;
#[path = "runtime/virtual/fill.rs"]
mod virtual_fill;
#[path = "runtime/motion/unseen_presence.rs"]
mod unseen_presence;
#[path = "runtime/virtual/row_reorder.rs"]
mod row_reorder;
#[path = "runtime/motion/growth_rows.rs"]
mod growth_rows;
#[path = "runtime/motion/growth_runs.rs"]
mod growth_runs;
#[path = "runtime/motion/fold_hold.rs"]
mod fold_hold;
#[path = "runtime/motion/landing_hold.rs"]
mod landing_hold;
#[path = "runtime/motion/content_slide.rs"]
mod content_slide;
mod elements;
#[path = "a11y/tree.rs"]
pub(crate) mod a11y_tree;
#[path = "a11y/node.rs"]
mod a11y_node;
#[path = "a11y/text.rs"]
mod a11y_text;
#[path = "a11y/actions.rs"]
mod a11y_actions;
use a11y_actions::A11yAct;
#[cfg(test)]
#[path = "a11y/tests.rs"]
mod a11y_tests;
mod paint_probe;
pub use elements::{ElementInfo, Pick, Picked, Pose};
#[path = "runtime/paint/cache_layer.rs"]
mod cache_layer;
use cache_layer::{ActiveCache, AnotherFrame, NodeCache, RasterPass, RasterPhase, ScaleSettle};
#[path = "runtime/paint/helpers.rs"]
mod paint_helpers;
use paint_helpers::*;
#[path = "runtime/paint/content.rs"]
mod paint_content;
#[path = "runtime/paint/grid.rs"]
mod grid_paint;
#[path = "runtime/input/hit.rs"]
mod hit;
#[path = "runtime/paint/build.rs"]
mod build;
#[path = "runtime/paint/keep.rs"]
mod paint_keep;
#[path = "runtime/paint/state.rs"]
mod paint_state;
mod text_fill;
#[path = "runtime/input/keys.rs"]
mod keys;
#[path = "runtime/input/pointer.rs"]
mod pointer;
use pointer::CLICK_SLOP;
#[path = "runtime/motion/live_check.rs"]
mod live_check;
pub(crate) use live_check::fold_live;
mod frame;
mod cx;
pub use cx::{Cx, Env, Waker, commit_generation};
pub(crate) use cx::caret_of;
use cx::COMMITS;
mod turn;
pub(crate) use turn::intersect;
use turn::{Turn, compose_turn, open_turned_layer, screen_clip_of};
mod types;
pub use types::{FrameStats, TitlebarAction};
pub(crate) use commit_why::CommitWhy;
#[cfg(debug_assertions)]
pub(crate) use layout_read::window_read_key;
pub(crate) use types::{AnimSource, Node};
use types::{AxisScroll, GlideTo, Inh, NodeCold, RowLayout, ScaleMotion, ScrollState, Stack, WidthHold, XMotion};
#[path = "runtime/motion/tracks.rs"]
mod tracks;
use tracks::{SCROLL_DECAY, mark_ancestors, scroll_offsets};
mod commit;
use commit::{Measured, Measurement, Pass, unwrapped_width};

/// Zero-sized alignment barrier terminating `RuntimeCore` and the identical `#[repr(C)]`
/// prefix of `Runtime<S>`, ensuring `Runtime<S>::state` always sits at or past
/// `size_of::<RuntimeCore>()`.
#[repr(align(16))]
pub(crate) struct CoreEnd;

macro_rules! runtime_core_fields {
    (
        $(
            $(#[doc = $doc:expr])*
            $(#[cfg($($cfg:tt)*)])*
            $vis:vis $name:ident : $ty:ty
        ),* $(,)?
    ) => {
        /// Non-generic retained tree, layout, scene, input, scroll, selection, and paint state
        /// forming the exact `#[repr(C)]` prefix of [`Runtime<S>`].
        #[repr(C)]
        pub(crate) struct RuntimeCore {
            $(
                $(#[doc = $doc])*
                $(#[cfg($($cfg)*)])*
                $vis $name: $ty,
            )*
            pub(crate) _core_end: CoreEnd,
        }

        #[repr(C)]
        pub struct Runtime<S: 'static> {
            $(
                $(#[doc = $doc])*
                $(#[cfg($($cfg)*)])*
                $vis $name: $ty,
            )*
            pub(crate) _core_end: CoreEnd,
            pub state: S,
            /// Called at the start of every frame with the clock; return true when state changed (new data).
            pub poll: Option<fn(&mut S, f64, &mut Tx) -> bool>,
            /// Called for keys unhandled by the focused control; return true to consume it.
            pub on_key: Option<fn(&mut S, &KeyEvent, &mut Tx) -> bool>,
            pub(crate) hooks: RuntimeHooks<S>,
        }

        impl<S: 'static> Runtime<S> {
            /// Builds a `Runtime<S>` from an initialized `RuntimeCore` and its `S`-dependent tail
            /// fields. Generated from the shared field list as an exhaustive struct literal so
            /// adding any field without initializing it is a compile error.
            pub(crate) fn from_core(core: RuntimeCore, state: S, view: fn(&S, &mut Cx<S>) -> El) -> Self {
                assert_prefix_layout::<S>();
                let hooks = RuntimeHooks::new(view);
                Self {
                    $(
                        $(#[cfg($($cfg)*)])*
                        $name: core.$name,
                    )*
                    _core_end: core._core_end,
                    state,
                    poll: None,
                    on_key: None,
                    hooks,
                }
            }
        }

        #[inline(always)]
        const fn assert_prefix_layout<S: 'static>() {
            const {
                assert!(std::mem::size_of::<CoreEnd>() == 0);
                assert!(std::mem::align_of::<RuntimeCore>() <= 16);
                $(
                    $(#[cfg($($cfg)*)])*
                    assert!(std::mem::offset_of!(Runtime<S>, $name) == std::mem::offset_of!(RuntimeCore, $name));
                )*
                assert!(std::mem::offset_of!(Runtime<S>, _core_end) == std::mem::offset_of!(RuntimeCore, _core_end));
                assert!(std::mem::size_of::<RuntimeCore>() == std::mem::offset_of!(RuntimeCore, _core_end));
                assert!(std::mem::offset_of!(Runtime<S>, state) >= std::mem::size_of::<RuntimeCore>());
                assert!(std::mem::offset_of!(Runtime<S>, poll) >= std::mem::size_of::<RuntimeCore>());
                assert!(std::mem::offset_of!(Runtime<S>, on_key) >= std::mem::size_of::<RuntimeCore>());
                assert!(std::mem::offset_of!(Runtime<S>, hooks) >= std::mem::size_of::<RuntimeCore>());
                assert!(std::mem::align_of::<Runtime<S>>() >= std::mem::align_of::<RuntimeCore>());
            }
        }
    };
}

// Field order is load-bearing under `#[repr(C)]`: the leading `Env`/clock scalars, `tree`,
// `instances`/`marks`, `hit`, `paint`, `motion`, `scroll`, `scene`, `frame` and `commit` are read
// per node or per frame during paint, commit and idle walks, so they are kept together and first
// ahead of colder input, text and host state.
runtime_core_fields! {
    pub env: Env,
    /// Viewport in points: the view's, the window's over the zoom.
    pub viewport: (f32, f32),
    /// Animation time (seconds, scaled by the time scale).
    pub t: f64,
    /// Seconds since the first frame, unscaled (virtual when headless).
    pub clock: f64,
    pub ime_area: Option<[f32; 4]>,
    pub time_scale: f32,
    pub reduced_motion: bool,
    pub track_marks: bool,
    pub animating: bool,
    /// This frame's commit landed every track (`Tx::settle`): a deliberate cut.
    pub settled: bool,
    /// Transparent native surfaces/headless exports clear to transparent black.
    pub transparent: bool,

    /// Retained node tree, id/key lookup tables and per-commit capability flags.
    pub(crate) tree: types::TreeState,
    /// Seconds until the next frame while only [`Cycle`](crate::el::Cycle)s move on screen
    /// (the shell sleeps until then instead of drawing every display frame).
    pub ambient: Option<f32>,
    pub instances: Vec<Instance>,
    /// Per instance, the `El::mark` of its nearest marked node (0 for none). Filled only while
    /// `track_marks` is set (a compositor is attached).
    pub marks: Vec<u32>,

    /// Per-frame hit-test lists built in paint order.
    hit: hit::HitState,
    /// Two presses closer than this (seconds) are a double press: the system setting on macOS
    /// and Windows, 0.5 elsewhere and headless.
    pub double_click_interval: f64,

    /// Paint scratch buffers, reuse record, layer caches, text quads and paint outputs.
    pub(crate) paint: paint_state::PaintState,
    /// Render-side growth stacks, reveals, flights, loops and rewind capture.
    pub(crate) motion: tracks::MotionState,
    /// Scroll viewports, wheel axis locking, scroll requests and sticky headers.
    scroll: requests::ScrollTables,
    pub scene: Scene,
    /// Action queue, pending `Tx`, wake deadlines, live latch, tracing and hot reload.
    pub(crate) frame: timer_wake::FrameState,
    /// View commit, Taffy layout, memo cache, virtual lists and layout callback state.
    pub(crate) commit: commit::CommitState,
    /// Pointer position, capture, hover/tooltip tracking, touch, autoscroll and file/OS drags.
    pointer: capture::PointerState,
    /// Keyboard focus, modal focus/scope, type-select, held keys and modifier tracking.
    kbd: focus_machine::KeyboardState,

    pub surfaces: Vec<crate::native_surface::SurfaceDraw>,
    /// RGBA images elements show (`El::pixels`), uploaded by the renderer.
    pub images: crate::pixels::ImageAtlas,
    pub text: TextSystem,

    /// Selectable text, links, field click/scroll, caret, IME, spellcheck and clipboard state.
    pub(crate) edit: caret::TextInputState,
    /// Window/host settings, zoom, titlebar, window requests, popups, dialogs, a11y and web page.
    pub(crate) host: window_requests::HostState,

    /// Text the focused field asked to copy (the shell writes it to the clipboard).
    pub copy_request: Option<crate::ClipboardWrite>,
    /// A handler or `update` asked to close the window or quit (the shell takes it).
    pub shell_request: Option<Shell>,
    /// Windowed runs read the clock from the wall instead of summing frame steps.
    pub wall_start: Option<crate::time::Instant>,
    /// Input markers for traces and headless tests, at most `event_log::CAP` of them unread.
    pub events: Vec<String>,
    /// Counters for perf gates.
    pub stats: FrameStats,
    /// How long (seconds) the pointer rests on an `El::help` element before its tooltip shows:
    /// the system's hover time on Windows, 0.5 elsewhere and headless. macOS windows use
    /// AppKit's own tooltips, unless the view sets its own look (`Cx::help_look`), whose delay
    /// may replace this one.
    pub help_delay: f64,
}

// Moving a non-Copy field out fails with E0509 if `RuntimeCore` or `Runtime<S>` ever implements `Drop`.
const _: () = {
    fn check<S: 'static>(core: RuntimeCore, rt: Runtime<S>) -> (Vec<Node>, Vec<Node>) {
        (core.tree.nodes, rt.tree.nodes)
    }
    let _ = check::<()>;
};

impl<S: 'static> Runtime<S> {
    pub fn new(state: S, view: fn(&S, &mut Cx<S>) -> El, viewport: (f32, f32), env: Env) -> Self {
        Self::with_text(state, view, viewport, env, TextSystem::default())
    }

    pub fn with_text(state: S, view: fn(&S, &mut Cx<S>) -> El, viewport: (f32, f32), env: Env, text: TextSystem) -> Self {
        Self::from_core(RuntimeCore::new(viewport, env, text), state, view)
    }

    #[inline(always)]
    pub(crate) fn core(&self) -> &RuntimeCore {
        assert_prefix_layout::<S>();
        // SAFETY: `Runtime<S>` and `RuntimeCore` are `#[repr(C)]` with an identical field prefix
        // ending at zero-sized `_core_end: CoreEnd`, and neither implements custom `Drop` (guarded
        // at compile time above). `assert_prefix_layout::<S>()` proves `size_of::<CoreEnd>() == 0`,
        // identical `_core_end` offsets equal to `size_of::<RuntimeCore>()`, and
        // `align_of::<Runtime<S>>() >= align_of::<RuntimeCore>()`.
        unsafe { &*std::ptr::from_ref(self).cast::<RuntimeCore>() }
    }

    /// Mutable view of the shared non-generic prefix.
    ///
    /// Mutate fields through the returned reference only; never replace the whole value
    /// (`std::mem::swap`, `std::mem::replace`, `*core = ...`), which would pair this
    /// `Runtime<S>`'s tail fields with another runtime's core.
    #[inline(always)]
    pub(crate) fn core_mut(&mut self) -> &mut RuntimeCore {
        assert_prefix_layout::<S>();
        // SAFETY: `Runtime<S>` and `RuntimeCore` are `#[repr(C)]` with an identical field prefix
        // ending at zero-sized `_core_end: CoreEnd`, and neither implements custom `Drop` (guarded
        // at compile time above). `assert_prefix_layout::<S>()` proves `size_of::<CoreEnd>() == 0`,
        // identical `_core_end` offsets equal to `size_of::<RuntimeCore>()`, and
        // `align_of::<Runtime<S>>() >= align_of::<RuntimeCore>()`.
        unsafe { &mut *std::ptr::from_mut(self).cast::<RuntimeCore>() }
    }

    #[inline(always)]
    pub(crate) fn split(&self) -> (&RuntimeCore, AppRef<'_, S>) {
        assert_prefix_layout::<S>();
        let ptr = std::ptr::from_ref(self);
        // SAFETY: `Runtime<S>` and `RuntimeCore` share an identical `#[repr(C)]` prefix ending at
        // zero-sized `_core_end: CoreEnd`, and neither implements custom `Drop`. `assert_prefix_layout::<S>()`
        // proves `state` and `hooks` sit at offsets `>= size_of::<RuntimeCore>()`, so shared
        // references to the prefix (preserving any interior `UnsafeCell` bytes at identical offsets)
        // and the trailing fields may coexist.
        unsafe {
            let core = &*ptr.cast::<RuntimeCore>();
            let app = AppRef {
                state: &(*ptr).state,
                hooks: &(*ptr).hooks,
            };
            (core, app)
        }
    }

    /// Disjoint mutable borrow of the shared prefix and the `S`-dependent tail.
    ///
    /// As with [`Self::core_mut`], mutate fields on the returned `&mut RuntimeCore` rather than
    /// replacing the whole value.
    #[inline(always)]
    pub(crate) fn split_mut(&mut self) -> (&mut RuntimeCore, AppRefMut<'_, S>) {
        assert_prefix_layout::<S>();
        let ptr = std::ptr::from_mut(self);
        // SAFETY: `Runtime<S>` and `RuntimeCore` share an identical `#[repr(C)]` prefix ending
        // at zero-sized `_core_end: CoreEnd`, and neither implements custom `Drop`.
        // `assert_prefix_layout::<S>()` proves at compile time that `size_of::<CoreEnd>() == 0`,
        // `size_of::<RuntimeCore>() == offset_of!(RuntimeCore, _core_end)`,
        // `align_of::<Runtime<S>>() >= align_of::<RuntimeCore>()`, and every tail field (`state`,
        // `poll`, `on_key`, `hooks`) sits at an offset `>= size_of::<RuntimeCore>()`. Every
        // projection below derives from `ptr` and covers strictly disjoint byte ranges:
        // `[0..size_of::<RuntimeCore>())` for `core` and distinct tail fields at offsets
        // `>= size_of::<RuntimeCore>()` for `app`.
        unsafe {
            let poll = *std::ptr::addr_of!((*ptr).poll);
            let on_key = *std::ptr::addr_of!((*ptr).on_key);
            let state = &mut *std::ptr::addr_of_mut!((*ptr).state);
            let hooks = &mut *std::ptr::addr_of_mut!((*ptr).hooks);
            let core = &mut *ptr.cast::<RuntimeCore>();
            (core, AppRefMut { state, poll, on_key, hooks })
        }
    }

    /// Attaches a live playground to this runtime and keeps `has_playground` in sync.
    pub(crate) fn attach_playground(&mut self, live: crate::playground::Live<S>) {
        self.hooks.playground = Some(Box::new(live));
        self.tree.has_playground = true;
    }
}

#[cfg(test)]
mod prefix_layout_tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;

    #[repr(align(64))]
    struct OverAligned(u8);

    struct ZeroSized;

    struct DropCounter(Rc<Cell<usize>>);

    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    fn empty_view<S: 'static>(_: &S, _: &mut Cx<S>) -> El {
        crate::el()
    }

    fn verify_layout<S: 'static>(state: S, bump: impl FnOnce(&mut S), check: impl FnOnce(&S)) {
        assert_prefix_layout::<S>();
        assert_eq!(std::mem::size_of::<CoreEnd>(), 0);
        assert_eq!(std::mem::offset_of!(Runtime<S>, tree), std::mem::offset_of!(RuntimeCore, tree));
        assert_eq!(std::mem::offset_of!(Runtime<S>, hit), std::mem::offset_of!(RuntimeCore, hit));
        assert_eq!(std::mem::offset_of!(Runtime<S>, motion), std::mem::offset_of!(RuntimeCore, motion));
        assert_eq!(std::mem::offset_of!(Runtime<S>, scroll), std::mem::offset_of!(RuntimeCore, scroll));
        assert_eq!(std::mem::offset_of!(Runtime<S>, paint), std::mem::offset_of!(RuntimeCore, paint));
        assert_eq!(std::mem::offset_of!(Runtime<S>, pointer), std::mem::offset_of!(RuntimeCore, pointer));
        assert_eq!(std::mem::offset_of!(Runtime<S>, kbd), std::mem::offset_of!(RuntimeCore, kbd));
        assert_eq!(std::mem::offset_of!(Runtime<S>, commit), std::mem::offset_of!(RuntimeCore, commit));
        assert_eq!(std::mem::offset_of!(Runtime<S>, edit), std::mem::offset_of!(RuntimeCore, edit));
        assert_eq!(std::mem::offset_of!(Runtime<S>, frame), std::mem::offset_of!(RuntimeCore, frame));
        assert_eq!(std::mem::offset_of!(Runtime<S>, host), std::mem::offset_of!(RuntimeCore, host));
        assert!(std::mem::offset_of!(Runtime<S>, state) >= std::mem::size_of::<RuntimeCore>());
        assert!(std::mem::offset_of!(Runtime<S>, poll) >= std::mem::size_of::<RuntimeCore>());
        assert!(std::mem::offset_of!(Runtime<S>, on_key) >= std::mem::size_of::<RuntimeCore>());
        assert!(std::mem::offset_of!(Runtime<S>, hooks) >= std::mem::size_of::<RuntimeCore>());
        assert!(std::mem::align_of::<Runtime<S>>() >= std::mem::align_of::<RuntimeCore>());

        let mut rt = Runtime::new(state, empty_view::<S>, (320.0, 240.0), Env::new(true, 2.0));
        assert_eq!(rt.core().viewport, (320.0, 240.0));
        assert_eq!(rt.core().env.scale, 2.0);
        assert_eq!(rt.core().tree.next_id, 1);
        assert!(rt.core().commit.dirty);

        rt.core_mut().viewport = (400.0, 300.0);
        assert_eq!(rt.viewport, (400.0, 300.0));
        assert_eq!(rt.core().viewport, (400.0, 300.0));

        {
            let (core, app) = rt.split_mut();
            core.viewport = (640.0, 480.0);
            core.t = 1.5;
            bump(app.state);
        }
        assert_eq!(rt.viewport, (640.0, 480.0));
        assert_eq!(rt.t, 1.5);

        {
            let (core, app) = rt.split();
            assert_eq!(core.viewport, (640.0, 480.0));
            assert_eq!(core.t, 1.5);
            check(app.state);
        }

        // Reading through `split()` while an interior `RefCell` field of the core is mutably
        // borrowed must preserve `UnsafeCell` aliasing semantics.
        {
            let mut scratch = rt.motion.motion_scratch.borrow_mut();
            scratch.push((7, 3, true));
            let (core, app) = rt.split();
            assert_eq!(core.viewport, (640.0, 480.0));
            assert!(core.motion.motion_scratch.try_borrow().is_err());
            let _ = std::ptr::from_ref(app.state);
        }
        assert_eq!(rt.core().motion.motion_scratch.borrow().as_slice(), &[(7, 3, true)]);
    }

    #[test]
    fn shared_prefix_layout_holds_across_state_alignments() {
        verify_layout((), |_| {}, |_| {});

        let mut seen_u8 = 0u8;
        verify_layout(
            1u8,
            |s| { *s = 7; seen_u8 = *s; },
            |s| assert_eq!(*s, 7),
        );
        assert_eq!(seen_u8, 7);

        let mut seen_u128 = 0u128;
        verify_layout(
            1u128,
            |s| { *s = 0x1234_5678_9abc_def0; seen_u128 = *s; },
            |s| assert_eq!(*s, 0x1234_5678_9abc_def0),
        );
        assert_eq!(seen_u128, 0x1234_5678_9abc_def0);

        let mut seen_over = 0u8;
        verify_layout(
            OverAligned(3),
            |s| { s.0 = 42; seen_over = s.0; },
            |s| assert_eq!(s.0, 42),
        );
        assert_eq!(seen_over, 42);

        verify_layout(ZeroSized, |_| {}, |_| {});

        let drops = Rc::new(Cell::new(0));
        verify_layout(
            DropCounter(drops.clone()),
            |s| assert_eq!(s.0.get(), 0),
            |s| assert_eq!(s.0.get(), 0),
        );
        assert_eq!(drops.get(), 1, "state with Drop must drop exactly once");
    }
}

#[cfg(test)]
mod tests;
