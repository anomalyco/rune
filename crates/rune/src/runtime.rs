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
#[path = "runtime/drag_out_host.rs"]
mod drag_out_host;
#[path = "runtime/paint/exit_mask.rs"]
mod exit_mask;
#[path = "runtime/paint/raster_check.rs"]
mod raster_check;
#[path = "runtime/font_families.rs"]
mod font_families;
#[path = "runtime/key_index.rs"]
mod key_index;
#[path = "runtime/system_settings.rs"]
mod system_settings;
#[path = "runtime/zoom.rs"]
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
#[path = "runtime/event_log.rs"]
pub(crate) mod event_log;
#[path = "runtime/layout_read.rs"]
pub(crate) mod layout_read;
#[path = "runtime/visible.rs"]
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
#[path = "runtime/anchor.rs"]
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
#[path = "runtime/hot_state.rs"]
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
#[path = "runtime/window_requests.rs"]
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
#[path = "runtime/timer_wake_tests.rs"]
mod timer_wake_tests;
#[cfg(test)]
#[path = "runtime/grid_tests.rs"]
mod grid_tests;
#[cfg(test)]
#[path = "runtime/truncate_tests.rs"]
mod truncate_tests;
#[cfg(test)]
#[path = "runtime/truncate_fade_tests.rs"]
mod truncate_fade_tests;
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
#[cfg(test)]
#[path = "runtime/live_check_tests.rs"]
mod live_check_tests;
#[cfg(test)]
#[path = "runtime/virtual_store_tests.rs"]
mod virtual_store_tests;
#[cfg(test)]
#[path = "runtime/virtual/fill_resume_tests.rs"]
mod fill_resume_tests;
#[path = "runtime/selection/select_all.rs"]
mod select_all;
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
#[path = "runtime/paint_probe.rs"]
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
#[path = "runtime/text_fill.rs"]
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
    ($($field:tt)*) => {
        /// Non-generic retained tree, layout, scene, input, scroll, selection, and paint state
        /// forming the exact `#[repr(C)]` prefix of [`Runtime<S>`].
        #[repr(C)]
        pub(crate) struct RuntimeCore {
            $($field)*
            pub(crate) _core_end: CoreEnd,
        }

        #[repr(C)]
        pub struct Runtime<S: 'static> {
            $($field)*
            pub(crate) _core_end: CoreEnd,
            pub state: S,
            /// Called at the start of every frame with the clock; return true when state changed (new data).
            pub poll: Option<fn(&mut S, f64, &mut Tx) -> bool>,
            /// Called for keys unhandled by the focused control; return true to consume it.
            pub on_key: Option<fn(&mut S, &KeyEvent, &mut Tx) -> bool>,
            pub(crate) hooks: RuntimeHooks<S>,
        }
    };
}

runtime_core_fields! {
    /// The modifiers `App::on_modifiers` last heard.
    mods_heard: crate::Mods,
    /// The composition `App::on_ime` currently has open.
    pub(crate) app_ime_comp: app_ime::AppComposition,
    /// The node index the app's view returned when the playground is attached.
    pub(crate) app_root: usize,
    /// Whether this runtime has an attached playground (`hooks.playground.is_some()`).
    pub(crate) has_playground: bool,
    #[cfg(feature = "hot")]
    pub(crate) hot: Option<crate::hot::Session>,
    pub text: TextSystem,
    /// RGBA images elements show (`El::pixels`), uploaded by the renderer.
    pub images: crate::pixels::ImageAtlas,
    pub scene: Scene,
    pub env: Env,
    /// Viewport in points: the view's, the window's over the zoom.
    pub viewport: (f32, f32),
    /// The view's UI zoom and the window it applies to (`Cx::zoom`).
    zoom: zoom::Zoom,
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
    /// What [`Cx::system_contrast`] reports.
    pub(crate) system_contrast: Option<crate::SystemContrast>,
    /// What [`Cx::system_increased_contrast`] reports.
    pub(crate) system_increased_contrast: bool,
    scroll_bars: system_settings::ScrollBarHost,
    pub time_scale: f32,
    /// The last view's `Cx::foreground` and `Cx::focus_ring`, for rows built after it and the
    /// rings painted until the next.
    defaults: cx::ViewDefaults,
    /// Seconds since the first frame, unscaled (virtual when headless).
    pub clock: f64,
    wake_deadlines: Vec<f64>,
    timer_wake: timer_wake::TimerWake,
    idle_meter: crate::tooling::perf_warn::IdleMeter,
    watch: crate::tooling::perf_warn::Watch,
    /// Windowed runs read the clock from the wall instead of summing frame steps.
    pub wall_start: Option<crate::time::Instant>,
    #[cfg(test)]
    pub(crate) test_clock: Option<crate::Clock>,
    pub(crate) nodes: Vec<Node>,
    /// The last commit's node buffer, emptied, for the next flatten to fill: an app of a few
    /// thousand elements otherwise maps and faults in megabytes every commit.
    spare_nodes: Vec<Node>,
    apply_scratch: apply_scratch::ApplyScratch,
    layout: LayoutEngine,
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
    /// The press the pointer holds, or the click a next press may pair with
    /// (`runtime/input/capture.rs`).
    press: capture::Press,
    /// The secondary button's press, held beside it.
    secondary: capture::Secondary,
    /// The middle or secondary button's pressed look (`capture::Tint`).
    tint: capture::Tint,
    /// Linux's primary selection (`runtime/input/primary_selection.rs`).
    #[cfg(target_os = "linux")]
    pub(crate) primary: primary_selection::PrimarySelection,
    /// The middle button's autoscroll (`runtime/input/autoscroll.rs`).
    autoscroll: autoscroll::Autoscroll,
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
    /// What a wheel chains up from, in paint order: what takes input, every scroll view and every
    /// painted surface (`Runtime::chain_start`).
    wheel_hits: Vec<(NodeId, [f32; 4])>,
    overlay_hits: Vec<[f32; 4]>,
    glyph_scratch: Vec<GlyphQuad>,
    /// What the last paint painted, for subtrees that would paint it again (`paint_keep`).
    paint_keep: paint_keep::PaintKeep,
    /// The device rows of each field with waiting paragraphs its last paint showed (`text_fill`).
    text_rows: HashMap<NodeId, (f32, f32)>,
    /// Each still text's quads as it drew them, by node and layout id, and the paint they were
    /// last used in (`paint_content`): a frame that draws it the same copies them.
    text_quads: HashMap<(NodeId, crate::text::LayoutId), (crate::el::grid::DrawnRow, u64)>,
    quads_frame: u64,
    /// Still texts that copied their quads, for tests.
    #[cfg(test)]
    quads_copied: u64,
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
    exit_mask: exit_mask::Mask,
    key_index: key_index::KeyIndex,
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
    /// Some element is `El::sticky_top` or `El::sticky_bottom` (see `runtime/scroll/sticky.rs`).
    has_sticky: bool,
    /// Each node's sticky shift this frame (points), while `has_sticky`.
    stuck: Vec<f32>,
    /// Per node, what its children stick within: the nearest vertical viewport's top and
    /// bottom, then the node's own content box top and bottom (unscaled points), while
    /// `has_sticky`.
    sticky_frame: Vec<[f32; 8]>,
    /// Sections whose rows fade under their stuck header this frame (`ScrollFade::below_sticky`),
    /// with the paint clip top the header keeps.
    sticky_bands: Vec<(usize, f32)>,
    /// `Tx::keep_on_screen` across the commit underway, and `Tx::scroll_into_view` requests that
    /// follow their target while growth/layout springs run.
    requests: requests::Requests,
    /// Some element is `El::raise`d: paint order isn't tree order.
    has_raise: bool,
    /// Some element sets `El::paint_order`: its siblings paint out of tree order.
    has_paint_order: bool,
    /// Key releases an input method takes, and the focus where the held non-IME key(s) went
    /// down, so a release after focus moves still reaches that element first.
    key_releases: key_releases::KeyReleases,
    /// Pictures whose image changed shape since the last commit, which lays them out at once.
    aspect_snap: Vec<NodeId>,
    /// Drawn rect per node (points, root space), for tools and hit tests.
    drawn: Vec<[f32; 4]>,
    /// The scale each node's text drew at (its own and its ancestors'), for text hit tests.
    drawn_scale: Vec<f32>,
    /// Per node: it drew inside its clip and the window in the last build.
    seen: Vec<bool>,
    scrolls: HashMap<NodeId, ScrollState>,
    /// The axis the current wheel or trackpad gesture scrolls (`runtime/scroll/x.rs`).
    wheel_lock: scroll_x::WheelLock,
    /// Whole notches of a mouse wheel ease where the platform's browsers do (`Runtime::set_wheel_glide`).
    wheel_glide: bool,
    /// The finger that is down and the viewport a touch holds (`runtime/input/touch.rs`).
    touch: touch_input::Touch,
    grows: HashMap<NodeId, GrowTrack>,
    stacks: HashMap<NodeId, Stack>,
    pub(crate) virtuals: HashMap<String, crate::el::virtual_list::VirtualList>,
    /// The recorder `Tx::record_rewind` attached (`runtime/motion/rewind.rs`).
    rewind: Option<crate::rewind::Rewind>,
    /// The memo pass the current commit's first view build was. Memos none of its builds used
    /// go once it applies.
    memo_since: u64,
    /// Whether an index-built list still fills its overscan, and for how many frames.
    virtual_fill: virtual_fill::Fill,
    /// True when the commit in flight is for scroll/layout only (`self.state` wasn't mutated).
    state_unchanged: bool,
    reuse: relayout::Reuse,
    pub(crate) virtual_culled: HashSet<String>,
    /// Fresh view accessors for focused text fields omitted by virtualization.
    virtual_inputs: HashMap<String, Box<InputSpec>>,
    reveals: HashMap<NodeId, RevealPhase>,
    /// The reveal states `reveals` keeps for texts that have left the tree, newest first, at
    /// most `prune::REVEALS_KEPT` of them (see `prune_exited`).
    reveals_kept: Vec<NodeId>,
    /// The newest streamed word of any text, which the next text's words queue behind.
    reveal_chain: RevealChain,
    flights: Vec<TextFlight>,
    /// Per node index this frame: presented row top dy, row height override, and bottom feather mask.
    row_dy: Vec<f32>,
    row_h: Vec<f32>,
    row_mask: Vec<f32>,
    /// Growth rows whose contents draw lifted this frame (`GrowTrack::hide_above`): index, points.
    row_lifts: Vec<(usize, f32)>,
    /// The header each lifted fold shuts under, by the fold's id, which draws where it's laid out.
    fold_headers: HashMap<NodeId, String>,
    /// What this frame's commits held at their landing width (`El::hold_landing_width`).
    landing_holds: landing_hold::LandingHolds,
    caught_up: f32,
    slack_dt: f32,
    /// What has focus, how it shows and where Tab starts (`runtime/input/focus_machine.rs`).
    focus: focus_machine::FocusState,
    /// Type-select's typed prefix and when its last key arrived (`now_clock`).
    type_select: focus_machine::TypeSelect,
    /// The activation key (Enter, Escape, Space) held down and the focus it was first pressed
    /// on. Its repeats are dropped once focus has moved, so a held key that closed something
    /// can't act again on whatever took focus next.
    held: physical_dispatch::Activation,
    /// Modifier keys down, for `Key::Modifier` (off unless the app opts in).
    modifier_keys: modifier_keys::ModifierKeys,
    /// Tab visits every control; off, only text fields and lists (macOS Full Keyboard
    /// Access off). Native hosts sample the system setting; headless runtimes default on.
    full_keyboard_access: bool,
    /// `App::tab_visits_controls`: Tab visits every control whatever `full_keyboard_access` says.
    tab_visits_controls: bool,
    /// The open modals and where focus returns as each closes (`modal_focus`).
    modal_focus: modal_focus::ModalFocus,
    /// Focus when the running commit began, before a new view's focus request applied: what a
    /// modal that opens in this commit returns focus to.
    focus_before_commit: Option<String>,
    /// `Tx::focus`: the key the next commit focuses, and whether its ring shows.
    focus_request: Option<(String, bool, crate::el::FocusAsk)>,
    /// `Tx::focus_when_shown` still waiting for its element: the key and whether its ring shows.
    focus_owed: Option<(String, bool)>,
    /// This commit's focus is a `Tx::return_focus` hand-back, which a modal opening in it
    /// doesn't take as where focus was.
    focus_returned: bool,
    /// Focused text fields that unmounted without being blurred: while another field holds focus,
    /// remounting one with its old `focused = true` flag does not steal focus back.
    unmounted_focused: Vec<std::sync::Weak<crate::edit::History>>,
    /// The innermost open modal this commit shows and what input may reach inside it, worked
    /// out from the nodes at each commit; none while no modal shows.
    modal_scope: Option<focus::ModalScope>,
    loops: HashMap<NodeId, rune_motion::LoopTrack>,
    loop_clock: loop_clock::LoopClock,
    loops_active: bool,
    /// Last scale observation per node that has a scale or had one. See [`ScaleMotion`].
    static_scales: HashMap<NodeId, ScaleSettle>,
    /// Frame requested by scale settling or pixel snapping in the last paint.
    scale_frame: Option<AnotherFrame>,
    /// Some node ran on a scaled clock (`El::time_scale`) at the last commit.
    time_scaled: bool,
    /// Shared host deadline: unscaled wake time and corresponding animation time.
    loop_wake: Option<(f64, f64)>,
    hovered: Option<String>,
    /// Elements with `on_hover_change` that the pointer is inside.
    hover_within: Vec<String>,
    /// `on_hover_change` handlers of elements that left the tree, owed their queued leave.
    hover_gone: Vec<(String, crate::el::WindowFocusFn)>,
    /// Elements with `hover_delays`: which heard the enter, and which wait out a delay.
    hover_delays: hover_delay::HoverDelays,
    /// A press or blur stopped the waits to report an enter; the pointer's next move restarts them.
    hover_paused: bool,
    /// Hover waiting out layout motion under a still pointer.
    hover_hold: hover::HoverHold,
    /// Files dragged over the window from another app (`drop_dispatch`).
    file_drag: drop_dispatch::FileDrag,
    pub ime_area: Option<[f32; 4]>,
    /// The `El::ime_cursor` that has the input method asks for Option as Alt (`El::ime_option_as_alt`).
    pub(crate) ime_option_as_alt: bool,
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
    /// Whether each element with `El::on_visible` was last on screen.
    visible_fired: HashMap<NodeId, bool>,
    /// The window's content top-left in global screen points, as its host last saw it.
    window_origin: (f32, f32),
    /// The window's content top-left in desktop pixels, as its host last saw it (`Drag::screen_point`).
    window_pixels: Option<(i32, i32)>,
    /// Whether layout feedback may still cause a commit (see `layout_read`).
    feedback: layout_read::Feedback,
    /// An edit ran this frame: keep the caret in its scroll viewport after the commit.
    caret_follow: bool,
    links: links::LinkState,
    /// The selection in `El::selectable` text.
    selectable: selectable::phase::TextSel,
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
    /// A handler or `update` asked to minimize the window (`Tx::minimize`); the shell takes it,
    /// after a show the same turn asked for (`window::Minimize`).
    pub(crate) minimize: crate::window::Minimize,
    /// Links handlers asked to open that this runtime left unopened (`Runtime::take_url_requests`).
    pub(crate) url_requests: Vec<String>,
    /// True when attached to a live window, overlay or web canvas, not a headless or scripted run.
    pub(crate) open_urls_natively: bool,
    /// Pop-up menus asked for (`Tx::popup_menu`) and not yet answered, oldest first.
    pub(crate) popups: popup_dispatch::Popups,
    /// The file dialog asked for (`Tx::open_file`, `Tx::save_file`) and not yet answered.
    pub(crate) file_dialog: crate::file_dialog::DialogState,
    /// File dialog callbacks, by id, until their answer runs.
    file_dialog_answers: Vec<(u64, crate::file_dialog::AnswerFn)>,
    next_file_dialog: u64,
    /// A frame change for the window (`Tx::resize_window`), with its spring resolved.
    pub(crate) window_frame: Option<crate::window::frame::FrameRequest>,
    /// The window's title (`Tx::set_title`), from `AppSpec`'s.
    pub(crate) window_title: window_requests::Title,
    /// `Tx::announce`'s last words, for the accessibility tree's live region.
    pub(crate) announcement: crate::a11y::Announcement,
    /// The window's height follows the root's natural height (`App::fit_content_height`).
    pub(crate) fit_height: bool,
    /// The height a window that fits its content last asked for, and the frame its move
    /// starts with. The host takes the hold as it starts the move.
    pub(crate) fit: window_requests::FitWindow,
    /// The band of a taller document the window draws (`App::web_document`).
    pub(crate) page: Option<web_page::Page>,
    /// The host's window frame isn't moving or about to: set by the host before each frame.
    pub(crate) window_resting: bool,
    /// What `update` asked of the next commit.
    pending: Tx,
    /// Motion has a cause (`Tx::unprompted`): a commit no input in this window caused mounts and
    /// unmounts at once. On for every window an app opens (`tooling::runtime_for`); a bare
    /// `Runtime` keeps every commit's motion.
    pub(crate) motion_needs_cause: bool,
    pub animating: bool,
    /// Why the last frame was animating, as [`AnimSource`] bits, for traces.
    pub(crate) anim_sources: u16,
    /// Why this frame committed, as [`commit_why::CommitWhy`] bits, until the frame log takes them.
    pub(crate) commit_why: u16,
    /// Each node's place in the paint order, kept by the frame build while `Some` (the motion
    /// gate's `z-order`).
    pub(crate) paint_rank: Option<Vec<u32>>,
    /// While `paint_rank` is kept: this frame's icons drawn through their transparent one-texel
    /// apron (`icon_inst` off the pixel grid), by atlas place (`apron_key`), for the gate's raster.
    pub(crate) icon_aprons: Vec<[u32; 3]>,
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
    live: live_latch::LiveLane,
    live_clock: Arc<()>,
    window_focused: Option<bool>,
    /// The focused spellchecked field's word at the caret (`spell::pause`).
    spell_watch: Option<crate::spell::pause::Watch>,
    /// `keys_lost` queued its `FocusLost` since the window last had focus.
    keys_lost: bool,
    live_subscriptions: Vec<Arc<crate::live::Subscription>>,
    /// Events whose handler said `Tx::live_only`, and so didn't commit.
    live_only_skips: u64,
    /// The clock at the last commit.
    last_commit_clock: f64,
    /// Debug builds check `Tx::live_only` events (`skip_view`).
    #[cfg(debug_assertions)]
    live_only_check: bool,
    /// The live-only event this frame's commit checks: its handler, and what the view had built.
    #[cfg(debug_assertions)]
    live_check: Option<(String, live_check::ViewPrint)>,
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
    /// The presses counted into `Drag::clicks`.
    press_clicks: press_run::PressRun,
    /// The held press's drag repeat (`Tx::drag_repeat_after`).
    press_repeat: press_repeat::PressRepeat,
    titlebar: titlebar::Titlebar,
    /// Two presses closer than this (seconds) are a double press: the system setting on macOS
    /// and Windows, 0.5 elsewhere and headless.
    pub double_click_interval: f64,
    /// How long (seconds) the pointer rests on an `El::help` element before its tooltip shows:
    /// the system's hover time on Windows, 0.5 elsewhere and headless. macOS windows use
    /// AppKit's own tooltips, unless the view sets its own look (`Cx::help_look`), whose delay
    /// may replace this one.
    pub help_delay: f64,
    /// The host starts OS drags, so `drag_out` elements start one (headless tools act as one).
    os_drag: bool,
    drag_out_request: Option<crate::drag_out::DragOutStart>,
    /// How OS drags ended, filled by the host's session callbacks between frames.
    drag_ends: crate::drag_out::DragEnds,
    /// Drag-outs whose `on_drag_start` ran and whose `on_drag_end` hasn't yet.
    drags_open: usize,
    /// The accessibility tree, while an assistive client listens (`a11y/tree.rs`).
    a11y: Option<Box<a11y_tree::A11yTree>>,
}

#[inline(always)]
const fn assert_prefix_layout<S: 'static>() {
    const {
        assert!(std::mem::align_of::<RuntimeCore>() <= 16);
        assert!(std::mem::offset_of!(Runtime<S>, _core_end) == std::mem::offset_of!(RuntimeCore, _core_end));
        assert!(std::mem::size_of::<RuntimeCore>() == std::mem::offset_of!(RuntimeCore, _core_end));
        assert!(std::mem::offset_of!(Runtime<S>, state) >= std::mem::size_of::<RuntimeCore>());
        assert!(std::mem::align_of::<Runtime<S>>() >= std::mem::align_of::<RuntimeCore>());
    }
}

impl<S: 'static> Runtime<S> {
    pub fn new(state: S, view: fn(&S, &mut Cx<S>) -> El, viewport: (f32, f32), env: Env) -> Self {
        Self::with_text(state, view, viewport, env, TextSystem::default())
    }

    pub fn with_text(state: S, view: fn(&S, &mut Cx<S>) -> El, viewport: (f32, f32), env: Env, text: TextSystem) -> Self {
        Self::from_core(RuntimeCore::new(viewport, env, text), state, view)
    }

    pub(crate) fn from_core(core: RuntimeCore, state: S, view: fn(&S, &mut Cx<S>) -> El) -> Self {
        assert_prefix_layout::<S>();
        let hooks = RuntimeHooks::new(view);
        let mut uninit = std::mem::MaybeUninit::<Self>::uninit();
        let ptr = uninit.as_mut_ptr();
        // SAFETY: `Runtime<S>` and `RuntimeCore` are `#[repr(C)]` with an identical field prefix
        // ending at `_core_end: CoreEnd`, verified at compile time by `assert_prefix_layout::<S>()`.
        // `ptr.cast::<RuntimeCore>().write(core)` initializes the entire shared prefix, and the
        // four trailing `write` calls initialize every remaining field (`state`, `poll`, `on_key`,
        // `hooks`). None of these writes can panic, so `assume_init()` returns a fully initialized
        // `Runtime<S>` with no custom `Drop` mismatch.
        unsafe {
            ptr.cast::<RuntimeCore>().write(core);
            std::ptr::addr_of_mut!((*ptr).state).write(state);
            std::ptr::addr_of_mut!((*ptr).poll).write(None);
            std::ptr::addr_of_mut!((*ptr).on_key).write(None);
            std::ptr::addr_of_mut!((*ptr).hooks).write(hooks);
            uninit.assume_init()
        }
    }

    #[inline(always)]
    pub(crate) fn core(&self) -> &RuntimeCore {
        assert_prefix_layout::<S>();
        // SAFETY: `Runtime<S>` begins with the identical `#[repr(C)]` prefix of `RuntimeCore`,
        // with alignment >= `align_of::<RuntimeCore>()` proven at compile time by
        // `assert_prefix_layout::<S>()`.
        unsafe { &*std::ptr::from_ref(self).cast::<RuntimeCore>() }
    }

    #[inline(always)]
    pub(crate) fn core_mut(&mut self) -> &mut RuntimeCore {
        assert_prefix_layout::<S>();
        // SAFETY: `Runtime<S>` begins with the identical `#[repr(C)]` prefix of `RuntimeCore`,
        // with alignment >= `align_of::<RuntimeCore>()` proven at compile time by
        // `assert_prefix_layout::<S>()`.
        unsafe { &mut *std::ptr::from_mut(self).cast::<RuntimeCore>() }
    }

    #[inline(always)]
    pub(crate) fn split(&self) -> (&RuntimeCore, AppRef<'_, S>) {
        assert_prefix_layout::<S>();
        let ptr = std::ptr::from_ref(self);
        // SAFETY: `Runtime<S>` begins with the identical `#[repr(C)]` prefix of `RuntimeCore`,
        // and shared references to the prefix and trailing fields may coexist.
        unsafe {
            let core = &*ptr.cast::<RuntimeCore>();
            let app = AppRef {
                state: &(*ptr).state,
                hooks: &(*ptr).hooks,
            };
            (core, app)
        }
    }

    #[inline(always)]
    pub(crate) fn split_mut(&mut self) -> (&mut RuntimeCore, AppRefMut<'_, S>) {
        assert_prefix_layout::<S>();
        let ptr = std::ptr::from_mut(self);
        // SAFETY: `Runtime<S>` and `RuntimeCore` share an identical `#[repr(C)]` prefix ending
        // at `_core_end: CoreEnd`. `assert_prefix_layout::<S>()` proves at compile time that
        // `offset_of!(Runtime<S>, state) >= size_of::<RuntimeCore>()` and
        // `align_of::<Runtime<S>>() >= align_of::<RuntimeCore>()`. Every projection below derives
        // once from `ptr` and covers strictly disjoint byte ranges: `[0..size_of::<RuntimeCore>())`
        // for `core` and `[offset_of!(Runtime<S>, state)..size_of::<Runtime<S>>())` for `app`.
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
        self.has_playground = true;
    }
}

#[cfg(test)]
mod prefix_layout_tests {
    use super::*;

    #[repr(align(64))]
    struct OverAligned(u8);

    fn empty_view<S: 'static>(_: &S, _: &mut Cx<S>) -> El {
        crate::el()
    }

    fn verify_layout<S: 'static>(state: S, bump: impl FnOnce(&mut S)) {
        assert_prefix_layout::<S>();
        assert!(std::mem::offset_of!(Runtime<S>, state) >= std::mem::size_of::<RuntimeCore>());
        assert!(std::mem::align_of::<Runtime<S>>() >= std::mem::align_of::<RuntimeCore>());
        let mut rt = Runtime::new(state, empty_view::<S>, (320.0, 240.0), Env::new(true, 2.0));
        assert_eq!(rt.core().viewport, (320.0, 240.0));
        assert_eq!(rt.core().env.scale, 2.0);
        {
            let (core, app) = rt.split_mut();
            core.viewport = (640.0, 480.0);
            core.t = 1.5;
            bump(app.state);
        }
        assert_eq!(rt.viewport, (640.0, 480.0));
        assert_eq!(rt.t, 1.5);
        let (core, _) = rt.split();
        assert_eq!(core.viewport, (640.0, 480.0));
    }

    #[test]
    fn shared_prefix_layout_holds_across_state_alignments() {
        verify_layout((), |_| {});
        let mut seen_u8 = 0u8;
        verify_layout(1u8, |s| { *s = 7; seen_u8 = *s; });
        assert_eq!(seen_u8, 7);
        let mut seen_u128 = 0u128;
        verify_layout(1u128, |s| { *s = 0x1234_5678_9abc_def0; seen_u128 = *s; });
        assert_eq!(seen_u128, 0x1234_5678_9abc_def0);
        let mut seen_over = 0u8;
        verify_layout(OverAligned(3), |s| { s.0 = 42; seen_over = s.0; });
        assert_eq!(seen_over, 42);
    }
}

#[cfg(test)]
#[path = "runtime/tests/mod.rs"]
mod tests;
