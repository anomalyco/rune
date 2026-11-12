//! Bounded painter scopes. The ordinary path retains its instance buffer, batches and pass;
//! only scopes requiring isolation acquire a target. Scope records are also compositor fences.
use super::*;
use std::ops::Range;

mod plan;
use plan::{Describe, Disposition, Held, Kept, Layer, Move, Planned, Planner, Pools};

// Linear premultiplied intermediates preserve HDR colours and avoid accumulating 8-bit
// quantization through nested layers. The destination format is applied only at composition.
pub(super) const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Default offscreen texture budget for [`El::cache`](crate::El::cache) subtrees (64 MiB).
pub const DEFAULT_CACHE_BUDGET: u64 = 64 * 1024 * 1024;
/// What spare layer targets may hold (Layers::spare): a full-window layer at 2880x1800 or two menus' worth.
const SPARE_BYTES: u64 = 32 * 1024 * 1024;
/// What they keep once the window rests (`window::rest`): the most recently used, which the
/// screen on show takes again for its menus and hovers. Making a target costs the frame that
/// takes it a few ms on an integrated GPU.
const RESTING_SPARE_BYTES: u64 = 8 * 1024 * 1024;
/// Maximum width or height of a cached subtree's offscreen texture in device pixels.
const MAX_CACHE_DIM: u32 = 4096;

#[cfg(test)]
#[path = "layers/plan_frame_tests.rs"]
mod plan_frame_tests;
#[cfg(test)]
#[path = "../runtime/layer_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "../runtime/layer_cache_tests.rs"]
mod cache_tests;
#[cfg(test)]
#[path = "layer_filter_tests.rs"]
mod filter_tests;
#[cfg(test)]
#[path = "../runtime/rounded_clip_tests.rs"]
mod rounded_clip_tests;
#[cfg(test)]
#[path = "continuous_corner_tests.rs"]
mod continuous_corner_tests;
#[cfg(test)]
#[path = "layer_show_tests.rs"]
mod show_tests;
#[cfg(test)]
#[path = "layer_rebind_tests.rs"]
mod rebind_tests;

pub(super) struct Layers {
    plan: Vec<Layer>,
    /// What each layer does this frame, and the moves the target pools take to get there.
    planner: Planner,
    pub targets: Vec<Option<Target>>,
    retained: Vec<Target>,
    /// Targets no layer drew into this frame (a menu closed, a layer's place in the plan moved),
    /// kept for the next layer of their size: opening a menu or a popover again makes no texture.
    spare: Vec<Target>,
    stack: Vec<usize>,
    pipelines: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    pub additive: Option<(wgpu::RenderPipeline, wgpu::RenderPipeline)>,
    pub blur: Option<layer_blur::Pipeline>,
    viewport: (u32, u32),
    image_generation: u64,
    allocations: u64,
    /// The atlas and outline texture generation: every target's base bind group names the
    /// generation it was made against, and a parked target is bound again only when a layer takes
    /// it in a later one.
    bindings: u64,
    cache_budget: u64,
    frame_stamp: u64,
    /// Test-only: the most bytes of live targets and their blur scratches a frame has held at once.
    #[cfg(test)]
    peak: u64,
    /// Test-only: how many times a target's base bind group has been made again.
    #[cfg(test)]
    pub(super) binds: u64,
}

impl Layers {
    /// Whether every group of the frame lands where its instances say: drawn straight into its
    /// parent's pass, or into a target composited back at its own place, neither turned nor
    /// blurred, so a changed instance changes only the pixels its bounds cover
    /// (`Renderer::redraws_in_place`).
    #[cfg(target_os = "windows")]
    pub(super) fn in_place(&self) -> bool {
        self.plan.iter().enumerate().all(|(i, l)| match self.planner.dispositions.get(i) {
            Some(Disposition::Inline | Disposition::Skip) | None => true,
            Some(Disposition::Render { .. } | Disposition::Cached { .. }) => l.turn.is_none() && !l.free && l.blur == 0.0,
        })
    }

    /// Kept textures get the budget left after this frame's visible caches.
    fn trim_retained(&mut self, active_bytes: u64) {
        let available = self.cache_budget.saturating_sub(active_bytes);
        let mut bytes: u64 = self.retained.iter().map(Renderer::target_bytes).sum();
        while bytes > available {
            let oldest = self.retained.iter().enumerate().min_by_key(|(_, t)| t.last_used).unwrap().0;
            let evicted = self.retained.swap_remove(oldest);
            bytes -= Renderer::target_bytes(&evicted);
        }
    }

    /// Keeps `t` for a later layer of its size, within `SPARE_BYTES`; the least recently used go.
    fn park(&mut self, mut t: Target) {
        t.cache = None;
        self.spare.push(t);
        self.trim_spare(SPARE_BYTES);
    }

    /// Drops the least recently used spare targets until they hold at most `limit` bytes.
    fn trim_spare(&mut self, limit: u64) {
        let mut bytes: u64 = self.spare.iter().map(Renderer::target_bytes).sum();
        while bytes > limit {
            let oldest = self.spare.iter().enumerate().min_by_key(|(_, t)| t.last_used).unwrap().0;
            bytes -= Renderer::target_bytes(&self.spare.swap_remove(oldest));
        }
    }

    /// Test-only: notes the peak of the bytes this frame holds live, targets and blur scratches
    /// alike, so a test can pin it against the peak dev held.
    #[cfg(test)]
    fn watch_peak(&mut self) {
        let bytes: u64 = self.targets.iter().flatten().chain(&self.retained).chain(&self.spare)
            .map(Renderer::target_bytes).sum();
        self.peak = self.peak.max(bytes);
    }

    /// Test-only: the most live target bytes the last frame held at once.
    #[cfg(test)]
    pub(super) fn peak_bytes(&self) -> u64 { self.peak }

    /// Makes every blurred layer write its blur uniforms again on its next prepare.
    pub(super) fn forget_blur_uniforms(&mut self) {
        let targets = self.targets.iter_mut().flatten().chain(&mut self.retained).chain(&mut self.spare);
        for blur in targets.filter_map(|t| t.blur.as_mut()) { blur.forget_uniforms(); }
    }
}

impl Default for Layers {
    fn default() -> Self {
        Self {
            plan: Vec::new(),
            planner: Planner::default(),
            targets: Vec::new(),
            retained: Vec::new(),
            spare: Vec::new(),
            stack: Vec::new(),
            pipelines: Vec::new(),
            additive: None,
            blur: None,
            viewport: (0, 0),
            image_generation: 0,
            allocations: 0,
            bindings: 0,
            cache_budget: DEFAULT_CACHE_BUDGET,
            frame_stamp: 0,
            #[cfg(test)]
            peak: 0,
            #[cfg(test)]
            binds: 0,
        }
    }
}

/// The upright bounds of edges `r` turned `angle` radians clockwise about `p`.
fn turned_bounds(r: [f32; 4], angle: f32, p: [f32; 2]) -> [f32; 4] {
    let (sn, c) = angle.sin_cos();
    let mut out = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
    for (x, y) in [(r[0], r[1]), (r[2], r[1]), (r[0], r[3]), (r[2], r[3])] {
        let (dx, dy) = (x - p[0], y - p[1]);
        let (qx, qy) = (p[0] + c * dx - sn * dy, p[1] + sn * dx + c * dy);
        out = [out[0].min(qx), out[1].min(qy), out[2].max(qx), out[3].max(qy)];
    }
    out
}

pub(super) struct Target {
    identity: u64,
    composite_parent: Option<u64>,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    projection: wgpu::Buffer,
    quad: wgpu::Buffer,
    composite: wgpu::BindGroup,
    pub base: wgpu::BindGroup,
    pub images: Vec<Option<wgpu::BindGroup>>,
    pub glyphs: Vec<wgpu::BindGroup>,
    image_generation: u64,
    blur: Option<layer_blur::Scratch>,
    /// The node and signature of the cached content, if this target is kept.
    cache: Option<(u64, u64)>,
    last_used: u64,
    content_size: [f32; 2],
    /// The `Layers::bindings` generation the base bind group was made against.
    binding: u64,
    /// Test-only: the outline texture's height the base bind group was made against.
    #[cfg(test)]
    outline_rows: u32,
}

impl Describe for Target {
    /// What the plan reads about it: its size, what it keeps and what it costs, with no handle.
    fn held(&self) -> Held {
        Held {
            size: [self.texture.width(), self.texture.height()],
            bytes: Renderer::target_bytes(self),
            cache: self.cache,
            last_used: self.last_used,
            content: self.content_size,
        }
    }
}

/// Binds `t` to the frame's atlas and outline texture, and to `generation`: its base bind group
/// here, its composite and image and glyph pages at the next `prepare_layers`.
fn rebind_target(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, globals: &wgpu::Buffer,
    outlines: Option<&wgpu::Texture>, atlas: &wgpu::Texture, t: &mut Target, generation: u64) {
    t.base = Renderer::bind_group(device, layout, globals, outlines, atlas, atlas, Some(&t.projection));
    // No target's identity is 0, so the composite is made again.
    t.composite_parent = Some(0);
    t.image_generation = u64::MAX;
    t.binding = generation;
    #[cfg(test)]
    { t.outline_rows = outlines.map_or(0, wgpu::Texture::height); }
}

/// Resource/pass counters for diagnostics. The ordinary path creates no layer resources.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LayerStats {
    pub targets: usize,
    pub bytes: u64,
    pub extra_passes: usize,
    pub allocations: u64,
}

fn union(a: &mut [f32; 4], b: [f32; 4]) {
    if b[2] <= b[0] || b[3] <= b[1] { return; }
    a[0] = a[0].min(b[0]); a[1] = a[1].min(b[1]);
    a[2] = a[2].max(b[2]); a[3] = a[3].max(b[3]);
}

impl Renderer {
    fn target_bytes(t: &Target) -> u64 {
        let mut bytes = u64::from(t.texture.width()) * u64::from(t.texture.height())
            * u64::from(FORMAT.block_copy_size(None).unwrap());
        if let Some(blur) = &t.blur {
            bytes += blur.bytes();
        }
        bytes
    }

    /// Maximum bytes of offscreen textures retained for [`El::cache`](crate::El::cache) subtrees
    /// before least-recently-used entries are evicted. Defaults to 64 MiB.
    pub fn set_cache_budget(&mut self, bytes: u64) {
        self.layers.cache_budget = bytes;
        self.layers.trim_retained(0);
    }

    /// Current [`El::cache`](crate::El::cache) texture memory budget in bytes.
    pub fn cache_budget(&self) -> u64 {
        self.layers.cache_budget
    }

    /// With `alloc-sites` (a measuring build), the heap dump's `rune-gpu` section lists what this
    /// renderer's pools hold after its latest frame or rest, as `<pool> <count> <bytes>`.
    #[cfg(all(feature = "alloc-sites", not(target_arch = "wasm32")))]
    pub(super) fn report_pools(&self) {
        static POOLS: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
        static ONCE: std::sync::Once = std::sync::Once::new();
        fn texture_bytes(t: &wgpu::Texture) -> u64 {
            let block = u64::from(t.format().block_copy_size(None).unwrap_or(4));
            (0..t.mip_level_count()).map(|m| u64::from((t.width() >> m).max(1)) * u64::from((t.height() >> m).max(1)) * block).sum()
        }
        let pool = |name: &str, ts: &mut dyn Iterator<Item = u64>| {
            let (n, bytes) = ts.fold((0, 0), |(n, b), x| (n + 1, b + x));
            format!("{name} {n} {bytes}\n")
        };
        let l = &self.layers;
        let out = [
            pool("layer-live", &mut l.targets.iter().flatten().filter(|t| t.cache.is_none()).map(Self::target_bytes)),
            pool("layer-cache-live", &mut l.targets.iter().flatten().filter(|t| t.cache.is_some()).map(Self::target_bytes)),
            pool("layer-retained", &mut l.retained.iter().map(Self::target_bytes)),
            pool("layer-spare", &mut l.spare.iter().map(Self::target_bytes)),
            pool("backdrops", &mut self.backdrops.iter().map(|b| texture_bytes(&b.texture))),
            pool("glyph-atlas", &mut std::iter::once(&self.atlas).chain(self.atlas_pages.iter().map(|p| &p.0)).map(texture_bytes)),
            pool("image-pages", &mut self.images.iter().flatten().map(|p| texture_bytes(&p.0))),
        ].concat();
        *POOLS.lock().unwrap_or_else(|e| e.into_inner()) = out;
        ONCE.call_once(|| procwatch::alloc::sites::contribute("rune-gpu", || POOLS.lock().unwrap_or_else(|e| e.into_inner()).clone()));
    }

    /// What a window at rest gives back (`window::rest`): spare targets past the most recently
    /// used `RESTING_SPARE_BYTES`, and cache textures no layer drew last frame. With `all`, every
    /// spare target, and the scratch targets of this screen's layers and its backdrop copies and
    /// blur pyramid, which its next frame makes again (a few ms on an integrated GPU). It keeps the
    /// textures of caches on screen, which the last frame's instances name instead of their
    /// contents, and takes the ones it drops out of `resident`, so their next frame paints them.
    /// What no submission still uses is freed here; the rest when the device next polls.
    pub(crate) fn give_back(&mut self, all: bool, resident: &mut std::collections::HashMap<u64, u64>) {
        for t in self.layers.retained.drain(..) {
            if let Some((node, _)) = t.cache { resident.remove(&node); }
        }
        self.layers.trim_spare(if all { 0 } else { RESTING_SPARE_BYTES });
        if all {
            for slot in &mut self.layers.targets {
                if slot.as_ref().is_some_and(|t| t.cache.is_none()) { *slot = None; }
            }
            self.backdrops.clear();
            if let Some(blur) = &mut self.backdrop_blur { blur.give_back(); }
        }
        #[cfg(all(feature = "alloc-sites", not(target_arch = "wasm32")))]
        self.report_pools();
    }

    /// Frees the staging chunks kept for reuse (`window::rest`, out of sight only: the next
    /// frame's uploads would make new ones).
    pub(crate) fn trim_uploads(&self) {
        self.uploads.trim();
    }

    pub(super) fn sync_resident_caches(&self, out: &mut std::collections::HashMap<u64, u64>) {
        if !out.is_empty() {
            out.clear();
        }
        for t in self.layers.targets.iter().flatten().chain(self.layers.retained.iter()) {
            if let Some((node, sig)) = t.cache {
                out.insert(node, sig);
            }
        }
    }

    pub fn layer_stats(&self) -> LayerStats {
        let mut stats = LayerStats { allocations: self.layers.allocations, ..Default::default() };
        for (index, t) in self.layers.targets.iter().enumerate() {
            let Some(t) = t else { continue };
            stats.targets += 1;
            stats.bytes += Self::target_bytes(t);
            if matches!(self.layers.planner.dispositions.get(index), Some(Disposition::Render { .. })) {
                stats.extra_passes += 1 + t.blur.as_ref().map_or(0, layer_blur::Scratch::passes);
            }
        }
        for t in &self.layers.retained {
            stats.targets += 1;
            stats.bytes += Self::target_bytes(t);
        }
        stats
    }

    pub(super) fn prepare_layers(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, instances: &[Instance], viewport: (u32, u32)) {
        let staging = self.uploads.staging(device, queue);
        self.layers.viewport = viewport;
        #[cfg(test)]
        { self.layers.peak = 0; }
        let before = self.layers.plan.len();
        for slot in &mut self.layers.targets {
            if slot.as_ref().is_some_and(|t| t.cache.is_some()) {
                self.layers.retained.push(slot.take().unwrap());
            }
        }
        self.layers.plan.clear();
        self.layers.stack.clear();
        for (index, i) in instances.iter().enumerate() {
            if i.params[1] == KIND_LAYER {
                let parent = self.layers.stack.last().copied();
                let parent_cached_hit = parent.is_some_and(|p| {
                    self.layers.plan[p].cache.is_some_and(|key| {
                        self.layers.retained.iter().any(|t| t.cache == Some(key))
                    })
                });
                let opacity = i.fill[3].clamp(0.0, 1.0);
                let planned = match parent.map(|p| self.layers.plan[p].planned) {
                    _ if opacity == 0.0 => Planned::Hidden,
                    Some(Planned::Hidden) => Planned::Hidden,
                    Some(Planned::Covered) => Planned::Covered,
                    _ if parent_cached_hit => Planned::Covered,
                    _ => Planned::Inline,
                };
                // Bound storage by the ancestor's required input, including filter support.
                // Keep the actual clip separate so rounded corners never move.
                let limit = parent.map_or(NO_CLIP, |p| {
                    let p = &self.layers.plan[p];
                    let grow = (3.0 * p.blur).ceil();
                    [p.clip[0].max(p.limit[0]) - grow, p.clip[1].max(p.limit[1]) - grow,
                        p.clip[2].min(p.limit[2]) + grow, p.clip[3].min(p.limit[3]) + grow]
                });
                let blur = layer_blur::sigma(i.params[2]);
                let cache = (i.params[3] == 1.0).then(|| (unpack_u64_f32(i.radii), unpack_u64_f32(i.uv)));
                let turn = (i.border[1] != 0.0).then_some((i.border[1], [i.border[2], i.border[3]]));
                let parent_free = parent.is_some_and(|p| self.layers.plan[p].free);
                let free = turn.is_some() || parent_free;
                let frame = if free {
                    let outer = match parent {
                        Some(p) if parent_free => self.layers.plan[p].frame,
                        _ => {
                            let support = parent.map_or(0.0, |p| self.layers.plan[p].support);
                            [-support, -support, viewport.0 as f32 + support, viewport.1 as f32 + support]
                        }
                    };
                    let shown = [outer[0].max(i.clip[0]), outer[1].max(i.clip[1]), outer[2].min(i.clip[2]), outer[3].min(i.clip[3])];
                    match turn {
                        Some((angle, pivot)) if shown[2] > shown[0] && shown[3] > shown[1] => turned_bounds(shown, -angle, pivot),
                        _ => shown,
                    }
                } else { [0.0; 4] };
                let limit = if parent_free { NO_CLIP } else { limit };
                self.layers.stack.push(self.layers.plan.len());
                self.layers.plan.push(Layer {
                    begin: index as u32, end: 0, parent,
                    edges: [i.rect[0], i.rect[1], i.rect[0] + i.rect[2], i.rect[1] + i.rect[3]],
                    vis: [0.0; 4],
                    clip: i.clip,
                    limit,
                    fade: i.fade,
                    opacity,
                    blend: if i.params[0] == 1.0 { Blend::PlusLighter } else { Blend::SourceOver },
                    blur,
                    support: parent.map_or(0.0, |p| self.layers.plan[p].support) + (3.0 * blur).ceil(),
                    cache,
                    planned,
                    turn,
                    free,
                    frame,
                    // A cached scope's `radii` name its node instead.
                    hole: if cache.is_some() { [0.0; 4] } else { i.radii },
                });
            } else if i.params[1] == KIND_LAYER_END {
                let at = self.layers.stack.pop().expect("unbalanced layer end");
                let l = &mut self.layers.plan[at];
                l.end = index as u32;
                let grow = (3.0 * l.blur).ceil();
                // Retain off-viewport sources that can contribute through this or an ancestor's
                // filter. Clipping to the destination viewport before filtering loses those taps.
                let e = l.edges;
                let vis = if l.free {
                    [(e[0] - grow).max(l.frame[0] - grow).floor(), (e[1] - grow).max(l.frame[1] - grow).floor(),
                        (e[2] + grow).min(l.frame[2] + grow).ceil(), (e[3] + grow).min(l.frame[3] + grow).ceil()]
                } else {
                    [(e[0] - grow).max(l.clip[0].max(l.limit[0]) - grow).max(-l.support).floor(),
                        (e[1] - grow).max(l.clip[1].max(l.limit[1]) - grow).max(-l.support).floor(),
                        (e[2] + grow).min(l.clip[2].min(l.limit[2]) + grow).min(viewport.0 as f32 + l.support).ceil(),
                        (e[3] + grow).min(l.clip[3].min(l.limit[3]) + grow).min(viewport.1 as f32 + l.support).ceil()]
                };
                l.vis = vis;
                if l.cache.is_some() {
                    if vis[2] <= vis[0] || vis[3] <= vis[1] {
                        l.planned = Planned::Hidden;
                    }
                    l.edges = [e[0].floor(), e[1].floor(), e[2].ceil(), e[3].ceil()];
                } else {
                    l.edges = vis;
                }
                let edges = match l.turn {
                    Some((angle, pivot)) if l.edges[2] > l.edges[0] && l.edges[3] > l.edges[1] => turned_bounds(l.edges, angle, pivot),
                    _ => l.edges,
                };
                if let Some(parent) = l.parent.filter(|&p| self.layers.plan[p].cache.is_none()) {
                    union(&mut self.layers.plan[parent].edges, edges);
                }
            } else if let Some(&at) = self.layers.stack.last() {
                if self.layers.plan[at].cache.is_some() { continue; }
                let mut rect = i.rect;
                if i.params[1] == KIND_GLYPH && i.params[0] != 0.0 {
                    let (sin, cos) = i.params[0].sin_cos();
                    let width = cos.abs() * rect[2] + sin.abs() * rect[3];
                    let height = sin.abs() * rect[2] + cos.abs() * rect[3];
                    rect = [rect[0] + (rect[2] - width) * 0.5, rect[1] + (rect[3] - height) * 0.5, width, height];
                }
                let grow = match i.params[1] {
                    KIND_GLYPH => 0.0,
                    KIND_SHADOW => i.params[2] * 3.0 + 1.0,
                    KIND_EFFECT => i.params[2] + 1.0,
                    KIND_RECT | KIND_GRADIENT | KIND_DASHED | KIND_RADIAL => 1.0 + i.params[2] * 2.0,
                    _ => 1.0,
                };
                union(&mut self.layers.plan[at].edges, [
                    (rect[0] - grow).max(i.clip[0]), (rect[1] - grow).max(i.clip[1]),
                    (rect[0] + rect[2] + grow).min(i.clip[2]),
                    (rect[1] + rect[3] + grow).min(i.clip[3]),
                ]);
            }
        }
        assert!(self.layers.stack.is_empty(), "unclosed layer scope");
        let isolate = self.check_layer_features(device);
        while self.layers.targets.len() > self.layers.plan.len() {
            if let Some(t) = self.layers.targets.pop().flatten() { self.layers.park(t); }
        }
        self.layers.targets.resize_with(self.layers.plan.len(), || None);
        if !self.layers.plan.is_empty() || !self.layers.retained.is_empty() {
            self.layers.frame_stamp += 1;
        }
        let max_dim = device.limits().max_texture_dimension_2d.min(MAX_CACHE_DIM);
        if self.layers.plan.is_empty() {
            // No layers to plan: what closed at the last frame's end is all this frame keeps or
            // drops, exactly as dev trimmed the pool. Nothing here reads a target's size.
            self.layers.trim_retained(0);
        } else {
            // One disposition per layer, and every change to the target pools it needs, decided on
            // the pools as they are: `commit` then carries the moves out and `paint_layers` draws them.
            let layers = &mut self.layers;
            let pools = Pools { slots: &layers.targets, retained: &layers.retained, spare: &layers.spare };
            layers.planner.decide(&layers.plan, &pools, layers.cache_budget, isolate, max_dim);
            self.commit(device);
            self.paint_layers(device, &staging);
        }
        // A frame whose layers fell gives back what a burst grew the plan's scratch to; a frame
        // that keeps its layers, or stays empty, leaves it alone.
        if self.layers.plan.len() < before {
            let layers = &mut self.layers;
            layers.planner.settle(layers.plan.len(), layers.retained.len(), layers.spare.len());
        }
        // Rebind child composites to their nearest materialized parent (opaque scopes flatten).
        for index in 0..self.layers.plan.len() {
            if self.layers.targets[index].is_none() { continue; }
            let parent = self.layer_target(self.layers.plan[index].parent);
            let t = self.layers.targets[index].as_ref().unwrap();
            let parent_identity = parent.map(|p| self.layers.targets[p].as_ref().unwrap().identity);
            if t.composite_parent == parent_identity { continue; }
            let composite = Self::bind_group(device, &self.bind_layout, &self.globals, self.uber.outlines(), &self.atlas, &t.texture,
                parent.map(|p| &self.layers.targets[p].as_ref().unwrap().projection));
            let t = self.layers.targets[index].as_mut().unwrap();
            t.composite = composite;
            t.composite_parent = parent_identity;
        }
        if self.layers.targets.iter().any(Option::is_some) {
            self.add_format(device, FORMAT);
            self.add_layer_format(device, self.format);
            for index in 0..self.extra.len() { self.add_layer_format(device, self.extra[index].0); }
        }
        if self.layers.plan.iter().any(|l| l.blend == Blend::PlusLighter) && let Some(s) = &mut self.surfaces {
            s.add_plus_lighter(device, FORMAT);
        }
        debug_assert!(self.layers.planner.dispositions.iter().zip(&self.layers.targets)
            .all(|(d, t)| t.is_some() == matches!(d, Disposition::Render { .. } | Disposition::Cached { .. })),
            "only Render and Cached layers hold a target");
        self.refresh_layer_bindings(device);
        if before > 0 || !self.layers.plan.is_empty() { self.position_layer_surfaces(queue); }
    }

    /// Carries out the moves the plan decided, in its order: the one place a target changes pool.
    fn commit(&mut self, device: &wgpu::Device) {
        for i in 0..self.layers.planner.moves.len() {
            match self.layers.planner.moves[i] {
                Move::Park(slot) => {
                    // The plan's `Drop`s trim the spares; nothing trims here.
                    if let Some(mut t) = self.layers.targets[slot].take() { t.cache = None; self.layers.spare.push(t); }
                }
                Move::Set { slot, from } => {
                    let mut t = match from {
                        Kept::Retained(at) => self.layers.retained.swap_remove(at),
                        Kept::Spare(at) => self.layers.spare.swap_remove(at),
                    };
                    // A target parked before the atlas or outline texture changed binds it now,
                    // once; a spare no layer takes again never makes a bind group at all.
                    self.rebind_if_stale(device, &mut t);
                    self.layers.targets[slot] = Some(t);
                }
                Move::Create { slot, size } => self.layers.targets[slot] = Some(self.make_target(device, size)),
                Move::Unblur(slot) => {
                    if let Some(t) = self.layers.targets[slot].as_mut() { t.blur = None; }
                }
                Move::Drop(Kept::Retained(at)) => { self.layers.retained.swap_remove(at); }
                Move::Drop(Kept::Spare(at)) => { self.layers.spare.swap_remove(at); }
            }
            #[cfg(test)]
            self.layers.watch_peak();
        }
    }

    /// A target of `size`, with the buffers and bind groups a layer draws into it through.
    fn make_target(&mut self, device: &wgpu::Device, size: [u32; 2]) -> Target {
        let (width, height) = (size[0], size[1]);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("isolated group"), size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: FORMAT, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let projection = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("layer projection"), size: 16, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let base = Self::bind_group(device, &self.bind_layout, &self.globals, self.uber.outlines(), &self.atlas, &self.atlas, Some(&projection));
        let composite = Self::bind_group(device, &self.bind_layout, &self.globals, self.uber.outlines(), &self.atlas, &texture, None);
        self.layers.allocations += 1;
        Target { texture, view, projection,
            identity: self.layers.allocations, composite_parent: None,
            quad: Self::instance_buffer(device, 1), base, composite, images: Vec::new(), glyphs: Vec::new(), image_generation: u64::MAX, blur: None,
            cache: None, last_used: 0, content_size: [0.0; 2], binding: self.layers.bindings,
            #[cfg(test)]
            outline_rows: self.uber.outlines().map_or(0, wgpu::Texture::height) }
    }

    /// Draws what each layer's disposition says: its target's descriptor, its blur, and the quad
    /// that composites it.
    fn paint_layers(&mut self, device: &wgpu::Device, staging: &rune_staging::Staging) {
        let stamp = self.layers.frame_stamp;
        for index in 0..self.layers.plan.len() {
            let l = self.layers.plan[index];
            match self.layers.planner.dispositions[index] {
                Disposition::Skip | Disposition::Inline => {}
                Disposition::Cached { rect, cache } => {
                    let t = self.layers.targets[index].as_mut().expect("a claimed cache has its target");
                    t.cache = cache;
                    if cache.is_some() { t.last_used = stamp; }
                    let mut quad = Instance::begin_layer(rect, l.clip, LayerSpec {
                        opacity: l.opacity, blend: l.blend, ..Default::default() });
                    // A hit composites the same turned quad a miss drew: the turn belongs to the
                    // scope, not to the pass that filled its texture.
                    if let Some((angle, pivot)) = l.turn { quad = quad.turned(angle, pivot); }
                    quad.fade = l.fade;
                    if (t.content_size[0] - rect[2]).abs() > 0.5 || (t.content_size[1] - rect[3]).abs() > 0.5 {
                        quad.uv = [0.0, 0.0, t.content_size[0], t.content_size[1]];
                    }
                    staging.write_buffer(&t.quad, 0, bytemuck::bytes_of(&quad));
                }
                Disposition::Render { rect, cache } => {
                    let t = self.layers.targets[index].as_mut().expect("a rendering layer has its target");
                    t.cache = cache;
                    t.last_used = stamp;
                    t.content_size = [rect[2], rect[3]];
                    // A scratch the layer no longer wants was dropped by `Move::Unblur`, before any
                    // later layer's texture existed.
                    if l.blur > 0.0 {
                        let pipeline = self.layers.blur.as_ref().expect("prepared layer blur");
                        if t.blur.is_none() {
                            t.blur = Some(pipeline.scratch(device, &t.texture));
                            self.layers.allocations += 1;
                        }
                        if t.blur.as_mut().unwrap().prepare(device, pipeline, staging,
                            [rect[2], rect[3]], l.blur, l.blend == Blend::PlusLighter) {
                            self.layers.allocations += 1;
                        }
                    }
                    staging.write_buffer(&t.projection, 0, bytemuck::cast_slice(&rect));
                    let mut quad = Instance::begin_layer(rect, l.clip, LayerSpec {
                        opacity: l.opacity, blend: l.blend, ..Default::default() });
                    if let Some((angle, pivot)) = l.turn { quad = quad.turned(angle, pivot); }
                    quad.fade = l.fade;
                    quad.radii = l.hole;
                    staging.write_buffer(&t.quad, 0, bytemuck::bytes_of(&quad));
                }
            }
            #[cfg(test)]
            self.layers.watch_peak();
        }
    }

    /// Builds and checks the pipelines this frame's layers need (`breaker`), and plans the frame
    /// without the ones the device refused or hasn't taken yet: a dissolve as source-over, a blur
    /// unblurred. Whether layers can isolate; when they can't, each group draws in its parent.
    fn check_layer_features(&mut self, device: &wgpu::Device) -> bool {
        // A layer a cache covers counts: the cache can still miss and draw it this frame.
        let shown = || self.layers.plan.iter().enumerate().filter(|(_, l)| l.planned != Planned::Hidden);
        let isolation = shown().any(|(index, _)| self.isolates(index));
        let plus_lighter = shown().any(|(_, l)| l.blend == Blend::PlusLighter);
        let blur = shown().any(|(_, l)| l.blur > 0.0);
        if !isolation && !plus_lighter && !blur && !self.breakers.checking() { return false; }
        self.check_features(device, |feature| match feature {
            breaker::Feature::Isolation => isolation,
            breaker::Feature::PlusLighter => plus_lighter,
            breaker::Feature::LayerBlur => blur,
        });
        let working = |feature| self.breakers.health(feature).working();
        let (isolate, add, blur) = (working(breaker::Feature::Isolation), working(breaker::Feature::PlusLighter), working(breaker::Feature::LayerBlur));
        for l in &mut self.layers.plan {
            if !add { l.blend = Blend::SourceOver; }
            if !blur { l.blur = 0.0; }
        }
        isolate
    }

    /// Whether layer `index` needs a target of its own to draw as asked, while planning.
    fn isolates(&self, index: usize) -> bool {
        let l = &self.layers.plan[index];
        let additive_parent = l.parent.is_some_and(|p| self.layers.plan[p].blend == Blend::PlusLighter);
        l.cache.is_some() || l.opacity != 1.0 || l.blend != Blend::SourceOver || additive_parent
            || l.blur != 0.0 || l.fade[3] != 0 || l.clip != NO_CLIP || l.turn.is_some() || l.hole[3] != 0.0
    }

    /// Drops the composite pipelines (`breaker`), and with `targets` every target, which no
    /// frame will draw into again.
    pub(super) fn drop_layer_pipelines(&mut self, targets: bool) {
        self.layers.pipelines.clear();
        if targets {
            self.layers.targets.clear();
            self.layers.retained.clear();
            self.layers.spare.clear();
        }
    }

    /// Drops the layer blur's pipelines and every target's blur scratch (`breaker`).
    pub(super) fn drop_layer_blur(&mut self) {
        self.layers.blur = None;
        for t in self.layers.targets.iter_mut().flatten().chain(&mut self.layers.retained).chain(&mut self.layers.spare) { t.blur = None; }
    }

    pub(super) fn add_layer_format(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        if !self.layers.pipelines.iter().any(|(f, _)| *f == format) {
            self.layers.pipelines.push((format, Self::pipeline(device, &self.layout, &self.shader, format, "vs", Blend::SourceOver)));
        }
    }

    fn layer_target(&self, mut layer: Option<usize>) -> Option<usize> {
        while let Some(index) = layer {
            if self.layers.targets[index].is_some() { return Some(index); }
            layer = self.layers.plan[index].parent;
        }
        None
    }

    pub(super) fn layer_blend(&self, target: Option<usize>) -> Blend {
        target.map_or(Blend::SourceOver, |p| self.layers.plan[p].blend)
    }

    pub(super) fn images_changed(&mut self) { self.layers.image_generation += 1; }

    /// Makes every live target's bind groups again (a new atlas or outline texture), and starts a
    /// generation: the slots and retained caches bind it here, a parked target binds it when a
    /// layer takes it back, and a spare no layer takes again makes nothing.
    pub(super) fn rebind_layers(&mut self, device: &wgpu::Device) {
        self.layers.bindings += 1;
        let generation = self.layers.bindings;
        let (layout, globals, outlines, atlas) = (&self.bind_layout, &self.globals, self.uber.outlines(), &self.atlas);
        for t in self.layers.targets.iter_mut().flatten().chain(&mut self.layers.retained) {
            rebind_target(device, layout, globals, outlines, atlas, t, generation);
            #[cfg(test)]
            { self.layers.binds += 1; }
        }
    }

    /// Binds `t` to the atlas and outline texture this frame draws with, if it was made or parked
    /// against an earlier generation.
    fn rebind_if_stale(&mut self, device: &wgpu::Device, t: &mut Target) {
        let generation = self.layers.bindings;
        if t.binding == generation { return; }
        rebind_target(device, &self.bind_layout, &self.globals, self.uber.outlines(), &self.atlas, t, generation);
        #[cfg(test)]
        { self.layers.binds += 1; }
    }

    pub(super) fn refresh_layer_bindings(&mut self, device: &wgpu::Device) {
        for t in self.layers.targets.iter_mut().flatten() {
            if t.image_generation == self.layers.image_generation { continue; }
            t.images.clear();
            t.images.extend(self.images.iter().map(|page| page.as_ref().map(|(texture, _)|
                Self::bind_group(device, &self.bind_layout, &self.globals, self.uber.outlines(), &self.atlas, texture, Some(&t.projection)))));
            t.glyphs.clear();
            t.glyphs.extend(self.atlas_pages.iter().map(|(texture, _)|
                Self::bind_group(device, &self.bind_layout, &self.globals, self.uber.outlines(), texture, texture, Some(&t.projection))));
            t.image_generation = self.layers.image_generation;
        }
    }

    pub(super) fn position_layer_surfaces(&self, queue: &wgpu::Queue) {
        let Some(s) = &self.surfaces else { return };
        for index in 0..s.len() {
            let before = s.before_instance(index);
            let layer = self.layers.plan.iter().rposition(|l| before > l.begin && before <= l.end);
            let bounds = self.layer_target(layer).map_or(
                [0.0, 0.0, self.layers.viewport.0 as f32, self.layers.viewport.1 as f32],
                |p| self.layers.planner.dispositions.get(p).map_or([0.0; 4], |d| d.rect()));
            // prepare_surfaces may precede prepare on the first frame.
            if bounds[2] > 0.0 && bounds[3] > 0.0 { s.target(queue, index, bounds); }
        }
    }

    /// Encode isolated scopes before opening a compositor render pass that calls `draw_range`.
    /// `draw`/`draw_timed` do this automatically. Contents are redrawn each presentation so GPU
    /// springs and newly latched native surfaces stay live; textures, not content, are pooled.
    pub fn encode_layers(&self, encoder: &mut wgpu::CommandEncoder) {
        self.encode_layers_timed(encoder, None, &mut |_, _| {});
    }

    /// `encode_layers` that calls `hook` at each effect slot inside an isolated layer, in painter
    /// order within that layer's pass. The layer's opacity, blur, dissolve and clip then apply to
    /// whatever the hook drew when the layer composites. Slots outside any layer are reached by
    /// `draw_range_with` or `draw_with`.
    pub fn encode_layers_with(&self, encoder: &mut wgpu::CommandEncoder, hook: &mut EffectHook<'_>) {
        self.encode_layers_timed(encoder, None, &mut |phase, site| {
            if let EffectPhase::Draw(pass) = phase { hook(pass, site); }
        });
    }

    /// `encode_layers_with` that also calls `hook(EffectPhase::Backdrop(encoder), site)` outside
    /// the render pass at each backdrop slot (`El::effect_backdrop(true)`) inside an isolated
    /// layer, right after copying the layer's target up to that slot into `EffectSite::backdrop`
    /// and before re-opening the layer's pass for `EffectPhase::Draw(pass)`.
    pub fn encode_layers_with_backdrop(&self, encoder: &mut wgpu::CommandEncoder, hook: &mut EffectBackdropHook<'_>) {
        self.encode_layers_timed(encoder, None, hook);
    }

    /// Origin and size of a pass's target: a layer's bounds, or the frame viewport.
    pub(super) fn target_frame(&self, target: Option<usize>) -> ([f32; 2], [u32; 2]) {
        match target {
            Some(index) => {
                let b = self.layers.planner.dispositions.get(index).map_or([0.0; 4], |d| d.rect());
                ([b[0], b[1]], [b[2] as u32, b[3] as u32])
            }
            None => ([0.0, 0.0], [self.layers.viewport.0, self.layers.viewport.1]),
        }
    }

    pub(super) fn encode_layers_timed(&self, encoder: &mut wgpu::CommandEncoder, timestamps: Option<&wgpu::RenderPassTimestampWrites<'_>>, hook: &mut EffectBackdropHook<'_>) {
        self.flush_uploads(encoder);
        let mut first = true;
        for (index, l) in self.layers.plan.iter().enumerate().rev() {
            let disposition = self.layers.planner.dispositions.get(index).copied().unwrap_or(Disposition::Skip);
            if !matches!(disposition, Disposition::Render { .. }) { continue; }
            let Some(target) = &self.layers.targets[index] else { continue };
            let rect = disposition.rect();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("isolated group"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view, depth_slice: None, resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })], depth_stencil_attachment: None,
                timestamp_writes: if first { timestamps.map(|t| wgpu::RenderPassTimestampWrites {
                    query_set: t.query_set, beginning_of_pass_write_index: t.beginning_of_pass_write_index,
                    end_of_pass_write_index: None,
                }) } else { None },
                occlusion_query_set: None, multiview_mask: None,
            }).forget_lifetime();
            first = false;
            pass.set_viewport(0.0, 0.0, rect[2], rect[3], 0.0, 1.0);
            let mut active = ActivePass::owned(pass, encoder, &target.view);
            self.draw_layer_range(&mut active, FORMAT, l.begin + 1..l.end, Some(index), hook);
            drop(active);
            if let Some(blur) = &target.blur {
                self.layers.blur.as_ref().unwrap().encode(encoder, blur, &target.view);
            }
        }
    }

    pub(super) fn draw_layer_range(&self, pass: &mut ActivePass<'_, '_>, format: wgpu::TextureFormat, range: Range<u32>, target: Option<usize>, hook: &mut EffectBackdropHook<'_>) {
        let mut cursor = range.start;
        for (index, l) in self.layers.plan.iter().enumerate() {
            if l.begin < cursor || l.begin >= range.end { continue; }
            assert!(l.end < range.end, "draw_range must not split a layer scope");
            self.draw_plain(pass, format, cursor..l.begin, target, true, hook);
            let disposition = self.layers.planner.dispositions.get(index).copied().unwrap_or(Disposition::Skip);
            if let (Disposition::Render { .. } | Disposition::Cached { .. }, Some(t)) = (disposition, &self.layers.targets[index]) {
                let pipeline = if self.layer_blend(target) == Blend::PlusLighter {
                    &self.layers.additive.as_ref().expect("prepared dissolve").1
                } else { &self.layers.pipelines.iter().find(|(f, _)| *f == format).expect("Renderer::add_format first").1 };
                pass.set_pipeline(pipeline);
                if let Some(properties) = &self.springs { pass.set_bind_group(1, &properties.group, &[]); }
                pass.set_bind_group(0, &t.composite, &[]);
                pass.set_vertex_buffer(0, t.quad.slice(..));
                pass.draw(0..4, 0..1);
            } else if disposition == Disposition::Inline {
                // An opaque group that needs no target, or any group while layers can't isolate
                // (`breaker`).
                if l.clip != NO_CLIP {
                    let (origin, size) = self.target_frame(target);
                    let edge = |v: f32, o: f32, max: u32| (v - o).clamp(0.0, max as f32) as u32;
                    let (mut x0, mut y0) = (edge(l.clip[0].floor(), origin[0], size[0]), edge(l.clip[1].floor(), origin[1], size[1]));
                    let (mut x1, mut y1) = (edge(l.clip[2].ceil(), origin[0], size[0]), edge(l.clip[3].ceil(), origin[1], size[1]));
                    // Inside an enclosing inline clip: both apply.
                    let outer = pass.scissor();
                    if let Some([ox, oy, ow, oh]) = outer {
                        (x0, y0, x1, y1) = (x0.max(ox), y0.max(oy), x1.min(ox + ow), y1.min(oy + oh));
                    }
                    if x1 > x0 && y1 > y0 {
                        pass.set_scissor(Some([x0, y0, x1 - x0, y1 - y0]), size);
                        self.draw_layer_range(pass, format, l.begin + 1..l.end, target, hook);
                        pass.set_scissor(outer, size);
                    }
                } else {
                    self.draw_layer_range(pass, format, l.begin + 1..l.end, target, hook);
                }
            }
            cursor = l.end + 1;
        }
        let include_end = range.end == self.count || self.layers.plan.iter().any(|l| l.end == range.end);
        self.draw_plain(pass, format, cursor..range.end, target, include_end, hook);
    }

    pub(super) fn validate_layer_range(&self, range: &Range<u32>) {
        for l in &self.layers.plan {
            assert!(!(l.begin < range.start && range.start <= l.end)
                && !(l.begin < range.end && range.end <= l.end), "draw_range must not split a layer scope");
        }
    }

    fn draw_plain(&self, pass: &mut ActivePass<'_, '_>, format: wgpu::TextureFormat, range: Range<u32>, target: Option<usize>, include_end: bool, hook: &mut EffectBackdropHook<'_>) {
        let mut cursor = range.start;
        if let Some(s) = &self.surfaces {
            for index in 0..s.len() {
                let before = s.before_instance(index);
                if before < range.start || before > range.end || (before == range.end && !include_end) { continue; }
                self.draw_with_effects(pass, format, cursor..before, target, hook);
                // Only the window's own pass can punch a video's place out for its layer.
                match target {
                    None => s.draw_main(pass, index, format, self.layer_blend(target)),
                    Some(_) => s.draw_isolated(pass, index, format, self.layer_blend(target)),
                }
                cursor = before;
            }
        }
        self.draw_with_effects(pass, format, cursor..range.end, target, hook);
    }
}
