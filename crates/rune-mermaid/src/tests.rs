use super::canvas::{Canvas, Op, dist, smooth};
use super::*;

#[test]
fn clean_turns_markup_into_text() {
    assert_eq!(clean("a<br>b<br/>c<BR />d\\ne"), "a\nb\nc\nd\ne");
    assert_eq!(clean("#quot;hi#quot; &amp; &lt;3 &apos;ok&apos;"), "\"hi\" & <3 'ok'");
    assert_eq!(clean("`**md**`"), "**md**");
    assert_eq!(clean("# not an entity & also not"), "# not an entity & also not");
    assert_eq!(clean_quoted(" \"avatar ≈ Abattoir\" "), "avatar ≈ Abattoir");
}

#[test]
fn other_diagram_types_and_garbage_are_unsupported() {
    for src in ["", "pie title Pets\n\"Dogs\" : 386", "gantt\ntitle x", "erDiagram\nA ||--o{ B : has", "hello", "%% only a comment"] {
        assert!(!supported(src), "{src:?}");
    }
    let with_title = scene("---\ntitle: Hi\n---\ngraph TD\nA-->B", &DiagramStyle::neutral(true), None).unwrap();
    assert_eq!(with_title.title.as_deref(), Some("Hi"), "front matter title is preserved");
    assert!(supported("%%{\n  init: {'theme': 'dark'}\n}%%\nflowchart LR\nA-->B"), "multiline init directive is skipped");
    assert!(supported("stateDiagram-v2\n[*] --> Idle\nIdle --> [*]"));
    assert!(supported("classDiagram\nA <|-- B"));
    assert!(supported("flowchart TD\n  A[Runtime&lt;S&gt;] --> B[a; b]; B --> C"), "entities and bracketed `;` stay intact");
}

#[test]
fn node_style_keeps_contrast_in_dark_and_light_themes() {
    let pastel = parse_style_props("fill:#dcf5dc,stroke:#333");
    for dark in [true, false] {
        let style = DiagramStyle::neutral(dark);
        let (fill, _border, text) = pastel.resolve(&style);
        let diff = (fill.to_oklab()[0] - text.to_oklab()[0]).abs();
        assert!(diff > 0.45, "dark={dark} contrast {diff} between {fill:?} and {text:?}");
    }
    assert!(parse_color_spec("rgb(30, 60, 90)").is_some());
    assert!(parse_color_spec("lightgreen").is_some());
}

#[test]
fn dashes_cover_the_line_in_on_off_runs() {
    let mut c = Canvas::default();
    c.dashed(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]], 1.0, Color::WHITE, 3.0, 1.0);
    let dashes: Vec<&Vec<[f32; 2]>> = c.ops.iter().map(|op| match op { Op::Stroke { points, .. } => points, Op::Fill(..) => unreachable!() }).collect();
    assert_eq!(dashes.len(), 5, "20 points of line in 4-point periods");
    assert_eq!(dashes[0].first(), Some(&[0.0, 0.0]));
    let corner = dashes[2];
    assert_eq!(corner.len(), 3, "a dash turns the corner: {corner:?}");
    assert!(dist(corner[0], [8.0, 0.0]) < 1e-4 && dist(corner[1], [10.0, 0.0]) < 1e-4 && dist(corner[2], [10.0, 1.0]) < 1e-4);
}

#[test]
fn smooth_passes_through_its_points() {
    let pts = [[0.0, 0.0], [20.0, 40.0], [0.0, 80.0]];
    let curve = smooth(&pts);
    assert_eq!(curve.first(), Some(&pts[0]));
    assert_eq!(curve.last(), Some(&pts[2]));
    assert!(curve.iter().any(|p| dist(*p, pts[1]) < 0.01));
}

fn hostile(src: &str, draws: bool) {
    let paint = DiagramStyle::neutral(true);
    let t = std::time::Instant::now();
    let scene = scene(src, &paint, None);
    let took = t.elapsed();
    assert_eq!(scene.is_some(), draws, "{}…", &src[..src.len().min(80)]);
    assert!(took.as_secs_f32() < 5.0, "{took:?} for {}…", &src[..src.len().min(80)]);
}

fn ids(prefix: &str, n: usize) -> String {
    (1..=n).map(|i| format!("{prefix}{i}")).collect::<Vec<_>>().join(" & ")
}

#[test]
fn deep_subgraphs_fall_back() {
    hostile(&format!("graph TD\n{}A\n{}", "subgraph s\n".repeat(5000), "end\n".repeat(5000)), false);
    hostile(&format!("graph TD\n{}A\n{}", "subgraph s\n".repeat(9), "end\n".repeat(9)), false);
    hostile(&format!("graph TD\n{}A\n{}", "subgraph s\n".repeat(8), "end\n".repeat(8)), true);
    hostile(&format!("graph TD\nA\n{}", "subgraph s\nend\n".repeat(5000)), false);
}

#[test]
fn a_huge_cross_product_falls_back_before_it_is_built() {
    hostile(&format!("graph TD\n{} --> {}", ids("A", 500), ids("B", 500)), false);
    hostile(&format!("graph TD\n{}", ids("A", 5000)), false);
    hostile(&format!("graph TD\n{} --> {}", ids("A", 10), ids("B", 10)), true);
}

#[test]
fn a_huge_dash_count_draws_at_the_longest_link() {
    let paint = DiagramStyle::neutral(true);
    hostile(&format!("graph TD\nA {}> B", "-".repeat(10_000)), true);
    hostile(&format!("graph TD\nA -{}-> B", ".".repeat(10_000)), true);
    let longest = scene(&format!("graph TD\nA {}> B", "-".repeat(10_000)), &paint, None).unwrap();
    let eight = scene(&format!("graph TD\nA {}> B", "-".repeat(9)), &paint, None).unwrap();
    assert_eq!(longest.size, eight.size);
}

#[test]
fn long_chords_over_a_chain_fall_back() {
    let chain: Vec<String> = (0..129).map(|i| format!("N{i} --> N{}", i + 1)).collect();
    let chords: Vec<String> = (0..140).map(|k| format!("N{} --> N{}", k % 10, 120 + k % 10)).collect();
    hostile(&format!("graph TD\n{}\n{}", chain.join("\n"), chords.join("\n")), false);
    let short: Vec<String> = (0..4).map(|k| format!("N{k} --> N{}", 30 + k)).collect();
    hostile(&format!("graph TD\n{}\n{}", chain[..40].join("\n"), short.join("\n")), true);
}

#[test]
fn the_biggest_allowed_charts_draw_in_bounded_time() {
    let mut lines = vec!["graph LR".to_owned()];
    for i in 0..130 {
        lines.push(format!("N{i} --> N{} & N{}", (i * 7 + 1) % 130, (i * 13 + 5) % 130));
    }
    let src = lines.join("\n");
    assert!(parse(&src).is_some());
    let paint = DiagramStyle::neutral(true);
    let t = std::time::Instant::now();
    let drawn = scene(&src, &paint, None).is_some();
    assert!(!drawn, "dense 260-edge random graph hits MAX_DUMMIES and falls back");
    assert!(t.elapsed().as_secs_f32() < 5.0, "{:?}", t.elapsed());
    let mut big = String::from("graph TD\nsubgraph left\n");
    for i in 0..59 {
        big.push_str(&format!("N{i} --> N{}\n", i + 1));
        if i == 30 {
            big.push_str("end\nsubgraph right\n");
        }
    }
    big.push_str("end\n");
    for k in 0..40 {
        big.push_str(&format!("N{} --> N{}\n", k % 20, k % 20 + 37));
    }
    let t = std::time::Instant::now();
    assert!(scene(&big, &paint, None).is_some());
    assert!(t.elapsed().as_secs_f32() < 5.0, "{:?}", t.elapsed());
    let mut nested = String::from("graph TD\n");
    for d in 0..8 {
        nested.push_str(&format!("subgraph s{d}\nA{d} --> B{d} --> C{d}\n"));
    }
    nested.push_str(&"end\n".repeat(8));
    nested.push_str("A0 --> C7\nC7 --> A0");
    hostile(&nested, true);
}

#[test]
fn repeated_create_is_skipped_without_recursing() {
    hostile(&format!("sequenceDiagram\n{}participant A\nA->>B: hi", "create ".repeat(100_000)), true);
    hostile("sequenceDiagram\ncreate loop x\nA->>B: hi\nend", true);
    hostile("sequenceDiagram\ncreate participant A\nA->>B: hi", true);
    hostile("sequenceDiagram\nA->>B: hi\ncreate actor C as Carol\nB->>C: hello", true);
}

#[test]
fn dashes_stop_on_huge_and_non_finite_lengths() {
    let ink = Color::BLACK;
    for end in [1e9, f32::INFINITY, f32::NAN] {
        let mut c = Canvas::default();
        c.dashed(&[[0.0, 0.0], [end, 0.0]], 1.0, ink, 4.0, 3.0);
        assert!(c.ops.len() <= 20_000);
    }
}
