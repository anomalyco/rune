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
    pub label: Option<String>,
    pub(super) live: Vec<crate::live::Bound>,
    /// The run a cell measures and draws from (see `El::advance_in`).
    pub(super) run_layout: Option<Rc<crate::text::RunLayout>>,
    pub(super) live_text_seen: Option<u128>,
    pub(super) looping: Option<Box<rune_motion::LoopSpec>>,
    pub(super) opacity_motion: Option<Box<rune_motion::Anim>>,
    pub(super) lead: Option<(Spring, Spring)>,
    pub(super) transition: Option<Box<Transition>>,
    /// `El::matched` id, with the element's own (not inherited) layout spring.
    pub(super) matched: Option<(Rc<str>, Option<Spring>)>,
    pub(super) above: Option<crate::el::Anchor>,
    pub(super) scroll: Option<ScrollSpec>,
    pub(super) scroll_axes: crate::el::ScrollAxes,
    pub(super) scroll_edges: crate::el::ScrollEdges,
    /// `El::sticky_top`, `El::sticky_bottom` and `El::stuck_shadow`.
    pub(super) sticky: Option<Box<crate::el::Sticky>>,
    pub(super) growth: Option<Box<GrowthSpec>>,
    pub(super) glide: Option<rune_motion::Spring>,
    pub(super) a11y: crate::el::A11y,
    /// The image ratio this picture's layout took, when it sizes one side from its pixels.
    pub(super) pixel_aspect: Option<f32>,
    /// `El::aspect_fit`: the ratio and the element's own style, which the fitted size replaces.
    pub(super) aspect_fit: Option<Box<(f32, Style)>>,
    pub(super) material: Option<crate::SystemMaterial>,
    /// `El::help`.
    pub(super) help: Option<String>,
    /// `El::help_when_truncated`.
    pub(super) help_cut: bool,
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
        live_text_seen: None,
        looping: None,
        opacity_motion: None,
        lead: None,
        transition: None,
        matched: None,
        above: None,
        scroll: None,
        scroll_axes: crate::el::ScrollAxes::Y,
        scroll_edges: crate::el::ScrollEdges { fade: 0.0, margin: 0.0 },
        sticky: None,
        growth: None,
        glide: None,
        a11y: crate::el::A11y { level: None, expanded: None, secure: false },
        pixel_aspect: None,
        aspect_fit: None,
        material: None,
        help: None,
        help_cut: false,
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
    pub(super) taffy: taffy::NodeId,
    pub text: Option<crate::cold::SharedBox<TextSpec>>,
    pub text_layout: Option<Rc<Layout<Ink>>>,
    /// Where `text_layout` cut the text for `El::truncate` and its kin.
    pub(crate) elided: Option<Elision>,
    pub(super) input: Option<Box<InputSpec>>,
    pub(super) focus: crate::cold::Cold<crate::el::FocusSpec>,
    pub(super) handlers: crate::cold::Cold<crate::el::Handlers>,
    pub(super) cold: crate::cold::Cold<NodeCold>,
    pub vis: Visual,
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
    pub(super) streaming: bool,
    pub(super) whole: bool,
    /// Inside an `El::inert` subtree (this element or an ancestor).
    pub(crate) inert: bool,
    /// `P_SCALE` has a track: paint reads the sprung factor.
    pub(super) scale_track: bool,
    /// `P_GROUP_REST` fades a dropped `El::group_opacity` to full: paint reads it until it lands.
    pub(super) group_fade: bool,
    pub(super) virtual_mount: bool,
    /// `El::selection_group`.
    pub(super) selection_group: bool,
    pub(super) window_drag: bool,
    pub(super) ime_cursor: bool,
    /// Placeholder left in `self.nodes` after `try_reuse_memo` moves a node into the new pass.
    pub(super) moved_stub: bool,
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
            target: old.target,
            local: old.local,
            inset: old.inset,
            motion: old.motion,
            static_scale: 1.0,
            time_scale: 1.0,
            in_row: RowLayout::Free,
            width_leads: false,
            instant_changes: false,
            streaming: false,
            whole: false,
            inert: false,
            scale_track: old.scale_track,
            group_fade: old.group_fade,
            virtual_mount: false,
            selection_group: false,
            window_drag: false,
            ime_cursor: false,
            moved_stub: true,
        }
    }

    #[inline]
    pub(super) fn is_moved_stub(&self) -> bool {
        self.moved_stub
    }

    /// Whether its text is a secret: marked secure, redacted, or a secure field.
    pub(super) fn is_secure(&self) -> bool {
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
    pub const NAMES: [&'static str; 10] =
        ["tracks", "growth", "scroll", "reveal", "loop", "scale-settle", "video", "text-flight", "live", "virtual-fill"];

    pub fn names(bits: u16) -> Vec<&'static str> {
        Self::NAMES.iter().enumerate().filter(|(i, _)| bits & (1 << i) != 0).map(|(_, n)| *n).collect()
    }
}

#[derive(Clone, Copy)]
pub(super) struct Inh {
    pub(super) opacity: f32,
    pub(super) paint_opacity: f32,
    pub(super) clip: [f32; 4],
    pub(super) fade: [f32; 4],
    pub(super) fade_origin: [f32; 4],
    /// Paint clips are consumed by isolation; logical clips remain for hit tests/inspection.
    pub(super) paint_clip: [f32; 4],
    pub(super) paint_fade: [f32; 4],
    pub(super) paint_fade_origin: [f32; 4],
    pub(super) rounded: Option<([f32; 4], [f32; 4])>,
    /// Ancestors' scales, composed: a point `x` (points, with `dx`/`dy` applied) draws at
    /// `anchor_to + (x - anchor) * scale`. The anchors differ when scales nest.
    pub(super) scale: f32,
    pub(super) anchor: [f32; 2],
    pub(super) anchor_to: [f32; 2],
    pub(super) blur: f32,
    /// Vertical offset in points from scroll viewports and growth rows above.
    pub(super) dy: f32,
    /// Horizontal offset in points: a matched flight centring its scaled children.
    pub(super) dx: f32,
    /// The part of `dx`/`dy` from flights' covers above. A flight nested in another travels its
    /// own root-space path, so it drops this.
    pub(super) cover: [f32; 2],
    /// The clip, fade and fade origin outside the nearest flight above, for a flight nested in it.
    pub(super) outer: Option<[[f32; 4]; 3]>,
    pub(super) mark: u32,
    /// The nearest hover group's hover amount (0–1), for `group_*` styles.
    pub(super) group: f32,
    /// How `scale` is changing, which decides how text rasterizes under it.
    pub(super) scale_motion: ScaleMotion,
    /// `(scale, anchor, anchor_to)` of the ancestors whose scales hold still from the root down,
    /// as an app-wide zoom does, stopping at the first that moves. A flight keeps this part.
    pub(super) still: (f32, [f32; 2], [f32; 2]),
    /// Inside an active `El::cache` scope: do not cull children against the window viewport.
    pub(super) in_cache: bool,
    /// Inside an elided or off-screen `El::cache` scope: skip primitive emission.
    pub(super) skip_paint: bool,
    /// Ancestors' `El::rotate` layers, composed, as an index into `turn_scratch` plus one
    /// (0: none): where this subtree's device pixels land. An index keeps `Inh` small.
    pub(super) turn: u32,
    pub(super) snap: pixel_snap::Inherit,
}

/// How a subtree's scale is changing. Text under a still scale rasterizes at its scaled size.
/// Under a live zoom gesture it steps through logarithmic $2^{1/4}$ buckets and lands on the
/// exact resting scale once still; under a short spring it samples its rest-size glyphs.
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ScaleMotion {
    #[default]
    Still,
    /// Bound to a live scale that is actively changing: rasterize at $2^{1/4}$ octave buckets.
    Live,
    Moving,
}

impl ScaleMotion {
    /// The scale text under `sc` rasterizes at. It draws at `sc`, filtered, when they differ.
    pub(super) fn raster(self, sc: f32) -> f32 {
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
pub(super) struct ScrollState {
    pub(super) offset: f64,
    pub(super) v: f32,
    pub(super) pinned: bool,
    pub(super) max: f64,
    pub(super) direct: bool,
    pub(super) scrolling: bool,
    /// Velocity of a glide to the bottom (`Tx::scroll_to_bottom`) or to `glide_to`
    /// (`Tx::scroll_to`) while one runs.
    pub(super) glide: Option<f32>,
    /// The glide is `ease_to_shrunk_end`'s, which retargets or ends it as the rows move.
    pub(super) shrink_glide: bool,
    /// A hand scrolled while the growth stack moved: no shrink glide until it settles.
    pub(super) shrink_cut: bool,
    /// Where a glide goes, when not to the bottom.
    pub(super) glide_to: Option<f64>,
    /// The growth stack's layout end while its rows still move (see `ease_to_shrunk_end`).
    pub(super) laid: Option<f64>,
    /// At-bottom state last sent to `El::on_at_bottom`.
    pub(super) at_bottom: Option<bool>,
    /// State last sent to `El::on_far_from_bottom`.
    pub(super) far_from_bottom: Option<bool>,
    /// Whether the last frame drew the content's bottom on the viewport's (see `stuck_offset`).
    pub(super) held: bool,
    /// Room freed under a pinned growth stack, held under its last row (see `step_slack`).
    pub(super) slack: scroll_bottom::Slack,
    /// The horizontal axis of `El::scroll_x` and `El::scroll_xy`; all zero on a vertical one.
    pub(super) x: AxisScroll,
    /// What `El::on_scroll_offset` was last told (`runtime/scroll_watch.rs`).
    pub(super) told: Option<crate::ScrollPos>,
}

/// A scroll viewport's horizontal offset, momentum velocity and limit (points).
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct AxisScroll {
    pub(super) offset: f64,
    pub(super) v: f32,
    pub(super) max: f64,
    /// Velocity of a glide to `aim` (`Tx::scroll_to`) while one runs.
    pub(super) glide: Option<f32>,
    pub(super) aim: f64,
}

#[derive(Clone, Debug, Default)]
pub(super) struct RevealState {
    pub(super) len: usize,
    /// (word start byte, start time) for words still fading.
    pub(super) words: Vec<(u32, f64)>,
    pub(super) fade: f32,
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
    /// Measure at the landing width immediately; defer height targets until it is stable.
    pub(super) width_hold_until: f64,
    pub(super) width_shrink_until: f64,
    pub(super) pending_heights: HashMap<NodeId, f32>,
    /// The window size at the last commit.
    pub(super) window: (f32, f32),
    /// How far its rows grew (positive steps only) this frame.
    pub(super) grown: f32,
    /// Whether any row in the stack still has height left to grow.
    pub(crate) growing: bool,
    /// Whether any row in the stack is still animating.
    pub(crate) moving: bool,
    pub(super) reorder: row_reorder::Reorder,
    pub(super) slides: content_slide::ContentSlides,
}

pub(super) struct LayoutEntry {
    pub(super) id: taffy::NodeId,
    pub(super) measure: Option<u64>,
    pub(super) used: bool,
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
