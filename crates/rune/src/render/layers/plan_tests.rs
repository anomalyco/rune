//! A walk of every frame sequence through the plan, checked against origin/dev's planning passes
//! transcribed over plain records: what each layer draws, and what becomes of the target pools.
//!
//! The reference is dev's code as it was, not a cleaned-up version of it: the claim pass, the
//! settle pass and the allocate pass, each with the pools it reads and rewrites in the order it
//! rewrote them. The side under test is the real `Layers::decide`, whose moves are carried out by
//! `apply` (a transcription of `commit`) and whose target descriptors are written by `finish` (a
//! transcription of `paint_layers`), so a move in the wrong order, at the wrong index, or with a
//! disposition changed, fails the walk.
use super::*;

/// A layer of a frame's scene, as the planning walk would record it.
#[derive(Clone, Copy, Debug)]
struct Spec {
    parent: Option<usize>,
    edges: [f32; 4],
    vis: [f32; 4],
    cache: Option<(u64, u64)>,
    opacity: f32,
    blend: Blend,
    blur: f32,
    fade: u32,
    clip: [f32; 4],
    turn: Option<(f32, [f32; 2])>,
    hole: [f32; 4],
}

impl Spec {
    fn plain(edges: [f32; 4], vis: [f32; 4]) -> Self {
        Self { parent: None, edges, vis, cache: None, opacity: 1.0, blend: Blend::SourceOver, blur: 0.0,
            fade: 0, clip: NO_CLIP, turn: None, hole: [0.0; 4] }
    }

    fn cached(edges: [f32; 4], vis: [f32; 4], cache: (u64, u64)) -> Self {
        Self { cache: Some(cache), ..Self::plain(edges, vis) }
    }

    fn child(parent: usize, edges: [f32; 4], vis: [f32; 4]) -> Self {
        Self { parent: Some(parent), ..Self::plain(edges, vis) }
    }
}

/// The walk's scene: a frame's layers, with the scopes the specs nest into.
fn scenes() -> Vec<Vec<Spec>> {
    let window = [0.0, 0.0, 32.0, 16.0];
    vec![
        // A cached card with a half-faded child (`layer_show_tests`' scene).
        vec![Spec::cached([4.0, 2.0, 24.0, 12.0], window, (7, 1)),
            Spec { opacity: 0.5, ..Spec::child(0, [8.0, 4.0, 16.0, 6.0], window) }],
        // Two sibling caches, so one budget can hold one and not the other.
        vec![Spec::cached([0.0, 0.0, 16.0, 8.0], window, (11, 1)),
            Spec::cached([16.0, 8.0, 16.0, 8.0], window, (12, 3))],
        // A cache inside a cache, and a child of the inner one.
        vec![Spec::cached([2.0, 1.0, 28.0, 14.0], window, (21, 2)),
            Spec { parent: Some(0), ..Spec::cached([4.0, 2.0, 12.0, 8.0], window, (22, 5)) },
            Spec { opacity: 0.75, ..Spec::child(1, [6.0, 3.0, 8.0, 4.0], window) }],
        // Something that flattens, a clip that isolates, a blur and a dissolve.
        vec![Spec::plain([0.0, 0.0, 8.0, 8.0], window),
            Spec { clip: [4.0, 2.0, 20.0, 10.0], ..Spec::child(0, [4.0, 2.0, 16.0, 8.0], window) },
            Spec { blur: 2.0, ..Spec::plain([8.0, 0.0, 16.0, 8.0], window) },
            Spec { blend: Blend::PlusLighter, ..Spec::plain([12.0, 4.0, 8.0, 8.0], window) }],
        // The cached card grown, so the target it kept no longer fits its bucket.
        vec![Spec::cached([4.0, 2.0, 26.0, 13.0], window, (7, 1)),
            Spec { opacity: 0.5, ..Spec::child(0, [8.0, 4.0, 18.0, 7.0], window) }],
        // A turned cache, and a fade that isolates without a target of its own.
        vec![Spec { turn: Some((0.3, [16.0, 8.0])), ..Spec::cached([8.0, 4.0, 16.0, 8.0], window, (31, 4)) },
            Spec { fade: 1200, hole: [16.0, 8.0, 6.0, 8.0], ..Spec::plain([6.0, 2.0, 20.0, 12.0], window) }],
    ]
}

/// A scene's layers, with the scopes the specs nest into and the walk's verdict on each: a layer
/// inside a cache the pool holds is covered, a transparent one is hidden.
fn plan_of(scene: &[Spec], held: &[Held]) -> Vec<Layer> {
    let mut end: Vec<u32> = (0..scene.len() as u32).collect();
    for (index, spec) in scene.iter().enumerate() {
        let mut parent = spec.parent;
        while let Some(p) = parent {
            end[p] = end[p].max(index as u32);
            parent = scene[p].parent;
        }
    }
    let mut planned = Vec::new();
    scene.iter().enumerate().map(|(index, spec)| {
        let above = spec.parent.map(|p| planned[p]);
        let covered = spec.parent.is_some_and(|p| scene[p].cache.is_some_and(|key| held.iter().any(|t| t.cache == Some(key))));
        planned.push(match above {
            _ if spec.opacity == 0.0 => Planned::Hidden,
            Some(Planned::Hidden) => Planned::Hidden,
            Some(Planned::Covered) => Planned::Covered,
            _ if covered => Planned::Covered,
            _ => Planned::Inline,
        });
        Layer {
            begin: index as u32, end: end[index], parent: spec.parent,
            edges: spec.edges, vis: spec.vis, clip: spec.clip, limit: NO_CLIP,
            fade: [0, 0, 0, spec.fade], opacity: spec.opacity, blend: spec.blend, blur: spec.blur,
            support: 0.0, cache: spec.cache, planned: planned[index], turn: spec.turn,
            free: spec.turn.is_some(), frame: spec.edges, hole: spec.hole,
        }
    }).collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum OldShow { Hidden, Covered, Inline, Render, Cached }

/// A layer as origin/dev's `Layer` held it, with the fields its passes read and wrote.
#[derive(Clone, Copy, Debug)]
struct OldLayer {
    begin: u32, end: u32, parent: Option<usize>,
    edges: [f32; 4], vis: [f32; 4], rect: [f32; 4],
    cache: Option<(u64, u64)>, cache_target: Option<(u64, u64)>,
    opacity: f32, blend: Blend, blur: f32, fade: [u32; 4], clip: [f32; 4],
    turn: Option<(f32, [f32; 2])>, hole: [f32; 4],
    show: OldShow,
}

impl From<&Layer> for OldLayer {
    fn from(l: &Layer) -> Self {
        Self {
            begin: l.begin, end: l.end, parent: l.parent, edges: l.edges, vis: l.vis, rect: [0.0; 4],
            cache: l.cache, cache_target: None, opacity: l.opacity, blend: l.blend, blur: l.blur,
            fade: l.fade, clip: l.clip, turn: l.turn, hole: l.hole,
            show: match l.planned {
                Planned::Hidden => OldShow::Hidden,
                Planned::Covered => OldShow::Covered,
                Planned::Inline => OldShow::Inline,
            },
        }
    }
}

/// A target as dev's pools held it, with the fields the plan reads.
#[derive(Clone, Copy, Debug, PartialEq)]
struct OldHeld { size: [u32; 2], bytes: u64, cache: Option<(u64, u64)>, last_used: u64, content: [f32; 2] }

impl From<&Held> for OldHeld {
    fn from(h: &Held) -> Self {
        Self { size: h.size, bytes: h.bytes, cache: h.cache, last_used: h.last_used, content: h.content }
    }
}

impl From<&OldHeld> for Held {
    fn from(h: &OldHeld) -> Self {
        Self { size: h.size, bytes: h.bytes, cache: h.cache, last_used: h.last_used, content: h.content }
    }
}

/// Dev's target pools, and the three passes that read them.
#[derive(Clone, Default, Debug, PartialEq)]
struct Old { retained: Vec<OldHeld>, spare: Vec<OldHeld>, slots: Vec<Option<OldHeld>> }

impl Old {
    fn park(&mut self, mut t: OldHeld) {
        t.cache = None;
        self.spare.push(t);
        let mut bytes: u64 = self.spare.iter().map(|t| t.bytes).sum();
        while bytes > SPARE_BYTES {
            let oldest = self.spare.iter().enumerate().min_by_key(|(_, t)| t.last_used).unwrap().0;
            bytes -= self.spare.swap_remove(oldest).bytes;
        }
    }

    fn release(&mut self, index: usize) {
        if let Some(t) = self.slots[index].take() { self.park(t); }
    }

    fn trim_retained(&mut self, active_bytes: u64, budget: u64) {
        let available = budget.saturating_sub(active_bytes);
        let mut bytes: u64 = self.retained.iter().map(|t| t.bytes).sum();
        while bytes > available {
            let oldest = self.retained.iter().enumerate().min_by_key(|(_, t)| t.last_used).unwrap().0;
            bytes -= self.retained.swap_remove(oldest).bytes;
        }
    }

    fn take_spare(&mut self, width: u32, height: u32) -> Option<OldHeld> {
        let fits = |t: &OldHeld| {
            let size = (t.size[0], t.size[1]);
            size.0 >= width && size.1 >= height && u64::from(size.0) * u64::from(size.1) <= 2 * u64::from(width.max(64)) * u64::from(height.max(64))
        };
        let at = self.spare.iter().position(fits)?;
        Some(self.spare.swap_remove(at))
    }

    /// Dev's `prepare_layers`: the claim pass, the settle pass and the allocate pass as they were.
    fn passes(&mut self, plan: &mut [OldLayer], isolate: bool, max_dim: u32, budget: u64) {
        let bpp = u64::from(FORMAT.block_copy_size(None).unwrap());
        while self.slots.len() > plan.len() {
            if let Some(t) = self.slots.pop().flatten() { self.park(t); }
        }
        self.slots.resize(plan.len(), None);
        let mut active_cache_bytes = 0u64;
        // First claim resident cache hits so existing on-screen caches never thrash under a tight budget.
        for index in 0..plan.len() {
            let l = &mut plan[index];
            let e = l.edges;
            l.rect = [e[0], e[1], (e[2] - e[0]).max(0.0), (e[3] - e[1]).max(0.0)];
            if l.rect[2] == 0.0 || l.rect[3] == 0.0 { l.show = OldShow::Hidden; }
            if !isolate || l.show != OldShow::Inline { continue; }
            let Some(key) = l.cache else { continue };
            let Some(pos) = self.retained.iter().position(|t| t.cache == Some(key)) else { continue };
            let t_bytes = self.retained[pos].bytes;
            let over_budget = active_cache_bytes + t_bytes > budget;
            if over_budget && l.begin + 1 < l.end { continue; }
            let t = self.retained.swap_remove(pos);
            if !over_budget { active_cache_bytes += t_bytes; }
            l.show = OldShow::Cached;
            l.cache_target = if over_budget { None } else { t.cache };
            self.slots[index] = Some(t);
        }
        // Settle what the planning walk couldn't know at a layer's start.
        for index in 0..plan.len() {
            let parent = plan[index].parent.map(|p| plan[p].show);
            let l = &mut plan[index];
            l.show = match (l.show, parent) {
                (OldShow::Cached, _) => OldShow::Cached,
                (_, Some(OldShow::Hidden)) => OldShow::Hidden,
                (OldShow::Covered, Some(OldShow::Covered | OldShow::Cached)) => OldShow::Covered,
                (OldShow::Covered, _) => OldShow::Inline,
                (show, _) => show,
            };
        }
        for index in 0..plan.len() {
            match plan[index].show {
                OldShow::Inline => {}
                OldShow::Cached | OldShow::Render => continue,
                OldShow::Hidden | OldShow::Covered => { self.release(index); continue; }
            }
            let additive_parent = plan[index].parent.is_some_and(|p| plan[p].blend == Blend::PlusLighter);
            let l = &mut plan[index];
            if !isolate { self.release(index); continue; }
            let mut bounds = l.rect;
            let (mut width, mut height) = (bounds[2] as u32, bounds[3] as u32);
            let mut bucket = (width.div_ceil(64) * 64, height.div_ceil(64) * 64);
            let mut cache_target = None;
            if let Some((node_id, sig)) = l.cache {
                let previous = self.retained.iter().position(|t| t.cache.is_some_and(|(n, _)| n == node_id));
                let needed_bytes = u64::from(bucket.0) * u64::from(bucket.1) * bpp;
                if bucket.0 <= max_dim && bucket.1 <= max_dim && active_cache_bytes + needed_bytes <= budget {
                    active_cache_bytes += needed_bytes;
                    cache_target = Some((node_id, sig));
                    if let Some(pos) = previous {
                        self.release(index);
                        self.slots[index] = Some(self.retained.swap_remove(pos));
                    } else {
                        self.trim_retained(active_cache_bytes, budget);
                    }
                } else {
                    if let Some(pos) = previous {
                        self.retained.swap_remove(pos);
                    }
                    let v = l.vis;
                    bounds = [v[0], v[1], (v[2] - v[0]).max(0.0), (v[3] - v[1]).max(0.0)];
                    l.rect = bounds;
                    (width, height) = (bounds[2] as u32, bounds[3] as u32);
                    bucket = (width.div_ceil(64) * 64, height.div_ceil(64) * 64);
                }
            }
            let l = &mut plan[index];
            if cache_target.is_none() && l.opacity == 1.0 && l.blend == Blend::SourceOver && !additive_parent
                && l.blur == 0.0 && l.fade[3] == 0 && l.clip == NO_CLIP && l.turn.is_none() && l.hole[3] == 0.0 {
                self.release(index);
                continue;
            }
            l.show = OldShow::Render;
            l.cache_target = cache_target;
            if self.slots[index].as_ref().is_none_or(|t| {
                let size = (t.size[0], t.size[1]);
                size.0 < width || size.1 < height || (size != bucket
                    && u64::from(size.0) * u64::from(size.1) > 2 * u64::from(width) * u64::from(height))
            }) && let Some(spare) = { self.release(index); self.take_spare(width, height) } {
                self.slots[index] = Some(spare);
            }
            if self.slots[index].is_none() {
                let (width, height) = bucket;
                self.slots[index] = Some(OldHeld { size: [width, height], bytes: u64::from(width) * u64::from(height) * bpp,
                    cache: None, last_used: 0, content: [0.0; 2] });
            }
        }
        self.trim_retained(active_cache_bytes, budget);
    }

    /// Dev's writes into the target a layer drew into, which `paint_layers` carries out.
    fn finish(&mut self, plan: &[OldLayer], stamp: u64) {
        for (index, l) in plan.iter().enumerate() {
            let Some(t) = self.slots[index].as_mut() else { continue };
            match l.show {
                OldShow::Cached => {
                    t.cache = l.cache_target;
                    if l.cache_target.is_some() { t.last_used = stamp; }
                }
                OldShow::Render => {
                    t.cache = l.cache_target;
                    t.last_used = stamp;
                    t.content = [l.rect[2], l.rect[3]];
                }
                OldShow::Hidden | OldShow::Covered | OldShow::Inline => {}
            }
        }
    }
}

/// `Renderer::commit`, transcribed: the moves the plan decided, in its order.
fn apply(pools: &mut Old, moves: &[Move]) {
    for &m in moves {
        match m {
            Move::Park(slot) => {
                if let Some(mut t) = pools.slots[slot].take() { t.cache = None; pools.spare.push(t); }
            }
            Move::Set { slot, from } => {
                pools.slots[slot] = Some(match from {
                    Kept::Retained(at) => pools.retained.swap_remove(at),
                    Kept::Spare(at) => pools.spare.swap_remove(at),
                });
            }
            Move::Create { slot, size } => {
                let bpp = u64::from(FORMAT.block_copy_size(None).unwrap());
                pools.slots[slot] = Some(OldHeld { size, bytes: u64::from(size[0]) * u64::from(size[1]) * bpp,
                    cache: None, last_used: 0, content: [0.0; 2] });
            }
            Move::Drop(Kept::Retained(at)) => { pools.retained.swap_remove(at); }
            Move::Drop(Kept::Spare(at)) => { pools.spare.swap_remove(at); }
        }
    }
}

/// `Renderer::paint_layers`, transcribed: what a target learned about the frame it drew.
fn finish(pools: &mut Old, dispositions: &[Disposition], stamp: u64) {
    for (index, disposition) in dispositions.iter().enumerate() {
        let Some(t) = pools.slots[index].as_mut() else { continue };
        match *disposition {
            Disposition::Cached { cache, .. } => {
                t.cache = cache;
                if cache.is_some() { t.last_used = stamp; }
            }
            Disposition::Render { rect, cache } => {
                t.cache = cache;
                t.last_used = stamp;
                t.content = [rect[2], rect[3]];
            }
            Disposition::Skip | Disposition::Inline => {}
        }
    }
}

/// One frame: the plan decides from the pools as they are, both sides, and every verdict and every
/// pool is compared.
fn frame(pools: &Old, old_pools: &mut Old, scene: &[Spec], isolate: bool, budget: u64, stamp: u64) {
    let mut layers = Layers::default();
    layers.cache_budget = budget;
    // `prepare_layers` parks the slots a shorter plan no longer has, and gives the caches they
    // held back to the pool, before the planning walk asks whether a parent's cache is resident.
    let mut before = pools.clone();
    while before.slots.len() > scene.len() {
        if let Some(t) = before.slots.pop().flatten() { before.park(t); }
    }
    before.slots.resize(scene.len(), None);
    for slot in &mut before.slots {
        if slot.as_ref().is_some_and(|t| t.cache.is_some()) {
            before.retained.push(slot.take().unwrap());
        }
    }
    layers.plan = plan_of(scene, &before.retained.iter().map(Held::from).collect::<Vec<_>>());
    layers.kept = before.retained.iter().map(Held::from).collect();
    layers.parked = before.spare.iter().map(Held::from).collect();
    layers.slots = before.slots.iter().map(|t| t.as_ref().map(Held::from)).collect();
    layers.decide(isolate, 4096);

    let mut plan: Vec<OldLayer> = layers.plan.iter().map(OldLayer::from).collect();
    old_pools.clone_from(&before);
    old_pools.passes(&mut plan, isolate, 4096, budget);

    for (index, old) in plan.iter().enumerate() {
        let got = layers.dispositions[index];
        match (old.show, got) {
            (OldShow::Hidden | OldShow::Covered, Disposition::Skip)
                | (OldShow::Inline, Disposition::Inline) => {}
            (OldShow::Render, Disposition::Render { rect, cache })
                | (OldShow::Cached, Disposition::Cached { rect, cache }) => {
                assert_eq!((rect, cache), (old.rect, old.cache_target), "layer {index} at {stamp}");
            }
            (show, got) => panic!("layer {index} at {stamp}: dev says {show:?}, the plan says {got:?}"),
        }
    }
    assert_eq!(layers.kept, old_pools.retained.iter().map(Held::from).collect::<Vec<_>>(), "kept at {stamp}");
    assert_eq!(layers.parked, old_pools.spare.iter().map(Held::from).collect::<Vec<_>>(), "spare at {stamp}");
    assert_eq!(layers.slots, old_pools.slots.iter().map(|t| t.as_ref().map(Held::from)).collect::<Vec<_>>(), "slots at {stamp}");

    // The moves say the same thing as the passes did, at the same indices.
    let mut moved = before;
    apply(&mut moved, &layers.moves);
    old_pools.finish(&plan, stamp);
    finish(&mut moved, &layers.dispositions, stamp);
    assert_eq!(moved, *old_pools, "moves at {stamp}");
}

#[test]
fn every_frame_walk_matches_dev() {
    let scenes = scenes();
    // Each event is a scene the next frame draws, the budget it has, and whether layers can isolate.
    let events: Vec<(usize, u64, bool)> = vec![
        (0, DEFAULT_CACHE_BUDGET, true), (1, DEFAULT_CACHE_BUDGET, true), (1, 40_000, true),
        (2, DEFAULT_CACHE_BUDGET, true), (3, DEFAULT_CACHE_BUDGET, true), (0, 40_000, true),
        (4, DEFAULT_CACHE_BUDGET, true), (5, DEFAULT_CACHE_BUDGET, true), (1, DEFAULT_CACHE_BUDGET, false),
    ];
    fn walk(scenes: &[Vec<Spec>], events: &[(usize, u64, bool)], pools: &Old, stamp: u64, depth: u32) {
        if depth == 0 { return; }
        for &(scene, budget, isolate) in events {
            let mut after = Old::default();
            let next = stamp + 1;
            frame(pools, &mut after, &scenes[scene], isolate, budget, next);
            walk(scenes, events, &after, next, depth - 1);
        }
    }
    walk(&scenes, &events, &Old::default(), 0, 5);
}
