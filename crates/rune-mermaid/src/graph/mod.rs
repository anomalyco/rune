//! Node-and-edge graph diagrams: flowcharts, state, class, entity-relationship, C4,
//! requirement and block diagrams, with lenient `{ … }` block recovery.

pub(crate) mod block;
pub(crate) mod c4;
pub(crate) mod class;
pub(crate) mod er;
pub(crate) mod flowchart;
mod lenient;
pub(crate) mod requirement;
pub(crate) mod state;
