//! Windows-specific window host pieces: CPU first-frame rendering and GPU hand-over,
//! deferred post-frame chores, occlusion look, covered redraws, swap-chain
//! waitable slots and DirectComposition video layers.

pub(in crate::window) mod covered_draw;
pub(in crate::window) mod cpu_frame;
pub(in crate::window) mod handover;
pub(in crate::window) mod look;
pub(crate) mod poll_pace;
pub(in crate::window) mod post_frame;
pub(in crate::window) mod present_time;
pub(in crate::window) mod resize_sync;
pub(crate) mod slot;
pub(in crate::window) mod video_layer;
