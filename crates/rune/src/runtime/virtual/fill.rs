//! An index-built list's fill: how many frames in a row the host kept drawing because a list
//! still wanted rows. A frame that finds no list asking settles it; the bound stops a list whose
//! heights keep changing from asking forever.

use crate::machine::Effects;

/// The most frames in a row an index-built list keeps the host drawing to fill its overscan.
/// Filling takes four at most (half a viewport per side each); this bounds a list whose
/// heights keep changing.
pub(super) const FILL_FRAMES: u16 = 12;

/// Whether an index-built list still fills its overscan, and for how long it has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Fill {
    /// No list wants another frame.
    #[default]
    Idle,
    /// This many frames in a row found a list wanting one. Past [`FILL_FRAMES`] the overscan
    /// stops filling and the host has nothing to draw.
    Filling { frames: u16 },
}

/// What a finished frame found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FillEvent {
    /// A frame ended; `wanted` is whether an index-built list still wants its window moved.
    Frame { wanted: bool },
}

/// What to do about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FillEffect {
    /// Draw another frame now: a commit of that frame mounts more of the overscan.
    Draw,
}

impl Fill {
    pub(crate) fn step(&mut self, event: FillEvent) -> Effects<FillEffect> {
        match (*self, event) {
            (Fill::Idle, FillEvent::Frame { wanted: true }) => {
                *self = Fill::Filling { frames: 1 };
                Effects::one(FillEffect::Draw)
            }
            (Fill::Filling { frames }, FillEvent::Frame { wanted: true }) => {
                // Dev counted past the bound in a `u32` and never read it again; only `<= 12`
                // decides a draw, so the state stops counting where the answer does.
                let frames = frames.saturating_add(1).min(FILL_FRAMES + 1);
                *self = Fill::Filling { frames };
                if frames <= FILL_FRAMES { Effects::one(FillEffect::Draw) } else { Effects::none() }
            }
            (_, FillEvent::Frame { wanted: false }) => {
                *self = Fill::Idle;
                Effects::none()
            }
        }
    }

    /// Frames filling in a row; zero when idle.
    #[cfg(test)]
    pub(super) fn frames(self) -> u16 {
        match self {
            Fill::Idle => 0,
            Fill::Filling { frames } => frames,
        }
    }
}

#[cfg(test)]
#[path = "fill_tests.rs"]
mod tests;
