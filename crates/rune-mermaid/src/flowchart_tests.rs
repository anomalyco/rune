use super::*;
use crate::DiagramStyle;
use crate::canvas::dist;
use crate::layered_draw::boundary;

fn flow(src: &str) -> Flowchart {
    match crate::parse(src).map(|p| p.diagram) {
        Some(crate::Diagram::Flowchart(f)) => f,
        _ => panic!("not a flowchart: {src:?}"),
    }
}

fn edges(f: &Flowchart) -> Vec<(&str, &str)> {
    f.edges.iter().map(|e| (f.nodes[e.from].id.as_str(), f.nodes[e.to].id.as_str())).collect()
}

#[test]
fn directions_shapes_and_labels() {
    let f = flow(
        "flowchart LR\n  A[Start] --> B(Round)\n  B --> C([Stadium]) --> D[[Sub]]\n  D --> E[(Database)] & F((Circle))\n  F --> G>Flag] --> H{Decide?} --> I{{Hex}}\n  I --> J[/Lean/] --> K[/Trap\\]\n  K --> L[\\Back\\] --> M[\\Top/] --> N(((Stop)))",
    );
    assert_eq!(f.dir, Dir::Right);
    let shapes: Vec<(&str, &str, NodeShape)> = f.nodes.iter().map(|n| (n.id.as_str(), n.label.as_str(), n.shape)).collect();
    assert_eq!(
        shapes,
        vec![
            ("A", "Start", NodeShape::Rect),
            ("B", "Round", NodeShape::Round),
            ("C", "Stadium", NodeShape::Stadium),
            ("D", "Sub", NodeShape::Subroutine),
            ("E", "Database", NodeShape::Cylinder),
            ("F", "Circle", NodeShape::Circle),
            ("G", "Flag", NodeShape::Asymmetric),
            ("H", "Decide?", NodeShape::Diamond),
            ("I", "Hex", NodeShape::Hexagon),
            ("J", "Lean", NodeShape::LeanRight),
            ("K", "Trap", NodeShape::TrapBottom),
            ("L", "Back", NodeShape::LeanLeft),
            ("M", "Top", NodeShape::TrapTop),
            ("N", "Stop", NodeShape::DoubleCircle),
        ]
    );
    assert_eq!(edges(&f)[3..5], [("D", "E"), ("D", "F")], "`&` fans out");
}

#[test]
fn v11_at_shapes_subgraph_endpoints_and_link_styles() {
    let f = flow(
        "flowchart TD\n\
         App[Viewer] --> core\n\
         subgraph core [Rune Core]\n\
           El[El tree] --> Store@{ shape: cyl, label: \"AnimationStore\" }\n\
         end\n\
         core --> GPU@{ shape: hex, label: \"wgpu\" }\n\
         linkStyle 0 stroke:#22c55e,stroke-width:2px",
    );
    assert_eq!(f.nodes.len(), 4, "placeholder node for `core` was redirected and removed");
    assert_eq!(edges(&f), [("App", "El"), ("El", "Store"), ("Store", "GPU")]);
    let store = f.nodes.iter().find(|n| n.id == "Store").unwrap();
    assert_eq!((store.shape, store.label.as_str()), (NodeShape::Cylinder, "AnimationStore"));
    assert!(f.edges[0].style.stroke.is_some());
    assert_eq!(f.edges[0].style.width, Some(2.0));
}

#[test]
fn every_v11_shape_name_reads_and_draws() {
    let names = [
        ("rect", NodeShape::Rect),
        ("event", NodeShape::Round),
        ("pill", NodeShape::Stadium),
        ("subproc", NodeShape::Subroutine),
        ("db", NodeShape::Cylinder),
        ("circ", NodeShape::Circle),
        ("dbl-circ", NodeShape::DoubleCircle),
        ("odd", NodeShape::Asymmetric),
        ("question", NodeShape::Diamond),
        ("prepare", NodeShape::Hexagon),
        ("in-out", NodeShape::LeanRight),
        ("out-in", NodeShape::LeanLeft),
        ("priority", NodeShape::TrapBottom),
        ("manual", NodeShape::TrapTop),
        ("start", NodeShape::SmallCircle),
        ("junction", NodeShape::FilledCircle),
        ("stop", NodeShape::FramedCircle),
        ("summary", NodeShape::CrossedCircle),
        ("bang", NodeShape::Bang),
        ("cloud", NodeShape::Cloud),
        ("card", NodeShape::Card),
        ("collate", NodeShape::Hourglass),
        ("com-link", NodeShape::Bolt),
        ("comment", NodeShape::BraceLeft),
        ("brace-r", NodeShape::BraceRight),
        ("braces", NodeShape::Braces),
        ("delay", NodeShape::Delay),
        ("das", NodeShape::HCylinder),
        ("disk", NodeShape::LinedCylinder),
        ("display", NodeShape::CurvedTrapezoid),
        ("div-proc", NodeShape::DividedRect),
        ("document", NodeShape::Document),
        ("lin-doc", NodeShape::LinedDocument),
        ("docs", NodeShape::Documents),
        ("tag-doc", NodeShape::TaggedDocument),
        ("extract", NodeShape::Triangle),
        ("manual-file", NodeShape::FlippedTriangle),
        ("join", NodeShape::Fork),
        ("internal-storage", NodeShape::WindowPane),
        ("shaded-process", NodeShape::LinedRect),
        ("loop-limit", NodeShape::NotchedPentagon),
        ("manual-input", NodeShape::SlopedRect),
        ("processes", NodeShape::StackedRect),
        ("paper-tape", NodeShape::Flag),
        ("stored-data", NodeShape::BowRect),
        ("tag-rect", NodeShape::TaggedRect),
        ("text", NodeShape::Text),
    ];
    let mut src = String::from("flowchart TD\n");
    for (i, (name, _)) in names.iter().enumerate() {
        src += &format!("  n{i}@{{ shape: {name}, label: \"Step {i}\" }}\n");
        if i > 0 {
            src += &format!("  n{} --> n{i}\n", i - 1);
        }
    }
    let f = flow(&src);
    let got: Vec<NodeShape> = f.nodes.iter().map(|n| n.shape).collect();
    let want: Vec<NodeShape> = names.iter().map(|&(_, s)| s).collect();
    assert_eq!(got, want);
    assert!(f.nodes.iter().all(|n| n.label.starts_with("Step ")));
    assert!(crate::scene(&src, &DiagramStyle::neutral(false), None).is_some());
}

#[test]
fn icons_and_images_draw_as_their_frame() {
    let f = flow("flowchart TD\n  A@{ icon: \"fa:user\", form: \"circle\", label: \"User\" }\n  B@{ img: \"https://example.com/a.png\", label: \"Pic\" }\n  A --> B");
    let nodes: Vec<(&str, NodeShape)> = f.nodes.iter().map(|n| (n.label.as_str(), n.shape)).collect();
    assert_eq!(nodes, [("User", NodeShape::Circle), ("Pic", NodeShape::Rect)]);
}

#[test]
fn named_edges_and_their_settings_add_no_nodes() {
    let f = flow("flowchart LR\n  A e1@--> B\n  B e2@-.-> C\n  e1@{ animate: true }\n  e2@{ animation: fast }");
    assert_eq!(edges(&f), [("A", "B"), ("B", "C")]);
    assert_eq!(f.edges[1].line, Line::Dotted);
    assert_eq!(f.nodes.len(), 3);
}

#[test]
fn link_styles_heads_lengths_and_labels() {
    let f = flow(
        "graph TD\nA --> B\nA --- C\nA -.-> D\nA ==> E\nA -- yes --> F\nA -->|no| G\nA <--> H\nA --o I\nA --x J\nA ---> K\nA -. maybe .-> L\nA == sure ==> M\nA ~~~ N\nA-->O;O-->P",
    );
    let e = |to: &str| f.edges.iter().find(|e| f.nodes[e.to].id == to).expect(to);
    assert_eq!((e("B").line, e("B").end, e("B").minlen), (Line::Solid, Head::Arrow, 1));
    assert_eq!((e("C").line, e("C").end), (Line::Solid, Head::None));
    assert_eq!((e("D").line, e("D").end), (Line::Dotted, Head::Arrow));
    assert_eq!((e("E").line, e("E").end), (Line::Thick, Head::Arrow));
    assert_eq!(e("F").label.as_deref(), Some("yes"));
    assert_eq!(e("G").label.as_deref(), Some("no"));
    assert_eq!((e("H").start, e("H").end), (Head::Arrow, Head::Arrow));
    assert_eq!(e("I").end, Head::Circle);
    assert_eq!(e("J").end, Head::Cross);
    assert_eq!(e("K").minlen, 2);
    assert_eq!((e("L").line, e("L").label.as_deref()), (Line::Dotted, Some("maybe")));
    assert_eq!((e("M").line, e("M").label.as_deref()), (Line::Thick, Some("sure")));
    assert_eq!(e("N").line, Line::Invisible);
    assert_eq!(e("P").from, f.nodes.iter().position(|n| n.id == "O").unwrap(), "`;` separates statements");
}

#[test]
fn quoted_edge_labels_nested_brackets_and_styles() {
    let f = flow(
        "flowchart LR\n\
         A[Audio] --> B[Parakeet writes text<br/>'went to the avatar']\n\
         B --> C{Any word that looks<br/>or sounds like a term?}\n\
         C -- no --> F[Paste as is]\n\
         C -- \"avatar ≈ Abattoir\" --> D[Score the same audio twice:<br/>as 'avatar' and as 'Abattoir']\n\
         D --> E(Call fn(x)) --> G[items[0]]\n\
         G -->|\"a | b\"| F\n\
         style D fill:#dcf5dc",
    );
    let to_d = f.edges.iter().find(|e| f.nodes[e.to].id == "D").unwrap();
    assert_eq!(to_d.label.as_deref(), Some("avatar ≈ Abattoir"));
    let to_f = f.edges.iter().rfind(|e| f.nodes[e.to].id == "F").unwrap();
    assert_eq!(to_f.label.as_deref(), Some("a | b"));
    assert_eq!(f.nodes.iter().find(|n| n.id == "E").unwrap().label, "Call fn(x)");
    assert_eq!(f.nodes.iter().find(|n| n.id == "G").unwrap().label, "items[0]");
    assert!(f.nodes.iter().find(|n| n.id == "D").unwrap().style.fill.is_some());
}

#[test]
fn subgraphs_nest_and_claim_nodes() {
    let f = flow(
        "graph TB\n  c1-->a2\n  subgraph one [The first]\n    a1-->a2\n    subgraph inner\n      x1\n    end\n  end\n  subgraph \"two\"\n    b1-->b2\n  end\n  classDef hot fill:#f00\n  class a1 hot\n  style b1 fill:#0f0\n  click b2 callback\n  a1:::hot --> b1",
    );
    let titles: Vec<&str> = f.subgraphs.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(titles, ["The first", "inner", "two"]);
    assert_eq!(f.subgraphs[1].parent, Some(0));
    let cluster = |id: &str| f.nodes.iter().find(|n| n.id == id).unwrap().cluster;
    assert_eq!(cluster("c1"), None);
    assert_eq!(cluster("a2"), Some(0), "first used outside, then inside");
    assert_eq!(cluster("x1"), Some(1));
    assert_eq!(cluster("b2"), Some(2));
    assert!(f.nodes.iter().find(|n| n.id == "a1").unwrap().style.fill.is_some());
    assert!(f.nodes.iter().find(|n| n.id == "b1").unwrap().style.fill.is_some());
}

#[test]
fn quoted_labels_breaks_and_dashed_ids() {
    let f = flow("graph TD\n  web-server[\"Web (edge) server\"] --> db[(\"Postgres<br/>primary\")]");
    assert_eq!(f.nodes[0].id, "web-server");
    assert_eq!(f.nodes[0].label, "Web (edge) server");
    assert_eq!(f.nodes[1].label, "Postgres\nprimary");
}

#[test]
fn only_sources_where_nothing_reads_are_rejected() {
    for src in ["graph TD", "graph TD\nend", "graph TD\n-->", "graph LR\n  this is not valid", "graph TD\n%% just a comment", "notADiagram\nA --> B"] {
        assert!(crate::parse(src).is_none(), "{src:?}");
    }
}

#[test]
fn an_unclosed_shape_is_a_rectangle_with_its_text_as_written() {
    // Mermaid itself rejects `[/…]` without its `/]`; we draw it as a plain rectangle on purpose,
    // so one stray slash doesn't turn the whole chart back into source.
    let src = "flowchart LR\n  A[List + count] --> B[/items split view]\n  B --> C[Details]\n  C --> D[Pause / Resume / Delete]";
    let f = flow(src);
    let nodes: Vec<(&str, &str, NodeShape)> = f.nodes.iter().map(|n| (n.id.as_str(), n.label.as_str(), n.shape)).collect();
    assert_eq!(
        nodes,
        [
            ("A", "List + count", NodeShape::Rect),
            ("B", "/items split view", NodeShape::Rect),
            ("C", "Details", NodeShape::Rect),
            ("D", "Pause / Resume / Delete", NodeShape::Rect),
        ]
    );
    assert_eq!(edges(&f), [("A", "B"), ("B", "C"), ("C", "D")]);
    assert!(crate::scene(src, &DiagramStyle::neutral(true), None).is_some(), "the whole chart draws, no fallback");
    let label = |src: &str| flow(src).nodes[0].label.clone();
    assert_eq!(label("graph TD\nA[\\back slash]"), "\\back slash");
    assert_eq!(label("graph TD\nA(open round"), "open round");
    assert_eq!(label("graph TD\nA[unclosed --> B"), "unclosed --> B");
    assert_eq!(label("flowchart TD\n  A{\"quote never closes} --> B"), "\"quote never closes} --> B");
    assert_eq!(flow("graph TD\nA@{ shape: not_a_shape } --> B").nodes[0].shape, NodeShape::Rect);
    assert_eq!(flow("graph TD\nA@{ unclosed --> B").nodes[0].label, "A");
}

#[test]
fn punctuation_in_labels_is_plain_text() {
    // `->` inside a cylinder is its label, not an edge; `_`, `*`, `+` and `.` are plain text; and
    // the unclosed `[/…]` (which Mermaid itself rejects) is a rectangle on purpose.
    let src = "flowchart LR\n  a[setting.some_name] --> b[run_* + /go]\n  c[(store -> row)] --> d[Item.Worker]\n  d --> e[/api/v2/item]";
    let f = flow(src);
    let nodes: Vec<(&str, &str, NodeShape)> = f.nodes.iter().map(|n| (n.id.as_str(), n.label.as_str(), n.shape)).collect();
    assert_eq!(
        nodes,
        [
            ("a", "setting.some_name", NodeShape::Rect),
            ("b", "run_* + /go", NodeShape::Rect),
            ("c", "store -> row", NodeShape::Cylinder),
            ("d", "Item.Worker", NodeShape::Rect),
            ("e", "/api/v2/item", NodeShape::Rect),
        ]
    );
    assert_eq!(edges(&f), [("a", "b"), ("c", "d"), ("d", "e")]);
    assert!(crate::scene(src, &DiagramStyle::neutral(false), None).is_some(), "the whole chart draws, no fallback");
}

#[test]
fn a_statement_that_does_not_read_is_skipped() {
    for (src, want) in [
        ("graph XY\nA-->B", vec![("A", "B")]),
        ("graph TD\nA -- --> B\nC --> D", vec![("C", "D")]),
        ("graph TD\nA -->|unclosed B\nC --> D", vec![("C", "D")]),
        ("graph TD\nA - B\nC --> D", vec![("C", "D")]),
        ("graph TD\nsubgraph open\n  A --> B", vec![("A", "B")]),
        ("graph TD\nend\nA --> B\nend", vec![("A", "B")]),
        ("graph TD\nA --> \nC --> D", vec![("C", "D")]),
    ] {
        let f = flow(src);
        assert_eq!(edges(&f), want, "{src:?}");
        let ids: Vec<&str> = f.nodes.iter().map(|n| n.id.as_str()).collect();
        let mut used: Vec<&str> = want.iter().flat_map(|&(a, b)| [a, b]).collect();
        used.dedup();
        if !want.is_empty() {
            assert_eq!(ids, used, "a statement that fails adds no nodes: {src:?}");
        }
    }
}

#[test]
fn going_past_the_bounds_still_rejects() {
    let deep = (0..=crate::MAX_DEPTH).map(|d| format!("subgraph s{d}\nA{d}")).collect::<Vec<_>>().join("\n");
    assert!(crate::parse(&format!("graph TD\n{deep}")).is_none());
    let wide = (0..crate::MAX_ITEMS).map(|i| format!("N{i} --> M{i}")).collect::<Vec<_>>().join("\n");
    assert!(crate::parse(&format!("graph TD\n{wide}")).is_none());
}

#[test]
fn every_prefix_of_a_source_parses_or_falls_back_without_panicking() {
    let src = "flowchart TD\n  A[Start] -->|go| B{Ok?}\n  B -- yes --> C([Done])\n  B -.->|no| A\n  subgraph s [Group]\n    C --> D[(Store)]\n  end";
    let paint = DiagramStyle::neutral(false);
    for (i, _) in src.char_indices() {
        let _ = crate::scene(&src[..i], &paint, None);
    }
    assert!(crate::scene(src, &paint, None).is_some());
}

#[test]
fn boundary_points_sit_on_the_outline() {
    let c = [0.0, 0.0];
    assert_eq!(boundary(NodeShape::Rect, c, [100.0, 40.0], [0.0, 100.0]), [0.0, 20.0]);
    assert_eq!(boundary(NodeShape::Rect, c, [100.0, 40.0], [100.0, 0.0]), [50.0, 0.0]);
    let d = boundary(NodeShape::Diamond, c, [100.0, 40.0], [100.0, 40.0]);
    assert!((d[0].abs() / 50.0 + d[1].abs() / 20.0 - 1.0).abs() < 1e-4);
    let e = boundary(NodeShape::Circle, c, [40.0, 40.0], [30.0, 40.0]);
    assert!((dist(c, e) - 20.0).abs() < 1e-4);
}
