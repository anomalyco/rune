//! The retained tree: view → keyed nodes → Taffy layout at the target state → `rune-motion` Scene.
//! Animation samples tracks at frame time; it never re-runs view functions.
//!
//! Beside Scene's tracks the runtime owns four render-side behaviours that never touch app code:
//! scroll viewports (offset, momentum, follow-the-end), growth stacks (row heights on a capped
//! spring, positions derived), streaming word reveals, and text-field carets. Each frame:
//! queued input → (commit if state changed) → Scene → growth → scroll → reveals → build.

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::Arc;

use parley::Layout;
use parley::editing::{Cursor, Selection};
use parley::layout::Affinity;
use rune_motion::{
    Anim, Corners, LayoutAnim, Mount, NodeId, NodeSpec, Phase as PresencePhase, Prop, PropClass, Rect as MRect, Scene,
    Spring, SpringPair, Transition, prop, snap::grid,
};
use taffy::prelude::*;
use taffy::TraversePartialTree;

use crate::color::{Color, over};
use crate::edit::{EditOutcome, Ime, KeyEvent};
use crate::el::{Align, Drag, DragFn, El, GrowthSpec, InputSpec, Phase, PointerPress, PressFn, SecondaryFn, ScrollSpec, Shell, TextSpec, Tx, Visual};
use crate::growth::{GrowTrack, SUBSTEP};
use crate::flight::{TextFlight, transformed};
use crate::render::{Instance, KIND_GLYPH, KIND_GRADIENT, KIND_IMAGE, KIND_INSET, KIND_RECT, KIND_SEGMENT, KIND_SHADOW, KIND_TRIANGLE, NO_CLIP};
use crate::shape::{LineCap, Op, Shape};
use crate::text::{Deco, GlyphQuad, Ink, TextSystem, layout_height};
use crate::tok::motion;

#[path = "focus.rs"]
mod focus;

const P_FILL: Prop<Color> = Prop::custom(64, PropClass::Color);
const P_BORDER: Prop<Color> = Prop::custom(65, PropClass::Color);
const P_TEXT: Prop<Color> = Prop::custom(66, PropClass::Color);
const P_HOVER_C: Prop<Color> = Prop::custom(67, PropClass::Color);
const P_PRESS_C: Prop<Color> = Prop::custom(68, PropClass::Color);
const P_SHADOWS: [Prop<Color>; 4] = [
    Prop::custom(69, PropClass::Color),
    Prop::custom(70, PropClass::Color),
    Prop::custom(71, PropClass::Color),
    Prop::custom(72, PropClass::Color),
];
const P_HOVER: Prop<f32> = Prop::custom(73, PropClass::Opacity);
const P_PRESS: Prop<f32> = Prop::custom(74, PropClass::Opacity);
const P_GRADIENT: [Prop<Color>; 2] = [Prop::custom(75, PropClass::Color), Prop::custom(76, PropClass::Color)];
const P_OUTLINE: Prop<Color> = Prop::custom(77, PropClass::Color);

/// Scroll momentum decay (iOS normal: 0.998 per ms).
const SCROLL_DECAY: f32 = 2.002;

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

/// Passed to view functions: the environment, plus runtime settings a view may declare.
pub struct Cx<S> {
    env: Env,
    system_reduced_motion: bool,
    reduced_motion: bool,
    time_scale: f32,
    _s: PhantomData<fn(&S)>,
}

impl<S> Cx<S> {
    pub fn env(&self) -> Env {
        self.env
    }
    pub fn system_dark(&self) -> bool {
        self.env.system_dark
    }
    /// The host's OS accessibility preference, independent of this commit's override.
    pub fn system_reduced_motion(&self) -> bool {
        self.system_reduced_motion
    }
    /// Override the host preference for this commit: travel jumps, opacity/colour briefly fade.
    pub fn reduced_motion(&mut self, on: bool) {
        self.reduced_motion = on;
    }
    /// Scale the animation clock (slow motion below 1).
    pub fn time_scale(&mut self, scale: f32) {
        self.time_scale = scale.clamp(0.0, 8.0);
    }
}

pub(crate) struct Node {
    on_paste: Option<crate::el::PasteFn>,
    on_window_focus: Option<crate::el::WindowFocusFn>,
    focus: crate::el::FocusSpec,
    modal: Option<crate::el::ModalSpec>,
    looping: Option<rune_motion::LoopSpec>,
    pub id: NodeId,
    pub path: String,
    pub key: Option<String>,
    pub role: &'static str,
    pub label: Option<String>,
    pub vis: Visual,
    live: Vec<crate::live::Bound>,
    pub text: Option<TextSpec>,
    pub text_layout: Option<Rc<Layout<Ink>>>,
    live_text_seen: Option<u128>,
    on_press: Option<PressFn>,
    on_secondary_press: Option<SecondaryFn>,
    on_hover: Option<PressFn>,
    on_pointer_move: Option<SecondaryFn>,
    above: Option<crate::el::Anchor>,
    on_drag: Option<DragFn>,
    motion: Spring,
    opacity_motion: Option<rune_motion::Anim>,
    lead: Option<(Spring, Spring)>,
    width_leads: bool,
    transition: Transition,
    scroll: Option<ScrollSpec>,
    growth: Option<GrowthSpec>,
    streaming: bool,
    input: Option<Box<InputSpec>>,
    cursor: Option<crate::el::Cursor>,
    window_drag: bool,
    /// Inside a growth row: layout snaps (the row's height carries the motion).
    in_row: bool,
    virtual_mount: bool,
    pub parent: Option<usize>,
    taffy: taffy::NodeId,
    /// Layout target in root space: x0, y0, x1, y1 in points.
    pub target: [f32; 4],
    /// Layout target in parent space.
    pub local: MRect,
    /// Padding plus border (left, top, right, bottom): text draws inside it.
    inset: [f32; 4],
}

enum Action {
    WindowFocus(bool),
    FocusControl(String),
    Dismiss(String),
    PointerMove(String, PointerPress),
    Hover(String),
    Press(String),
    SecondaryPress(String, PointerPress),
    Drag(String, Drag),
    Key(KeyEvent),
    Ime(Ime),
    Paste(zeroize::Zeroizing<String>, Option<Option<String>>),
    Focus(String, f32, f32),
    Blur,
}

/// What tools see of one element.
#[derive(Clone, Debug)]
pub struct ElementInfo {
    pub loop_phase: Option<f64>,
    pub focusable: bool,
    pub focused: bool,
    pub focus_visible: bool,
    pub inert: bool,
    pub path: String,
    pub key: Option<String>,
    pub role: &'static str,
    pub label: Option<String>,
    pub text: Option<String>,
    pub rect: [f32; 4],
    pub target: [f32; 4],
    pub opacity: f32,
    pub radius: f32,
    pub fill: Color,
    pub text_color: Option<Color>,
    pub hover: f32,
    pub press: f32,
    pub moving: bool,
    pub parent: Option<usize>,
    /// The part of `rect` its ancestors' clips leave visible.
    pub visible: [f32; 4],
    /// Words of this text are still fading in.
    pub revealing: bool,
    /// Inside a growth row whose height is moving: its visible part is being revealed.
    pub in_growing_row: bool,
    /// This node is riding user-driven wheel/trackpad scrolling or its momentum.
    pub scrolling: bool,
    /// Invisible estimated extent of a virtual list, not a drawable surface.
    pub virtual_extent: bool,
    /// Property-specific intentional cuts for 1:1 live bindings (never smoothed bindings).
    pub live_intent: u32,
}

#[derive(Clone, Copy)]
struct Inh {
    opacity: f32,
    paint_opacity: f32,
    clip: [f32; 4],
    fade: [f32; 4],
    fade_origin: [f32; 4],
    scale: f32,
    anchor: [f32; 2],
    blur: f32,
    /// Vertical offset in points from scroll viewports and growth rows above.
    dy: f32,
    mark: u32,
    /// The nearest hover group's hover amount (0–1), for `group_*` styles.
    group: f32,
}

impl Default for Inh {
    fn default() -> Self {
        Inh { opacity: 1.0, paint_opacity: 1.0, clip: NO_CLIP, fade: [0.0; 4], fade_origin: NO_CLIP, scale: 1.0, anchor: [0.0; 2], blur: 0.0, dy: 0.0, mark: 0, group: 0.0 }
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
    width: f32,
    /// Measure at the landing width immediately; defer height targets until it is stable.
    width_hold_until: f64,
    width_shrink_until: f64,
    pending_heights: HashMap<NodeId, f32>,
}

struct LayoutEntry {
    id: taffy::NodeId,
    measure: Option<u64>,
    used: bool,
}

pub struct Runtime<S: 'static> {
    pub state: S,
    view: fn(&S, &mut Cx<S>) -> El,
    pub text: TextSystem,
    /// RGBA images elements show (`El::pixels`), uploaded by the renderer.
    pub images: crate::pixels::ImageAtlas,
    pub scene: Scene,
    pub env: Env,
    /// Viewport in points.
    pub viewport: (f32, f32),
    /// Transparent native surfaces/headless exports clear to transparent black.
    pub transparent: bool,
    pub(crate) native_overlay: bool,
    /// Animation time (seconds, scaled by the time scale).
    pub t: f64,
    pub reduced_motion: bool,
    system_reduced_motion: bool,
    pub time_scale: f32,
    /// Called at the start of every frame with the clock; return true when state changed (new data).
    pub poll: Option<fn(&mut S, f64, &mut Tx) -> bool>,
    /// Seconds since the first frame, unscaled (virtual when headless).
    pub clock: f64,
    #[cfg(all(feature = "dev", debug_assertions))]
    pub(crate) dev_clock: Option<crate::dev_clock::DevClock>,
    #[cfg(all(feature = "dev", debug_assertions))]
    pub(crate) dev: Option<crate::dev::Session<S>>,
    wake_deadlines: Vec<f64>,
    /// Windowed runs read the clock from the wall instead of summing frame steps.
    pub wall_start: Option<std::time::Instant>,
    /// Called for keys unhandled by the focused control; return true to consume it.
    pub on_key: Option<fn(&mut S, &KeyEvent, &mut Tx) -> bool>,
    nodes: Vec<Node>,
    layout: TaffyTree<usize>,
    layout_entries: HashMap<String, LayoutEntry>,
    node_index: HashMap<NodeId, usize>,
    path_ids: HashMap<String, NodeId>,
    next_id: u64,
    commits: u64,
    dirty: bool,
    relayout: bool,
    settle: bool,
    queue: Vec<Action>,
    pointer: Option<(f32, f32)>,
    capture: Option<(String, Drag)>,
    secondary_capture: Option<String>,
    capture_inside: bool,
    drag_data: f32,
    pub instances: Vec<Instance>,
    /// Per instance, the `El::mark` of its nearest marked node (0 for none). Filled only while
    /// `track_marks` is set (a compositor is attached).
    pub marks: Vec<u32>,
    pub track_marks: bool,
    pub surfaces: Vec<crate::native_surface::SurfaceDraw>,
    hits: Vec<(NodeId, [f32; 4])>,
    overlay_hits: Vec<[f32; 4]>,
    glyph_scratch: Vec<GlyphQuad>,
    deco_scratch: Vec<Deco>,
    inh_scratch: Vec<Inh>,
    /// Only nonempty when retained exit nodes make storage order differ from subtree order.
    layer_paint_order: Vec<usize>,
    layer_scratch: Vec<(usize, usize, usize)>,
    /// Sideways scroll of focused single-line fields, in points, keyed by node path.
    input_scroll: HashMap<String, f32>,
    /// Drawn rect per node (points, root space), for tools and hit tests.
    drawn: Vec<[f32; 4]>,
    scrolls: HashMap<NodeId, ScrollState>,
    grows: HashMap<NodeId, GrowTrack>,
    stacks: HashMap<NodeId, Stack>,
    virtuals: HashMap<String, crate::virtual_list::VirtualList>,
    virtual_culled: HashSet<String>,
    /// Fresh view accessors for focused text fields omitted by virtualization.
    virtual_inputs: HashMap<String, Box<InputSpec>>,
    reveals: HashMap<NodeId, RevealState>,
    flights: Vec<TextFlight>,
    /// Per node index this frame: presented row top dy, row height override, and bottom feather mask.
    row_dy: Vec<f32>,
    row_h: Vec<f32>,
    row_mask: Vec<f32>,
    grow_acc: f32,
    focused: Option<String>,
    keyboard_focus: bool,
    modal_stack: Vec<(String, Option<String>)>,
    modal_index: Option<usize>,
    modal_members: Vec<bool>,
    loops: HashMap<NodeId, rune_motion::LoopTrack>,
    loops_active: bool,
    /// Shared host deadline: unscaled wake time and corresponding animation time.
    loop_wake: Option<(f64, f64)>,
    surface_visible: bool,
    hovered: Option<String>,
    pub ime_area: Option<[f32; 4]>,
    /// The shell cancels native composition before enabling the new text focus.
    pub(crate) ime_reset: bool,
    /// Text the focused field asked to copy (the shell writes it to the clipboard).
    pub copy_request: Option<crate::ClipboardWrite>,
    pub(crate) paste_request: bool,
    /// A handler or `update` asked to close the window or quit (the shell takes it).
    pub shell_request: Option<Shell>,
    /// What `update` asked of the next commit.
    pending: Tx,
    pub animating: bool,
    /// This frame's commit landed every track (`Tx::settle`): a deliberate cut.
    pub settled: bool,
    /// Events since the last frame, for traces.
    pub events: Vec<String>,
    /// Counters for perf gates.
    pub stats: FrameStats,
    live_wake: Arc<crate::live::Wake>,
    live_clock: Arc<()>,
    window_focused: Option<bool>,
    live_subscriptions: Vec<Arc<crate::live::Subscription>>,
    live_dirty: bool,
    /// Opt-in tooling checks. None keeps release commits free of key traversal/allocation.
    pub(crate) key_warnings: Option<Vec<String>>,
    /// Seconds until the next frame while only [`Cycle`](crate::el::Cycle)s move on screen
    /// (the shell sleeps until then instead of drawing every display frame).
    pub ambient: Option<f32>,
    /// Where (window points) the current press started.
    press_at: (f32, f32),
    /// The last press on a window drag region: when (clock seconds) and where.
    last_drag_press: Option<(f64, f32, f32)>,
    titlebar_action: Option<TitlebarAction>,
    /// Two presses on a drag region closer than this (seconds) are a double press.
    pub double_click_interval: f64,
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
        Runtime {
            state,
            view,
            text,
            images: Default::default(),
            scene,
            env,
            viewport,
            transparent: false,
            native_overlay: false,
            t: 0.0,
            reduced_motion: false,
            system_reduced_motion: false,
            time_scale: 1.0,
            poll: None,
            clock: 0.0,
            #[cfg(all(feature = "dev", debug_assertions))]
            dev_clock: None,
            #[cfg(all(feature = "dev", debug_assertions))]
            dev: None,
            wake_deadlines: Vec::new(),
            wall_start: None,
            on_key: None,
            nodes: Vec::new(),
            layout: {
                let mut tree = TaffyTree::new();
                tree.disable_rounding();
                tree
            },
            layout_entries: HashMap::new(),
            node_index: HashMap::new(),
            path_ids: HashMap::new(),
            next_id: 1,
            commits: 0,
            dirty: true,
            relayout: false,
            settle: false,
            queue: Vec::new(),
            pointer: None,
            capture: None,
            secondary_capture: None,
            capture_inside: false,
            drag_data: 0.0,
            instances: Vec::new(),
            marks: Vec::new(),
            track_marks: false,
            surfaces: Vec::new(),
            hits: Vec::new(),
            overlay_hits: Vec::new(),
            glyph_scratch: Vec::new(),
            deco_scratch: Vec::new(),
            inh_scratch: Vec::new(),
            layer_paint_order: Vec::new(),
            layer_scratch: Vec::new(),
            input_scroll: HashMap::new(),
            drawn: Vec::new(),
            scrolls: HashMap::new(),
            grows: HashMap::new(),
            stacks: HashMap::new(),
            virtuals: HashMap::new(),
            virtual_culled: HashSet::new(),
            virtual_inputs: HashMap::new(),
            reveals: HashMap::new(),
            flights: Vec::new(),
            row_dy: Vec::new(),
            row_h: Vec::new(),
            row_mask: Vec::new(),
            grow_acc: 0.0,
            focused: None,
            keyboard_focus: false,
            modal_stack: Vec::new(),
            modal_index: None,
            modal_members: Vec::new(),
            loops: HashMap::new(),
            loops_active: false,
            loop_wake: None,
            surface_visible: true,
            hovered: None,
            ime_area: None,
            ime_reset: false,
            copy_request: None,
            paste_request: false,
            shell_request: None,
            pending: Tx::default(),
            animating: false,
            settled: false,
            events: Vec::new(),
            stats: FrameStats::default(),
            live_wake: crate::live::Wake::new(Arc::new(|| {})),
            live_clock: Arc::new(()),
            window_focused: None,
            live_subscriptions: Vec::new(),
            live_dirty: false,
            key_warnings: None,
            ambient: None,
            press_at: (0.0, 0.0),
            last_drag_press: None,
            titlebar_action: None,
            double_click_interval: 0.5,
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
        if viewport != self.viewport || scale != self.env.scale {
            if scale != self.env.scale { self.flights.clear(); }
            self.viewport = viewport;
            self.env.scale = scale;
            self.scene.set_pixel_scale(scale);
            self.relayout = true;
        }
    }

    /// Install before the first frame. Live updates use this separate coalesced wake lane.
    pub fn set_live_waker(&mut self, waker: Waker) {
        for sub in &self.live_subscriptions { sub.active.store(false, std::sync::atomic::Ordering::Release); }
        self.live_subscriptions.clear();
        self.live_wake = crate::live::Wake::new(waker);
        self.live_wake.visible(self.surface_visible);
        self.subscribe_live();
    }

    pub fn set_live_visible(&mut self, visible: bool) {
        self.live_wake.visible(visible);
        self.surface_visible = visible;
        if !visible { self.loop_wake = None; }
    }
    pub fn live_pending(&self) -> bool {
        self.surface_visible && (self.live_dirty || self.live_wake.pending.load(std::sync::atomic::Ordering::Acquire))
    }

    fn subscribe_live(&mut self) {
        use std::sync::atomic::Ordering;
        for node in &mut self.nodes {
            for bound in &mut node.live { bound.attach(&self.live_clock, self.clock); }
        }
        for sub in &self.live_subscriptions { sub.active.store(false, Ordering::Release); }
        for node in &self.nodes {
            if self.is_exiting(node.id) { continue; }
            let text = node.text.as_ref().and_then(|t| t.live.as_ref()).map(|t| t.source());
            for source in node.live.iter().map(|b| &b.source).chain(text) {
                if let Some(sub) = self.live_subscriptions.iter().find(|s| s.source_id == source.id()) {
                    sub.active.store(true, Ordering::Release);
                } else {
                    self.live_subscriptions.push(source.subscribe(&self.live_wake));
                }
            }
        }
        self.live_subscriptions.retain(|s| s.active.load(Ordering::Acquire));
        self.live_dirty = !self.live_subscriptions.is_empty();
    }

    fn latch_live(&mut self) -> bool {
        if !self.surface_visible { return false; }
        // Clear before sampling: a concurrent publication either participates in this frame or
        // retains a pending wake for the next one. Never clear after sampling (lost wake race).
        let pending = self.live_wake.pending.swap(false, std::sync::atomic::Ordering::AcqRel);
        if self.live_subscriptions.is_empty() { return false; }
        if !pending && !self.live_dirty { return false; }
        self.live_dirty = false;
        for node in &mut self.nodes {
            if let Some(spec) = &mut node.text
                && let Some(live) = &spec.live
                && let Some(text) = live.read(&mut node.live_text_seen)
                && spec.content != *text
            {
                spec.content = text.to_string();
                node.text_layout = Some(self.text.layout_live(&text, spec.style, self.env.scale));
            }
            for bound in &mut node.live {
                let initial = bound.seen.is_none();
                let (bits, active) = bound.read(&self.live_clock, self.clock);
                self.live_dirty |= active;
                let Some(bits) = bits else { continue };
                let spring = if initial { None } else { bound.spring };
                use crate::live::{LiveValue, Property};
                fn apply<T: rune_motion::Animatable>(scene: &mut Scene, id: NodeId, prop: Prop<T>, value: T, spring: Option<Spring>) {
                    if let Some(spring) = spring { scene.animate(id, prop, value, spring); }
                    else { scene.set(id, prop, value); }
                }
                match bound.property {
                    Property::Opacity => apply(&mut self.scene, node.id, prop::OPACITY, f32::decode(bits).clamp(0.0, 1.0), spring),
                    Property::Scale => apply(&mut self.scene, node.id, prop::SCALE, f32::decode(bits).max(0.0), spring),
                    Property::Offset => {
                        let [x, y] = <[f32; 2]>::decode(bits);
                        apply(&mut self.scene, node.id, prop::OFFSET, rune_motion::Vec2::new(x, y), spring);
                    }
                    Property::Fill => {
                        // The renderer uses the exact authored colour after its track settles.
                        node.vis.fill = Color::decode(bits);
                        apply(&mut self.scene, node.id, P_FILL, node.vis.fill, spring);
                    }
                    Property::Rect => {
                        let [x, y, w, h] = <[f32; 4]>::decode(bits);
                        self.scene.set_frame(node.id, MRect::new(x, y, w.max(0.0), h.max(0.0)),
                            spring.map_or(LayoutAnim::INSTANT, LayoutAnim::new));
                    }
                }
            }
        }
        self.live_dirty
    }

    pub fn set_system_dark(&mut self, dark: bool) {
        if dark != self.env.system_dark {
            self.env.system_dark = dark;
            self.dirty = true;
        }
    }

    pub fn set_system_reduced_motion(&mut self, reduced: bool) {
        if self.system_reduced_motion != reduced {
            self.system_reduced_motion = reduced;
            self.dirty = true;
        }
    }
    /// Host accessibility preference; false by default in a headless runtime.
    pub fn system_reduced_motion(&self) -> bool {
        self.system_reduced_motion
    }

    /// Queue a native focus transition. Repeated notifications do not repeat the callback.
    pub fn window_focus(&mut self, focused: bool) {
        if self.window_focused != Some(focused) {
            self.window_focused = Some(focused);
            self.queue.push(Action::WindowFocus(focused));
        }
    }

    /// Marks app state changed outside a handler (the next frame re-runs the view).
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    /// Whether a compositor should apply its optional rune-fx motion blur.
    /// Always true when no dev panel is attached.
    pub fn motion_blur_enabled(&self) -> bool {
        #[cfg(all(feature = "dev", debug_assertions))]
        if let Some(dev) = &self.dev { return dev.blur; }
        true
    }

    #[cfg(all(feature = "dev", debug_assertions))]
    pub(crate) fn dev_velocity_overlay(&mut self) {
        for (i, node) in self.nodes.iter().enumerate() {
            let Some(v) = self.scene.store().velocity(node.id, prop::FRAME) else { continue };
            let (vx, vy) = (v.x + v.w * 0.5, v.y + v.h * 0.5);
            if vx.hypot(vy) < 1.0 { continue; }
            let [x, y, w, h] = self.drawn[i];
            let start = [x + w * 0.5, y + h * 0.5];
            let end = [start[0] + vx * 0.05, start[1] + vy * 0.05];
            let shape = Shape::new().line(start, end, 2.0, LineCap::Round, Color::WHITE);
            let scale = self.env.scale;
            push_shape(&mut self.instances, &shape, [0.0; 2], [scale; 2], scale, 0.8, NO_CLIP, [0.0; 4]);
        }
        if self.track_marks { self.marks.resize(self.instances.len(), 0); }
    }

    /// Changes the state from outside any handler, for data another thread delivered
    /// (`AppHandle::update`). Runs now, so it works while no window shows; the next frame commits
    /// it with the motion `f` asked for.
    pub fn update(&mut self, f: impl FnOnce(&mut S, &mut Tx)) {
        let mut one = Tx::default();
        f(&mut self.state, &mut one);
        self.shell_request = self.shell_request.max(one.shell.take());
        merge(&mut self.pending, one);
        self.dirty = true;
    }

    /// Next requested wake on the unscaled app clock. None lets the shell sleep indefinitely.
    pub fn wake_deadline(&self) -> Option<f64> {
        let deadline = self.logical_wake_deadline()?;
        #[cfg(all(feature = "dev", debug_assertions))]
        if let Some(clock) = &self.dev_clock { return clock.deadline(deadline); }
        Some(deadline)
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
        #[cfg(all(feature = "dev", debug_assertions))]
        if let Some(mut dev) = self.dev.take() {
            let scope = dev.scope();
            let moving = crate::dev::scope(scope, || {
                // Bring existing tracks/timers to the command's timestamp first. Idle wall
                // time must never be charged to a transition or spring created by that command.
                let commands = dev.prepare();
                if commands { self.frame_clock(dt); }
                dev.before(self);
                self.frame_clock(if commands { 0.0 } else { dt });
                dev.after(self)
            });
            self.dev = Some(dev);
            return moving;
        }
        self.frame_clock(dt)
    }

    fn frame_clock(&mut self, dt: f64) -> bool {
        // Like a native shell: the logical clock follows host time (timers, poll), while
        // animations step by the frame's own dt, so idle time never lands on a fresh commit.
        #[cfg(not(all(feature = "dev", debug_assertions)))]
        let elapsed = dt;
        #[cfg(all(feature = "dev", debug_assertions))]
        let (elapsed, dt) = match &mut self.dev_clock {
            Some(clock) => clock.advance(self.wall_start.map(|t| t.elapsed().as_secs_f64()), dt),
            None => (dt, dt),
        };
        // A coarse headless step still polls at every timer deadline, independent of FPS.
        // Input queued for the requested frame stays at that frame's timestamp.
        if self.wall_start.is_none()
            && self.logical_wake_deadline().is_some_and(|deadline| deadline > self.clock && deadline < self.clock + elapsed)
        {
            let target = self.clock + elapsed;
            let input = std::mem::take(&mut self.queue);
            while let Some(deadline) = self.logical_wake_deadline()
                && deadline > self.clock && deadline < target
            {
                self.frame_step(deadline - self.clock, deadline - self.clock);
            }
            self.queue.extend(input);
            return self.frame_step(target - self.clock, target - self.clock);
        }
        self.frame_step(elapsed, dt)
    }

    /// `elapsed` advances the logical clock when no wall clock drives it; `dt` steps animations.
    fn frame_step(&mut self, elapsed: f64, dt: f64) -> bool {
        let mut tx = std::mem::take(&mut self.pending);
        let mut ran = false;
        let wall_start = self.wall_start;
        #[cfg(all(feature = "dev", debug_assertions))]
        let wall_start = wall_start.filter(|_| self.dev_clock.is_none());
        self.clock = match wall_start {
            Some(t0) => t0.elapsed().as_secs_f64(),
            None => self.clock + elapsed,
        };
        if let Some((deadline, animation_time)) = self.loop_wake
            && !self.animating
        {
            // Native shells cap dt after idle. Advance the sleeping part of the
            // delay before new handlers retarget, without double-stepping manual time.
            let elapsed = animation_time - (deadline - self.clock).max(0.0) * self.time_scale as f64;
            self.t += (elapsed - self.t - dt * self.time_scale as f64).max(0.0);
            self.scene.set_time(self.t);
            if self.clock >= deadline { self.loop_wake = None; }
        }
        let due = self.wake_deadlines.partition_point(|t| *t <= self.clock);
        self.wake_deadlines.drain(..due);
        if let Some(poll) = self.poll {
            let mut one = Tx::default();
            // Scheduling does not imply a model change.
            if poll(&mut self.state, self.clock, &mut one) {
                ran = true;
            }
            merge(&mut tx, one);
        }
        if self.commits == 0 && self.queue.iter().any(|a| matches!(a, Action::WindowFocus(_))) {
            self.commit(Tx::default());
        }
        for action in std::mem::take(&mut self.queue) {
            ran |= self.run_action(action, &mut tx);
        }
        let shaped0 = self.text.shaped;
        for delay in tx.wakes.drain(..) {
            let deadline = self.clock + delay.as_secs_f64();
            if let Err(index) = self.wake_deadlines.binary_search_by(|t| t.total_cmp(&deadline)) {
                self.wake_deadlines.insert(index, deadline);
            }
        }
        if let Some(copy) = tx.copy.take() {
            self.copy_request = Some(copy);
        }
        self.paste_request |= tx.paste;
        self.shell_request = self.shell_request.max(tx.shell.take());
        for (source, target) in std::mem::take(&mut tx.text_flights) {
            self.capture_text_flight(source, target);
        }
        let scroll_to = std::mem::take(&mut tx.scroll_to);
        if ran || self.dirty || self.relayout || self.settle || self.virtual_window_changed() {
            if self.relayout && !ran && !self.dirty {
                tx.relayout = true;
            }
            if self.settle {
                tx.instant = true;
                tx.settle = true;
            }
            self.settled = tx.settle;
            self.commit(tx);
        } else {
            self.settled = false;
        }
        if !scroll_to.is_empty() { self.step_scroll(0.0); }
        for key in scroll_to { self.reveal_row(&key); }
        self.update_interaction();
        let live_animating = self.latch_live();
        let step = dt * self.time_scale as f64;
        self.t += step;
        self.scene.set_time(self.t);
        let out = self.scene.frame();
        let mut animating = out.animating || live_animating;
        if !out.exited.is_empty() {
            self.prune_exited();
        }
        animating |= self.step_growth(step as f32);
        animating |= self.step_scroll(step as f32);
        animating |= self.reveals_active();
        self.build();
        self.images.end_frame();
        animating |= self.loops_active;
        animating |= self.draw_text_flights();
        if self.track_marks {
            self.marks.resize(self.instances.len(), 0);
        }
        self.stats.shaped = self.text.shaped - shaped0;
        self.animating = animating && self.time_scale > 0.0;
        #[cfg(all(feature = "dev", debug_assertions))]
        if self.dev_clock.as_ref().is_some_and(|clock| clock.paused) { self.animating = false; }
        self.animating
    }

    fn prune_exited(&mut self) {
        let old = std::mem::take(&mut self.nodes);
        let mut remap = vec![None; old.len()];
        self.node_index.clear();
        for (old_idx, mut n) in old.into_iter().enumerate() {
            if !self.scene.contains(n.id) {
                self.reveals.remove(&n.id);
                self.scrolls.remove(&n.id);
                self.loops.remove(&n.id);
                continue;
            }
            n.parent = n.parent.and_then(|p| remap[p]);
            let idx = self.nodes.len();
            remap[old_idx] = Some(idx);
            self.node_index.insert(n.id, idx);
            self.nodes.push(n);
        }
        self.virtuals.retain(|path, _| self.nodes.iter().any(|n| n.path == *path));
        self.refresh_modal_index();
        if !self.layer_paint_order.is_empty() { self.update_layer_paint_order(); }
    }

    fn update_layer_paint_order(&mut self) {
        let n = self.nodes.len();
        let mut first = vec![n; n + 1];
        let mut next = vec![n; n];
        for i in (0..n).rev() {
            let parent = self.nodes[i].parent.map_or(0, |p| p + 1);
            next[i] = first[parent]; first[parent] = i;
        }
        self.layer_paint_order.clear();
        let mut i = first[0];
        while i < n {
            self.layer_paint_order.push(i);
            if first[i + 1] < n { i = first[i + 1]; continue; }
            while next[i] == n {
                let Some(parent) = self.nodes[i].parent else { i = n; break };
                i = parent;
            }
            if i < n { i = next[i]; }
        }
        if self.layer_paint_order.iter().copied().eq(0..n) { self.layer_paint_order.clear(); }
    }

    fn capture_text_flight(&mut self, source: String, target: String) {
        let Some((i, n)) = self.nodes.iter().enumerate().find(|(_, n)| n.key.as_deref() == Some(&source)) else { return };
        let (Some(spec), Some(layout), Some(r)) = (&n.text, &n.text_layout, self.drawn.get(i)) else { return };
        if spec.content.is_empty() { return; }
        let scale = self.env.scale;
        let rect = MRect::new(r[0] + n.inset[0], r[1] + n.inset[1],
            (layout.full_width() / scale).min(r[2] - n.inset[0] - n.inset[2]),
            (layout_height(layout, spec.style, scale) / scale).min(r[3] - n.inset[1] - n.inset[3]));
        let clip = n.parent.and_then(|p| self.inh_scratch.get(p)).map_or(NO_CLIP, |h| h.clip).map(|v| v / scale);
        let clip = [rect.x.max(clip[0]).max(0.0), rect.y.max(clip[1]).max(0.0),
            (rect.x + rect.w).min(clip[2]).min(self.viewport.0), (rect.y + rect.h).min(clip[3]).min(self.viewport.1)];
        if rect.w <= 0.0 || clip[2] <= clip[0] || clip[3] <= clip[1] { return; }
        self.glyph_scratch.clear();
        self.text.glyphs_for_layout(layout, scale, [rect.x * scale, rect.y * scale], false, &mut self.glyph_scratch);
        let color = spec.color;
        let glyphs = self.glyph_scratch.iter().filter(|q| q.rect[1] < clip[3] * scale && q.rect[1] + q.rect[3] > clip[1] * scale)
            .map(|q| glyph(q, color.premul(1.0), glyph_gamma(color, scale), clip.map(|v| v * scale), [0.0; 4])).collect();
        self.flights.push(TextFlight::new(source, target, rect, clip, glyphs, scale, self.t));
    }

    /// All destination rectangles below come from this frame's completed growth/scroll build.
    /// The overlay only writes draw instances, never layout or app state.
    fn draw_text_flights(&mut self) -> bool {
        let s = self.env.scale;
        let now = self.t;
        let reduced = self.reduced_motion;
        let viewport = [0.0, 0.0, self.viewport.0, self.viewport.1];
        let nodes = &self.nodes;
        let scene = &self.scene;
        self.flights.retain(|f| nodes.iter().any(|n| n.key.as_deref() == Some(&f.target_key)
            && !matches!(scene.phase(n.id), Some(PresencePhase::Exiting { .. }))));
        let mut done = Vec::new();
        for (fi, f) in self.flights.iter_mut().enumerate() {
            let Some((i, n)) = nodes.iter().enumerate().find(|(_, n)| n.key.as_deref() == Some(&f.target_key)) else { continue };
            let (Some(spec), Some(layout)) = (&n.text, &n.text_layout) else { done.push(fi); continue };
            let r = self.drawn[i];
            let target = MRect::new(r[0] + n.inset[0], r[1] + n.inset[1], r[2] - n.inset[0] - n.inset[2], r[3] - n.inset[1] - n.inset[3]);
            if f.measure(target, now, reduced) { done.push(fi); }
            // Land sub-quantum size residuals before snapping both edges. Otherwise a settled
            // height can alternate by one quantum while its still-moving top crosses the grid.
            let quantum = 1.0 / (64.0 * s);
            if (f.presented.w - target.w).abs() < quantum { f.presented.w = target.w; }
            if (f.presented.h - target.h).abs() < quantum { f.presented.h = target.h; }
            let edges = f.presented.edges().map(|v| grid(v, s));
            f.presented = MRect::new(edges[0], edges[1], edges[2] - edges[0], edges[3] - edges[1]);
            let p = f.progress(now, reduced);
            let (source_alpha, target_alpha) = f.alphas(target, now, reduced);
            let actual_clip = n.parent.and_then(|p| self.inh_scratch.get(p)).map_or(NO_CLIP, |h| h.clip).map(|v| v / s);
            let actual_clip = [actual_clip[0].max(0.0), actual_clip[1].max(0.0), actual_clip[2].min(viewport[2]), actual_clip[3].min(viewport[3])];
            let current = f.presented;
            let zoom = current.w / f.source.w;
            let start_clip = [current.x + (f.source_clip[0] - f.source.x) * zoom,
                current.y + (f.source_clip[1] - f.source.y) * zoom,
                current.x + (f.source_clip[2] - f.source.x) * zoom,
                current.y + (f.source_clip[3] - f.source.y) * zoom];
            // A travelling overlay is above the composer and the growing bubble's mask. Using
            // that destination mask here can hide the text before it has reached the bubble.
            // The normal ancestor clip takes over only once the flight has landed.
            let end_clip = viewport;
            let clip = std::array::from_fn(|j| start_clip[j] + (end_clip[j] - start_clip[j]) * p);
            f.clip = if reduced { actual_clip } else { clip };
            f.source_alpha = source_alpha;
            f.target_alpha = target_alpha;
            let final_fade = n.parent.and_then(|p| self.inh_scratch.get(p)).map_or([0.0; 4], |h| h.fade.map(|v| v / s));
            let fade = [0.0; 4];
            if source_alpha > 0.0 {
                let source_rect = if reduced { f.source } else { current };
                let source_clip = if reduced { f.source_clip } else { clip };
                for q in &f.glyphs {
                    self.instances.push(transformed(*q, f.source, source_rect, f.scale, source_alpha, source_clip, fade));
                }
            }
            self.glyph_scratch.clear();
            self.text.glyphs_for_layout(layout, s, [target.x * s, target.y * s], false, &mut self.glyph_scratch);
            for q in &self.glyph_scratch {
                let q = glyph(q, spec.color.premul(1.0), glyph_gamma(spec.color, s), NO_CLIP, [0.0; 4]);
                self.instances.push(transformed(q, target, current, s, target_alpha,
                    if reduced { actual_clip } else { clip }, if reduced { final_fade } else { fade }));
            }
        }
        for i in done.into_iter().rev() { self.flights.remove(i); }
        !self.flights.is_empty()
    }

    fn run_action(&mut self, action: Action, tx: &mut Tx) -> bool {
        let mut one = Tx::default();
        let ran = match action {
            Action::WindowFocus(focused) => {
                let Some(f) = self.nodes.first().and_then(|n| n.on_window_focus.clone()) else { return false };
                f(&mut self.state as &mut dyn Any, focused, &mut one);
                true
            }
            Action::FocusControl(path) => {
                self.set_focus(Some(path), false);
                true
            }
            Action::Dismiss(path) => {
                let Some(f) = self.node(&path).and_then(|n| n.modal.as_ref().map(|m| m.dismiss.clone())) else { return false };
                self.set_focus(None, self.keyboard_focus);
                f(&mut self.state, &mut one);
                true
            }
            Action::PointerMove(path, point) => {
                let Some(f) = self.node(&path).and_then(|n| n.on_pointer_move.clone()) else { return false };
                f(&mut self.state as &mut dyn Any, point, &mut one);
                true
            }
            Action::Hover(path) => {
                let Some(f) = self.node(&path).and_then(|n| n.on_hover.clone()) else { return false };
                f(&mut self.state as &mut dyn Any, &mut one);
                true
            }
            Action::Press(path) => {
                let Some(f) = self.node(&path).and_then(|n| n.on_press.clone()) else { return false };
                self.events.push(format!("press {path}"));
                f(&mut self.state as &mut dyn Any, &mut one);
                true
            }
            Action::SecondaryPress(path, point) => {
                let Some(n) = self.node(&path).filter(|n| !self.is_exiting(n.id)) else { return false };
                let Some(f) = n.on_secondary_press.clone() else { return false };
                self.events.push(format!("secondary press {path}"));
                f(&mut self.state as &mut dyn Any, point, &mut one);
                true
            }
            Action::Drag(path, mut d) => {
                let Some(f) = self.node(&path).and_then(|n| n.on_drag.clone()) else { return false };
                if d.phase == Phase::Move {
                    one.instant = true;
                }
                d.data = if d.phase == Phase::Down { 0.0 } else { self.drag_data };
                f(&mut self.state as &mut dyn Any, &mut d, &mut one);
                self.drag_data = d.data;
                if d.phase != Phase::Move {
                    self.events.push(format!("drag {:?} {path}", d.phase));
                }
                true
            }
            Action::Key(ev) => {
                self.keyboard_focus = true;
                // With no focused element, app-owned navigation (e.g. a toolbar)
                // gets first refusal before Rune starts a new Tab traversal.
                let app_first = self.focused.is_none();
                if app_first && let Some(f) = self.on_key
                    && f(&mut self.state, &ev, &mut one)
                {
                    merge(tx, one);
                    return true;
                }
                if let Some(f) = self.focused_input().and_then(|input| input.key.clone())
                    && f(&mut self.state as &mut dyn Any, &ev, &mut one)
                {
                    merge(tx, one);
                    return true;
                }
                if self.focus_key(&ev, &mut one) {
                    merge(tx, one);
                    return true;
                }
                let input_handles = match &ev.key {
                    crate::edit::Key::Function(_) | crate::edit::Key::Tab => false,
                    crate::edit::Key::Command(c) => matches!(c, 'a' | 'c' | 'x' | 'z'),
                    _ => true,
                };
                if input_handles {
                    let layout = self.focused_layout();
                    if self.with_focused(|e| e.key(&ev, layout.as_deref()), &mut one) {
                        merge(tx, one);
                        return true;
                    }
                }
                if !app_first && let Some(f) = self.on_key
                    && f(&mut self.state, &ev, &mut one)
                {
                    self.events.push(format!("key {:?}", ev.key));
                    merge(tx, one);
                    return true;
                }
                false
            }
            Action::Ime(ev) => self.with_focused(|e| e.ime(&ev), &mut one),
            Action::Paste(s, target) => {
                if target.is_some_and(|target| target != self.paste_target()) { return false; }
                if self.with_focused(|e| e.paste(&s), &mut one) {
                    self.events.push("paste field".into());
                    true
                } else if let Some(f) = self.app_paste_handler() {
                    f(&mut self.state, &s, &mut one);
                    self.events.push("paste window".into());
                    true
                } else { false }
            }
            Action::Focus(path, x, y) => {
                let Some(access) = self.node(&path).and_then(|n| n.input.as_ref()?.access.clone()) else { return false };
                let layout = self.node(&path).and_then(|n| n.text_layout.clone());
                let s = self.env.scale;
                let scroll = self.input_scroll.get(&path).copied().unwrap_or(0.0);
                self.set_focus(Some(path), false);
                let e = access(&mut self.state as &mut dyn Any);
                e.break_history_group();
                if let Some(l) = layout {
                    let x = x + scroll;
                    let i = e.model_index(Cursor::from_point(&l, x * s, y * s).index());
                    e.anchor = i;
                    e.focus = i;
                }
                one.instant = true;
                true
            }
            Action::Blur => {
                if self.focused.is_none() { return false; }
                self.set_focus(None, self.keyboard_focus);
                true
            }
        };
        merge(tx, one);
        ran
    }

    fn focused_layout(&mut self) -> Option<Rc<Layout<Ink>>> {
        let path = self.focused.clone()?;
        let s = self.env.scale;
        let n = self.node(&path)?;
        let (spec, w) = (n.text.clone()?, n.target[2] - n.target[0] - n.inset[0] - n.inset[2]);
        let text = zeroize::Zeroizing::new(n.input.as_ref()?.edit.display().0);
        Some(self.text.layout_aligned(&text, &spec.spans, spec.style, s, spec.wrap.then_some(w * s), line_alignment(&spec)))
    }

    fn with_focused(&mut self, f: impl FnOnce(&mut crate::edit::TextEdit) -> EditOutcome, tx: &mut Tx) -> bool {
        let Some(path) = self.focused.clone() else { return false };
        let Some((access, submit, change, animate_layout)) =
            self.focused_input().map(|i| (i.access.clone(), i.submit.clone(), i.change.clone(), i.animate_layout))
        else {
            return false;
        };
        let Some(access) = access else { return false };
        let before = change.as_ref().map(|_| zeroize::Zeroizing::new(access(&mut self.state as &mut dyn Any).text.clone()));
        let out = f(access(&mut self.state as &mut dyn Any));
        // Typing is the user's own hand: no animation.
        tx.instant = !animate_layout;
        if let Some(change) = change
            && before.as_ref().map(|s| s.as_str()) != Some(access(&mut self.state as &mut dyn Any).text.as_str())
        { change(&mut self.state as &mut dyn Any, tx); }
        if let Some(c) = out.copy {
            tx.copy(c);
        }
        if out.blur {
            self.set_focus(None, self.keyboard_focus);
        }
        if out.submit
            && let Some(sub) = submit
        {
            self.events.push(format!("submit {path}"));
            tx.instant = false;
            sub(&mut self.state as &mut dyn Any, tx);
        }
        true
    }

    fn node(&self, path: &str) -> Option<&Node> {
        self.nodes.iter().enumerate().find(|(i, n)| n.path == path && !self.subtree_exiting(*i)).map(|(_, n)| n)
    }

    fn focused_input(&self) -> Option<&InputSpec> {
        let path = self.focused.as_deref()?;
        self.node(path).and_then(|n| n.input.as_deref()).or_else(|| self.virtual_inputs.get(path).map(Box::as_ref))
    }

    fn subtree_exiting(&self, mut i: usize) -> bool {
        loop {
            if self.is_exiting(self.nodes[i].id) { return true; }
            let Some(parent) = self.nodes[i].parent else { return false };
            i = parent;
        }
    }

    fn is_exiting(&self, id: NodeId) -> bool {
        matches!(self.scene.phase(id), Some(PresencePhase::Exiting { .. }))
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

    // ---------------------------------------------------------------------------------------
    // Commit: view → nodes → layout → Scene retarget

    fn virtual_window_changed(&self) -> bool {
        self.virtuals.iter().any(|(path, v)| {
            let Some(&i) = self.path_ids.get(path).and_then(|id| self.node_index.get(id)) else { return false };
            if self.subtree_exiting(i) { return false; }
            self.path_ids.get(&v.scroll_path).and_then(|id| self.scrolls.get(id))
                .is_some_and(|s| v.needs_window(s.offset as f32, s.pinned))
        })
    }
}

/// Measurement capabilities: text/layout caches and virtual geometry only. In particular this
/// type has no Scene, growth tracks, reveal tracks, draw instances, or mutable app state.
struct Measurement<'a, S> {
    state: &'a S,
    view: fn(&S, &mut Cx<S>) -> El,
    env: Env,
    viewport: (f32, f32),
    system_reduced_motion: bool,
    reduced_motion: bool,
    time_scale: f32,
    commits: u64,
    nodes: &'a [Node],
    text: &'a mut TextSystem,
    layout: &'a mut TaffyTree<usize>,
    layout_entries: &'a mut HashMap<String, LayoutEntry>,
    node_index: &'a mut HashMap<NodeId, usize>,
    path_ids: &'a mut HashMap<String, NodeId>,
    next_id: &'a mut u64,
    scrolls: &'a mut HashMap<NodeId, ScrollState>,
    virtuals: &'a mut HashMap<String, crate::virtual_list::VirtualList>,
    virtual_culled: &'a mut HashSet<String>,
    virtual_inputs: &'a mut HashMap<String, Box<InputSpec>>,
    stats: &'a mut FrameStats,
    key_warnings: &'a mut Option<Vec<String>>,
}

/// Owned result of completed measurement. Scene mutation consumes this once; retries never
/// return it. Keeping this separate from El makes unmeasured views invalid commit inputs.
struct Measured {
    layout: TaffyTree<usize>,
    nodes: Vec<Node>,
    reduced_motion: bool,
    time_scale: f32,
}

fn collect_virtual_inputs(el: &El, parent: &str, index: usize, inputs: &mut HashMap<String, Box<InputSpec>>) {
    let segment = el.key.clone().unwrap_or_else(|| format!("#{index}"));
    let path = format!("{parent}/{segment}");
    if let Some(input) = &el.input
        && input.edit.focused && input.access.is_some()
    {
        inputs.insert(path.clone(), input.clone());
    }
    for (i, child) in el.children.iter().enumerate() {
        collect_virtual_inputs(child, &path, i, inputs);
    }
}

impl<S> Measurement<'_, S> {
    fn finish(mut self, tx: &Tx) -> Measured {
        self.virtual_culled.clear();
        let mut passes = 0;
        let (layout, nodes) = loop {
            passes += 1;
            self.virtual_inputs.clear();
            // Degenerate zero-height runs fall back to a full measurement instead of
            // an unbounded retry loop. Normal transcripts need one pass.
            let (layout, nodes) = self.measure_view(tx, passes > 8);
            if !self.measure_virtuals(&nodes, tx.settle) { break (layout, nodes); }
            *self.layout = layout;
        };
        // Exiting virtual subtrees still use their origin and measured extent while
        // fading. Runtime prunes this metadata with the retained scene nodes.
        Measured { layout, nodes, reduced_motion: self.reduced_motion, time_scale: self.time_scale }
    }

    fn node_id(&mut self, path: &str) -> NodeId {
        if let Some(&id) = self.path_ids.get(path) { return id; }
        let id = NodeId(*self.next_id);
        *self.next_id += 1;
        self.path_ids.insert(path.to_owned(), id);
        id
    }

    fn window_tree(&mut self, el: &mut El, parent: &str, index: usize, scroll: Option<(&str, bool)>, settle: bool, force_all: bool) {
        let seg = el.key.clone().unwrap_or_else(|| format!("#{index}"));
        let path = if parent.is_empty() { seg } else { format!("{parent}/{seg}") };
        if let Some(estimate) = el.virtual_estimate {
            assert!(el.growth.is_some(), "virtual rows need a growth stack");
            let (scroll_path, sticky) = scroll.expect("virtual stack must be the direct child of a scroll viewport");
            let old_scroll = self.path_ids.get(scroll_path).and_then(|id| self.scrolls.get(id));
            let pinned = (settle && sticky) || old_scroll.map_or(sticky, |s| s.pinned);
            let mut offset = old_scroll.map_or(0.0, |s| s.offset as f32);
            let height = self.nodes.iter().find(|n| n.path == scroll_path).map_or(self.viewport.1, |n| n.local.h);
            let padding = |p: LengthPercentage| match p.expand() {
                taffy::style::ExpandedLengthPercentage::Length(x) => x,
                _ => panic!("virtual stack padding must be in points"),
            };
            let v = self.virtuals.entry(path.clone()).or_default();
            v.estimate = estimate;
            v.scroll_path = scroll_path.to_owned();
            v.viewport = height;
            v.pad_top = padding(el.style.padding.top);
            v.pad_bottom = padding(el.style.padding.bottom);
            let old_keys: HashSet<_> = v.keys.get(v.range.clone()).unwrap_or_default().iter().cloned().collect();
            let keys: Vec<_> = el.children.iter().map(|c| c.key.clone().expect("virtual rows require stable keys")).collect();
            let all: HashSet<_> = keys.iter().cloned().collect();
            assert_eq!(all.len(), keys.len(), "virtual row keys must be unique");
            let mut anchor = None;
            if !pinned {
                let mut y = v.pad_top;
                for key in &v.keys {
                    let h = v.height(key);
                    if y + h > offset && all.contains(key) { anchor = Some((key.clone(), y)); break; }
                    y += h;
                }
            }
            v.keys = keys;
            v.heights.retain(|k, _| all.contains(k));
            v.born.retain(|k, _| all.contains(k));
            for key in &v.keys { v.born.entry(key.clone()).or_insert(self.commits); }
            v.anchor = anchor;
            let delta = v.anchor_delta();
            offset = (offset + delta).max(0.0);
            if let Some(st) = self.path_ids.get(scroll_path).and_then(|id| self.scrolls.get_mut(id)) {
                if !pinned { st.offset = (st.offset + f64::from(delta)).max(0.0); }
            }
            v.select(offset, pinned);
            if force_all { v.range = 0..v.keys.len(); v.before = 0.0; v.after = 0.0; }
            let mounted: HashSet<_> = v.keys[v.range.clone()].iter().cloned().collect();
            for key in old_keys.difference(&mounted).filter(|k| all.contains(*k)) {
                self.virtual_culled.insert(format!("{path}/{key}"));
            }
            el.style.padding.top = length(v.pad_top + v.before);
            el.style.padding.bottom = length(v.pad_bottom + v.after);
            el.children = std::mem::take(&mut el.children).into_iter().enumerate().filter_map(|(i, mut child)| {
                if !v.range.contains(&i) {
                    collect_virtual_inputs(&child, &path, i, self.virtual_inputs);
                    return None;
                }
                child.virtual_mount = v.born[child.key.as_ref().unwrap()] < self.commits;
                Some(child)
            }).collect();
        }
        let scroll = el.scroll.map(|s| (path.as_str(), s.stick_bottom));
        for (i, child) in el.children.iter_mut().enumerate() {
            self.window_tree(child, &path, i, scroll, settle, force_all);
        }
    }

    /// Finishes all row measurements before scene/growth mutation. Newly measured estimates
    /// above the viewport adjust the scroll offset by exactly the same amount.
    fn measure_virtuals(&mut self, nodes: &[Node], settle: bool) -> bool {
        let mut retry = false;
        for (path, v) in self.virtuals.iter_mut() {
            let Some((ci, _)) = nodes.iter().enumerate().find(|(_, n)| n.path == *path) else { continue };
            for n in nodes.iter().filter(|n| n.parent == Some(ci)) {
                v.heights.insert(n.key.clone().unwrap(), n.local.h);
            }
            let Some(scroll) = nodes.iter().find(|n| n.path == v.scroll_path) else { continue };
            v.viewport = scroll.local.h;
            let st = self.scrolls.get_mut(&scroll.id);
            let sticky = scroll.scroll.unwrap().stick_bottom;
            let pinned = (settle && sticky) || st.as_ref().map_or(sticky, |s| s.pinned);
            let offset = if let Some(st) = st {
                if !pinned { st.offset = (st.offset + f64::from(v.anchor_delta())).max(0.0); }
                st.offset as f32
            } else { 0.0 };
            let mounted_height = v.prefix(v.range.end) - v.before;
            // Never present a blank caused by an estimate. Expand and measure again first.
            retry |= if pinned {
                v.range.start > 0 && mounted_height < v.viewport
            } else {
                (v.range.start > 0 && offset < v.pad_top + v.before)
                    || (v.range.end < v.keys.len() && offset + v.viewport > v.pad_top + v.prefix(v.range.end))
            };
        }
        retry
    }

    fn measure_view(&mut self, tx: &Tx, force_all: bool) -> (TaffyTree<usize>, Vec<Node>) {
        let scale = self.env.scale;
        let mut cx = Cx::<S> {
            env: self.env,
            system_reduced_motion: self.system_reduced_motion,
            reduced_motion: self.system_reduced_motion,
            time_scale: self.time_scale,
            _s: PhantomData,
        };
        let mut root = (self.view)(self.state, &mut cx);
        if let Some(warnings) = self.key_warnings.as_mut() {
            *warnings = crate::key_diagnostics::warnings(&root);
            // Do not hand an invalid identity tree to Taffy. --tree still emits diagnostics.
            if !warnings.is_empty() { root = crate::el(); }
        } else {
            #[cfg(debug_assertions)]
            {
                let warnings = crate::key_diagnostics::warnings(&root);
                debug_assert!(warnings.is_empty(), "{}", warnings.join("\n"));
            }
        }
        self.window_tree(&mut root, "", 0, None, tx.settle, force_all);
        self.reduced_motion = cx.reduced_motion;
        self.time_scale = cx.time_scale;

        let mut taffy = std::mem::replace(self.layout, TaffyTree::with_capacity(0));
        for entry in self.layout_entries.values_mut() {
            entry.used = false;
        }
        let mut nodes = Vec::with_capacity(self.nodes.len().max(32));
        // Root constraints are part of the retained style, not a dirty write after flattening.
        root.style.size = Size { width: length(self.viewport.0), height: length(self.viewport.1) };
        let root_id = flatten(root, None, "", 0, motion::DEFAULT, false, &mut taffy, &mut nodes, &mut self.layout_entries, scale);
        self.layout_entries.retain(|_, entry| {
            if !entry.used { taffy.remove(entry.id).unwrap(); }
            entry.used
        });
        {
            let text_sys = &mut self.text;
            let nodes_ref = &nodes;
            taffy
                .compute_layout_with_measure(
                    root_id,
                    Size {
                        width: AvailableSpace::Definite(self.viewport.0),
                        height: AvailableSpace::Definite(self.viewport.1),
                    },
                    |inputs, _id, ctx, style| {
                        taffy::compute_leaf_layout(inputs, style, |_, _| 0.0, |known, avail| {
                            let Some(&mut i) = ctx else { return Size::ZERO };
                            let spec = nodes_ref[i].text.as_ref().unwrap();
                            let w = if spec.wrap {
                                known.width.or(match avail.width {
                                    AvailableSpace::Definite(w) => Some(w),
                                    AvailableSpace::MinContent => Some(0.0),
                                    AvailableSpace::MaxContent => None,
                                })
                            } else {
                                None
                            };
                            let l = text_sys.layout_rich(&spec.content, &spec.spans, spec.style, scale, w.map(|w| w * scale));
                            let mut lw = (l.full_width() / scale).ceil_to(scale);
                            if (spec.truncate || spec.truncate_fade)
                                && let AvailableSpace::Definite(a) = avail.width
                            {
                                lw = lw.min(a);
                            }
                            Size {
                                width: known.width.unwrap_or(lw),
                                height: known.height.unwrap_or(layout_height(&l, spec.style, scale) / scale),
                            }
                        })
                    },
                )
                .unwrap();
        }
        self.stats.layout_nodes = nodes.len();

        self.node_index.clear();
        self.node_index.reserve(nodes.len());
        for i in 0..nodes.len() {
            let id = self.node_id(&nodes[i].path);
            nodes[i].id = id;
            self.node_index.insert(id, i);
            let l = taffy.layout(nodes[i].taffy).unwrap();
            nodes[i].local = MRect::new(l.location.x, l.location.y, l.size.width, l.size.height);
            let (px, py) = nodes[i].parent.map_or((0.0, 0.0), |p| (nodes[p].target[0], nodes[p].target[1]));
            let mut x = px + l.location.x;
            let mut y = py + l.location.y;
            if let Some(a) = &nodes[i].above
                && let Some(j) = nodes[..i].iter().position(|n| n.key.as_ref() == Some(&a.key))
            {
                // Anchors inside scroll viewports sit where they are scrolled to.
                let mut target = nodes[j].target;
                let mut up = nodes[j].parent;
                while let Some(p) = up {
                    if let Some(st) = self.scrolls.get(&nodes[p].id) {
                        target[1] -= st.offset as f32;
                        target[3] -= st.offset as f32;
                    }
                    up = nodes[p].parent;
                }
                (x, y) = anchor_position(a, target, (l.size.width, l.size.height), self.viewport);
                nodes[i].local.x = x - px;
                nodes[i].local.y = y - py;
            }
            nodes[i].target = [x, y, x + l.size.width, y + l.size.height];
            let ins = [
                l.padding.left + l.border.left,
                l.padding.top + l.border.top,
                l.padding.right + l.border.right,
                l.padding.bottom + l.border.bottom,
            ];
            nodes[i].inset = ins;
            if let Some(spec) = &nodes[i].text {
                let w = (l.size.width - ins[0] - ins[2]).max(0.0);
                let wrap = spec.wrap.then_some(w * scale);
                let mut layout = self.text.layout_aligned(&spec.content, &spec.spans, spec.style, scale, wrap, line_alignment(spec));
                if spec.truncate && layout.full_width() > w * scale + 0.5 {
                    layout = truncated(&mut self.text, &spec.content, spec, scale, w * scale);
                }
                nodes[i].text_layout = Some(layout);
            }
        }

        (taffy, nodes)
    }
}

impl<S: 'static> Runtime<S> {
    fn commit(&mut self, tx: Tx) {
        let mut measured = self.measure(&tx);
        if self.sync_requested_focus(&mut measured.nodes) {
            self.layout = measured.layout;
            measured = self.measure(&tx);
        }
        self.apply(measured, tx);
    }

    fn measure(&mut self, tx: &Tx) -> Measured {
        Measurement {
            state: &self.state,
            view: self.view,
            env: self.env,
            viewport: self.viewport,
            system_reduced_motion: self.system_reduced_motion,
            reduced_motion: self.reduced_motion,
            time_scale: self.time_scale,
            commits: self.commits,
            nodes: &self.nodes,
            text: &mut self.text,
            layout: &mut self.layout,
            layout_entries: &mut self.layout_entries,
            node_index: &mut self.node_index,
            path_ids: &mut self.path_ids,
            next_id: &mut self.next_id,
            scrolls: &mut self.scrolls,
            virtuals: &mut self.virtuals,
            virtual_culled: &mut self.virtual_culled,
            virtual_inputs: &mut self.virtual_inputs,
            stats: &mut self.stats,
            key_warnings: &mut self.key_warnings,
        }.finish(tx)
    }

    /// The mutation phase cannot accept an El or an unfinished measurement pass.
    fn apply(&mut self, measured: Measured, tx: Tx) {
        let Measured { layout: taffy, mut nodes, reduced_motion, time_scale } = measured;
        if reduced_motion != self.reduced_motion {
            self.reduced_motion = reduced_motion;
            self.scene.set_reduced_motion(reduced_motion);
        }
        self.time_scale = time_scale;
        if tx.settle { self.flights.clear(); }
        let scale = self.env.scale;
        // Culling is not a model removal. Retire offscreen scene/growth state immediately;
        // re-entering historical rows mount settled and don't replay streaming words.
        for path in &self.virtual_culled {
            if let Some(id) = self.path_ids.get(path) {
                self.scene.unmount(*id);
                self.grows.remove(id);
            }
        }
        let first = self.commits == 0;
        self.commits += 1;
        self.stats.commits = self.commits;
        let instant = tx.instant || first;
        let now = self.t;

        // Growth stacks: rows of a stack that existed before grow; a new stack mounts settled.
        for c in 0..nodes.len() {
            let Some(_spec) = nodes[c].growth else { continue };
            let cid = nodes[c].id;
            // A reader away from the tail is anchored using measured row heights. Land those
            // heights with the anchor correction; delayed growth would move the reading position.
            let reading = self.virtuals.get(&nodes[c].path)
                .and_then(|v| self.path_ids.get(&v.scroll_path))
                .and_then(|id| self.scrolls.get(id)).is_some_and(|s| !s.pinned);
            let rows: Vec<usize> = (c + 1..nodes.len()).filter(|&j| nodes[j].parent == Some(c)).collect();
            let existed = self.stacks.contains_key(&cid) && !tx.settle;
            let mut stack = self.stacks.remove(&cid).unwrap_or_default();
            let new_ids: Vec<NodeId> = rows.iter().map(|&j| nodes[j].id).collect();
            let new_set: HashSet<NodeId> = new_ids.iter().copied().collect();
            // Merge: keep leaving rows where they stood.
            let mut order = Vec::with_capacity(new_ids.len() + 4);
            let mut ni = 0;
            for &old in &stack.order {
                if new_set.contains(&old) {
                    while ni < new_ids.len() && new_ids[ni] != old {
                        if !order.contains(&new_ids[ni]) {
                            order.push(new_ids[ni]);
                        }
                        ni += 1;
                    }
                    if !order.contains(&old) {
                        order.push(old);
                    }
                    ni += 1;
                } else if self.grows.get(&old).is_some_and(|g| g.h > 0.01) && existed && !instant {
                    stack.closing.entry(old).or_insert(now + 0.15);
                    order.push(old);
                } else {
                    self.grows.remove(&old);
                    stack.closing.remove(&old);
                }
            }
            for &id in &new_ids[ni.min(new_ids.len())..] {
                if !order.contains(&id) {
                    order.push(id);
                }
            }
            let ctop = nodes[c].target[1];
            let width = nodes[c].local.w;
            if first || !existed || tx.settle || self.reduced_motion || reading {
                stack.width_hold_until = 0.0;
                stack.width_shrink_until = 0.0;
                stack.pending_heights.clear();
            } else if (width - stack.width).abs() > 0.25 {
                stack.width_hold_until = now + 0.12;
                stack.width_shrink_until = now + 0.4;
            }
            stack.width = width;
            stack.pending_heights.retain(|id, _| new_set.contains(id));
            stack.pad_top = rows.first().map_or(0.0, |&j| nodes[j].target[1] - ctop);
            stack.pad_bottom = rows.last().map_or(nodes[c].target[3] - ctop, |&j| nodes[c].target[3] - nodes[j].target[3]);
            for &j in &rows {
                let id = nodes[j].id;
                let mut h = nodes[j].target[3] - nodes[j].target[1];
                stack.closing.remove(&id);
                stack.tops.insert(id, nodes[j].target[1] - ctop);
                let streaming = nodes[j].streaming;
                if streaming && existed && !tx.settle && !first && !self.reduced_motion && !reading {
                    // Measure the final wrapped text before mutating the growth track. Targets
                    // retain full layout height; only the presented stack extent lags the fill.
                    let mut end = j + 1;
                    while end < nodes.len() && nodes[end].parent.is_some_and(|p| p >= j) { end += 1; }
                    if let Some(n) = nodes[j..end].iter().rev().find(|n| n.text.as_ref().is_some_and(|t| t.wrap && t.reveal >= 0.0))
                        && let Some(l) = &n.text_layout
                    {
                        let width = (n.target[2] - n.target[0] - n.inset[0] - n.inset[2]) * scale;
                        h = (h - crate::text::stream_height_lag(l, width, scale)).max(0.0);
                    }
                }
                let g = self.grows.entry(id).or_insert_with(|| {
                    if existed && !instant && !nodes[j].virtual_mount { GrowTrack { streaming, ..GrowTrack::settled(0.0) } } else { GrowTrack::settled(h) }
                });
                g.streaming = streaming;
                if tx.settle || !existed || first || self.reduced_motion || reading {
                    *g = GrowTrack { streaming, ..GrowTrack::settled(h) };
                } else {
                    // Only a width/scale rewrap may shrink an active streaming row, including
                    // when fresh text arrives in the same frame as the resize.
                    let t = if streaming && !self.relayout && now >= stack.width_shrink_until { h.max(g.h.min(g.target)) } else { h };
                    if now < stack.width_hold_until {
                        stack.pending_heights.insert(id, t);
                    } else {
                        stack.pending_heights.remove(&id);
                        g.retarget(t);
                    }
                }
            }
            stack.order = order;
            self.stacks.insert(cid, stack);
        }

        for i in 0..nodes.len() {
            let id = nodes[i].id;
            let parent_id = nodes[i].parent.map(|p| nodes[p].id);
            let spring_l = tx.motion.unwrap_or(nodes[i].motion);
            let spring_c = tx.color_motion.unwrap_or(nodes[i].motion);
            // Text leads its background so theme crossfades never dip through zero contrast.
            let spring_tc = Spring::new((spring_c.duration * 0.5).max(0.1), spring_c.bounce);
            let row_like = nodes[i].in_row || nodes[i].parent.is_some_and(|p| nodes[p].growth.is_some());
            let layout_instant = instant || row_like || tx.relayout || self.virtuals.contains_key(&nodes[i].path);
            let mut scene_rect = nodes[i].local;
            if let Some(v) = nodes[i].parent.and_then(|p| self.virtuals.get(&nodes[p].path)) {
                scene_rect.y -= v.before;
            }

            let was_present = self.scene.contains(id);
            let was_exiting = self.is_exiting(id);
            if !was_present || was_exiting {
                let mut tr = nodes[i].transition;
                if nodes[i].motion != motion::DEFAULT {
                    tr.springs = SpringPair::with_exit_ratio(spring_l, 0.8);
                }
                // Revive presence first, then let this transaction own the layout retarget.
                // Scene::insert otherwise starts its default .4 s reflow before our .25 s
                // menu curve; the equal target makes the intended retarget a no-op.
                let mount_rect = if was_exiting {
                    self.scene.store().target(id, prop::FRAME).unwrap_or(scene_rect)
                } else { scene_rect };
                let mut spec = NodeSpec::new(mount_rect)
                    .opacity(nodes[i].vis.opacity)
                    .radius(Corners(nodes[i].vis.radius))
                    .transition(tr);
                if let Some(pid) = parent_id {
                    spec = spec.parent(pid);
                }
                let mount = if instant || nodes[i].virtual_mount || (row_like && !was_exiting && !self.reduced_motion) { Mount::Initial } else { Mount::Animated };
                self.scene.insert(id, spec, mount);
            }
            if was_present {
                let stack_row = nodes[i].parent.is_some_and(|p| nodes[p].growth.is_some()) || self.virtuals.contains_key(&nodes[i].path);
                let layout_anim = if stack_row && !instant && !tx.relayout {
                    // Growth and virtual windows own a row's vertical place; its horizontal place
                    // still springs, or a re-centred column jumps against its moving parent.
                    LayoutAnim::axes(spring_l, Anim::INSTANT)
                } else if layout_instant {
                    LayoutAnim::INSTANT
                } else {
                    let mut la = LayoutAnim::new(spring_l);
                    if tx.motion.is_none() {
                        if let Some((lead, trail)) = nodes[i].lead {
                            la = la.lead(lead, trail);
                        } else if nodes[i].width_leads {
                            la = la.width_leads();
                        }
                    }
                    la
                };
                if !nodes[i].live.iter().any(|b| b.property == crate::live::Property::Rect) {
                    self.scene.set_frame(id, scene_rect, layout_anim);
                }
                self.set_or_anim(id, prop::RADIUS, Corners(nodes[i].vis.radius), spring_l, instant);
                if nodes[i].live.iter().any(|b| b.property == crate::live::Property::Opacity) {
                    // The live source owns this property, including across unrelated commits.
                } else if !instant && let Some(anim) = nodes[i].opacity_motion {
                    self.scene.animate(id, prop::OPACITY, nodes[i].vis.opacity, anim);
                } else {
                    self.set_or_anim(id, prop::OPACITY, nodes[i].vis.opacity, spring_l, instant);
                }
            }
            let vis = &nodes[i].vis;
            if !nodes[i].live.iter().any(|b| b.property == crate::live::Property::Fill) {
                self.set_or_anim(id, P_FILL, vis.fill, spring_c, instant);
            }
            if vis.border_width > 0.0 {
                self.set_or_anim(id, P_BORDER, vis.border, spring_c, instant);
            }
            if let Some(c) = nodes[i].text.as_ref().map(|t| t.color).or(nodes[i].vis.icon.map(|(_, c)| c)) {
                self.set_or_anim(id, P_TEXT, c, spring_tc, instant);
            }
            if vis.hover.a > 0.0 || self.scene.store().has(id, P_HOVER_C) {
                self.set_or_anim(id, P_HOVER_C, vis.hover, spring_c, instant);
            }
            if vis.press.a > 0.0 || self.scene.store().has(id, P_PRESS_C) {
                self.set_or_anim(id, P_PRESS_C, vis.press, spring_c, instant);
            }
            for (k, sh) in vis.shadows.iter().take(P_SHADOWS.len()).enumerate() {
                self.set_or_anim(id, P_SHADOWS[k], sh.color, spring_c, instant);
            }
            if let Some(g) = vis.gradient {
                self.set_or_anim(id, P_GRADIENT[0], g[0], spring_c, instant);
                self.set_or_anim(id, P_GRADIENT[1], g[1], spring_c, instant);
            }
            if let Some(ol) = vis.outline {
                self.set_or_anim(id, P_OUTLINE, ol.color, spring_c, instant);
            }
            if (vis.hover.a > 0.0 || vis.press.a > 0.0 || vis.hover_group) && !self.scene.store().has(id, P_HOVER) {
                self.scene.set(id, P_HOVER, 0.0);
                self.scene.set(id, P_PRESS, 0.0);
            }

            // Streaming word reveal.
            if let Some(spec) = &nodes[i].text
                && spec.reveal >= 0.0
            {
                let content = &spec.content;
                let fade = if self.reduced_motion { 0.15 } else { spec.reveal };
                let mounted_late = !was_present && !instant && !nodes[i].virtual_mount;
                let st = self.reveals.entry(id).or_insert_with(|| RevealState {
                    len: if mounted_late { 0 } else { content.len() },
                    words: Vec::new(),
                    fade,
                });
                st.fade = fade;
                if instant || (!was_present && nodes[i].virtual_mount) || content.len() < st.len || !content.is_char_boundary(st.len) {
                    st.len = content.len();
                    st.words.clear();
                } else if content.len() > st.len && fade > 0.0 {
                    let mut starts = Vec::new();
                    word_starts(content, st.len, &mut starts);
                    // A word continuing across the old end keeps its time.
                    let mut tn = st.words.last().map_or(now, |w| w.1.max(now));
                    let pending = st.words.iter().filter(|w| w.1 > now).count() + starts.len();
                    let stagger = if self.reduced_motion {
                        0.0
                    } else {
                        let k = (pending as f32 / 24.0).clamp(0.0, 1.0);
                        (0.020 + (0.0055 - 0.020) * k) as f64
                    };
                    for b in starts {
                        st.words.push((b, tn));
                        tn += stagger;
                    }
                    st.len = content.len();
                } else {
                    st.len = content.len();
                }
            }
        }

        // Scroll viewports: a new sticky viewport opens at the end.
        for n in &nodes {
            if let Some(sp) = n.scroll {
                let e = self.scrolls.entry(n.id).or_insert(ScrollState {
                    offset: 0.0,
                    v: 0.0,
                    pinned: sp.stick_bottom,
                    max: 0.0,
                    direct: false,
                    scrolling: false,
                });
                if tx.settle && sp.stick_bottom {
                    e.pinned = true;
                    e.v = 0.0;
                }
            }
        }

        // Removed nodes stay mounted in `Scene` until their exit transition settles at opacity 0.
        let present_len = nodes.len();
        let old_nodes = std::mem::take(&mut self.nodes);
        let old_ids: Vec<NodeId> = old_nodes.iter().map(|n| n.id).collect();
        for mut old in old_nodes {
            if let Some(&i) = self.node_index.get(&old.id) {
                let mut fill = None;
                for bound in &mut nodes[i].live {
                    if let Some(previous) = old.live.iter().find(|b| b.property == bound.property && Arc::ptr_eq(&b.source, &bound.source)) {
                        bound.seen = if tx.settle { None } else { previous.seen };
                        if bound.property == crate::live::Property::Fill { fill = Some(old.vis.fill); }
                    }
                }
                if let Some(fill) = fill { nodes[i].vis.fill = fill; }
                // Removing a binding restores the normal property, including transform defaults.
                for previous in &old.live {
                    if !nodes[i].live.iter().any(|b| b.property == previous.property) {
                        match previous.property {
                            crate::live::Property::Offset => self.scene.set(old.id, prop::OFFSET, rune_motion::Vec2::ZERO),
                            crate::live::Property::Scale => self.scene.set(old.id, prop::SCALE, 1.0),
                            _ => {}
                        }
                    }
                }
            }
            if self.node_index.contains_key(&old.id) || !self.scene.contains(old.id) {
                continue;
            }
            let old_parent_id = old.parent.and_then(|p| old_ids.get(p).copied());
            let parent_remained_present =
                old_parent_id.is_none_or(|pid| self.node_index.get(&pid).is_some_and(|&i| i < present_len));
            if parent_remained_present {
                if tx.settle || (instant && !self.is_exiting(old.id)) {
                    self.scene.unmount(old.id);
                } else {
                    self.scene.remove(old.id);
                }
            }
            old.parent = old_parent_id.and_then(|pid| self.node_index.get(&pid).copied());
            old.live.clear();
            // Exit visuals outlive their model. Never call an accessor captured by
            // a deleted row while normalizing a later focus change.
            if let Some(input) = &mut old.input {
                input.access = None;
                input.key = None;
                input.change = None;
                input.edit.focused = false;
                input.edit.clear_preedit();
            }
            self.node_index.insert(old.id, nodes.len());
            nodes.push(old);
        }

        self.nodes = nodes;
        if self.nodes.len() > present_len { self.update_layer_paint_order(); }
        else { self.layer_paint_order.clear(); }
        self.virtuals.retain(|path, _| self.nodes.iter().any(|n| n.path == *path));
        self.subscribe_live();
        self.refresh_modal_index();
        self.sync_modals();
        self.loops.retain(|id, _| self.scene.contains(*id));
        for n in &self.nodes {
            if let Some(spec) = n.looping {
                self.loops.entry(n.id).and_modify(|l| { l.retime(spec, self.t); l.enabled(true, self.t); })
                    .or_insert_with(|| rune_motion::LoopTrack::new(spec, self.t));
            } else if let Some(l) = self.loops.get_mut(&n.id) {
                l.enabled(false, self.t);
            }
        }
        self.layout = taffy;
        self.text.end_commit();
        self.dirty = false;
        self.relayout = false;
        self.settle = false;
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
            let Some(spec) = self.nodes[ci].growth else { continue };
            let n_rows = stack.order.len();
            for (k, id) in stack.order.iter().enumerate() {
                let Some(g) = self.grows.get_mut(id) else { continue };
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
                moving |= g.moving();
            }
            stack.order.retain(|id| {
                let keep = !stack.closing.contains_key(id) || self.grows.get(id).is_some_and(|g| g.h > 0.01 || g.target > 0.0);
                if !keep {
                    self.grows.remove(id);
                }
                keep
            });
            stack.closing.retain(|id, _| stack.order.contains(id));
            // Keep the shared clock awake through the hold. No timer thread or remeasurement.
            moving |= !stack.pending_heights.is_empty();
        }
        moving || (steps == 0 && self.grows.values().any(|g| g.moving()))
    }

    /// Presented content extent of node `c` if it is a growth stack (points).
    fn stack_extent(&self, c: usize) -> Option<f64> {
        let n = &self.nodes[c];
        n.growth?;
        let stack = self.stacks.get(&n.id)?;
        let rows: f64 = stack.order.iter().filter_map(|id| self.grows.get(id)).map(|g| f64::from(g.h)).sum();
        Some(f64::from(stack.pad_top) + rows + f64::from(stack.pad_bottom))
    }

    fn step_scroll(&mut self, dt: f32) -> bool {
        let mut moving = false;
        for i in 0..self.nodes.len() {
            let Some(sp) = self.nodes[i].scroll else { continue };
            if self.subtree_exiting(i) { continue; }
            let id = self.nodes[i].id;
            let view_h = self.nodes[i].target[3] - self.nodes[i].target[1];
            let content = (i + 1..self.nodes.len()).find(|&j| self.nodes[j].parent == Some(i));
            let content_h = content.map_or(0.0, |c| {
                let n = &self.nodes[c];
                let top = n.target[1] - self.nodes[i].target[1];
                f64::from(top) + self.stack_extent(c).unwrap_or(f64::from(n.target[3] - n.target[1]))
            });
            let Some(st) = self.scrolls.get_mut(&id) else { continue };
            st.scrolling = st.direct || st.v.abs() > 1.0;
            st.direct = false;
            st.max = (content_h - f64::from(view_h)).max(0.0);
            if st.pinned && sp.stick_bottom {
                st.offset = st.max;
                st.v = 0.0;
            } else if st.v.abs() > 1.0 {
                st.offset += f64::from(st.v) * f64::from(dt);
                st.v *= (-SCROLL_DECAY * dt).exp();
                if st.offset <= 0.0 || st.offset >= st.max {
                    st.v = 0.0;
                }
                moving = true;
            } else {
                st.v = 0.0;
            }
            st.offset = st.offset.clamp(0.0, st.max);
            if sp.stick_bottom && st.offset >= st.max - 0.5 && st.v.abs() <= 1.0 {
                st.pinned = true;
            }
        }
        moving
    }

    fn reveals_active(&mut self) -> bool {
        let now = self.t;
        let mut any = false;
        for st in self.reveals.values_mut() {
            let fade = st.fade as f64;
            // Words fully shown leave the list (only from the front, which keeps lookups valid).
            let done = st.words.iter().take_while(|w| now - w.1 >= fade).count();
            if done > 0 && done == st.words.len() {
                st.words.clear();
            }
            any |= !st.words.is_empty();
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

    fn primary_target(&self, x: f32, y: f32) -> Option<usize> {
        self.hits.iter().rev().find_map(|&(id, r)| {
            if x < r[0] || x > r[2] || y < r[1] || y > r[3] { return None; }
            let i = *self.node_index.get(&id)?;
            let n = &self.nodes[i];
            // A secondary-only child must not obscure primary input or background blur.
            (!self.subtree_exiting(i) && self.allowed(i) && (n.on_press.is_some() || n.on_hover.is_some()
                || n.on_drag.is_some() || n.input.is_some() || n.window_drag || n.focus.enabled || n.modal.is_some())).then_some(i)
        })
    }

    pub fn pointer_move(&mut self, x: f32, y: f32) {
        self.pointer = Some((x, y));
        for &(id, rect) in &self.hits {
            if crate::overlay_options::contains(rect, x, y)
                && let Some(n) = self.node_index.get(&id).map(|&i| &self.nodes[i])
                && n.on_pointer_move.is_some()
            {
                self.queue.push(Action::PointerMove(n.path.clone(), PointerPress { x, y }));
            }
        }
        self.refresh_hover(x, y);
        let Some((path, d)) = self.capture.clone() else { return };
        let Some((id, r, has_drag)) = self.node(&path).map(|n| (n.id, n.target, n.on_drag.is_some())) else { return };
        // A hit rect is where the element was drawn (after scrolling); `target` is its
        // unscrolled layout rect, kept only for elements that were never drawn.
        let mut hits = self.hits.iter().filter(|(hit, _)| *hit == id).peekable();
        self.capture_inside = if self.native_overlay || hits.peek().is_some() {
            hits.any(|(_, r)| crate::overlay_options::contains(*r, x, y))
        } else { x >= r[0] && x <= r[2] && y >= r[1] && y <= r[3] };
        if has_drag {
            let (dx, dy) = (x - self.press_at.0, y - self.press_at.1);
            let d = Drag { phase: Phase::Move, x: x - r[0], y: y - r[1], dx, dy, ..d };
            self.queue.push(Action::Drag(path, d));
        }
    }

    /// Overlay observation updates coordinates without presenting unchanged hover pixels.
    pub(crate) fn observe_pointer(&mut self, x: f32, y: f32) -> bool {
        if self.pointer == Some((x, y)) { return false; }
        let visual_changed = self.nodes.iter().enumerate().any(|(i, n)| {
            if n.vis.hover.a <= 0.0 && n.vis.press.a <= 0.0 { return false; }
            let r = self.drawn.get(i).map_or(n.target, |d| [d[0], d[1], d[0] + d[2], d[1] + d[3]]);
            let inside = |p: Option<(f32, f32)>| p.is_some_and(|(x, y)| x >= r[0] && y >= r[1] && x <= r[2] && y <= r[3]);
            inside(self.pointer) != inside(Some((x, y)))
        });
        let (queued, capture_inside) = (self.queue.len(), self.capture_inside);
        self.pointer_move(x, y);
        visual_changed || self.queue.len() != queued || self.capture_inside != capture_inside
    }

    pub(crate) fn pointer_captured(&self) -> bool { self.capture.is_some() || self.secondary_capture.is_some() }

    pub(crate) fn cancel_pointer(&mut self) {
        self.capture = None;
        self.secondary_capture = None;
        self.capture_inside = false;
        self.pointer_leave();
    }

    fn refresh_hover(&mut self, x: f32, y: f32) {
        let hovered = self.primary_target(x, y);
        if hovered.map(|i| self.nodes[i].path.as_str()) == self.hovered.as_deref() { return; }
        self.hovered = hovered.map(|i| self.nodes[i].path.clone());
        if let Some(path) = &self.hovered { self.queue.push(Action::Hover(path.clone())); }
    }

    pub fn pointer_down(&mut self, x: f32, y: f32) {
        self.pointer = Some((x, y));
        self.keyboard_focus = false;
        let target = self.primary_target(x, y);
        if let Some(i) = self.active_modal()
            && target.is_none_or(|t| !self.allowed(t))
        {
            if self.nodes[i].modal.as_ref().is_some_and(|m| m.outside) {
                self.queue.push(Action::Dismiss(self.nodes[i].path.clone()));
            }
            return;
        }
        let Some(ti) = target else {
            if self.focused.is_some() {
                self.queue.push(Action::Blur);
            }
            return;
        };
        let n = &self.nodes[ti];
        if n.window_drag && n.on_press.is_none() && n.on_drag.is_none() && n.input.is_none() {
            let now = self.now_clock();
            let double = self.last_drag_press.is_some_and(|(t, px, py)| {
                now - t <= self.double_click_interval && (x - px).abs() <= 4.0 && (y - py).abs() <= 4.0
            });
            self.last_drag_press = if double { None } else { Some((now, x, y)) };
            self.titlebar_action = Some(if double { TitlebarAction::DoubleClick } else { TitlebarAction::Drag });
            if self.focused.is_some() {
                self.queue.push(Action::Blur);
            }
            return;
        }
        self.press_at = (x, y);
        let r = self.drawn.get(ti).copied().unwrap_or([n.target[0], n.target[1], n.target[2] - n.target[0], n.target[3] - n.target[1]]);
        if n.input.is_some() {
            self.queue.push(Action::Focus(n.path.clone(), x - r[0] - n.inset[0], y - r[1] - n.inset[1]));
            return;
        }
        if n.focus.enabled {
            self.queue.push(Action::FocusControl(n.path.clone()));
        } else if self.focused.is_some() {
            self.queue.push(Action::Blur);
        }
        let r = n.target;
        let d = Drag {
            phase: Phase::Down,
            x: x - r[0],
            y: y - r[1],
            w: r[2] - r[0],
            h: r[3] - r[1],
            start_x: x - r[0],
            start_y: y - r[1],
            dx: 0.0,
            dy: 0.0,
            data: 0.0,
        };
        let path = n.path.clone();
        if n.on_drag.is_some() {
            self.queue.push(Action::Drag(path.clone(), d));
        }
        self.capture = Some((path, d));
        self.capture_inside = true;
    }

    fn secondary_target(&self, x: f32, y: f32) -> Option<&str> {
        let id = self.hits.iter().rev().find(|&&(_, r)| x >= r[0] && x <= r[2] && y >= r[1] && y <= r[3])?.0;
        let mut index = *self.node_index.get(&id)?;
        loop {
            let n = &self.nodes[index];
            if self.subtree_exiting(index) || !self.allowed(index) { return None; }
            if n.on_secondary_press.is_some() { return Some(&n.path); }
            index = n.parent?;
        }
    }

    /// Capture a secondary gesture without activating primary actions or text selection.
    pub fn secondary_down(&mut self, x: f32, y: f32) {
        self.pointer = Some((x, y));
        self.keyboard_focus = false;
        self.secondary_capture = self.secondary_target(x, y).map(str::to_owned);
    }

    pub fn secondary_up(&mut self, x: f32, y: f32) {
        self.pointer = Some((x, y));
        if let Some(path) = self.secondary_capture.take()
            && self.secondary_target(x, y) == Some(path.as_str())
        {
            self.queue.push(Action::SecondaryPress(path, PointerPress { x, y }));
        }
    }

    pub fn pointer_up(&mut self, x: f32, y: f32) {
        self.pointer_move(x, y);
        let Some((path, d)) = self.capture.take() else { return };
        let Some((r, has_drag, has_press)) =
            self.node(&path).map(|n| (n.target, n.on_drag.is_some(), n.on_press.is_some()))
        else {
            return;
        };
        if has_drag {
            let (dx, dy) = (x - self.press_at.0, y - self.press_at.1);
            self.queue.push(Action::Drag(path.clone(), Drag { phase: Phase::Up, x: x - r[0], y: y - r[1], dx, dy, ..d }));
        }
        if has_press && self.capture_inside {
            self.queue.push(Action::Press(path));
        }
        self.capture_inside = false;
    }

    pub fn pointer_leave(&mut self) {
        if self.capture.is_none() && self.secondary_capture.is_none() {
            self.pointer = None;
            self.hovered = None;
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
    /// the pointer, else the hovered one's, from the nearest ancestor that sets one.
    pub fn cursor(&self) -> crate::el::Cursor {
        let path = self.capture.as_ref().map(|(p, _)| p.as_str()).or(self.hovered.as_deref());
        let Some(mut i) = path.and_then(|p| self.nodes.iter().position(|n| n.path == p)) else {
            return crate::el::Cursor::Default;
        };
        if self.nodes[i].cursor.is_none() && self.nodes[i].input.is_some() {
            return crate::el::Cursor::Text;
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
        self.queue.push(Action::Key(ev));
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
        self.queue.push(Action::Paste(zeroize::Zeroizing::new(text.into()), None));
    }

    fn app_paste_handler(&self) -> Option<crate::el::PasteFn> {
        self.nodes.get(self.active_modal().unwrap_or(0))?.on_paste.clone()
    }
    pub(crate) fn accepts_paste(&self) -> bool {
        self.paste_target().is_some() || self.app_paste_handler().is_some()
    }
    pub(crate) fn paste_target(&self) -> Option<String> {
        self.focused_input().filter(|i| i.access.is_some()).and(self.focused.clone())
    }
    pub(crate) fn paste_result(&mut self, target: Option<String>, text: zeroize::Zeroizing<String>) {
        self.queue.push(Action::Paste(text, Some(target)));
    }

    /// Whether the focused editor requires secure native keyboard input.
    pub fn has_secure_focus(&self) -> bool {
        self.focused_input().is_some_and(|i| i.edit.is_secure())
    }

    pub fn has_focus(&self) -> bool {
        self.focused.is_some()
    }

    fn reveal_row(&mut self, key: &str) {
        let geometry = self.virtuals.iter().find_map(|(_, v)| {
            let index = v.keys.iter().position(|candidate| candidate == key)?;
            Some((v.scroll_path.clone(), f64::from(v.pad_top + v.prefix(index)), f64::from(v.height(key)), f64::from(v.viewport)))
        }).or_else(|| {
            let (i, row) = self.nodes.iter().enumerate().find(|(_, n)| n.key.as_deref() == Some(key) || n.path == key)?;
            let mut ancestor = self.nodes[i].parent;
            while let Some(p) = ancestor {
                let parent = &self.nodes[p];
                if parent.scroll.is_some() {
                    return Some((parent.path.clone(), f64::from(row.target[1] - parent.target[1]),
                        f64::from(row.local.h), f64::from(parent.local.h)));
                }
                ancestor = parent.parent;
            }
            None
        });
        let Some((path, top, height, viewport)) = geometry else { return };
        let Some(id) = self.path_ids.get(&path) else { return };
        let Some(scroll) = self.scrolls.get_mut(id) else { return };
        let offset = if top < scroll.offset { top } else if top + height > scroll.offset + viewport {
            top + height - viewport
        } else { scroll.offset };
        scroll.offset = offset.clamp(0.0, scroll.max);
        scroll.pinned = false;
        scroll.v = 0.0;
        scroll.direct = true;
        if self.virtual_window_changed() { self.commit(Tx::default()); }
    }

    fn scroll_at(&self, x: f32, y: f32) -> Option<NodeId> {
        self.nodes
            .iter()
            .enumerate()
            .rev()
            .filter(|(i, n)| n.scroll.is_some() && !self.subtree_exiting(*i) && self.allowed(*i))
            .find(|(i, _)| {
                let r = self.drawn.get(*i).copied().unwrap_or([0.0; 4]);
                x >= r[0] && x <= r[0] + r[2] && y >= r[1] && y <= r[1] + r[3]
            })
            .map(|(_, n)| n.id)
    }

    /// Wheel or trackpad delta in points (positive `dy` moves content down, like a browser's
    /// negative deltaY). Applied 1:1: the platform supplies its own momentum phases.
    pub fn wheel(&mut self, x: f32, y: f32, dy: f32) {
        let Some(id) = self.scroll_at(x, y) else { return };
        let stick = self.node_index.get(&id).and_then(|&i| self.nodes[i].scroll).is_some_and(|s| s.stick_bottom);
        if let Some(st) = self.scrolls.get_mut(&id) {
            st.offset = (st.offset - f64::from(dy)).clamp(0.0, st.max);
            st.v = 0.0;
            st.direct = true;
            st.pinned = stick && st.offset >= st.max - 0.5;
            self.events.push(format!("wheel {dy:.1}"));
        }
    }

    /// Releases a scroll with `velocity` (points per second, positive moves content down);
    /// the runtime decays it (for platforms without OS momentum and headless scripts).
    pub fn fling(&mut self, x: f32, y: f32, velocity: f32) {
        let Some(id) = self.scroll_at(x, y) else { return };
        if let Some(st) = self.scrolls.get_mut(&id) {
            st.v = -velocity;
            st.pinned = false;
            self.events.push(format!("fling {velocity:.0}"));
        }
    }

    pub fn has_pending_input(&self) -> bool {
        !self.queue.is_empty()
    }

    fn update_interaction(&mut self) {
        let captured = self.capture.as_ref().map(|(p, _)| p.as_str()).or(self.secondary_capture.as_deref());
        for i in 0..self.nodes.len() {
            let n = &self.nodes[i];
            let tracks_hover = n.vis.hover.a > 0.0 || n.vis.hover_group;
            if self.is_exiting(n.id) || (!tracks_hover && n.vis.press.a <= 0.0) {
                continue;
            }
            let r = self.drawn.get(i).map_or(
                [n.target[0], n.target[1], n.target[2], n.target[3]],
                |d| [d[0], d[1], d[0] + d[2], d[1] + d[3]],
            );
            // A group inside a clipped region (a scrolled list) only hovers where it shows.
            let c = n.parent.and_then(|p| self.inh_scratch.get(p)).map_or(NO_CLIP, |h| h.clip).map(|v| v / self.env.scale);
            let inside = self.pointer.is_some_and(|(x, y)| {
                x >= r[0].max(c[0]) && x <= r[2].min(c[2]) && y >= r[1].max(c[1]) && y <= r[3].min(c[3])
            });
            // A group stays hovered while one of its descendants holds the press.
            let within = |p: &str| p == n.path || (p.len() > n.path.len() && p.starts_with(&n.path) && p.as_bytes()[n.path.len()] == b'/');
            let hovered = inside && tracks_hover && captured.is_none_or(within);
            let capture_inside = if self.secondary_capture.is_some() { inside } else { self.capture_inside };
            let pressed = captured == Some(n.path.as_str()) && capture_inside && n.vis.press.a > 0.0;
            let id = n.id;
            let hover_anim = match n.vis.hover_motion {
                Some((enter, exit)) => if hovered { enter } else { exit },
                None => motion::SNAPPY.into(),
            };
            self.scene.animate(id, P_HOVER, hovered as u8 as f32, hover_anim);
            self.scene.animate(id, P_PRESS, pressed as u8 as f32, if pressed { motion::INSTANT } else { motion::SNAPPY });
        }
    }

    // ---------------------------------------------------------------------------------------
    // Frame build: Scene Presented → instances

    fn build(&mut self) {
        let s = self.env.scale;
        let snap_px = |v: f32| grid(v, s) * s;
        self.instances.clear();
        self.marks.clear();
        self.surfaces.clear();
        self.hits.clear();
        self.loops_active = false;
        self.loop_wake = None;
        self.overlay_hits.clear();
        let n_nodes = self.nodes.len();
        self.inh_scratch.clear();
        self.inh_scratch.resize(n_nodes, Inh::default());
        self.drawn.clear();
        self.drawn.resize(n_nodes, [0.0; 4]);
        self.ime_area = None;
        let now = self.t;
        let vp = [0.0, 0.0, self.viewport.0 * s, self.viewport.1 * s];
        let mut glyph_count = 0usize;
        let mut ambient: Option<f32> = None;
        // Retain scope capacity across frames. Records preserve absolute instance indices
        // for images, video insertions, retained springs and compositor list ranges.
        let mut layers = std::mem::take(&mut self.layer_scratch);
        layers.clear();

        // Growth rows: presented top offset, height and bottom feather mask per node index.
        self.row_dy.clear();
        self.row_dy.resize(n_nodes, 0.0);
        self.row_h.clear();
        self.row_h.resize(n_nodes, -1.0);
        self.row_mask.clear();
        self.row_mask.resize(n_nodes, -1.0);
        for (cid, stack) in &self.stacks {
            let Some(&ci) = self.node_index.get(cid) else { continue };
            let Some(spec) = self.nodes[ci].growth else { continue };
            let origin = self.virtuals.get(&self.nodes[ci].path).map_or(0.0, |v| v.before);
            let mut y = stack.pad_top - origin;
            for id in &stack.order {
                let Some(g) = self.grows.get(id) else { continue };
                let top_rel = stack.tops.get(id).copied().unwrap_or(0.0) - origin;
                let cur = y;
                if let Some(&ri) = self.node_index.get(id) {
                    self.row_dy[ri] = cur - top_rel;
                    self.row_h[ri] = g.h;
                    let withheld = if g.streaming { (self.nodes[ri].local.h - g.target).max(0.0) } else { 0.0 };
                    let rewrap = stack.pending_heights.contains_key(id) && self.nodes[ri].local.h > g.h + 0.01;
                    if spec.edge.clip && (g.h < g.target - 0.01 || g.mask > 0.01 || withheld > 0.01 || rewrap) {
                        self.row_mask[ri] = g.mask.max(withheld.min(spec.edge.floor)).max(if rewrap { spec.edge.floor } else { 0.0 });
                    }
                }
                y = cur + g.h;
            }
            // The stack itself presents the rows' heights, which scrolling follows.
            self.row_h[ci] = y + stack.pad_bottom;
        }

        // Exiting descendants are appended to storage. Bring them back into their subtree
        // only when isolation requires it; the ordinary painter stream stays byte-identical.
        let tree_order = self.layer_paint_order.windows(2).any(|pair| {
            let n = &self.nodes[pair[0]];
            self.nodes[pair[1]].parent == Some(pair[0]) && (n.vis.blend == crate::render::Blend::PlusLighter || n.vis.group_opacity.is_some()
                || matches!(n.looping, Some(rune_motion::LoopSpec { kind: rune_motion::LoopKind::Opacity { .. } | rune_motion::LoopKind::OpacityCycle(_), .. }))
                || self.scene.presented(n.id).is_some_and(|p| p.opacity > 0.0 && p.opacity < 1.0))
        });
        for paint_index in 0..n_nodes {
            let i = if tree_order { self.layer_paint_order[paint_index] } else { paint_index };
            let n = &self.nodes[i];
            while let Some(&(ancestor, start, surface_start)) = layers.last() {
                let mut parent = n.parent;
                while parent.is_some_and(|p| p != ancestor) { parent = self.nodes[parent.unwrap()].parent; }
                if parent == Some(ancestor) { break; }
                crate::render::end_layer(&mut self.instances, &mut self.surfaces, &mut self.marks, start, surface_start);
                layers.pop();
            }
            let Some(p) = self.scene.presented(n.id) else { continue };
            let mut pinh = n.parent.map_or(Inh::default(), |pr| self.inh_scratch[pr]);
            pinh.dy += self.row_dy[i];
            let mark = if n.vis.mark != 0 { n.vis.mark } else { pinh.mark };
            let mut own = p.opacity;
            if let Some([rest, hov]) = n.vis.group_opacity {
                own *= rest + (hov - rest) * pinh.group;
            }
            let base_opacity = (own * pinh.opacity).clamp(0.0, 1.0);
            let group = if n.vis.hover.a > 0.0 || n.vis.hover_group {
                self.scene.get(n.id, P_HOVER).unwrap_or(0.0).clamp(0.0, 1.0)
            } else {
                pinh.group
            };
            let loop_value = self.loops.get(&n.id).map(|l| l.sample_at(now, self.clock, self.reduced_motion));
            if let (Some(rune_motion::LoopSpec { kind: rune_motion::LoopKind::Opacity { .. } | rune_motion::LoopKind::OpacityCycle(_), .. }), Some(v)) = (n.looping, loop_value) {
                own *= v;
            }
            let effective_opacity = (own * pinh.opacity).clamp(0.0, 1.0);
            let next = if tree_order { self.layer_paint_order.get(paint_index + 1).copied() } else { (i + 1 < n_nodes).then_some(i + 1) };
            let additive_parent = n.parent.is_some_and(|p| self.nodes[p].vis.blend == crate::render::Blend::PlusLighter);
            let isolate = effective_opacity > 0.001 && (n.vis.blend == crate::render::Blend::PlusLighter
                || additive_parent || (own > 0.001 && own < 1.0
                    && next.is_some_and(|j| self.nodes[j].parent == Some(i))));
            let o = if isolate { 1.0 } else { (own * pinh.paint_opacity).clamp(0.0, 1.0) };
            let (mut clip, mut fade) = (pinh.clip, pinh.fade);
            let mut fade_origin = pinh.fade_origin;
            let own_sc = p.transform.scale.x;
            let (sc, anchor) = if (own_sc - 1.0).abs() > 1e-5 {
                (pinh.scale * own_sc, [p.anchor.x, p.anchor.y + pinh.dy])
            } else {
                (pinh.scale, pinh.anchor)
            };
            let blur = pinh.blur.max(p.blur);

            let mut raw_edges = p.rect.edges();
            raw_edges[1] += pinh.dy;
            raw_edges[3] += pinh.dy;
            if self.row_h[i] >= 0.0 {
                raw_edges[3] = raw_edges[1] + self.row_h[i];
            }
            let mut rect_pt = if (sc - 1.0).abs() > 1e-5 {
                [
                    anchor[0] + (raw_edges[0] - anchor[0]) * sc,
                    anchor[1] + (raw_edges[1] - anchor[1]) * sc,
                    anchor[0] + (raw_edges[2] - anchor[0]) * sc,
                    anchor[1] + (raw_edges[3] - anchor[1]) * sc,
                ]
            } else {
                raw_edges
            };
            // Land a sub-quantum size residual before snapping both edges. Otherwise a
            // translating/scaling box can alternate between neighbouring widths at rest.
            for (axis, target) in [n.local.w, n.local.h].into_iter().enumerate() {
                let target = grid(target, s);
                if ((rect_pt[axis + 2] - rect_pt[axis]) - target).abs() * s < 1.0 / 32.0 {
                    rect_pt[axis + 2] = rect_pt[axis] + target;
                }
            }
            let r = rect_pt.map(snap_px);
            let (w, h) = ((r[2] - r[0]).max(0.0), (r[3] - r[1]).max(0.0));
            self.drawn[i] = [r[0] / s, r[1] / s, w / s, h / s];
            let radii = p.radius.0.map(|v| v * sc * s);

            if !p.mask.is_none() {
                let ins = p.mask.inset.0.map(|v| v * sc * s);
                let fea = p.mask.feather.0.map(|v| v * sc * s);
                clip = [
                    (r[0] + ins[0]).max(clip[0]),
                    (r[1] + ins[1]).max(clip[1]),
                    (r[2] - ins[2]).min(clip[2]),
                    (r[3] - ins[3]).min(clip[3]),
                ];
                inherit_fade(&mut fade, &mut fade_origin, [r[0] + ins[0], r[1] + ins[1], r[2] - ins[2], r[3] - ins[3]], fea);
            }
            let visible = r[2] >= clip[0].max(vp[0]) - 2.0
                && r[0] <= clip[2].min(vp[2]) + 2.0
                && r[3] >= clip[1].max(vp[1]) - 2.0
                && r[1] <= clip[3].min(vp[3]) + 2.0;
            let text_tint = |c: Color| match n.vis.group_color {
                Some(g) if pinh.group > 0.0 => c.mix(g, pinh.group),
                _ => c,
            };

            if self.surface_visible && base_opacity > 0.001 && !self.reduced_motion
                && r[2] > clip[0].max(vp[0]) && r[0] < clip[2].min(vp[2])
                && r[3] > clip[1].max(vp[1]) && r[1] < clip[3].min(vp[3])
                && let Some(track) = self.loops.get(&n.id)
            {
                if let Some(dt) = track.frame_interval().filter(|_| track.active(now)) {
                    ambient = Some(ambient.map_or(dt as f32, |a| a.min(dt as f32)));
                    let wake = self.clock + dt;
                    if self.loop_wake.is_none_or(|w| wake < w.0) {
                        self.loop_wake = Some((wake, now + dt * self.time_scale as f64));
                    }
                } else if self.time_scale > 0.0 {
                    self.loops_active |= track.active(now);
                }
                if self.time_scale > 0.0 && let Some(start) = track.pending_start(now) {
                    let wake = self.clock + (start - now) / self.time_scale as f64;
                    if self.loop_wake.is_none_or(|w| wake < w.0) { self.loop_wake = Some((wake, start)); }
                }
            }
            if isolate {
                layers.push((i, self.instances.len(), self.surfaces.len()));
                self.instances.push(Instance::begin_layer([r[0], r[1], w, h], clip,
                    crate::render::LayerSpec { opacity: own * pinh.paint_opacity, blend: n.vis.blend }));
            }
            // Include native surface geometry, which lives beside the primitive stream.
            if n.vis.surface.is_some() {
                for &(_, at, _) in &layers {
                    let b = &mut self.instances[at].rect;
                    let hi = [(b[0] + b[2]).max(r[2].min(clip[2])), (b[1] + b[3]).max(r[3].min(clip[3]))];
                    b[0] = b[0].min(r[0].max(clip[0])); b[1] = b[1].min(r[1].max(clip[1]));
                    b[2] = hi[0] - b[0]; b[3] = hi[1] - b[1];
                }
            }

            let first_instance = self.instances.len();
            if effective_opacity > 0.001 && o > 0.001 && visible {
                // Disabled styling is distributed to each authored paint leaf at build time.
                // Keep this local to emission: it neither propagates to children nor opens scopes.
                let o = o * n.vis.paint_alpha;
                let k_px = sc * s;
                for (k, sh) in n.vis.shadows.iter().enumerate() {
                    let c = if k < P_SHADOWS.len() { self.color(n.id, P_SHADOWS[k], sh.color) } else { sh.color };
                    if c.a <= 0.0 || sh.inset {
                        continue;
                    }
                    let sp = sh.spread * k_px;
                    let rect = [r[0] + sh.x * k_px - sp, r[1] + sh.y * k_px - sp, w + 2.0 * sp, h + 2.0 * sp];
                    let sradii = radii.map(|v| if v > 0.0 { (v + sp).max(0.0) } else { 0.0 });
                    if sh.blur + blur <= 0.0 {
                        // A crisp shadow: a ring when it sits exactly around the box, else a solid
                        // offset copy (hidden under an opaque element, as in CSS).
                        let ring = sh.x == 0.0 && sh.y == 0.0;
                        if ring && sp <= 0.0 {
                            continue;
                        }
                        self.instances.push(Instance {
                            rect,
                            radii: sradii,
                            fill: if ring { [0.0; 4] } else { c.premul(o) },
                            border: if ring { c.premul(o) } else { [0.0; 4] },
                            params: [if ring { sp } else { 0.0 }, KIND_RECT, 0.0, 1.0],
                            uv: [0.0; 4],
                            clip,
                            fade: crate::render::pack_fade(fade, clip, clip),
                        });
                        continue;
                    }
                    self.instances.push(Instance {
                        rect,
                        radii: sradii,
                        fill: c.premul(o),
                        border: [0.0; 4],
                        params: [0.0, KIND_SHADOW, ((sh.blur + blur) * s * 0.5).max(0.5), 1.0],
                        uv: [0.0; 4],
                        clip,
                        fade: crate::render::pack_fade(fade, clip, clip),
                    });
                }
                let mut overlay = [0.0f32; 4];
                if n.vis.hover.a > 0.0 || self.scene.store().is_animating(n.id, P_HOVER_C) {
                    let hov_v = self.scene.get(n.id, P_HOVER).unwrap_or(0.0).clamp(0.0, 1.0);
                    if hov_v > 0.0 {
                        overlay = over(self.color(n.id, P_HOVER_C, n.vis.hover).premul(hov_v), overlay);
                    }
                }
                if n.vis.press.a > 0.0 || self.scene.store().is_animating(n.id, P_PRESS_C) {
                    let prs_v = self.scene.get(n.id, P_PRESS).unwrap_or(0.0).clamp(0.0, 1.0);
                    if prs_v > 0.0 {
                        overlay = over(self.color(n.id, P_PRESS_C, n.vis.press).premul(prs_v), overlay);
                    }
                }
                let base = self.color(n.id, P_FILL, n.vis.fill).premul(1.0);
                let layered = n.vis.gradient.is_some() || n.vis.surface.is_some();
                let fill = if layered { base } else { over(overlay, base) }.map(|c| c * o);
                let bw = n.vis.border_width * sc * s;
                let border = if bw > 0.0 { self.color(n.id, P_BORDER, n.vis.border).premul(o) } else { [0.0; 4] };
                if layered {
                    // Fill, gradient and overlays first, then the border on top, as in CSS.
                    if fill[3] > 0.0005 {
                        self.instances.push(Instance {
                            rect: [r[0], r[1], w, h],
                            radii,
                            fill,
                            border: [0.0; 4],
                            params: [0.0, KIND_RECT, blur * s, 1.0],
                            uv: [0.0; 4],
                            clip,
                            fade: crate::render::pack_fade(fade, clip, clip),
                        });
                    }
                    let g = n.vis.gradient.unwrap_or_default();
                    let top = self.color(n.id, P_GRADIENT[0], g[0]).premul(o);
                    let bottom = self.color(n.id, P_GRADIENT[1], g[1]).premul(o);
                    if top[3] > 0.0005 || bottom[3] > 0.0005 {
                        self.instances.push(Instance {
                            rect: [r[0], r[1], w, h],
                            radii,
                            fill: top,
                            border: [0.0; 4],
                            params: [0.0, KIND_GRADIENT, 0.0, 1.0],
                            uv: bottom,
                            clip,
                            fade: crate::render::pack_fade(fade, clip, clip),
                        });
                    }
                    if let Some((surface, fit)) = &n.vis.surface {
                        self.surfaces.push(crate::native_surface::SurfaceDraw {
                            surface: surface.clone(), fit: *fit, before_instance: self.instances.len() as u32,
                            rect: [r[0], r[1], w, h], radii, clip, fade: crate::render::pack_fade(fade, fade_origin, clip), opacity: o,
                        });
                    }
                    let ov = overlay.map(|c| c * o);
                    if ov[3] > 0.0005 || border[3] > 0.0005 {
                        self.instances.push(Instance {
                            rect: [r[0], r[1], w, h],
                            radii,
                            fill: ov,
                            border,
                            params: [bw, KIND_RECT, blur * s, 1.0],
                            uv: [0.0; 4],
                            clip,
                            fade: crate::render::pack_fade(fade, clip, clip),
                        });
                    }
                } else if fill[3] > 0.0005 || border[3] > 0.0005 {
                    self.instances.push(Instance {
                        rect: [r[0], r[1], w, h],
                        radii,
                        fill,
                        border,
                        params: [bw, KIND_RECT, blur * s, 1.0],
                        uv: [0.0; 4],
                        clip,
                        fade: crate::render::pack_fade(fade, clip, clip),
                    });
                }
                for (k, sh) in n.vis.shadows.iter().enumerate() {
                    if !sh.inset {
                        continue;
                    }
                    let c = if k < P_SHADOWS.len() { self.color(n.id, P_SHADOWS[k], sh.color) } else { sh.color };
                    if c.a <= 0.0 {
                        continue;
                    }
                    self.instances.push(Instance {
                        rect: [r[0], r[1], w, h],
                        radii,
                        fill: c.premul(o),
                        border: [0.0; 4],
                        params: [sh.spread * k_px, KIND_INSET, if sh.blur > 0.0 { (sh.blur * s * 0.5).max(0.5) } else { 0.0 }, 1.0],
                        uv: [sh.x * k_px, sh.y * k_px, 0.0, 0.0],
                        clip,
                        fade: crate::render::pack_fade(fade, clip, clip),
                    });
                }
                let focus_ring = self.focus_ring_visible(i).then_some(n.focus.ring).flatten();
                if let Some(ol) = focus_ring.or(n.vis.outline) {
                    let c = if focus_ring.is_some() { ol.color } else { self.color(n.id, P_OUTLINE, ol.color) };
                    let g = (ol.offset + ol.width) * k_px;
                    if c.a > 0.0 && ol.width > 0.0 {
                        self.instances.push(Instance {
                            rect: [r[0] - g, r[1] - g, w + 2.0 * g, h + 2.0 * g],
                            radii: radii.map(|v| if v > 0.0 { v + g } else { 0.0 }),
                            fill: [0.0; 4],
                            border: c.premul(o),
                            params: [ol.width * k_px, KIND_RECT, 0.0, 1.0],
                            uv: [0.0; 4],
                            clip,
                            fade: crate::render::pack_fade(fade, clip, clip),
                        });
                    }
                }
                if let Some(shape) = &n.vis.shape {
                    let k = match shape.view_box {
                        Some([vw, vh]) => [w / vw, h / vh],
                        None => [k_px, k_px],
                    };
                    push_shape(&mut self.instances, shape, [r[0], r[1]], k, k_px, o, clip, fade);
                }
                if let Some(pixels) = &n.vis.pixels
                    && let Some(image) = self.images.place(pixels, [w, h])
                {
                    self.instances.push(Instance {
                        rect: [r[0], r[1], w, h],
                        radii,
                        fill: [0.0, 0.0, 0.0, o],
                        border: [0.0; 4],
                        params: [image.page as f32, KIND_IMAGE, image.premultiplied as u8 as f32, 1.0],
                        uv: image.rect.map(|v| v as f32),
                        clip,
                        fade: crate::render::pack_fade(fade, clip, clip),
                    });
                }
                if let Some((ic, c)) = n.vis.icon {
                    let color = text_tint(self.color(n.id, P_TEXT, c));
                    let px = (w.min(h)).round().max(1.0) as u32;
                    if let Some(q) = self.text.icon_quad(ic, px, [r[0], r[1]]) {
                        self.instances.push(Instance {
                            rect: q.rect,
                            radii: [0.0; 4],
                            fill: color.premul(o),
                            border: [0.0; 4],
                            params: [if matches!(n.looping.map(|l| l.kind), Some(rune_motion::LoopKind::Rotation)) { loop_value.unwrap_or(0.0) } else { 0.0 }, KIND_GLYPH, 0.0, 1.0],
                            uv: [q.uv[0], q.uv[1], 0.0, 0.0],
                            clip,
                            fade: crate::render::pack_fade(fade, clip, clip),
                        });
                    }
                } else if let (Some(spec), Some(layout)) = (&n.text, &n.text_layout) {
                    let color = text_tint(self.color(n.id, P_TEXT, spec.color));
                    let tw = layout.full_width().ceil();
                    let ins = n.inset.map(|v| v * sc * s);
                    let (x0, x1) = (r[0] + ins[0], r[2] - ins[2]);
                    // Wrapped text aligns each line inside the wrap width itself.
                    let x = match spec.align {
                        _ if line_alignment(spec) != parley::Alignment::Start => x0,
                        Align::Start => x0,
                        Align::Center => x0 + (x1 - x0 - tw) * 0.5,
                        Align::End => x1 - tw,
                    };
                    let mut origin = [x, r[1] + ins[1]];
                    let mut clip = clip;
                    if let Some(inp) = n.input.as_ref().filter(|i| i.single_line) {
                        let scroll = single_line_scroll(&mut self.input_scroll, &n.path, inp, &spec.content, layout, s, x1 - x0);
                        origin[0] -= scroll;
                        if layout.full_width() > x1 - x0 {
                            clip = [clip[0].max(x0), clip[1], clip[2].min(x1), clip[3]];
                        }
                    }
                    // Text fields: selection under the text, placeholder, caret over it.
                    let mut caret = None;
                    if let Some(inp) = &n.input {
                        let e = &inp.edit;
                        let departing = self.flights.iter().any(|f| n.key.as_deref() == Some(&f.source_key)
                            && f.alphas(f.destination, now, self.reduced_motion).0 > 0.0);
                        if e.text.is_empty() && e.preedit.is_none() && !inp.placeholder.is_empty() && !departing {
                            let pl = self.text.layout_rich(&inp.placeholder, &[], spec.style, s, Some(w));
                            self.glyph_scratch.clear();
                            self.text.glyphs_for_layout(&pl, s, origin, false, &mut self.glyph_scratch);
                            let pc = inp.placeholder_color.premul(o);
                            for q in &self.glyph_scratch {
                                self.instances.push(glyph(q, pc, glyph_gamma(inp.placeholder_color, s), clip, fade));
                            }
                        }
                        if e.focused && !departing {
                            let (a, b) = e.range();
                            if a != b && e.preedit.is_none() {
                                let sel = Selection::new(
                                    Cursor::from_byte_index(layout, e.display_index(a), Affinity::Downstream),
                                    Cursor::from_byte_index(layout, e.display_index(b), Affinity::Upstream),
                                );
                                for (bb, _) in sel.geometry(layout) {
                                    self.instances.push(rect_inst(
                                        [origin[0] + bb.x0 as f32, origin[1] + bb.y0 as f32, (bb.x1 - bb.x0) as f32, (bb.y1 - bb.y0) as f32],
                                        inp.selection.premul(o),
                                        2.0 * s,
                                        clip,
                                        fade,
                                    ));
                                }
                            }
                            let ci = caret_index(e, &spec.content);
                            let cur = Cursor::from_byte_index(layout, ci, Affinity::Downstream);
                            let bb = cur.geometry(layout, 1.5 * s);
                            let line = spec.style.line_px() * s;
                            let (cx0, cy0) = (origin[0] + bb.x0 as f32, origin[1] + bb.y0 as f32);
                            let ch = if bb.y1 > bb.y0 { (bb.y1 - bb.y0) as f32 } else { line };
                            caret = Some([cx0.round(), cy0 + ch * 0.1, (1.5 * s).round(), ch * 0.8]);
                            if !e.is_secure() { self.ime_area = Some([cx0 / s, cy0 / s, 2.0, ch / s]); }
                            if let Some((_, _)) = &e.preedit
                                && let (_, Some((pa, pb))) = e.display()
                            {
                                let sel = Selection::new(
                                    Cursor::from_byte_index(layout, pa, Affinity::Downstream),
                                    Cursor::from_byte_index(layout, pb, Affinity::Upstream),
                                );
                                for (bb, _) in sel.geometry(layout) {
                                    self.instances.push(rect_inst(
                                        [origin[0] + bb.x0 as f32, origin[1] + bb.y1 as f32 - s, (bb.x1 - bb.x0) as f32, s],
                                        color.premul(o),
                                        0.0,
                                        clip,
                                        fade,
                                    ));
                                }
                            }
                        }
                    }
                    // Span backgrounds (inline code), then glyphs.
                    self.deco_scratch.clear();
                    self.text.decorations(layout, &spec.spans, s, origin, &mut self.deco_scratch);
                    for d in &self.deco_scratch {
                        let c = d.color.premul(o);
                        self.instances.push(rect_inst(d.rect, c, d.radius, clip, fade));
                    }
                    let reveal = self.reveals.get(&n.id).filter(|st| !st.words.is_empty());
                    self.glyph_scratch.clear();
                    self.text.glyphs_for_layout(layout, s, origin, reveal.is_some(), &mut self.glyph_scratch);
                    let gamma = glyph_gamma(color, s);
                    let base = color.premul(o);
                    for q in &self.glyph_scratch {
                        if self.flights.iter().any(|f| n.key.as_deref() == Some(&f.target_key)) { continue; }
                        if q.rect[1] > clip[3] || q.rect[1] + q.rect[3] < clip[1] {
                            continue;
                        }
                        let (mut f, mut gm) = (base, gamma);
                        if q.ink > 0
                            && let Some(c) = spec.spans.get(q.ink as usize - 1).and_then(|sp| sp.color)
                        {
                            f = c.premul(o);
                            gm = glyph_gamma(c, s);
                        }
                        if let Some(st) = reveal {
                            let a = Self::reveal_alpha(st, q.byte, now);
                            if a <= 0.0 {
                                continue;
                            }
                            f = f.map(|v| v * a);
                        }
                        self.instances.push(glyph(q, f, gm, clip, fade));
                        glyph_count += 1;
                    }
                    if let (Some(c), Some(inp)) = (caret, &n.input) {
                        self.instances.push(rect_inst(c, inp.caret.premul(o), 0.0, clip, fade));
                    }
                }
            }
            let packed_fade = crate::render::pack_fade(fade, fade_origin, clip);
            for instance in &mut self.instances[first_instance..] {
                instance.fade = packed_fade;
            }
            // Text-only clipping leaves the box's fill/border and descendants untouched.
            if let (Some(spec), Some(layout)) = (&n.text, &n.text_layout)
                && (spec.truncate_fade || spec.live.is_some())
            {
                let ins = n.inset.map(|v| v * sc * s);
                let bounds = [r[0] + ins[0], r[1] + ins[1], r[2] - ins[2], r[3] - ins[3]];
                let text_clip = [clip[0].max(bounds[0]), clip[1].max(bounds[1]), clip[2].min(bounds[2]), clip[3].min(bounds[3])];
                let mut widths = fade;
                let mut origins = fade_origin;
                // Layout width already includes display scale `s`; bounds also include the
                // element/ancestor transform `sc`. Compare both after that transform.
                if spec.truncate_fade && layout.full_width() * sc > bounds[2] - bounds[0] + 0.5 * sc {
                    let width = (spec.style.size * sc * s).min((bounds[2] - bounds[0]).max(0.0));
                    let own = if spec.align == Align::End { [width, 0.0, 0.0, 0.0] }
                        else { [0.0, 0.0, width, 0.0] };
                    inherit_fade(&mut widths, &mut origins, bounds, own);
                }
                for instance in &mut self.instances[first_instance..] {
                    if instance.params[1] == KIND_GLYPH {
                        instance.clip = text_clip;
                        instance.fade = crate::render::pack_fade(widths, origins, text_clip);
                    }
                }
            }
            if self.track_marks {
                self.marks.resize(self.instances.len(), mark);
            }
            // A scroll viewport clips its content with a hard edge unless it sets soft ones.
            if let Some(f) = n.vis.clip.or(n.scroll.map(|_| [0.0; 4])) {
                clip = [r[0].max(clip[0]), r[1].max(clip[1]), r[2].min(clip[2]), r[3].min(clip[3])];
                inherit_fade(&mut fade, &mut fade_origin, r, f.map(|v| v * sc * s));
            }
            let mask = self.row_mask[i];
            if mask >= 0.0 {
                clip[3] = clip[3].min(r[3]);
                inherit_fade(&mut fade, &mut fade_origin, r, [0.0, 0.0, 0.0, mask * s]);
            }
            let mut dy = pinh.dy;
            if n.scroll.is_some()
                && let Some(st) = self.scrolls.get(&n.id)
            {
                let origin = self.virtuals.values().find(|v| v.scroll_path == n.path).map_or(0.0, |v| f64::from(v.before));
                dy -= grid((st.offset - origin) as f32, s);
            }
            self.inh_scratch[i] = Inh { opacity: effective_opacity, paint_opacity: o, clip, fade, fade_origin, scale: sc, anchor, blur, dy, mark, group };
            if (n.vis.hit || n.on_pointer_move.is_some() || n.on_press.is_some() || n.on_secondary_press.is_some() || n.on_hover.is_some() || n.on_drag.is_some() || n.input.is_some() || n.window_drag || n.focus.enabled || n.modal.is_some())
                && !self.subtree_exiting(i)
                && self.allowed(i) && effective_opacity > 0.001 && visible
            {
                let c = self.inh_scratch[n.parent.unwrap_or(0)].clip;
                let cr = [
                    rect_pt[0].max(c[0] / s),
                    rect_pt[1].max(c[1] / s),
                    rect_pt[2].min(c[2] / s),
                    rect_pt[3].min(c[3] / s),
                ];
                let rect = if n.parent.is_some() { cr } else { rect_pt };
                self.hits.push((n.id, rect));
                if n.vis.hit && effective_opacity > 0.0 && rect[2] > rect[0] && rect[3] > rect[1] {
                    self.overlay_hits.push(rect);
                }
            }
        }
        while let Some((_, start, surface_start)) = layers.pop() {
            crate::render::end_layer(&mut self.instances, &mut self.surfaces, &mut self.marks, start, surface_start);
        }
        self.layer_scratch = layers;
        if self.track_marks { self.marks.resize(self.instances.len(), 0); }
        self.stats.glyphs = glyph_count;
        self.ambient = ambient;
        // A moving chip can enter a stationary pointer. Re-evaluate against this frame's
        // geometry without inventing pointer-move or drag events on animation frames.
        if self.native_overlay && let Some((x, y)) = self.pointer { self.refresh_hover(x, y); }
    }

    // ---------------------------------------------------------------------------------------
    // Inspection

    pub fn elements(&self) -> Vec<ElementInfo> {
        fn live_intent(n: &Node) -> u32 {
            n.live.iter().filter(|b| b.spring.is_none()).fold(0, |bits, b| bits | match b.property {
                crate::live::Property::Offset => 1,
                crate::live::Property::Scale | crate::live::Property::Rect => 1 | 2,
                crate::live::Property::Opacity => 4,
                crate::live::Property::Fill => 0,
            })
        }
        let store = self.scene.store();
        let mut elements: Vec<_> = self.nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                let p = self.scene.presented(n.id)?;
                let flight = self.flights.iter().find(|f| n.key.as_deref() == Some(&f.target_key));
                let mut moving = flight.is_some() || (!self.reduced_motion && self.loops.get(&n.id).is_some_and(|l| l.active(self.t)))
                    || store.is_animating(n.id, prop::FRAME)
                    || store.is_animating(n.id, prop::TRAVEL)
                    || store.is_animating(n.id, prop::PRESENCE)
                    || store.is_animating(n.id, prop::OFFSET) || store.is_animating(n.id, prop::SCALE)
                    || self.grows.get(&n.id).is_some_and(|g| g.moving());
                let rect = flight.map_or_else(|| self.drawn.get(i).copied().unwrap_or([p.rect.x, p.rect.y, p.rect.w, p.rect.h]),
                    |f| [f.presented.x, f.presented.y, f.presented.w, f.presented.h]);
                let s = self.env.scale;
                let clip = flight.map_or_else(|| n.parent.and_then(|pi| self.inh_scratch.get(pi)).map_or(NO_CLIP, |h| h.clip),
                    |f| f.clip.map(|v| v * s));
                let (x0, y0) = (rect[0].max(clip[0] / s), rect[1].max(clip[1] / s));
                let (x1, y1) = ((rect[0] + rect[2]).min(clip[2] / s), (rect[1] + rect[3]).min(clip[3] / s));
                let visible = [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)];
                let revealing = flight.is_some() || self.reveals.get(&n.id).is_some_and(|r| !r.words.is_empty());
                let mut in_growing_row = false;
                let mut scrolling = false;
                let mut live_intent = live_intent(n);
                // A group_opacity swap under an instant hover group is a deliberate cut (Beta's
                // status/close swap), for it and what it contains: opacity and transition intent.
                let up = |from: Option<usize>| std::iter::successors(from, |&pi| self.nodes[pi].parent);
                if let Some(gi) = up(Some(i)).find(|&j| self.nodes[j].vis.group_opacity.is_some()) {
                    let group = up(self.nodes[gi].parent)
                        .map(|pi| &self.nodes[pi].vis)
                        .find(|v| v.hover.a > 0.0 || v.hover_group);
                    if group.and_then(|v| v.hover_motion).is_some_and(|(e, x)| [e, x].iter().any(|a| matches!(a.curve, rune_motion::Curve::Instant))) {
                        live_intent |= 4 | 16;
                    }
                }
                let mut a = n.parent;
                while let Some(pi) = a {
                    let parent = self.nodes[pi].id;
                    moving |= store.is_animating(parent, prop::FRAME) || store.is_animating(parent, prop::TRAVEL)
                        || store.is_animating(parent, prop::PRESENCE)
                        || store.is_animating(parent, prop::OFFSET) || store.is_animating(parent, prop::SCALE);
                    for b in &self.nodes[pi].live {
                        if b.spring.is_none() {
                            live_intent |= match b.property {
                                crate::live::Property::Offset => 1,
                                crate::live::Property::Scale | crate::live::Property::Rect => 1 | 2,
                                crate::live::Property::Opacity => 4,
                                crate::live::Property::Fill => 0,
                            };
                        }
                    }
                    scrolling |= self.scrolls.get(&self.nodes[pi].id).is_some_and(|s| s.scrolling);
                    if self.grows.get(&self.nodes[pi].id).is_some_and(|g| g.moving()) {
                        in_growing_row = true;
                    }
                    a = self.nodes[pi].parent;
                }
                Some(ElementInfo {
                    loop_phase: self.loops.get(&n.id).map(|l| if self.reduced_motion { 0.0 } else { l.phase(self.t) }),
                    focusable: self.can_focus(i),
                    focused: self.focused.as_deref() == Some(&n.path),
                    focus_visible: self.keyboard_focus && self.focused.as_deref() == Some(&n.path),
                    inert: !self.allowed(i) || self.subtree_exiting(i),
                    path: n.path.clone(),
                    key: n.key.clone(),
                    role: n.role,
                    label: n.label.clone(),
                    text: n.text.as_ref().filter(|_| !n.input.as_ref().is_some_and(|i| i.edit.is_secure())).map(|t| t.content.clone()),
                    rect,
                    target: [n.target[0], n.target[1], n.target[2] - n.target[0], n.target[3] - n.target[1]],
                    opacity: flight.map_or_else(|| self.inh_scratch.get(i).map_or(p.opacity, |h| h.opacity), |f| f.target_alpha),
                    radius: p.radius.0[0],
                    fill: self.color(n.id, P_FILL, n.vis.fill),
                    text_color: n.text.as_ref().map(|t| t.color).or(n.vis.icon.map(|(_, c)| c)).map(|c| self.color(n.id, P_TEXT, c)),
                    hover: self.scene.get(n.id, P_HOVER).unwrap_or(0.0),
                    press: self.scene.get(n.id, P_PRESS).unwrap_or(0.0),
                    moving,
                    parent: n.parent,
                    visible,
                    revealing,
                    in_growing_row,
                    scrolling,
                    live_intent,
                    virtual_extent: self.virtuals.contains_key(&n.path) && n.vis.fill.a == 0.0
                        && n.vis.border_width == 0.0 && n.vis.shadows.is_empty() && n.text.is_none() && n.vis.icon.is_none(),
                })
            })
            .collect();
        for f in &self.flights {
            let r = if self.reduced_motion { f.source } else { f.presented };
            let c = if self.reduced_motion { f.source_clip } else { f.clip };
            let path = format!("flight-source/{}", f.target_key);
            let t = f.destination;
            elements.push(ElementInfo {
                loop_phase: None,
                focusable: false, focused: false, focus_visible: false, inert: true,
                key: Some(path.clone()), path, role: "text", label: None, text: None,
                rect: [r.x, r.y, r.w, r.h], target: [t.x, t.y, t.w, t.h],
                opacity: f.source_alpha, radius: 0.0, fill: Color::TRANSPARENT, text_color: None,
                hover: 0.0, press: 0.0, moving: true, parent: None,
                visible: [r.x.max(c[0]), r.y.max(c[1]), (r.edges()[2].min(c[2]) - r.x.max(c[0])).max(0.0),
                    (r.edges()[3].min(c[3]) - r.y.max(c[1])).max(0.0)],
                revealing: true, in_growing_row: false, scrolling: false, virtual_extent: false, live_intent: 0,
            });
        }
        elements
    }

    /// Centre of the first non-exiting element whose key is `key`, or whose path ends with `key`
    /// when it contains a `/` (e.g. `slider/knob`), in points.
    pub fn find(&self, key: &str) -> Option<(f32, f32)> {
        let i = if key.contains('/') {
            let suffix = format!("/{key}");
            self.nodes.iter().position(|n| !self.is_exiting(n.id) && (n.path == key || n.path.ends_with(&suffix)))?
        } else {
            self.nodes.iter().position(|n| !self.is_exiting(n.id) && n.key.as_deref() == Some(key))?
        };
        let r = self.drawn.get(i).copied()?;
        Some((r[0] + r[2] * 0.5, r[1] + r[3] * 0.5))
    }

    /// Drawn rect `[x, y, w, h]` in points and effective opacity (ancestors included) of the
    /// element keyed `key`, exiting or not, as of the last frame.
    pub fn presented(&self, key: &str) -> Option<([f32; 4], f32)> {
        let i = self.nodes.iter().position(|n| n.key.as_deref() == Some(key))?;
        Some((*self.drawn.get(i)?, self.inh_scratch.get(i)?.opacity))
    }

    pub fn clear_color(&self) -> [f32; 4] {
        if self.transparent { return [0.0; 4]; }
        self.nodes.first().map_or([0.0, 0.0, 0.0, 1.0], |n| {
            let c = self.color(n.id, P_FILL, n.vis.fill);
            [c.r, c.g, c.b, 1.0]
        })
    }
}

fn merge(tx: &mut Tx, one: Tx) {
    tx.paste |= one.paste;
    tx.scroll_to.extend(one.scroll_to);
    tx.wakes.extend(one.wakes);
    tx.text_flights.extend(one.text_flights);
    if one.copy.is_some() { tx.copy = one.copy; }
    tx.shell = tx.shell.max(one.shell);
    tx.instant |= one.instant;
    tx.settle |= one.settle;
    tx.motion = one.motion.or(tx.motion);
    tx.color_motion = one.color_motion.or(tx.color_motion);
}

trait CeilTo {
    fn ceil_to(self, scale: f32) -> f32;
}

impl CeilTo for f32 {
    /// Rounds up to whole device pixels.
    fn ceil_to(self, scale: f32) -> f32 {
        (self * scale - 1e-3).ceil() / scale
    }
}

/// How far a single-line field's text shifts left (device px) so its caret stays inside
/// `visible` device px. Unfocused fields rest at their start, like a native text field.
fn single_line_scroll(scrolls: &mut HashMap<String, f32>, path: &str, inp: &crate::el::InputSpec, content: &str, layout: &Layout<Ink>, s: f32, visible: f32) -> f32 {
    if !inp.edit.focused {
        scrolls.remove(path);
        return 0.0;
    }
    let caret_w = (1.5 * s).round();
    let caret = Cursor::from_byte_index(layout, caret_index(&inp.edit, content), Affinity::Downstream);
    let cx = caret.geometry(layout, 1.5 * s).x0 as f32;
    let mut scroll = scrolls.get(path).copied().unwrap_or(0.0) * s;
    if cx - scroll > visible - caret_w {
        scroll = cx - visible + caret_w;
    }
    scroll = scroll.min(cx).clamp(0.0, (layout.full_width() + caret_w - visible).max(0.0));
    scrolls.insert(path.to_owned(), scroll / s);
    scroll
}

/// The display byte index the caret sits at, inside any IME preedit.
fn caret_index(e: &crate::edit::TextEdit, content: &str) -> usize {
    match &e.preedit {
        Some((p, Some((_, c)))) => (e.range().0 + (*c).min(p.len())).min(content.len()),
        Some((p, None)) => (e.range().0 + p.len()).min(content.len()),
        None => e.display_index(e.focus).min(content.len()),
    }
}

/// Parley line alignment for a text node: only wrapped text aligns per line.
fn line_alignment(spec: &TextSpec) -> parley::Alignment {
    match spec.align {
        Align::Center if spec.wrap => parley::Alignment::Center,
        Align::End if spec.wrap => parley::Alignment::End,
        _ => parley::Alignment::Start,
    }
}

/// The longest prefix of `content` (at a char boundary, trailing space dropped) that fits in
/// `max_w` device px with an ellipsis.
fn truncated(text: &mut TextSystem, content: &str, spec: &TextSpec, scale: f32, max_w: f32) -> Rc<Layout<Ink>> {
    let bounds: Vec<usize> = content.char_indices().map(|(i, _)| i).collect();
    let (mut lo, mut hi) = (0usize, bounds.len());
    let make = |text: &mut TextSystem, n: usize| {
        let cut = bounds.get(n).copied().unwrap_or(content.len());
        let s = format!("{}…", content[..cut].trim_end());
        text.layout_rich(&s, &spec.spans, spec.style, scale, None)
    };
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if make(text, mid).full_width() <= max_w + 0.5 { lo = mid } else { hi = mid - 1 }
    }
    make(text, lo)
}

// Carry the declaring edge with the inherited width. Descendant clipping must not restart
// its ramp; an inner fade takes over once the ancestor's ramp is fully opaque there.
fn inherit_fade(width: &mut [f32; 4], origin: &mut [f32; 4], rect: [f32; 4], own: [f32; 4]) {
    for edge in 0..4 {
        let inset = if edge < 2 { rect[edge] - origin[edge] } else { origin[edge] - rect[edge] };
        if own[edge] > 0.0 && (own[edge] >= width[edge] || inset >= width[edge]) {
            width[edge] = own[edge];
            origin[edge] = rect[edge];
        }
    }
}

fn glyph(q: &GlyphQuad, fill: [f32; 4], gamma: f32, clip: [f32; 4], fade: [f32; 4]) -> Instance {
    Instance {
        rect: q.rect,
        radii: [0.0; 4],
        fill,
        border: [0.0; 4],
        params: [0.0, KIND_GLYPH, 0.0, gamma],
        uv: [q.uv[0], q.uv[1], 0.0, 0.0],
        clip,
        fade: crate::render::pack_fade(fade, clip, clip),
    }
}

/// Where an anchored overlay of `size` goes next to `t` (x0, y0, x1, y1), flipped to the
/// opposite side when it doesn't fit, then kept 8 points inside the viewport.
fn anchor_position(a: &crate::el::Anchor, t: [f32; 4], size: (f32, f32), vp: (f32, f32)) -> (f32, f32) {
    use crate::el::Side;
    let (w, h) = size;
    let margin = 8.0;
    let along = |lo: f32, hi: f32, len: f32| match a.align {
        Align::Start => lo,
        Align::Center => (lo + hi - len) * 0.5,
        Align::End => hi - len,
    };
    let (mut x, mut y) = match a.side {
        Side::Top => (along(t[0], t[2], w), t[1] - h - a.gap),
        Side::Bottom => (along(t[0], t[2], w), t[3] + a.gap),
        Side::Left => (t[0] - w - a.gap, along(t[1], t[3], h)),
        Side::Right => (t[2] + a.gap, along(t[1], t[3], h)),
    };
    // Flip the side when the other one fits (Floating UI's `flip`).
    match a.side {
        Side::Bottom if y + h > vp.1 - margin && t[1] - h - a.gap >= margin => y = t[1] - h - a.gap,
        Side::Top if y < margin && t[3] + a.gap + h <= vp.1 - margin => y = t[3] + a.gap,
        Side::Right if x + w > vp.0 - margin && t[0] - w - a.gap >= margin => x = t[0] - w - a.gap,
        Side::Left if x < margin && t[2] + a.gap + w <= vp.0 - margin => x = t[2] + a.gap,
        _ => {}
    }
    // Flip the alignment when it overflows and the other end fits, then shift along the side.
    let vertical = matches!(a.side, Side::Top | Side::Bottom);
    let (pos, lo, hi, len, room) = if vertical { (&mut x, t[0], t[2], w, vp.0) } else { (&mut y, t[1], t[3], h, vp.1) };
    match a.align {
        Align::End if *pos < margin && lo + len <= room - margin => *pos = lo,
        Align::Start if *pos + len > room - margin && hi - len >= margin => *pos = hi - len,
        _ => {}
    }
    *pos = pos.clamp(margin, (room - len - margin).max(margin));
    // Along the main axis only keep the start edge inside; a long list may run off the far end.
    if vertical { y = y.max(margin) } else { x = x.max(margin) }
    (x, y)
}

/// Instances for a vector shape whose origin sits at `origin` (device px), with `k` device px per
/// coordinate unit on each axis and `k_px` device px per point for stroke widths.
#[allow(clippy::too_many_arguments)]
fn push_shape(out: &mut Vec<Instance>, shape: &Shape, origin: [f32; 2], k: [f32; 2], k_px: f32, opacity: f32, clip: [f32; 4], fade: [f32; 4]) {
    let at = |p: [f32; 2]| [origin[0] + p[0] * k[0], origin[1] + p[1] * k[1]];
    for op in &shape.ops {
        match op {
            Op::Stroke { points, closed, width, cap, color } => {
                let fill = color.premul(opacity);
                if fill[3] <= 0.0005 {
                    continue;
                }
                let hw = width * k_px * 0.5;
                let pad = hw.max(0.5) + 1.0;
                let n = points.len();
                let count = if *closed { n } else { n - 1 };
                for i in 0..count {
                    let (a, b) = (at(points[i]), at(points[(i + 1) % n]));
                    let (x0, y0) = (a[0].min(b[0]) - pad, a[1].min(b[1]) - pad);
                    out.push(Instance {
                        rect: [x0, y0, a[0].max(b[0]) + pad - x0, a[1].max(b[1]) + pad - y0],
                        radii: [a[0], a[1], b[0], b[1]],
                        fill,
                        border: [0.0; 4],
                        params: [hw, KIND_SEGMENT, if *cap == LineCap::Round { 1.0 } else { 0.0 }, 1.0],
                        uv: [0.0; 4],
                        clip,
                        fade: crate::render::pack_fade(fade, clip, clip),
                    });
                }
            }
            Op::Fill { points, color } => {
                let fill = color.premul(opacity);
                if fill[3] <= 0.0005 {
                    continue;
                }
                for ([ia, ib, ic], aa) in crate::shape::triangulate(points) {
                    let (a, b, c) = (at(points[ia]), at(points[ib]), at(points[ic]));
                    let (x0, y0) = (a[0].min(b[0]).min(c[0]), a[1].min(b[1]).min(c[1]));
                    let (x1, y1) = (a[0].max(b[0]).max(c[0]), a[1].max(b[1]).max(c[1]));
                    out.push(Instance {
                        rect: [x0, y0, x1 - x0, y1 - y0],
                        radii: [a[0], a[1], b[0], b[1]],
                        fill,
                        border: [0.0; 4],
                        params: [aa as f32, KIND_TRIANGLE, 0.0, 1.0],
                        uv: [c[0], c[1], 0.0, 0.0],
                        clip,
                        fade: crate::render::pack_fade(fade, clip, clip),
                    });
                }
            }
        }
    }
}

fn rect_inst(rect: [f32; 4], fill: [f32; 4], radius: f32, clip: [f32; 4], fade: [f32; 4]) -> Instance {
    Instance {
        rect,
        radii: [radius; 4],
        fill,
        border: [0.0; 4],
        params: [0.0, KIND_RECT, 0.0, 1.0],
        uv: [0.0; 4],
        clip,
        fade: crate::render::pack_fade(fade, clip, clip),
    }
}

fn glyph_gamma(c: Color, scale: f32) -> f32 {
    crate::text::glyph_coverage_exponent(c, scale)
}

#[allow(clippy::too_many_arguments)]
fn flatten(
    el: El,
    parent: Option<usize>,
    parent_path: &str,
    index: usize,
    motion: Spring,
    in_row: bool,
    taffy: &mut TaffyTree<usize>,
    nodes: &mut Vec<Node>,
    entries: &mut HashMap<String, LayoutEntry>,
    scale: f32,
) -> taffy::NodeId {
    let seg = el.key.clone().unwrap_or_else(|| format!("#{index}"));
    let path = if parent_path.is_empty() { seg } else { format!("{parent_path}/{seg}") };
    let motion = el.motion.unwrap_or(motion);
    let me = nodes.len();
    let parent_is_stack = parent.is_some_and(|p| nodes[p].growth.is_some());
    let virtual_mount = el.virtual_mount || parent.is_some_and(|p| nodes[p].virtual_mount);
    let El {
        on_paste,
        on_window_focus,
        focus,
        modal,
        looping,
        key,
        role,
        label,
        style,
        vis,
        live,
        text,
        children,
        on_press,
        on_secondary_press,
        on_hover,
        on_pointer_move,
        above,
        on_drag,
        opacity_motion,
        lead,
        width_leads,
        transition,
        scroll,
        growth,
        streaming,
        input,
        cursor,
        window_drag,
        ..
    } = el;
    if let Some(t) = &text && t.live.is_some() {
        assert!(input.is_none() && !t.wrap && t.spans.is_empty() && t.reveal < 0.0 && !t.truncate,
            "live_text requires plain single-line text; use truncate_fade for overflow");
    }
    let measured = text.is_some();
    let measure = text.as_ref().map(|t| crate::text::text_hash(&t.content, &t.spans, t.style, scale, None) ^ u64::from(t.wrap) ^ (u64::from(t.truncate || t.truncate_fade) << 1));
    nodes.push(Node {
        on_paste,
        on_window_focus,
        focus,
        modal,
        looping,
        id: NodeId(0),
        path: path.clone(),
        key,
        role,
        label,
        vis,
        live,
        text,
        text_layout: None,
        live_text_seen: None,
        on_press,
        on_secondary_press,
        on_hover,
        on_pointer_move,
        above,
        on_drag,
        motion,
        opacity_motion,
        lead,
        width_leads,
        transition,
        scroll,
        growth,
        streaming,
        input,
        cursor,
        window_drag,
        in_row,
        virtual_mount,
        parent,
        taffy: taffy::NodeId::from(0u64),
        target: [0.0; 4],
        local: MRect::ZERO,
        inset: [0.0; 4],
    });
    let child_in_row = in_row || parent_is_stack;
    let kids: Vec<taffy::NodeId> = children
        .into_iter()
        .enumerate()
        .map(|(i, c)| flatten(c, Some(me), &path, i, motion, child_in_row, taffy, nodes, entries, scale))
        .collect();
    let id = if let Some(entry) = entries.get_mut(&path) {
        let id = entry.id;
        if taffy.style(id).unwrap() != &style {
            taffy.set_style(id, style).unwrap();
        }
        if !taffy.child_ids(id).eq(kids.iter().copied()) {
            taffy.set_children(id, &kids).unwrap();
        }
        // Context indices can move after insertion/reorder without invalidating measurements.
        // Only text/style/scale changes invalidate the cached leaf result.
        if entry.measure.is_some() != measured {
            taffy.set_node_context(id, measured.then_some(me)).unwrap();
        } else if measured {
            *taffy.get_node_context_mut(id).unwrap() = me;
        }
        if entry.measure != measure {
            taffy.mark_dirty(id).unwrap();
        }
        entry.measure = measure;
        entry.used = true;
        id
    } else if !kids.is_empty() {
        taffy.new_with_children(style, &kids).unwrap()
    } else if measured {
        taffy.new_leaf_with_context(style, me).unwrap()
    } else {
        taffy.new_leaf(style).unwrap()
    };
    if !entries.contains_key(&path) {
        if measured && taffy.get_node_context(id).is_none() {
            taffy.set_node_context(id, Some(me)).unwrap();
        }
        entries.insert(path, LayoutEntry { id, measure, used: true });
    }
    nodes[me].taffy = id;
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::el::{col, el};

    fn duplicate_view(_: &(), _: &mut Cx<()>) -> El {
        col().children([el().key("card"), el().key("card")])
    }

    #[test]
    fn overlay_hit_regions_follow_live_geometry_and_ancestor_clips_in_the_same_frame() {
        struct State { offset: crate::Live<[f32; 2]>, opacity: crate::Live<f32>, pressed: u32, hovered: u32, visible: bool }
        fn view(s: &State, _: &mut Cx<State>) -> El {
            el().child(el().size(60.0, 60.0).clip_soft(0.0)
                .children(s.visible.then(|| el().key("chip").size(40.0, 20.0).pin(Some(30.0), Some(0.0), None, None)
                    .live_offset(&s.offset).live_opacity(&s.opacity).hit()
                    .on_hover(|s: &mut State| s.hovered += 1).on_press(|s: &mut State| s.pressed += 1))))
                .child(el().size(10.0, 10.0).on_hover(|_: &mut State| {}))
        }
        let offset = crate::Live::new([0.0, 0.0]);
        let opacity = crate::Live::new(1.0);
        let mut rt = Runtime::new(State { offset: offset.clone(), opacity: opacity.clone(), pressed: 0, hovered: 0, visible: true }, view, (100.0, 100.0), Env { system_dark: true, scale: 2.0 });
        rt.native_overlay = true;
        rt.frame(0.0);
        assert_eq!(rt.hit_regions(), &[[30.0, 0.0, 60.0, 20.0]]);
        rt.pointer_move(15.0, 10.0);
        offset.set([20.0, 0.0]);
        rt.frame(0.0);
        assert_eq!(rt.hit_regions(), &[[50.0, 0.0, 60.0, 20.0]], "logical regions must use this frame's live offset, not the previous layout");
        offset.set([-20.0, 0.0]);
        rt.frame(0.0);
        assert_eq!(rt.hit_regions(), &[[10.0, 0.0, 50.0, 20.0]]);
        rt.pointer_down(15.0, 10.0);
        rt.pointer_up(15.0, 10.0);
        rt.frame(0.0);
        assert_eq!(rt.state.pressed, 1, "the native hit and Rune release must agree on the moved chip");
        assert_eq!(rt.state.hovered, 1, "a chip moving under a stationary pointer enters hover once");
        assert!(!rt.has_pending_input(), "stationary hover must not generate an endless frame loop");
        rt.pointer_down(15.0, 10.0);
        rt.pointer_up(55.0, 10.0);
        rt.frame(0.0);
        assert_eq!(rt.state.pressed, 1, "release inside the old layout but outside the drawn chip must cancel");
        opacity.set(0.0);
        rt.frame(0.0);
        assert!(rt.hit_regions().is_empty(), "transparent nodes and hover-only elements do not block native input");
        opacity.set(1.0);
        rt.update(|s, _| s.visible = false);
        rt.frame(0.0);
        assert!(rt.hit_regions().is_empty(), "exiting chips must immediately release native input");
    }

    #[test]
    fn overlay_pointer_observation_does_not_require_a_native_hit_region() {
        fn view(_: &Vec<PointerPress>, _: &mut Cx<Vec<PointerPress>>) -> El {
            el().on_pointer_move(|s: &mut Vec<PointerPress>, p| s.push(p))
        }
        let mut rt = Runtime::new(Vec::new(), view, (100.0, 100.0), Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        rt.pointer_move(20.0, 40.0);
        rt.frame(0.0);
        assert_eq!(rt.state, [PointerPress { x: 20.0, y: 40.0 }]);
        assert!(rt.hit_regions().is_empty());
    }

    #[test]
    fn overlay_idle_pointer_does_not_request_unchanged_frames() {
        fn view(_: &u32, _: &mut Cx<u32>) -> El {
            el().child(el().size(40.0, 20.0).hit().on_hover(|n: &mut u32| *n += 1))
        }
        let mut rt = Runtime::new(0, view, (100.0, 100.0), Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        assert!(rt.observe_pointer(5.0, 5.0), "enter must run the hover callback");
        rt.frame(0.0);
        assert_eq!(rt.state, 1);
        assert!(!rt.observe_pointer(6.0, 5.0), "movement within an unchanged chip needs no frame");
        assert!(!rt.observe_pointer(6.0, 5.0), "duplicate monitor/winit observations need no frame");
        assert!(!rt.observe_pointer(90.0, 90.0));
        assert!(!rt.observe_pointer(200.0, 90.0), "movement outside the canvas needs no frame");
        assert!(!rt.has_pending_input());
    }

    #[test]
    fn deleting_a_focused_row_does_not_call_its_stale_edit_accessor() {
        fn view(s: &Vec<crate::TextEdit>, _: &mut Cx<Vec<crate::TextEdit>>) -> El {
            col().children(s.iter().enumerate().map(|(i, edit)| {
                crate::text_input(edit, crate::TextStyle::new(14.0, 400.0))
                    .key(format!("row-{i}"))
                    .on_edit(crate::edit_access(move |s: &mut Vec<crate::TextEdit>| &mut s[i]))
            })).child(el().key("delete").size(40.0, 40.0).on_press(|s: &mut Vec<crate::TextEdit>| { s.clear(); }))
        }
        let mut edit = crate::TextEdit::default();
        edit.focused = true;
        let mut rt = Runtime::new(vec![edit], view, (100.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let path = rt.nodes.iter().find(|n| n.key.as_deref() == Some("delete")).unwrap().path.clone();
        rt.queue.push(Action::Press(path));
        rt.frame(0.0);
        assert!(rt.state.is_empty());
        assert!(!rt.has_focus());
    }

    #[test]
    fn secondary_only_children_preserve_primary_click_hover_and_background_blur() {
        type State = (u32, u32, bool);
        fn view(s: &State, _: &mut Cx<State>) -> El {
            let parent = el().key("parent").size(80.0, 80.0);
            let parent = if s.2 { parent } else {
                parent.on_press(|s: &mut State| s.0 += 1)
                    .on_hover(|s: &mut State| s.1 += 1)
            };
            parent.child(el().key("child").size(40.0, 40.0)
                .on_secondary_press(|_: &mut State, _, _| {}))
        }
        let mut rt = Runtime::new((0, 0, false), view, (100.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.pointer_move(10.0, 10.0);
        rt.pointer_down(10.0, 10.0);
        rt.pointer_up(10.0, 10.0);
        rt.frame(0.0);
        assert_eq!(rt.state, (1, 1, false));
        rt.state.2 = true;
        rt.invalidate();
        rt.frame(0.0);
        rt.focused = Some("previous-input".into());
        rt.pointer_move(10.0, 10.0);
        rt.pointer_down(10.0, 10.0);
        assert!(rt.hovered.is_none());
        assert!(matches!(rt.queue.last(), Some(Action::Blur)));
        assert!(rt.secondary_target(10.0, 10.0).is_some());
    }

    #[test]
    fn secondary_press_routes_to_ancestor_without_primary_and_cancels_outside() {
        type State = (u32, Vec<PointerPress>, bool);
        fn view(s: &State, _: &mut Cx<State>) -> El {
            col().child(if s.2 {
                el()
            } else {
                el().key("parent").size(80.0, 80.0)
                    .on_secondary_press(|s: &mut State, p, _| s.1.push(p))
                    .child(el().key("child").size(40.0, 40.0).on_press(|s: &mut State| s.0 += 1))
            })
        }
        let mut rt = Runtime::new((0, Vec::new(), false), view, (100.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.secondary_down(10.0, 10.0);
        rt.secondary_up(12.0, 12.0);
        rt.frame(0.0);
        assert_eq!(rt.state.0, 0);
        assert_eq!(rt.state.1, [PointerPress { x: 12.0, y: 12.0 }]);
        rt.secondary_down(10.0, 10.0);
        rt.secondary_up(90.0, 90.0);
        rt.frame(0.0);
        assert_eq!(rt.state.1.len(), 1);
        rt.secondary_down(10.0, 10.0);
        rt.state.2 = true;
        rt.invalidate();
        rt.frame(0.0);
        rt.secondary_up(12.0, 12.0);
        rt.frame(0.0);
        assert_eq!(rt.state.1.len(), 1);
    }

    #[test]
    fn timed_wakes_are_exact_coalesced_and_do_not_commit_or_animate() {
        use std::time::Duration;
        fn view(_: &Vec<f64>, cx: &mut Cx<Vec<f64>>) -> El {
            cx.time_scale(0.1);
            el()
        }
        fn poll(seen: &mut Vec<f64>, clock: f64, tx: &mut Tx) -> bool {
            seen.push(clock);
            if clock == 0.0 {
                tx.wake_after(Duration::from_millis(250));
                tx.wake_after(Duration::from_millis(250));
                tx.wake_after(Duration::from_millis(375));
            }
            false
        }
        let mut rt = Runtime::new(Vec::new(), view, (100.0, 100.0), Env::new(true, 1.0));
        rt.poll = Some(poll);
        assert!(!rt.frame(0.0));
        assert_eq!(rt.wake_deadlines, [0.25, 0.375]);
        assert!(!rt.frame(0.5));
        assert_eq!(rt.state, [0.0, 0.25, 0.375, 0.5]);
        assert_eq!(rt.commits, 1);
        assert_eq!(rt.wake_deadline(), None);
        assert!((rt.t - 0.05).abs() < 1e-8);
    }

    #[test]
    fn timed_wakes_from_handlers_survive_transaction_merging() {
        fn view(_: &(), _: &mut Cx<()>) -> El {
            el().key("button").size(100.0, 100.0).on_press_tx(|_: &mut (), tx| {
                tx.wake_after(std::time::Duration::from_millis(250));
            })
        }
        let mut rt = Runtime::new((), view, (100.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.pointer_down(10.0, 10.0);
        rt.pointer_up(10.0, 10.0);
        rt.frame(0.1);
        assert_eq!(rt.wake_deadline(), Some(0.35));
        rt.frame(0.25);
        assert_eq!(rt.wake_deadline(), None);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "duplicate sibling key \"card\"")]
    fn duplicate_sibling_keys_fail_before_layout() {
        let mut rt = Runtime::new((), duplicate_view, (100.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
    }

    #[test]
    fn duplicate_sibling_keys_tooling_reports_without_invalid_layout() {
        let mut rt = Runtime::new((), duplicate_view, (100.0, 100.0), Env::new(true, 1.0));
        rt.key_warnings = Some(Vec::new());
        rt.frame(0.0);
        assert_eq!(rt.key_warnings.as_ref().unwrap().len(), 1);
        assert_eq!(rt.nodes.len(), 1);
        assert_eq!(rt.nodes[0].local.w, 100.0);
    }

    #[test]
    fn side_panel_wraps_once_and_releases_height_after_stable_width() {
        use crate::el::{row, text};
        fn view(s: &(f32, bool), cx: &mut Cx<(f32, bool)>) -> El {
            cx.reduced_motion(s.1);
            row().key("window").w_full().h_full()
                .child(col().key("viewport").grow(1.0).shrink().h_full()
                    .scroll(ScrollSpec { stick_bottom: true }).clip_soft(18.0)
                    .child(col().key("stack").w_full().growth(GrowthSpec::default())
                        .child(col().key("message").w_full().child(text("Wrap once at the landing width. ".repeat(30)).wrap().w_full()))))
                .child(el().key("side-panel").w(s.0).h_full())
        }
        fn run(fps: usize, reduced: bool) -> Vec<f32> {
            let mut rt = Runtime::new((0.0, reduced), view, (480.0, 700.0), Env::new(true, 2.0));
            rt.frame(0.0);
            let id = rt.nodes.iter().find(|n| n.key.as_deref() == Some("message")).unwrap().id;
            let original = rt.grows[&id].h;
            let mut trace = std::env::var_os("RUNE_WIDTH_TRACE").map(|dir| {
                let dir = std::path::PathBuf::from(dir).join(format!("width-{fps}-{}", if reduced { "reduced" } else { "normal" }));
                std::fs::create_dir_all(&dir).unwrap();
                crate::tooling::Trace::open(&dir.join("trace.jsonl"), (480.0, 700.0), fps as f64, 2.0)
            });
            if let Some(trace) = &mut trace { trace.frame(&mut rt, 0.0, 0.0); }
            rt.state.0 = 160.0;
            rt.invalidate();
            rt.frame(0.0);
            let commits = rt.stats.commits;
            let measured = rt.nodes.iter().find(|n| n.id == id).unwrap().local.h;
            assert!(measured > original);
            if reduced { assert_eq!(rt.grows[&id].h, measured); }
            else { assert_eq!(rt.grows[&id].target, original); }
            let mut samples = vec![];
            for frame in 1..=fps * 2 {
                rt.frame(1.0 / fps as f64);
                assert_eq!(rt.stats.commits, commits, "presentation must not remeasure the wrapping");
                assert_eq!(rt.stats.shaped, 0);
                if frame as f64 / (fps as f64) < 0.12 && !reduced {
                    assert_eq!(rt.grows[&id].h, original);
                    assert!(rt.animating, "the hold must keep the shared clock awake");
                }
                if frame % (fps / 30) == 0 { samples.push(rt.grows[&id].h); }
                if let Some(trace) = &mut trace { trace.frame(&mut rt, frame as f64 * 1000.0 / fps as f64, 0.0); }
            }
            assert!((rt.grows[&id].h - measured).abs() < 0.05);
            // A second target during a hold restarts the stable-width interval.
            rt.state.0 = 80.0;
            rt.invalidate();
            rt.frame(0.0);
            for _ in 0..fps / 10 { rt.frame(1.0 / fps as f64); }
            rt.state.0 = 120.0;
            rt.invalidate();
            rt.frame(0.0);
            for _ in 0..fps / 10 { rt.frame(1.0 / fps as f64); }
            if !reduced {
                assert_eq!(rt.grows[&id].h, measured, "a changing width must restart the hold");
                assert!(!rt.stacks.values().next().unwrap().pending_heights.is_empty());
            }
            // Settle/resume bypasses even an active hold completely.
            rt.settle_all();
            rt.frame(0.0);
            assert!(rt.stacks.values().all(|s| s.pending_heights.is_empty()));
            assert!(!rt.animating);
            samples
        }
        for reduced in [false, true] {
            let a = run(30, reduced);
            let b = run(120, reduced);
            for (a, b) in a.into_iter().zip(b) { assert!((a - b).abs() < 0.05, "frame-rate dependent hold: {a} vs {b}"); }
        }
    }

    #[test]
    fn presses_land_inside_a_scrolled_view() {
        struct State { pressed: Vec<usize> }
        fn view(_: &State, _: &mut Cx<State>) -> El {
            let mut list = col().key("list").w_full();
            for i in 0..10 {
                list = list.child(el().key(format!("b{i}")).size(200.0, 40.0).on_press(move |s: &mut State| s.pressed.push(i)));
            }
            col().size(200.0, 200.0).child(col().key("scroll").size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).child(list))
        }
        let mut rt = Runtime::new(State { pressed: Vec::new() }, view, (200.0, 200.0), Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        rt.wheel(100.0, 100.0, -120.0);
        rt.frame(1.0 / 120.0);
        let (x, y) = rt.find("b5").expect("b5 is drawn");
        assert!(y < 200.0, "b5 scrolled into view at {y}");
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 120.0);
        assert_eq!(rt.state.pressed, [5]);
    }

    #[test]
    fn presses_on_a_window_drag_region_move_the_window_but_not_its_buttons() {
        struct State { pressed: u32 }
        fn view(_: &State, _: &mut Cx<State>) -> El {
            col().size(200.0, 200.0)
                .child(crate::el::row().key("titlebar").size(200.0, 40.0).window_drag()
                    .child(el().key("button").size(40.0, 40.0).on_press(|s: &mut State| s.pressed += 1)))
                .child(el().key("body").size(200.0, 40.0))
        }
        let mut rt = Runtime::new(State { pressed: 0 }, view, (200.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        for (x, y, drags) in [(100.0, 20.0, true), (20.0, 20.0, false), (100.0, 60.0, false)] {
            rt.pointer_move(x, y);
            rt.pointer_down(x, y);
            assert_eq!(rt.take_titlebar_action() == Some(TitlebarAction::Drag), drags, "at {x}, {y}");
            assert!(rt.take_titlebar_action().is_none(), "a drag request is taken once");
            rt.pointer_up(x, y);
        }
        rt.frame(1.0 / 120.0);
        assert_eq!(rt.state.pressed, 1, "the button inside the title bar still presses");
    }

    #[test]
    fn cursor_follows_the_hovered_target_and_its_ancestors() {
        use crate::el::Cursor;
        struct State { field: crate::edit::TextEdit }
        fn view(s: &State, _: &mut Cx<State>) -> El {
            col().size(200.0, 200.0)
                .child(el().key("button").size(200.0, 40.0).cursor(Cursor::Pointer).on_press(|_: &mut State| {}))
                .child(el().key("handle").size(200.0, 40.0).cursor(Cursor::Grab)
                    .child(el().key("inner").size(100.0, 40.0).on_press(|_: &mut State| {})))
                .child(crate::el::text_input(&s.field, crate::tok::TextStyle::new(14.0, 400.0)).key("field").size(200.0, 40.0)
                    .on_edit(|s: &mut State| &mut s.field))
                .child(el().key("plain").size(200.0, 40.0).on_press(|_: &mut State| {}))
        }
        let mut rt = Runtime::new(State { field: crate::edit::TextEdit::new("") }, view, (200.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        assert_eq!(rt.cursor(), Cursor::Default);
        for (y, cursor) in [(20.0, Cursor::Pointer), (60.0, Cursor::Grab), (100.0, Cursor::Text), (140.0, Cursor::Default)] {
            rt.pointer_move(50.0, y);
            assert_eq!(rt.cursor(), cursor, "at y {y}");
        }
        rt.pointer_move(50.0, 20.0);
        rt.pointer_down(50.0, 20.0);
        rt.pointer_move(50.0, 140.0);
        assert_eq!(rt.cursor(), Cursor::Pointer, "a held press keeps its cursor");
        rt.pointer_up(50.0, 140.0);
        assert_eq!(rt.cursor(), Cursor::Default);
        rt.pointer_leave();
        assert_eq!(rt.cursor(), Cursor::Default);
    }

    #[test]
    fn single_line_input_scrolls_to_the_caret_instead_of_wrapping() {
        fn view(s: &crate::TextEdit, _: &mut Cx<crate::TextEdit>) -> El {
            crate::text_input(s, crate::TextStyle::new(16.0, 400.0)).single_line()
                .key("search").size(100.0, 24.0).on_edit(|s: &mut crate::TextEdit| s)
        }
        let text = "a very long search query that keeps going past the edge";
        let mut edit = crate::TextEdit::new(text);
        edit.focused = true;
        let mut rt = Runtime::new(edit, view, (100.0, 24.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let n = rt.nodes.iter().find(|n| n.key.as_deref() == Some("search")).unwrap();
        let path = n.path.clone();
        assert_eq!(n.text_layout.as_ref().unwrap().lines().count(), 1);
        let scroll = rt.input_scroll[&path];
        assert!(scroll > 100.0, "the end of the text is in view: {scroll}");
        let caret_x = rt.ime_area.unwrap()[0];
        assert!((0.0..=100.0).contains(&caret_x), "caret {caret_x} stays inside the field");

        // A click lands on the scrolled text, not on the hidden start.
        rt.queue.push(Action::Focus(path.clone(), 1.0, 12.0));
        rt.frame(0.0);
        assert!(rt.state.focus > 10, "clicked index {}", rt.state.focus);

        rt.queue.push(Action::Blur);
        rt.frame(0.0);
        assert!(!rt.state.focused);
        assert!(!rt.input_scroll.contains_key(&path), "unfocused fields rest at their start");
    }

    #[test]
    fn secure_input_masks_layout_inspector_and_unicode_pointer_indices() {
        fn view(s: &crate::TextEdit, _: &mut Cx<crate::TextEdit>) -> El {
            crate::text_input(s, crate::TextStyle::new(16.0, 400.0)).secure()
                .key("secret").size(200.0, 40.0).on_edit(|s: &mut crate::TextEdit| s)
        }
        let mut edit = crate::TextEdit::new("hé🙂");
        edit.focused = true;
        let mut rt = Runtime::new(edit, view, (200.0, 40.0), Env::new(true, 1.0));
        rt.frame(0.0);
        assert!(rt.state.is_secure() && rt.has_secure_focus());
        assert!(rt.ime_area.is_none());
        assert!(rt.elements().iter().all(|e| e.text.is_none()));
        let n = rt.nodes.iter().find(|n| n.key.as_deref() == Some("secret")).unwrap();
        assert_eq!(n.text.as_ref().unwrap().content, "•••");
        let path = n.path.clone();
        let layout = rt.focused_layout().unwrap();
        let caret = Cursor::from_byte_index(&layout, 6, Affinity::Downstream).geometry(&layout, 1.0);
        rt.queue.push(Action::Focus(path, caret.x0 as f32 + 0.1, ((caret.y0 + caret.y1) * 0.5) as f32));
        rt.frame(0.0);
        assert_eq!(rt.state.focus, 3);
        rt.paste("界"); rt.frame(0.0);
        assert_eq!(rt.state.text, "hé界🙂");
        rt.key(KeyEvent { key: crate::Key::Command('a'), mods: Default::default() }); rt.frame(0.0);
        rt.key(KeyEvent { key: crate::Key::Command('c'), mods: Default::default() }); rt.frame(0.0);
        assert!(rt.copy_request.is_none());
        rt.ime(Ime::Commit("ignored".into())); rt.frame(0.0);
        assert_eq!(rt.state.text, "hé界🙂");
        assert!(rt.events.iter().all(|e| !e.contains("hé")));
    }

    #[test]
    fn clipboard_paste_routes_to_window_or_field_and_discards_late_reads() {
        #[derive(Default)]
        struct State { field: crate::TextEdit, pasted: String }
        fn view(s: &State, _: &mut Cx<State>) -> El {
            col().size(200.0, 100.0)
                .on_paste(|s: &mut State, text, tx| { s.pasted = text.to_owned(); tx.copy_secret(text, std::time::Duration::from_secs(30)); })
                .child(crate::text_input(&s.field, crate::TextStyle::new(16.0, 400.0)).key("field").size(200.0, 40.0)
                    .on_edit(|s: &mut State| &mut s.field))
        }
        let mut rt = Runtime::new(State::default(), view, (200.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
        assert!(rt.accepts_paste());
        rt.paste("window fixture"); rt.frame(0.0);
        assert_eq!(rt.state.pasted, "window fixture");
        assert!(rt.events.iter().any(|e| e == "paste window"));
        assert!(!format!("{:?}", rt.copy_request).contains("window fixture"));
        rt.update(|s, _| s.field.focused = true); rt.frame(0.0);
        rt.paste_result(None, zeroize::Zeroizing::new("late window reply".into())); rt.frame(0.0);
        assert!(rt.state.field.text.is_empty());
        assert_eq!(rt.state.pasted, "window fixture");
        let target = rt.paste_target();
        rt.paste_result(target, zeroize::Zeroizing::new("field fixture".into())); rt.frame(0.0);
        assert_eq!(rt.state.field.text, "field fixture");
        assert!(rt.events.iter().any(|e| e == "paste field"));
    }

    #[test]
    fn delayed_loops_sleep_on_the_shared_deadline_and_resume_after_native_idle() {
        fn view(s: &(f32, f32), cx: &mut Cx<(f32, f32)>) -> El {
            cx.time_scale(s.1);
            el().size(100.0, 100.0).opacity(s.0).child(el().size(20.0, 20.0)
                .looping(rune_motion::LoopSpec::opacity(1.0, 2.0, 0.0, 1.0)))
        }
        let make = |opacity, scale| Runtime::new((opacity, scale), view, (100.0, 100.0), Env::new(true, 1.0));
        let mut rt = make(1.0, 1.0);
        assert!(!rt.frame(0.0));
        assert_eq!(rt.wake_deadline(), Some(2.0));
        let commits = rt.stats.commits;
        assert!(!rt.frame(1.0));
        assert_eq!(rt.wake_deadline(), Some(2.0));
        assert!(rt.frame(1.0), "a zero-opacity first pose must still start the loop");
        assert_eq!(rt.stats.commits, commits);
        assert_eq!(rt.wake_deadline(), None);
        let mut rt = make(1.0, 1.0);
        rt.frame(0.0);
        rt.wall_start = Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
        assert!(!rt.frame(1.0 / 120.0));
        assert!((rt.wake_deadline().unwrap() - 2.0).abs() < 0.001, "an input during idle must not postpone the loop");
        rt.wall_start = Some(std::time::Instant::now() - std::time::Duration::from_secs(2));
        assert!(rt.frame(1.0 / 120.0));
        assert!((rt.t - 2.0).abs() < 0.001, "idle dt must not postpone the start again");
        let mut rt = make(1.0, 0.5);
        assert!(!rt.frame(0.0));
        assert_eq!(rt.wake_deadline(), Some(4.0));
        rt.set_live_visible(false);
        assert_eq!(rt.wake_deadline(), None);
        rt.frame(0.0);
        assert_eq!(rt.wake_deadline(), None);
        for (opacity, scale) in [(0.0, 1.0), (1.0, 0.0)] {
            let mut rt = make(opacity, scale);
            assert!(!rt.frame(0.0));
            assert_eq!(rt.wake_deadline(), None);
        }
    }

    #[test]
    fn cycles_share_deadlines_and_sleep_when_hidden_by_hover_groups() {
        fn view(hidden: &bool, _: &mut Cx<bool>) -> El {
            el().size(100.0, 100.0).hover_group().child(el().key("cycle").size(20.0, 20.0)
                .group_opacity(if *hidden { 0.0 } else { 1.0 }, 1.0)
                .cycle(crate::Cycle { period: 1.0, phase: 0.0,
                    stops: &[(0.0, 0.0), (0.5, 1.0), (1.0, 0.0)], still: 0.4, fps: 30.0 }))
        }
        let mut rt = Runtime::new(false, view, (100.0, 100.0), Env::new(true, 1.0));
        assert!(!rt.frame(0.0), "cycles must sleep rather than request continuous frames");
        assert!((rt.wake_deadline().unwrap() - 1.0 / 30.0).abs() < 1e-7);
        assert_eq!(rt.loops.len(), 1);
        let commits = rt.stats.commits;
        rt.frame(0.5);
        assert_eq!(rt.stats.commits, commits);
        assert!((rt.ambient.unwrap() - 1.0 / 30.0).abs() < 1e-7);
        rt.state = true; rt.invalidate(); rt.frame(0.0);
        assert_eq!(rt.wake_deadline(), None, "group-opacity zero must not wake");
        rt.state = false; rt.invalidate(); rt.frame(0.0);
        assert!(rt.wake_deadline().is_some());
        rt.set_live_visible(false); rt.frame(0.0);
        assert_eq!(rt.wake_deadline(), None);
        rt.set_live_visible(true); rt.frame(0.0);
        assert!(rt.wake_deadline().is_some());
        rt.reduced_motion = true; rt.frame(0.0);
        assert_eq!(rt.wake_deadline(), None);
    }

    #[test]
    fn loops_sleep_offscreen_hidden_reduced_and_after_exit() {
        fn view(s: &(f32, bool, bool), cx: &mut Cx<(f32, bool, bool)>) -> El {
            cx.reduced_motion(s.2);
            let mut root = col().key("root").size(100.0, 100.0).clip_soft(0.0);
            if s.1 {
                root = root.child(el().key("loop").absolute(0.0, s.0).size(20.0, 20.0).fill(Color::WHITE)
                    .looping(rune_motion::LoopSpec::opacity(1.0, 0.0, 0.4, 1.0)));
            }
            root
        }
        for (y, shown, reduced, active) in [(0.0, true, false, true), (200.0, true, false, false),
            (0.0, false, false, false), (0.0, true, true, false)] {
            let mut rt = Runtime::new((y, shown, reduced), view, (100.0, 100.0), Env::new(true, 2.0));
            assert_eq!(rt.frame(0.0), active);
            // Model the shell's scheduling gate: zero requested frames over ten seconds at rest.
            let mut requested = 0;
            for _ in 0..1200 { if rt.animating { requested += 1; rt.frame(1.0 / 120.0); } }
            assert_eq!(requested, if active { 1200 } else { 0 });
            if active {
                rt.state.1 = false; rt.invalidate(); rt.frame(0.0);
                assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("loop") && e.opacity > 0.0));
                for _ in 0..240 { rt.frame(1.0 / 120.0); }
                assert!(!rt.animating);
                assert!(!rt.elements().iter().any(|e| e.key.as_deref() == Some("loop")));
            }
        }
    }

    #[test]
    fn delayed_opacity_handoff_never_overprints_the_outgoing_label() {
        use rune_motion::AnimExt;
        fn view(s: &(bool, bool), cx: &mut Cx<(bool, bool)>) -> El {
            cx.reduced_motion(s.1);
            let fade = |show| rune_motion::Curve::ease_in_out(0.18).delay(if show { 0.18 } else { 0.0 });
            col().child(el().key("copy").size(28.0, 28.0).opacity(if s.0 { 0.0 } else { 1.0 }).opacity_motion(fade(!s.0)))
                .child(el().key("working").size(28.0, 28.0).opacity(if s.0 { 1.0 } else { 0.0 }).opacity_motion(fade(s.0)))
        }
        for reduced in [false, true] {
            let mut rt = Runtime::new((false, reduced), view, (100.0, 100.0), Env::new(true, 2.0));
            rt.frame(0.0);
            for busy in [true, false] {
                rt.state.0 = busy;
                rt.invalidate();
                for _ in 0..60 {
                    rt.frame(1.0 / 120.0);
                    let alpha = |key| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().opacity;
                    assert!(alpha("copy").min(alpha("working")) < 0.001);
                }
                assert!(!rt.animating);
            }
        }
    }

    #[test]
    fn measurement_cannot_change_presented_geometry_or_motion_policy() {
        fn view(changed: &bool, cx: &mut Cx<bool>) -> El {
            cx.reduced_motion(*changed);
            col().key("stack").w_full().growth(GrowthSpec::default())
                .child(el().key("row").w_full().h(if *changed { 160.0 } else { 40.0 }))
        }
        let mut rt = Runtime::new(false, view, (300.0, 300.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let id = rt.nodes.iter().find(|n| n.key.as_deref() == Some("row")).unwrap().id;
        let before = rt.scene.presented(id).unwrap().rect;
        rt.state = true;
        let measured = rt.measure(&Tx::default());
        assert_eq!(rt.scene.presented(id).unwrap().rect, before);
        assert_eq!(rt.scene.store().active_len(), 0);
        assert_eq!(rt.grows[&id].h, 40.0);
        assert!(!rt.reduced_motion && !rt.scene.store().reduced_motion());
        rt.apply(measured, Tx::default());
        assert!(rt.reduced_motion && rt.scene.store().reduced_motion());
        assert_eq!(rt.grows[&id].h, 160.0);
    }

    #[test]
    fn send_flight_follows_the_measured_bubble_while_earlier_rows_land() {
        struct State { draft: crate::edit::TextEdit, sent: bool, height: f32 }
        fn view(s: &State, _: &mut Cx<State>) -> El {
            let mut rows = col().w_full().growth(GrowthSpec::default())
                .child(el().key("earlier").w_full().h(s.height));
            if s.sent { rows = rows.child(col().key("bubble").w_full().py(8.0)
                .child(crate::el::text("héllo flight").key("destination").wrap())); }
            col().w_full().h_full()
                .child(col().key("scroll").w_full().grow(1.0).shrink().scroll(ScrollSpec { stick_bottom: true }).child(rows))
                .child(crate::el::text_input(&s.draft, crate::tok::TextStyle::new(14.0, 400.0)).key("source").w_full().h(60.0)
                    .on_edit(|s: &mut State| &mut s.draft)
                    .on_submit(|s: &mut State, tx: &mut Tx| {
                        s.sent = true;
                        s.draft.clear();
                        tx.fly_text("source", "destination");
                    }))
        }
        let mut draft = crate::edit::TextEdit::new("héllo flight");
        draft.focused = true;
        let mut rt = Runtime::new(State { draft, sent: false, height: 80.0 }, view, (400.0, 300.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.state.height = 400.0;
        rt.invalidate();
        rt.frame(1.0 / 120.0);
        rt.key(KeyEvent { key: crate::edit::Key::Enter, mods: Default::default() });
        rt.frame(1.0 / 120.0);
        assert_eq!(rt.flights.len(), 1);
        assert!(!rt.flights[0].glyphs.is_empty(), "the visible composer glyphs survive clearing the model");
        let mut previous_target = rt.flights[0].destination;
        let mut retargets = 0;
        for _ in 0..240 {
            rt.frame(1.0 / 120.0);
            if let Some(f) = rt.flights.first() {
                assert!(rt.instances.iter().any(|q| q.params[1] == KIND_GLYPH && q.uv[2] > 0.0 && q.fill[3] > 0.1
                    && q.rect[0].max(q.clip[0]) + 1.0 < (q.rect[0] + q.rect[2]).min(q.clip[2])
                    && q.rect[1].max(q.clip[1]) + 1.0 < (q.rect[1] + q.rect[3]).min(q.clip[3])),
                    "the source/destination handoff never loses every visible glyph");
                let target = rt.nodes.iter().position(|n| n.key.as_deref() == Some("destination")).unwrap();
                assert!((f.destination.y - rt.drawn[target][1]).abs() < 0.01, "destination is this frame's post-scroll geometry");
                if f.destination != previous_target { retargets += 1; }
                previous_target = f.destination;
            }
        }
        assert!(retargets > 3, "earlier row growth actually moved the destination");
        assert!(rt.flights.is_empty(), "flight lands and releases the captured glyphs");
        assert!(!rt.animating);
    }

    #[test]
    fn virtual_rows_match_full_layout_while_paging_and_keep_a_keyed_anchor() {
        struct State { virtualized: bool, keys: Vec<usize> }
        fn view(s: &State, _: &mut Cx<State>) -> El {
            let mut stack = col().key("rows").w_full().pt(8.0).pb(24.0).growth(GrowthSpec::default());
            if s.virtualized { stack = stack.virtualize(80.0); }
            for &i in &s.keys {
                stack = stack.child(col().key(format!("r{i}")).w_full().pb(9.0).child(
                    crate::el::text(format!("Row {i}: {}", "variable height héllo naïve ".repeat(i % 7 + 1))).wrap().w_full()
                ));
            }
            col().key("scroll").w_full().h_full().clip_fade(0.0, 8.0, 0.0, 8.0).scroll(ScrollSpec { stick_bottom: true }).child(stack)
        }
        let env = Env::new(true, 2.0);
        let mut full = Runtime::new(State { virtualized: false, keys: (0..600).collect() }, view, (240.0, 300.0), env);
        let mut window = Runtime::new(State { virtualized: true, keys: (0..600).collect() }, view, (240.0, 300.0), env);
        full.frame(0.0);
        window.frame(0.0);
        assert!(window.nodes.len() < 100, "offscreen rows aren't laid out or retained in the scene");
        assert!(window.text.shaped < full.text.shaped / 5, "offscreen text isn't shaped");
        let visible = |r: &Runtime<State>| r.elements().into_iter()
            .filter(|e| e.key.as_ref().is_some_and(|k| k.starts_with('r') && k != "rows") && e.visible[3] > 0.0)
            .map(|e| (e.key.unwrap(), e.rect)).collect::<Vec<_>>();
        for step in 0..135 {
            let a = visible(&full);
            let b = visible(&window);
            assert_eq!(a.len(), b.len(), "visible count at step {step}: {a:?} vs {b:?}");
            for ((ak, ar), (bk, br)) in a.iter().zip(&b) {
                assert_eq!(ak, bk, "visible identity at step {step}");
                assert!(ar.iter().zip(br).all(|(a, b)| (a - b).abs() < 0.05), "anchor at step {step}: {ak} {ar:?} vs {br:?}");
            }
            let dy = if step < 90 { 137.25 } else { -137.25 };
            full.wheel(120.0, 150.0, dy);
            window.wheel(120.0, 150.0, dy);
            full.frame(1.0 / 120.0);
            window.frame(1.0 / 120.0);
        }
        let before = visible(&window);
        window.state.keys.insert(0, 9000);
        window.invalidate();
        window.frame(1.0 / 120.0);
        assert_eq!(before, visible(&window), "prepending outside the window preserves its keyed anchor");
        assert!(window.nodes.len() < 150 && window.grows.len() < 70, "paging releases old row state");
        let anchor = visible(&window)[0].clone();
        window.set_viewport((320.0, 300.0), 2.0);
        window.frame(0.0);
        for _ in 0..60 {
            let after = visible(&window);
            let rect = after.iter().find(|(key, _)| key == &anchor.0).expect("resize retains the visible keyed anchor");
            assert!((rect.1[1] - anchor.1[1]).abs() < 0.05, "resize anchor moved: {:?} vs {:?}", anchor, rect);
            window.frame(1.0 / 120.0);
        }
        window.settle_all();
        window.frame(0.0);
        let last = visible(&window).pop().unwrap();
        assert_eq!(last.0, "r599");
        assert!((last.1[1] + last.1[3] - 276.0).abs() < 0.05, "return lands at the exact bottom");
        assert!(!window.animating, "moving: {:?}; active: {}", window.elements().into_iter().filter(|e| e.moving).map(|e| (e.path, e.rect, e.target, e.opacity)).collect::<Vec<_>>(), window.scene.store().active_len());
    }

    #[test]
    fn reduced_motion_rows_land_immediately_but_keep_the_presence_fade() {
        fn view(show: &bool, cx: &mut Cx<bool>) -> El {
            cx.reduced_motion(true);
            let mut stack = col().w_full().growth(GrowthSpec::default());
            if *show {
                stack = stack.child(el().key("row").size(200.0, 100.0));
            }
            stack
        }
        let mut rt = Runtime::new(false, view, (200.0, 300.0), Env::new(true, 2.0));
        assert!(!rt.frame(0.0));
        rt.state = true;
        rt.invalidate();
        rt.frame(1.0 / 120.0);
        let row = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("row")).unwrap();
        assert_eq!(row.rect[3], 100.0, "reduced motion removes growth travel");
        assert!(row.opacity > 0.0 && row.opacity < 1.0, "new rows still fade in");
        rt.frame(0.2);
        assert!(!rt.animating);
        assert_eq!(rt.elements().into_iter().find(|e| e.key.as_deref() == Some("row")).unwrap().opacity, 1.0);
        rt.state = false;
        rt.invalidate();
        rt.frame(1.0 / 120.0);
        let row = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("row")).unwrap();
        assert!(row.opacity > 0.0 && row.opacity < 1.0, "removed rows stay for their fade");
        for _ in 0..60 { rt.frame(1.0 / 120.0); }
        assert!(!rt.animating);
        assert!(rt.elements().iter().all(|e| e.key.as_deref() != Some("row")));
    }

    #[test]
    fn clipboard_requests_follow_handler_order_and_are_consumed_once() {
        use crate::edit::{Key, KeyEvent, Mods, TextEdit};
        fn view(edit: &TextEdit, _: &mut Cx<TextEdit>) -> El {
            col()
                .child(el().key("copy").size(40.0, 28.0).on_press_tx(|_: &mut TextEdit, tx| tx.copy("response")))
                .child(crate::el::text_input(edit, crate::tok::TextStyle::new(14.0, 400.0)).key("input").on_edit(|e: &mut TextEdit| e))
        }
        let mut edit = TextEdit::new("héllo 👋");
        edit.anchor = 0;
        let mut rt = Runtime::new(edit, view, (300.0, 200.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let button = rt.nodes.iter().find(|n| n.key.as_deref() == Some("copy")).unwrap().path.clone();
        rt.focused = Some(rt.nodes.iter().find(|n| n.key.as_deref() == Some("input")).unwrap().path.clone());
        let copy_key = KeyEvent { key: Key::Command('c'), mods: Mods { cmd: true, ..Mods::default() } };
        rt.queue.push(Action::Press(button.clone()));
        rt.queue.push(Action::Key(copy_key.clone()));
        rt.frame(1.0 / 120.0);
        assert_eq!(rt.copy_request.take().as_deref(), Some("héllo 👋"));
        rt.frame(1.0 / 120.0);
        assert!(rt.copy_request.is_none());
        rt.queue.push(Action::Key(copy_key));
        rt.queue.push(Action::Press(button));
        rt.frame(1.0 / 120.0);
        assert_eq!(rt.copy_request.take().as_deref(), Some("response"));
    }

    #[test]
    fn streaming_fill_and_width_rewrap_land_on_completion() {
        fn view(state: &(String, bool), _: &mut Cx<(String, bool)>) -> El {
            col().w_full().growth(GrowthSpec::default()).child(
                col().key("row").w_full().streaming(state.1).child(crate::el::text(state.0.clone()).wrap().w_full().reveal(0.18))
            )
        }
        let mut rt = Runtime::new(("one".into(), true), view, (180.0, 500.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.state.0 = "one two three four five six seven eight nine ten eleven".into();
        rt.invalidate();
        rt.frame(1.0 / 120.0);
        let row = rt.nodes.iter().find(|n| n.key.as_deref() == Some("row")).unwrap().id;
        let n = &rt.nodes[rt.node_index[&row]];
        let full = n.local.h;
        assert!(rt.grows[&row].target < full, "new line is held back while filling");
        for _ in 0..240 { rt.frame(1.0 / 120.0); }
        assert!(!rt.animating, "partial line at rest must stop ticking");
        let old_h = rt.grows[&row].h;
        rt.set_viewport((600.0, 500.0), 2.0);
        rt.state.0.push_str(" twelve");
        rt.invalidate();
        rt.frame(1.0 / 120.0);
        assert_eq!(rt.grows[&row].target, old_h, "rewrap waits for stable width");
        for _ in 0..15 { rt.frame(1.0 / 120.0); }
        assert!(rt.grows[&row].target < old_h, "width change may shrink a streaming row");
        assert!(rt.grows[&row].h > rt.grows[&row].target, "rewrapped height eases instead of snapping");
        rt.state.0 = "one two".into();
        rt.invalidate();
        rt.frame(0.0);
        assert!(rt.grows[&row].target < rt.grows[&row].h, "streaming may still shrink inside the 400 ms rewrap window");
        for _ in 0..120 { rt.frame(1.0 / 120.0); }
        let landed = rt.grows[&row].h;
        rt.state.0 = "o".into();
        rt.invalidate();
        rt.frame(0.0);
        assert_eq!(rt.grows[&row].target, landed, "streaming cannot shrink after the rewrap window");
        rt.state.1 = false;
        rt.invalidate();
        for _ in 0..240 { rt.frame(1.0 / 120.0); }
        assert!(!rt.animating);
        assert_eq!(rt.grows[&row].h, rt.nodes[rt.node_index[&row]].local.h);
    }

    #[test]
    fn retained_layout_matches_fresh_tree_across_edits() {
        fn view(seed: &usize, _: &mut Cx<usize>) -> El {
            let mut root = col().key("root").w_full();
            for k in 0..8 {
                let i = (k + seed) % 8;
                if (i + seed) % 5 == 0 { continue; }
                let body = "some words ".repeat((i + seed) % 10 + 1);
                let child = if (i + seed) % 7 == 0 { el().h(31.0) } else { crate::el::text(body).wrap() };
                root = root.child(child.key(format!("row-{i}")).w_full().px((seed % 3) as f32));
            }
            root
        }
        let env = Env::new(true, 2.0);
        let mut cached = Runtime::new(0usize, view, (320.0, 600.0), env);
        for seed in 0..40 {
            let size = (240.0 + (seed % 4) as f32 * 30.0, 600.0);
            let scale = if seed % 3 == 0 { 1.0 } else { 2.0 };
            cached.state = seed;
            cached.set_viewport(size, scale);
            cached.settle_all();
            cached.frame(0.0);
            let mut fresh = Runtime::new(seed, view, size, Env { scale, ..env });
            fresh.frame(0.0);
            let rects = |r: &Runtime<usize>| r.elements().into_iter().filter(|e| e.opacity > 0.0).map(|e| (e.path, e.target)).collect::<Vec<_>>();
            assert_eq!(rects(&cached), rects(&fresh), "edit {seed}");
        }
    }

    #[test]
    fn presence_exits_stay_drawn_and_revive_mid_flight() {
        fn view(show: &bool, _cx: &mut Cx<bool>) -> El {
            let mut root = col().key("root").size(200.0, 100.0);
            if *show {
                root = root.child(
                    el().key("badge")
                        .size(80.0, 30.0)
                        .fill(Color::WHITE)
                        .transition(Transition::blur_replace()),
                );
            }
            root
        }

        let mut rt = Runtime::new(true, view, (200.0, 100.0), Env::new(true, 2.0));
        assert!(!rt.frame(0.0), "first mount never animates");
        assert_eq!(rt.elements().len(), 2);

        // Remove the badge: it stays in the tree while its blur-replace exit runs.
        rt.state = false;
        rt.dirty = true;
        assert!(rt.frame(1.0 / 60.0));
        let badge = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("badge")).unwrap();
        assert!(badge.opacity > 0.0 && badge.opacity < 1.0);

        // Revive mid-exit: continues without a jump.
        let before = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("badge")).unwrap().opacity;
        rt.state = true;
        rt.dirty = true;
        rt.frame(1.0 / 60.0);
        let after = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("badge")).unwrap().opacity;
        assert!((after - before).abs() < 0.15, "{before} -> {after}");

        // Remove again and run until settled: unmounts only after opacity reaches 0.
        rt.state = false;
        rt.dirty = true;
        for _ in 0..120 {
            rt.frame(1.0 / 60.0);
        }
        assert!(!rt.animating);
        assert!(rt.elements().iter().all(|e| e.key.as_deref() != Some("badge")));
    }

    #[test]
    fn growth_rows_grow_and_sticky_scroll_lands_at_bottom() {
        use crate::el::{GrowthSpec, ScrollSpec};
        fn view(n: &usize, _cx: &mut Cx<usize>) -> El {
            let mut stack = col().key("stack").growth(GrowthSpec::default());
            for i in 0..*n {
                stack = stack.child(el().key(format!("r{i}")).h(100.0).w_full());
            }
            col().key("root").child(col().key("scroll").h(300.0).w_full().scroll(ScrollSpec { stick_bottom: true }).child(stack))
        }
        let mut rt = Runtime::new(10usize, view, (200.0, 300.0), Env::new(true, 2.0));
        rt.frame(0.0);
        // First frame: at the bottom, nothing animating (guarantee 8).
        assert!(!rt.animating);
        let last = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r9")).unwrap();
        assert!((last.rect[1] + last.rect[3] - 300.0).abs() < 0.05, "{:?}", last.rect);
        // A new row grows from 0 and the end stays pinned.
        rt.state = 11;
        rt.dirty = true;
        rt.frame(1.0 / 60.0);
        assert!(rt.animating);
        let r10 = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r10")).unwrap();
        assert!(r10.rect[3] < 100.0 && r10.rect[3] > 0.0);
        for _ in 0..120 {
            rt.frame(1.0 / 60.0);
        }
        assert!(!rt.animating);
        let r10 = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r10")).unwrap();
        assert!((r10.rect[1] + r10.rect[3] - 300.0).abs() < 0.05, "{:?}", r10.rect);
    }
}
