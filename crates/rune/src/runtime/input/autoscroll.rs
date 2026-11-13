//! Middle-click autoscroll, as Chromium has it on Windows and Linux (Blink's
//! `AutoscrollController`). A middle press that no element takes (`El::on_middle_press`, a
//! link's `on_link`, `El::pointer_buttons`), over a viewport that can scroll, scrolls that
//! viewport toward the pointer, on each axis it scrolls on, at a speed that grows with the
//! pointer's distance from where the press landed: none within [`RADIUS`], then
//! `RATE · d^EXPONENT` points a second (measured on Desktop at a 1x raster: 29 at 20 px, 616 at
//! 80, 4,632 at 200, 11,290 at 300). Released after the pointer has left the radius, it stops;
//! released before, it keeps scrolling until the next press of any button or any key, which it
//! takes and nothing else hears. Wheel scrolls go on as usual meanwhile. The pointer shows
//! Chromium's pan cursors. A step asks for the next frame only while it moves something, so a
//! pointer resting inside the radius, or a viewport at its end, wakes nothing until it moves.
use crate::machine::{Attempt, Effects, Seq};
use crate::PointerButton;

use super::*;

/// The platforms whose browsers autoscroll on a middle click: Chromium on Windows and Linux.
/// macOS has no such gesture.
pub(super) const ENABLED: bool = cfg!(any(target_os = "windows", target_os = "linux"));
/// Points from where the press landed, per axis, that don't scroll (Blink's
/// `kNoMiddleClickAutoscrollRadius`).
const RADIUS: f32 = 15.0;
/// Speed is `RATE · d^EXPONENT` points a second for a pointer `d` points away on an axis.
const RATE: f32 = 0.04;
const EXPONENT: f32 = 2.2;
/// The longest a step covers, so a late frame doesn't jump.
const MAX_STEP: f64 = 0.05;

#[derive(Debug, Default)]
pub(super) struct Autoscroll {
    seq: Seq,
    state: State,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum State {
    #[default]
    Idle,
    /// Scrolling viewport `scroll` toward the pointer from `origin` (window points), on the
    /// `axes` (horizontal, vertical) it scrolls on. `held`: the middle button that started it is
    /// still down. `away`: the pointer has left the radius since, so that button's release
    /// stops it. `last`: when it last stepped. `attempt`: the step asked for, while one is.
    Active { scroll: NodeId, origin: (f32, f32), axes: (bool, bool), held: bool, away: bool, last: f64, attempt: Option<Attempt> },
    /// A press of this button ended it: its release goes nowhere either.
    Swallowing(PointerButton),
}

enum Event {
    Down { scroll: NodeId, origin: (f32, f32), axes: (bool, bool), now: f64 },
    Moved { at: (f32, f32), now: f64 },
    /// A frame for the step asked for as `attempt`, with the pointer `at`.
    Due { attempt: Attempt, now: f64, at: (f32, f32) },
    /// The step scrolled nothing: the viewport is at its end that way.
    Stuck,
    Press(PointerButton),
    Release(PointerButton),
    Key,
    Cancel,
}

#[derive(Debug, PartialEq)]
enum Effect {
    Scroll { scroll: NodeId, by: (f64, f64) },
    WakeAt(f64),
}

/// What a button or key did to it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Took {
    No,
    Yes,
}

/// Points a second on one axis for a pointer `d` points from the origin.
fn speed(d: f32) -> f32 {
    if d.abs() <= RADIUS { return 0.0; }
    RATE * d.abs().powf(EXPONENT) * d.signum()
}

fn velocity(origin: (f32, f32), at: (f32, f32), axes: (bool, bool)) -> (f32, f32) {
    let v = |on: bool, d: f32| if on { speed(d) } else { 0.0 };
    (v(axes.0, at.0 - origin.0), v(axes.1, at.1 - origin.1))
}

fn step(seq: &mut Seq, state: &mut State, event: Event) -> (Took, Effects<Effect>) {
    let none = |took| (took, Effects::none());
    match (event, *state) {
        (Event::Down { scroll, origin, axes, now }, State::Idle) => {
            *state = State::Active { scroll, origin, axes, held: true, away: false, last: now, attempt: None };
            none(Took::Yes)
        }
        (Event::Moved { at, now }, State::Active { scroll, origin, axes, held, away, last, attempt }) => {
            let v = velocity(origin, at, axes);
            let moving = v != (0.0, 0.0);
            // A pointer back inside the radius stops asking for frames; one that leaves it asks again.
            let (attempt, last, effects) = match (moving, attempt) {
                (false, _) => (None, last, Effects::none()),
                (true, Some(a)) => (Some(a), last, Effects::none()),
                (true, None) => (Some(seq.next()), now, Effects::one(Effect::WakeAt(now))),
            };
            *state = State::Active { scroll, origin, axes, held, away: away || moving, last, attempt };
            (Took::No, effects)
        }
        (Event::Due { attempt, now, at }, State::Active { scroll, origin, axes, held, away, last, attempt: Some(armed) }) if armed == attempt => {
            let (vx, vy) = velocity(origin, at, axes);
            let dt = (now - last).clamp(0.0, MAX_STEP);
            let by = (f64::from(vx) * dt, f64::from(vy) * dt);
            let next = seq.next();
            *state = State::Active { scroll, origin, axes, held, away, last: now, attempt: Some(next) };
            let mut effects = Effects::one(Effect::Scroll { scroll, by });
            effects.push(Effect::WakeAt(now));
            (Took::No, effects)
        }
        (Event::Stuck, State::Active { scroll, origin, axes, held, away, last, .. }) => {
            *state = State::Active { scroll, origin, axes, held, away, last, attempt: None };
            none(Took::No)
        }
        (Event::Release(PointerButton::Middle), State::Active { scroll, origin, axes, held: true, away, last, attempt }) => {
            *state = if away { State::Idle } else { State::Active { scroll, origin, axes, held: false, away, last, attempt } };
            none(Took::Yes)
        }
        (Event::Press(button), State::Active { .. }) => {
            *state = State::Swallowing(button);
            none(Took::Yes)
        }
        (Event::Release(button), State::Swallowing(held)) if button == held => {
            *state = State::Idle;
            none(Took::Yes)
        }
        (Event::Key, State::Active { .. }) => {
            *state = State::Idle;
            none(Took::Yes)
        }
        (Event::Cancel, _) => {
            *state = State::Idle;
            none(Took::No)
        }
        _ => none(Took::No),
    }
}

impl Autoscroll {
    fn attempt(&self) -> Option<Attempt> {
        match self.state {
            State::Active { attempt, .. } => attempt,
            _ => None,
        }
    }
}

impl<S: 'static> Runtime<S> {
    fn autoscroll_step(&mut self, event: Event) -> bool {
        let a = &mut self.autoscroll;
        let (took, effects) = step(&mut a.seq, &mut a.state, event);
        for effect in effects {
            match effect {
                // A step of no length (the first, in the frame of the move that armed it) moves
                // nothing and says nothing about the end.
                Effect::Scroll { scroll, by } => {
                    if by != (0.0, 0.0) && !self.autoscroll_by(scroll, by) {
                        let a = &mut self.autoscroll;
                        let _ = step(&mut a.seq, &mut a.state, Event::Stuck);
                    }
                }
                Effect::WakeAt(due) => self.wake_at(due),
            }
        }
        took == Took::Yes
    }

    /// A middle press at (x, y) that no element took: autoscroll the viewport there, if one can
    /// scroll. True when it started.
    pub(super) fn autoscroll_down(&mut self, x: f32, y: f32) -> bool {
        if !ENABLED { return false; }
        let Some((scroll, axes)) = self.autoscroll_viewport(x, y) else { return false };
        let now = self.now_clock();
        self.autoscroll_step(Event::Down { scroll, origin: (x, y), axes, now })
    }

    /// A press of `button` while it runs: it ends, and the press goes nowhere. True when it took
    /// the press.
    pub(super) fn autoscroll_press(&mut self, button: PointerButton) -> bool {
        self.autoscroll_step(Event::Press(button))
    }

    /// A release of `button`: the middle button that started it (which stops it once the pointer
    /// has left the radius), or the press that ended it. True when it took the release.
    pub(super) fn autoscroll_release(&mut self, button: PointerButton) -> bool {
        self.autoscroll_step(Event::Release(button))
    }

    /// A key went down while it runs: it ends, and the key goes nowhere. True when it took it.
    pub(super) fn autoscroll_key(&mut self) -> bool {
        self.autoscroll_step(Event::Key)
    }

    pub(super) fn autoscroll_moved(&mut self, x: f32, y: f32) {
        if !matches!(self.autoscroll.state, State::Active { .. }) { return; }
        let now = self.now_clock();
        self.autoscroll_step(Event::Moved { at: (x, y), now });
    }

    /// The window lost the pointer or its focus.
    pub(super) fn autoscroll_cancel(&mut self) {
        self.autoscroll_step(Event::Cancel);
    }

    /// Takes the step due by this frame.
    pub(super) fn fire_autoscroll(&mut self) {
        let (Some(attempt), Some(at)) = (self.autoscroll.attempt(), self.pointer) else { return };
        // The viewport went: nothing to scroll.
        if let State::Active { scroll, .. } = self.autoscroll.state
            && !self.scrolls.contains_key(&scroll)
        {
            self.autoscroll_cancel();
            return;
        }
        self.autoscroll_step(Event::Due { attempt, now: self.clock, at });
    }

    /// The pan cursor while it runs: which way it scrolls, or the axes it can.
    pub(super) fn autoscroll_cursor(&self) -> Option<crate::el::Cursor> {
        let State::Active { origin, axes, .. } = self.autoscroll.state else { return None };
        let (vx, vy) = self.pointer.map_or((0.0, 0.0), |at| velocity(origin, at, axes));
        Some(crate::el::Cursor::Image(crate::cursor::pan(axes, (vx, vy))))
    }

    /// The innermost viewport at (x, y) that can scroll, and the axes (horizontal, vertical) it can.
    fn autoscroll_viewport(&self, x: f32, y: f32) -> Option<(NodeId, (bool, bool))> {
        self.nodes.iter().enumerate().rev()
            .filter(|(i, n)| n.scroll.is_some() && !self.subtree_exiting(*i) && self.allowed(*i) && self.node_contains(*i, x, y))
            .find_map(|(_, n)| {
                let st = self.scrolls.get(&n.id)?;
                let axes = (n.scroll_axes.x() && st.x.max > 0.0, n.scroll_axes.has(false) && st.max > 0.0);
                (axes != (false, false)).then_some((n.id, axes))
            })
    }

    /// Scrolls viewport `id` by `by` (horizontal, vertical) points, as the wheel does. True when
    /// it moved.
    fn autoscroll_by(&mut self, id: NodeId, by: (f64, f64)) -> bool {
        let stick = self.node_index.get(&id).and_then(|&i| self.nodes[i].scroll).is_some_and(|s| s.stick_bottom);
        let Some(st) = self.scrolls.get_mut(&id) else { return false };
        let before = (st.x.offset, st.offset);
        st.x.offset = (st.x.offset + st.x.toward(by.0)).clamp(0.0, st.x.max);
        st.x.v = 0.0;
        st.x.glide = None;
        st.offset = (st.offset + by.1).clamp(0.0, st.max);
        st.moved();
        st.land(stick && by.1 >= 0.0);
        let moved = (st.x.offset, st.offset) != before;
        if moved { self.scroll_cancels_press(id); }
        moved
    }
}
