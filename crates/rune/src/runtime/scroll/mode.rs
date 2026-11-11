//! What moves a scroll viewport's vertical offset, and the hand latches around it, one frame
//! at a time. One `ScrollMode` carries the motion that is valid with the frame's hand data, and
//! one [`step`] folds a frame's facts into it, so a hand move, the step's derived `scrolling`,
//! the watcher's `by_hand`, a growth stack's `shrink_cut` and its layout end can't sit in
//! combinations that mean nothing.
use super::*;
use crate::machine::Effects;

/// What moves a viewport's vertical offset, one motion at a time, so it can't follow its
/// bottom while it glides, or coast while a glide carries it, with the hand latches that belong
/// to the frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum ScrollMode {
    /// At rest where a hand, an app or a glide left it.
    Rest(Hand),
    /// A stick-to-bottom viewport following its content's end.
    Pinned(Hand),
    /// Coasting from a fling, keys or wheel momentum at `v` points a second, decaying.
    Momentum { v: f32, hand: Hand },
    /// Gliding on a spring (`ScrollState::glide_spring`) at `v` points a second.
    Glide { v: f32, to: GlideTo, hand: Hand },
}

/// The hand latches a mode carries from frame to frame, whatever moves the offset.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Hand {
    /// A hand (the wheel, keys, a drag, a touch) or an app's instant jump moved the offset
    /// since the last step. The step clears it.
    pub(super) direct: bool,
    /// It moved in the last step: a hand, its momentum or a glide (derived there).
    pub(super) scrolling: bool,
    /// A hand or an app's instant jump moved it since `El::on_scroll_offset` was last told,
    /// across the frame's steps; taken once at the frame's end.
    pub(super) by_hand: bool,
    /// A hand scrolled while the growth stack moved: no shrink glide until it settles.
    pub(super) shrink_cut: bool,
    /// The growth stack's layout end while its rows still move (see `ease_to_shrunk_end`).
    pub(super) laid: Option<f64>,
}

impl Default for ScrollMode {
    fn default() -> Self { ScrollMode::Rest(Hand::default()) }
}

impl ScrollMode {
    /// At rest, with no hand data: a viewport nobody has moved.
    pub(super) fn rest() -> Self { ScrollMode::Rest(Hand::default()) }

    /// Following its content's end, with no hand data: a stick-to-bottom viewport that opens at
    /// the bottom.
    pub(super) fn following() -> Self { ScrollMode::Pinned(Hand::default()) }

    /// The hand latches this mode carries.
    pub(super) fn hand(self) -> Hand {
        match self {
            ScrollMode::Rest(hand) | ScrollMode::Pinned(hand) => hand,
            ScrollMode::Momentum { hand, .. } | ScrollMode::Glide { hand, .. } => hand,
        }
    }

    fn hand_mut(&mut self) -> &mut Hand {
        match self {
            ScrollMode::Rest(hand) | ScrollMode::Pinned(hand) => hand,
            ScrollMode::Momentum { hand, .. } | ScrollMode::Glide { hand, .. } => hand,
        }
    }

    /// The same mode at rest, keeping the hand's latches.
    pub(super) fn to_rest(self) -> Self { ScrollMode::Rest(self.hand()) }

    /// The same mode following its bottom, keeping the hand's latches.
    pub(super) fn to_pinned(self) -> Self { ScrollMode::Pinned(self.hand()) }

    /// The same mode coasting at `v`, keeping the hand's latches.
    pub(super) fn to_momentum(self, v: f32) -> Self { ScrollMode::Momentum { v, hand: self.hand() } }

    /// The same mode gliding at `v` toward `to`, keeping the hand's latches.
    pub(super) fn to_glide(self, v: f32, to: GlideTo) -> Self { ScrollMode::Glide { v, to, hand: self.hand() } }

    /// Its momentum (points a second), 0 unless coasting.
    pub(super) fn coast(self) -> f32 {
        match self {
            ScrollMode::Momentum { v, .. } => v,
            _ => 0.0,
        }
    }

    /// It follows its content's bottom.
    pub(super) fn pinned(self) -> bool { matches!(self, ScrollMode::Pinned(_)) }

    /// The running glide's speed (points a second), while one runs.
    pub(super) fn glide(self) -> Option<f32> {
        match self {
            ScrollMode::Glide { v, .. } => Some(v),
            _ => None,
        }
    }
}

/// What a frame did to a viewport.
pub(super) enum Event {
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
    /// The viewport follows its content's end from here.
    Pin,
    /// The viewport stops following its content's end, where it stands.
    Unpin,
    /// A hand or an app landed the offset: follow the end only if it sits there (`at_end`)
    /// and the viewport sticks.
    Land { stick: bool, at_end: bool },
}

#[derive(Debug, PartialEq)]
pub(super) enum Effect {
    /// `El::on_scroll_offset`'s handler hears of a hand move at the frame's end.
    Tell,
}

/// Folds `event` into `mode`, returning what the caller should carry out.
pub(super) fn step(mode: &mut ScrollMode, event: Event) -> Effects<Effect> {
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
        Event::Land { stick, at_end } => {
            *mode = if stick && at_end { mode.to_pinned() } else { mode.to_rest() };
        }
    }
    Effects::none()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// origin/dev's hand latches, transcribed from `runtime/motion/tracks.rs` (the step's
    /// derivation), `runtime/scroll/offset.rs` (`ease_to_shrunk_end`'s cut) and
    /// `runtime/scroll/touch.rs` (a finger's hold and coast), as the loose fields they were.
    #[derive(Default)]
    struct Dev {
        direct: bool,
        scrolling: bool,
        by_hand: bool,
        shrink_cut: bool,
        laid: Option<f64>,
        mode: DevMode,
    }

    #[derive(Clone, Copy, Default, PartialEq, Debug)]
    enum DevMode {
        #[default]
        Rest,
        Pinned,
        Momentum(f32),
        Glide(f32),
    }

    impl Dev {
        fn coast(&self) -> f32 {
            match self.mode {
                DevMode::Momentum(v) => v,
                _ => 0.0,
            }
        }

        /// `runtime/motion/tracks.rs`: the step's `scrolling`, `by_hand` and `direct`.
        fn step(&mut self, x_v: f32) {
            self.scrolling = self.direct || self.coast().abs() > 1.0 || x_v.abs() > 1.0;
            self.by_hand |= self.scrolling;
            self.direct = false;
        }

        /// `runtime/scroll/offset.rs`: the cut a hand puts on a shrink glide.
        fn growth(&mut self, moving: bool) {
            self.shrink_cut = moving && (self.shrink_cut || self.direct || self.coast().abs() > 1.0);
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum Act {
        Moved,
        Hold,
        Coast(bool),
        Still,
        Growth(bool),
        Step { x_v: f32 },
        Laid(bool),
        Take,
        Rest,
        Halt,
        Pin,
        Unpin,
        Land { stick: bool, at_end: bool },
        Momentum(f32),
        Glide,
    }

    fn production_mode(mode: ScrollMode) -> DevMode {
        match mode {
            ScrollMode::Rest(_) => DevMode::Rest,
            ScrollMode::Pinned(_) => DevMode::Pinned,
            ScrollMode::Momentum { v, .. } => DevMode::Momentum(v),
            ScrollMode::Glide { v, .. } => DevMode::Glide(v),
        }
    }

    /// Drives every sequence through the production machine and through dev's fields, one
    /// event at a time, comparing what each holds after every event.
    fn walk(acts: &[Act]) {
        let (mut mode, mut dev) = (ScrollMode::default(), Dev::default());
        for (i, act) in acts.iter().enumerate() {
            let told = match act {
                Act::Moved => { let _ = step(&mut mode, Event::Moved); dev.direct = true; false }
                Act::Hold => {
                    let _ = step(&mut mode, Event::Hold);
                    dev.scrolling = true;
                    dev.by_hand = true;
                    false
                }
                Act::Coast(moving) => {
                    let _ = step(&mut mode, Event::Coast { moving: *moving });
                    dev.scrolling = *moving;
                    dev.by_hand |= *moving;
                    dev.direct = false;
                    false
                }
                Act::Still => { let _ = step(&mut mode, Event::Still); dev.scrolling = false; false }
                Act::Growth(moving) => { let _ = step(&mut mode, Event::Growth { moving: *moving }); dev.growth(*moving); false }
                Act::Step { x_v } => { let _ = step(&mut mode, Event::Step { x_v: *x_v }); dev.step(*x_v); false }
                Act::Laid(has) => {
                    let end = has.then_some(120.0);
                    let _ = step(&mut mode, Event::Laid(end));
                    dev.laid = end;
                    false
                }
                Act::Take => {
                    let effects: Vec<Effect> = step(&mut mode, Event::Take).into_iter().collect();
                    let was = std::mem::take(&mut dev.by_hand);
                    assert_eq!(effects, if was { vec![Effect::Tell] } else { vec![] }, "{acts:?}, event {i}: the take");
                    false
                }
                Act::Rest => { mode = mode.to_rest(); dev.mode = DevMode::Rest; false }
                Act::Halt => {
                    let _ = step(&mut mode, Event::Halt);
                    if dev.mode != DevMode::Pinned { dev.mode = DevMode::Rest; }
                    false
                }
                Act::Pin => {
                    let _ = step(&mut mode, Event::Pin);
                    if dev.mode != DevMode::Pinned { dev.mode = DevMode::Pinned; }
                    false
                }
                Act::Unpin => {
                    let _ = step(&mut mode, Event::Unpin);
                    if dev.mode == DevMode::Pinned { dev.mode = DevMode::Rest; }
                    false
                }
                Act::Land { stick, at_end } => {
                    let _ = step(&mut mode, Event::Land { stick: *stick, at_end: *at_end });
                    dev.mode = if *stick && *at_end { DevMode::Pinned } else { DevMode::Rest };
                    false
                }
                Act::Momentum(v) => { mode = mode.to_momentum(*v); dev.mode = DevMode::Momentum(*v); false }
                Act::Glide => { mode = mode.to_glide(2.0, GlideTo::Bottom); dev.mode = DevMode::Glide(2.0); false }
            };
            let at = format!("{acts:?}, event {i} ({act:?})");
            assert_eq!(mode.hand().direct, dev.direct, "direct: {at}");
            assert_eq!(mode.hand().scrolling, dev.scrolling, "scrolling: {at}");
            assert_eq!(mode.hand().by_hand, dev.by_hand, "by_hand: {at}");
            assert_eq!(mode.hand().shrink_cut, dev.shrink_cut, "shrink_cut: {at}");
            assert_eq!(mode.hand().laid, dev.laid, "laid: {at}");
            assert_eq!(production_mode(mode), dev.mode, "mode: {at}");
            let _ = told;
        }    }

    /// Every order of hand moves, growth steps, step derivations, takes and motion changes up
    /// to five long: the folded latches must match dev's loose fields after each event.
    #[test]
    fn the_latches_match_devs_fields_over_every_order() {
        let acts = [
            Act::Moved, Act::Hold, Act::Coast(true), Act::Coast(false), Act::Still,
            Act::Growth(true), Act::Growth(false), Act::Step { x_v: 4.0 }, Act::Step { x_v: 0.0 },
            Act::Laid(true), Act::Laid(false), Act::Take, Act::Rest, Act::Halt, Act::Pin,
            Act::Unpin, Act::Land { stick: true, at_end: true }, Act::Land { stick: false, at_end: true },
            Act::Momentum(2.0), Act::Glide,
        ];
        for len in 0..=4 {
            walk_all(&acts, len, &mut Vec::new());
        }
    }

    fn walk_all(acts: &[Act], len: usize, prefix: &mut Vec<Act>) {
        if prefix.len() == len {
            walk(prefix);
            return;
        }
        for &act in acts {
            prefix.push(act);
            walk_all(acts, len, prefix);
            prefix.pop();
        }
    }

    /// A hand move is cleared by the step that derives it, and a take reports the hand's move
    /// once, however many steps grew it.
    #[test]
    fn a_hand_move_is_told_once_and_the_steps_keep_growing_it() {
        let mut mode = ScrollMode::default();
        let _ = step(&mut mode, Event::Moved);
        let _ = step(&mut mode, Event::Step { x_v: 0.0 });
        assert!(mode.hand().scrolling && mode.hand().by_hand && !mode.hand().direct);
        let _ = step(&mut mode, Event::Step { x_v: 0.0 });
        assert!(!mode.hand().scrolling, "the step moved nothing");
        assert!(mode.hand().by_hand, "the watcher still has to hear it");
        assert_eq!(step(&mut mode, Event::Take).into_iter().collect::<Vec<_>>(), [Effect::Tell]);
        assert_eq!(step(&mut mode, Event::Take).into_iter().collect::<Vec<_>>(), []);
    }

    /// Whichever motion the mode changes to, the hand's latches come with it.
    #[test]
    fn a_motion_change_keeps_the_hands_latches() {
        let mut mode = ScrollMode::default();
        let _ = step(&mut mode, Event::Moved);
        let _ = step(&mut mode, Event::Laid(Some(300.0)));
        for changed in [mode.to_glide(1.0, GlideTo::Offset(40.0)), mode.to_momentum(2.0), mode.to_pinned(), mode.to_rest()] {
            assert!(changed.hand().direct && changed.hand().laid == Some(300.0), "{changed:?}");
            mode = changed;
        }
        assert!(mode.hand().direct && mode.hand().laid == Some(300.0));
    }
}
