//! Compact, swappable storage adapter for Taffy's layout solver.
//!
//! Keeps Taffy's Flexbox, CSS Grid, Block, out-of-flow positioning, intrinsic sizing and
//! rounding algorithms while replacing `TaffyTree`'s `SlotMap`s, 552-byte inline `Style`,
//! 408-byte inline `Cache`, dual 92-byte `Layout`s and per-node child `Vec`s with a dense
//! `SparseSet` of compact node slots.

mod cache;
mod children;
pub mod prelude;
mod style;
#[cfg(test)]
mod tests;
mod tree;
mod view;

pub use cache::CompactCache;
pub use children::SmallChildren;
pub use children::SmallChildrenIter;
pub use style::CompactStyle;
pub use taffy::TaffyTree as BaselineTaffyTree;
pub use tree::CompactTaffyTree;
pub use tree::StorageStats;
pub use tree::baseline_storage_stats;

#[cfg(not(feature = "baseline-taffy"))]
pub type TaffyTree<NodeContext = ()> = CompactTaffyTree<NodeContext>;

#[cfg(feature = "baseline-taffy")]
pub type TaffyTree<NodeContext = ()> = BaselineTaffyTree<NodeContext>;
