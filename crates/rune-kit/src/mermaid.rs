//! Native ```` ```mermaid ```` diagrams inside [`crate::Markdown`], backed by [`rune_mermaid`].
//!
//! Covers flowcharts (`graph`/`flowchart` in `TD`/`TB`/`BT`/`LR`/`RL` with subgraphs, all node
//! shapes including `@{ shape: … }`, link styles and `style`/`classDef`/`linkStyle` tints),
//! sequence diagrams (`sequenceDiagram` with participants, actors, `box` groups, `rect` bands,
//! activations, notes and frames), state diagrams (`stateDiagram`/`stateDiagram-v2` with `[*]`,
//! composite states and `<<choice>>`) and class diagrams (`classDiagram` with fields, methods,
//! stereotypes, namespaces and UML relations). Anything outside the supported subset returns
//! `None`, and [`crate::Markdown`] falls back to the code block.

use rune::El;

use crate::markdown::{DiagramPaint, MarkdownStyle};

pub use rune_mermaid::{DiagramStyle, Hover, supported};

/// `src` drawn as a diagram card with `style`'s diagram colours, or `None` when it is outside
/// the supported subset.
pub fn render(src: &str, style: &MarkdownStyle) -> Option<El> {
    diagram(src, &style.diagram)
}

pub(crate) fn diagram(src: &str, paint: &DiagramPaint) -> Option<El> {
    rune_mermaid::render_with_hover(
        src,
        &DiagramStyle {
            text: paint.text,
            muted: paint.muted,
            surface: paint.surface,
            surface_border: paint.surface_border,
            node_fill: paint.node_fill,
            node_border: paint.node_border,
            line: paint.line,
            cluster_fill: paint.cluster_fill,
            note_fill: paint.note_fill,
            note_border: paint.note_border,
            font: paint.font,
            max_width: paint.max_width,
        },
        match (paint.hover.a > 0.0, paint.hover_neighbours) {
            (false, _) => Hover::Off,
            (true, false) => Hover::Edges,
            (true, true) => Hover::Neighbours,
        },
        paint.hover,
    )
}
