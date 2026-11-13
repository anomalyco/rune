//! Middle-click autoscroll, as Chromium has it on Windows and Linux (Blink's
//! `AutoscrollController`). A middle press that no element takes (`El::on_middle_press`, a
//! link's `on_link`, `El::pointer_buttons`), over a viewport that can scroll, scrolls toward the
//! pointer: on each axis, the innermost viewport under the press that scrolls on it, at a speed
//! that grows with the pointer's distance from where the press landed: none within [`RADIUS`],
//! then `RATE · d^EXPONENT` points a second (measured on Desktop at a 1x raster: 29 at 20 px, 616
//! at 80, 4,632 at 200, 11,290 at 300). Released after the pointer has left the radius, it stops;
//! released before, it keeps scrolling until the next press of any button or any key, which it
//! takes and nothing else hears. Wheel scrolls go on as usual meanwhile, and a pointer that leaves
//! the window keeps the speed its last position gave, as Chromium's does. The pointer shows
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

/// The viewports it scrolls: the innermost under the press on each axis (horizontal, vertical).
type Viewports = (Option<NodeId>, Option<NodeId>);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum State {
    #[default]
    Idle,
    /// Scrolling `views` toward the pointer, last seen `at`, from `origin` (window points).
    /// `held`: the middle button that started it is still down. `away`: the pointer has left the
    /// radius since, so that button's release stops it. `last`: when it last stepped.
    /// `attempt`: the step asked for, while one is.
    Active { views: Viewports, origin: (f32, f32), at: (f32, f32), held: bool, away: bool, last: f64, attempt: Option<Attempt> },
    /// A press of this button ended it: its release goes nowhere either.
    Swallowing(PointerButton),
}

enum Event {
    Down { views: Viewports, origin: (f32, f32), now: f64 },
    Moved { at: (f32, f32), now: f64 },
    /// A frame for the step asked for as `attempt`.
    Due { attempt: Attempt, now: f64 },
    /// The step scrolled nothing: the viewports are at their ends that way.
    Stuck,
    Press(PointerButton),
    Release(PointerButton),
    Key,
    Cancel,
}

#[derive(Debug, PartialEq)]
enum Effect {
    Scroll { views: Viewports, by: (f64, f64) },
    WakeAt(f64),
}

/// Points a second on one axis for a pointer `d` points from the origin.
fn speed(d: f32) -> f32 {
    if d.abs() <= RADIUS { return 0.0; }
    RATE * d.abs().powf(EXPONENT) * d.signum()
}

fn velocity(views: Viewports, origin: (f32, f32), at: (f32, f32)) -> (f32, f32) {
    let v = |on: Option<NodeId>, d: f32| if on.is_some() { speed(d) } else { 0.0 };
    (v(views.0, at.0 - origin.0), v(views.1, at.1 - origin.1))
}

/// Steps it; true when it took the input.
fn step(seq: &mut Seq, state: &mut State, event: Event) -> (bool, Effects<Effect>) {
    match (event, *state) {
        (Event::Down { views, origin, now }, State::Idle) => {
            *state = State::Active { views, origin, at: origin, held: true, away: false, last: now, attempt: None };
            (true, Effects::none())
        }
        (Event::Moved { at, now }, State::Active { views, origin, held, away, last, attempt, .. }) => {
            let moving = velocity(views, origin, at) != (0.0, 0.0);
            // A pointer back inside the radius stops asking for frames; one that leaves it asks again.
            let (attempt, last, effects) = match (moving, attempt) {
                (false, _) => (None, last, Effects::none()),
                (true, Some(a)) => (Some(a), last, Effects::none()),
                (true, None) => (Some(seq.next()), now, Effects::one(Effect::WakeAt(now))),
            };
            *state = State::Active { views, origin, at, held, away: away || moving, last, attempt };
            (false, effects)
        }
        (Event::Due { attempt, now }, State::Active { views, origin, at, held, away, last, attempt: Some(armed) }) if armed == attempt => {
            let (vx, vy) = velocity(views, origin, at);
            let dt = (now - last).clamp(0.0, MAX_STEP);
            *state = State::Active { views, origin, at, held, away, last: now, attempt: Some(seq.next()) };
            let mut effects = Effects::one(Effect::Scroll { views, by: (f64::from(vx) * dt, f64::from(vy) * dt) });
            effects.push(Effect::WakeAt(now));
            (false, effects)
        }
        (Event::Stuck, State::Active { views, origin, at, held, away, last, .. }) => {
            *state = State::Active { views, origin, at, held, away, last, attempt: None };
            (false, Effects::none())
        }
        (Event::Release(PointerButton::Middle), State::Active { views, origin, at, held: true, away, last, attempt }) => {
            *state = if away { State::Idle } else { State::Active { views, origin, at, held: false, away, last, attempt } };
            (true, Effects::none())
        }
        (Event::Press(button), State::Active { .. }) => {
            *state = State::Swallowing(button);
            (true, Effects::none())
        }
        (Event::Release(button), State::Swallowing(held)) if button == held => {
            *state = State::Idle;
            (true, Effects::none())
        }
        (Event::Key, State::Active { .. }) => {
            *state = State::Idle;
            (true, Effects::none())
        }
        (Event::Cancel, _) => {
            *state = State::Idle;
            (false, Effects::none())
        }
        _ => (false, Effects::none()),
    }
}

impl<S: 'static> Runtime<S> {
    fn autoscroll_step(&mut self, event: Event) -> bool {
        let a = &mut self.autoscroll;
        let (took, effects) = step(&mut a.seq, &mut a.state, event);
        for effect in effects {
            match effect {
                // A step of no length (the first, in the frame of the move that armed it) moves
                // nothing and says nothing about the ends.
                Effect::Scroll { views, by } => {
                    if by != (0.0, 0.0) && !self.autoscroll_by(views, by) {
                        let a = &mut self.autoscroll;
                        let _ = step(&mut a.seq, &mut a.state, Event::Stuck);
                    }
                }
                Effect::WakeAt(due) => self.wake_at(due),
            }
        }
        took
    }

    /// A middle press at (x, y) that no element took: autoscroll the viewports there, if one can
    /// scroll. True when it started.
    pub(super) fn autoscroll_down(&mut self, x: f32, y: f32) -> bool {
        if !ENABLED { return false; }
        let views = (self.autoscroll_viewport(x, y, true), self.autoscroll_viewport(x, y, false));
        if views == (None, None) { return false; }
        let now = self.now_clock();
        self.autoscroll_step(Event::Down { views, origin: (x, y), now })
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

    /// The window lost its focus or the pointer's capture.
    pub(super) fn autoscroll_cancel(&mut self) {
        self.autoscroll_step(Event::Cancel);
    }

    /// Takes the step due by this frame.
    pub(super) fn fire_autoscroll(&mut self) {
        let State::Active { views, attempt: Some(attempt), .. } = self.autoscroll.state else { return };
        // The viewports went: nothing to scroll.
        if [views.0, views.1].into_iter().flatten().all(|id| !self.scrolls.contains_key(&id)) {
            self.autoscroll_cancel();
            return;
        }
        self.autoscroll_step(Event::Due { attempt, now: self.clock });
    }

    /// The pan cursor while it runs: which way it scrolls, or the axes it can.
    pub(super) fn autoscroll_cursor(&self) -> Option<crate::el::Cursor> {
        let State::Active { views, origin, at, .. } = self.autoscroll.state else { return None };
        let v = velocity(views, origin, self.pointer.unwrap_or(at));
        Some(crate::el::Cursor::Image(crate::cursor::pan((views.0.is_some(), views.1.is_some()), v)))
    }

    /// The innermost viewport at (x, y) that can scroll on the horizontal axis or the vertical.
    fn autoscroll_viewport(&self, x: f32, y: f32, horizontal: bool) -> Option<NodeId> {
        self.nodes.iter().enumerate().rev()
            .filter(|(i, n)| n.scroll.is_some() && n.scroll_axes.has(horizontal) && !self.subtree_exiting(*i) && self.allowed(*i) && self.node_contains(*i, x, y))
            .find(|(_, n)| self.scrolls.get(&n.id).is_some_and(|st| if horizontal { st.x.max > 0.0 } else { st.max > 0.0 }))
            .map(|(_, n)| n.id)
    }

    /// Scrolls `views` by `by` (horizontal, vertical) points, as the wheel does. True when one
    /// moved.
    fn autoscroll_by(&mut self, views: Viewports, by: (f64, f64)) -> bool {
        let mut moved = false;
        if let Some(id) = views.0
            && let Some(st) = self.scrolls.get_mut(&id)
        {
            let before = st.x.offset;
            st.x.offset = (st.x.offset + st.x.toward(by.0)).clamp(0.0, st.x.max);
            st.x.v = 0.0;
            st.x.glide = None;
            st.moved();
            if st.x.offset != before {
                moved = true;
                self.scroll_cancels_press(id);
            }
        }
        if let Some(id) = views.1 {
            let stick = self.node_index.get(&id).and_then(|&i| self.nodes[i].scroll).is_some_and(|s| s.stick_bottom);
            if let Some(st) = self.scrolls.get_mut(&id) {
                let before = st.offset;
                st.offset = (st.offset + by.1).clamp(0.0, st.max);
                st.moved();
                st.land(stick && by.1 >= 0.0);
                if st.offset != before {
                    moved = true;
                    self.scroll_cancels_press(id);
                }
            }
        }
        moved
    }
}
