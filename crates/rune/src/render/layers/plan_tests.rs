//! A walk of every frame sequence through the plan, checked against origin/dev's planning passes
//! transcribed over plain records: what each layer draws, and what becomes of the target pools.
//!
//! The reference is dev's code as it was, not a cleaned-up version of it: the claim pass, the settle
//! pass and the allocate pass, each with the pools it reads and rewrites in the order it rewrote
//! them, its target writes where it made them, and what a blur scratch costs. The side under test
//! is the real `Planner::decide`, whose moves are carried out by `apply` (a transcription of
//! `commit`) and whose target descriptors are written by `finish` (a transcription of
//! `paint_layers`), so a move in the wrong order, at the wrong index, or with a disposition changed,
//! fails the walk.
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
    /// The blur's sigma, as `layer_blur::sigma` would have it.
    blur: f32,
    fade: u32,
    clip: [f32; 4],
    turn: Option<(f32, [f32; 2])>,
    hole: [f32; 4],
    /// The ordinary draws its scope opens with, before any child layer's.
    draws: u32,
}

impl Spec {
    fn plain(edges: [f32; 4], vis: [f32; 4]) -> Self {
        Self { parent: None, edges, vis, cache: None, opacity: 1.0, blend: Blend::SourceOver, blur: 0.0,
            fade: 0, clip: NO_CLIP, turn: None, hole: [0.0; 4], draws: 0 }
    }

    fn cached(edges: [f32; 4], vis: [f32; 4], cache: (u64, u64)) -> Self {
        Self { cache: Some(cache), ..Self::plain(edges, vis) }
    }

    fn child(parent: usize, edges: [f32; 4], vis: [f32; 4]) -> Self {
        Self { parent: Some(parent), ..Self::plain(edges, vis) }
    }
}

/// The walk's scenes: what a frame's instance stream holds.
fn scenes() -> Vec<Vec<Spec>> {
    let window = [0.0, 0.0, 32.0, 16.0];
    vec![
        // A cached card with a half-faded child (`layer_show_tests`' scene).
        vec![Spec::cached([4.0, 2.0, 24.0, 12.0], window, (7, 1)),
            Spec { opacity: 0.5, ..Spec::child(0, [8.0, 4.0, 16.0, 6.0], window) }],
        // Two sibling caches, so one budget can hold one and not the other.
        vec![Spec::cached([0.0, 0.0, 16.0, 8.0], window, (11, 1)),
             Spec::cached([16.0, 8.0, 32.0, 16.0], window, (12, 3))],
        // A cache inside a cache, and a child of the inner one: the outer scope has contents.
        vec![Spec::cached([2.0, 1.0, 28.0, 14.0], window, (21, 2)),
            Spec { parent: Some(0), ..Spec::cached([4.0, 2.0, 12.0, 8.0], window, (22, 5)) },
            Spec { opacity: 0.75, ..Spec::child(1, [6.0, 3.0, 8.0, 4.0], window) }],
        // Something that flattens, a clip off whole pixels that isolates, a blur, a dissolve, and a
        // clip of whole pixels that's a scissor and flattens.
        vec![Spec::plain([0.0, 0.0, 8.0, 8.0], window),
            Spec { clip: [4.5, 2.0, 20.0, 10.0], ..Spec::child(0, [4.0, 2.0, 16.0, 8.0], window) },
            Spec { blur: 2.0, ..Spec::plain([8.0, 0.0, 16.0, 8.0], window) },
            Spec { blend: Blend::PlusLighter, ..Spec::plain([12.0, 4.0, 8.0, 8.0], window) },
            Spec { clip: [8.0, 4.0, 24.0, 12.0], ..Spec::child(0, [8.0, 4.0, 16.0, 8.0], window) }],
        // The cached card grown, so the target it kept no longer fits its bucket.
        vec![Spec::cached([4.0, 2.0, 26.0, 13.0], window, (7, 1)),
            Spec { opacity: 0.5, ..Spec::child(0, [8.0, 4.0, 18.0, 7.0], window) }],
        // A turned cache, and a fade that isolates without a target of its own.
        vec![Spec { turn: Some((0.3, [16.0, 8.0])), ..Spec::cached([8.0, 4.0, 16.0, 8.0], window, (31, 4)) },
            Spec { fade: 1200, hole: [16.0, 8.0, 6.0, 8.0], ..Spec::plain([6.0, 2.0, 12.0, 12.0], window) }],
        // A cached card that blurs widely: its target and the scratch's own work target are three
        // 64×64 textures, more than the budget below will keep.
        vec![Spec { blur: 12.0, ..Spec::cached([4.0, 2.0, 68.0, 66.0], window, (41, 1)) },
            Spec { opacity: 0.5, ..Spec::child(0, [8.0, 4.0, 16.0, 6.0], window) }],
        // Two cached leaves side by side: the first draws a rect, a cache with contents and no
        // child layer; the second draws nothing at all.
        vec![Spec { draws: 1, ..Spec::cached([0.0, 0.0, 16.0, 8.0], window, (11, 1)) },
            Spec::cached([16.0, 8.0, 32.0, 16.0], window, (12, 3))],
    ]
}

/// Where each layer's scope opens and closes in the instance stream: a layer is a `KIND_LAYER` and a
/// `KIND_LAYER_END`, with its draws and then its child layers between them, so a cache with contents
/// of either kind has `begin + 1 < end` and one with none doesn't.
fn marks(scene: &[Spec]) -> (Vec<u32>, Vec<u32>) {
    let mut at = 0u32;
    let (mut begin, mut end) = (vec![0u32; scene.len()], vec![0u32; scene.len()]);
    fn visit(scene: &[Spec], index: usize, at: &mut u32, begin: &mut [u32], end: &mut [u32]) {
        begin[index] = *at;
        *at += 1 + scene[index].draws;
        for (child, spec) in scene.iter().enumerate() {
            if spec.parent == Some(index) { visit(scene, child, at, begin, end); }
        }
        end[index] = *at;
        *at += 1;
    }
    for (index, spec) in scene.iter().enumerate() {
        if spec.parent.is_none() { visit(scene, index, &mut at, &mut begin, &mut end); }
    }
    (begin, end)
}

/// A scene's layers, with the scopes their instance stream nests into and the walk's verdict on
/// each: a layer inside a cache the pool holds is covered, a transparent one is hidden.
fn plan_of(scene: &[Spec], held: &[Held]) -> Vec<Layer> {
    let (begin, end) = marks(scene);
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
            begin: begin[index], end: end[index], parent: spec.parent,
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

/// The reference's layers: dev's instance walk opened and closed each scope where the scene's
/// stream does, and the pass's own records carry the rest.
fn old_layers(scene: &[Spec], plan: &[Layer]) -> Vec<OldLayer> {
    let (begin, end) = marks(scene);
    plan.iter().enumerate().map(|(index, l)| OldLayer { begin: begin[index], end: end[index], ..OldLayer::from(l) }).collect()
}

/// Dev's blur scratch on a target: its own texture at the target's size, and the reduced work target
/// `prepare` makes once a sigma wide enough asks for one.
#[derive(Clone, Copy, Debug, PartialEq)]
struct OldScratch { work: bool }

/// A target as dev's pools held it, and what dev's `target_bytes` counts for it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct OldHeld {
    size: [u32; 2],
    cache: Option<(u64, u64)>,
    last_used: u64,
    content: [f32; 2],
    blur: Option<OldScratch>,
}

impl OldHeld {
    fn new(size: [u32; 2]) -> Self {
        Self { size, cache: None, last_used: 0, content: [0.0; 2], blur: None }
    }

    /// The texture, and the scratch's own texture plus its work target when it has one.
    fn bytes(&self) -> u64 {
        let texture = u64::from(self.size[0]) * u64::from(self.size[1]) * 8;
        let scratch = self.blur.map_or(0, |b| if b.work { 2 * texture } else { texture });
        texture + scratch
    }

    /// What dev's `prepare` did to the scratch this frame: make one if there is none, and make the
    /// work target when the sigma needs an intermediate resolution.
    fn blur(&mut self, sigma: f32, content: [f32; 2]) {
        let scratch = self.blur.get_or_insert(OldScratch { work: false });
        let reduction = (sigma / 3.0).clamp(1.0, 8.0);
        let reduced = [content[0] / reduction, content[1] / reduction].map(f32::ceil);
        if reduced != content { scratch.work = true; }
    }
}

impl From<&OldHeld> for Held {
    fn from(h: &OldHeld) -> Self {
        Self { size: h.size, bytes: h.bytes(), cache: h.cache, last_used: h.last_used, content: h.content }
    }
}

/// The plan reads a walk's targets where they are, as it reads a frame's.
impl Describe for OldHeld {
    fn held(&self) -> Held { Held::from(self) }
}

/// Dev's target pools, and the three passes that read them.
#[derive(Clone, Default, Debug, PartialEq)]
struct Old { retained: Vec<OldHeld>, spare: Vec<OldHeld>, slots: Vec<Option<OldHeld>> }

impl Old {
    fn park(&mut self, mut t: OldHeld) {
        t.cache = None;
        self.spare.push(t);
        let mut bytes: u64 = self.spare.iter().map(OldHeld::bytes).sum();
        while bytes > SPARE_BYTES {
            let oldest = self.spare.iter().enumerate().min_by_key(|(_, t)| t.last_used).unwrap().0;
            bytes -= self.spare.swap_remove(oldest).bytes();
        }
    }

    fn release(&mut self, index: usize) {
        if let Some(t) = self.slots[index].take() { self.park(t); }
    }

    fn trim_retained(&mut self, active_bytes: u64, budget: u64) {
        let available = budget.saturating_sub(active_bytes);
        let mut bytes: u64 = self.retained.iter().map(OldHeld::bytes).sum();
        while bytes > available {
            let oldest = self.retained.iter().enumerate().min_by_key(|(_, t)| t.last_used).unwrap().0;
            bytes -= self.retained.swap_remove(oldest).bytes();
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
    fn passes(&mut self, plan: &mut [OldLayer], isolate: bool, max_dim: u32, budget: u64, stamp: u64) {
        let bpp = u64::from(FORMAT.block_copy_size(None).unwrap());
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
            let t_bytes = self.retained[pos].bytes();
            let over_budget = active_cache_bytes + t_bytes > budget;
            if over_budget && l.begin + 1 < l.end { continue; }
            let mut t = self.retained.swap_remove(pos);
            if over_budget {
                t.cache = None;
            } else {
                t.last_used = stamp;
                active_cache_bytes += t_bytes;
            }
            l.show = OldShow::Cached;
            l.cache_target = t.cache;
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
                && l.blur == 0.0 && l.fade[3] == 0 && l.turn.is_none() && l.hole[3] == 0.0
                && (l.clip == NO_CLIP || (l.fade == [0; 4] && l.clip.iter().all(|v| (v - v.round()).abs() < 1e-3))) {
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
                self.slots[index] = Some(OldHeld::new([width, height]));
            }
            let sigma = plan[index].blur;
            let bounds = plan[index].rect;
            let t = self.slots[index].as_mut().unwrap();
            t.cache = cache_target;
            t.last_used = stamp;
            t.content = [bounds[2], bounds[3]];
            if sigma > 0.0 {
                t.blur(sigma, [bounds[2], bounds[3]]);
            } else {
                t.blur = None;
            }
        }
        self.trim_retained(active_cache_bytes, budget);
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
            Move::Create { slot, size } => pools.slots[slot] = Some(OldHeld::new(size)),
            Move::Unblur(slot) => {
                if let Some(t) = pools.slots[slot].as_mut() { t.blur = None; }
            }
            Move::Drop(Kept::Retained(at)) => { pools.retained.swap_remove(at); }
            Move::Drop(Kept::Spare(at)) => { pools.spare.swap_remove(at); }
        }
    }
}

/// `Renderer::paint_layers`, transcribed: what a target learned about the frame it drew.
fn finish(pools: &mut Old, plan: &[Layer], dispositions: &[Disposition], stamp: u64) {
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
                // The plan dropped a scratch the layer no longer wants (`Move::Unblur`).
                if plan[index].blur > 0.0 { t.blur(plan[index].blur, [rect[2], rect[3]]); }
            }
            Disposition::Skip | Disposition::Inline => {}
        }
    }
}

/// One frame: the plan decides from the pools as they are, both sides, and every verdict and every
/// pool is compared. `planner` is the same one frame after frame, warm with what the last left in
/// it. Returns what the frame decided for each layer.
fn frame(planner: &mut Planner, pools: &Old, old_pools: &mut Old, scene: &[Spec], isolate: bool, budget: u64, stamp: u64) -> Vec<Disposition> {
    // `prepare_layers`' prelude, in its order: a slot that still keeps a cache gives it back to the
    // pool first, then the slots a shorter plan no longer has are parked, then the slots shrink.
    let previous = pools.slots.len();
    let mut before = pools.clone();
    for slot in &mut before.slots {
        if slot.as_ref().is_some_and(|t| t.cache.is_some()) { before.retained.push(slot.take().unwrap()); }
    }
    while before.slots.len() > scene.len() {
        if let Some(t) = before.slots.pop().flatten() { before.park(t); }
    }
    before.slots.resize(scene.len(), None);

    let layers = plan_of(scene, &before.retained.iter().map(Held::from).collect::<Vec<_>>());
    old_pools.clone_from(&before);
    let mut plan: Vec<OldLayer> = old_layers(scene, &layers);
    old_pools.passes(&mut plan, isolate, 4096, budget, stamp);

    let mut decisions = Vec::new();
    if scene.is_empty() {
        // A layerless frame plans nothing: dev's only work is the trim its passes end with.
        before.trim_retained(0, budget);
    } else {
        let pools = Pools { slots: &before.slots, retained: &before.retained, spare: &before.spare };
        planner.decide(&layers, &pools, budget, isolate, 4096);
        for (index, old) in plan.iter().enumerate() {
            let got = planner.dispositions[index];
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
        apply(&mut before, &planner.moves);
        finish(&mut before, &layers, &planner.dispositions, stamp);
        decisions = planner.dispositions.clone();
    }
    assert_eq!(before, *old_pools, "the pools at {stamp}, from {:?}: {:?}", planner.moves, planner.dispositions);
    // `prepare_layers`' tail: a frame whose layers fell gives the plan's scratch back.
    if scene.len() < previous { planner.settle(scene.len(), before.retained.len(), before.spare.len()); }
    decisions
}

#[test]
fn every_frame_walk_matches_dev() {
    let scenes = scenes();
    const TIGHT: u64 = 40_000;
    // Each event is a scene the next frame draws, the budget it has, and whether layers can isolate.
    let events: Vec<(usize, u64, bool)> = vec![
        (0, DEFAULT_CACHE_BUDGET, true), (1, DEFAULT_CACHE_BUDGET, true), (1, TIGHT, true),
        (0, 1, true), (2, DEFAULT_CACHE_BUDGET, true), (3, DEFAULT_CACHE_BUDGET, true),
        (6, TIGHT, true), (4, DEFAULT_CACHE_BUDGET, true), (5, DEFAULT_CACHE_BUDGET, true),
        (0, TIGHT, true), (1, DEFAULT_CACHE_BUDGET, false),
        (7, DEFAULT_CACHE_BUDGET, true), (7, 1, true),
    ];
    fn walk(scenes: &[Vec<Spec>], events: &[(usize, u64, bool)], pools: &Old, planner: &Planner, stamp: u64, depth: u32) -> bool {
        if depth == 0 { return false; }
        let mut saw_split_budget = false;
        for &(scene, budget, isolate) in events {
            let (mut after, mut planner) = (Old::default(), planner.clone());
            let next = stamp + 1;
            let decisions = frame(&mut planner, pools, &mut after, &scenes[scene], isolate, budget, next);
            // Both siblings hit, but the budget keeps only the first for another frame.
            saw_split_budget |= scene == 1 && budget == TIGHT && matches!(decisions.as_slice(), [
                Disposition::Cached { cache: Some((11, 1)), .. }, Disposition::Cached { cache: None, .. },
            ]);
            saw_split_budget |= walk(scenes, events, &after, &planner, next, depth - 1);
            // Nothing is left for a later frame to find: every scene closes what it opened.
            assert!(after.slots.len() <= scenes[scene].len());
        }
        saw_split_budget
    }
    // An empty scene closes the frame's layers and shows what a layerless frame does to the pools.
    let empty = scenes.len();
    let mut scenes = scenes;
    scenes.push(Vec::new());
    let events: Vec<(usize, u64, bool)> = events.into_iter().chain([(empty, DEFAULT_CACHE_BUDGET, true)]).collect();
    assert!(walk(&scenes, &events, &Old::default(), &Planner::default(), 0, 5),
        "the tight budget never kept one sibling cache while borrowing the other");
}

/// What dev's `target_bytes` counts for a kept target: its own texture, plus the blur scratch's
/// texture and the reduced work target a wide sigma adds at the same size.
#[test]
fn a_kept_target_costs_its_texture_and_its_blur() {
    let (mut direct, mut reduced) = (OldHeld::new([64, 64]), OldHeld::new([64, 64]));
    direct.blur(1.0, [64.0, 64.0]);
    reduced.blur(12.0, [64.0, 64.0]);
    assert_eq!(direct.bytes(), 65_536, "64×64 at eight bytes, and a direct scratch beside it");
    assert_eq!(reduced.bytes(), 98_304, "and the work target a wide sigma adds");
}

/// A cache whose layers close keeps its texture for the next frame of its node: dev gave every slot
/// that still kept a cache back to the pool *before* it shrank the slots, so a menu that closes and
/// opens again makes no texture.
#[test]
fn a_closed_cache_is_still_kept_when_its_layers_come_back() {
    let scenes = scenes();
    let (mut pools, mut after, mut planner) = (Old::default(), Old::default(), Planner::default());
    frame(&mut planner, &pools, &mut after, &scenes[0], true, DEFAULT_CACHE_BUDGET, 1);
    std::mem::swap(&mut pools, &mut after);
    frame(&mut planner, &pools, &mut after, &[], true, DEFAULT_CACHE_BUDGET, 2);
    std::mem::swap(&mut pools, &mut after);
    assert!(pools.retained.iter().any(|t| t.cache == Some((7, 1))), "a closed cache is kept for its node");
    let decisions = frame(&mut planner, &pools, &mut after, &scenes[0], true, DEFAULT_CACHE_BUDGET, 3);
    assert!(matches!(decisions[0], Disposition::Cached { .. }), "the frame that closes and opens again hits it");
}

/// The plan reads a pool in place until it first moves a target, and after that through its own
/// order: every sequence of parks and takes, from pools of every small size, leaves the places
/// where `Vec::push` and `Vec::swap_remove` would, which is what `commit` replays on the pool.
#[test]
fn a_pool_view_moves_its_targets_as_a_vec_does() {
    fn walk(view: &View, model: &[Source], fresh: usize, depth: u32) {
        assert_eq!(view.len, model.len());
        for (place, &source) in model.iter().enumerate() { assert_eq!(view.source(place), source, "{model:?}"); }
        if depth == 0 { return; }
        let (mut parked, mut pushed) = (view.clone(), model.to_vec());
        parked.push(Source::Slot(fresh));
        pushed.push(Source::Slot(fresh));
        walk(&parked, &pushed, fresh + 1, depth - 1);
        for at in 0..model.len() {
            let (mut view, mut model) = (view.clone(), model.to_vec());
            assert_eq!(view.swap_remove(at), model.swap_remove(at));
            walk(&view, &model, fresh + 1, depth - 1);
        }
    }
    for len in 0..4 {
        let mut view = View::new(Source::Spare);
        view.reset(len);
        let model: Vec<Source> = (0..len).map(Source::Spare).collect();
        walk(&view, &model, 0, 6);
    }
}

/// A cached leaf that draws something can't be made again cheaply, so over budget dev let its kept
/// texture go rather than borrow it; a leaf that draws nothing borrows it. Neither has a child layer.
#[test]
fn a_cached_leaf_with_a_draw_keeps_its_place_under_the_budget() {
    let scenes = scenes();
    let (mut pools, mut after, mut planner) = (Old::default(), Old::default(), Planner::default());
    frame(&mut planner, &pools, &mut after, &scenes[7], true, DEFAULT_CACHE_BUDGET, 1);
    std::mem::swap(&mut pools, &mut after);
    let (begin, end) = marks(&scenes[7]);
    assert_eq!((end[0] - begin[0], end[1] - begin[1]), (2, 1), "the first leaf draws a rect, the second nothing");
    let decisions = frame(&mut planner, &pools, &mut after, &scenes[7], true, 1, 2);
    assert_eq!(decisions[0], Disposition::Inline, "the leaf with a draw draws again, straight into the window");
    assert!(!after.retained.iter().chain(after.slots.iter().flatten()).any(|t| t.cache == Some((11, 1))), "and its texture goes");
    assert!(matches!(decisions[1], Disposition::Cached { cache: None, .. }), "the empty leaf borrows its texture: {decisions:?}");
}
