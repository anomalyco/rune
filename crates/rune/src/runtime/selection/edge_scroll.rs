//! A held press extending a selection keeps scrolling its list while the pointer stays at or past
//! the list's top or bottom edge, as an AppKit text view autoscrolls on its periodic events: each
//! step moves the list by an amount that grows with how far past the edge the pointer is. A step
//! is a timer: [`step`] keeps its `Attempt` and asks for the next frame, which feeds it back as
//! `Due`. Each step first extends the selection to the point under the pointer, over the frame
//! that laid out the step before it, and the step that reaches the end is followed by one that
//! only extends, so the rows the last scroll brought in are selected too. A pointer back inside,
//! the end of the list, a step that moved nothing (a list pinned to its bottom pulling it back),
//! and the drag ending stop it. Each step lets go of a list stuck to its bottom before
//! `step_scroll` can pull it back, so it leaves the bottom however small its steps are; once it
//! stops within half a point of the bottom, the list sticks there again.
use crate::machine::{Attempt, Effects, Seq};

use super::*;

/// Points a second per point the pointer is past the edge. AppKit's `autoscroll:` moves a clip
/// view by the pointer's distance outside it on each periodic event, which a text view's drag asks
/// for every tenth of a second.
const RATE: f32 = 10.0;
/// The least distance past the edge a step counts, so a pointer held at the edge (a window flush
/// with the screen's edge stops it there) still scrolls, at 80 points a second.
const MIN_PAST: f32 = 8.0;
/// The fastest it scrolls, in points a second.
const MAX_SPEED: f32 = 4000.0;
/// How far inside the edge still counts as at it, in points.
const AT: f32 = 1.0;
/// The longest a step covers, so a late frame doesn't jump.
const MAX_STEP: f64 = 0.05;

/// The edge scroll: stepping one viewport or not, and the attempts its steps carry.
#[derive(Debug, Default)]
pub(in crate::runtime) struct EdgeScroll {
    seq: Seq,
    state: State,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(in crate::runtime) enum State {
    #[default]
    Idle,
    /// Scrolling viewport `scroll`: it last stepped at `last` (clock seconds), from offset `from`
    /// when that step moved it, and steps again at `due` as attempt `attempt`.
    Running { attempt: Attempt, scroll: NodeId, last: f64, from: Option<f64>, due: f64 },
    /// The last step reached `scroll`'s end; at `due`, once that's laid out, the selection
    /// follows it there.
    Landing { attempt: Attempt, scroll: NodeId, due: f64 },
}

/// The held pointer at or past an edge of the viewport the selection is in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::runtime) struct Edge {
    pub(in crate::runtime) scroll: NodeId,
    /// Points a second, positive scrolling down.
    pub(in crate::runtime) speed: f32,
    /// How much further it can scroll that way, in points.
    pub(in crate::runtime) room: f64,
    /// Where it has scrolled to, in points.
    pub(in crate::runtime) offset: f64,
}

impl Edge {
    fn moves(&self) -> bool { self.speed != 0.0 && self.room > 0.0 }
}

pub(in crate::runtime) enum Event {
    /// The held pointer moved, to `edge` or away from every edge.
    Moved { now: f64, edge: Option<Edge> },
    /// A frame at `now` for the step asked for as `attempt`, with the pointer at `edge`.
    Due { attempt: Attempt, now: f64, edge: Option<Edge> },
    /// The drag ended.
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::runtime) enum Effect {
    /// Extend the selection to the pointer, then scroll `scroll` by `by` points.
    Step { scroll: NodeId, by: f64 },
    /// Extend the selection to the pointer.
    Extend,
    /// Wake at this clock time, when the next step is due.
    WakeAt(f64),
}

pub(in crate::runtime) fn step(seq: &mut Seq, state: &mut State, event: Event) -> Effects<Effect> {
    match event {
        Event::Moved { now, edge } => match (edge.filter(Edge::moves), *state) {
            (None, _) => {
                *state = State::Idle;
                Effects::none()
            }
            // The next step reads how far past the edge the pointer is now.
            (Some(e), State::Running { scroll, .. }) if scroll == e.scroll => Effects::none(),
            (Some(e), _) => {
                let attempt = seq.next();
                *state = State::Running { attempt, scroll: e.scroll, last: now, from: None, due: now };
                Effects::one(Effect::WakeAt(now))
            }
        },
        Event::Due { attempt, now, edge } => match *state {
            State::Landing { attempt: armed, due, .. } if armed == attempt && now >= due => {
                *state = State::Idle;
                Effects::one(Effect::Extend)
            }
            State::Running { attempt: armed, scroll, last, from, due } if armed == attempt && now >= due => {
                let Some(e) = edge.filter(|e| e.moves() && e.scroll == scroll) else {
                    *state = State::Idle;
                    return Effects::none();
                };
                // The last step moved nothing: whatever undid it would undo the next.
                if from.is_some_and(|f| (e.offset - f).abs() <= 1e-9) {
                    *state = State::Idle;
                    return Effects::none();
                }
                let by = f64::from(e.speed) * (now - last).clamp(0.0, MAX_STEP);
                let attempt = seq.next();
                // A step that reaches the end, or comes within a thousandth of a point of it,
                // goes exactly there.
                let (by, landing) = if e.room - by.abs() <= 1e-3 { (e.room.copysign(f64::from(e.speed)), true) } else { (by, false) };
                *state = if landing {
                    State::Landing { attempt, scroll, due: now }
                } else {
                    State::Running { attempt, scroll, last: now, from: (by != 0.0).then_some(e.offset), due: now }
                };
                Effects::two(Effect::Step { scroll, by }, Effect::WakeAt(now))
            }
            _ => Effects::none(),
        },
        Event::Stop => {
            *state = State::Idle;
            Effects::none()
        }
    }
}

impl EdgeScroll {
    fn step(&mut self, event: Event) -> Effects<Effect> { step(&mut self.seq, &mut self.state, event) }

    /// The attempt the next step carries, while one is asked for.
    fn attempt(&self) -> Option<Attempt> {
        match self.state {
            State::Running { attempt, .. } | State::Landing { attempt, .. } => Some(attempt),
            State::Idle => None,
        }
    }

    pub(in crate::runtime) fn running(&self) -> bool { self.attempt().is_some() }

}

/// Points a second for a pointer `past` points beyond the edge.
fn speed(past: f32) -> f32 { (RATE * past.max(MIN_PAST)).min(MAX_SPEED) }

impl<S: 'static> Runtime<S> {
    /// The held pointer moved to height `y`: step while it's at or past an edge the selection's
    /// viewport can still scroll past, and stop when it isn't.
    pub(in crate::runtime) fn edge_scroll_moved(&mut self, y: f32) {
        if !self.selectable.phase.dragging() && !self.selectable.edge.running() { return; }
        let edge = self.edge_at(y);
        let now = self.now_clock();
        let effects = self.selectable.edge.step(Event::Moved { now, edge });
        self.carry_edge_scroll(effects);
    }

    /// The drag ended.
    pub(in crate::runtime) fn stop_edge_scroll(&mut self) { let _ = self.selectable.edge.step(Event::Stop); }

    /// Takes the step due by this frame.
    pub(in crate::runtime) fn fire_edge_scroll(&mut self) {
        let Some(attempt) = self.selectable.edge.attempt() else { return };
        let edge = self.pointer.and_then(|(_, y)| self.edge_at(y));
        let effects = self.selectable.edge.step(Event::Due { attempt, now: self.clock, edge });
        self.carry_edge_scroll(effects);
    }

    fn carry_edge_scroll(&mut self, effects: Effects<Effect>) {
        for effect in effects {
            match effect {
                Effect::Step { scroll, by } => {
                    self.extend_to_pointer();
                    self.edge_scroll_by(scroll, by);
                }
                Effect::Extend => self.extend_to_pointer(),
                Effect::WakeAt(due) => self.wake_at(due),
            }
        }
    }

    /// The selection follows the pointer over what the last frame laid out.
    fn extend_to_pointer(&mut self) {
        if let Some((x, y)) = self.pointer { self.selection_drag(x, y); }
    }

    /// Scrolls viewport `id` by `by` points, as the wheel does, except that a list stuck to its
    /// bottom lets go of it even when the step leaves it within the half point the wheel pins in.
    fn edge_scroll_by(&mut self, id: NodeId, by: f64) {
        let Some(st) = self.scrolls.get_mut(&id) else { return };
        let before = st.offset;
        st.offset = (st.offset + by).clamp(0.0, st.max);
        st.moved();
        st.land(false);
        if st.offset != before { self.scroll_cancels_press(id); }
    }

    /// Where height `y` is against the edges of the vertical viewport around the selection a
    /// held press is extending: at or past its top or bottom (as drawn, inside its clips and the
    /// window), how fast that scrolls, and how far it still can.
    fn edge_at(&self, y: f32) -> Option<Edge> {
        if !self.selectable.phase.dragging() || !self.press.is_held() { return None; }
        let holder = self.selection_holder()?;
        let i = std::iter::successors(Some(holder), |&p| self.nodes[p].parent)
            .find(|&p| self.nodes[p].scroll.is_some() && self.nodes[p].scroll_axes.has(false))?;
        let id = self.nodes[i].id;
        let st = self.scrolls.get(&id)?;
        let clip = self.screen_clip_of_parent(self.nodes[i].parent).map(|v| v / self.env.scale);
        let [_, top, _, bottom] = intersect(intersect(self.drawn_edges(i), clip), [0.0, 0.0, self.viewport.0, self.viewport.1]);
        if bottom - top <= 2.0 * AT { return None; }
        let (speed, room) = if y >= bottom - AT {
            (speed(y - (bottom - AT)), st.max - st.offset)
        } else if y <= top + AT {
            (-speed(top + AT - y), st.offset)
        } else {
            return None;
        };
        Some(Edge { scroll: id, speed, room, offset: st.offset })
    }
}

#[cfg(test)]
#[path = "edge_scroll_tests.rs"]
mod tests;
