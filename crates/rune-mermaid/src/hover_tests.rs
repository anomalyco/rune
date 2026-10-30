use rune::{Color, Cx, El, Env, Runtime, col};

use super::*;
use crate::canvas::Scene;
use crate::{DiagramStyle, scene};

const LIT: Color = Color { r: 0.9, g: 0.2, b: 0.5, a: 1.0 };

fn lit_scene(src: &str) -> Scene {
    scene(src, &DiagramStyle::neutral(false), Some((Hover::Edges, LIT))).expect("draws")
}

/// The index of the label reading `text`.
fn label(s: &Scene, text: &str) -> usize {
    s.labels.iter().position(|l| l.text == text).unwrap_or_else(|| panic!("no label {text:?}"))
}

fn spots<'a>(s: &'a Scene, prefix: &'a str) -> impl Iterator<Item = &'a Spot> + 'a {
    s.hovers.iter().filter(move |h| h.key.starts_with(prefix))
}

fn inside(r: [f32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] && p[0] <= r[0] + r[2] && p[1] >= r[1] && p[1] <= r[1] + r[3]
}

fn centre(s: &Scene, i: usize) -> [f32; 2] {
    let l = &s.labels[i];
    [l.at[0] + l.size[0] / 2.0, l.at[1] + l.size[1] / 2.0]
}

#[test]
fn a_node_lights_its_edges_labels() {
    let s = lit_scene("flowchart TD\n  A -->|go| B");
    let go = label(&s, "go");
    for node in ["node0", "node1"] {
        let spot = spots(&s, node).next().unwrap_or_else(|| panic!("{node} has a hover group"));
        assert_eq!(spot.labels, [go], "{node} lights the label of the edge it lights");
    }
    let st = lit_scene("stateDiagram-v2\n  Idle --> Busy : start");
    let start = label(&st, "start");
    assert!(spots(&st, "node").all(|h| h.labels == [start]));
}

#[test]
fn an_edge_lights_itself_and_its_label_and_nodes_win() {
    let s = lit_scene("stateDiagram-v2\n  Idle --> Busy : start\n  Busy --> Done");
    let start = label(&s, "start");
    let first: Vec<&Spot> = spots(&s, "edge0.").collect();
    assert!(first.len() >= 2, "the line and the label are both hover targets: {}", first.len());
    assert!(first.iter().all(|h| h.labels == [start]), "hovering the edge lights its label");
    let nodes: Vec<[f32; 4]> = spots(&s, "node").map(|h| h.rect).collect();
    let overlap = |a: [f32; 4], b: [f32; 4]| a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3];
    for h in spots(&s, "edge") {
        assert!(h.rect[2] >= 4.0 && h.rect[3] >= 4.0, "{} is big enough to aim at: {:?}", h.key, h.rect);
        assert!(!nodes.iter().any(|&n| overlap(n, h.rect)), "{} stays off every node so the node wins: {:?}", h.key, h.rect);
        assert!(h.rect.iter().all(|v| v.is_finite()));
    }
    let thin = first.iter().map(|h| h.rect[2].min(h.rect[3])).fold(f32::MAX, f32::min);
    assert!(thin >= 2.0 * EDGE_SLOP - 0.5, "the line's slop reaches both sides: {thin}");
    let second: Vec<&Spot> = spots(&s, "edge1.").collect();
    assert!(!second.is_empty() && second.iter().all(|h| h.labels.is_empty()), "an edge without a label lights no label");
}

#[test]
fn a_label_lights_itself_and_its_edge() {
    let s = lit_scene("flowchart LR\n  A -->|go on| B\n  B --> C");
    let go = label(&s, "go on");
    let over: Vec<&Spot> = s.hovers.iter().filter(|h| inside(h.rect, centre(&s, go))).collect();
    assert!(!over.is_empty(), "the label is a hover target");
    assert!(over.iter().all(|h| h.key.starts_with("edge0.") && h.labels == [go]), "it lights the label and its own edge only");
}

#[test]
fn an_unlit_edges_label_stays_unlit() {
    let s = lit_scene("flowchart TD\n  A -->|go| B\n  C -->|stay| D");
    let (go, stay) = (label(&s, "go"), label(&s, "stay"));
    for h in spots(&s, "node0").chain(spots(&s, "node1")).chain(spots(&s, "edge0.")) {
        assert_eq!(h.labels, [go], "{} lights only its own edge's label", h.key);
        assert!(!h.labels.contains(&stay));
    }
    assert!(!s.hovers.iter().any(|h| h.labels.contains(&go) && h.labels.contains(&stay)));
}

#[test]
fn off_draws_no_hover_groups() {
    let s = scene("flowchart TD\n  A -->|go| B", &DiagramStyle::neutral(false), None).unwrap();
    assert!(s.hovers.is_empty());
}

#[test]
fn line_hits_cover_a_bend_and_stay_bounded() {
    let hits = line_hits(&[[0.0, 0.0], [0.0, 100.0], [100.0, 100.0]], EDGE_SLOP);
    assert_eq!(hits.len(), 2, "two straight runs, one rectangle each: {hits:?}");
    let wiggle: Vec<[f32; 2]> = (0..400).map(|i| [i as f32 * 3.0, if i % 2 == 0 { 0.0 } else { 30.0 }]).collect();
    assert!(line_hits(&wiggle, EDGE_SLOP).len() <= 16);
    assert_eq!(cut_away(vec![[0.0, 0.0, 16.0, 100.0]], &[[-10.0, 80.0, 40.0, 40.0]]), [[0.0, 0.0, 16.0, 80.0]]);
    assert!(cut_away(vec![[0.0, 0.0, 16.0, 16.0]], &[[-10.0, -10.0, 40.0, 40.0]]).is_empty());
}

#[test]
fn hovering_an_edge_lights_it_instantly_and_leaving_drops_it() {
    fn view(_: &u32, _: &mut Cx<u32>) -> El {
        let paint = DiagramStyle::neutral(false);
        let s = scene("stateDiagram-v2\n  Idle --> Busy : start", &paint, Some((Hover::Edges, LIT))).unwrap();
        col().w_full().child(s.el(&paint))
    }
    let mut rt = Runtime::new(0u32, view, (400.0, 400.0), Env::new(false, 1.0));
    rt.frame(0.0);
    let find = |rt: &Runtime<u32>, key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}"));
    let label_spot = rt.elements().into_iter().filter(|e| e.key.as_deref().is_some_and(|k| k.starts_with("edge0."))).last().expect("the label's hover group");
    let key = label_spot.key.clone().unwrap();
    let r = label_spot.rect;
    rt.pointer_move(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(find(&rt, &key).hover, 1.0, "on in the first frame, like a node");
    assert_eq!(find(&rt, "node0").hover, 0.0);
    rt.pointer_move(1.0, 399.0);
    for _ in 0..12 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(find(&rt, &key).hover, 0.0, "and off as the pointer leaves");
}
