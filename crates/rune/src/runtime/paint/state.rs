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
    pub(crate) drawn: Vec<[f32; 4]>,
    pub(super) quads_frame: u64,
    /// Opt-in gamma-space (sRGB) blending for translucent fills, shadows and blurs.
    pub(in crate::runtime) srgb_blend: bool,
    /// The shell shows native views for the first `MAX_NATIVE_MATERIALS` of `materials`; their
    /// elements skip their fill and cut their shadows out of their interior.
    pub(super) native_materials: bool,
    /// A drawn video is playing or seeking: keep drawing frames.
    pub(super) videos_active: bool,
    /// A drawn animated SVG icon is advancing right now: keep drawing frames.
    pub(super) animated_icons: bool,
    /// A painted animated SVG icon is active (moving or waiting on a flat hold), even while the
    /// window is covered: tells Windows occlusion polling (`loops_shown`) to watch for uncover.
    pub(super) animated_icons_shown: bool,
    /// Current node painted a live animated icon (moving or waiting): prevents `PaintKeep` from
    /// freezing its quad across frames while active, without blocking sibling subtrees.
    pub(super) node_animated_icon: bool,
    /// Earliest runtime timestamp when a flat-hold animated icon resumes moving (`wake_for_motion`).
    pub(super) icon_anim_wake: Option<f64>,
    /// Mount time (runtime `t`) per `(NodeId, icon_ptr)` for finite-duration animated icons.
    pub(super) icon_anim_mounts: HashMap<(NodeId, usize), f64>,
    /// Frame requested by scale settling or pixel snapping in the last paint.
    pub(super) scale_frame: Option<AnotherFrame>,
    /// Per node index this frame: presented row top dy, row height override, and bottom feather mask.
    pub(in crate::runtime) row_dy: Vec<f32>,
    pub(in crate::runtime) row_h: Vec<f32>,
    pub(in crate::runtime) row_mask: Vec<f32>,
    /// The scale each node's text drew at (its own and its ancestors'), for text hit tests.
    pub(crate) drawn_scale: Vec<f32>,
    /// Per node: it drew inside its clip and the window in the last build.
    pub(super) seen: Vec<bool>,
    /// Drawn videos' state generations; a change re-runs the view.
    pub(super) video_signature: u64,
    pub(super) pixel_snap: pixel_snap::Glides,
    /// Last scale observation per node that has a scale or had one. See [`ScaleMotion`].
    pub(super) static_scales: HashMap<NodeId, ScaleSettle>,
    /// What the last paint painted, for subtrees that would paint it again (`paint_keep`).
    pub(super) keep: paint_keep::PaintKeep,
    /// The composed turns `Inh::turn` points into, rebuilt each paint.
    pub(in crate::runtime) turn_scratch: Vec<Turn>,
    /// For nodes inside a turned layer: `(unturned_rect_pt, unturned_clip_px)`. Empty unless a
    /// node turns.
    pub(in crate::runtime) unturned_drawn: Vec<([f32; 4], [f32; 4])>,
    /// Growth rows whose contents draw lifted this frame (`GrowTrack::hide_above`): index, points.
    pub(super) row_lifts: Vec<(usize, f32)>,
    /// Only nonempty when retained exit nodes or matched flights make storage order differ
    /// from paint order.
    pub(super) layer_paint_order: Vec<usize>,
    /// Sorted indices of anchored overlays lifted in `update_layer_paint_order` so they paint
    /// after (and above) a `.raise()` or flying subtree containing their anchor.
    pub(super) lifted_anchors: Vec<usize>,
    /// Last resolved `(target_rect, zoom)` per anchored overlay node id, so an exiting overlay
    /// holds its position if its anchor unmounts mid-exit instead of snapping to `(0, 0)`.
    pub(in crate::runtime) last_anchor_targets: HashMap<NodeId, ([f32; 4], f32)>,
    /// Deduplicated `(overlay_path, anchor_key)` pairs logged when an anchored overlay could not
    /// resolve its anchor and was suppressed.
    pub(in crate::runtime) unresolved_anchor_warnings: HashSet<(String, String)>,
    pub(super) dissolve_scratch: Vec<bool>,
    pub(in crate::runtime) glyph_scratch: Vec<GlyphQuad>,
    pub(super) deco_scratch: Vec<PaintedDeco>,
    pub(super) layer_scratch: Vec<(usize, usize, usize)>,
    pub(in crate::runtime) cache_live_scratch: Vec<bool>,
    pub(super) cache_check_scratch: Vec<bool>,
    pub(super) cache_paint_scratch: Vec<u64>,
    pub(super) cache_inside_scratch: Vec<bool>,
    pub(in crate::runtime) node_caches: HashMap<NodeId, NodeCache>,
    /// The device rows of each field with waiting paragraphs its last paint showed (`text_fill`).
    pub(in crate::runtime) text_rows: HashMap<NodeId, (f32, f32)>,
    /// Each still text's quads as it drew them, by node and layout id, and the paint they were
    /// last used in (`paint_content`): a frame that draws it the same copies them.
    pub(super) text_quads: HashMap<(NodeId, crate::text::LayoutId), (crate::el::grid::DrawnRow, u64)>,
    /// `El::system_material` elements built this frame, in paint order.
    pub(super) materials: Vec<crate::system_material::MaterialDraw>,
    /// Each node's place in the paint order, kept by the frame build while `Some` (the motion
    /// gate's `z-order`).
    pub(super) paint_rank: Option<Vec<u32>>,
    /// While `paint_rank` is kept: this frame's icons drawn through their transparent one-texel
    /// apron (`icon_inst` off the pixel grid), by atlas place (`apron_key`), for the gate's raster.
    pub(super) icon_aprons: Vec<[u32; 3]>,
    /// Still texts that copied their quads, for tests.
    #[cfg(test)]
    pub(super) quads_copied: u64,
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
            animated_icons: false,
            animated_icons_shown: false,
            node_animated_icon: false,
            icon_anim_wake: None,
            icon_anim_mounts: HashMap::default(),
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
            lifted_anchors: Vec::new(),
            last_anchor_targets: HashMap::default(),
            unresolved_anchor_warnings: HashSet::default(),
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

    #[inline]
    pub(in crate::runtime) fn srgb_blend(&self) -> bool {
        self.srgb_blend
    }

    #[inline]
    pub(in crate::runtime) fn set_srgb_blend(&mut self, enabled: bool) -> bool {
        let changed = self.srgb_blend != enabled;
        self.srgb_blend = enabled;
        changed
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn native_materials(&self) -> bool {
        self.native_materials
    }

    #[cfg(any(test, target_os = "macos"))]
    #[inline]
    pub(crate) fn set_native_materials(&mut self, native: bool) {
        self.native_materials = native;
    }

    #[inline]
    pub(in crate::runtime) fn videos_active(&self) -> bool {
        self.videos_active
    }

    #[inline]
    pub(in crate::runtime) fn animated_icons(&self) -> bool {
        self.animated_icons
    }

    #[inline]
    pub(in crate::runtime) fn animated_icons_shown(&self) -> bool {
        self.animated_icons_shown
    }

    #[inline]
    pub(in crate::runtime) fn icon_anim_wake(&self) -> Option<f64> {
        self.icon_anim_wake
    }

    #[inline]
    pub(in crate::runtime) fn note_icon_anim_wake(&mut self, at: f64) {
        self.icon_anim_wake = Some(match self.icon_anim_wake {
            Some(prev) => prev.min(at),
            None => at,
        });
    }

    #[inline]
    pub(in crate::runtime) fn icon_mount_time(&self, id: NodeId, icon_ptr: usize, fallback: f64) -> f64 {
        self.icon_anim_mounts.get(&(id, icon_ptr)).copied().unwrap_or(fallback)
    }

    #[inline]
    pub(in crate::runtime) fn get_or_insert_icon_mount(&mut self, id: NodeId, icon_ptr: usize, now: f64) -> f64 {
        *self.icon_anim_mounts.entry((id, icon_ptr)).or_insert(now)
    }

    #[inline]
    pub(in crate::runtime) fn scale_settling(&self) -> bool {
        self.scale_frame.is_some() || self.pixel_snap.easing()
    }

    #[inline]
    pub(in crate::runtime) fn seen(&self) -> &[bool] {
        &self.seen
    }

    #[inline]
    pub(in crate::runtime) fn was_seen(&self, i: usize, default: bool) -> bool {
        self.seen.get(i).copied().unwrap_or(default)
    }

    #[inline]
    pub(in crate::runtime) fn prune_static_scales(&mut self, scene: &Scene) {
        if !self.static_scales.is_empty() {
            self.static_scales.retain(|id, _| scene.contains(*id));
        }
    }

    #[cfg(test)]
    #[inline]
    pub(in crate::runtime) fn tracks_static_scale(&self, id: NodeId) -> bool {
        self.static_scales.contains_key(&id)
    }

    #[inline]
    pub(in crate::runtime) fn prune_caches(
        &mut self,
        has_cache: bool,
        scene: &Scene,
        resident_caches: &mut std::collections::HashMap<u64, u64>,
    ) {
        if !has_cache {
            if !self.node_caches.is_empty() { self.node_caches.clear(); }
            if !resident_caches.is_empty() { resident_caches.clear(); }
        } else {
            if !self.node_caches.is_empty() {
                self.node_caches.retain(|id, _| scene.contains(*id));
            }
            if !resident_caches.is_empty() {
                resident_caches.retain(|id, _| scene.contains(NodeId(*id)));
            }
        }
    }

    #[inline]
    pub(in crate::runtime) fn begin_frame(&mut self) {
        self.keep.frame += 1;
    }

    #[inline]
    pub(in crate::runtime) fn record_moved(&mut self, ids: impl IntoIterator<Item = NodeId>) {
        self.keep.moved.extend(ids);
    }

    #[inline]
    pub(in crate::runtime) fn note_cycle_other_deadline(&mut self) {
        self.keep.cycles.other_deadline();
    }

    #[inline]
    pub(in crate::runtime) fn take_cycle_check_clock(&mut self) -> Option<f64> {
        self.keep.cycles.check_clock.take()
    }

    #[inline]
    pub(in crate::runtime) fn clear_row_lifts(&mut self) {
        self.row_lifts.clear();
    }

    #[inline]
    pub(in crate::runtime) fn push_row_lift(&mut self, ri: usize, lift: f32) {
        self.row_lifts.push((ri, lift));
    }

    #[inline]
    pub(in crate::runtime) fn row_lifts(&self) -> &[(usize, f32)] {
        &self.row_lifts
    }

    #[inline]
    pub(in crate::runtime) fn layer_paint_order(&self) -> &[usize] {
        &self.layer_paint_order
    }

    #[inline]
    pub(in crate::runtime) fn clear_layer_paint_order(&mut self) {
        self.layer_paint_order.clear();
        self.lifted_anchors.clear();
    }

    #[inline]
    pub(in crate::runtime) fn is_lifted_anchor(&self, i: usize) -> bool {
        self.lifted_anchors.binary_search(&i).is_ok()
    }

    #[inline]
    pub(in crate::runtime) fn has_lifted_anchors(&self) -> bool {
        !self.lifted_anchors.is_empty()
    }

    /// Subtree (preorder) paint order, then each flying, raised, or lifted-anchor element's
    /// subtree on top. A sticky element paints after its normal siblings (and before overlay
    /// siblings), so it covers the rows it's stuck over.
    pub(in crate::runtime) fn update_layer_paint_order(
        &mut self,
        tree: &types::TreeState,
        flying: &[(NodeId, MRect)],
        sticky_of: impl Fn(usize) -> bool,
        exiting: impl Fn(usize) -> bool,
    ) {
        let n = tree.nodes.len();
        let mut first = vec![n; n + 1];
        let mut next = vec![n; n];
        // Siblings paint by `El::paint_order`, then (with sticky elements about) sticky ones over
        // the rows they're stuck over and anchored overlays and modals over both.
        let rank = |i: usize| {
            let nd = &tree.nodes[i];
            let place = if !tree.has_sticky { 0 }
                else if nd.above.is_some() || nd.handlers.modal.is_some() { 2 }
                else if sticky_of(i) { 1 }
                else { 0 };
            (nd.vis.paint_order, place)
        };
        if tree.has_sticky || tree.has_paint_order {
            let mut ranks: Vec<_> = (0..n).map(|i| (rank(i), i)).collect();
            ranks.sort_unstable();
            for &(_, i) in ranks.iter().rev() {
                let parent = tree.nodes[i].parent.map_or(0, |p| p + 1);
                next[i] = first[parent]; first[parent] = i;
            }
        } else {
            for i in (0..n).rev() {
                let parent = tree.nodes[i].parent.map_or(0, |p| p + 1);
                next[i] = first[parent]; first[parent] = i;
            }
        }
        let mut lifted: Vec<usize> = flying.iter().filter_map(|(id, _)| tree.node_index.get(id).copied()).collect();
        if tree.has_raise { lifted.extend((0..n).filter(|&i| tree.nodes[i].vis_flags.raise())); }
        lifted.sort_unstable();
        lifted.dedup();
        self.lifted_anchors.clear();
        let mut lifted_pos = if !lifted.is_empty() || tree.has_above { vec![0u32; n] } else { Vec::new() };
        let mut next_pass = 1u32;
        for &r in &lifted {
            lifted_pos[r] = next_pass;
            next_pass += 1;
        }
        if tree.has_above {
            // An anchored overlay whose anchor paints after it (either mounted later in tree order,
            // such as a select portal built before its dialog, or inside a `.raise()`/flying
            // subtree) must paint after its anchor so `drawn[anchor]` is populated and the overlay
            // draws and hit-tests above it.
            let mut base_rank = vec![0usize; n];
            let mut rank_pos = 0usize;
            let mut cur = first[0];
            while cur < n {
                base_rank[cur] = rank_pos;
                rank_pos += 1;
                if first[cur + 1] < n { cur = first[cur + 1]; continue; }
                loop {
                    if next[cur] < n { cur = next[cur]; break; }
                    match tree.nodes[cur].parent { Some(parent) => cur = parent, None => { cur = n; break; } }
                }
            }
            let above_nodes: Vec<usize> = (0..n).filter(|&i| tree.nodes[i].above.is_some()).collect();
            let pass_of = |mut k: usize, pos: &[u32]| -> u32 {
                loop {
                    if pos[k] != 0 { return pos[k]; }
                    let Some(p) = tree.nodes[k].parent else { return 0 };
                    k = p;
                }
            };
            let max_rounds = above_nodes.len() + lifted.len() + 2;
            for _ in 0..max_rounds {
                let mut changed = false;
                for &i in &above_nodes {
                    let Some(a) = &tree.nodes[i].above else { continue };
                    let Some(j) = super::anchor_index_live(&tree.nodes, i, a, &exiting) else { continue };
                    if (pass_of(j, &lifted_pos), base_rank[j]) > (pass_of(i, &lifted_pos), base_rank[i]) {
                        if lifted_pos[i] == 0 {
                            self.lifted_anchors.push(i);
                        }
                        lifted_pos[i] = next_pass;
                        next_pass += 1;
                        changed = true;
                    }
                }
                // Keep any lifted node after every lifted ancestor so `inh_scratch[parent]` is
                // populated before the lifted child paints.
                for u in 0..n {
                    if lifted_pos[u] == 0 { continue; }
                    let Some(p) = tree.nodes[u].parent else { continue };
                    if pass_of(p, &lifted_pos) > lifted_pos[u] {
                        lifted_pos[u] = next_pass;
                        next_pass += 1;
                        changed = true;
                    }
                }
                if !changed { break; }
            }
            if !self.lifted_anchors.is_empty() || !lifted.is_empty() {
                lifted.clear();
                lifted.extend((0..n).filter(|&i| lifted_pos[i] != 0));
                lifted.sort_unstable_by_key(|&i| lifted_pos[i]);
                self.lifted_anchors.sort_unstable();
                self.lifted_anchors.dedup();
            }
        }
        self.layer_paint_order.clear();
        for top in std::iter::once(None).chain(lifted.iter().map(|&r| Some(r))) {
            let mut i = top.unwrap_or(first[0]);
            while i < n {
                // A flying, raised or lifted-anchor subtree paints in its own pass, not where the tree reaches it.
                if top == Some(i) || lifted_pos.get(i).copied().unwrap_or(0) == 0 {
                    self.layer_paint_order.push(i);
                    if first[i + 1] < n { i = first[i + 1]; continue; }
                }
                loop {
                    if top == Some(i) { i = n; break; }
                    if next[i] < n { i = next[i]; break; }
                    match tree.nodes[i].parent { Some(parent) => i = parent, None => { i = n; break; } }
                }
            }
        }
        if self.layer_paint_order.iter().copied().eq(0..n) {
            self.layer_paint_order.clear();
            self.lifted_anchors.clear();
        }
    }

    #[cfg(test)]
    #[inline]
    pub(in crate::runtime) fn dissolve_scratch(&self) -> &[bool] {
        &self.dissolve_scratch
    }

    #[inline]
    pub(crate) fn materials(&self) -> &[crate::system_material::MaterialDraw] {
        &self.materials
    }

    #[inline]
    pub(crate) fn enable_paint_rank(&mut self) {
        self.paint_rank = Some(Vec::new());
    }

    #[inline]
    pub(in crate::runtime) fn paint_rank(&self, i: usize) -> u32 {
        self.paint_rank.as_ref().and_then(|r| r.get(i)).copied().unwrap_or(i as u32)
    }

    #[inline]
    pub(crate) fn icon_aprons(&self) -> &[[u32; 3]] {
        &self.icon_aprons
    }

    #[cfg(test)]
    #[inline]
    pub(in crate::runtime) fn quads_copied(&self) -> u64 {
        self.quads_copied
    }
}
