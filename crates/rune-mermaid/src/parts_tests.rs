use rune::{Cx, El, Env, ElementInfo, Runtime, col};

use crate::{DiagramStyle, Hover, render_streaming, render_with_hover, scene};

fn keys(src: &str) -> Vec<String> {
    let s = scene(src, &DiagramStyle::neutral(true), Some((Hover::Edges, DiagramStyle::neutral(true).text))).expect("draws");
    s.pieces.iter().map(|p| p.key.clone()).chain(s.label_keys.iter().map(|k| k.0.clone())).collect()
}

fn has(keys: &[String], want: &[&str]) -> bool {
    want.iter().all(|w| keys.iter().any(|k| k == w))
}

#[test]
fn flowchart_pieces_are_keyed_by_mermaid_id_across_rebuilds() {
    let a = keys("graph TD\n  A[Start] --> B{Ok?}\n");
    assert!(has(&a, &["n:A", "n:B", "e:A>B", "n:A.t0", "n:B.t0"]), "{a:?}");
    // A line arriving adds its own pieces and keeps every key it had.
    let b = keys("graph TD\n  A[Start] --> B{Ok?}\n  B -->|yes| C[Done]\n  B --> C\n");
    assert!(a.iter().all(|k| b.contains(k)), "{a:?} → {b:?}");
    assert!(has(&b, &["n:C", "e:B>C", "e:B>C~1", "e:B>C.t0"]), "{b:?}");
    // A subgraph is a frame by its id; nodes inside it keep their ids when it arrives around them.
    let c = keys("graph TD\n  subgraph core [Core]\n    A --> B\n  end\n  B --> C\n");
    assert!(has(&c, &["g:core", "g:core.t0", "n:A", "n:B", "n:C", "e:A>B", "e:B>C"]), "{c:?}");
}

#[test]
fn sequence_and_state_pieces_keep_their_keys_as_lines_arrive() {
    let a = keys("sequenceDiagram\n  participant A\n  participant B\n  A->>B: hi\n");
    assert!(has(&a, &["n:p0", "n:p1", "n:p0.end", "g:life0", "e:m0", "e:m0.t0"]), "{a:?}");
    let b = keys("sequenceDiagram\n  participant A\n  participant B\n  A->>B: hi\n  B-->>A: hello\n  Note over A,B: done\n");
    assert!(a.iter().all(|k| b.contains(k)), "{a:?} → {b:?}");
    assert!(has(&b, &["e:m1", "n:note2"]), "{b:?}");

    let s = keys("stateDiagram-v2\n  [*] --> Idle\n  Idle --> Busy : go\n");
    assert!(has(&s, &["n:Idle", "n:Busy", "e:Idle>Busy", "e:Idle>Busy.t0"]), "{s:?}");
}

fn find(rt: &Runtime<(String, bool)>, key: &str) -> Option<ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key))
}

fn view(s: &(String, bool), _: &mut Cx<(String, bool)>) -> El {
    let paint = DiagramStyle::neutral(true);
    let render = if s.1 { render_streaming } else { render_with_hover };
    col().w_full().children(render(&s.0, &paint, Hover::Edges, paint.text))
}

const ONE: &str = "graph TD\n  A[Start] --> B[Check]\n";
const TWO: &str = "graph TD\n  A[Start] --> B[Check]\n  B --> C[Done]\n";

#[test]
fn a_finished_diagram_shows_whole_the_frame_it_mounts() {
    let mut rt = Runtime::new((String::new(), false), view, (600.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.0 = TWO.to_owned());
    rt.frame(0.0);
    // Everything but the hover groups, which stay unlit until hovered.
    let shown: Vec<ElementInfo> = rt.elements().into_iter().filter(|e| e.key.as_deref().is_none_or(|k| !k.starts_with("hover:") && k != "lit")).collect();
    assert!(find(&rt, "n:C").is_some() && find(&rt, "e:B>C").is_some());
    for e in shown.iter().filter(|e| !e.path.contains("/hover:")) {
        assert_eq!((e.opacity, e.scale), (1.0, 1.0), "{:?} fades or grows in", e.key);
    }
    // A rebuild that adds a node shows it whole too, and moves nothing that was there.
    let mut rt = Runtime::new((ONE.to_owned(), false), view, (600.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.0 = TWO.to_owned());
    rt.frame(0.0);
    assert_eq!(find(&rt, "n:C").unwrap().opacity, 1.0);
}

#[test]
fn a_streaming_diagram_brings_new_nodes_in_and_glides_the_rest() {
    let mut rt = Runtime::new((ONE.to_owned(), true), view, (600.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
    let a = find(&rt, "n:A").unwrap();
    rt.update(|s, _| s.0 = TWO.to_owned());
    rt.frame(1.0 / 60.0);
    let c = find(&rt, "n:C").unwrap();
    assert!(c.opacity < 0.5 && c.scale < 1.0, "a new node fades and grows in: {} {}", c.opacity, c.scale);
    assert_eq!(find(&rt, "n:A").unwrap().opacity, 1.0, "what's there stays");
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
    let (c, a2) = (find(&rt, "n:C").unwrap(), find(&rt, "n:A").unwrap());
    assert_eq!((c.opacity, c.scale), (1.0, 1.0));
    assert_eq!(a.path, a2.path, "the same element throughout");
}

#[test]
fn a_streaming_diagram_glides_inside_a_growth_row() {
    use rune::GrowthSpec;
    // A reply's row in a timeline snaps its contents' layout; the pieces still glide.
    fn view(s: &String, _: &mut Cx<String>) -> El {
        let paint = DiagramStyle::neutral(true);
        col().key("stack").w_full().growth(GrowthSpec::default())
            .child(col().key("row").w_full().children(render_streaming(s, &paint, Hover::Edges, paint.text)))
    }
    let find = |rt: &Runtime<String>, key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap();
    let mut rt = Runtime::new(TWO.to_owned(), view, (600.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let before = find(&rt, "n:B").rect;
    rt.update(|s, _| s.push_str("  Z[First] --> A\n"));
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
    let after = find(&rt, "n:B").rect;
    assert!((after[1] - before[1]).abs() > 10.0, "the new node moves B: {before:?} {after:?}");
    let mut rt = Runtime::new(TWO.to_owned(), view, (600.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.push_str("  Z[First] --> A\n"));
    rt.frame(1.0 / 60.0);
    let mid = find(&rt, "n:B").rect;
    assert!((mid[1] - before[1]).abs() < (after[1] - before[1]).abs() - 1.0, "B glides, never jumps: {before:?} {mid:?} {after:?}");
}
