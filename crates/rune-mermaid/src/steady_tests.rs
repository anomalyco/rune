use std::collections::HashMap;
use std::rc::Rc;

use crate::canvas::Scene;
use crate::scenes::{Cache, SCENES, growing as growing_scene};
use crate::{DiagramStyle, scene};

/// A reply's flowchart whose fourth line pulls `On` a rank further, which used to leave the
/// diagram shorter for one line (119, 90, 119, 156 tall).
const RETRIES: &str = "flowchart LR
  Drop[Socket drops] --> R1[Retry at once]
  R1 -->|ok| On[Online]
  R1 -->|fails| R2[Back off 1 s]
  R2 -->|ok| On
  R2 -->|fails| Off[Offline]
  Off -->|retry| R2
";

/// Each node's centre, before the scene shrinks to fit.
fn centres(s: &Scene) -> HashMap<String, [f32; 2]> {
    s.pieces
        .iter()
        .filter(|p| p.key.starts_with("n:") && !p.key.contains('.'))
        .map(|p| (p.key.clone(), [p.at[0] + p.size[0] / 2.0, p.at[1] + p.size[1] / 2.0]))
        .collect()
}

/// Each prefix of `src`, a line longer each time, built as it streams in.
fn stream(src: &str) -> Vec<Rc<Scene>> {
    let lines: Vec<&str> = src.lines().collect();
    (2..=lines.len())
        .filter_map(|k| growing_scene(&(lines[..k].join("\n") + "\n"), &DiagramStyle::neutral(true), None))
        .collect()
}

/// How far any node moves back along an axis it has already moved along the other way.
fn worst_reversal(scenes: &[&Scene]) -> (f32, String) {
    let mut paths: HashMap<String, Vec<[f32; 2]>> = HashMap::new();
    for s in scenes {
        for (k, c) in centres(s) {
            paths.entry(k).or_default().push(c);
        }
    }
    let mut worst = (0.0, String::new());
    for (k, path) in &paths {
        for axis in 0..2 {
            let (mut lo, mut hi, mut up, mut down) = (f32::MAX, f32::MIN, 0.0f32, 0.0f32);
            for p in path {
                let v = p[axis];
                // Back from the furthest point either way.
                up = up.max(hi - v);
                down = down.max(v - lo);
                lo = lo.min(v);
                hi = hi.max(v);
            }
            let back = up.min(down);
            if back > worst.0 {
                worst = (back, format!("{k} axis {axis}: {path:?}"));
            }
        }
    }
    worst
}

/// A top-down chart with a subgraph, a merge and a loop back.
const DEPLOY: &str = "flowchart TD
  A[Push] --> B{Tests pass?}
  B -->|no| C[Fix]
  C --> A
  B -->|yes| D[Build image]
  subgraph Ship
    D --> E[Canary]
    E --> F[Roll out]
  end
  E -->|errors| G[Roll back]
  G --> C
  F --> H([Done])
";

/// A state diagram whose later lines add states beside and before the first ones.
const CONNECTION: &str = "stateDiagram-v2
  [*] --> Idle
  Idle --> Connecting: open
  Connecting --> Open: ok
  Connecting --> Backoff: fail
  Backoff --> Connecting: timer
  Open --> Closing: close
  Open --> Backoff: drop
  Closing --> [*]
";

#[test]
fn a_streaming_diagram_only_grows() {
    for src in [RETRIES, DEPLOY, CONNECTION] {
        let scenes = stream(src);
        let sizes: Vec<[f32; 2]> = scenes.iter().map(|s| s.size).collect();
        assert!(sizes.windows(2).all(|w| w[1][0] >= w[0][0] - 0.01 && w[1][1] >= w[0][1] - 0.01), "it never shrinks: {sizes:?}\n{src}");
        let (back, which) = worst_reversal(&scenes.iter().map(|s| &**s).collect::<Vec<_>>());
        assert!(back <= 1.0, "no node doubles back more than a point: {back} at {which}\n{src}");
    }
}

#[test]
fn a_finished_stream_lays_out_as_if_it_never_streamed() {
    let paint = DiagramStyle::neutral(true);
    let before = scene(RETRIES, &paint, None).unwrap();
    let streamed = stream(RETRIES);
    let after = scene(RETRIES, &paint, None).unwrap();
    assert_eq!(centres(&before), centres(&after), "a streaming build leaves nothing behind for the next build");
    assert_eq!(before.size, after.size);
    assert!(!streamed.is_empty());
}

#[test]
fn a_growing_build_is_the_same_however_many_lines_arrived_at_once() {
    let paint = DiagramStyle::neutral(true);
    let by_line = stream(RETRIES).pop().unwrap();
    SCENES.with_borrow_mut(|c| *c = Cache::default());
    let at_once = growing_scene(RETRIES, &paint, None).unwrap();
    assert_eq!(centres(&by_line), centres(&at_once));
    assert_eq!(by_line.size, at_once.size);
}

#[test]
fn nodes_keep_their_places_by_name_when_one_before_them_becomes_a_subgraph() {
    // `S` starts as a node and turns into a frame, so `W` and `Y` each move up the node list.
    let scenes = stream("graph TD\n  X -->|in| S\n  S --> W\n  W -->|out| Y\n  subgraph S\n    D\n  end\n");
    let (back, which) = worst_reversal(&scenes.iter().map(|s| &**s).collect::<Vec<_>>());
    assert!(back <= 1.0, "no node doubles back more than a point: {back} at {which}");
}
