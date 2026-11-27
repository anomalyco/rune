//! Finite presentation poses, sampled by the shared LoopTrack clock.
use crate::Easing;

/// A full pose at elapsed seconds. Easing belongs to the segment arriving here.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Beat {
    pub at: f64,
    pub opacity: f32,
    pub scale: f32,
    /// Offset in points from where layout put the element, like CSS `translate`.
    pub translate: [f32; 2],
    /// Clockwise radians about the element's origin, like CSS `rotate`.
    pub rotate: f32,
    pub easing: Easing,
}

impl Beat {
    pub const fn new(at: f64) -> Self {
        Self { at, opacity: 1.0, scale: 1.0, translate: [0.0; 2], rotate: 0.0, easing: Easing::EASE_IN_OUT }
    }
    pub const fn opacity(mut self, value: f32) -> Self { self.opacity = value; self }
    pub const fn scale(mut self, value: f32) -> Self { self.scale = value; self }
    /// Move the element by `(x, y)` points, like CSS `translate: x y`, on top of its
    /// `El::offset`. The element and its subtree draw and hit-test there; layout keeps its place,
    /// so siblings don't move. An `El::live_offset` owns the offset, so beats don't move it.
    ///
    /// While a sequence plays (a repeating one from its start until the element drops it), and
    /// while a holding one-shot waits out its `delay` on its first beat, the offset follows the
    /// beats and an `El::offset` change lands at once. Once a one-shot has finished, or before a
    /// repeating or [`Sequence::release`] one starts, an `El::offset` change springs as usual
    /// (toward [`Sequence::rest`]). Dropping the sequence springs the element back to its
    /// `El::offset`.
    pub const fn translate(mut self, x: f32, y: f32) -> Self { self.translate = [x, y]; self }
    /// Turn the element `radians` clockwise about its origin (`El::origin`, the centre by default),
    /// like CSS `rotate`, on top of its `El::rotate`. An `El::live_rotation` owns the rotation,
    /// so beats don't turn it. `El::rotate` changes spring (on `El::rotate_motion`) or land at
    /// once just as [`Self::translate`] describes for `El::offset`.
    pub const fn rotate(mut self, radians: f32) -> Self { self.rotate = radians; self }
    pub const fn easing(mut self, easing: Easing) -> Self { self.easing = easing; self }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SequenceMode {
    /// One-shot that holds its first beat during `delay` and its last beat after ending.
    Hold,
    /// One-shot that leaves the element at its own pose when not playing (CSS `animation-fill-mode: none`).
    Release,
    /// Loops continuously until removed (CSS `animation-iteration-count: infinite`).
    Repeating,
}

/// An opacity/scale/translate/rotate keyframe sequence: one-shot holding its final pose by
/// default, releasing back to the element's own pose with [`Sequence::release`], or looping
/// continuously with [`Sequence::repeating`]. Static slices keep specs Copy, like Cycle; use a
/// new element key to replay a finite sequence.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sequence {
    frames: &'static [Beat],
    pub(crate) delay: f64,
    pub(crate) mode: SequenceMode,
}

impl Sequence {
    pub fn new(frames: &'static [Beat]) -> Self {
        assert!(frames.len() >= 2 && frames[0].at == 0.0, "keyframes need at least two poses, starting at zero");
        assert!(frames.windows(2).all(|p| p[1].at.is_finite() && p[1].at > p[0].at), "keyframe times must increase");
        assert!(frames.iter().all(|p| p.opacity.is_finite() && p.scale.is_finite() && p.scale >= 0.0
            && p.translate.iter().all(|v| v.is_finite()) && p.rotate.is_finite()), "invalid keyframe pose");
        Self { frames, delay: 0.0, mode: SequenceMode::Hold }
    }
    pub fn delay(mut self, seconds: f64) -> Self {
        assert!(seconds.is_finite() && seconds >= 0.0);
        self.delay = seconds; self
    }
    /// Return the element to its own opacity, scale, offset and rotation when this one-shot isn't
    /// playing (before a [`delay`](Self::delay), after the last beat, under Reduce Motion, or on
    /// settle), instead of holding its beats (like CSS `animation-fill-mode: none`).
    ///
    /// ```
    /// use rune_motion::{Beat, Easing, LoopSpec, LoopTrack, Sequence};
    ///
    /// const TURN: &[Beat] = &[
    ///     Beat::new(0.0).rotate(0.0),
    ///     Beat::new(0.2).rotate(core::f32::consts::PI).easing(Easing::LINEAR),
    /// ];
    /// let track = LoopTrack::new(LoopSpec::sequence(Sequence::new(TURN).release()), 0.0);
    /// assert!((track.pose(0.1, false).unwrap().rotate - core::f32::consts::FRAC_PI_2).abs() < 1e-5);
    /// assert!(!track.active(0.2));
    /// assert_eq!(track.pose(0.2, false).unwrap().rotate, 0.0);
    /// ```
    pub const fn release(mut self) -> Self {
        self.mode = SequenceMode::Release;
        self
    }
    /// Loop the sequence continuously instead of holding its final pose (like CSS
    /// `animation-iteration-count: infinite`).
    ///
    /// Under Reduce Motion a repeating sequence holds its calm first pose (like other periodic
    /// loops), and removing it cools smoothly toward rest via the track's amplitude envelope.
    /// Its translate and rotate don't cool: the element springs back to its own offset and
    /// rotation instead.
    ///
    /// ```
    /// use rune_motion::{Beat, Easing, LoopSpec, Sequence};
    ///
    /// const PULSE: &[Beat] = &[
    ///     Beat::new(0.0).opacity(0.4).scale(0.85),
    ///     Beat::new(0.4).opacity(1.0).scale(1.0).easing(Easing::EASE_IN_OUT),
    ///     Beat::new(0.8).opacity(0.4).scale(0.85).easing(Easing::EASE_IN_OUT),
    /// ];
    /// let spec = LoopSpec::sequence(Sequence::new(PULSE).repeating());
    /// # let _ = spec;
    /// ```
    pub const fn repeating(mut self) -> Self {
        self.mode = SequenceMode::Repeating;
        self
    }
    pub(crate) const fn is_repeating(self) -> bool {
        matches!(self.mode, SequenceMode::Repeating)
    }
    pub fn duration(self) -> f64 { self.frames.last().unwrap().at }
    /// Whether any beat moves the element (a translate or a rotate), so the runtime drives
    /// its offset and rotation from this sequence.
    pub fn moves(self) -> bool { self.frames.iter().any(|b| b.translate != [0.0; 2] || b.rotate != 0.0) }
    /// The pose the sequence rests at when it isn't playing: a holding one-shot's last beat, a
    /// [`Self::release`] one-shot's neutral pose (leaving the element's own opacity, scale, offset
    /// and rotation intact), or a repeating one's first beat (where Reduce Motion holds it).
    pub fn rest(self) -> Beat {
        match self.mode {
            SequenceMode::Hold => self.frames[self.frames.len() - 1],
            SequenceMode::Release => Beat::new(self.duration()),
            SequenceMode::Repeating => self.frames[0],
        }
    }
    pub(crate) fn sample(self, elapsed: f64) -> Beat {
        if elapsed <= 0.0 { return self.frames[0]; }
        let Some(pair) = self.frames.windows(2).find(|p| elapsed < p[1].at) else { return *self.frames.last().unwrap() };
        let [a, b] = [pair[0], pair[1]];
        let t = b.easing.eval(((elapsed - a.at) / (b.at - a.at)) as f32);
        let lerp = |x: f32, y: f32| x + (y - x) * t;
        Beat { at: elapsed, opacity: lerp(a.opacity, b.opacity), scale: lerp(a.scale, b.scale).max(0.0),
            translate: [lerp(a.translate[0], b.translate[0]), lerp(a.translate[1], b.translate[1])],
            rotate: lerp(a.rotate, b.rotate), easing: b.easing }
    }
}
