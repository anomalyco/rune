use super::*;
use crate::PointerPress;
use crate::el::{col, el};

mod app_ime;
mod context_menu_key;
mod event_markers;
mod glyph_replay;
mod group_opacity;
mod id_pruning;
mod input_clipboard;
mod layout_hover;
mod layout_reads;
mod layout_rows;
mod loop_sleep;
mod no_motion;
mod picture_scale;
mod pointer_drag;
mod pointer_hit;
mod scroll_children;
mod scroll_growth;
mod scroll_hover;
mod secure_text;
mod shimmer;
mod springs;
mod text_keys;
mod text_rebreak;
mod window_wake;

thread_local! {
    /// Runs once inside the next frame, after layout and just before live values latch: a
    /// publication from another thread landing at the worst moment.
    pub(super) static MID_FRAME: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

fn duplicate_view(_: &(), _: &mut Cx<()>) -> El {
    col().children([el().key("card"), el().key("card")])
}
