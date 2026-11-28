//! Error reporting for Mermaid diagrams that produce no drawable scene.

use std::fmt;

/// Why a Mermaid diagram source could not be parsed or laid out.
///
/// `rune-mermaid` is intentionally lenient: a diagram that draws at least one valid item still
/// succeeds even if individual unrecognized statements were skipped. A `MermaidError` is returned
/// only when nothing could be rendered.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MermaidError {
    /// The source contained no diagram content (only whitespace, comments, directives, or front
    /// matter).
    Empty,
    /// The diagram header names an unsupported or misspelled diagram type (for example `kanban`).
    UnsupportedDiagram {
        /// The unrecognized leading header keyword.
        header: String,
    },
    /// The diagram exceeded a configured item, participant, member, or attribute limit.
    ItemLimitExceeded {
        /// The recognized diagram header (for example `"flowchart"` or `"sequenceDiagram"`).
        diagram: String,
        /// The bound that was exceeded.
        limit: usize,
        /// Which bounded resource hit the limit (`"items"`, `"participants"`, `"members"`, or
        /// `"attributes"`).
        kind: &'static str,
    },
    /// Nested blocks (subgraphs, composite states, sequence frames, block groups, C4 boundaries,
    /// or mindmap indentation) exceeded the maximum nesting depth.
    DepthLimitExceeded {
        /// The recognized diagram header.
        diagram: String,
        /// The maximum supported nesting depth.
        limit: usize,
    },
    /// Layered graph layout exceeded its edge-routing complexity limit (too many long-span
    /// crossing edges).
    LayoutLimitExceeded {
        /// The recognized diagram header.
        diagram: String,
    },
    /// The diagram header was recognized, but no valid drawable items were parsed.
    NoItems {
        /// The recognized diagram header.
        diagram: String,
        /// 1-based line number of the first unrecognized statement in the source, if any.
        line: Option<usize>,
        /// The first unrecognized statement encountered, if any.
        statement: Option<String>,
    },
    /// An unexpected internal panic occurred while laying out the diagram.
    InternalError,
}

impl fmt::Display for MermaidError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "diagram source is empty"),
            Self::UnsupportedDiagram { header } => {
                write!(f, "unsupported diagram type `{header}`")
            }
            Self::ItemLimitExceeded { diagram, limit, kind } => {
                write!(f, "{diagram} diagram exceeds maximum of {limit} {kind}")
            }
            Self::DepthLimitExceeded { diagram, limit } => {
                write!(f, "{diagram} diagram exceeds maximum nesting depth of {limit}")
            }
            Self::LayoutLimitExceeded { diagram } => {
                write!(f, "{diagram} diagram exceeds layout complexity limit")
            }
            Self::NoItems { diagram, line: Some(line), statement: Some(stmt) } => {
                write!(
                    f,
                    "{diagram} diagram has no valid items (first unrecognized statement at line {line}: `{stmt}`)"
                )
            }
            Self::NoItems { diagram, line: None, statement: Some(stmt) } => {
                write!(
                    f,
                    "{diagram} diagram has no valid items (first unrecognized statement: `{stmt}`)"
                )
            }
            Self::NoItems { diagram, .. } => {
                write!(f, "{diagram} diagram has no valid items")
            }
            Self::InternalError => write!(f, "internal layout error"),
        }
    }
}

impl std::error::Error for MermaidError {}

/// A cleaned line of diagram source together with its original 1-based line number and leading
/// indentation.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SourceLine<'a> {
    pub line: usize,
    pub indent: usize,
    pub text: &'a str,
}

/// Internal failure returned by an individual diagram parser before attaching the header keyword.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ParseFail {
    ItemLimit { limit: usize, kind: &'static str },
    DepthLimit(usize),
    NoItems { first_unrecognized: Option<(usize, String)> },
}

impl ParseFail {
    pub(crate) fn into_error(self, diagram: &str) -> MermaidError {
        let diagram = diagram.to_owned();
        match self {
            Self::ItemLimit { limit, kind } => MermaidError::ItemLimitExceeded { diagram, limit, kind },
            Self::DepthLimit(limit) => MermaidError::DepthLimitExceeded { diagram, limit },
            Self::NoItems { first_unrecognized } => {
                let (line, statement) = match first_unrecognized {
                    Some((l, s)) => (Some(l), Some(s)),
                    None => (None, None),
                };
                MermaidError::NoItems { diagram, line, statement }
            }
        }
    }
}

/// Records the first unrecognized statement encountered while leniently parsing a diagram.
#[derive(Clone, Debug, Default)]
pub(crate) struct Skipped {
    first: Option<(usize, String)>,
}

impl Skipped {
    pub(crate) fn record(&mut self, line: usize, stmt: &str) {
        if self.first.is_none() {
            let trimmed = stmt.trim();
            if !trimmed.is_empty() {
                self.first = Some((line, truncate_stmt(trimmed, 80)));
            }
        }
    }

    pub(crate) fn into_no_items(self) -> ParseFail {
        ParseFail::NoItems { first_unrecognized: self.first }
    }
}

fn truncate_stmt(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let prefix: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
