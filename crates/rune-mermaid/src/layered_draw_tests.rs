use crate::DiagramStyle;
use crate::canvas::{Canvas, Label, Op, measure_balanced};

/// A subgraph with a long title, a link to the subgraph itself and two labelled exits.
const CHAT: &str = "flowchart LR\n  subgraph Thing[\"The chat (task): has a history\"]\n    H1[your point] --> H2[result 1] --> H3[your reply] --> H4[result 2]\n  end\n  H4 -->|\"newest turn is a question\"| N[Needs you]\n  H4 -->|\"newest turn is a result you haven't read\"| R[Ready]\n  Note[(Note: lasts, has versions)] -.linked.- Thing";

fn paint_at(dark: bool, max_width: f32) -> DiagramStyle {
    DiagramStyle { max_width, ..DiagramStyle::neutral(dark) }
}

fn canvas(src: &str, paint: &DiagramStyle) -> Canvas {
    let Some(crate::Parsed { diagram: crate::Diagram::Flowchart(f), .. }) = crate::parse(src) else { panic!("parses: {src}") };
    crate::flowchart::draw_canvas(&f, paint, Some((crate::Hover::Edges, paint.text))).expect("draws")
}

fn label<'a>(c: &'a Canvas, text: &str) -> &'a Label {
    c.labels.iter().find(|l| l.text == text).unwrap_or_else(|| panic!("no label {text:?}"))
}

fn rect(l: &Label) -> [f32; 4] {
    [l.at[0], l.at[1], l.at[0] + l.size[0], l.at[1] + l.size[1]]
}

/// The first cluster's box: clusters draw first.
fn cluster(c: &Canvas) -> [f32; 4] {
    let Some(Op::Fill(pts, _)) = c.ops.first() else { panic!("a cluster first") };
    pts.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, p| [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])])
}

/// Every open stroke: edges, never outlines.
fn lines(c: &Canvas) -> impl Iterator<Item = &Vec<[f32; 2]>> {
    c.ops.iter().filter_map(|op| match op {
        Op::Stroke { points, closed: false, .. } => Some(points),
        _ => None,
    })
}

fn hits(w: &[[f32; 2]], r: [f32; 4]) -> bool {
    (0..=16).any(|k| {
        let p = crate::canvas::lerp(w[0], w[1], k as f32 / 16.0);
        p[0] > r[0] && p[0] < r[2] && p[1] > r[1] && p[1] < r[3]
    })
}

#[test]
fn no_edge_crosses_a_subgraph_title() {
    for dark in [false, true] {
        for width in [680.0, 420.0, 2000.0] {
            let paint = paint_at(dark, width);
            let c = canvas(CHAT, &paint);
            let title = rect(label(&c, "The chat (task): has a history"));
            for l in lines(&c) {
                assert!(!l.windows(2).any(|w| hits(w, title)), "an edge crosses the title {title:?} at width {width}: {l:?}");
            }
        }
    }
}

/// Each subgraph's title has the top of its box to itself, as in mermaid.js: one line, centred,
/// and no node or edge inside its band.
#[test]
fn subgraph_titles_keep_a_band_of_their_own() {
    let plan = include_str!("../fixtures/plan.mmd");
    let titles = ["Prep, done", "Stage 2: first pass on the new parser (the pilot)", "AST track: the new parser reads the old tool's syntax trees", "Stage 3: formatter on the new parser", "Key"];
    for dir in ["LR", "TD"] {
        let src = plan.replacen("flowchart LR", &format!("flowchart {dir}"), 1);
        for width in [680.0, 4000.0] {
            let c = canvas(&src, &paint_at(true, width));
            let boxes: Vec<[f32; 4]> = c.ops.iter().filter_map(|op| match op {
                Op::Fill(pts, _) if pts.len() > 8 => Some(pts.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, p| [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])])),
                _ => None,
            }).collect();
            for t in titles {
                let l = label(&c, t);
                let r = rect(l);
                let what = format!("{t:?} {dir} at width {width}: {r:?}");
                assert!(!l.wrap, "one line: {what}");
                let b = boxes.iter().filter(|b| b[0] <= r[0] && b[2] >= r[2] && b[1] <= r[1] && b[3] >= r[3]).min_by(|a, b| (a[2] - a[0]).total_cmp(&(b[2] - b[0]))).unwrap_or_else(|| panic!("in a box: {what}"));
                assert!(((b[0] + b[2]) / 2.0 - (r[0] + r[2]) / 2.0).abs() < 0.5, "centred in {b:?}: {what}");
                let band = [b[0], b[1], b[2], r[3]];
                for n in c.labels.iter().filter(|n| !titles.contains(&n.text.as_str())) {
                    let o = rect(n);
                    assert!(o[0] >= band[2] || o[2] <= band[0] || o[1] >= band[3] || o[3] <= band[1], "{:?} is in the band of {what}", n.text);
                }
                for l in lines(&c) {
                    assert!(!l.windows(2).any(|w| hits(w, r)), "an edge crosses {what}: {l:?}");
                }
            }
        }
    }
}

/// Edges that share a node meet it side by side, in the order they come from, never on one point.
#[test]
fn edges_sharing_an_end_fan_out() {
    let plan = include_str!("../fixtures/plan.mmd");
    for dir in ["LR", "TD"] {
        let fan = format!("flowchart {dir}\n  A[one source] --> C[where they meet]\n  B[another source] --> C\n  X[far] --> C\n  C --> D[next]\n  C --> E[and next]");
        let srcs = [plan.replacen("flowchart LR", &format!("flowchart {dir}"), 1), fan];
        for (src, width) in srcs.iter().flat_map(|s| [(s, 680.0), (s, 4000.0)]) {
            let paint = paint_at(true, width);
            let c = canvas(src, &paint);
            let tips: Vec<[f32; 2]> = c.ops.iter().filter_map(|op| match op {
                Op::Fill(p, col) if *col == paint.line && p.len() == 3 => Some(p[0]),
                _ => None,
            }).collect();
            let starts: Vec<[f32; 2]> = lines(&c).map(|l| l[0]).collect();
            for (what, pts, gap) in [("arrowheads", &tips, 11.5), ("edges leaving", &starts, 6.0)] {
                for (a, p) in pts.iter().enumerate() {
                    for q in &pts[a + 1..] {
                        let d = crate::canvas::dist(*p, *q);
                        assert!(d >= gap, "{what} {p:?} and {q:?} are {d} apart in {dir} at width {width}: {}", src.lines().next().unwrap_or(""));
                    }
                }
            }
        }
    }
}

/// Edges whose ends both lie outside a subgraph go around its box, as dagre routes them, never
/// through it.
#[test]
fn edges_route_around_subgraphs_they_do_not_belong_to() {
    let plan = include_str!("../fixtures/plan.mmd");
    let past = "flowchart TD\n  A[start] --> B[finish]\n  subgraph S[In between]\n    M1[one] --> M2[two] --> M3[three]\n  end\n  A --> M1\n  M3 --> B\n  A --> C[aside] --> B";
    for dir in ["LR", "TD"] {
        let srcs = [plan.replacen("flowchart LR", &format!("flowchart {dir}"), 1), past.replacen("flowchart TD", &format!("flowchart {dir}"), 1)];
        for (src, width) in srcs.iter().flat_map(|s| [(s, 680.0), (s, 4000.0)]) {
            let Some(crate::Parsed { diagram: crate::Diagram::Flowchart(f), .. }) = crate::parse(src) else { panic!() };
            let c = canvas(src, &paint_at(true, width));
            let bounds = |pts: &[[f32; 2]]| pts.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, p| [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]);
            let within = |v: usize, k: usize| std::iter::successors(f.nodes[v].cluster, |&s| f.subgraphs[s].parent).any(|s| s == k);
            let boxes: Vec<[f32; 4]> = c.ops.iter().filter_map(|op| match op {
                Op::Fill(pts, _) if pts.len() > 8 => Some(bounds(pts)),
                _ => None,
            }).collect();
            for k in 0..f.subgraphs.len() {
                let members: Vec<[f32; 4]> = (0..f.nodes.len()).filter(|&v| within(v, k)).map(|v| bounds(&c.regions[v])).collect();
                let Some(b) = boxes.iter().filter(|b| members.iter().all(|m| b[0] < m[0] - 4.0 && b[1] < m[1] - 4.0 && b[2] > m[2] + 4.0 && b[3] > m[3] + 4.0)).min_by(|a, b| ((a[2] - a[0]) * (a[3] - a[1])).total_cmp(&((b[2] - b[0]) * (b[3] - b[1])))) else { continue };
                let inner = [b[0] + 1.5, b[1] + 1.5, b[2] - 1.5, b[3] - 1.5];
                for t in &c.traces {
                    if within(t.ends.0, k) || within(t.ends.1, k) {
                        continue;
                    }
                    let into = t.line.iter().find(|p| p[0] > inner[0] && p[0] < inner[2] && p[1] > inner[1] && p[1] < inner[3]);
                    assert!(into.is_none(), "{} → {} runs through {:?} at {into:?}, {dir} at width {width}", f.nodes[t.ends.0].id, f.nodes[t.ends.1].id, f.subgraphs[k].title);
                }
            }
        }
    }
}

#[test]
fn an_edge_to_a_subgraph_stops_on_its_border() {
    let paint = DiagramStyle::neutral(false);
    let c = canvas(CHAT, &paint);
    let b = cluster(&c);
    let dashes: Vec<&Vec<[f32; 2]>> = c.ops.iter().filter_map(|op| match op {
        Op::Stroke { points, cap: rune::LineCap::Butt, .. } => Some(points),
        _ => None,
    }).collect();
    assert!(!dashes.is_empty(), "the dotted link draws");
    let inner = [b[0] + 0.5, b[1] + 0.5, b[2] - 0.5, b[3] - 0.5];
    for d in &dashes {
        assert!(!d.iter().any(|p| p[0] > inner[0] && p[0] < inner[2] && p[1] > inner[1] && p[1] < inner[3]), "the link reaches inside the subgraph {b:?}: {d:?}");
    }
    let near = dashes.iter().flat_map(|d| d.iter()).map(|p| {
        let dx = (b[0] - p[0]).max(p[0] - b[2]).max(0.0);
        let dy = (b[1] - p[1]).max(p[1] - b[3]).max(0.0);
        dx.max(dy)
    });
    assert!(near.fold(f32::MAX, f32::min) < 1.5, "the link ends on the border {b:?}");
}

/// How far below its title a subgraph's first node sits, for `dir`.
fn title_gap(dir: &str) -> f32 {
    let paint = paint_at(false, 4000.0);
    let c = canvas(&format!("flowchart {dir}\n  subgraph S[A title]\n    A[One] --> B[Two]\n  end"), &paint);
    let t = rect(label(&c, "A title"));
    let top = c.regions.iter().flatten().map(|p| p[1]).fold(f32::MAX, f32::min);
    top - t[3]
}

#[test]
fn a_subgraph_title_keeps_the_same_gap_above_its_nodes_either_way() {
    let (down, right) = (title_gap("TD"), title_gap("LR"));
    assert!((down - right).abs() < 0.5, "TD {down} vs LR {right}");
    assert!((down - 14.0).abs() < 0.5, "the gap {down} is the flowchart's subgraph padding");
}

#[test]
fn edges_leave_a_subgraph_steeply() {
    for width in [680.0, 2000.0] {
        let paint = paint_at(false, width);
        let c = canvas(CHAT, &paint);
        let b = cluster(&c);
        let turned = b[3] - b[1] > b[2] - b[0];
        let mut crossings = 0;
        for l in lines(&c) {
            for w in l.windows(2) {
                let (a, z) = (w[0], w[1]);
                // The far border along the rank axis: the bottom when turned, else the right.
                let (along, across, edge) = if turned { (1, 0, b[3]) } else { (0, 1, b[2]) };
                if (a[along] - edge) * (z[along] - edge) < 0.0 && a[across] > b[across] && a[across] < b[across + 2] {
                    crossings += 1;
                    let slope = (z[across] - a[across]).abs() / (z[along] - a[along]).abs();
                    assert!(slope < 0.6, "an edge leaves the subgraph at slope {slope} (width {width})");
                }
            }
        }
        assert_eq!(crossings, 2, "both exits cross the border once (width {width})");
    }
}

#[test]
fn wrapped_labels_balance_their_lines() {
    let paint = DiagramStyle::neutral(false);
    let m = measure_balanced("newest turn is a result you haven't read", paint.font);
    let one = measure_balanced("newest", paint.font).size[1];
    assert!(m.wrap && m.size[1] < one * 2.5, "two lines: {:?}", m.size);
    assert!(m.size[0] < 160.0, "balanced, not one long line and a lone word: {}", m.size[0]);
}

#[test]
fn a_chart_that_fits_its_card_keeps_its_direction() {
    let narrow = paint_at(false, 300.0);
    let wide = paint_at(false, 2000.0);
    let tall = |c: &Canvas| {
        let b = cluster(c);
        b[3] - b[1] > b[2] - b[0]
    };
    assert!(!tall(&canvas(CHAT, &wide)), "LR stays LR with room");
    assert!(tall(&canvas(CHAT, &narrow)), "LR turns down when it can't fit");
}

#[test]
fn an_edge_runs_straight_through_its_label() {
    for width in [420.0, 680.0, 2000.0] {
        let c = canvas(CHAT, &paint_at(false, width));
        for text in ["newest turn is a question", "newest turn is a result you haven't read"] {
            let r = rect(label(&c, text));
            let off = |p: [f32; 2]| (r[0] - p[0]).max(p[0] - r[2]).max(r[1] - p[1]).max(p[1] - r[3]);
            let ends: Vec<[f32; 2]> = lines(&c).flat_map(|l| [l[0], l[l.len() - 1]]).filter(|&p| off(p) > -0.5 && off(p) < 10.0).collect();
            assert_eq!(ends.len(), 2, "the line stops either side of {text:?} (width {width}): {ends:?}");
            let (a, b) = (ends[0], ends[1]);
            assert!((a[0] - b[0]).abs() < 1.0 || (a[1] - b[1]).abs() < 1.0, "the line jogs through {text:?} (width {width}): {a:?} to {b:?}");
        }
    }
}
