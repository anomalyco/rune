use std::collections::HashSet;

use super::*;

struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn keyed_churn_prunes_dictionaries_and_never_reuses_retired_ids() {
    fn view(n: &u32, _: &mut Cx<u32>) -> El {
        let mut root = crate::col()
            .key("root")
            .size(100.0, 100.0)
            .child(crate::el().key("pinned").size(20.0, 20.0))
            .child(crate::el().key(format!("row-{n}")).size(20.0, 20.0));
        if (n / 20) % 2 == 0 {
            root = root.child(crate::el().key("toggle").size(20.0, 20.0));
        }
        root
    }

    let tmp = TempFile(std::env::temp_dir().join(format!("rune-trace-churn-{}.jsonl", std::process::id())));
    let mut rt = Runtime::new(0u32, view, (100.0, 100.0), Env { scale: 1.0, system_dark: true });
    let mut live_counts = Vec::new();
    {
        let mut trace = Trace::open(&tmp.0, rt.viewport, 60.0, 1.0);
        for n in 0..256u32 {
            rt.update(|state, _| *state = n);
            rt.frame(1.0 / 60.0);
            trace.frame(&mut rt, f64::from(n) * 1000.0 / 60.0, 0.0);
            let live = trace.states.len();
            assert_eq!(trace.ids.len(), live, "frame {n}: ids must track live elements");
            assert_eq!(trace.described.len(), live, "frame {n}: described must track live elements");
            live_counts.push(live);
        }
        // Let any exiting rows finish unmounting so only the final frame's live elements remain.
        rt.frame(2.0);
        trace.frame(&mut rt, 256.0 * 1000.0 / 60.0 + 2000.0, 0.0);
        assert_eq!(trace.states.len(), 4, "settled frame: root, pinned, row-255, toggle");
        assert_eq!(trace.ids.len(), 4, "settled frame: ids");
        assert_eq!(trace.described.len(), 4, "settled frame: described");
        live_counts.push(trace.states.len());
    }

    let text = std::fs::read_to_string(&tmp.0).unwrap();
    let frames: Vec<Value> = text.lines().skip(1).map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(frames.len(), live_counts.len());

    let mut live = HashSet::new();
    let mut retired = HashSet::new();
    let mut described = HashSet::new();
    let mut toggle_ids = HashSet::new();
    for (n, line) in frames.iter().enumerate() {
        let mut frame_metas = HashSet::new();
        if let Some(metas) = line.get("m").and_then(Value::as_array) {
            for meta in metas {
                let id = meta["id"].as_u64().unwrap() as u32;
                assert!(!retired.contains(&id), "frame {n}: retired id {id} reused in m");
                assert!(frame_metas.insert(id), "frame {n}: duplicate id {id} in m");
                if let Some(pv) = meta.get("pv").and_then(Value::as_u64) {
                    assert!(!retired.contains(&(pv as u32)), "frame {n}: retired parent id {pv} in m");
                }
                if meta["nm"].as_str().is_some_and(|nm| nm.ends_with("toggle")) {
                    toggle_ids.insert(id);
                }
                described.insert(id);
            }
        }
        if let Some(elems) = line.get("e").and_then(Value::as_array) {
            for row in elems {
                let id = row[0].as_u64().unwrap() as u32;
                assert!(!retired.contains(&id), "frame {n}: retired id {id} reused in e");
                assert!(described.contains(&id), "frame {n}: id {id} emitted in e before m");
                live.insert(id);
            }
        }
        if let Some(gone) = line.get("g").and_then(Value::as_array) {
            for id in gone {
                let id = id.as_u64().unwrap() as u32;
                assert!(live.remove(&id), "frame {n}: id {id} in g was not live");
                assert!(retired.insert(id), "frame {n}: id {id} retired twice");
            }
        }
        assert_eq!(live.len(), live_counts[n], "frame {n}: live ids must match live elements");
    }
    assert!(live_counts.iter().copied().max().unwrap() < 32, "live element count must stay bounded");
    assert_eq!(toggle_ids.len(), 7, "remounted toggle must receive a fresh id each cycle, got {toggle_ids:?}");
    assert_eq!(retired.len(), 255 + toggle_ids.len() - 1);
    assert_eq!(live.len(), 4);
}

#[test]
fn trace_records_refresh_rate_changes_for_present_summary() {
    struct Presents(std::sync::Mutex<Vec<crate::frame::present_time::Presented>>);
    impl crate::frame::present_time::Source for Presents {
        fn take(&self, out: &mut Vec<crate::frame::present_time::Presented>) { out.append(&mut self.0.lock().unwrap()); }
    }
    let view = |_: &bool, _: &mut Cx<bool>| crate::el().key("box").size(20.0, 20.0);
    let mut rt = Runtime::new(false, view, (100.0, 100.0), Env { scale: 1.0, system_dark: true });
    let tmp = TempFile(std::env::temp_dir().join(format!("rune-trace-hz-{}.jsonl", std::process::id())));
    let start = crate::time::now();
    let source = std::sync::Arc::new(Presents(Default::default()));
    {
        let mut trace = Trace::open_clock(&tmp.0, rt.viewport, 60.0000024, 1.0, "wall");
        trace.record_presents(source.clone(), start);
        let steps = [
            (0.0, 20_000u64, Some(60.0000024)),
            (8.3, 28_333, Some(120.0000048)),
            (25.0, 45_000, Some(119.9999904)), // 16.667 ms gap: missed at 120 Hz, not at 60 Hz
            (33.3, 53_333, Some(120.0)),
        ];
        for (k, (t_ms, present_us, hz)) in steps.into_iter().enumerate() {
            rt.frame(1.0 / 120.0);
            let seq = k as u64 + 1;
            let timing = FrameTiming { target_ms: Some(t_ms), drawable: Some(seq), continuous: k > 0, hz };
            trace.frame_timed(&mut rt, t_ms, 0.0, timing);
            source.0.lock().unwrap().push((seq, Some(start + std::time::Duration::from_micros(present_us))));
        }
    }
    let text = std::fs::read_to_string(&tmp.0).unwrap();
    let frames: Vec<Value> = text.lines().skip(1).map(|l| serde_json::from_str(l).unwrap()).collect();
    assert!(frames[0].get("hz").is_none(), "unchanged rate is omitted: {}", frames[0]);
    assert_eq!(frames[1]["hz"], 120.0, "rate change is recorded on the frame where it settled");
    assert!(frames[2].get("hz").is_none(), "sub-0.01 Hz jitter does not repeat hz: {}", frames[2]);
    assert!(frames[3].get("hz").is_none(), "steady rate is omitted: {}", frames[3]);
    let summary = crate::frame::present_time::Summary::of_trace(&text).unwrap();
    assert_eq!((summary.hz, summary.frames, summary.presented, summary.missed), (120.0, 4, 4, 1));
    assert!(summary.to_string().contains("at 120 Hz") && summary.to_string().contains("1 missed (>12.50 ms)"), "{summary}");
}
