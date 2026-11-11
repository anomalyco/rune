use super::*;

#[derive(Clone, Copy, Debug)]
enum Event { Paint, Zoom, CommitZoom, Wait, LoseRaster, Settle }

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
        if !committed_scale_moving {
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

#[test]
fn every_paint_cache_walk_matches_dev() {
    let mut rt = Runtime::new((), |_, _| crate::el().key("cache").size(32.0, 32.0).cache().fill(Color::WHITE),
        (64.0, 64.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let i = rt.nodes.iter().position(|n| n.key.as_deref() == Some("cache")).unwrap();
    let id = rt.nodes[i].id;
    rt.node_caches.clear();
    rt.static_scales.clear();
    fn walk(rt: &mut Runtime<()>, i: usize, id: NodeId, old: Old, scale: f32, now: f64, generation: u64, depth: u32) {
        if depth == 5 { return; }
        let saved = rt.node_caches.clone();
        let tracked = rt.static_scales.clone();
        for event in [Event::Paint, Event::Zoom, Event::CommitZoom, Event::Wait, Event::LoseRaster, Event::Settle] {
            rt.node_caches.clone_from(&saved);
            rt.static_scales.clone_from(&tracked);
            let mut old = old;
            let scale = if matches!(event, Event::Zoom | Event::CommitZoom) { 3.0 - scale } else { scale };
            let now = now + if matches!(event, Event::Wait) { 0.1 } else { 0.03125 };
            let generation = generation + matches!(event, Event::CommitZoom) as u64;
            let settled = matches!(event, Event::Settle);
            let resident = !matches!(event, Event::LoseRaster);
            // A nonzero cached glyph count makes reuse distinguishable from repainting this rect.
            rt.text.atlas.resident_caches.clear();
            if let Some(entry) = rt.node_caches.get_mut(&id) {
                entry.glyphs = 17;
                if resident { rt.text.atlas.resident_caches.insert(id.0, entry.signature); }
            }
            if let Some(entry) = &mut old.entry { entry.glyphs = 17; }
            rt.nodes[i].static_scale = scale;
            rt.t = now;
            rt.settled = settled;
            rt.commit_gen = generation;
            rt.build();
            let expected_frame = old.paint(scale, now, generation, settled, resident);
            let e = old.entry.unwrap();
            let a = &rt.node_caches[&id];
            assert_eq!(rt.scale_frame.is_some(), expected_frame, "{event:?} at {now}");
            assert_eq!(rt.static_scales.get(&id).copied().map(fields), old.static_scale);
            assert_eq!((a.raster_scale, fields(a.scale), a.env_scale, matches!(a.phase, RasterPhase::Live), a.glyphs, a.commit_gen),
                (e.raster_scale, (e.seen_scale, e.scale_changed_at), e.env_scale, e.was_live, e.glyphs, e.commit_gen));
            walk(rt, i, id, old, scale, now, generation, depth + 1);
        }
    }
    walk(&mut rt, i, id, Old::default(), 1.0, 0.0, 1, 0);
}
