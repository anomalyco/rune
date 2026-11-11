//! The layer plan: the record the planning walk leaves for each layer, and the one decision that
//! turns it into what the layer does this frame. Deciding on plain data means everything after it
//! only carries the plan out: no layer's verdict and no target's place in a pool changes under a
//! later pass, which is what a rewrite mid-prepare used to do.
use super::*;

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;

/// A layer's scope, as the planning walk recorded it.
#[derive(Clone, Copy, Debug)]
pub(super) struct Layer {
    pub(super) begin: u32,
    pub(super) end: u32,
    pub(super) parent: Option<usize>,
    /// The edges of what it draws, `[x0, y0, x1, y1]`, while planning.
    pub(super) edges: [f32; 4],
    /// The part of `edges` that can show, which a cache that doesn't fit falls back to.
    pub(super) vis: [f32; 4],
    pub(super) clip: [f32; 4],
    pub(super) limit: [f32; 4], // ancestor output needed by their filters; never a replacement rounded clip
    pub(super) fade: [u32; 4],
    pub(super) opacity: f32,
    pub(super) blend: Blend,
    pub(super) blur: f32,
    pub(super) support: f32,
    /// The node and signature `El::cache` asked to keep it under.
    pub(super) cache: Option<(u64, u64)>,
    pub(super) planned: Planned,
    /// `Instance::turned`: the composite turns `angle` radians about `pivot`.
    pub(super) turn: Option<(f32, [f32; 2])>,
    /// Turned, or inside a turned layer: the window, clips and filter limits above don't line
    /// up with this layer's pixels, so `frame` bounds it instead.
    pub(super) free: bool,
    /// For a free layer, the edges of its contents that can reach the window.
    pub(super) frame: [f32; 4],
    /// The round hole its composite cuts (`El::cutout_circle`): centre, then the radii it's clear
    /// within and whole from, in window device pixels; zeros for none.
    pub(super) hole: [f32; 4],
}

/// What the planning walk knew when it reached a layer, before the frame settles it into a
/// [`Disposition`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Planned {
    /// Nothing to draw: transparent, empty, or inside a transparent layer.
    Hidden,
    /// Inside a cache that composites from its texture, so not drawn on its own.
    Covered,
    /// Drawn straight into its parent's pass, unless a cache covers it.
    Inline,
}

/// What a layer does this frame, and the one place it draws into: decided once, by
/// [`Layers::decide`], so nothing that follows can change its mind.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Disposition {
    /// Nothing to draw: transparent, empty, or inside a cache that composites from its texture.
    Skip,
    /// Drawn straight into its parent's pass.
    Inline,
    /// Drawn into a target of its own, then composited. The target keeps `cache` for the next frame.
    Render { rect: [f32; 4], cache: Option<(u64, u64)> },
    /// Composited from the texture its cache kept, without drawing into it. `cache` is `None` when
    /// the budget only let this frame borrow the texture.
    Cached { rect: [f32; 4], cache: Option<(u64, u64)> },
}

impl Disposition {
    /// The place of the layer's target in the window, for the two that have one.
    pub(super) fn rect(self) -> [f32; 4] {
        match self {
            Self::Render { rect, .. } | Self::Cached { rect, .. } => rect,
            Self::Skip | Self::Inline => [0.0; 4],
        }
    }
}

/// A target as the plan sees it: its size, what it holds and what it costs, with no handle, so
/// [`Layers::decide`] needs no device and a walk can drive it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Held {
    pub(super) size: [u32; 2],
    pub(super) bytes: u64,
    pub(super) cache: Option<(u64, u64)>,
    pub(super) last_used: u64,
    pub(super) content: [f32; 2],
}

impl Held {
    /// What a target of `size` costs, and holds, before anything draws into it.
    fn new(size: [u32; 2], bpp: u64) -> Self {
        Self { size, bytes: u64::from(size[0]) * u64::from(size[1]) * bpp, cache: None, last_used: 0, content: [0.0; 2] }
    }
}

/// Which pool a kept target is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kept {
    Retained(usize),
    Spare(usize),
}

/// One change to the target pools, in the order the plan decided it. `Renderer::commit` is the
/// only place a target changes pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Move {
    /// Parks a slot's target for a later layer of its size.
    Park(usize),
    /// Puts a kept target in a slot, dropping whatever it held.
    Set { slot: usize, from: Kept },
    /// Makes a target of `size` for a slot.
    Create { slot: usize, size: [u32; 2] },
    /// Drops a slot target's blur scratch: the layer draws unblurred from now on, and the scratch
    /// must go before a later layer makes a texture of its own (dev dropped it in the same place).
    Unblur(usize),
    /// Drops a kept target.
    Drop(Kept),
}

/// A layer's bounds, from the edges of what it draws.
pub(super) fn rect_of(edges: [f32; 4]) -> [f32; 4] {
    [edges[0], edges[1], (edges[2] - edges[0]).max(0.0), (edges[3] - edges[1]).max(0.0)]
}

/// A target's texture size for `bounds`: whole 64-pixel buckets, beside the size it was asked for.
fn bucket_of(bounds: [f32; 4]) -> ([u32; 2], u32, u32) {
    let (width, height) = (bounds[2] as u32, bounds[3] as u32);
    ([width.div_ceil(64) * 64, height.div_ceil(64) * 64], width, height)
}

impl Layers {
    /// Decides what every layer does this frame, and every change to the target pools it takes,
    /// reading the [`Held`]s [`Layers::gather`] copied. It reads plain data and allocates nothing,
    /// so a walk of frames drives it without a device.
    pub(super) fn decide(&mut self, isolate: bool, max_dim: u32) {
        let bpp = u64::from(FORMAT.block_copy_size(None).unwrap());
        self.dispositions.clear();
        self.dispositions.resize(self.plan.len(), Disposition::Skip);
        self.slots.resize(self.plan.len(), None);
        self.moves.clear();
        let mut active = 0u64;
        // First claim resident cache hits, so caches already on screen never thrash under a tight
        // budget.
        for index in 0..self.plan.len() {
            let layer = self.plan[index];
            let rect = rect_of(layer.edges);
            if rect[2] == 0.0 || rect[3] == 0.0 || !isolate || layer.planned != Planned::Inline { continue; }
            let Some(cache) = layer.cache else { continue };
            let Some(at) = self.kept.iter().position(|t| t.cache == Some(cache)) else { continue };
            let bytes = self.kept[at].bytes;
            let over = active + bytes > self.cache_budget;
            // A cache with contents keeps its place under the budget: it can't be made again cheaply.
            if over && layer.begin + 1 < layer.end { continue; }
            if !over { active += bytes; }
            self.dispositions[index] = Disposition::Cached { rect, cache: (!over).then_some(cache) };
            self.slots[index] = Some(self.kept.swap_remove(at));
            self.moves.push(Move::Set { slot: index, from: Kept::Retained(at) });
        }
        for index in 0..self.plan.len() {
            if matches!(self.dispositions[index], Disposition::Cached { .. }) { continue; }
            let layer = self.plan[index];
            let rect = rect_of(layer.edges);
            let parent = layer.parent.map(|p| self.dispositions[p]);
            // A layer inside a cache that composites from its texture, or inside a transparent one,
            // draws nothing: what a cache kept comes from its texture.
            if layer.planned == Planned::Hidden || rect[2] == 0.0 || rect[3] == 0.0
                || matches!(parent, Some(Disposition::Skip | Disposition::Cached { .. })) {
                self.dispositions[index] = Disposition::Skip;
                self.park_slot(index);
                continue;
            }
            if !isolate {
                // Without isolation (`breaker`), every group draws straight into its parent.
                self.dispositions[index] = Disposition::Inline;
                self.park_slot(index);
                continue;
            }
            let mut bounds = rect;
            let (mut bucket, mut width, mut height) = bucket_of(bounds);
            let mut cache = None;
            if let Some((node, sig)) = layer.cache {
                let previous = self.kept.iter().position(|t| t.cache.is_some_and(|(n, _)| n == node));
                let needed = u64::from(bucket[0]) * u64::from(bucket[1]) * bpp;
                if bucket[0] <= max_dim && bucket[1] <= max_dim && active + needed <= self.cache_budget {
                    active += needed;
                    cache = Some((node, sig));
                    match previous {
                        // Take the target this node kept: the new bounds may only be smaller.
                        Some(at) => {
                            self.park_slot(index);
                            self.slots[index] = Some(self.kept.swap_remove(at));
                            self.moves.push(Move::Set { slot: index, from: Kept::Retained(at) });
                        }
                        None => self.trim(active),
                    }
                } else {
                    if let Some(at) = previous {
                        self.kept.swap_remove(at);
                        self.moves.push(Move::Drop(Kept::Retained(at)));
                    }
                    // Drawn like an uncached group, it only needs the part that shows.
                    bounds = rect_of(layer.vis);
                    (bucket, width, height) = bucket_of(bounds);
                }
            }
            let additive_parent = layer.parent.is_some_and(|p| self.plan[p].blend == Blend::PlusLighter);
            if cache.is_none() && layer.opacity == 1.0 && layer.blend == Blend::SourceOver && !additive_parent
                && layer.blur == 0.0 && layer.fade[3] == 0 && layer.clip == NO_CLIP
                && layer.turn.is_none() && layer.hole[3] == 0.0 {
                self.dispositions[index] = Disposition::Inline;
                self.park_slot(index);
                continue;
            }
            self.dispositions[index] = Disposition::Render { rect: bounds, cache };
            let fits = self.slots[index].is_some_and(|t| {
                let size = t.size;
                // The minimum bucket is unavoidable for tiny controls. Do not recreate that
                // same bucket every frame when rounding alone exceeds the 2x area limit.
                size[0] >= width && size[1] >= height && (size == bucket
                    || u64::from(size[0]) * u64::from(size[1]) <= 2 * u64::from(width) * u64::from(height))
            });
            if !fits {
                self.park_slot(index);
                let spare = self.parked.iter().position(|t| {
                    let size = t.size;
                    size[0] >= width && size[1] >= height
                        && u64::from(size[0]) * u64::from(size[1]) <= 2 * u64::from(width.max(64)) * u64::from(height.max(64))
                });
                match spare {
                    Some(at) => {
                        self.slots[index] = Some(self.parked.swap_remove(at));
                        self.moves.push(Move::Set { slot: index, from: Kept::Spare(at) });
                    }
                    None => {
                        self.slots[index] = Some(Held::new(bucket, bpp));
                        self.moves.push(Move::Create { slot: index, size: bucket });
                    }
                }
            }
            // The scratch a blur left behind goes before the next layer makes its own texture.
            if layer.blur == 0.0 { self.moves.push(Move::Unblur(index)); }
        }
        self.trim(active);
    }

    /// Parks slot `index`'s target for a later layer of its size: what `spare` stops holding once
    /// it passes `SPARE_BYTES`, least recently used first, goes.
    fn park_slot(&mut self, index: usize) {
        let Some(mut held) = self.slots[index].take() else { return };
        held.cache = None;
        self.moves.push(Move::Park(index));
        self.parked.push(held);
        let mut bytes: u64 = self.parked.iter().map(|t| t.bytes).sum();
        while bytes > SPARE_BYTES {
            let oldest = self.parked.iter().enumerate().min_by_key(|(_, t)| t.last_used).unwrap().0;
            bytes -= self.parked.swap_remove(oldest).bytes;
            self.moves.push(Move::Drop(Kept::Spare(oldest)));
        }
    }

    /// Drops the least recently used kept targets until the budget left after this frame's `active`
    /// cache bytes holds them.
    fn trim(&mut self, active: u64) {
        let available = self.cache_budget.saturating_sub(active);
        let mut bytes: u64 = self.kept.iter().map(|t| t.bytes).sum();
        while bytes > available {
            let oldest = self.kept.iter().enumerate().min_by_key(|(_, t)| t.last_used).unwrap().0;
            bytes -= self.kept.swap_remove(oldest).bytes;
            self.moves.push(Move::Drop(Kept::Retained(oldest)));
        }
    }
}
