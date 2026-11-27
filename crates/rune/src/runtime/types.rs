//! Retained node, inherited paint state, scroll, growth stack and frame stats types.
use super::*;

/// A node's place under a growth stack's row. The row's height carries vertical motion there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum RowLayout {
    /// Not inside a growth row.
    Free,
    /// Inside one: layout snaps.
    Snaps,
    /// Inside one under `El::animate_layout`: x and width spring, y and height snap.
    Horizontal,
}

/// Rarely set per-node metadata, motion overrides, accessibility, live bindings and container
/// specs, kept behind `Cold<NodeCold>` so common nodes stay under four hundred bytes.
#[derive(Clone)]
pub(crate) struct NodeCold {
    pub trace_name: Option<&'static str>,
    pub trace_namespace: Option<(&'static str, &'static str)>,
    pub label: Option<std::borrow::Cow<'static, str>>,
    pub(super) live: Vec<crate::live::Bound>,
    /// The run a cell measures and draws from (see `El::advance_in`).
    pub(super) run_layout: Option<Rc<crate::text::RunLayout>>,
    /// Verified tabular-digit glyph substitution for a single-line `LiveText` node.
    pub(super) tabular_layout: Option<Rc<crate::text::TabularGlyphs>>,
    pub(super) live_text_seen: Option<u128>,
    pub(super) looping: Option<Box<rune_motion::LoopSpec>>,
    pub(super) loop_times: Option<u32>,
    pub(super) frame_motion: Option<Box<rune_motion::Anim>>,
    pub(super) opacity_motion: Option<Box<rune_motion::Anim>>,
    pub(super) lead: Option<(Spring, Spring)>,
    pub(super) transition: Option<Box<Transition>>,
    pub(super) transition_motion: Option<Box<(rune_motion::Anim, rune_motion::Anim)>>,
    pub(super) enter_in_instant: bool,
    /// `El::matched` id, with the element's own (not inherited) layout spring.
    pub(super) matched: Option<(Rc<str>, Option<Spring>)>,
    pub(super) matched_motion: Option<Box<rune_motion::Anim>>,
    pub(super) above: Option<crate::el::Anchor>,
    pub(super) above_boundary: Option<String>,
    pub(super) resolved_anchor: Option<(crate::Side, crate::Align)>,
    pub(crate) scroll: Option<ScrollSpec>,
    pub(super) scroll_axes: crate::el::ScrollAxes,
    pub(super) scroll_edges: crate::el::ScrollEdges,
    /// `El::wheel_scrolls`: the viewport a wheel over this node scrolls, in place of what it
    /// starts on.
    pub(super) wheel_scrolls: Option<Box<str>>,
    /// `El::sticky_top`, `El::sticky_bottom` and `El::stuck_shadow`.
    pub(super) sticky: Option<Box<crate::el::Sticky>>,
    pub(super) growth: Option<Box<GrowthSpec>>,
    pub(super) glide: Option<crate::growth::FoldGlide>,
    pub(crate) a11y: crate::el::A11y,
    /// The image ratio this picture's layout took, when it sizes one side from its pixels.
    pub(super) pixel_aspect: Option<f32>,
    /// `El::aspect_fit`: the ratio and the element's own style, which the fitted size replaces.
    pub(super) aspect_fit: Option<Box<(f32, CompactStyle)>>,
    pub(super) material: Option<(crate::SystemMaterial, crate::MaterialParams)>,
    /// `El::help`.
    pub(crate) help: Option<String>,
    /// `El::help_when_truncated`.
    pub(crate) help_cut: bool,
    /// `El::help_side`.
    pub(super) help_side: Option<crate::Side>,
    /// `El::help_open`.
    pub(super) help_open: bool,
    /// `El::data`.
    pub(super) data: Vec<crate::el::data::ElData>,
    #[cfg(debug_assertions)]
    pub(super) scroll_source: Option<&'static std::panic::Location<'static>>,
}

impl crate::cold::Empty for NodeCold {
    const EMPTY: &'static Self = &NodeCold {
        trace_name: None,
        trace_namespace: None,
        label: None,
        live: Vec::new(),
        run_layout: None,
        tabular_layout: None,
        live_text_seen: None,
        looping: None,
        loop_times: None,
        frame_motion: None,
        opacity_motion: None,
        lead: None,
        transition: None,
        transition_motion: None,
        enter_in_instant: false,
        matched: None,
        matched_motion: None,
        above: None, above_boundary: None, resolved_anchor: None,
        scroll: None,
        scroll_axes: crate::el::ScrollAxes::Y,
        scroll_edges: crate::el::ScrollEdges { fade: 0.0, fade_style: crate::el::FadeStyle::SMOOTH, margin: 0.0, anchor: false, no_anchor: false },
        wheel_scrolls: None,
        sticky: None,
        growth: None,
        glide: None,
        a11y: crate::el::A11y::EMPTY,
        pixel_aspect: None,
        aspect_fit: None,
        material: None,
        help: None,
        help_cut: false,
        help_side: None,
        help_open: false,
        data: Vec::new(),
        #[cfg(debug_assertions)]
        scroll_source: None,
    };
}

#[derive(Clone)]
pub(crate) struct Node {
    pub id: NodeId,
    pub path: String,
    pub key: Option<String>,
    pub role: &'static str,
    pub parent: Option<usize>,
    pub(super) taffy: SlotId,
    pub text: Option<crate::cold::SharedBox<TextSpec>>,
    pub text_layout: Option<crate::text::TextLayout>,
    /// Where `text_layout` cut the text for `El::truncate` and its kin.
    pub(crate) elided: Option<Elision>,
    pub(crate) input: Option<Box<InputSpec>>,
    pub(crate) focus: crate::cold::Cold<crate::el::FocusSpec>,
    pub(crate) handlers: crate::cold::Cold<crate::el::Handlers>,
    pub(super) cold: crate::cold::Cold<NodeCold>,
    pub vis: Visual,
    pub(crate) vis_flags: crate::el::VisualFlags,
    /// The hover curves of the nearest `El::hover_motion_within` at or above it (or above its
    /// anchor). Kept here, not in `vis`, so inheriting them doesn't copy every node's visuals.
    pub(super) hover_within: Option<Rc<(Anim, Anim)>>,
    /// Layout target in root space: x0, y0, x1, y1 in points.
    pub target: [f32; 4],
    /// Layout target in parent space.
    pub local: MRect,
    /// Padding plus border (left, top, right, bottom): text draws inside it.
    pub(super) inset: [f32; 4],
    pub(super) motion: Spring,
    /// `El::scale`: a factor on the element's own scale, springing on `P_SCALE`.
    pub(super) static_scale: f32,
    /// `El::time_scale`: this element's own factor; the scene multiplies ancestors' in.
    pub(super) time_scale: f32,
    /// Where it sits relative to a growth row, which carries its contents' motion.
    pub(super) in_row: RowLayout,
    pub(super) width_leads: bool,
    /// Inside an `El::instant_changes` element, or one itself.
    pub(super) instant_changes: bool,
    /// `El::animated_changes` set directly on this element: stops an ancestor's `instant_changes`.
    pub(super) animated_changes: bool,
    pub(super) streaming: bool,
    pub(super) whole: bool,
    /// Inside an `El::inert` subtree (this element or an ancestor).
    pub(crate) inert: bool,
    /// Inside an `El::selection_enabled(false)` subtree no `true` reopens: its texts don't select.
    pub(crate) selection_off: bool,
    /// The direction it lays out in (`El::dir`, its own or inherited): right to left, its
    /// children's x mirror inside it and its inset's sides swap.
    pub(crate) flow: crate::el::Flow,
    /// Whether an explicit direction governs this subtree's paragraphs.
    pub(crate) directed: bool,
    /// Where it sits in its parent (`El::unmirrored`).
    pub(crate) pin: crate::el::Pin,
    /// `P_SCALE` has a track: paint reads the sprung factor.
    pub(super) scale_track: bool,
    /// `P_GROUP_REST` fades a dropped `El::group_opacity` to full: paint reads it until it lands.
    pub(super) group_fade: bool,
    pub(super) virtual_mount: bool,
    /// Built from `El::virtualize` or `El::virtual_rows`.
    pub(super) virtual_list: bool,
    /// Built from `El::virtual_items`.
    pub(super) virtual_items: bool,
    /// `El::selection_group`.
    pub(super) selection_group: bool,
    pub(super) window_drag: crate::el::WindowDrag,
    pub(super) ime_cursor: bool,
    /// `El::ime_option_as_alt` / `El::ime_option_as_alt_mode`.
    pub(super) ime_option_as_alt: crate::OptionAsAlt,
    /// Participates in its parent's normal flow (`position != Absolute && display != None`).
    pub(super) in_flow: bool,
    /// Exiting in-flow child whose main-axis slot is collapsing analytically (`hold_layout`).
    pub(super) collapsing: bool,
    /// Placeholder left in `self.nodes` after `try_reuse_memo` moves a node into the new pass.
    pub(super) moved_stub: bool,
    /// A new one each time it's built from its element; a node a region keeps keeps it
    /// (`paint_keep`).
    pub(super) epoch: u32,
}

impl std::ops::Deref for Node {
    type Target = NodeCold;
    #[inline(always)]
    fn deref(&self) -> &NodeCold {
        &self.cold
    }
}

impl Node {
    #[inline]
    pub(super) fn cold_mut(&mut self) -> &mut NodeCold {
        &mut self.cold
    }

    #[inline]
    pub(super) fn clear_run_layout(&mut self) {
        if self.cold.get().is_some_and(|c| c.run_layout.is_some()) {
            self.cold_mut().run_layout = None;
        }
    }

    #[inline]
    pub(super) fn clear_tabular_layout(&mut self) {
        if self.cold.get().is_some_and(|c| c.tabular_layout.is_some()) {
            self.cold_mut().tabular_layout = None;
        }
    }

    /// Zero-allocation placeholder left in `self.nodes` when a committing pass moves a
    /// `same_path` memo-reused node into the new `nodes` slice. Retains the last commit's
    /// identity, parent index, geometry and track flags that `apply` and retry restoration read.
    pub(super) fn moved_stub(old: &Self) -> Self {
        Node {
            id: old.id,
            path: String::new(),
            key: None,
            role: "group",
            parent: old.parent,
            taffy: old.taffy,
            text: None,
            text_layout: None,
            elided: None,
            input: None,
            focus: Default::default(),
            handlers: Default::default(),
            cold: Default::default(),
            vis: Visual::default(),
            vis_flags: crate::el::VisualFlags::default(),
            hover_within: None,
            target: old.target,
            local: old.local,
            inset: old.inset,
            motion: old.motion,
            static_scale: 1.0,
            time_scale: 1.0,
            in_row: RowLayout::Free,
            width_leads: false,
            instant_changes: old.instant_changes,
            animated_changes: old.animated_changes,
            streaming: false,
            whole: false,
            inert: false,
            selection_off: old.selection_off,
            flow: old.flow,
            directed: old.directed,
            pin: old.pin,
            scale_track: old.scale_track,
            group_fade: old.group_fade,
            virtual_mount: false,
            virtual_list: false,
            virtual_items: false,
            selection_group: false,
            window_drag: crate::el::WindowDrag::None,
            ime_cursor: false,
            ime_option_as_alt: crate::OptionAsAlt::None,
            in_flow: old.in_flow,
            collapsing: false,
            moved_stub: true,
            epoch: 0,
        }
    }

    /// Its hover track's curves: its own `El::hover_motion`, else the inherited
    /// `El::hover_motion_within` pair.
    #[inline]
    pub(super) fn hover_motion(&self) -> Option<(Anim, Anim)> {
        self.vis.hover_motion.as_deref().or(self.hover_within.as_deref()).copied()
    }

    #[inline]
    pub(crate) fn is_moved_stub(&self) -> bool {
        self.moved_stub
    }

    /// Whether its text is a secret: marked secure, redacted, or a secure field.
    pub(crate) fn is_secure(&self) -> bool {
        self.cold.get().is_some_and(|c| c.a11y.secure)
            || self.text.as_ref().is_some_and(|t| t.redacted)
            || self.input.as_ref().is_some_and(|i| i.edit.is_secure())
    }
    /// Whether a press, hover or drag can go to this node itself.
    pub(super) fn has_primary_handler(&self) -> bool {
        self.handlers.get().is_some_and(crate::el::Handlers::has_primary)
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
    pub const TEXT_FILL: u16 = 1024;
    pub const ICON_ANIM: u16 = 2048;
    pub const NAMES: [&'static str; 12] =
        ["tracks", "growth", "scroll", "reveal", "loop", "scale-settle", "video", "text-flight", "live", "virtual-fill", "text-fill", "icon-anim"];

    pub fn names(bits: u16) -> Vec<&'static str> {
        Self::NAMES.iter().enumerate().filter(|(i, _)| bits & (1 << i) != 0).map(|(_, n)| *n).collect()
    }
}

/// Where a glide goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum GlideTo {
    /// The content's bottom, as it moves (`Tx::scroll_to_bottom`).
    Bottom,
    /// An offset an app or a reveal asked for (`Tx::scroll_to`), while the content reaches it.
    Offset(f64),
    /// The end a growth stack shrinks to under a reader who scrolled up (`ease_to_shrunk_end`),
    /// retargeted as its rows move.
    ShrunkEnd(f64),
}

impl GlideTo {
    /// The offset it heads for, `None` for the bottom.
    pub(super) fn offset(self) -> Option<f64> {
        match self {
            GlideTo::Bottom => None,
            GlideTo::Offset(to) | GlideTo::ShrunkEnd(to) => Some(to),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ScrollState {
    pub(crate) offset: f64,
    /// What moves it, with the hand's latches for this frame (`runtime/scroll/mode.rs`).
    pub(super) mode: ScrollMode,
    pub(crate) max: f64,
    /// At-bottom state last sent to `El::on_at_bottom`.
    pub(super) at_bottom: Option<bool>,
    /// State last sent to `El::on_far_from_bottom`.
    pub(super) far_from_bottom: Option<bool>,
    /// Whether the last frame drew the content's bottom on the viewport's (see `stuck_offset`).
    pub(super) held: bool,
    /// The horizontal offset the last frame drew the content at, in points.
    pub(crate) drawn_x: f32,
    /// Where it drew the vertical offset, and the pinned end's hold (`scroll_drawn`).
    pub(crate) drawn: PixelHold,
    /// Room freed under a pinned growth stack, held under its last row (see `step_slack`).
    pub(super) slack: scroll_bottom::Slack,
    /// The horizontal axis of `El::scroll_x` and `El::scroll_xy`; all zero on a vertical one.
    pub(crate) x: AxisScroll,
    /// What `El::on_scroll_offset` has been told, and which handler heard it
    /// (`runtime/scroll/watch.rs`).
    pub(super) watch: super::scroll_watch::Watch,
    /// The spring `Tx::scroll_to_with` gave the running glide, on both axes; `GLIDE` when none.
    pub(super) spring: Option<rune_motion::Spring>,
    /// An app's instant scroll (`Tx::scroll_to_instant` and the like) landed less than
    /// `scroll_nudge::NUDGE_HOLD` ago, so content moved under a still pointer and hover waits
    /// for it to pass while the app steps its own glide frame by frame
    /// (`runtime/input/hover.rs`).
    pub(super) nudge: scroll_nudge::Nudge,
}

/// What moves a viewport's horizontal offset, one motion at a time.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum XMotion {
    /// Where a hand, an app or a motion left it.
    #[default]
    Rest,
    /// Coasting from a fling or a touch at `v` points a second, decaying.
    Momentum(f32),
    /// Gliding on a spring (`ScrollState::glide_spring`) at `v` points a second to `aim`
    /// (`Tx::scroll_to`).
    Glide { v: f32, aim: f64 },
    /// Easing to where a notched mouse wheel sent it (`runtime/scroll/wheel.rs`).
    Wheel(scroll_wheel::WheelCurve),
}

/// A scroll viewport's horizontal offset, motion and limit (points).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct AxisScroll {
    pub(crate) offset: f64,
    pub(super) motion: XMotion,
    pub(crate) max: f64,
    /// The viewport's flow (`El::dir`): right to left, its content starts at its right edge and
    /// `offset` counts how far it moved right, uncovering what is to the left.
    pub(super) flow: crate::el::Flow,
}

impl AxisScroll {
    /// How far left the content draws, in points: `offset`, or less it in a right-to-left
    /// viewport.
    pub(super) fn shift(&self) -> f64 {
        match self.flow { crate::el::Flow::Ltr => self.offset, crate::el::Flow::Rtl => -self.offset }
    }

    /// `delta` points of physical movement (positive moving content right) as a change of
    /// `offset`.
    pub(crate) fn toward(&self, delta: f64) -> f64 {
        match self.flow { crate::el::Flow::Ltr => -delta, crate::el::Flow::Rtl => delta }
    }

    /// Its momentum (points a second), 0 unless coasting.
    pub(super) fn coast(&self) -> f32 {
        match self.motion { XMotion::Momentum(v) => v, _ => 0.0 }
    }

    /// A glide or a wheel's ease moves it on its own.
    pub(super) fn carried(&self) -> bool {
        matches!(self.motion, XMotion::Glide { .. } | XMotion::Wheel(_))
    }

    /// Coasting at `v` from here; at 0 a coast stops, and a glide or a wheel's ease runs on.
    pub(super) fn set_coast(&mut self, v: f32) {
        if v != 0.0 { self.motion = XMotion::Momentum(v); } else if matches!(self.motion, XMotion::Momentum(_)) { self.motion = XMotion::Rest; }
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct Stack {
    /// Row ids in presented order, including rows still closing after removal.
    pub(super) order: Vec<NodeId>,
    /// Removed rows: when their space starts closing.
    pub(super) closing: HashMap<NodeId, f64>,
    /// Last known target top (container-relative) per row, for rows that left.
    pub(super) tops: HashMap<NodeId, f32>,
    pub(super) pad_top: f32,
    pub(super) pad_bottom: f32,
    pub(super) gap: f32,
    pub(super) width: f32,
    /// A width change the rows still wait out (heights it defers go in `pending_heights`).
    pub(super) width_hold: WidthHold,
    pub(super) pending_heights: HashMap<NodeId, f32>,
    /// The window size at the last commit.
    pub(super) window: (f32, f32),
    /// The list viewport in the last layout, apart from the window (e.g. above a dock).
    pub(super) viewport_height: f32,
    pub(super) handoff: row_reorder::handoff::Handoff,
    /// How far its rows grew (positive steps only) this frame.
    pub(super) grown: f32,
    /// Whether any row in the stack still has height left to grow.
    pub(crate) growing: bool,
    /// Whether any row in the stack is still animating.
    pub(crate) moving: bool,
    pub(super) reorder: row_reorder::Reorder,
    pub(super) seams: Vec<row_reorder::joined::Seam>,
    pub(super) slides: content_slide::ContentSlides,
    /// Fold rows opening or shutting together as one block.
    pub(super) runs: Vec<growth_runs::GrowthRun>,
}

/// A growth stack whose width changed: its rows measure at the new width at once, but their
/// new heights wait until the width is stable, and a streaming row shrinks only a while after.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum WidthHold {
    #[default]
    Off,
    /// Heights wait until `until`; a streaming row may shrink from `shrink_from`.
    Rewrap { until: f64, shrink_from: f64 },
}

impl WidthHold {
    /// Seconds new heights wait after a width change.
    const HOLD: f64 = 0.12;
    /// Seconds a streaming row keeps its height after a width change before it may shrink.
    const SHRINK: f64 = 0.4;

    /// The width changed at `now`.
    pub(super) fn rewrap(now: f64) -> WidthHold {
        WidthHold::Rewrap { until: now + Self::HOLD, shrink_from: now + Self::SHRINK }
    }

    /// Whether a height set at `at` waits for the width to settle.
    pub(super) fn defers(self, at: f64) -> bool {
        matches!(self, WidthHold::Rewrap { until, .. } if at < until)
    }

    /// Whether a streaming row may shrink at `now`.
    pub(super) fn may_shrink(self, now: f64) -> bool {
        !matches!(self, WidthHold::Rewrap { shrink_from, .. } if now < shrink_from)
    }
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
    /// View commits run since the runtime started. In debug builds, one self-check commit runs a
    /// quarter second after the last commit on the next event whose handler called
    /// [`Tx::live_only`](crate::Tx::live_only) unless
    /// [`Runtime::check_live_only`](crate::Runtime::check_live_only)`(false)` was called (release
    /// builds never check), so a test counting commits turns the check off.
    pub commits: u64,
    pub layout_nodes: usize,
    pub glyphs: usize,
    pub shaped: u32,
}

/// Retained node tree, id/key lookup tables and per-commit node capability flags.
/// Ordered hot-first under `#[repr(C)]` so `nodes`, per-commit capability flags and `node_index`
/// sit at offset 0 (landing inside `RuntimeCore`'s 128-byte `disp8` window).
#[repr(C)]
pub(crate) struct TreeState {
    pub(crate) nodes: Vec<Node>,
    /// Some element is `El::sticky_top` or `El::sticky_bottom` (see `runtime/scroll/sticky.rs`).
    pub(in crate::runtime) has_sticky: bool,
    pub(in crate::runtime) has_dissolve: bool,
    /// Some element is `El::raise`d: paint order isn't tree order.
    pub(in crate::runtime) has_raise: bool,
    /// Some element sets `El::paint_order`: its siblings paint out of tree order.
    pub(in crate::runtime) has_paint_order: bool,
    /// Some element is `El::anchored` / `El::above`.
    pub(in crate::runtime) has_above: bool,
    pub(in crate::runtime) has_cache: bool,
    /// Some picture sizes a side from its image's ratio (`pixel_aspect`).
    pub(in crate::runtime) has_pixel_aspect: bool,
    pub(in crate::runtime) has_drawn_layout: bool,
    pub(in crate::runtime) has_mount: bool,
    pub(in crate::runtime) has_key_up: bool,
    pub(in crate::runtime) has_any_press: bool,
    pub(in crate::runtime) has_link_hover: bool,
    pub(in crate::runtime) has_pointer_lock: bool,
    pub(in crate::runtime) node_index: NodeIndex,
    /// The node ids the last build indexed its scratch by.
    pub(in crate::runtime) built_ids: Vec<NodeId>,
    /// The last commit's node buffer, emptied, for the next flatten to fill: an app of a few
    /// thousand elements otherwise maps and faults in megabytes every commit.
    pub(in crate::runtime) spare_nodes: Vec<Node>,
    pub(in crate::runtime) next_id: u64,
    /// The node index the app's view returned when the playground is attached.
    app_root: usize,
    pub(in crate::runtime) path_ids: HashMap<String, NodeId>,
    /// Pictures whose image changed shape since the last commit, which lays them out at once.
    pub(in crate::runtime) aspect_snap: Vec<NodeId>,
    /// `inh_scratch` and `row_dy` index `nodes`: no commit since the last build.
    pub(in crate::runtime) built: bool,
    /// Whether this runtime has an attached playground (`hooks.playground.is_some()`).
    pub(crate) has_playground: bool,
    pub(in crate::runtime) exit_mask: exit_mask::Mask,
    pub(in crate::runtime) key_index: key_index::KeyIndex,
    /// Packed explicit piecewise-linear scroll-fade stops (`ScrollFade::stops`) for `Globals::flags[2..4]`.
    pub(in crate::runtime) fade_stops: [f32; 2],
}

impl TreeState {
    // Single-caller constructor for `RuntimeCore::new`; inlined so it stays in its caller's
    // codegen unit instead of shifting partitioning and dropping inlining in the paint walk.
    #[inline]
    pub(in crate::runtime) fn new() -> Self {
        Self {
            nodes: Vec::new(),
            has_sticky: false,
            has_dissolve: false,
            has_raise: false,
            has_paint_order: false,
            has_above: false,
            has_cache: false,
            has_pixel_aspect: false,
            has_drawn_layout: false,
            has_mount: false,
            has_key_up: false,
            has_any_press: false,
            has_link_hover: false,
            has_pointer_lock: false,
            built: false,
            has_playground: false,
            node_index: NodeIndex::default(),
            path_ids: HashMap::default(),
            spare_nodes: Vec::new(),
            built_ids: Vec::new(),
            aspect_snap: Vec::new(),
            next_id: 1,
            app_root: 0,
            exit_mask: Default::default(),
            key_index: Default::default(),
            fade_stops: [0.0; 2],
        }
    }

    /// Whether any node in the tree is `El::sticky_top` or `El::sticky_bottom`.
    #[inline]
    pub(crate) fn has_sticky(&self) -> bool {
        self.has_sticky
    }

    #[inline]
    pub(super) fn has_drawn_layout(&self) -> bool {
        self.has_drawn_layout
    }

    #[inline]
    pub(super) fn is_built(&self) -> bool {
        self.built
    }

    /// Marks this tree as hosting a live playground so [`Self::app_root`] tracks the story root.
    #[inline]
    pub(super) fn attach_playground(&mut self) {
        self.has_playground = true;
    }

    /// The node the app's view returned: the first, or with the playground attached, the one
    /// above its bar or on its stage. Root handlers (window focus, paste, file drops) are there.
    #[inline]
    pub(crate) fn app_root(&self) -> usize {
        if self.has_playground { self.app_root } else { self.view_root() }
    }

    /// The node the view returned: the first, or the first inside the frame that holds the
    /// Windows caption buttons over it (`titlebar::frame`).
    #[inline]
    fn view_root(&self) -> usize {
        usize::from(self.nodes.first().is_some_and(|n| n.key.as_deref() == Some(crate::titlebar::FRAME_KEY)))
    }

    pub(super) fn find_app_root(&mut self) {
        let view_root = self.view_root();
        if !self.has_playground { self.app_root = view_root; return; }
        // A scrolling stage holds the root in a margin.
        let margin = |k: Option<&str>| k == Some("playground-margin");
        let wraps = |k: &str| k == "playground-closed" || margin(Some(k)) || crate::playground::is_stage(k);
        let nodes = &self.nodes;
        self.app_root = nodes.iter().position(|n| !margin(n.key.as_deref()) && n.parent.is_some_and(|p| nodes[p].key.as_deref().is_some_and(wraps))).unwrap_or(view_root);
    }

    /// The kept key index, when `nodes` are the current nodes as of `commit`.
    #[inline]
    pub(super) fn key_index_for(&self, nodes: &[Node], commit: u64) -> Option<&key_index::KeyIndex> {
        self.key_index.get(nodes, commit)
    }

    /// Prepares the key index for `self.nodes` as of `commit` when `lookups` warrants it.
    #[inline]
    pub(super) fn prepare_key_index(&mut self, commit: u64, lookups: usize) {
        self.key_index.prepare(&self.nodes, commit, lookups);
    }

    /// `self.nodes` changed without a commit: kept per-node answers no longer hold.
    #[inline]
    pub(super) fn nodes_replaced(&mut self) {
        self.key_index.forget();
        self.exit_mask.forget();
    }

    #[inline]
    pub(super) fn index_of_id(&self, id: NodeId) -> Option<usize> {
        self.node_index.get(&id).copied()
    }

    #[inline]
    pub(super) fn node_by_id(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(*self.node_index.get(&id)?)
    }

    #[inline]
    pub(super) fn id_of_path(&self, path: &str) -> Option<NodeId> {
        self.path_ids.get(path).copied()
    }

    /// Current node index for `path`, verified against `self.nodes` so stale ids return `None`.
    #[inline]
    pub(super) fn index_of_path(&self, path: &str) -> Option<usize> {
        self.path_ids.get(path).and_then(|id| self.node_index.get(id)).copied().filter(|&i| self.nodes.get(i).is_some_and(|n| n.path == path))
    }

    /// Whether node `i` is `ancestor` or a descendant of `ancestor`.
    #[inline]
    pub(super) fn is_descendant(&self, i: usize, ancestor: usize) -> bool {
        std::iter::successors(Some(i), |&p| self.nodes[p].parent).any(|p| p == ancestor)
    }

    /// Whether node `i` or an ancestor has a key `is` takes.
    #[inline]
    pub(super) fn under(&self, i: usize, is: impl Fn(&str) -> bool) -> bool {
        std::iter::successors(Some(i), |&j| self.nodes[j].parent).any(|j| self.nodes[j].key.as_deref().is_some_and(&is))
    }

    /// The path of the element keyed `key`.
    #[inline]
    pub(super) fn key_path(&self, key: &str) -> Option<String> {
        self.nodes.iter().find(|n| n.key.as_deref() == Some(key)).map(|n| n.path.clone())
    }

    #[inline]
    pub(super) fn selects_index(&self, selector: &str) -> Option<usize> {
        self.nodes.iter().position(|n| crate::flight::selects(selector, &n.path, n.key.as_deref()))
    }

    /// Index of the last node inside `root`'s contiguous pre-order subtree (`root` if leaf).
    #[inline]
    pub(super) fn subtree_end(&self, root: usize) -> usize {
        (root + 1..self.nodes.len()).take_while(|&j| self.is_descendant(j, root)).last().unwrap_or(root)
    }

    /// Direct child of `parent` before `end` whose path equals `path`.
    #[inline]
    pub(super) fn direct_child_by_path(&self, parent: usize, end: usize, path: &str) -> Option<usize> {
        (parent + 1..end).find(|&j| self.nodes[j].parent == Some(parent) && self.nodes[j].path == path)
    }
}

#[cfg(test)]
#[path = "width_hold_tests.rs"]
mod width_hold_tests;
