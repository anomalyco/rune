//! Text selection, links, text marks, field click/caret handling and select-all.
use super::*;

pub(super) mod caret;
pub(super) mod click;
mod link_decos;
pub(super) mod links;
pub(super) mod select_all;
pub(crate) mod selectable;
pub(super) mod text_marks;
