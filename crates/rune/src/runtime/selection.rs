//! Text selection, links, text marks, field click/caret handling and select-all.
use super::*;

pub(super) mod caret;
pub(super) mod click;
pub(super) mod field_lines;
pub(crate) mod image_menu;
mod link_decos;
pub(crate) mod link_menu;
pub(super) mod links;
pub(crate) mod positioned;
pub(super) mod select_all;
pub(crate) mod selectable;
pub(super) mod text_marks;
pub(super) mod visibility;

#[cfg(test)]
mod selection_extend_tests;
