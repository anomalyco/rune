//! Native ```` ```mermaid ```` diagrams inside [`crate::Markdown`], backed by [`rune_mermaid`].
//!
//! Covers flowcharts (`graph`/`flowchart` in `TD`/`TB`/`BT`/`LR`/`RL` with subgraphs, all node
//! shapes including `@{ shape: … }`, link styles and `style`/`classDef`/`linkStyle` tints),
//! sequence diagrams (`sequenceDiagram` with participants and their v11 types, actors, `box`
//! groups, `rect` bands, activations, notes, frames, `create`/`destroy` and `autonumber`), state
//! diagrams (`stateDiagram`/`stateDiagram-v2` with `[*]`, composite states and `--` regions,
//! notes, `<<choice>>` and `<<fork>>`/`<<join>>`), class diagrams (`classDiagram` with fields,
//! methods, stereotypes, namespaces and UML relations), entity-relationship diagrams
//! (`erDiagram` with attribute tables and crow's-foot cardinalities), gantt charts (`gantt` with
//! sections, durations, `after`/`until` links and `excludes`), pie charts (`pie`, with
//! `showData`), mindmaps (`mindmap`), timelines (`timeline`), user journeys (`journey`), git
//! graphs (`gitGraph` with branches, merges, cherry-picks, tags and commit types), quadrant
//! charts (`quadrantChart`), XY charts (`xychart-beta` bars and lines, upright or horizontal),
//! sankey diagrams (`sankey-beta`), block diagrams (`block-beta` with columns, spans, groups and
//! block arrows), requirement diagrams (`requirementDiagram`) and C4 diagrams (`C4Context`,
//! `C4Container`, `C4Component`, `C4Dynamic` and `C4Deployment`). Lines that don't read are
//! skipped and the rest draws; an unknown diagram type, or a source where nothing reads, returns
//! `None`, and [`crate::Markdown`] falls back to the code block.

use rune::El;

use crate::markdown::{DiagramPaint, MarkdownStyle};

pub use rune_mermaid::{
    DiagramStyle, Drawing, DrawingModel, Hover, MermaidError, may_draw, render_svg,
    render_svg_bare, supported, supported_with, try_prepare_drawing, try_render_drawing,
    try_render_svg, try_render_svg_bare, try_render_with_hover,
};

/// Prepare the drawn label runs for a viewer that searches and highlights a diagram.
pub fn prepare(src: &str, style: &MarkdownStyle, max_width: f32) -> Option<DrawingModel> {
    try_prepare(src, style, max_width).ok()
}

/// Like [`prepare`], returning a [`MermaidError`] explaining why `src` produced no diagram.
pub fn try_prepare(src: &str, style: &MarkdownStyle, max_width: f32) -> Result<DrawingModel, MermaidError> {
    let paint = &style.diagram;
    let mut full = self::style(paint);
    full.max_width = max_width;
    rune_mermaid::try_prepare_drawing(src, &full, hover(paint), paint.hover)
}

/// `src` drawn as a diagram card with `style`'s diagram colours, or `None` when it is outside
/// the supported subset.
pub fn render(src: &str, style: &MarkdownStyle) -> Option<El> {
    try_render(src, style).ok()
}

/// Like [`render`], returning a [`MermaidError`] explaining why `src` produced no diagram.
pub fn try_render(src: &str, style: &MarkdownStyle) -> Result<El, MermaidError> {
    let paint = &style.diagram;
    let card = rune_mermaid::try_render_with_hover(src, &self::style(paint), hover(paint), paint.hover)?;
    Ok(card.role("figure").label(kind(src)).description(src.trim()))
}

/// `src` rendered as a static, self-contained framed-card SVG document with `style`'s diagram
/// colours, or `None` when it is outside the supported subset.
pub fn svg(src: &str, style: &MarkdownStyle) -> Option<String> {
    try_svg(src, style).ok()
}

/// Like [`svg`], returning a [`MermaidError`] explaining why `src` produced no diagram.
pub fn try_svg(src: &str, style: &MarkdownStyle) -> Result<String, MermaidError> {
    rune_mermaid::try_render_svg(src, &self::style(&style.diagram))
}

/// Like [`svg`], without the surrounding card frame or card padding.
pub fn svg_bare(src: &str, style: &MarkdownStyle) -> Option<String> {
    try_svg_bare(src, style).ok()
}

/// Like [`svg_bare`], returning a [`MermaidError`] explaining why `src` produced no diagram.
pub fn try_svg_bare(src: &str, style: &MarkdownStyle) -> Result<String, MermaidError> {
    rune_mermaid::try_render_svg_bare(src, &self::style(&style.diagram))
}

/// [`render`]'s title and drawing without the card, laid out for a card `max_width` points wide
/// rather than `style`'s column: a lightbox passes the window's width, so a wide flowchart draws
/// as it reads best instead of turned to fit the column, and a chart fills the room. For a
/// viewer that frames and scales the drawing itself.
pub fn drawing(src: &str, style: &MarkdownStyle, max_width: f32) -> Option<Drawing> {
    try_drawing(src, style, max_width).ok()
}

/// Like [`drawing`], returning a [`MermaidError`] explaining why `src` produced no diagram.
pub fn try_drawing(src: &str, style: &MarkdownStyle, max_width: f32) -> Result<Drawing, MermaidError> {
    let paint = &style.diagram;
    let mut full = self::style(paint);
    full.max_width = max_width;
    let mut d = rune_mermaid::try_render_drawing(src, &full, hover(paint), paint.hover)?;
    d.el = std::mem::replace(&mut d.el, rune::el()).role("figure").label(kind(src)).description(src.trim());
    Ok(d)
}

fn hover(paint: &DiagramPaint) -> Hover {
    match (paint.hover.a > 0.0, paint.hover_neighbours) {
        (false, _) => Hover::Off,
        (true, false) => Hover::Edges,
        (true, true) => Hover::Neighbours,
    }
}

/// `streaming`: the reply is still arriving, so what's new since the last build comes in on its
/// own; `open`: `src` is the complete lines of a fence still arriving, so the diagram only grows
/// ([`rune_mermaid::render_streaming`]) until the fence closes and it settles
/// ([`rune_mermaid::render_streamed`]).
pub(crate) fn diagram(src: &str, paint: &DiagramPaint, streaming: bool, open: bool) -> Option<El> {
    let render = match (streaming, open) {
        (false, _) => rune_mermaid::render_with_hover,
        (true, true) => rune_mermaid::render_streaming,
        (true, false) => rune_mermaid::render_streamed,
    };
    let card = render(src, &style(paint), hover(paint), paint.hover)?;
    // A drawing says nothing to a screen reader: it reads the diagram's kind, then its source.
    Some(card.role("figure").label(kind(src)).description(src.trim()))
}

/// What a diagram's first word says it is, as a reader names it.
fn kind(src: &str) -> &'static str {
    let first = src.split_whitespace().find(|w| !w.starts_with("%%")).unwrap_or("");
    match first.trim_end_matches("-beta").trim_end_matches("-v2") {
        "graph" | "flowchart" => "Flowchart",
        "sequenceDiagram" => "Sequence diagram",
        "stateDiagram" => "State diagram",
        "classDiagram" => "Class diagram",
        "erDiagram" => "Entity relationship diagram",
        "gantt" => "Gantt chart",
        "pie" => "Pie chart",
        "mindmap" => "Mind map",
        "timeline" => "Timeline",
        "journey" => "User journey",
        "gitGraph" => "Git graph",
        "quadrantChart" => "Quadrant chart",
        "xychart" => "XY chart",
        "sankey" => "Sankey diagram",
        "block" => "Block diagram",
        "requirementDiagram" => "Requirement diagram",
        c if c.starts_with("C4") => "C4 diagram",
        _ => "Diagram",
    }
}

/// `paint` as `rune_mermaid`'s style.
fn style(paint: &DiagramPaint) -> DiagramStyle {
    let mut s = DiagramStyle::neutral(false).min_scale(paint.min_scale).max_items(paint.max_items);
    s.text = paint.text;
    s.muted = paint.muted;
    s.surface = paint.surface;
    s.surface_border = paint.surface_border;
    s.node_fill = paint.node_fill;
    s.node_border = paint.node_border;
    s.line = paint.line;
    s.cluster_fill = paint.cluster_fill;
    s.note_fill = paint.note_fill;
    s.note_border = paint.note_border;
    s.font = paint.font;
    s.max_width = paint.max_width;
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kit_mermaid_svg_and_reexports_render_framed_and_bare() {
        let md = MarkdownStyle::neutral(false);
        let src = "flowchart LR\n  A[One] --> B[Two]\n";
        let framed = svg(src, &md).expect("kit mermaid::svg");
        let bare = svg_bare(src, &md).expect("kit mermaid::svg_bare");
        let direct = render_svg(src, &DiagramStyle::neutral(false)).expect("re-exported render_svg");
        assert!(framed.starts_with(r#"<svg xmlns="http://www.w3.org/2000/svg""#));
        assert!(framed.contains("<rect "));
        assert!(!bare.contains("<rect "));
        assert!(direct.contains(">One</text>"));
        assert!(svg("unsupported", &md).is_none());
    }

    #[test]
    fn kit_mermaid_try_apis_expose_errors_and_preserve_option_behavior() {
        let md = MarkdownStyle::neutral(false);
        let valid = "flowchart LR\n  A[One] --> B[Two]\n";
        let unknown = "kanban\n  col[Item]";
        let empty = "   \n%% only comment";
        let no_items = "flowchart LR\n  not valid syntax";

        assert_eq!(try_svg(valid, &md).ok(), svg(valid, &md));
        assert_eq!(try_svg_bare(valid, &md).ok(), svg_bare(valid, &md));
        assert!(try_render(valid, &md).is_ok());
        assert!(try_drawing(valid, &md, 680.0).is_ok());
        assert!(try_prepare(valid, &md, 680.0).is_ok());
        assert!(try_render_svg(valid, &DiagramStyle::neutral(false)).is_ok());
        assert!(try_render_svg_bare(valid, &DiagramStyle::neutral(false)).is_ok());

        assert_eq!(try_svg(unknown, &md), Err(MermaidError::UnsupportedDiagram { header: "kanban".into() }));
        assert_eq!(try_svg_bare(empty, &md), Err(MermaidError::Empty));
        assert_eq!(
            try_render(no_items, &md).err(),
            Some(MermaidError::NoItems {
                diagram: "flowchart".into(),
                line: Some(2),
                statement: Some("not valid syntax".into()),
            })
        );
        assert_eq!(try_drawing(unknown, &md, 680.0).err(), Some(MermaidError::UnsupportedDiagram { header: "kanban".into() }));
        assert_eq!(try_prepare(unknown, &md, 680.0).err(), Some(MermaidError::UnsupportedDiagram { header: "kanban".into() }));
        assert_eq!(
            try_render_svg(unknown, &DiagramStyle::neutral(false)),
            Err(MermaidError::UnsupportedDiagram { header: "kanban".into() })
        );

        let small = MarkdownStyle::neutral(false).diagram_max_items(2);
        assert_eq!(
            try_svg("flowchart TD\n  A --> B\n  B --> C", &small),
            Err(MermaidError::ItemLimitExceeded { diagram: "flowchart".into(), limit: 2, kind: "items" })
        );
    }

    #[test]
    fn markdown_mermaid_fence_rendering_and_fallback_remain_unchanged() {
        let md = MarkdownStyle::neutral(false);
        // Valid fence renders a diagram card; invalid fence returns None so Markdown falls back to code block.
        assert!(diagram("flowchart LR\n  A --> B", &md.diagram, false, false).is_some());
        assert!(diagram("kanban\n  todo", &md.diagram, false, false).is_none());
        let doc = "```mermaid\nflowchart LR\n  A --> B\n```\n\n```mermaid\nkanban\n  todo\n```\n".to_owned();
        let mut rt = rune::Runtime::new(
            doc,
            |src: &String, _: &mut rune::Cx<String>| {
                let st = MarkdownStyle::neutral(false);
                rune::col().w(680.0).child(crate::Markdown::new(src).render(&st).key("md"))
            },
            (680.0, 800.0),
            rune::Env::new(false, 1.0),
        );
        rt.frame(0.0);
        let keys: Vec<String> = rt.elements().into_iter().filter_map(|e| e.key).collect();
        assert!(keys.iter().any(|k| k == "diagram"), "valid fence renders diagram: {keys:?}");
        assert!(keys.iter().any(|k| k == "source"), "invalid fence falls back to code block: {keys:?}");
    }
}
