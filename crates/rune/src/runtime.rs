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
use event_log::log_event;

#[inline(always)]
pub(crate) fn handler_state<S: 'static>(state: &mut S) -> &mut dyn Any {
    state
}

/// Calls an app's runtime callback (`poll`, `on_key`, `on_focus`, `on_mods`,
/// `on_visibility`, `on_ime`). With `rune/hot` it goes through subsecond's jump table,
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
    state
}

pub(crate) mod driver;
pub(crate) use driver::{AppDriver, AppRef, AppRefMut, RuntimeHooks, ViewDriver};

pub(crate) mod anchor;
mod api;
mod commit;
mod cx;
mod drag_out_host;
mod elements;
pub(crate) mod event_log;
mod font_families;
mod frame;
#[cfg(feature = "hot")]
mod hot_state;
mod init;
mod input;
mod interaction;
mod key_index;
pub(crate) mod layout_read;
mod layout_seed;
pub use layout_seed::LayoutSeed;
mod motion;
mod node_index;
use node_index::NodeIndex;
mod paint;
mod paint_probe;
mod pick;
mod playground;
mod scroll;
mod selection;
mod system_settings;
mod text_fill;
mod turn;
mod types;
mod virtual_list;
mod visible;
mod web;
mod window_requests;
mod zoom;

pub(crate) use commit::memo_reuse;
pub(crate) use commit::path as element_path;
pub(crate) use commit::text_layout::{caret_index, text_width};
pub(crate) use commit::why::CommitWhy;
#[cfg(all(test, debug_assertions))]
use commit::layout_diagnostics;
#[cfg(test)]
use commit::{prune, relayout, text_layout};
use commit::{Measured, Measurement, flatten, measure};
use commit::flatten::*;
use commit::text_layout::*;

#[cfg(test)]
pub(crate) use input::action;
pub(crate) use input::field_menu;
pub use input::help_look::HelpLook;
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(unused_imports))]
pub(crate) use input::physical::Unhandled;
pub(crate) use input::titlebar::WindowMenu;
#[cfg(target_os = "windows")]
pub(crate) use input::titlebar::{DragHit, drag_hit_at};
pub use input::touch::TouchPhase;
#[cfg_attr(not(target_arch = "wasm32"), allow(unused_imports))]
pub(crate) use input::touch::{TouchHoldCommand, TouchHoldPolicy};
#[cfg(any(target_os = "linux", test))]
use input::primary_selection;
use input::{
    app_ime, autoscroll, capture, drop as drop_dispatch, focus, focus_machine,
    gesture as gesture_dispatch, help, hit, hover, hover_delay, key_releases, modal_focus,
    modifier_keys, physical as physical_dispatch, popup as popup_dispatch, press_repeat, press_run,
    titlebar, touch as touch_input,
};
use input::action::*;
use input::pointer::CLICK_SLOP;

pub(crate) use motion::live_check::fold_live;
pub use motion::motion_elements::MotionElement;
#[cfg(test)]
use motion::live_check;
use motion::{content_slide, flights, growth_runs, timer_wake, tracks};
use motion::props::*;
use motion::reveal_phase::{Arrival, RevealChain, RevealEffect, RevealEvent, RevealPhase};
use motion::tracks::{SCROLL_DECAY, mark_ancestors, scroll_offsets};

pub(crate) use paint::blend::{apron_key, glyph, icon_inst, premul};
use paint::{
    border as border_paint, content as paint_content, exit_mask, keep as paint_keep, pixel_snap,
    raster_check, shadow as shadow_paint, slots as paint_slots, state as paint_state,
    tracks as paint_tracks,
};
use paint::cache_layer::{ActiveCache, AnotherFrame, NodeCache, RasterPass, RasterPhase, ScaleSettle};
use paint::helpers::*;
use paint::shape::push_shape;
use paint::soft_edges::{in_fade, inherit_fade};

pub(crate) use scroll::offset::{ScrollAnchor, ScrollRequest};
pub(crate) use scroll::reveal::ScrollReveal;
#[doc(hidden)]
pub use commit::path::{child_path, path_segment};
#[doc(hidden)]
pub use scroll::watch::scroll_watch_state;
#[cfg(not(target_os = "windows"))]
pub(crate) use scroll::x::LINE_POINTS;
pub(crate) use scroll::x::{SHIFT_TURNS_WHEEL, wheel_delta};
use scroll::{
    bottom as scroll_bottom, edge as scroll_edge, mode as scroll_mode, nudge as scroll_nudge,
    offset as scroll_offset, requests, sticky, touch as scroll_touch, watch as scroll_watch,
    wheel as scroll_wheel, x as scroll_x,
};
use scroll::drawn::PixelHold;
use scroll::mode::ScrollMode;

#[cfg(test)]
pub(crate) use selection::links::count_span_box_queries;
pub(crate) use selection::links::origin as text_origin;
pub(crate) use selection::selectable;
pub use selection::selectable::{SelectedText, SelectionChange, TextSelection};
pub use selection::text_marks::TextMark;
use selection::{caret, click as input_click, links, select_all, text_marks};

use virtual_list::{fill as virtual_fill, row_reorder};

pub(crate) use web::{a11y as web_a11y, export as web_export, page as web_page};
#[cfg(any(test, target_arch = "wasm32"))]
pub(crate) use web::runtime as web_runtime;

pub(crate) use anchor::anchor_position;
use anchor::{anchor_ancestors, anchor_index, anchor_index_live, step_up};
pub use cx::{Cx, Env, Waker, commit_generation};
pub(crate) use cx::caret_of;
use cx::COMMITS;
pub use elements::{ElementInfo, MatchedHandover, Pose, ResolvedTextStyle};
pub use pick::{Pick, Picked};
#[cfg(debug_assertions)]
pub(crate) use layout_read::{anchor_read_key, window_read_key};
pub(crate) use turn::intersect;
use turn::{Turn, compose_turn, open_turned_layer, screen_clip_of};
pub(crate) use types::{AnimSource, Node};
pub use types::{FrameStats, TitlebarAction};
use types::{AxisScroll, GlideTo, Inh, NodeCold, RowLayout, ScaleMotion, ScrollState, Stack, WidthHold, XMotion};
pub(crate) use window_requests::HostState;

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
mod aspect_ratio_tests;
#[cfg(test)]
mod anchor_fit_tests;
#[cfg(test)]
mod drop_tests;
#[cfg(test)]
mod elements_tests;
#[cfg(test)]
mod any_handlers_tests;
#[cfg(test)]
mod on_key_tests;
#[cfg(test)]
mod live_only_tests;
#[cfg(test)]
mod live_check_tests;
#[cfg(test)]
mod virtual_store_tests;
#[cfg(test)]
mod select_all_tests;
#[cfg(test)]
mod selection_api_tests;
#[cfg(test)]
mod anchored_grow_tests;
#[cfg(test)]
mod languages_tests;
#[cfg(test)]
mod layout_seed_tests;
#[cfg(test)]
mod pixel_snap_tests;
#[cfg(test)]
mod recorder_tests;
#[cfg(test)]
mod size_tests;
#[cfg(test)]
mod video_tests;

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
    /// Two presses closer than this (seconds) are a double press: the system setting on macOS,
    /// Windows and Linux (XDG Settings portal `org.gnome.desktop.peripherals.mouse`
    /// `double-click`, falling back to GSettings or `kdeglobals` `DoubleClickInterval`), 0.5
    /// elsewhere and headless.
    pub double_click_interval: f64,

    /// Paint scratch buffers, reuse record, layer caches, text quads and paint outputs.
    pub(crate) paint: paint_state::PaintState,
    /// Render-side growth stacks, reveals, flights, loops and rewind capture.
    pub(crate) motion: tracks::MotionState,
    /// Scroll viewports, wheel axis locking, scroll requests and sticky headers.
    pub(crate) scroll: requests::ScrollTables,
    pub scene: Scene,
    /// Action queue, pending `Tx`, wake deadlines, live latch, tracing and hot reload.
    pub(crate) frame: timer_wake::FrameState,
    /// View commit, Taffy layout, memo cache, virtual lists and layout callback state.
    pub(crate) commit: commit::CommitState,
    /// Pointer position, capture, hover/tooltip tracking, touch, autoscroll and file/OS drags.
    pointer: capture::PointerState,
    /// Keyboard focus, modal focus/scope, type-select, held keys and modifier tracking.
    pub(crate) kbd: focus_machine::KeyboardState,

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
    #[allow(unsafe_code)]
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
    #[allow(unsafe_code)]
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
    #[allow(unsafe_code)]
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
    #[allow(unsafe_code)]
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
        self.tree.attach_playground();
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
            let _ptr = std::ptr::from_ref(app.state);
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
