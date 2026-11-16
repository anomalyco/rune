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
/// [`Planner::decide`], so nothing that follows can change its mind.
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
/// [`Planner::decide`] needs no device and a walk can drive it.
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

/// What a texel of a layer target costs.
fn bpp() -> u64 {
    u64::from(FORMAT.block_copy_size(None).unwrap())
}

/// Where a target the plan has placed is before the frame's moves: in a layer's slot, at a place
/// in a pool, or not yet made. The plan reads its [`Held`] there, in place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Source {
    Slot(usize),
    Retained(usize),
    Spare(usize),
    New([u32; 2]),
}

/// A target that can say what it is to the plan, without a device.
pub(super) trait Describe {
    fn held(&self) -> Held;
}

/// The target pools as the frame starts. The plan reads them where they are and moves nothing.
pub(super) struct Pools<'a, T> {
    pub(super) slots: &'a [Option<T>],
    pub(super) retained: &'a [T],
    pub(super) spare: &'a [T],
}

impl<T: Describe> Pools<'_, T> {
    fn held(&self, source: Source) -> Held {
        match source {
            Source::Slot(index) => self.slots[index].as_ref().expect("the plan places only targets that exist").held(),
            Source::Retained(at) => self.retained[at].held(),
            Source::Spare(at) => self.spare[at].held(),
            Source::New(size) => Held::new(size, bpp()),
        }
    }
}

/// A pool as the plan has rearranged it so far. Until the plan first moves a target in or out, it
/// is the pool itself, read in place; from then on `order` says where each place's target is.
#[derive(Clone)]
pub(super) struct View {
    own: fn(usize) -> Source,
    len: usize,
    moved: bool,
    order: Vec<Source>,
    /// Test-only: every place the plan has read or copied, over every frame.
    #[cfg(test)]
    pub(super) visits: u64,
}

impl View {
    fn new(own: fn(usize) -> Source) -> Self {
        Self { own, len: 0, moved: false, order: Vec::new(), #[cfg(test)] visits: 0 }
    }

    /// Starts a frame on a pool of `len` targets.
    fn reset(&mut self, len: usize) {
        self.len = len;
        self.moved = false;
        self.order.clear();
    }

    /// The target at `place`.
    fn held<T: Describe>(&mut self, place: usize, pools: &Pools<'_, T>) -> Held {
        #[cfg(test)]
        { self.visits += 1; }
        pools.held(self.source(place))
    }

    /// Where the target at `place` is before the frame's moves.
    fn source(&self, place: usize) -> Source {
        if self.moved { self.order[place] } else { (self.own)(place) }
    }

    /// The first place whose target passes `test`.
    fn position<T: Describe>(&mut self, pools: &Pools<'_, T>, test: impl Fn(&Held) -> bool) -> Option<usize> {
        (0..self.len).find(|&place| test(&self.held(place, pools)))
    }

    /// What every target in it costs.
    fn bytes<T: Describe>(&mut self, pools: &Pools<'_, T>) -> u64 {
        (0..self.len).map(|place| self.held(place, pools).bytes).sum()
    }

    /// The place of the least recently used target, the first of equals.
    fn oldest<T: Describe>(&mut self, pools: &Pools<'_, T>) -> usize {
        (0..self.len).min_by_key(|&place| self.held(place, pools).last_used).unwrap()
    }

    /// Takes the target at `place` out, the last one taking its place, as `Vec::swap_remove` does.
    fn swap_remove(&mut self, place: usize) -> Source {
        self.own_order();
        self.len -= 1;
        self.order.swap_remove(place)
    }

    fn push(&mut self, source: Source) {
        self.own_order();
        self.len += 1;
        self.order.push(source);
    }

    /// From the first move on, the plan keeps the pool's order itself.
    fn own_order(&mut self) {
        if self.moved { return; }
        #[cfg(test)]
        { self.visits += self.len as u64; }
        let own = self.own;
        self.order.extend((0..self.len).map(own));
        self.moved = true;
    }
}

/// What the plan's scratch keeps between frames, in entries: an ordinary frame's layers, so a menu
/// that opens and closes allocates nothing.
const WARM: usize = 64;

/// Gives back what a burst grew `scratch` to, beyond twice what `keep` entries need.
fn shrink<T>(scratch: &mut Vec<T>, keep: usize) {
    if scratch.capacity() > 2 * keep { scratch.shrink_to(keep); }
}

/// The plan of a frame's layers, and the scratch it is decided in, kept warm between frames.
#[derive(Clone)]
pub(super) struct Planner {
    /// What each layer does this frame, one per layer: `decide` fills it, and only its two
    /// targetted variants carry the place their target draws.
    pub(super) dispositions: Vec<Disposition>,
    /// The changes to the target pools, in the order `Renderer::commit` carries them out.
    pub(super) moves: Vec<Move>,
    /// Where the target in each layer's slot is before the moves.
    slots: Vec<Option<Source>>,
    pub(super) kept: View,
    pub(super) parked: View,
    /// Test-only: how many frames have given scratch back.
    #[cfg(test)]
    pub(super) settles: u64,
}

impl Default for Planner {
    fn default() -> Self {
        Self {
            dispositions: Vec::new(),
            moves: Vec::new(),
            slots: Vec::new(),
            kept: View::new(Source::Retained),
            parked: View::new(Source::Spare),
            #[cfg(test)]
            settles: 0,
        }
    }
}

impl Planner {
    /// Decides what every layer of `plan` does this frame, and every change to the target pools it
    /// takes. It reads each target where `pools` holds it, and only where dev's passes read one: a
    /// layer whose slot fits reads no spare. It moves nothing and needs no device, so a walk of
    /// frames drives it.
    pub(super) fn decide<T: Describe>(&mut self, plan: &[Layer], pools: &Pools<'_, T>, budget: u64, isolate: bool, max_dim: u32) {
        let bpp = bpp();
        self.dispositions.clear();
        self.dispositions.resize(plan.len(), Disposition::Skip);
        self.slots.clear();
        self.slots.extend(pools.slots.iter().enumerate().map(|(index, t)| t.is_some().then_some(Source::Slot(index))));
        self.moves.clear();
        self.kept.reset(pools.retained.len());
        self.parked.reset(pools.spare.len());
        let mut active = 0u64;
        // First claim resident cache hits, so caches already on screen never thrash under a tight
        // budget.
        for (index, layer) in plan.iter().enumerate() {
            let rect = rect_of(layer.edges);
            if rect[2] == 0.0 || rect[3] == 0.0 || !isolate || layer.planned != Planned::Inline { continue; }
            let Some(cache) = layer.cache else { continue };
            let Some(at) = self.kept.position(pools, |t| t.cache == Some(cache)) else { continue };
            let bytes = self.kept.held(at, pools).bytes;
            let over = active + bytes > budget;
            // A cache with contents keeps its place under the budget: it can't be made again cheaply.
            if over && layer.begin + 1 < layer.end { continue; }
            if !over { active += bytes; }
            self.dispositions[index] = Disposition::Cached { rect, cache: (!over).then_some(cache) };
            self.slots[index] = Some(self.kept.swap_remove(at));
            self.moves.push(Move::Set { slot: index, from: Kept::Retained(at) });
        }
        for (index, layer) in plan.iter().enumerate() {
            if matches!(self.dispositions[index], Disposition::Cached { .. }) { continue; }
            let rect = rect_of(layer.edges);
            let parent = layer.parent.map(|p| self.dispositions[p]);
            // A layer inside a cache that composites from its texture, or inside a transparent one,
            // draws nothing: what a cache kept comes from its texture.
            if layer.planned == Planned::Hidden || rect[2] == 0.0 || rect[3] == 0.0
                || matches!(parent, Some(Disposition::Skip | Disposition::Cached { .. })) {
                self.dispositions[index] = Disposition::Skip;
                self.park_slot(index, pools);
                continue;
            }
            if !isolate {
                // Without isolation (`breaker`), every group draws straight into its parent.
                self.dispositions[index] = Disposition::Inline;
                self.park_slot(index, pools);
                continue;
            }
            let mut bounds = rect;
            let (mut bucket, mut width, mut height) = bucket_of(bounds);
            let mut cache = None;
            if let Some((node, sig)) = layer.cache {
                let previous = self.kept.position(pools, |t| t.cache.is_some_and(|(n, _)| n == node));
                let needed = u64::from(bucket[0]) * u64::from(bucket[1]) * bpp;
                if bucket[0] <= max_dim && bucket[1] <= max_dim && active + needed <= budget {
                    active += needed;
                    cache = Some((node, sig));
                    match previous {
                        // Take the target this node kept: the new bounds may only be smaller.
                        Some(at) => {
                            self.park_slot(index, pools);
                            self.slots[index] = Some(self.kept.swap_remove(at));
                            self.moves.push(Move::Set { slot: index, from: Kept::Retained(at) });
                        }
                        None => self.trim(active, budget, pools),
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
            let additive_parent = layer.parent.is_some_and(|p| plan[p].blend == Blend::PlusLighter);
            // A clip of whole pixels with square corners and no fade is a scissor, exactly: such a
            // group draws straight into its parent too (`Renderer::draw_layer_range`). That keeps a
            // clipped pane's video in the window's own pass, where its own layer can show it.
            let scissor_clip = layer.clip == NO_CLIP || (layer.fade == [0; 4] && layer.clip.iter().all(|v| (v - v.round()).abs() < 1e-3));
            if cache.is_none() && layer.opacity == 1.0 && layer.blend == Blend::SourceOver && !additive_parent
                && layer.blur == 0.0 && layer.fade[3] == 0 && scissor_clip
                && layer.turn.is_none() && layer.hole[3] == 0.0 {
                self.dispositions[index] = Disposition::Inline;
                self.park_slot(index, pools);
                continue;
            }
            self.dispositions[index] = Disposition::Render { rect: bounds, cache };
            let fits = self.slots[index].is_some_and(|source| {
                let size = pools.held(source).size;
                // The minimum bucket is unavoidable for tiny controls. Do not recreate that
                // same bucket every frame when rounding alone exceeds the 2x area limit.
                size[0] >= width && size[1] >= height && (size == bucket
                    || u64::from(size[0]) * u64::from(size[1]) <= 2 * u64::from(width) * u64::from(height))
            });
            if !fits {
                self.park_slot(index, pools);
                let spare = self.parked.position(pools, |t| {
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
                        self.slots[index] = Some(Source::New(bucket));
                        self.moves.push(Move::Create { slot: index, size: bucket });
                    }
                }
            }
            // The scratch a blur left behind goes before the next layer makes its own texture.
            if layer.blur == 0.0 { self.moves.push(Move::Unblur(index)); }
        }
        self.trim(active, budget, pools);
    }

    /// After a frame whose layers fell to `len`: forgets the plan when no layer is left, and gives
    /// back what a burst grew the scratch to. The per-layer buffers keep what `len` layers need; a
    /// pool's order keeps what the `retained` or `spare` targets left in that pool need, since the
    /// next frame to move one copies the whole pool's order. A frame that keeps its layers, or
    /// stays empty, never comes here, so it touches nothing.
    pub(super) fn settle(&mut self, len: usize, retained: usize, spare: usize) {
        #[cfg(test)]
        { self.settles += 1; }
        // The moves are carried out and the views read; only the dispositions outlive the frame.
        self.moves.clear();
        self.slots.clear();
        self.kept.reset(0);
        self.parked.reset(0);
        if len == 0 { self.dispositions.clear(); }
        let keep = len.max(WARM);
        shrink(&mut self.dispositions, keep);
        shrink(&mut self.moves, keep);
        shrink(&mut self.slots, keep);
        shrink(&mut self.kept.order, retained.max(WARM));
        shrink(&mut self.parked.order, spare.max(WARM));
    }

    /// Test-only: the scratch's capacities, which a warm frame never changes.
    #[cfg(test)]
    pub(super) fn capacities(&self) -> [usize; 5] {
        [self.dispositions.capacity(), self.moves.capacity(), self.slots.capacity(),
            self.kept.order.capacity(), self.parked.order.capacity()]
    }

    /// Parks slot `index`'s target for a later layer of its size: what `spare` stops holding once
    /// it passes `SPARE_BYTES`, least recently used first, goes.
    fn park_slot<T: Describe>(&mut self, index: usize, pools: &Pools<'_, T>) {
        let Some(source) = self.slots[index].take() else { return };
        self.moves.push(Move::Park(index));
        self.parked.push(source);
        let mut bytes = self.parked.bytes(pools);
        while bytes > SPARE_BYTES {
            let oldest = self.parked.oldest(pools);
            bytes -= self.parked.held(oldest, pools).bytes;
            self.parked.swap_remove(oldest);
            self.moves.push(Move::Drop(Kept::Spare(oldest)));
        }
    }

    /// Drops the least recently used kept targets until the budget left after this frame's `active`
    /// cache bytes holds them.
    fn trim<T: Describe>(&mut self, active: u64, budget: u64, pools: &Pools<'_, T>) {
        let available = budget.saturating_sub(active);
        let mut bytes = self.kept.bytes(pools);
        while bytes > available {
            let oldest = self.kept.oldest(pools);
            bytes -= self.kept.held(oldest, pools).bytes;
            self.kept.swap_remove(oldest);
            self.moves.push(Move::Drop(Kept::Retained(oldest)));
        }
    }
}
