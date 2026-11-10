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

pub use rune_mermaid::{DiagramStyle, Drawing, Hover, may_draw, supported};

/// `src` drawn as a diagram card with `style`'s diagram colours, or `None` when it is outside
/// the supported subset.
pub fn render(src: &str, style: &MarkdownStyle) -> Option<El> {
    diagram(src, &style.diagram, false, false)
}

/// [`render`]'s title and drawing without the card, laid out for a card `max_width` points wide
/// rather than `style`'s column: a lightbox passes the window's width, so a wide flowchart draws
/// as it reads best instead of turned to fit the column, and a chart fills the room. For a
/// viewer that frames and scales the drawing itself.
pub fn drawing(src: &str, style: &MarkdownStyle, max_width: f32) -> Option<Drawing> {
    let paint = &style.diagram;
    let mut full = self::style(paint);
    full.max_width = max_width;
    let mut d = rune_mermaid::render_drawing(src, &full, hover(paint), paint.hover)?;
    d.el = std::mem::replace(&mut d.el, rune::el()).role("figure").label(kind(src)).description(src.trim());
    Some(d)
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
    let mut s = DiagramStyle::neutral(false).min_scale(paint.min_scale);
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
