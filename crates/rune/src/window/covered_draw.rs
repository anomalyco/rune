//! A covered window's last frame, kept current. Windows' compositor shows a window the moment
//! nothing covers it any more, with the last frame it presented, before the occlusion tracker
//! tells the window (`platform::win32::occlusion`, 60-95 ms on blade): a change made while it
//! was covered showed stale until then (a reply's footer arriving behind another window). So a
//! covered window that was asked for a frame still draws one, settled, once the changes go quiet
//! (`QUIET`) and no later than `LONGEST` after the first: motion that runs on while it's covered
//! draws at most once a second.

use std::time::Duration;

use crate::time::Instant;

/// How long a covered window's changes must stop before it draws them.
const QUIET: Duration = Duration::from_millis(250);
/// The longest a covered window's change waits to be drawn.
const LONGEST: Duration = Duration::from_millis(1000);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum CoveredDraw {
    /// The last frame presented shows the window as it is.
    #[default]
    Current,
    /// A frame was asked for while covered: drawn at `at`.
    Owed { first: Instant, at: Instant },
    /// Its time came: the next frame draws, covered or not.
    Drawing,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum CoveredDrawEvent {
    /// A frame was asked for while the window was covered.
    Asked(Instant),
    /// The owed frame's time came.
    Due,
    /// The owed frame is drawing, or the window was uncovered (its first frame then draws).
    Drawn,
}

impl CoveredDraw {
    pub(super) fn step(self, event: CoveredDrawEvent) -> CoveredDraw {
        match (self, event) {
            (CoveredDraw::Current, CoveredDrawEvent::Asked(now)) => CoveredDraw::Owed { first: now, at: now + QUIET },
            (CoveredDraw::Owed { first, .. }, CoveredDrawEvent::Asked(now)) => CoveredDraw::Owed { first, at: (now + QUIET).min(first + LONGEST) },
            (CoveredDraw::Owed { .. }, CoveredDrawEvent::Due) => CoveredDraw::Drawing,
            (CoveredDraw::Drawing, CoveredDrawEvent::Asked(_)) | (CoveredDraw::Current | CoveredDraw::Drawing, CoveredDrawEvent::Due) => self,
            (_, CoveredDrawEvent::Drawn) => CoveredDraw::Current,
        }
    }

    /// When the owed frame draws.
    pub(super) fn deadline(self) -> Option<Instant> {
        match self {
            CoveredDraw::Current | CoveredDraw::Drawing => None,
            CoveredDraw::Owed { at, .. } => Some(at),
        }
    }

    pub(super) fn due(self, now: Instant) -> bool {
        self.deadline().is_some_and(|at| at <= now)
    }

    pub(super) fn drawing(self) -> bool {
        self == CoveredDraw::Drawing
    }
}
