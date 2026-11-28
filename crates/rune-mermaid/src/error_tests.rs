use std::error::Error;

use crate::{
    DiagramStyle, Hover, MAX_DEPTH, MAX_ITEMS_CEILING, MermaidError, prepare_drawing, render,
    render_drawing, render_svg, render_svg_bare, render_with_hover, try_prepare_drawing,
    try_render, try_render_drawing, try_render_svg, try_render_svg_bare, try_render_with_hover,
};

fn assert_error_trait(err: &MermaidError) -> &dyn Error {
    err
}

#[test]
fn empty_sources_return_empty_error() {
    let style = DiagramStyle::neutral(false);
    for src in [
        "",
        "   \n\t\n",
        "%% just a comment\n%% another",
        "%%{init: {'theme': 'dark'}}%%\n",
        "---\ntitle: Only Front Matter\n---\n",
    ] {
        let err = try_render_svg(src, &style).expect_err(src);
        assert_eq!(err, MermaidError::Empty, "source: {src:?}");
        assert_eq!(err.to_string(), "diagram source is empty");
        let _ = assert_error_trait(&err);
    }
}

#[test]
fn unknown_header_returns_unsupported_diagram_error() {
    let style = DiagramStyle::neutral(false);
    let err = try_render_svg("kanban\n  todo[Task]", &style).unwrap_err();
    assert_eq!(err, MermaidError::UnsupportedDiagram { header: "kanban".into() });
    assert_eq!(err.to_string(), "unsupported diagram type `kanban`");

    let typo = try_render_svg("---\ntitle: T\n---\nflowchrt TD\n  A --> B", &style).unwrap_err();
    assert_eq!(typo, MermaidError::UnsupportedDiagram { header: "flowchrt".into() });
}

#[test]
fn item_participant_member_and_attribute_limits_report_exact_reason() {
    let def = DiagramStyle::neutral(false);
    let small = def.max_items(2);

    // Flowchart item limit
    let err = try_render_svg("flowchart TD\n  A --> B\n  B --> C", &small).unwrap_err();
    assert_eq!(err, MermaidError::ItemLimitExceeded { diagram: "flowchart".into(), limit: 2, kind: "items" });
    assert_eq!(err.to_string(), "flowchart diagram exceeds maximum of 2 items");

    // Sequence participant limit (40)
    let mut many_people = String::from("sequenceDiagram\n");
    for i in 0..41 {
        many_people.push_str(&format!("  participant P{i}\n"));
    }
    let err = try_render_svg(&many_people, &def).unwrap_err();
    assert_eq!(err, MermaidError::ItemLimitExceeded { diagram: "sequenceDiagram".into(), limit: 40, kind: "participants" });
    assert!(err.to_string().contains("40 participants"));

    // Sequence event item limit
    let seq_events = "sequenceDiagram\n  A->>B: 1\n  B->>A: 2\n  A->>B: 3";
    let err = try_render_svg(seq_events, &small).unwrap_err();
    assert_eq!(err, MermaidError::ItemLimitExceeded { diagram: "sequenceDiagram".into(), limit: 2, kind: "items" });

    // Class member limit (4 * max_items)
    let mut many_members = String::from("classDiagram\n  class Big {\n");
    for i in 0..9 {
        many_members.push_str(&format!("    +int f{i}\n"));
    }
    many_members.push_str("  }\n");
    let err = try_render_svg(&many_members, &small).unwrap_err();
    assert_eq!(err, MermaidError::ItemLimitExceeded { diagram: "classDiagram".into(), limit: 8, kind: "members" });

    // ER attribute limit (4 * max_items)
    let mut many_attrs = String::from("erDiagram\n  BIG {\n");
    for i in 0..9 {
        many_attrs.push_str(&format!("    string a{i}\n"));
    }
    many_attrs.push_str("  }\n");
    let err = try_render_svg(&many_attrs, &small).unwrap_err();
    assert_eq!(err, MermaidError::ItemLimitExceeded { diagram: "erDiagram".into(), limit: 8, kind: "attributes" });

    // Chart item limit (pie)
    let pie_over = "pie\n  \"A\" : 1\n  \"B\" : 2\n  \"C\" : 3";
    let err = try_render_svg(pie_over, &small).unwrap_err();
    assert_eq!(err, MermaidError::ItemLimitExceeded { diagram: "pie".into(), limit: 2, kind: "items" });
}

#[test]
fn depth_limits_return_depth_limit_exceeded() {
    let high = DiagramStyle::neutral(false).max_items(MAX_ITEMS_CEILING);

    let flow_deep = format!("graph TD\n{}A\n{}", "subgraph s\n".repeat(MAX_DEPTH + 1), "end\n".repeat(MAX_DEPTH + 1));
    let err = try_render_svg(&flow_deep, &high).unwrap_err();
    assert_eq!(err, MermaidError::DepthLimitExceeded { diagram: "graph".into(), limit: MAX_DEPTH });
    assert_eq!(err.to_string(), format!("graph diagram exceeds maximum nesting depth of {MAX_DEPTH}"));

    let state_deep = format!("stateDiagram-v2\n{}A\n{}", "state S {\n".repeat(MAX_DEPTH + 1), "}\n".repeat(MAX_DEPTH + 1));
    assert_eq!(
        try_render_svg(&state_deep, &high).unwrap_err(),
        MermaidError::DepthLimitExceeded { diagram: "stateDiagram-v2".into(), limit: MAX_DEPTH }
    );

    let seq_deep = format!("sequenceDiagram\n{}A->>B: hi\n{}", "loop x\n".repeat(MAX_DEPTH + 1), "end\n".repeat(MAX_DEPTH + 1));
    assert_eq!(
        try_render_svg(&seq_deep, &high).unwrap_err(),
        MermaidError::DepthLimitExceeded { diagram: "sequenceDiagram".into(), limit: MAX_DEPTH }
    );

    let block_deep = format!("block-beta\n{}A\n{}", "block\n".repeat(MAX_DEPTH + 1), "end\n".repeat(MAX_DEPTH + 1));
    assert_eq!(
        try_render_svg(&block_deep, &high).unwrap_err(),
        MermaidError::DepthLimitExceeded { diagram: "block-beta".into(), limit: MAX_DEPTH }
    );

    let c4_deep = format!("C4Context\n{}Person(u, \"U\")\n{}", "Boundary(b, \"B\") {\n".repeat(MAX_DEPTH + 1), "}\n".repeat(MAX_DEPTH + 1));
    assert_eq!(
        try_render_svg(&c4_deep, &high).unwrap_err(),
        MermaidError::DepthLimitExceeded { diagram: "C4Context".into(), limit: MAX_DEPTH }
    );

    let mut mindmap_deep = String::from("mindmap\nroot\n");
    for d in 1..=(MAX_DEPTH + 1) {
        mindmap_deep.push_str(&format!("{}n{d}\n", "  ".repeat(d)));
    }
    assert_eq!(
        try_render_svg(&mindmap_deep, &high).unwrap_err(),
        MermaidError::DepthLimitExceeded { diagram: "mindmap".into(), limit: MAX_DEPTH }
    );
}

#[test]
fn dense_tangled_graph_returns_layout_limit_exceeded() {
    let high = DiagramStyle::neutral(true).max_items(MAX_ITEMS_CEILING);
    let mut tangled = vec!["graph LR".to_owned()];
    for i in 0..130 {
        tangled.push(format!("N{i} --> N{} & N{}", (i * 7 + 1) % 130, (i * 13 + 5) % 130));
    }
    let src = tangled.join("\n");
    let err = try_render_svg(&src, &high).unwrap_err();
    assert_eq!(err, MermaidError::LayoutLimitExceeded { diagram: "graph".into() });
    assert_eq!(err.to_string(), "graph diagram exceeds layout complexity limit");
}

#[test]
fn no_items_reports_metadata_only_and_first_unrecognized_statement_with_line_number() {
    let style = DiagramStyle::neutral(false);

    // Metadata / header only -> no unrecognized statement
    let err = try_render_svg("flowchart TD\n  %% only a comment\n  title Empty", &style).unwrap_err();
    assert_eq!(err, MermaidError::NoItems { diagram: "flowchart".into(), line: None, statement: None });
    assert_eq!(err.to_string(), "flowchart diagram has no valid items");

    // Unrecognized statement after front matter and comment preserves 1-based original line
    let src = "---\ntitle: Sample\n---\n%% comment\nflowchart LR\n  this is not valid\n  also bad";
    let err = try_render_svg(src, &style).unwrap_err();
    assert_eq!(
        err,
        MermaidError::NoItems {
            diagram: "flowchart".into(),
            line: Some(6),
            statement: Some("this is not valid".into()),
        }
    );
    assert_eq!(
        err.to_string(),
        "flowchart diagram has no valid items (first unrecognized statement at line 6: `this is not valid`)"
    );

    // Unrecognized statements across other diagram families
    for (src, diagram, line, stmt) in [
        ("sequenceDiagram\n  not a sequence message", "sequenceDiagram", 2, "not a sequence message"),
        ("stateDiagram-v2\n  not valid state syntax", "stateDiagram-v2", 2, "not valid state syntax"),
        ("classDiagram\n  not a class", "classDiagram", 2, "not a class"),
        ("erDiagram\n  nothing here at all", "erDiagram", 2, "nothing here at all"),
        ("gantt\n  title T\n  broken task", "gantt", 3, "broken task"),
        ("pie\n  \"bad\" : -5", "pie", 2, "\"bad\" : -5"),
        ("journey\n  bad task without score", "journey", 2, "bad task without score"),
        ("gitGraph\n  bogusCmd foo", "gitGraph", 2, "bogusCmd foo"),
        ("quadrantChart\n  bad line", "quadrantChart", 2, "bad line"),
        ("xychart-beta\n  bar bad", "xychart-beta", 2, "bar bad"),
        ("sankey-beta\n  not,a number,here", "sankey-beta", 2, "not,a number,here"),
        ("requirementDiagram\n  nonsense", "requirementDiagram", 2, "nonsense"),
        ("C4Context\n  not a c4 call", "C4Context", 2, "not a c4 call"),
        ("timeline\n  : orphan event", "timeline", 2, ": orphan event"),
    ] {
        let err = try_render_svg(src, &style).unwrap_err();
        assert_eq!(
            err,
            MermaidError::NoItems {
                diagram: diagram.into(),
                line: Some(line),
                statement: Some(stmt.into()),
            },
            "source: {src:?}"
        );
    }

    // Long multibyte statement truncates safely on a char boundary
    let long = format!("flowchart TD\n  bad {}", "é".repeat(120));
    let err = try_render_svg(&long, &style).unwrap_err();
    match err {
        MermaidError::NoItems { statement: Some(stmt), .. } => {
            assert!(stmt.ends_with('…'));
            assert_eq!(stmt.chars().count(), 81);
        }
        other => panic!("expected NoItems with truncated statement, got {other:?}"),
    }
}

#[test]
fn lenient_skipping_still_succeeds_when_valid_items_exist() {
    let style = DiagramStyle::neutral(false);
    let src = "flowchart LR\n  this line is skipped\n  A[Start] --> B[End]";
    let svg = try_render_svg(src, &style).expect("lenient diagram with valid nodes draws");
    assert!(svg.contains(">Start</text>"));
    assert!(svg.contains(">End</text>"));
}

#[test]
fn internal_error_implements_display_and_error() {
    let err = MermaidError::InternalError;
    assert_eq!(err.to_string(), "internal layout error");
    let _ = assert_error_trait(&err);
}

#[test]
fn all_try_entry_points_match_option_companions_and_cache_errors() {
    let style = DiagramStyle::neutral(false);
    let ok_src = "flowchart LR\n  A[One] --> B[Two]";
    let bad_src = "unknownKind\n  A --> B";
    let expected = MermaidError::UnsupportedDiagram { header: "unknownKind".into() };

    assert!(render(ok_src, &style).is_some());
    assert!(try_render(ok_src, &style).is_ok());
    assert!(render(bad_src, &style).is_none());
    assert_eq!(try_render(bad_src, &style).err(), Some(expected.clone()));

    assert!(render_with_hover(ok_src, &style, Hover::Edges, style.text).is_some());
    assert!(try_render_with_hover(ok_src, &style, Hover::Edges, style.text).is_ok());
    assert_eq!(try_render_with_hover(bad_src, &style, Hover::Edges, style.text).err(), Some(expected.clone()));

    assert!(render_drawing(ok_src, &style, Hover::Off, style.text).is_some());
    assert!(try_render_drawing(ok_src, &style, Hover::Off, style.text).is_ok());
    assert_eq!(try_render_drawing(bad_src, &style, Hover::Off, style.text).err(), Some(expected.clone()));

    assert!(prepare_drawing(ok_src, &style, Hover::Off, style.text).is_some());
    assert!(try_prepare_drawing(ok_src, &style, Hover::Off, style.text).is_ok());
    assert_eq!(try_prepare_drawing(bad_src, &style, Hover::Off, style.text).err(), Some(expected.clone()));

    assert_eq!(try_render_svg(ok_src, &style).ok(), render_svg(ok_src, &style));
    assert_eq!(try_render_svg(bad_src, &style), Err(expected.clone()));
    assert_eq!(try_render_svg_bare(ok_src, &style).ok(), render_svg_bare(ok_src, &style));
    assert_eq!(try_render_svg_bare(bad_src, &style), Err(expected));
}
