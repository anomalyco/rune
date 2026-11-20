//! The runtime's side of `perf_warn`'s idle meters: what woke a frame, and what it drew.
use super::*;

impl timer_wake::FrameState {
    /// Before a frame's poll. `timer`: a due `wake_after` woke it, or a resting wake's poll
    /// stands for it; `live`: a live value did.
    #[inline(always)]
    #[cfg_attr(not(debug_assertions), allow(unused_variables))]
    pub(in crate::runtime) fn idle_frame_begins(&mut self, animating: bool, timer: bool, live: bool) {
        #[cfg(debug_assertions)]
        self.idle_meter.begins(self.queue.is_empty() && !animating, timer, live);
    }

    /// After a frame built its instances.
    #[cfg(debug_assertions)]
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub(in crate::runtime) fn idle_frame_ends(
        &mut self,
        clock: f64,
        animating: bool,
        clear: [f32; 4],
        viewport: (f32, f32),
        scale: f32,
        srgb_blend: bool,
        transparent: bool,
        image_copies: u64,
        instances: &[Instance],
        opaque: bool,
        title: Option<&str>,
    ) {
        use std::hash::{Hash, Hasher};
        let mut key = std::collections::hash_map::DefaultHasher::new();
        (clear.map(f32::to_bits), viewport.0.to_bits(), viewport.1.to_bits(), scale.to_bits()).hash(&mut key);
        (srgb_blend, transparent, image_copies).hash(&mut key);
        let drawn = crate::tooling::perf_warn::Drawn {
            instances: bytemuck::cast_slice(instances),
            key: key.finish(),
            opaque,
        };
        self.idle_meter.ends(clock, animating, drawn, || title.map(str::to_owned));
    }
}
