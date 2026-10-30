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
mod keys;
mod pointer;
use pointer::CLICK_SLOP;
mod live_check;
pub(crate) use live_check::fold_live;
mod frame;
mod cx;
pub use cx::{Cx, Env, Waker, commit_generation};
pub(crate) use cx::caret_of;
use cx::COMMITS;

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

#[cfg(test)]
#[path = "runtime/tests/mod.rs"]
mod tests;
