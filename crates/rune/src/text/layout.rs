//! Text shaping, line breaking, caching, spans, decorations, cursors and runs.

pub(in crate::text) mod cache;
pub(in crate::text) mod case;
pub(in crate::text) mod cursor;
pub(in crate::text) mod decorations;
pub(in crate::text) mod hash;
pub(in crate::text) mod id;
pub(in crate::text) mod inline_fit;
pub(in crate::text) mod line_break;
pub(in crate::text) mod line_metrics;
pub(in crate::text) mod ltr_units;
pub(in crate::text) mod pill_chips;
pub(in crate::text) mod pretty;
pub(in crate::text) mod rich;
pub(in crate::text) mod run;
mod shape;
pub(in crate::text) mod span;
pub(in crate::text) mod underline;
