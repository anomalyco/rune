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
        "flowchart LR\n  A[Start] --> B(Round)\n  B --> C([Stadium]) --> D[[Sub]]\n  D --> E[(Database)] & F((Circle))\n  F --> G>Flag] --> H{Decide?} --> I{{Hex}}\n  I --> J[/Lean/] --> K[/Trap\\]",
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
            ("J", "Lean", NodeShape::Parallelogram),
            ("K", "Trap", NodeShape::Trapezoid),
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
fn malformed_and_unsupported_input_is_rejected() {
    for src in [
        "graph XY\nA-->B",
        "graph TD\nA[unclosed --> B",
        "graph TD\nA -- --> B",
        "graph TD\nA -->|unclosed B",
        "graph TD\nA - B",
        "graph TD\nA@{ shape: unknown_shape } --> B",
        "graph TD\nA@{ unclosed --> B",
        "graph TD\nsubgraph open\n  A --> B",
        "graph TD\nend",
        "graph TD\nA --> ",
        "graph TD",
        "flowchart TD\n  A{\"quote never closes} --> B",
    ] {
        assert!(crate::parse(src).is_none(), "{src:?}");
    }
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
