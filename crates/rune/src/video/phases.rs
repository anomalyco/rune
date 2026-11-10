//! Two small phases of a native player, apart from AVFoundation so they're plain data: where
//! its last seek stands, and whether a poster frame may land on the clip's surface.

use crate::time::Instant;
use std::time::Duration;

/// A seek that never lands stops requesting redraws (or ticks) after this long.
const SEEK_PATIENCE: Duration = Duration::from_secs(2);
/// An audio seek has landed once the player reports a time this close to the target.
const AUDIO_SEEK_SLOP: f64 = 0.25;

/// Where the player's last seek stands.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum Settle {
    #[default]
    Still,
    /// Waiting for the frame at `to` (for audio, the position), since `since`.
    Seeking { since: Instant, to: f64 },
    /// A paused seek within the frame on screen: nothing to wait for, but the player may report
    /// the old position until it lands, so the handle's time holds.
    Holding { to: f64 },
}

impl Settle {
    /// A seek to `to`, `same_frame` when it's paused within the frame on screen. It replaces
    /// any earlier seek, as the player cancels that one.
    pub(super) fn seek(&mut self, to: f64, same_frame: bool, now: Instant) {
        *self = if same_frame { Settle::Holding { to } } else { Settle::Seeking { since: now, to } };
    }

    /// A new frame reached the surface.
    pub(super) fn framed(&mut self) {
        if let Settle::Seeking { .. } = self {
            *self = Settle::Still;
        }
    }

    /// The player reports `current`. Audio has no frames, so its seek lands at the position;
    /// a hold ends once the player catches up, or when playback moves on from it.
    pub(super) fn observe(&mut self, current: f64, playing: bool, audio: bool) {
        match *self {
            Settle::Seeking { to, .. } if audio && (current - to).abs() < AUDIO_SEEK_SLOP => *self = Settle::Still,
            Settle::Holding { to } if playing || (current - to).abs() < 1e-3 => *self = Settle::Still,
            _ => {}
        }
    }

    /// A seek that hasn't landed by `now` stops being waited for.
    pub(super) fn expire(&mut self, now: Instant) {
        if let Settle::Seeking { since, .. } = *self
            && now.saturating_duration_since(since) > SEEK_PATIENCE
        {
            *self = Settle::Still;
        }
    }

    /// The player may still report the position from before the seek.
    pub(super) fn pending(self) -> bool {
        self != Settle::Still
    }

    /// A frame (or position) is still to come.
    pub(super) fn waiting(self) -> bool {
        matches!(self, Settle::Seeking { .. })
    }
}

/// Whether a poster frame may land on the clip's surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Poster {
    /// None asked for since the clip was made, or since a player that never showed a frame
    /// was released.
    #[default]
    Unasked,
    /// Asked for while no player runs: a finished one replaces the surface's frame.
    Due,
    /// A player owns the surface, or the clip is audio: a finished load brings only the
    /// clip's facts.
    Withheld,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_seek_replaces_an_earlier_one() {
        let now = Instant::now();
        let mut s = Settle::default();
        s.seek(3.0, false, now);
        assert!(s.waiting());
        // Paused, back within the frame on screen before the far seek landed: that frame is
        // already showing, so there's nothing left to wait for.
        s.seek(0.5, true, now);
        assert!(!s.waiting() && s.pending());
        s.observe(0.5, false, false);
        assert_eq!(s, Settle::Still);
    }

    #[test]
    fn a_seek_lands_with_a_frame_or_for_audio_at_its_position() {
        let now = Instant::now();
        let mut video = Settle::default();
        video.seek(2.0, false, now);
        video.observe(2.0, false, false);
        assert!(video.waiting(), "video waits for the frame, not the position");
        video.framed();
        assert_eq!(video, Settle::Still);

        let mut audio = Settle::default();
        audio.seek(2.0, false, now);
        audio.observe(1.0, true, true);
        assert!(audio.waiting());
        audio.observe(2.1, true, true);
        assert_eq!(audio, Settle::Still);
    }

    #[test]
    fn a_hold_ends_when_playback_moves_on_and_a_frame_does_not_end_it() {
        let now = Instant::now();
        let mut s = Settle::default();
        s.seek(0.5, true, now);
        s.framed();
        assert_eq!(s, Settle::Holding { to: 0.5 });
        s.observe(0.4, true, false);
        assert_eq!(s, Settle::Still);
    }

    #[test]
    fn a_seek_that_never_lands_expires() {
        let now = Instant::now();
        let mut s = Settle::default();
        s.seek(2.0, false, now);
        s.expire(now + SEEK_PATIENCE);
        assert!(s.waiting());
        s.expire(now + SEEK_PATIENCE + Duration::from_millis(1));
        assert_eq!(s, Settle::Still);
        // A hold has nothing to wait for, so nothing expires it.
        s.seek(1.0, true, now);
        s.expire(now + Duration::from_secs(60));
        assert_eq!(s, Settle::Holding { to: 1.0 });
    }
}
