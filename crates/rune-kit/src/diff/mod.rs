//! A diff view for reviewing changes: files of hunks, unified or split, with word changes,
//! syntax colours, expandable unchanged ranges, line annotations and line selection, on a
//! virtual list that keeps a fifty-thousand-line diff at frame rate.
//!
//! [`parse`] reads a patch (`git diff`) and [`FileDiff::from_texts`] diffs two texts. The app
//! keeps a [`DiffState`] of the files in its state and draws it with [`DiffView`].

mod copy;
mod model;
mod myers;
mod pairs;
mod state;
mod view;
mod words;

pub use model::{FileDiff, FileStatus, Hunk, Line, LineKind, Side, parse};
pub use pairs::SplitRow;
pub use state::{Annotation, DiffLine, DiffMode, DiffState, EXPAND_STEP, Expand, LineRange, LineRef};
pub use view::{DiffView, FoldRow};
