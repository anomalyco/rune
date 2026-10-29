//! An overlay frame's steps apart from its window: the animation step, the compositor or
//! default draw, and whether anything asks for another frame.

use crate::compose::{Compositor, Frame};
use crate::frame_clock::FrameClock;
use crate::runtime::Runtime;
use crate::time::Instant;

/// Whether nothing asks for the frame after one just presented. `next_tick` is the running
/// display link's next tick, if one paces the overlay: a timer due by then counts as asking,
/// so a poll stepping its own animation with `Tx::wake_after(Duration::ZERO)` gets every frame
/// at the display's rate instead of the link stopping after each frame. Unpaced, the timer
/// waits a frame in `wake_deadline` and never keeps the loop turning.
pub(super) fn idle_after<T: 'static>(rt: &Runtime<T>, animating: bool, next_tick: Option<Instant>) -> bool {
    !(animating || rt.has_pending_input() || crate::window::timer_wants_frame(rt, next_tick))
}

/// `draw_frame`'s animation step, apart from the surface so tests can drive it.
pub(super) fn step<T: 'static>(rt: &mut Runtime<T>, clock: &mut FrameClock, compositor: &Option<Box<dyn Compositor<T>>>, frame_time: Instant, prewarm: bool) -> (bool, f64) {
    if prewarm {
        clock.hold(frame_time);
        return (rt.frame(0.0), 0.0);
    }
    clock.advance(rt, frame_time, compositor.as_ref().is_some_and(|c| c.animating()))
}

/// Both native surface frames and offscreen prewarming use the same compositor path.
pub(super) fn render_frame<T: 'static>(f: &mut Frame<'_, T>, compositor: &mut Option<Box<dyn Compositor<T>>>) -> bool {
    f.ui.upload_images(f.device, f.queue, &mut f.rt.images);
    f.ui.prepare_surfaces(f.device, f.queue, &f.rt.surfaces, f.size);
    if let Some(c) = compositor {
        c.render(f);
        c.animating()
    } else {
        f.ui.prepare(f.device, f.queue, &mut f.rt.text.atlas, &f.rt.instances, f.size);
        f.ui.draw(f.encoder, f.target, [0.0; 4]);
        false
    }
}
