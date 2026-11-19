//! Per-frame paint scratch buffers, drawn geometry, layer caches, text quad caches and paint
//! outputs owned by [`RuntimeCore`].
use super::*;

/// Retained paint-pass scratch, reuse record, drawn geometry, layer caches and per-frame paint
/// outputs. Ordered hot-first under `#[repr(C)]` so per-node vectors (`inh_scratch`, `drawn`,
/// `row_dy`, `row_h`, `row_mask`, `drawn_scale`, `seen`) and `pixel_snap` sit together and first
/// ahead of the larger reuse record and cold cache tables.
#[repr(C)]
pub(crate) struct PaintState {
    pub(in crate::runtime) inh_scratch: Vec<Inh>,
    /// Drawn rect per node (points, root space), for tools and hit tests.
    pub(in crate::runtime) drawn: Vec<[f32; 4]>,
    pub(in crate::runtime) quads_frame: u64,
    /// Opt-in gamma-space (sRGB) blending for translucent fills, shadows and blurs.
    pub(crate) srgb_blend: bool,
    /// The shell shows native views for the first `MAX_NATIVE_MATERIALS` of `materials`; their
    /// elements skip their fill and cut their shadows out of their interior.
    pub(crate) native_materials: bool,
    /// A drawn video is playing or seeking: keep drawing frames.
    pub(in crate::runtime) videos_active: bool,
    /// Frame requested by scale settling or pixel snapping in the last paint.
    pub(in crate::runtime) scale_frame: Option<AnotherFrame>,
    /// Per node index this frame: presented row top dy, row height override, and bottom feather mask.
    pub(in crate::runtime) row_dy: Vec<f32>,
    pub(in crate::runtime) row_h: Vec<f32>,
    pub(in crate::runtime) row_mask: Vec<f32>,
    /// The scale each node's text drew at (its own and its ancestors'), for text hit tests.
    pub(in crate::runtime) drawn_scale: Vec<f32>,
    /// Per node: it drew inside its clip and the window in the last build.
    pub(in crate::runtime) seen: Vec<bool>,
    /// Drawn videos' state generations; a change re-runs the view.
    pub(in crate::runtime) video_signature: u64,
    pub(in crate::runtime) pixel_snap: pixel_snap::Glides,
    /// Last scale observation per node that has a scale or had one. See [`ScaleMotion`].
    pub(in crate::runtime) static_scales: HashMap<NodeId, ScaleSettle>,
    /// What the last paint painted, for subtrees that would paint it again (`paint_keep`).
    pub(in crate::runtime) keep: paint_keep::PaintKeep,
    /// The composed turns `Inh::turn` points into, rebuilt each paint.
    pub(in crate::runtime) turn_scratch: Vec<Turn>,
    /// For nodes inside a turned layer: `(unturned_rect_pt, unturned_clip_px)`. Empty unless a
    /// node turns.
    pub(in crate::runtime) unturned_drawn: Vec<([f32; 4], [f32; 4])>,
    /// Growth rows whose contents draw lifted this frame (`GrowTrack::hide_above`): index, points.
    pub(in crate::runtime) row_lifts: Vec<(usize, f32)>,
    /// Only nonempty when retained exit nodes or matched flights make storage order differ
    /// from paint order.
    pub(in crate::runtime) layer_paint_order: Vec<usize>,
    pub(in crate::runtime) dissolve_scratch: Vec<bool>,
    pub(in crate::runtime) glyph_scratch: Vec<GlyphQuad>,
    pub(in crate::runtime) deco_scratch: Vec<PaintedDeco>,
    pub(in crate::runtime) layer_scratch: Vec<(usize, usize, usize)>,
    pub(in crate::runtime) cache_live_scratch: Vec<bool>,
    pub(in crate::runtime) cache_check_scratch: Vec<bool>,
    pub(in crate::runtime) cache_paint_scratch: Vec<u64>,
    pub(in crate::runtime) cache_inside_scratch: Vec<bool>,
    pub(in crate::runtime) node_caches: HashMap<NodeId, NodeCache>,
    /// The device rows of each field with waiting paragraphs its last paint showed (`text_fill`).
    pub(in crate::runtime) text_rows: HashMap<NodeId, (f32, f32)>,
    /// Each still text's quads as it drew them, by node and layout id, and the paint they were
    /// last used in (`paint_content`): a frame that draws it the same copies them.
    pub(in crate::runtime) text_quads: HashMap<(NodeId, crate::text::LayoutId), (crate::el::grid::DrawnRow, u64)>,
    /// `El::system_material` elements built this frame, in paint order.
    pub(crate) materials: Vec<crate::system_material::MaterialDraw>,
    /// Each node's place in the paint order, kept by the frame build while `Some` (the motion
    /// gate's `z-order`).
    pub(crate) paint_rank: Option<Vec<u32>>,
    /// While `paint_rank` is kept: this frame's icons drawn through their transparent one-texel
    /// apron (`icon_inst` off the pixel grid), by atlas place (`apron_key`), for the gate's raster.
    pub(crate) icon_aprons: Vec<[u32; 3]>,
    /// Still texts that copied their quads, for tests.
    #[cfg(test)]
    pub(in crate::runtime) quads_copied: u64,
}

impl PaintState {
    // Single-caller constructor for `RuntimeCore::new`; inlined so it stays in its caller's
    // codegen unit instead of shifting partitioning and dropping inlining in the paint walk.
    #[inline]
    pub(in crate::runtime) fn new() -> Self {
        Self {
            inh_scratch: Vec::new(),
            drawn: Vec::new(),
            quads_frame: 0,
            srgb_blend: false,
            native_materials: false,
            videos_active: false,
            scale_frame: None,
            row_dy: Vec::new(),
            row_h: Vec::new(),
            row_mask: Vec::new(),
            drawn_scale: Vec::new(),
            seen: Vec::new(),
            video_signature: 0,
            pixel_snap: Default::default(),
            static_scales: HashMap::default(),
            keep: Default::default(),
            turn_scratch: Vec::new(),
            unturned_drawn: Vec::new(),
            row_lifts: Vec::new(),
            layer_paint_order: Vec::new(),
            dissolve_scratch: Vec::new(),
            glyph_scratch: Vec::new(),
            deco_scratch: Vec::new(),
            layer_scratch: Vec::new(),
            cache_live_scratch: Vec::new(),
            cache_check_scratch: Vec::new(),
            cache_paint_scratch: Vec::new(),
            cache_inside_scratch: Vec::new(),
            node_caches: HashMap::default(),
            text_rows: HashMap::default(),
            text_quads: HashMap::default(),
            materials: Vec::new(),
            paint_rank: None,
            icon_aprons: Vec::new(),
            #[cfg(test)]
            quads_copied: 0,
        }
    }
}
