//! An overlay frame's steps apart from its window: the animation step, the compositor or
//! default draw, whether anything asks for another frame, and the submit and present.

use super::{Backing, Live};
use crate::compose::{Compositor, Frame};
use crate::frame_clock::FrameClock;
use crate::render::{Gpu, Renderer};
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

/// Submits the frame's commands and presents its drawable. On macOS a plain frame (no material
/// views to move with the drawable, no transaction, no native video surface waiting on the
/// submission) is finished, submitted and presented on the overlay's present thread, so the
/// event loop's thread spends none of wgpu's encode, submit and present on it. Prewarm frames
/// present here, before the window is revealed.
pub(super) fn submit_and_present<T: 'static>(live: &mut Live, renderer: &Renderer, gpu: &Gpu, rt: &Runtime<T>,
    encoder: wgpu::CommandEncoder, frame: wgpu::SurfaceTexture, prewarm: bool) {
    #[allow(irrefutable_let_patterns)]
    if let Backing::Window(w) = &live.backing {
        w.pre_present_notify();
    }
    #[cfg(all(target_os = "macos", feature = "metal-spike"))]
    if crate::gpu_metal::native(&gpu.device) {
        renderer.submit(&gpu.queue, encoder);
        return gpu.queue.present(frame);
    }
    #[cfg(target_os = "macos")]
    {
        if !prewarm && live.materials.presents_plainly() && renderer.submits_plainly() {
            if live.present.is_none() { live.present = crate::present_worker::PresentWorker::spawn(); }
            if let Some(worker) = &mut live.present {
                let mut encoder = encoder;
                renderer.before_submit(&mut encoder);
                let queue = gpu.queue.clone();
                return worker.run(move || {
                    queue.submit([encoder.finish()]);
                    queue.present(frame);
                });
            }
        }
        renderer.submit(&gpu.queue, encoder);
        live.materials.present(&rt.materials, rt.env.scale, || gpu.queue.present(frame));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (rt, prewarm);
        renderer.submit(&gpu.queue, encoder);
        gpu.queue.present(frame);
    }
}

impl Live {
    /// Waits for the frame on the present thread, before the surface is touched again.
    pub(super) fn settle(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(worker) = &mut self.present { worker.wait(); }
    }
}
