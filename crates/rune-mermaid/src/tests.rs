use super::canvas::{Canvas, Op, dist, smooth};
use super::*;

#[test]
fn clean_turns_markup_into_text() {
    assert_eq!(clean("a<br>b<br/>c<BR />d\\ne"), "a\nb\nc\nd\ne");
    assert_eq!(clean("#quot;hi#quot; &amp; &lt;3 &apos;ok&apos;"), "\"hi\" & <3 'ok'");
    assert_eq!(clean("`**md**`"), "**md**");
    assert_eq!(clean("# not an entity & also not"), "# not an entity & also not");
    assert_eq!(clean_quoted(" \"capuchin ≈ Cappuccino\" "), "capuchin ≈ Cappuccino");
}

#[test]
fn other_diagram_types_and_garbage_are_unsupported() {
    for src in ["", "kanban\n  Todo", "pie title Pets", "gantt\ntitle x", "erDiagram", "hello", "%% only a comment"] {
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
fn directive_end_keeps_a_diagram_on_the_same_line() {
    for directive in ["%%{init: {}}%%", "%%{\ninit: {}\n}%%"] {
        let src = format!("{directive} mindmap\n  root\n    child");
        let (_, lines) = cleaned(&src);
        assert_eq!(lines.collect::<Vec<_>>(), [(0, "mindmap"), (2, "root"), (4, "child")]);
        assert!(may_draw(&src));
    }
}

#[test]
fn node_stroke_width_reads_mermaid_px() {
    assert_eq!(parse_style_props("stroke:#333,stroke-width:2px").width, Some(2.0));
    assert_eq!(parse_style_props("stroke-width:4").width, Some(4.0));
    assert_eq!(parse_style_props("stroke-width:wide").width, None);
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
    let t = rune::time::now();
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
    let t = rune::time::now();
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
    let t = rune::time::now();
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

pub(crate) fn assert_prefixes_never_panic(src: &str, dark: bool) {
    let paint = DiagramStyle::neutral(dark);
    for (i, _) in src.char_indices() {
        let _ = scene(&src[..i], &paint, None);
    }
    assert!(scene(src, &paint, None).is_some());
}

/// Seven lanes with long names: wider than a 420-point column even at 60%.
const WIDE_SEQUENCE: &str = "sequenceDiagram\n  autonumber\n  participant B as Browser on a laptop\n  participant E as Edge cache in front\n  participant A as Auth service cluster\n  participant P as Payments API server\n  participant W as Billing worker pool\n  participant S as Search index shard\n  participant L as Long-running audit log\n  B->>E: a request arrives at the edge\n  E->>A: is this session token still valid?\n  A-->>E: yes, here is the account scope\n  E->>P: forward the call with its scope\n  P->>W: enqueue the invoice with the scope\n  W->>S: index the invoice\n  S-->>W: ok\n  W->>L: record the change\n  W-->>P: done\n  P-->>E: the response";

#[test]
fn min_scale_zero_fits_any_width() {
    let narrow = DiagramStyle { max_width: 420.0, ..DiagramStyle::neutral(true) };
    let room = super::canvas::room(&narrow);
    let floored = scene(WIDE_SEQUENCE, &narrow, None).expect("draws");
    assert!((floored.scale - 0.6).abs() < 1e-4 && floored.size[0] > room, "by default it stops at 60%: {} at {}", floored.size[0], floored.scale);
    let fits = scene(WIDE_SEQUENCE, &narrow.min_scale(0.0), None).expect("draws");
    assert!(fits.size[0] <= room + 2.01, "min_scale 0 fits the room {room}: {}", fits.size[0]);
    assert!(fits.scale < 0.6 && (fits.full[1] * fits.scale - fits.size[1]).abs() < 0.01, "it shrinks evenly: {}", fits.scale);
    let roomy = DiagramStyle { max_width: 4000.0, ..narrow }.min_scale(0.0);
    assert_eq!(scene(WIDE_SEQUENCE, &roomy, None).expect("draws").scale, 1.0, "never grows past full size");
}

/// The bare drawing is the card's drawing without the card: the same size, plus its title.
#[test]
fn a_drawing_is_the_card_without_its_padding() {
    let roomy = DiagramStyle { max_width: 4000.0, ..DiagramStyle::neutral(true) };
    let full = scene(WIDE_SEQUENCE, &roomy, None).expect("draws");
    let d = render_drawing(WIDE_SEQUENCE, &roomy, Hover::Off, roomy.text).expect("draws");
    assert_eq!(d.size, full.size);
    let titled = format!("---\ntitle: Checkout\n---\n{WIDE_SEQUENCE}");
    let t = render_drawing(&titled, &roomy, Hover::Off, roomy.text).expect("draws");
    assert!(t.size[1] > d.size[1] + 20.0 && t.size[0] >= d.size[0], "the title adds a line: {:?} {:?}", t.size, d.size);
}

/// Lines shrink with the drawing, as an SVG's do: a shrunk diagram's full-size pieces are the
/// same as an unshrunk one's, so magnifying it never shows fat lines.
#[test]
fn lines_shrink_with_the_drawing() {
    let narrow = DiagramStyle { max_width: 420.0, ..DiagramStyle::neutral(true) }.min_scale(0.0);
    let roomy = DiagramStyle { max_width: 4000.0, ..narrow };
    let (small, full) = (scene(WIDE_SEQUENCE, &narrow, None).expect("draws"), scene(WIDE_SEQUENCE, &roomy, None).expect("draws"));
    assert!(small.scale < 0.5, "shrinks: {}", small.scale);
    for (a, b) in small.pieces.iter().zip(&full.pieces) {
        assert_eq!(a.key, b.key);
        let close = (a.size[0] - b.size[0]).abs() < 0.01 && (a.size[1] - b.size[1]).abs() < 0.01;
        assert!(close, "{} is {:?} shrunk, {:?} full", a.key, a.size, b.size);
    }
}

fn chain_src(nodes: usize) -> String {
    let mut s = String::from("flowchart TD\n");
    for i in 0..nodes.saturating_sub(1) {
        s.push_str(&format!("  N{i} --> N{}\n", i + 1));
    }
    s
}

fn style_hash(style: &DiagramStyle) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    style.hash(&mut h);
    h.finish()
}

#[test]
fn max_items_defaults_to_400_and_opts_in_to_larger_flowcharts() {
    // 210 nodes + 209 edges = 419 items: rejected at default 400, accepted with max_items(1_000).
    let chain_210 = chain_src(210);
    let def = DiagramStyle::neutral(false);
    let raised = def.max_items(1_000);
    assert!(!supported(&chain_210));
    assert!(!supported_with(&chain_210, &def));
    assert!(render(&chain_210, &def).is_none());
    assert!(supported_with(&chain_210, &raised));
    assert!(render(&chain_210, &raised).is_some());

    // A 500-edge flowchart (501 nodes + 500 edges = 1,001 items) renders with max_items(1_200).
    let chain_501 = chain_src(501);
    let big = def.max_items(1_200);
    assert!(render(&chain_501, &def).is_none());
    assert!(supported_with(&chain_501, &big));
    assert!(render(&chain_501, &big).is_some());

    // A denser 500-edge layered DAG (250 nodes + 500 edges = 750 items) also renders with max_items(1_200).
    let mut dag = String::from("flowchart TD\n");
    let mut edges = 0usize;
    for l in 0..24 {
        for w in 0..10 {
            if edges >= 500 { break; }
            let u = l * 10 + w;
            dag.push_str(&format!("  N{u} --> N{}\n", (l + 1) * 10 + w));
            edges += 1;
            if edges < 500 {
                dag.push_str(&format!("  N{u} -->|e{edges}| N{}\n", (l + 1) * 10 + ((w + 1) % 10)));
                edges += 1;
            }
        }
    }
    assert!(render(&dag, &def).is_none());
    assert!(render(&dag, &big).is_some());
}

#[test]
fn max_items_clamps_bounds_ignores_directives_and_keys_scene_cache() {
    let def = DiagramStyle::neutral(true);
    // Clamping: 0 -> 1, above MAX_ITEMS_CEILING -> MAX_ITEMS_CEILING.
    assert_eq!(style_hash(&def.max_items(0)), style_hash(&def.max_items(1)));
    assert_eq!(style_hash(&def.max_items(MAX_ITEMS_CEILING)), style_hash(&def.max_items(100_000)));
    assert_ne!(style_hash(&def), style_hash(&def.max_items(1_000)));

    let tiny = def.max_items(0);
    assert!(supported_with("flowchart TD\n  A", &tiny));
    assert!(!supported_with("flowchart TD\n  A --> B", &tiny));

    // At the ceiling (1,500): 750 nodes + 749 edges = 1,499 items passes; 751 + 750 = 1,501 fails.
    let over = def.max_items(100_000);
    assert!(supported_with(&chain_src(750), &over));
    assert!(!supported_with(&chain_src(751), &over));

    // Diagram directives and front matter cannot raise the cap past style.max_items.
    let directed = format!(
        r#"%%{{init: {{"maxEdges": 5000, "maxItems": 5000}}}}%%
---
maxEdges: 5000
---
{}"#,
        chain_src(210),
    );
    assert!(!supported(&directed));
    assert!(render(&directed, &def).is_none());
    assert!(render(&directed, &def.max_items(1_000)).is_some());

    // Scene cache differentiates styles with different max_items on the same source.
    let src = chain_src(210);
    assert!(scenes::built(&src, &def, None, None).0.is_none());
    assert!(scenes::built(&src, &def.max_items(1_000), None, None).0.is_some());
    assert!(scenes::built(&src, &def, None, None).0.is_none());
}

#[test]
fn max_items_preserves_dummy_and_depth_bounds_and_applies_across_diagram_types() {
    let high = DiagramStyle::neutral(true).max_items(MAX_ITEMS_CEILING);

    // MAX_DUMMIES = 3000 still rejects dense tangled graphs even at MAX_ITEMS_CEILING.
    let mut tangled = vec!["graph LR".to_owned()];
    for i in 0..130 {
        tangled.push(format!("N{i} --> N{} & N{}", (i * 7 + 1) % 130, (i * 13 + 5) % 130));
    }
    let tangled = tangled.join("\n");
    assert!(supported_with(&tangled, &high));
    assert!(scene(&tangled, &high, None).is_none(), "dense graph still hits MAX_DUMMIES");

    // MAX_DEPTH = 8 still rejects 9-deep nested subgraphs.
    let deep = format!("graph TD\n{}A\n{}", "subgraph s\n".repeat(9), "end\n".repeat(9));
    assert!(!supported_with(&deep, &high));

    // Non-flowchart diagram types honour max_items while keeping default 400 unchanged.
    let seq = format!("sequenceDiagram\n{}", (0..450).map(|i| format!("  A->>B: m{i}")).collect::<Vec<_>>().join("\n"));
    let pie = format!("pie\n{}", (0..450).map(|i| format!("  \"s{i}\" : 1")).collect::<Vec<_>>().join("\n"));
    let state = format!("stateDiagram-v2\n{}", (0..220).map(|i| format!("  S{i} --> S{}", i + 1)).collect::<Vec<_>>().join("\n"));
    let class = format!("classDiagram\n{}", (0..220).map(|i| format!("  C{i} --> C{}", i + 1)).collect::<Vec<_>>().join("\n"));
    let er = format!("erDiagram\n{}", (0..220).map(|i| format!("  E{i} ||--o{{ E{} : r", i + 1)).collect::<Vec<_>>().join("\n"));
    let gantt = format!("gantt\n{}", (0..450).map(|i| format!("  T{i} :2024-01-01, 1d")).collect::<Vec<_>>().join("\n"));
    let mindmap = format!("mindmap\nroot\n{}", (0..450).map(|i| format!("  n{i}")).collect::<Vec<_>>().join("\n"));
    let xy = format!("xychart-beta\n  bar [{}]", vec!["1"; 450].join(", "));
    for src in [&seq, &pie, &state, &class, &er, &gantt, &mindmap, &xy] {
        assert!(!supported(src), "default 400 rejects {}", &src[..20]);
        assert!(supported_with(src, &high), "raised cap accepts {}", &src[..20]);
    }
}
