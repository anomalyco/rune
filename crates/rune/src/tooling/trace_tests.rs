use std::collections::HashSet;

use super::*;

struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).ok();
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

#[test]
fn tree_json_emits_radius_matching_element_info() {
    let view = |_: &(), _: &mut Cx<()>| {
        crate::col().key("root").size(200.0, 200.0).children([
            crate::el().key("sharp").size(40.0, 40.0),
            crate::el().key("pill").size(80.0, 32.0).radius(12.5),
        ])
    };
    let mut rt = Runtime::new((), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let els = rt.elements();
    let tree = tree_json(&rt, 0.0);
    let tree_els = tree["elements"].as_array().unwrap();
    for (key, expected) in [("sharp", 0.0), ("pill", 12.5)] {
        let info = els.iter().find(|e| e.key.as_deref() == Some(key)).unwrap();
        assert_eq!(info.radius, expected);
        let node = tree_els.iter().find(|e| e["key"].as_str() == Some(key)).unwrap();
        assert_eq!(node["radius"], serde_json::json!(expected), "{key} radius in --tree: {node}");
    }
}

#[test]
fn element_info_and_tree_report_resolved_text_style_and_fallbacks() {
    const INTER: &[u8] = include_bytes!("../../assets/Inter-Regular.ttf");
    const PLEX: &[u8] = include_bytes!("../../assets/IBMPlexMono-Regular.ttf");
    let edit_filled = crate::TextEdit::new("hello\nworld");
    let mut edit_secret = crate::TextEdit::new("top-secret-password");
    edit_secret.set_secure(true);
    type State = (crate::TextEdit, crate::TextEdit);
    fn view((edit_filled, edit_secret): &State, _: &mut Cx<State>) -> El {
        let named_hit = crate::TextStyle::new(15.0, 600.0)
            .family(crate::Family::Named("MissingFont, IBM Plex Mono"))
            .lh(22.5)
            .tracking(0.45)
            .italic();
        let named_miss = crate::TextStyle::new(16.0, 400.0)
            .family(crate::Family::Named("MissingFontOnly"));
        let empty_hit = crate::TextStyle::new(14.0, 500.0)
            .family(crate::Family::Named("MissingFont, IBM Plex Mono"));
        let empty_miss = crate::TextStyle::new(12.0, 400.0)
            .family(crate::Family::Named("MissingFontOnly"));
        let rich_style = crate::TextStyle::new(13.0, 500.0).mono().tracking(-0.25);
        let input_style = crate::TextStyle::new(14.0, 400.0).family(crate::Family::Named("MissingFont, IBM Plex Mono"));
        let secret_style = crate::TextStyle::new(13.0, 400.0).family(crate::Family::Named("MissingFont, IBM Plex Mono"));
        crate::col().key("root").size(400.0, 400.0).children([
            crate::el().key("plain-box").size(30.0, 20.0).radius(6.0),
            crate::text("Hit").key("named-hit").font(named_hit),
            crate::text("Fallback").key("named-miss").font(named_miss),
            crate::text("").key("empty-hit").font(empty_hit),
            crate::text("").key("empty-miss").font(empty_miss),
            crate::rich("Rich span", vec![crate::Span::new(0, 4)]).key("rich-node").font(rich_style),
            crate::text_input(edit_filled, input_style).key("para-input").dir_by_paragraph(),
            crate::text_input(edit_secret, secret_style).key("secret-input"),
        ])
    }
    let text = crate::text::TextSystem::new(&[INTER, PLEX], ("Inter", "IBM Plex Mono"));
    let mut rt = Runtime::with_text((edit_filled, edit_secret), view, (400.0, 400.0), Env::new(true, 1.0), text);
    rt.frame(0.0);

    let els = rt.elements();
    let tree = tree_json(&rt, 0.0);
    let tree_els = tree["elements"].as_array().unwrap();
    let el = |key: &str| els.iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("missing {key}"));
    let json_el = |key: &str| tree_els.iter().find(|e| e["key"].as_str() == Some(key)).unwrap_or_else(|| panic!("missing json {key}"));

    // Non-text element has text_style: None and omits textStyle in --tree.
    assert_eq!(el("plain-box").text_style, None);
    assert!(json_el("plain-box").get("textStyle").is_none(), "non-text must omit textStyle: {}", json_el("plain-box"));
    assert_eq!(json_el("plain-box")["radius"], serde_json::json!(6.0));

    // MissingFont, IBM Plex Mono resolves to IBM Plex Mono; explicit line_height, tracking, italic match.
    assert_eq!(
        el("named-hit").text_style,
        Some(crate::ResolvedTextStyle {
            family: Some("IBM Plex Mono".into()),
            size: 15.0,
            weight: 600.0,
            line_height: 22.5,
            tracking: 0.45,
            italic: true,
        }),
    );
    assert_eq!(
        json_el("named-hit")["textStyle"],
        serde_json::json!({
            "family": "IBM Plex Mono",
            "size": 15.0,
            "weight": 600.0,
            "lineHeight": 22.5,
            "tracking": 0.45,
            "italic": true,
        }),
    );

    // MissingFontOnly falls back to sans slot (Inter); auto line_height = 1.25 * size = 20.0.
    assert_eq!(
        el("named-miss").text_style,
        Some(crate::ResolvedTextStyle {
            family: Some("Inter".into()),
            size: 16.0,
            weight: 400.0,
            line_height: 20.0,
            tracking: 0.0,
            italic: false,
        }),
    );
    assert_eq!(
        json_el("named-miss")["textStyle"],
        serde_json::json!({
            "family": "Inter",
            "size": 16.0,
            "weight": 400.0,
            "lineHeight": 20.0,
            "tracking": 0.0,
            "italic": false,
        }),
    );

    // Empty text still resolves family via the fallback stack.
    assert_eq!(el("empty-hit").text_style.as_ref().and_then(|s| s.family.as_deref()), Some("IBM Plex Mono"));
    assert_eq!(json_el("empty-hit")["textStyle"]["family"], "IBM Plex Mono");
    assert_eq!(el("empty-miss").text_style.as_ref().and_then(|s| s.family.as_deref()), Some("Inter"));
    assert_eq!(json_el("empty-miss")["textStyle"]["family"], "Inter");

    // Rich text and by-paragraph text_input report resolved text style.
    assert_eq!(el("rich-node").text_style.as_ref().and_then(|s| s.family.as_deref()), Some("IBM Plex Mono"));
    assert_eq!(json_el("rich-node")["textStyle"]["tracking"], -0.25);
    assert_eq!(el("para-input").text_style.as_ref().and_then(|s| s.family.as_deref()), Some("IBM Plex Mono"));
    assert_eq!(json_el("para-input")["textStyle"]["family"], "IBM Plex Mono");

    // Secure text_input resolves family without leaking plaintext.
    assert_eq!(el("secret-input").text, None);
    assert_eq!(el("secret-input").text_style.as_ref().and_then(|s| s.family.as_deref()), Some("IBM Plex Mono"));
    assert_eq!(json_el("secret-input")["textStyle"]["family"], "IBM Plex Mono");
    assert!(!tree.to_string().contains("top-secret-password"));
}

#[test]
fn trace_records_matched_handover_with_ids_and_rects() {
    fn view(open: &bool, _: &mut Cx<bool>) -> El {
        let card = |w: f32, h: f32| {
            crate::el().key("card").matched("card").size(w, h).fill(crate::Color::WHITE)
        };
        let mut root = crate::el().key("root").size(400.0, 300.0);
        if *open {
            root = root.child(card(160.0, 80.0).absolute(120.0, 20.0));
        } else {
            root = root.child(crate::el().key("board").size(400.0, 300.0).child(card(100.0, 50.0).absolute(40.0, 40.0)));
        }
        root
    }

    let tmp = TempFile(std::env::temp_dir().join(format!("rune-trace-handover-{}.jsonl", std::process::id())));
    let mut rt = Runtime::new(false, view, (400.0, 300.0), Env { scale: 1.0, system_dark: true });
    {
        let mut trace = Trace::open(&tmp.0, rt.viewport, 120.0, 1.0);
        trace.all = true;
        rt.frame(0.0);
        trace.frame(&mut rt, 0.0, 0.0);
        rt.frame(1.0 / 120.0);
        trace.frame(&mut rt, 1000.0 / 120.0, 0.0);
        rt.update(|open, _| *open = true);
        rt.frame(1.0 / 120.0);
        trace.frame(&mut rt, 2000.0 / 120.0, 0.0);
        rt.frame(1.0 / 120.0);
        trace.frame(&mut rt, 3000.0 / 120.0, 0.0);
    }

    let text = std::fs::read_to_string(&tmp.0).unwrap();
    let frames: Vec<Value> = text.lines().skip(1).map(|l| serde_json::from_str(l).unwrap()).collect();
    assert!(frames[0].get("handover").is_none());
    assert!(frames[1].get("handover").is_none());
    assert!(frames[3].get("handover").is_none());
    let hos = frames[2].get("handover").and_then(Value::as_array).expect("handover recorded on frame 2");
    assert_eq!(hos.len(), 1);
    let from_id = hos[0]["from"].as_u64().expect("from id");
    let to_id = hos[0]["to"].as_u64().expect("to id");
    assert_ne!(from_id, to_id);
    let gone: Vec<u64> = frames[2]["g"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    let born: Vec<u64> = frames[2]["m"].as_array().unwrap().iter().map(|v| v["id"].as_u64().unwrap()).collect();
    assert!(gone.contains(&from_id), "from_id {from_id} must be in g: {gone:?}");
    assert!(born.contains(&to_id), "to_id {to_id} must be in m: {born:?}");
    assert_eq!(hos[0]["from_rect"].as_array().unwrap().len(), 4);
    assert_eq!(hos[0]["to_rect"].as_array().unwrap().len(), 4);
}

#[test]
fn tree_json_emits_material_fallback_only_when_exceeding_cap() {
    let view = |_: &(), _: &mut Cx<()>| {
        crate::el().key("root").size(240.0, 120.0)
            .children((0..crate::MAX_NATIVE_MATERIALS + 1).map(|k| {
                crate::el()
                    .key(format!("mat-{k}"))
                    .absolute(k as f32 * 30.0, 0.0)
                    .size(24.0, 24.0)
                    .system_material(if k == 0 { crate::SystemMaterial::Sidebar } else { crate::SystemMaterial::Glass })
            }))
            .child(
                crate::el()
                    .key("mat-dim")
                    .absolute(180.0, 0.0)
                    .size(24.0, 24.0)
                    .dim(0.0)
                    .system_material(crate::SystemMaterial::Popover),
            )
    };
    let mut rt = Runtime::new((), view, (240.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let tree = tree_json(&rt, 0.0);
    let tree_els = tree["elements"].as_array().unwrap();
    let find_json = |key: &str| tree_els.iter().find(|e| e["key"].as_str() == Some(key)).unwrap();
    for k in 0..crate::MAX_NATIVE_MATERIALS {
        let node = find_json(&format!("mat-{k}"));
        assert!(node.get("materialFallback").is_none(), "mat-{k} within cap must omit materialFallback: {node}");
    }
    assert_eq!(find_json("mat-4")["materialFallback"], serde_json::json!(true));
    assert!(find_json("mat-dim").get("materialFallback").is_none(), "dimmed material must omit materialFallback");

    // Raising the cap clears materialFallback from --tree on the next frame.
    rt.set_max_native_materials(6);
    rt.frame(0.0);
    let raised = tree_json(&rt, 0.0);
    let raised_els = raised["elements"].as_array().unwrap();
    let fifth = raised_els.iter().find(|e| e["key"].as_str() == Some("mat-4")).unwrap();
    assert!(fifth.get("materialFallback").is_none(), "mat-4 within raised cap must omit materialFallback: {fifth}");
}

#[test]
fn tree_json_emits_description_and_border_color_when_present() {
    const ACCENT: crate::Color = crate::Color { r: 0.2, g: 0.4, b: 0.8, a: 1.0 };
    let view = |_: &(), _: &mut Cx<()>| {
        crate::col().key("root").size(240.0, 120.0)
            .child(crate::el().key("plain").size(40.0, 20.0))
            .child(crate::el().key("described").size(40.0, 20.0).description("2 running").help("Tooltip"))
            .child(crate::el().key("empty-desc").size(40.0, 20.0).description(""))
            .child(crate::el().key("bordered").size(40.0, 20.0).border(1.5, ACCENT))
    };
    let mut rt = Runtime::new((), view, (240.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let tree = tree_json(&rt, 0.0);
    let tree_els = tree["elements"].as_array().unwrap();
    let find_json = |key: &str| tree_els.iter().find(|e| e["key"].as_str() == Some(key)).unwrap();

    assert!(find_json("plain").get("description").is_none());
    assert!(find_json("plain").get("borderColor").is_none());
    assert_eq!(find_json("described")["description"], serde_json::json!("2 running"));
    assert_eq!(find_json("described")["help"], serde_json::json!("Tooltip"));
    assert!(find_json("empty-desc").get("description").is_none(), "empty description must omit JSON field");
    assert_eq!(find_json("bordered")["borderColor"], serde_json::json!(ACCENT.hex()));
}

#[test]
fn tree_json_reports_the_pointer_and_the_cursor_on_the_element_it_is_read_from() {
    use crate::Cursor;
    let view = |_: &(), _: &mut Cx<()>| {
        crate::col().key("root").size(200.0, 200.0).children([
            crate::el().key("handle").size(200.0, 40.0).hit().cursor(Cursor::ResizeColumn),
            crate::el().key("button").size(200.0, 40.0).on_press(|_: &mut ()| {}),
        ])
    };
    let mut rt = Runtime::new((), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let cursor_of = |tree: &serde_json::Value, key: &str| {
        tree["elements"].as_array().unwrap().iter().find(|e| e["key"].as_str() == Some(key)).unwrap()["cursor"].clone()
    };
    // Before the pointer has been anywhere there is nothing to say.
    let tree = tree_json(&rt, 0.0);
    assert!(tree.get("pointer").is_none(), "{}", tree["pointer"]);

    rt.pointer_move(100.0, 20.0);
    let tree = tree_json(&rt, 0.0);
    assert_eq!(tree["pointer"]["cursor"], "col-resize");
    assert_eq!(tree["pointer"]["at"], serde_json::json!([100.0, 20.0]));
    assert_eq!(cursor_of(&tree, "handle"), "col-resize");
    assert!(cursor_of(&tree, "button").is_null(), "only the element under the pointer carries it");

    // An element with no cursor of its own, under no ancestor that sets one, reports the arrow.
    rt.pointer_move(100.0, 60.0);
    let tree = tree_json(&rt, 0.0);
    assert_eq!(tree["pointer"]["cursor"], "default");
    assert_eq!(cursor_of(&tree, "button"), "default");
    assert!(cursor_of(&tree, "handle").is_null());
}

/// A sink that holds every write until its gate opens, as a disk that stalls does, and says which
/// thread wrote each piece.
#[cfg(not(target_arch = "wasm32"))]
struct GatedSink {
    gate: std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    written: std::sync::Arc<std::sync::Mutex<Vec<(std::thread::ThreadId, Vec<u8>)>>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl std::io::Write for GatedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let (open, opened) = &*self.gate;
        let mut open = open.lock().unwrap();
        while !*open {
            open = opened.wait(open).unwrap();
        }
        self.written.lock().unwrap().push((std::thread::current().id(), buf.to_vec()));
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn trace_lines_are_written_by_another_thread_and_a_stalled_disk_holds_no_frame() {
    use std::io::Write;
    let gate = std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let written = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let out = TraceOut::new(GatedSink { gate: gate.clone(), written: written.clone() });

    // The frames' thread: three lines while the sink can write nothing. A writer that wrote on
    // this thread would never answer; the test then opens the gate and fails instead of hanging.
    let (wrote, frames_done) = std::sync::mpsc::channel();
    let frames = std::thread::spawn(move || {
        let mut out = out;
        for k in 0..3 {
            writeln!(out, r#"{{"t":{k},"n":{}}}"#, k + 1).unwrap();
        }
        wrote.send(std::thread::current().id()).unwrap();
        out
    });
    let frames_thread = frames_done.recv_timeout(std::time::Duration::from_secs(10));
    let stalled = written.lock().unwrap().len();
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    let mut out = frames.join().unwrap();
    let frames_thread = frames_thread.expect("three lines were handed over while the sink was stalled");
    assert_eq!(stalled, 0, "nothing can have reached a stalled sink");

    out.flush().unwrap();
    let written = written.lock().unwrap();
    let text: Vec<u8> = written.iter().flat_map(|(_, bytes)| bytes.clone()).collect();
    assert_eq!(String::from_utf8(text).unwrap(), "{\"t\":0,\"n\":1}\n{\"t\":1,\"n\":2}\n{\"t\":2,\"n\":3}\n");
    assert_eq!(written.len(), 3, "one write a line, whole lines");
    assert!(written.iter().all(|(thread, _)| *thread != frames_thread && *thread != std::thread::current().id()), "the sink is written by the writer's own thread");
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn trace_file_holds_every_line_after_a_flush_and_after_the_drop() {
    use std::io::Write;
    let tmp = TempFile(std::env::temp_dir().join(format!("rune-trace-out-{}.jsonl", std::process::id())));
    let mut out = TraceOut::new(std::fs::File::create(&tmp.0).unwrap());
    writeln!(out, "first").unwrap();
    out.flush().unwrap();
    assert_eq!(std::fs::read_to_string(&tmp.0).unwrap(), "first\n", "a flush waits for the file");
    writeln!(out, "second").unwrap();
    write!(out, "an unfinished line").unwrap();
    drop(out);
    assert_eq!(std::fs::read_to_string(&tmp.0).unwrap(), "first\nsecond\nan unfinished line", "the drop writes what is left and waits for it");
}

#[test]
fn settled_trace_file_holds_every_frame_while_the_trace_is_still_open() {
    // A `--frames` run ends by a fast exit that drops nothing: the settle is its last chance.
    let view = |_: &bool, _: &mut Cx<bool>| crate::el().key("box").size(20.0, 20.0);
    let mut rt = Runtime::new(false, view, (100.0, 100.0), Env { scale: 1.0, system_dark: true });
    let tmp = TempFile(std::env::temp_dir().join(format!("rune-trace-settled-{}.jsonl", std::process::id())));
    let mut trace = Trace::open(&tmp.0, rt.viewport, 60.0, 1.0);
    for n in 0..40u32 {
        rt.frame(1.0 / 60.0);
        trace.frame(&mut rt, f64::from(n) * 1000.0 / 60.0, 0.0);
    }
    trace.settle_presents(std::time::Duration::ZERO);
    let text = std::fs::read_to_string(&tmp.0).unwrap();
    assert_eq!(text.lines().count(), 41, "the header and all forty frames, before the trace is dropped");
    assert!(text.ends_with('\n'), "whole lines");
    drop(trace);
}
