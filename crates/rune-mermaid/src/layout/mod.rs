//! Layered (Sugiyama) graph layout, within-layer ordering, edge routing, connectors,
//! shared layered scene rendering and monotonic streaming stability.

pub(crate) mod connector;
pub(crate) mod draw;
pub(crate) mod layered;
mod order;
pub(crate) mod route;
pub(crate) mod steady;
