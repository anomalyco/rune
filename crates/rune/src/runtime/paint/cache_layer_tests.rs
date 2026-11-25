use super::*;

#[derive(Clone, Copy, Debug)]
enum Event { Paint, Zoom, CommitZoom, Wait, LoseRaster, Settle, Overflow, Nan }

#[derive(Clone, Copy)]
struct OldEntry {
    raster_scale: f32, seen_scale: f32, scale_changed_at: f64, env_scale: f32,
    was_live: bool, glyphs: usize, commit_gen: u64, paint_state_hash: u64,
}

#[derive(Clone, Copy, Default)]
struct Old { entry: Option<OldEntry>, static_scale: Option<(f32, f64)> }

// One cached solid rect: no live bindings, animated ancestors or checked descendants.
// The paint hash is constant. The renderer reports residency separately for each paint.
impl Old {
    fn paint(&mut self, eff_scale: f32, now: f64, commit_gen: u64, settled: bool, is_resident: bool) -> bool {
        let mut static_scale_settling = false;
        let moving = if eff_scale != 1.0 || self.static_scale.is_some() {
            let seen = self.static_scale.get_or_insert((eff_scale, f64::NEG_INFINITY));
            if settled {
                *seen = (eff_scale, f64::NEG_INFINITY);
            } else if (seen.0 - eff_scale).abs() > 1e-5 {
                *seen = (eff_scale, now);
            }
            let moving = now - seen.1 < 0.1;
            static_scale_settling |= moving;
            moving
        } else { false };
        let mut committed_scale_moving = false;
        if let Some(entry) = &mut self.entry && entry.commit_gen != commit_gen && (eff_scale - entry.raster_scale).abs() > 1e-4 {
            if (eff_scale - entry.seen_scale).abs() > 1e-4 {
                entry.seen_scale = eff_scale;
                entry.scale_changed_at = now;
            }
            committed_scale_moving = now - entry.scale_changed_at < 0.1;
            if committed_scale_moving {
                entry.was_live = true;
                static_scale_settling = true;
            }
        }
        // Dev only opens a cache whose estimated bounds fit in 4096 px; this 32 px box
        // estimates at `32 * scale`, so an infinite, NaN or enormous scale opens none, and
        // the entry stays as it was.
        if !committed_scale_moving && (32.0 * eff_scale).ceil() <= 4096.0 {
            let (raster_scale, seen_scale, scale_changed_at, elided, _) = if let Some(entry) = self.entry {
                let env_same = (1.0 - entry.env_scale).abs() <= 1e-4 && !settled;
                let (seen_scale, scale_changed_at) = if !env_same {
                    (eff_scale, f64::NEG_INFINITY)
                } else if (eff_scale - entry.seen_scale).abs() > 1e-4 {
                    (eff_scale, now)
                } else { (entry.seen_scale, entry.scale_changed_at) };
                let scale_diff = (eff_scale - entry.raster_scale).abs() > 1e-4;
                let same_commit = commit_gen == entry.commit_gen;
                let scale_settling = same_commit && scale_diff && env_same && is_resident && !entry.was_live
                    && (now - scale_changed_at < 0.1 || moving);
                static_scale_settling |= scale_settling;
                let content_same = same_commit && (!scale_diff || scale_settling);
                let can_elide = is_resident && !entry.was_live && content_same && 0 == entry.paint_state_hash;
                let raster_scale = if can_elide && scale_settling { entry.raster_scale } else { eff_scale };
                (raster_scale, seen_scale, scale_changed_at, can_elide, entry.glyphs)
            } else { (eff_scale, eff_scale, f64::NEG_INFINITY, false, 0) };
            if elided {
                let entry = self.entry.as_mut().unwrap();
                entry.seen_scale = seen_scale;
                entry.scale_changed_at = scale_changed_at;
            } else {
                self.entry = Some(OldEntry { raster_scale, seen_scale, scale_changed_at, env_scale: 1.0,
                    was_live: false, glyphs: 0, commit_gen, paint_state_hash: 0 });
            }
        }
        static_scale_settling
    }
}

fn fields(scale: ScaleSettle) -> (f32, f64) {
    match scale {
        ScaleSettle::Still(s) => (s, f64::NEG_INFINITY),
        ScaleSettle::Settling { scale, since } => (scale, since),
    }
}

/// Bit patterns, so an infinite or NaN observation compares equal to itself.
fn bits((scale, since): (f32, f64)) -> (u32, u64) {
    (scale.to_bits(), since.to_bits())
}

#[test]
fn every_paint_cache_walk_matches_dev() {
    let mut rt = Runtime::new((), |_, _| crate::el().key("cache").size(32.0, 32.0).cache().fill(Color::WHITE),
        (64.0, 64.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let i = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("cache")).unwrap();
    let id = rt.tree.nodes[i].id;
    rt.paint.node_caches.clear();
    rt.paint.static_scales.clear();
    fn walk(rt: &mut Runtime<()>, i: usize, id: NodeId, old: Old, scale: f32, now: f64, generation: u64, depth: u32) {
        if depth == 5 { return; }
        let saved = rt.paint.node_caches.clone();
        let tracked = rt.paint.static_scales.clone();
        for event in [Event::Paint, Event::Zoom, Event::CommitZoom, Event::Wait, Event::LoseRaster, Event::Settle, Event::Overflow, Event::Nan] {
            rt.paint.node_caches.clone_from(&saved);
            rt.paint.static_scales.clone_from(&tracked);
            let mut old = old;
            // An overflowing commit reaches the cache at an infinite scale; the paints after
            // it return to a size the raster path can open at, so the reference's always-opens
            // model stays the one dev had.
            let commit = matches!(event, Event::CommitZoom | Event::Overflow | Event::Nan);
            let finite = if (-4.0..=4.0).contains(&scale) { scale } else { 1.5 };
            let scale = match event {
                Event::Zoom => 3.0 - finite,
                Event::CommitZoom => 3.0 - finite,
                Event::Overflow => f32::MAX,
                Event::Nan => f32::NAN,
                _ => finite,
            };
            let now = now + if matches!(event, Event::Wait) { 0.1 } else { 0.03125 };
            let generation = generation + commit as u64;
            let settled = matches!(event, Event::Settle);
            let resident = !matches!(event, Event::LoseRaster);
            // A nonzero cached glyph count makes reuse distinguishable from repainting this rect.
            rt.text.atlas.resident_caches.clear();
            if let Some(entry) = rt.paint.node_caches.get_mut(&id) {
                entry.glyphs = 17;
                if resident { rt.text.atlas.resident_caches.insert(id.0, entry.signature); }
            }
            if let Some(entry) = &mut old.entry { entry.glyphs = 17; }
            rt.tree.nodes[i].static_scale = scale;
            rt.t = now;
            rt.settled = settled;
            rt.commit.set_commit_gen(generation);
            rt.build();
            let expected_frame = old.paint(scale, now, generation, settled, resident);
            assert_eq!(rt.paint.scale_frame.is_some(), expected_frame, "{event:?} at {now}");
            assert_eq!(rt.paint.static_scales.get(&id).copied().map(fields).map(bits), old.static_scale.map(bits));
            match (old.entry, rt.paint.node_caches.get(&id)) {
                (Some(e), Some(a)) => assert_eq!(
                    (a.raster_scale.to_bits(), bits(fields(a.scale)), a.env_scale.to_bits(), matches!(a.phase, RasterPhase::Live), a.glyphs, a.commit_gen),
                    (e.raster_scale.to_bits(), bits((e.seen_scale, e.scale_changed_at)), e.env_scale.to_bits(), e.was_live, e.glyphs, e.commit_gen),
                    "{event:?} at {now}"),
                (None, None) => {},
                (e, a) => panic!("{event:?} at {now}: a cache entry survived on one side only: {} / {}", e.is_some(), a.is_some()),
            }
            walk(rt, i, id, old, scale, now, generation, depth + 1);
        }
    }
    walk(&mut rt, i, id, Old::default(), 1.0, 0.0, 1, 0);
}

#[test]
fn a_scale_that_overflows_at_two_x_stops_asking_for_frames() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        crate::el().key("cache").size(32.0, 32.0).cache().fill(Color::WHITE)
    }
    // Committing `f32::MAX` on a 2× display overflows the scale the cache sees to infinity,
    // so an unchanged paint compares `∞ - ∞`, which is NaN.
    let mut rt = Runtime::new((), view, (64.0, 64.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let i = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("cache")).unwrap();
    let id = rt.tree.nodes[i].id;
    assert!(rt.paint.node_caches.contains_key(&id), "a cached raster to reuse");
    rt.tree.nodes[i].static_scale = f32::MAX;
    let (mut now, mut generation, mut frames) = (1.0, rt.commit.commit_gen(), 0);
    // A drag commits every frame.
    while now < 1.5 {
        now += 1.0 / 60.0;
        generation += 1;
        rt.t = now;
        rt.commit.set_commit_gen(generation);
        rt.build();
        if rt.paint.scale_frame.is_none() { break; }
        frames += 1;
    }
    assert!(frames < 12, "frames stop after the settle window, not every frame: {frames}");
    for _ in 0..5 {
        now += 1.0 / 60.0;
        generation += 1;
        rt.t = now;
        rt.commit.set_commit_gen(generation);
        rt.build();
        assert!(rt.paint.scale_frame.is_none(), "and stay stopped");
    }
}

#[test]
fn an_el_scale_animation_stops_asking_for_frames_when_it_lands() {
    fn view(k: &f32, _: &mut Cx<f32>) -> El {
        crate::col().size(64.0, 64.0).child(
            crate::el().key("box").size(32.0, 32.0).scale(*k).fill(Color::WHITE),
        )
    }
    let mut rt = Runtime::new(0.9f32, view, (64.0, 64.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("box")).unwrap().id;

    // Spring from 0.9 to 1.0: stops asking for frames on the frame P_SCALE lands.
    rt.update(|k, _| *k = 1.0);
    let mut animating_frames = 0usize;
    let mut landed_animating = None;
    for _ in 0..240 {
        let wants = rt.frame(1.0 / 60.0);
        if rt.scene.store().is_animating(id, P_SCALE) {
            animating_frames += 1;
        } else {
            landed_animating = Some(wants);
            break;
        }
    }
    assert!(animating_frames > 0, "P_SCALE spring animated before landing");
    assert_eq!(rt.scene.get(id, P_SCALE), Some(1.0));
    let mut tail_frames = usize::from(landed_animating.expect("P_SCALE landed"));
    while tail_frames < 30 && rt.frame(1.0 / 60.0) {
        tail_frames += 1;
    }
    assert_eq!(tail_frames, 0, "no trailing scale_settle frames after El::scale lands at 1.0");
    assert!(rt.paint.scale_frame.is_none());

    // Spring from 1.0 to 0.5 (whole-pixel scaled box): also lands with zero trailing frames.
    rt.update(|k, _| *k = 0.5);
    let mut animating_frames = 0usize;
    let mut landed_animating = None;
    for _ in 0..240 {
        let wants = rt.frame(1.0 / 60.0);
        if rt.scene.store().is_animating(id, P_SCALE) {
            animating_frames += 1;
        } else {
            landed_animating = Some(wants);
            break;
        }
    }
    assert!(animating_frames > 0, "P_SCALE spring animated toward 0.5");
    assert_eq!(rt.scene.get(id, P_SCALE), Some(0.5));
    let mut tail_frames = usize::from(landed_animating.expect("P_SCALE landed at 0.5"));
    while tail_frames < 30 && rt.frame(1.0 / 60.0) {
        tail_frames += 1;
    }
    assert_eq!(tail_frames, 0, "no trailing scale_settle frames after El::scale lands at 0.5");

    // A spring that finishes in a single large frame step also leaves no settle tail.
    rt.update(|k, _| *k = 1.0);
    assert!(!rt.frame(10.0), "single-step completion requests no follow-up frame");
    assert!(rt.paint.scale_frame.is_none());
}

#[test]
fn a_cached_el_scale_animation_stops_asking_for_frames_when_it_lands() {
    fn self_cached(k: &f32, _: &mut Cx<f32>) -> El {
        crate::col().size(64.0, 64.0).child(
            crate::el().key("scaled").size(32.0, 32.0).scale(*k).cache().fill(Color::WHITE),
        )
    }
    fn child_cached(k: &f32, _: &mut Cx<f32>) -> El {
        crate::col().size(64.0, 64.0).child(
            crate::el().key("scaled").size(64.0, 64.0).scale(*k).child(
                crate::el().key("cache").size(32.0, 32.0).cache().fill(Color::WHITE),
            ),
        )
    }
    for (name, view) in [("self", self_cached as fn(&f32, &mut Cx<f32>) -> El), ("child", child_cached)] {
        let mut rt = Runtime::new(0.9f32, view, (64.0, 64.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("scaled")).unwrap().id;
        rt.update(|k, _| *k = 1.0);
        let mut animating_frames = 0usize;
        let mut landed_animating = None;
        for _ in 0..240 {
            let wants = rt.frame(1.0 / 60.0);
            if rt.scene.store().is_animating(id, P_SCALE) {
                animating_frames += 1;
            } else {
                landed_animating = Some(wants);
                break;
            }
        }
        assert!(animating_frames > 0, "{name}: P_SCALE spring animated");
        assert_eq!(landed_animating, Some(false), "{name}: no trailing settle frames after landing");
        assert!(rt.paint.scale_frame.is_none(), "{name}: scale_frame cleared on landing");
    }
}

#[test]
fn an_app_stepped_el_scale_keeps_its_settle_window() {
    fn view(k: &f32, _: &mut Cx<f32>) -> El {
        crate::col().size(64.0, 64.0).child(
            crate::el().key("box").size(32.0, 32.0).scale(*k).scale_motion(rune_motion::Curve::Instant).fill(Color::WHITE),
        )
    }
    let mut rt = Runtime::new(0.9f32, view, (64.0, 64.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|k, _| *k = 1.0);
    assert!(rt.frame(1.0 / 60.0), "instant scale step enters the settle window");
    assert!(rt.paint.scale_settling());
    let mut frames = 1usize;
    while frames < 30 && rt.frame(1.0 / 60.0) {
        frames += 1;
    }
    assert!((2..=12).contains(&frames), "settle window runs for ~0.1 s then stops: {frames}");
}

/// A material element under `El::cache()` keeps its rect in `Runtime::system_materials` on
/// frames where only something outside the cache moves, with native materials off. The control
/// (a plain fill in the same place) shows that the cache would otherwise reuse its raster on
/// those frames, skipping the paint that lists material rects.
#[test]
fn a_material_under_a_cache_stays_listed_while_something_else_animates() {
    fn view(s: &(f32, bool), _: &mut Cx<(f32, bool)>) -> El {
        let card = crate::el().absolute(30.0, 20.0).size(120.0, 60.0).radius(18.0);
        let card = if s.1 { card.system_material(crate::SystemMaterial::Glass) } else { card.fill(Color::WHITE) };
        crate::el().size(200.0, 120.0)
            .child(crate::el().key("cache").size(200.0, 100.0).cache().child(card))
            .child(crate::el().key("dot").absolute(s.0, 100.0).size(10.0, 10.0).fill(Color::WHITE))
    }
    let run = |material: bool, check: &mut dyn FnMut(&Runtime<(f32, bool)>)| {
        let mut rt = Runtime::new((0.0_f32, material), view, (200.0, 120.0), Env { scale: 2.0, system_dark: true });
        rt.paint.set_native_materials(false);
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        check(&rt);
        rt.update(|s, tx| { s.0 = 150.0; tx.motion(crate::Spring::new(0.3, 0.0)); });
        rt.frame(1.0 / 120.0);
        for _ in 0..3 {
            rt.frame(0.05);
            assert!(rt.animating, "the sibling is still moving");
            check(&rt);
        }
    };
    // Control: with a plain fill the cache keeps a raster across the sibling's motion.
    run(false, &mut |rt: &Runtime<(f32, bool)>| {
        let i = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("cache")).unwrap();
        assert!(rt.paint.node_caches.contains_key(&rt.tree.nodes[i].id), "the plain subtree is cached");
    });
    let mut first = None;
    run(true, &mut |rt: &Runtime<(f32, bool)>| {
        let now = rt.system_materials().to_vec();
        assert_eq!(now.len(), 1, "a cached ancestor must not hide the material's rect");
        assert_eq!(now[0].rect, [60.0, 40.0, 240.0, 120.0]);
        assert_eq!(*first.get_or_insert_with(|| now.clone()), now);
    });
}

/// Proves that a resident `El::cache()` subtree without any `El::system_material` still elides
/// its inner quads while a sibling animates, with zero `PaintKeep` mismatches.
#[test]
fn cached_subtree_without_material_elides_inner_quads_unchanged() {
    fn view(x: &f32, _: &mut Cx<f32>) -> El {
        crate::el().size(200.0, 120.0)
            .child(
                crate::el().key("plain-cache").size(160.0, 80.0).cache()
                    .child(crate::el().absolute(4.0, 4.0).size(40.0, 30.0).fill(Color::WHITE))
                    .child(crate::el().absolute(52.0, 4.0).size(40.0, 30.0).fill(Color::WHITE))
                    .child(crate::el().absolute(100.0, 4.0).size(40.0, 30.0).fill(Color::WHITE)),
            )
            .child(crate::el().key("mover").absolute(*x, 90.0).size(10.0, 10.0).fill(Color::WHITE))
    }
    let mut rt = Runtime::new(0.0_f32, view, (200.0, 120.0), Env { scale: 1.0, system_dark: true });
    rt.frame(0.0);
    let cache_id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("plain-cache")).unwrap().id;
    let first_instances = rt.instances.len();
    assert!(first_instances >= 5, "initial frame paints cached children + layer markers: {first_instances}");
    let sig = rt.paint.node_caches.get(&cache_id).expect("plain subtree must cache").signature;
    rt.text.atlas.resident_caches.insert(cache_id.0, sig);

    rt.update(|x, tx| { *x = 120.0; tx.motion(crate::Spring::new(0.3, 0.0)); });
    rt.frame(1.0 / 60.0);
    assert!(rt.animating);
    assert!(rt.system_materials().is_empty());
    assert!(rt.instances.len() < first_instances, "resident plain cache must elide inner children ({} vs {})", rt.instances.len(), first_instances);
    assert_eq!(rt.paint_keep_mismatches(), 0);
}

/// A resident `El::cache()` subtree holding a fallback-only `El::system_material` (native
/// materials off) must still elide its inner quads when an unrelated sibling changes, while
/// keeping `Runtime::system_materials()` exact and complete.
#[test]
fn cached_subtree_with_fallback_material_reuses_cache_and_keeps_material_list() {
    fn view(x: &f32, _: &mut Cx<f32>) -> El {
        crate::el().size(200.0, 120.0)
            .child(
                crate::el().key("mat-cache").size(160.0, 80.0).cache()
                    .child(crate::el().absolute(10.0, 8.0).size(120.0, 48.0).radius(16.0).system_material(crate::SystemMaterial::Glass))
                    .child(crate::el().absolute(10.0, 60.0).size(40.0, 12.0).fill(Color::WHITE))
                    .child(crate::el().absolute(60.0, 60.0).size(40.0, 12.0).fill(Color::WHITE)),
            )
            .child(crate::el().key("mover").absolute(*x, 96.0).size(10.0, 10.0).fill(Color::WHITE))
    }
    let mut rt = Runtime::new(0.0_f32, view, (200.0, 120.0), Env { scale: 2.0, system_dark: true });
    rt.paint.set_native_materials(false);
    rt.frame(0.0);
    let cache_id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("mat-cache")).unwrap().id;
    let first_instances = rt.instances.len();
    let expected_materials = rt.system_materials().to_vec();
    assert_eq!(expected_materials.len(), 1);
    assert_eq!(expected_materials[0].rect, [20.0, 16.0, 240.0, 96.0]);
    let entry = rt.paint.node_caches.get(&cache_id).expect("fallback-only material subtree must cache");
    rt.text.atlas.resident_caches.insert(cache_id.0, entry.signature);

    // Animate an unrelated sibling outside the cache: inner quads must elide (3 inner quads -> 0),
    // leaving [KIND_LAYER, KIND_LAYER_END, mover] = 3 instances instead of rebuilding all 6.
    rt.update(|x, tx| { *x = 120.0; tx.motion(crate::Spring::new(0.3, 0.0)); });
    rt.frame(1.0 / 60.0);
    assert!(rt.animating);
    assert_eq!(rt.instances.len(), 3, "resident cached subtree with fallback material must elide inner quads (first frame had {first_instances}, got {})", rt.instances.len());
    assert_eq!(rt.system_materials(), expected_materials.as_slice(), "elided cache must still replay its material rect");
    assert_eq!(rt.paint_keep_mismatches(), 0);
}

/// An elided `El::cache()` replays its recorded `MaterialDraw`s translated by the cache's
/// current origin, scaled during scale settle, and modulated by ancestor opacity (omitting draws
/// faded below 0.001). Turning `native_materials` on bypasses the cache so fills skip and shadow
/// holes cut live.
#[test]
fn elided_cached_material_tracks_ancestor_translation_scale_and_opacity() {
    type State = (crate::Live<[f32; 2]>, crate::Live<f32>, crate::Live<f32>);
    fn view(s: &State, _: &mut Cx<State>) -> El {
        crate::el().size(400.0, 240.0).child(
            crate::el().key("wrap").size(200.0, 120.0)
                .live_offset(&s.0)
                .live_scale(&s.1)
                .live_opacity(&s.2)
                .child(
                    crate::el().key("mat-cache").size(160.0, 80.0).cache()
                        .child(crate::el().absolute(10.0, 8.0).size(120.0, 48.0).radius(16.0).system_material(crate::SystemMaterial::Popover))
                        .child(crate::el().absolute(10.0, 60.0).size(40.0, 12.0).fill(Color::WHITE)),
                ),
        )
    }
    let off = crate::Live::new([0.0, 0.0]);
    let scale = crate::Live::new(1.0);
    let opacity = crate::Live::new(1.0);
    let mut rt = Runtime::new((off.clone(), scale.clone(), opacity.clone()), view, (400.0, 240.0), Env { scale: 2.0, system_dark: true });
    rt.paint.set_native_materials(false);
    rt.frame(0.0);
    let cache_id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("mat-cache")).unwrap().id;
    let sig = rt.paint.node_caches.get(&cache_id).expect("cached").signature;
    rt.text.atlas.resident_caches.insert(cache_id.0, sig);

    // 1. Translation via ancestor live_offset: inner quads elide and replayed rect shifts by (dx, dy) * 2.
    off.set([25.0, 15.0]);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.instances.len(), 2, "elided cache emits only [KIND_LAYER, KIND_LAYER_END]");
    assert_eq!(rt.system_materials().len(), 1);
    assert_eq!(rt.system_materials()[0].rect, [70.0, 46.0, 240.0, 96.0]);
    assert_eq!(rt.system_materials()[0].radius, 32.0);
    assert!((rt.system_materials()[0].opacity - 1.0).abs() < 1e-6);

    // 2. Ancestor live_opacity modulates replayed opacity, and 0.0 hides the draw completely.
    opacity.set(0.5);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.system_materials().len(), 1);
    assert!((rt.system_materials()[0].opacity - 0.5).abs() < 1e-6);
    opacity.set(0.0);
    rt.frame(1.0 / 60.0);
    assert!(rt.system_materials().is_empty(), "fully dimmed ancestor suppresses replayed material");
    opacity.set(1.0);

    // 3. Ancestor live_scale during settle elides and scales replayed rect and radius by k = 1.5.
    scale.set(1.5);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.instances.len(), 2, "scale settle elides cached raster");
    assert_eq!(rt.system_materials().len(), 1);
    let m = rt.system_materials()[0];
    assert!((m.rect[2] - 360.0).abs() < 1e-3 && (m.rect[3] - 144.0).abs() < 1e-3, "scaled size: {:?}", m.rect);
    assert!((m.radius - 48.0).abs() < 1e-3, "scaled radius: {}", m.radius);

    // 4. Enabling native_materials bypasses the cache so native views draw live (no fallback fill).
    scale.set(1.0);
    rt.paint.set_native_materials(true);
    rt.frame(1.0 / 60.0);
    assert!(!rt.instances.iter().any(|q| q.params[1] == crate::render::KIND_LAYER && q.params[3] == 1.0),
        "native_materials=true must bypass cached raster and draw live");
    assert_eq!(rt.system_materials().len(), 1);
}

/// When Wayland compositor blur mode (`wayland_blur_materials`) is active, an `El::system_material`
/// inside `El::cache()` draws live so `PaintState::wayland_material_blurs()` stays complete with
/// its exact rect and ancestor clip on every frame, including when switching from a previously
/// cached fallback raster.
#[test]
fn cached_material_stays_in_wayland_blur_region_when_wayland_mode_is_active() {
    fn view(x: &f32, _: &mut Cx<f32>) -> El {
        crate::el().size(200.0, 120.0)
            .child(
                crate::el().key("mat-cache").size(160.0, 80.0).cache()
                    .child(
                        crate::el().absolute(10.0, 8.0).size(120.0, 48.0).radius(16.0)
                            .fill(Color { r: 0.1, g: 0.1, b: 0.12, a: 0.7 })
                            .system_material(crate::SystemMaterial::Popover),
                    ),
            )
            .child(crate::el().key("mover").absolute(*x, 96.0).size(10.0, 10.0).fill(Color::WHITE))
    }
    let mut rt = Runtime::new(0.0_f32, view, (200.0, 120.0), Env { scale: 2.0, system_dark: true });
    // First cache as fallback-only (wayland_blur_materials off).
    rt.frame(0.0);
    let cache_id = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("mat-cache")).unwrap().id;
    let sig = rt.paint.node_caches.get(&cache_id).expect("fallback caches").signature;
    rt.text.atlas.resident_caches.insert(cache_id.0, sig);
    assert!(rt.paint.wayland_material_blurs().is_empty());

    // Turn Wayland blur mode on and animate an unrelated sibling: the cached raster is bypassed
    // and both `system_materials()` and `wayland_material_blurs()` stay populated every frame.
    assert!(rt.paint.set_wayland_blur_materials(true));
    rt.update(|x, tx| { *x = 120.0; tx.motion(crate::Spring::new(0.3, 0.0)); });
    for _ in 0..3 {
        rt.frame(1.0 / 60.0);
        assert!(rt.animating);
        assert_eq!(rt.system_materials().len(), 1);
        assert_eq!(rt.paint.wayland_material_blurs().len(), 1);
        assert_eq!(rt.paint.wayland_material_blurs()[0].rect, [20.0, 16.0, 240.0, 96.0]);
        assert_eq!(rt.paint.wayland_material_blurs()[0].radius, 32.0);
    }
}
