use super::*;

/// origin/dev's hand latches, transcribed from `runtime/motion/tracks.rs` (the step's
/// derivation), `runtime/scroll/offset.rs` (`ease_to_shrunk_end`'s cut) and
/// `runtime/scroll/touch.rs` (a finger's hold and coast), as the loose fields they were.
#[derive(Default)]
struct Dev {
    direct: bool,
    scrolling: bool,
    by_hand: bool,
    source: crate::ScrollSource,
    shrink_cut: bool,
    laid: Option<f64>,
    mode: DevMode,
    /// Mode writes made by the last event.
    writes: usize,
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

    fn write(&mut self, mode: DevMode) {
        self.mode = mode;
        self.writes += 1;
    }

    /// `ScrollState::pin`: writing nothing when it already follows its bottom.
    fn pin(&mut self) {
        if self.mode != DevMode::Pinned { self.write(DevMode::Pinned); }
    }

    fn halt(&mut self) {
        if self.mode != DevMode::Pinned { self.write(DevMode::Rest); }
    }

    fn unpin(&mut self) {
        if self.mode == DevMode::Pinned { self.write(DevMode::Rest); }
    }

    /// `ScrollState::land`: a pinned end stays as it is, and `stick` short-circuits the
    /// comparison with the offset and its max.
    fn land(&mut self, offset: f64, max: f64, stick: bool) {
        if stick && offset >= max - 0.5 { self.pin() } else { self.write(DevMode::Rest); }
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
    Land { offset: f64, max: f64, stick: bool },
    Momentum(f32),
    Glide,
}

fn production_mode(mode: ScrollMode) -> DevMode {
    match mode {
        ScrollMode::Rest(_) => DevMode::Rest,
        ScrollMode::Pinned(_) => DevMode::Pinned,
        ScrollMode::Momentum { v, .. } => DevMode::Momentum(v),
        ScrollMode::Glide { v, .. } => DevMode::Glide(v),
        ScrollMode::Wheel { .. } => unreachable!("no act starts a wheel's ease"),
    }
}

/// Drives every sequence through the production machine and through dev's fields, one
/// event at a time, comparing what each holds, and how many mode writes each made, after
/// every event.
fn walk(acts: &[Act]) {
    let (mut st, mut dev) = (ScrollState::default(), Dev::default());
    for (i, act) in acts.iter().enumerate() {
        dev.writes = 0;
        match act {
            Act::Moved => { st.moved(); dev.direct = true; }
            Act::Hold => {
                let _ = step(&mut st.mode, Event::Hold);
                dev.scrolling = true;
                dev.by_hand = true;
            }
            Act::Coast(moving) => {
                let source = if *moving { crate::ScrollSource::Momentum } else { crate::ScrollSource::None };
                let _ = step(&mut st.mode, Event::Coast { moving: *moving, source });
                dev.scrolling = *moving;
                dev.by_hand |= *moving;
                dev.source = merge_source(dev.source, source);
                dev.direct = false;
            }
            Act::Still => { let _ = step(&mut st.mode, Event::Still); dev.scrolling = false; }
            Act::Growth(moving) => { let _ = step(&mut st.mode, Event::Growth { moving: *moving }); dev.growth(*moving); }
            Act::Step { x_v } => { let _ = step(&mut st.mode, Event::Step { x_v: *x_v }); dev.step(*x_v); }
            Act::Laid(has) => {
                let end = has.then_some(120.0);
                let _ = step(&mut st.mode, Event::Laid(end));
                dev.laid = end;
            }
            Act::Take => {
                let effects: Vec<Effect> = step(&mut st.mode, Event::Take).into_iter().collect();
                let was = std::mem::take(&mut dev.by_hand);
                let src = std::mem::take(&mut dev.source);
                let expected = if was || src != crate::ScrollSource::None {
                    vec![Effect::Tell { by_hand: was, source: src }]
                } else {
                    vec![]
                };
                assert_eq!(effects, expected, "{acts:?}, event {i}: the take");
            }
            Act::Rest => { st.mode = st.mode.to_rest(); dev.write(DevMode::Rest); }
            Act::Halt => { st.halt(); dev.halt(); }
            Act::Pin => { st.pin(); dev.pin(); }
            Act::Unpin => { st.unpin(); dev.unpin(); }
            Act::Land { offset, max, stick } => {
                st.offset = *offset;
                st.max = *max;
                st.land(*stick);
                dev.land(*offset, *max, *stick);
            }
            Act::Momentum(v) => { st.mode = st.mode.to_momentum(*v); dev.write(DevMode::Momentum(*v)); }
            Act::Glide => { st.mode = st.mode.to_glide(2.0, GlideTo::Bottom); dev.write(DevMode::Glide(2.0)); }
        }
        let at = format!("{acts:?}, event {i} ({act:?})");
        assert_eq!(st.mode.hand().direct, dev.direct, "direct: {at}");
        assert_eq!(st.mode.hand().scrolling, dev.scrolling, "scrolling: {at}");
        assert_eq!(st.mode.hand().by_hand, dev.by_hand, "by_hand: {at}");
        assert_eq!(st.mode.hand().source, dev.source, "source: {at}");
        assert_eq!(st.mode.hand().shrink_cut, dev.shrink_cut, "shrink_cut: {at}");
        assert_eq!(st.mode.hand().laid, dev.laid, "laid: {at}");
        assert_eq!(production_mode(st.mode), dev.mode, "mode: {at}");
        assert_eq!(writes::take(), dev.writes, "mode writes: {at}");
    }
}

/// Every order of hand moves, growth steps, step derivations, takes and motion changes up
/// to five long: the folded latches must match dev's loose fields after each event.
#[test]
fn the_latches_match_devs_fields_over_every_order() {
    let acts = [
        Act::Moved, Act::Hold, Act::Coast(true), Act::Coast(false), Act::Still,
        Act::Growth(true), Act::Growth(false), Act::Step { x_v: 4.0 }, Act::Step { x_v: 0.0 },
        Act::Laid(true), Act::Laid(false), Act::Take, Act::Rest, Act::Halt, Act::Pin,
        Act::Unpin, Act::Land { offset: 100.0, max: 100.0, stick: true },
        Act::Land { offset: 0.0, max: 100.0, stick: true }, Act::Land { offset: 100.0, max: 100.0, stick: false },
        Act::Momentum(2.0), Act::Glide,
    ];
    for len in 0..=4 {
        walk_all(&acts, len, &mut Vec::new());
    }
}

/// dev's `land` calls the guarded `pin`, so a landing that changes nothing writes nothing.
#[test]
fn a_landing_on_a_pinned_viewport_writes_nothing() {
    let mut st = ScrollState { offset: 100.0, max: 100.0, ..ScrollState::default() };
    st.pin();
    let _ = writes::take();
    st.land(true);
    assert_eq!(writes::take(), 0, "already at the pinned end, the landing writes nothing");
    st.land(false);
    assert_eq!(writes::take(), 1, "off the end it writes the rest");
    st.offset = 0.0;
    st.land(true);
    assert_eq!(writes::take(), 1, "short of the end it writes the rest");
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
    assert_eq!(
        step(&mut mode, Event::Take).into_iter().collect::<Vec<_>>(),
        [Effect::Tell { by_hand: true, source: crate::ScrollSource::None }]
    );
    assert_eq!(step(&mut mode, Event::Take).into_iter().collect::<Vec<_>>(), []);
}

/// Whichever motion the mode changes to, the hand's latches come with it.
#[test]
fn a_motion_change_keeps_the_hands_latches() {
    let mut mode = ScrollMode::default();
    let _ = step(&mut mode, Event::Moved);
    let _ = step(&mut mode, Event::Source(crate::ScrollSource::EdgeReturn));
    let _ = step(&mut mode, Event::Laid(Some(300.0)));
    for changed in [mode.to_glide(1.0, GlideTo::Offset(40.0)), mode.to_momentum(2.0), mode.to_pinned(), mode.to_rest()] {
        assert!(changed.hand().direct && changed.hand().source == crate::ScrollSource::EdgeReturn && changed.hand().laid == Some(300.0), "{changed:?}");
        mode = changed;
    }
    assert!(mode.hand().direct && mode.hand().source == crate::ScrollSource::EdgeReturn && mode.hand().laid == Some(300.0));
}

/// Multiple scroll causes in one frame collapse by priority:
/// `Direct > Momentum > EdgeReturn > Programmatic > None`, and `Take` clears it back to `None`.
#[test]
fn scroll_source_priority_resolves_highest_and_take_clears_it() {
    use crate::ScrollSource;

    let mut mode = ScrollMode::default();
    let _ = step(&mut mode, Event::Source(ScrollSource::Programmatic));
    assert_eq!(mode.hand().source, ScrollSource::Programmatic);
    let _ = step(&mut mode, Event::Source(ScrollSource::None));
    assert_eq!(mode.hand().source, ScrollSource::Programmatic);
    let _ = step(&mut mode, Event::Source(ScrollSource::EdgeReturn));
    assert_eq!(mode.hand().source, ScrollSource::EdgeReturn);
    let _ = step(&mut mode, Event::Source(ScrollSource::Programmatic));
    assert_eq!(mode.hand().source, ScrollSource::EdgeReturn);
    let _ = step(&mut mode, Event::Coast { moving: true, source: ScrollSource::Momentum });
    assert_eq!(mode.hand().source, ScrollSource::Momentum);
    let _ = step(&mut mode, Event::Coast { moving: false, source: ScrollSource::EdgeReturn });
    assert_eq!(mode.hand().source, ScrollSource::Momentum);
    let _ = step(&mut mode, Event::Source(ScrollSource::Direct));
    assert_eq!(mode.hand().source, ScrollSource::Direct);
    let _ = step(&mut mode, Event::Coast { moving: true, source: ScrollSource::Momentum });
    assert_eq!(mode.hand().source, ScrollSource::Direct);

    assert_eq!(
        step(&mut mode, Event::Take).into_iter().collect::<Vec<_>>(),
        [Effect::Tell { by_hand: true, source: ScrollSource::Direct }]
    );
    assert_eq!(mode.hand().source, ScrollSource::None);
    assert_eq!(step(&mut mode, Event::Take).into_iter().collect::<Vec<_>>(), []);

    // A pure Programmatic source reports without setting by_hand.
    let _ = step(&mut mode, Event::Source(ScrollSource::Programmatic));
    assert_eq!(
        step(&mut mode, Event::Take).into_iter().collect::<Vec<_>>(),
        [Effect::Tell { by_hand: false, source: ScrollSource::Programmatic }]
    );
    assert_eq!(step(&mut mode, Event::Take).into_iter().collect::<Vec<_>>(), []);
}
