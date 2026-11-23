use std::collections::HashSet;

use crate::{Mount, NodeId, NodeSpec, Rect, Scene, Transition};

use super::*;

fn header() -> TraceHeader {
    TraceHeader { launch: "test".into(), width: 200.0, height: 100.0, hz: 60.0, scale: 1.0 }
}

fn spec(x: f32, y: f32) -> NodeSpec {
    NodeSpec::new(Rect::new(x, y, 20.0, 20.0))
}

/// Extracts all integer values immediately after `prefix` in `s`.
fn ints_after(s: &str, prefix: &str) -> Vec<u32> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find(prefix) {
        rest = &rest[i + prefix.len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            out.push(digits.parse().unwrap());
        }
    }
    out
}

/// Extracts the JSON array body for a top-level field like `"g":[` (non-nested arrays of ints).
fn int_array(line: &str, key: &str) -> Vec<u32> {
    let pat = format!(r#""{key}":["#);
    let Some(start) = line.find(&pat) else { return Vec::new() };
    let rest = &line[start + pat.len()..];
    let end = rest.find(']').unwrap();
    rest[..end].split(',').filter(|s| !s.is_empty()).map(|s| s.parse().unwrap()).collect()
}

/// Extracts the leading element id from each row in `"e":[[id,...],[id,...]]`.
fn elem_ids(line: &str) -> Vec<u32> {
    let pat = r#""e":["#;
    let Some(start) = line.find(pat) else { return Vec::new() };
    let rest = &line[start + pat.len()..];
    let end = rest.find("]]").unwrap() + 1;
    ints_after(&rest[..end], "[")
}

/// Extracts `(id, nm, pv)` from each object in `"m":[...]`.
fn meta_entries(line: &str) -> Vec<(u32, String, Option<u32>)> {
    let pat = r#""m":["#;
    let Some(start) = line.find(pat) else { return Vec::new() };
    let rest = &line[start + pat.len()..];
    let end = rest.find("}]").unwrap() + 1;
    let mut out = Vec::new();
    for obj in rest[..end].split("},{") {
        let id = ints_after(obj, r#""id":"#)[0];
        let nm_pat = r#""nm":""#;
        let nm_start = obj.find(nm_pat).unwrap() + nm_pat.len();
        let nm_rest = &obj[nm_start..];
        let nm_end = nm_rest.find('"').unwrap();
        let nm = nm_rest[..nm_end].to_owned();
        let pv = ints_after(obj, r#""pv":"#).into_iter().next();
        out.push((id, nm, pv));
    }
    out
}

#[test]
fn keyed_churn_prunes_maps_and_never_reuses_retired_ids() {
    let (trace, buf) = MotionTrace::in_memory(&header());
    let mut scene = Scene::new();
    scene.set_trace(Some(trace));

    let root = NodeId(1);
    let pinned = NodeId(2);
    let toggle = NodeId(3);
    scene.insert(root, spec(0.0, 0.0), Mount::Initial);
    scene.trace_name(root, Some("root"));
    scene.trace_namespace(root, Some(("rune", "test")));
    scene.insert(pinned, spec(10.0, 0.0).parent(root), Mount::Initial);
    scene.trace_name(pinned, Some("pinned"));
    scene.trace_namespace(pinned, Some(("rune", "test")));

    let mut live_counts = Vec::new();
    let mut toggle_mounted = false;
    let mut prev_row: Option<NodeId> = None;

    for n in 0..128u64 {
        if let Some(old) = prev_row.take() {
            scene.unmount(old);
        }
        let row = NodeId(1000 + n);
        scene.insert(row, spec(20.0, n as f32).parent(root), Mount::Initial);
        scene.trace_name(row, Some("row"));
        scene.trace_namespace(row, Some(("rune", "test")));
        prev_row = Some(row);

        let want_toggle = (n / 16) % 2 == 0;
        if want_toggle && !toggle_mounted {
            scene.insert(toggle, spec(40.0, 0.0).parent(root), Mount::Initial);
            scene.trace_name(toggle, Some("toggle"));
            scene.trace_namespace(toggle, Some(("rune", "test")));
            toggle_mounted = true;
        } else if !want_toggle && toggle_mounted {
            scene.unmount(toggle);
            toggle_mounted = false;
        }

        scene.frame_at(n as f64 / 60.0);
        let live = scene.len();
        let tr = scene.trace_mut().unwrap();
        assert_eq!(tr.ids.len(), live, "frame {n}: ids must track live nodes");
        assert_eq!(tr.described.len(), live, "frame {n}: described must track live nodes");
        assert_eq!(tr.states.len(), live, "frame {n}: states must track live nodes");
        assert_eq!(tr.names.len(), live, "frame {n}: names must track live nodes");
        assert_eq!(tr.namespaces.len(), live, "frame {n}: namespaces must track live nodes");
        live_counts.push(live);
    }

    drop(scene.take_trace());
    let text = buf.contents();
    let frames: Vec<&str> = text.lines().skip(1).collect();
    assert_eq!(frames.len(), live_counts.len());

    let mut live = HashSet::new();
    let mut retired = HashSet::new();
    let mut described = HashSet::new();
    let mut toggle_ids = HashSet::new();

    for (n, line) in frames.iter().enumerate() {
        let mut frame_metas = HashSet::new();
        for (id, nm, pv) in meta_entries(line) {
            assert!(!retired.contains(&id), "frame {n}: retired id {id} reused in m");
            assert!(frame_metas.insert(id), "frame {n}: duplicate id {id} in m");
            if let Some(pv) = pv {
                assert!(!retired.contains(&pv), "frame {n}: retired parent id {pv} in m");
            }
            if nm == "toggle" {
                toggle_ids.insert(id);
            }
            described.insert(id);
        }
        for id in elem_ids(line) {
            assert!(!retired.contains(&id), "frame {n}: retired id {id} reused in e");
            assert!(described.contains(&id), "frame {n}: id {id} emitted in e before m");
            live.insert(id);
        }
        for id in int_array(line, "g") {
            assert!(live.remove(&id), "frame {n}: id {id} in g was not live");
            assert!(retired.insert(id), "frame {n}: id {id} retired twice");
        }
        assert_eq!(live.len(), live_counts[n], "frame {n}: live ids must match live nodes");
    }

    assert_eq!(toggle_ids.len(), 4, "remounted toggle must get a fresh id each cycle: {toggle_ids:?}");
    assert_eq!(retired.len(), 127 + toggle_ids.len());
    assert_eq!(live.len(), 3, "root, pinned, row-127");
}

#[test]
fn unmounting_an_untraced_node_does_not_allocate_or_emit_a_phantom_id() {
    let (mut trace, buf) = MotionTrace::in_memory(&header());
    let ghost = NodeId(77);
    trace.name(ghost, Some("ghost"));
    trace.namespace(ghost, Some(("rune", "test")));
    trace.frame(0.0, &[], &[ghost], &[], &[]);
    assert!(trace.ids.is_empty());
    assert!(trace.described.is_empty());
    assert!(trace.names.is_empty());
    assert!(trace.namespaces.is_empty());
    assert_eq!(trace.next_id, 1);
    drop(trace);
    let text = buf.contents();
    let lines: Vec<_> = text.lines().collect();
    assert!(!lines[1].contains(r#""g":"#), "never-presented node must not emit g: {}", lines[1]);
}

#[test]
fn animated_exit_prunes_on_final_frame_and_remount_allocates_fresh_id() {
    let (trace, buf) = MotionTrace::in_memory(&header());
    let mut scene = Scene::new();
    scene.set_trace(Some(trace));

    let item = NodeId(42);
    scene.insert(item, spec(0.0, 0.0).transition(Transition::opacity()), Mount::Initial);
    scene.trace_name(item, Some("toast"));
    scene.trace_namespace(item, Some(("rune", "test")));
    scene.frame_at(0.0);

    scene.remove(item);
    let mut t = 0.0;
    while scene.contains(item) {
        t += 1.0 / 60.0;
        scene.frame_at(t);
    }
    {
        let tr = scene.trace_mut().unwrap();
        assert!(tr.ids.is_empty(), "exited node must be pruned from ids");
        assert!(tr.described.is_empty(), "exited node must be pruned from described");
        assert!(tr.states.is_empty(), "exited node must be pruned from states");
        assert!(tr.names.is_empty(), "exited node must be pruned from names");
        assert!(tr.namespaces.is_empty(), "exited node must be pruned from namespaces");
    }

    t += 1.0 / 60.0;
    scene.insert(item, spec(0.0, 0.0).transition(Transition::opacity()), Mount::Initial);
    scene.trace_name(item, Some("toast"));
    scene.frame_at(t);
    drop(scene.take_trace());

    let text = buf.contents();
    let metas: Vec<u32> = text
        .lines()
        .skip(1)
        .flat_map(meta_entries)
        .filter(|(_, nm, _)| nm == "toast")
        .map(|(id, _, _)| id)
        .collect();
    assert_eq!(metas, [1, 2], "remount after animated exit must allocate a new monotonic id");
    let gone: Vec<u32> = text.lines().skip(1).flat_map(|l| int_array(l, "g")).collect();
    assert_eq!(gone, [1]);
}
