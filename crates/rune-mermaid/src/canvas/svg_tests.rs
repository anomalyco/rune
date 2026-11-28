use crate::{DiagramStyle, Hover, prepare_drawing, render_drawing, render_svg, render_svg_bare};

fn parse_viewbox(svg: &str) -> [f32; 4] {
    let needle = "viewBox=\"";
    let start = svg.find(needle).expect("viewBox present") + needle.len();
    let end = start + svg[start..].find('"').expect("closing quote");
    let nums: Vec<f32> = svg[start..end]
        .split_whitespace()
        .map(|s| s.parse::<f32>().expect("numeric viewBox entry"))
        .collect();
    assert_eq!(nums.len(), 4, "viewBox has 4 numbers: {:?}", &svg[start..end]);
    [nums[0], nums[1], nums[2], nums[3]]
}

fn assert_static_safe(svg: &str) {
    assert!(svg.starts_with(r#"<svg xmlns="http://www.w3.org/2000/svg""#), "svg root: {svg}");
    assert!(svg.trim_end().ends_with("</svg>"), "svg closes cleanly");
    let lower = svg.to_ascii_lowercase();
    for forbidden in ["<script", "<image", "<foreignobject", "@font-face", "url(", "javascript:"] {
        assert!(!lower.contains(forbidden), "svg must not contain {forbidden:?}");
    }
}

#[test]
fn render_svg_emits_framed_card_with_paths_labels_and_title() {
    let src = "---\ntitle: Auth <Flow> & \"Check\"\n---\nflowchart LR\n  A[Start] --> B[End & Done]\n";
    let style = DiagramStyle::neutral(false);
    let svg = render_svg(src, &style).expect("supported flowchart renders to svg");
    assert_static_safe(&svg);

    let vb = parse_viewbox(&svg);
    assert_eq!([vb[0], vb[1]], [0.0, 0.0]);
    assert!(vb[2] > 40.0 && vb[3] > 40.0, "positive viewBox: {vb:?}");

    // Framed card rect with rounded corners, fill, and border.
    assert!(svg.contains(r#"<rect x="0.25" y="0.25""#), "framed card rect present");
    assert!(svg.contains(r#"rx="6" ry="6""#), "card corner radius present");
    assert!(svg.contains(r#"stroke-width="0.5""#), "card border stroke present");

    // Escaped title and node labels, plus fill and stroke paths.
    assert!(svg.contains("Auth &lt;Flow&gt; &amp; &quot;Check&quot;"), "title XML-escaped");
    assert!(svg.contains(">Start</text>"), "Start label present");
    assert!(svg.contains(">End &amp; Done</text>"), "End label XML-escaped");
    assert!(svg.contains("<path d=\"M"), "vector paths present");
    assert!(svg.contains("fill=\"none\" stroke="), "stroke paths present");
}

#[test]
fn bare_variant_omits_card_frame_and_reduces_viewbox_by_card_padding() {
    let src = "flowchart TD
  A[One] --> B[Two]
";
    let style = DiagramStyle::neutral(false);
    let framed = render_svg(src, &style).expect("framed");
    let bare = render_svg_bare(src, &style).expect("bare");
    assert_static_safe(&bare);

    assert!(framed.contains("<rect "), "framed has card rect");
    assert!(!bare.contains("<rect "), "bare omits card rect");

    let fvb = parse_viewbox(&framed);
    let bvb = parse_viewbox(&bare);
    assert!(((fvb[2] - bvb[2]) - 32.0).abs() < 0.25, "width diff is 2*CARD_PAD: {fvb:?} vs {bvb:?}");
    assert!(((fvb[3] - bvb[3]) - 32.0).abs() < 0.25, "height diff is 2*CARD_PAD: {fvb:?} vs {bvb:?}");
}

#[test]
fn drawing_and_drawing_model_to_svg_match_render_svg_and_preserve_fitted_scale() {
    let src = "flowchart TD
  Root --> A[Alpha Service] & B[Beta Gateway] & C[Gamma Router] & D[Delta Worker] & E[Epsilon Store]
";
    let mut style = DiagramStyle::neutral(false);
    style.max_width = 240.0;
    style = style.min_scale(0.2);

    let direct = render_svg(src, &style).expect("render_svg");
    let drawing = render_drawing(src, &style, Hover::Edges, style.text).expect("render_drawing");
    let model = prepare_drawing(src, &style, Hover::Edges, style.text).expect("prepare_drawing");

    let from_drawing = drawing.to_svg();
    let from_model = model.to_svg();
    assert_eq!(from_drawing, direct, "Drawing::to_svg matches render_svg");
    assert_eq!(from_model, direct, "DrawingModel::to_svg matches render_svg");
    assert_eq!(drawing.to_svg_bare(), model.to_svg_bare(), "bare outputs match");
    assert_static_safe(&from_drawing);

    // Wide LR diagram scaled down to fit max_width carries scale(...) in its transform.
    assert!(from_drawing.contains("scale("), "fitted diagram includes scale transform: {from_drawing}");
    let vb = parse_viewbox(&from_drawing);
    assert!(vb[2] <= style.max_width + 1.0, "fitted card viewBox width {} <= max_width {}", vb[2], style.max_width);
}

#[test]
fn multiline_labels_emit_tspans_with_increasing_baselines() {
    let src = "flowchart TD
  A[Line One<br/>Line Two<br/>Line Three]
";
    let style = DiagramStyle::neutral(false);
    let svg = render_svg(src, &style).expect("renders multiline label");
    assert_static_safe(&svg);
    assert!(svg.contains("<tspan "), "multi-line label uses tspans");
    assert!(svg.contains(">Line One</tspan>"));
    assert!(svg.contains(">Line Two</tspan>"));
    assert!(svg.contains(">Line Three</tspan>"));
}

#[test]
fn sequence_state_and_pie_diagrams_serialize_safely_in_light_and_dark_themes() {
    let cases = [
        "sequenceDiagram
  Alice->>Bob: Hello
  Bob-->>Alice: Hi
",
        "stateDiagram-v2
  [*] --> Idle
  Idle --> Active : start
",
        "pie title Pets\n  \"Cats\" : 60\n  \"Dogs\" : 40\n",
    ];
    let light = DiagramStyle::neutral(false);
    let dark = DiagramStyle::neutral(true);
    for src in cases {
        let l = render_svg(src, &light).expect("light svg");
        let d = render_svg(src, &dark).expect("dark svg");
        assert_static_safe(&l);
        assert_static_safe(&d);
        assert!(l.contains("<path ") && l.contains("<text "), "both geometry and text present");
        assert_ne!(l, d, "light and dark styles produce distinct svg fills");
    }
}

#[test]
fn unsupported_or_empty_mermaid_returns_none() {
    let style = DiagramStyle::neutral(false);
    assert!(render_svg("", &style).is_none());
    assert!(render_svg("notADiagram
  foo", &style).is_none());
    assert!(render_svg_bare("unknownKind", &style).is_none());
}
