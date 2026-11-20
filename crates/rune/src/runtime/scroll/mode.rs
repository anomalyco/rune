//! What moves a scroll viewport's vertical offset, and the hand latches around it, one frame
//! at a time. One `ScrollMode` carries the motion that is valid with the frame's hand data, and
//! one [`step`] folds a frame's facts into it, so a hand move, the step's derived `scrolling`,
//! the watcher's `by_hand`, a growth stack's `shrink_cut` and its layout end can't sit in
//! combinations that mean nothing.
use super::*;
use crate::machine::Effects;
use super::scroll_wheel::WheelCurve;

/// What moves a viewport's vertical offset, one motion at a time, so it can't follow its
/// bottom while it glides, or coast while a glide carries it, with the hand latches that belong
/// to the frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::runtime) enum ScrollMode {
    /// At rest where a hand, an app or a glide left it.
    Rest(Hand),
    /// A stick-to-bottom viewport following its content's end.
    Pinned(Hand),
    /// Coasting from a fling, keys or wheel momentum at `v` points a second, decaying.
    Momentum { v: f32, hand: Hand },
    /// Gliding on a spring (`ScrollState::glide_spring`) at `v` points a second.
    Glide { v: f32, to: GlideTo, hand: Hand },
    /// Easing to where a notched mouse wheel sent it (`runtime/scroll/wheel.rs`).
    Wheel { curve: WheelCurve, hand: Hand },
}

/// The hand latches a mode carries from frame to frame, whatever moves the offset.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(in crate::runtime) struct Hand {
    /// A hand (the wheel, keys, a drag, a touch) or an app's instant jump moved the offset
    /// since the last step. The step clears it.
    pub(in crate::runtime) direct: bool,
    /// It moved in the last step: a hand, its momentum or a glide (derived there).
    pub(in crate::runtime) scrolling: bool,
    /// A hand or an app's instant jump moved it since `El::on_scroll_offset` was last told,
    /// across the frame's steps; taken once at the frame's end.
    pub(in crate::runtime) by_hand: bool,
    /// A hand scrolled while the growth stack moved: no shrink glide until it settles.
    pub(in crate::runtime) shrink_cut: bool,
    /// The growth stack's layout end while its rows still move (see `ease_to_shrunk_end`).
    pub(in crate::runtime) laid: Option<f64>,
}

impl Default for ScrollMode {
    fn default() -> Self { ScrollMode::Rest(Hand::default()) }
}

/// Test builds count the mode writes each transition makes, so a landing that changes nothing
/// can be told from one that writes the same mode over itself.
#[cfg(test)]
mod writes {
    use std::cell::Cell;

    thread_local! { static COUNT: Cell<usize> = const { Cell::new(0) }; }

    pub(super) fn note() { COUNT.with(|c| c.set(c.get() + 1)); }

    /// The writes since the last call.
    pub(super) fn take() -> usize { COUNT.with(|c| c.replace(0)) }
}

#[cfg(test)]
use writes::note as note_write;

impl ScrollMode {
    /// At rest, with no hand data: a viewport nobody has moved.
    pub(in crate::runtime) fn rest() -> Self { ScrollMode::Rest(Hand::default()) }

    /// Following its content's end, with no hand data: a stick-to-bottom viewport that opens at
    /// the bottom.
    pub(in crate::runtime) fn following() -> Self { ScrollMode::Pinned(Hand::default()) }

    /// The hand latches this mode carries.
    pub(in crate::runtime) fn hand(self) -> Hand {
        match self {
            ScrollMode::Rest(hand) | ScrollMode::Pinned(hand) => hand,
            ScrollMode::Momentum { hand, .. } | ScrollMode::Glide { hand, .. } | ScrollMode::Wheel { hand, .. } => hand,
        }
    }

    fn hand_mut(&mut self) -> &mut Hand {
        match self {
            ScrollMode::Rest(hand) | ScrollMode::Pinned(hand) => hand,
            ScrollMode::Momentum { hand, .. } | ScrollMode::Glide { hand, .. } | ScrollMode::Wheel { hand, .. } => hand,
        }
    }

    /// The same mode at rest, keeping the hand's latches.
    pub(in crate::runtime) fn to_rest(self) -> Self {
        #[cfg(test)]
        note_write();
        ScrollMode::Rest(self.hand())
    }

    /// The same mode following its bottom, keeping the hand's latches.
    pub(in crate::runtime) fn to_pinned(self) -> Self {
        #[cfg(test)]
        note_write();
        ScrollMode::Pinned(self.hand())
    }

    /// The same mode coasting at `v`, keeping the hand's latches.
    pub(in crate::runtime) fn to_momentum(self, v: f32) -> Self {
        #[cfg(test)]
        note_write();
        ScrollMode::Momentum { v, hand: self.hand() }
    }

    /// The same mode gliding at `v` toward `to`, keeping the hand's latches.
    pub(in crate::runtime) fn to_glide(self, v: f32, to: GlideTo) -> Self {
        #[cfg(test)]
        note_write();
        ScrollMode::Glide { v, to, hand: self.hand() }
    }

    /// The same mode easing on a wheel's `curve`, keeping the hand's latches.
    pub(in crate::runtime) fn to_wheel(self, curve: WheelCurve) -> Self {
        ScrollMode::Wheel { curve, hand: self.hand() }
    }

    /// The wheel's ease, while one runs.
    pub(in crate::runtime) fn wheel(self) -> Option<WheelCurve> {
        match self {
            ScrollMode::Wheel { curve, .. } => Some(curve),
            _ => None,
        }
    }

    /// Its momentum (points a second), 0 unless coasting.
    pub(in crate::runtime) fn coast(self) -> f32 {
        match self {
            ScrollMode::Momentum { v, .. } => v,
            _ => 0.0,
        }
    }

    /// It follows its content's bottom.
    pub(in crate::runtime) fn pinned(self) -> bool { matches!(self, ScrollMode::Pinned(_)) }

    /// The running glide's speed (points a second), while one runs.
    pub(in crate::runtime) fn glide(self) -> Option<f32> {
        match self {
            ScrollMode::Glide { v, .. } => Some(v),
            _ => None,
        }
    }
}

/// What a frame did to a viewport.
pub(in crate::runtime) enum Event {
    /// A hand or an app put the offset where it is.
    Moved,
    /// A touch holds the viewport (`El::scroll` under a finger).
    Hold,
    /// A touch's coast moved it (`moving`) or came to rest.
    Coast { moving: bool },
    /// The viewport was concealed: nothing moved it, whatever a hand had done before.
    Still,
    /// A scroll step began: the growth stack is `moving`, so a hand's scroll cuts the shrink
    /// glide until it settles.
    Growth { moving: bool },
    /// The step's motion is over: derive `scrolling` from `direct` and the coast speeds, grow
    /// the watcher's `by_hand`, and clear `direct`.
    Step { x_v: f32 },
    /// The growth stack's layout end this frame.
    Laid(Option<f64>),
    /// `El::on_scroll_offset` asks for a hand move to report.
    Take,
    /// The motion stops, unless the viewport follows its bottom.
    Halt,
    /// The motion comes to rest where it is.
    Rest,
    /// The viewport follows its content's end from here.
    Pin,
    /// The viewport stops following its content's end, where it stands.
    Unpin,
}

#[derive(Debug, PartialEq)]
pub(in crate::runtime) enum Effect {
    /// `El::on_scroll_offset`'s handler hears of a hand move at the frame's end.
    Tell,
}

/// Folds `event` into `mode`, returning what the caller should carry out.
pub(in crate::runtime) fn step(mode: &mut ScrollMode, event: Event) -> Effects<Effect> {
    match event {
        Event::Moved => mode.hand_mut().direct = true,
        Event::Hold => {
            let hand = mode.hand_mut();
            hand.scrolling = true;
            hand.by_hand = true;
        }
        Event::Coast { moving } => {
            let hand = mode.hand_mut();
            hand.scrolling = moving;
            hand.by_hand |= moving;
            hand.direct = false;
        }
        Event::Still => mode.hand_mut().scrolling = false,
        Event::Growth { moving } => {
            let coast = mode.coast();
            let hand = mode.hand_mut();
            hand.shrink_cut = moving && (hand.shrink_cut || hand.direct || coast.abs() > 1.0);
        }
        Event::Step { x_v } => {
            let coast = mode.coast();
            let hand = mode.hand_mut();
            hand.scrolling = hand.direct || coast.abs() > 1.0 || x_v.abs() > 1.0;
            hand.by_hand |= hand.scrolling;
            hand.direct = false;
        }
        Event::Laid(laid) => mode.hand_mut().laid = laid,
        Event::Take => {
            return if std::mem::take(&mut mode.hand_mut().by_hand) {
                Effects::one(Effect::Tell)
            } else {
                Effects::none()
            };
        }
        Event::Halt => {
            if !mode.pinned() {
                *mode = mode.to_rest();
            }
        }
        Event::Pin => {
            if !mode.pinned() {
                *mode = mode.to_pinned();
            }
        }
        Event::Unpin => {
            if mode.pinned() {
                *mode = mode.to_rest();
            }
        }
        Event::Rest => *mode = mode.to_rest(),
    }
    Effects::none()
}

#[cfg(test)]
#[path = "mode_tests.rs"]
mod tests;
