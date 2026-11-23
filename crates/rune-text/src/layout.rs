//! Text shaping, line breaking, caching, spans, decorations, cursors and runs.

pub(crate) mod cache;
pub(crate) mod case;
pub(crate) mod cursor;
pub(crate) mod decorations;
pub(crate) mod hash;
pub(crate) mod id;
pub(crate) mod inline_fit;
pub(crate) mod line_break;
pub(crate) mod line_metrics;
pub(crate) mod ltr_units;
pub(crate) mod pill_chips;
pub(crate) mod pretty;
pub(crate) mod rich;
pub(crate) mod run;
mod shape;
pub(crate) mod span;
pub(crate) mod span_map;
pub(crate) mod underline;
