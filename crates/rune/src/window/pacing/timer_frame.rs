use crate::time::Instant;
use std::time::Duration;

/// Motion's frame interval when no display link paces a surface.
pub(crate) const TIMER_FRAME: Duration = Duration::from_micros(16_667);

/// A Wayland timer redraw may wait indefinitely for the compositor's frame callback.
/// Give it one frame to arrive, then keep timers moving without requesting more redraws.
#[derive(Default)]
pub(crate) enum TimerFrame {
    #[default]
    Idle,
    Waiting { fallback: Option<Instant> },
}

impl TimerFrame {
    pub(crate) fn requested(&mut self, now: Instant) {
        if matches!(self, Self::Idle) { *self = Self::Waiting { fallback: Some(now + TIMER_FRAME) }; }
    }

    pub(crate) fn deadline(&self, runtime: Option<Instant>) -> Option<Instant> {
        match self {
            Self::Waiting { fallback: Some(at) } => Some(*at),
            _ => runtime,
        }
    }

    pub(crate) fn poll(&mut self) -> bool {
        let Self::Waiting { fallback } = self else { return false };
        *fallback = None;
        true
    }
}
