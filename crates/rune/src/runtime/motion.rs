//! Render-side motion tracks, growth rows/runs, flights, reveals, loop/timer clocks, live latches,
//! idle metering and rewind recording.
use super::*;

pub(super) mod content_slide;
pub(super) mod flights;
pub(super) mod fold_hold;
pub(super) mod growth_rows;
pub(super) mod growth_runs;
pub(super) mod idle_meter;
pub(super) mod landing_hold;
pub(super) mod live_check;
pub(super) mod live_latch;
pub(super) mod loop_clock;
pub(super) mod motion_elements;
pub(super) mod props;
pub(super) mod reveal_phase;
pub(super) mod rewind;
pub(super) mod timer_wake;
pub(super) mod tracks;
pub(super) mod unseen_presence;
