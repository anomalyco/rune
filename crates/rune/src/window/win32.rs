//! Windows-specific window host pieces: CPU first-frame rendering and GPU hand-over,
//! deferred post-frame chores, occlusion look, covered redraws, damage planning, swap-chain
//! waitable slots and DirectComposition video layers.

pub(in crate::window) mod covered_draw;
pub(in crate::window) mod cpu_frame;
pub(in crate::window) mod damage;
pub(in crate::window) mod handover;
pub(in crate::window) mod look;
pub(in crate::window) mod post_frame;
pub(crate) mod slot;
pub(in crate::window) mod video_layer;
