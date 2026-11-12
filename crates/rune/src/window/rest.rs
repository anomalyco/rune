//! A window's memory at rest. Between frames the renderer keeps what the next one may reuse:
//! spare layer targets (`render::layers`, up to 32 MiB) and `El::cache` textures no layer drew
//! last frame (up to the cache budget), which the screen on show doesn't use; and the scratch
//! targets of its layers, its backdrop copies and blur pyramid, and returned staging chunks (up to
//! 4 MiB), which each of its frames uses again.
//!
//! A window that has drawn nothing for [`REST_AFTER`] (or [`PRESSED_AFTER`] while the system says
//! memory is low) gives back the first kind, keeping the most recently used 8 MiB of spare targets
//! for the menus and hovers of the screen on show: making a target costs the frame that takes it a
//! few ms on an integrated GPU.
//!
//! A window that can't be seen (hidden, minimised, covered whole) gives back both: the frame that
//! shows it again makes its targets afresh.

use super::*;
use crate::machine::Effects;

/// How long a window draws nothing before it gives back what its screen doesn't use.
pub(super) const REST_AFTER: Duration = Duration::from_secs(10);
/// The same while the system says memory is low.
pub(super) const PRESSED_AFTER: Duration = Duration::from_secs(1);

/// What the renderer may hold that a rest gives back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Rest {
    /// Nothing: nothing drawn since the window opened, or since it went out of sight.
    #[default]
    Rested,
    /// Drew; gives back what the screen doesn't use at `due`, unless it draws again first.
    Drawing { due: Instant },
    /// Gave that back; holds only what the screen on show draws into.
    Idle,
}

/// What happened to the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RestEvent {
    /// A frame reached the screen at `now`; `low` is the system saying memory is low.
    Drew { now: Instant, low: bool },
    /// Hidden, minimised, or covered whole.
    Unseen,
    /// The loop woke at `now`.
    Woke { now: Instant },
}

/// What a transition asks for (`Host::give_back`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RestEffect {
    /// What the screen on show doesn't use.
    GiveBackUnused,
    /// Everything the renderer keeps between frames.
    GiveBackAll,
}

impl Rest {
    /// When the loop must wake to give back.
    pub(super) fn deadline(self) -> Option<Instant> {
        match self {
            Rest::Drawing { due } => Some(due),
            Rest::Rested | Rest::Idle => None,
        }
    }
}

/// The one transition. A drawn frame sets the deadline afresh; the deadline passing gives back
/// what the screen doesn't use, once; going out of sight gives back everything, once.
pub(super) fn step(state: &mut Rest, event: RestEvent) -> Effects<RestEffect> {
    use RestEvent as E;
    match (*state, event) {
        (_, E::Drew { now, low }) => {
            *state = Rest::Drawing { due: now + if low { PRESSED_AFTER } else { REST_AFTER } };
            Effects::none()
        }
        (Rest::Drawing { due }, E::Woke { now }) if now >= due => { *state = Rest::Idle; Effects::one(RestEffect::GiveBackUnused) }
        (Rest::Drawing { .. } | Rest::Idle, E::Unseen) => { *state = Rest::Rested; Effects::one(RestEffect::GiveBackAll) }
        (Rest::Drawing { .. } | Rest::Idle | Rest::Rested, E::Woke { .. }) | (Rest::Rested, E::Unseen) => Effects::none(),
    }
}
impl<S: 'static> Host<S> {
    pub(super) fn rest_step(&mut self, event: RestEvent) {
        for effect in step(&mut self.rest, event) {
            match effect {
                RestEffect::GiveBackUnused => self.give_back(false),
                RestEffect::GiveBackAll => self.give_back(true),
            }
        }
    }

    fn give_back(&mut self, all: bool) {
        if let Some((gpu, renderer)) = &mut self.gpu {
            renderer.give_back(all, &mut self.rt.text.atlas.resident_caches);
            // Out of sight, what the last frame used goes too, and the staging chunks: an empty
            // submission and a wait land those frees now. At an idle rest the device isn't
            // polled: a poll costs the next two frames' submits 4-9 ms each on an Iris Xe.
            if all {
                gpu.queue.submit(std::iter::empty());
                let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
                renderer.trim_uploads();
            }
        }
    }
}

/// Whether the system says memory is low (Windows' low-memory resource notification).
#[cfg(target_os = "windows")]
pub(super) fn low_memory() -> bool {
    use windows_sys::Win32::System::Memory::{CreateMemoryResourceNotification, LowMemoryResourceNotification, QueryMemoryResourceNotification};
    static EVENT: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    // SAFETY: the notification handle is made once and never closed; the query writes one BOOL.
    let event = *EVENT.get_or_init(|| unsafe { CreateMemoryResourceNotification(LowMemoryResourceNotification) } as usize);
    let mut low = 0;
    // SAFETY: as above.
    event != 0 && unsafe { QueryMemoryResourceNotification(event as _, &mut low) } != 0 && low != 0
}

#[cfg(not(target_os = "windows"))]
pub(super) fn low_memory() -> bool { false }
