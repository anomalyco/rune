//! Touch scrolling: a finger pans the scroll viewport under it, rubber-bands past its ends,
//! and flings it on with momentum, on every platform that reports touches (`runtime/input/touch.rs`
//! turns touches into presses, drags and these pans).
use super::*;
use crate::machine::Effects;
use crate::runtime::scroll_mode::{self as mode, Event as ModeEvent};

/// What a touch holds: the viewport its finger went down on, one it pans, or one it let go
/// that is still settling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::runtime) enum TouchScroll {
    /// The finger is down on this viewport, before its pan picks an axis. `origin` is the
    /// offset it holds it at.
    Held { id: NodeId, origin: f64, horizontal: bool },
    /// The finger pans this viewport from `origin`, the offset it caught it at.
    Dragging { id: NodeId, origin: f64, horizontal: bool },
    /// The finger let go; the viewport settles on its own momentum or edge return.
    Coasting { id: NodeId, horizontal: bool },
}

impl TouchScroll {
    /// The viewport it holds.
    fn id(self) -> NodeId {
        match self {
            Self::Held { id, .. } | Self::Dragging { id, .. } | Self::Coasting { id, .. } => id,
        }
    }

    /// Whether it moves the viewport's horizontal axis (`El::scroll_x`).
    fn horizontal(self) -> bool {
        match self {
            Self::Held { horizontal, .. } | Self::Dragging { horizontal, .. } | Self::Coasting { horizontal, .. } => horizontal,
        }
    }

    /// The offset the finger holds or caught the viewport at, while it has it.
    fn origin(self) -> Option<f64> {
        match self {
            Self::Held { origin, .. } | Self::Dragging { origin, .. } => Some(origin),
            Self::Coasting { .. } => None,
        }
    }

    /// Whether the finger has let go and the viewport settles on its own.
    fn coasting(self) -> bool {
        matches!(self, Self::Coasting { .. })
    }
}

/// What happens to a touch's pan.
#[derive(Clone, Copy, Debug)]
pub(in crate::runtime) enum TouchEvent {
    /// A finger went down: `across` is the horizontal viewport under it and `vertical` the
    /// vertical one, each with its current offset.
    Down { across: Option<(NodeId, f64)>, vertical: Option<(NodeId, f64)> },
    /// The finger started panning, mostly horizontally or not.
    Pan { horizontal: bool, across: Option<(NodeId, f64)>, vertical: Option<(NodeId, f64)> },
    /// The finger lifted from `id` with `velocity` along its axis.
    Up { id: Option<NodeId>, velocity: f32 },
    /// The finger moved `d` points from the offset it caught the viewport at.
    Move { id: NodeId, d: f32 },
    /// A frame for the viewport's panned axis: its overflow bounds and the frame's time.
    Tick { id: NodeId, horizontal: bool, overflow: f64, dt: f32 },
    /// A settling viewport is still moving (`true`) or has come to rest (`false`).
    Settled { moving: bool },
}

/// What carrying out a transition does to the scroll viewports.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::runtime) enum TouchEffect {
    /// Stop this viewport's momentum.
    Halt(NodeId),
    /// Stop this viewport's horizontal momentum: a touch landed across it.
    StopAcross(NodeId),
    /// Give this viewport the lifted finger's velocity along its axis.
    Fling { id: NodeId, horizontal: bool, velocity: f32 },
    /// Move this viewport `d` points from the offset the finger caught it at.
    Pan { id: NodeId, origin: f64, horizontal: bool, d: f32 },
    /// A frame while the finger holds this viewport: set its pane's bounds to `overflow`.
    Hold { id: NodeId, horizontal: bool, overflow: f64 },
    /// A frame while the viewport coasts: step its momentum and edge return by `dt`.
    Coast { id: NodeId, horizontal: bool, overflow: f64, dt: f32 },
}

/// The pan's next state, and what to do to the viewports. A finger holds the viewport it
/// went down on until its pan picks an axis, drags it from the offset it caught it at, then
/// lets it coast until it comes to rest.
pub(in crate::runtime) fn step(state: &mut Option<TouchScroll>, event: TouchEvent) -> Effects<TouchEffect> {
    match (*state, event) {
        (current, TouchEvent::Down { across, vertical }) => {
            let caught = matches!((current, across), (Some(t), Some((id, _))) if t.horizontal() && t.id() == id);
            if caught {
                *state = across.map(|(id, at)| TouchScroll::Held { id, origin: at, horizontal: true });
            } else if let Some((id, at)) = vertical {
                *state = Some(TouchScroll::Held { id, origin: at, horizontal: false });
            }
            let mut effects = Effects::none();
            if let Some((id, _)) = across {
                effects.push(TouchEffect::StopAcross(id));
            }
            if let Some((id, _)) = vertical {
                effects.push(TouchEffect::Halt(id));
            }
            effects
        }
        (_, TouchEvent::Pan { horizontal: true, across: Some((id, at)), .. }) => {
            *state = Some(TouchScroll::Dragging { id, origin: at, horizontal: true });
            Effects::none()
        }
        (Some(t), TouchEvent::Pan { vertical: Some((id, at)), .. }) => {
            // A horizontal viewport the touch caught lets go, and the vertical one takes the pan.
            let origin = if t.horizontal() { at } else { t.origin().unwrap_or(at) };
            *state = Some(TouchScroll::Dragging { id, origin, horizontal: false });
            Effects::none()
        }
        (Some(t), TouchEvent::Pan { .. }) if t.horizontal() => {
            *state = Some(TouchScroll::Coasting { id: t.id(), horizontal: true });
            Effects::none()
        }
        (_, TouchEvent::Pan { .. }) => Effects::none(),
        (Some(t), TouchEvent::Up { id, velocity }) => {
            *state = Some(TouchScroll::Coasting { id: t.id(), horizontal: t.horizontal() });
            if Some(t.id()) == id {
                Effects::one(TouchEffect::Fling { id: t.id(), horizontal: t.horizontal(), velocity })
            } else {
                Effects::none()
            }
        }
        (Some(TouchScroll::Dragging { id: held, origin, horizontal }), TouchEvent::Move { id, d }) if held == id => {
            Effects::one(TouchEffect::Pan { id, origin, horizontal, d })
        }
        (Some(t), TouchEvent::Tick { id, horizontal, overflow, dt }) if t.id() == id && t.horizontal() == horizontal => {
            if t.coasting() {
                Effects::one(TouchEffect::Coast { id, horizontal, overflow, dt })
            } else {
                Effects::one(TouchEffect::Hold { id, horizontal, overflow })
            }
        }
        (Some(t), TouchEvent::Settled { moving: false }) if t.coasting() => {
            *state = None;
            Effects::none()
        }
        (_, _) => Effects::none(),
    }
}

/// A critically damped edge return, integrated exactly for variable frame times.
fn rebound(offset: f64, velocity: f32, target: f64, dt: f32) -> (f64, f32) {
    let dt = f64::from(dt);
    let displacement = offset - target;
    let b = f64::from(velocity) + 14.0 * displacement;
    let decay = (-14.0 * dt).exp();
    (target + (displacement + b * dt) * decay, ((f64::from(velocity) - 14.0 * b * dt) * decay) as f32)
}

impl RuntimeCore {
    /// A touch went down: it stops the momentum of the viewports under it and holds the
    /// vertical one until the pan picks its axis (`touch_scroll_pan`). A horizontal one still
    /// settling from an earlier touch (momentum or its edge return) holds where the finger
    /// caught it instead.
    pub(in crate::runtime) fn touch_scroll_start(&mut self, x: f32, y: f32) -> Option<NodeId> {
        let across = self.scroll_at_axis(x, y, true).and_then(|id| self.scroll.scrolls.get(&id).map(|st| (id, st.x.offset)));
        let vertical = self.scroll_at(x, y);
        let held = vertical.and_then(|id| self.scroll.scrolls.get(&id).map(|st| (id, st.offset)));
        self.step_touch(TouchEvent::Down { across, vertical: held });
        vertical
    }

    /// The touch that went down at (x, y) started panning, mostly horizontally or not: a
    /// horizontal pan takes the innermost horizontal viewport there, and any other pan stays
    /// with the vertical one. Returns the viewport and whether it moves horizontally.
    pub(in crate::runtime) fn touch_scroll_pan(&mut self, x: f32, y: f32, horizontal: bool, vertical: Option<NodeId>) -> (Option<NodeId>, bool) {
        let across = horizontal
            .then(|| self.scroll_at_axis(x, y, true))
            .flatten()
            .and_then(|id| self.scroll.scrolls.get(&id).map(|st| (id, st.x.offset)));
        let held = vertical.and_then(|id| self.scroll.scrolls.get(&id).map(|st| (id, st.offset)));
        self.step_touch(TouchEvent::Pan { horizontal, across, vertical: held });
        match across {
            Some((id, _)) => (Some(id), true),
            None => (vertical, false),
        }
    }

    /// The pan moved `d` points along its axis from where the touch went down.
    pub(in crate::runtime) fn touch_scroll_move(&mut self, id: NodeId, d: f32) {
        self.step_touch(TouchEvent::Move { id, d });
    }

    /// The touch lifted from viewport `id` (None when it held none) with `velocity` along its
    /// axis. Whatever it held lets go, a viewport it caught without panning included.
    pub(in crate::runtime) fn touch_scroll_end(&mut self, id: Option<NodeId>, velocity: f32) {
        self.step_touch(TouchEvent::Up { id, velocity });
    }

    /// Own the touched viewport's panned axis through momentum and its edge return. Other
    /// viewports use the native wheel path, including native clamping and stick-to-bottom.
    pub(in crate::runtime) fn step_touch_scroll(&mut self, id: NodeId, overflow: f64, dt: f32, horizontal: bool) -> Option<bool> {
        let effects = step(self.pointer.touch_scroll_mut(), TouchEvent::Tick { id, horizontal, overflow, dt });
        self.apply_touch(effects)
    }

    /// Step the pan and carry out what the transition asked for.
    fn step_touch(&mut self, event: TouchEvent) {
        let effects = step(self.pointer.touch_scroll_mut(), event);
        let _ = self.apply_touch(effects);
    }

    /// Carry out what a touch transition asked for on the scroll viewports: the drag's and the
    /// coast's math lives here, and a coast reports whether its viewport is still moving.
    fn apply_touch(&mut self, effects: Effects<TouchEffect>) -> Option<bool> {
        let mut active = None;
        for effect in effects {
            match effect {
                TouchEffect::Halt(id) => {
                    if let Some(st) = self.scroll.scrolls.get_mut(&id) { st.halt(); }
                }
                TouchEffect::StopAcross(id) => {
                    if let Some(st) = self.scroll.scrolls.get_mut(&id) && matches!(st.x.motion, XMotion::Momentum(_) | XMotion::Wheel(_)) { st.x.motion = XMotion::Rest; }
                }
                TouchEffect::Fling { id, horizontal, velocity } => {
                    let Some(st) = self.scroll.scrolls.get_mut(&id) else { continue };
                    let v = f64::from(velocity.clamp(-8000.0, 8000.0));
                    if horizontal { st.x.set_coast(st.x.toward(v) as f32) } else { st.mode = st.mode.to_momentum(-v as f32) }
                }
                TouchEffect::Pan { id, origin, horizontal, d } => {
                    let Some(st) = self.scroll.scrolls.get_mut(&id) else { continue };
                    let toward = if horizontal { st.x.toward(f64::from(d)) } else { -f64::from(d) };
                    let (offset, max) = st.axis(horizontal);
                    let before = *offset;
                    let to = origin + toward;
                    let edge = to.clamp(0.0, max);
                    let over = to - edge;
                    *offset = edge + over * 0.55 / (1.0 + over.abs() * 0.55 / 300.0);
                    let moved = (*offset - before).abs() > 1e-6;
                    if !horizontal { st.mode = st.mode.to_rest(); }
                    st.moved();
                    if moved { st.note_source(crate::ScrollSource::Direct); }
                    self.pointer.scroll_cancels_press(&self.tree.nodes, id);
                }
                TouchEffect::Hold { id, horizontal, overflow } => {
                    let Some(st) = self.scroll.scrolls.get_mut(&id) else { continue };
                    if horizontal { st.x.max = overflow.max(0.0); } else { st.max = overflow.max(0.0); }
                    let _ = mode::step(&mut st.mode, ModeEvent::Hold);
                    active = Some(false);
                }
                TouchEffect::Coast { id, horizontal, overflow, dt } => {
                    let moving = {
                        let Some(st) = self.scroll.scrolls.get_mut(&id) else { continue };
                        if horizontal { st.x.max = overflow.max(0.0); } else { st.max = overflow.max(0.0); }
                        let mut v = if horizontal { st.x.coast() } else { st.coast() };
                        let v0 = v;
                        let (offset, max) = st.axis(horizontal);
                        let before = *offset;
                        let edge = offset.clamp(0.0, max);
                        let source = if *offset != edge {
                            (*offset, v) = rebound(*offset, v, edge, dt);
                            if (*offset - edge).abs() < 0.1 && v.abs() < 1.0 { *offset = edge; v = 0.0; }
                            if dt > 0.0 || (*offset - before).abs() > 1e-6 { crate::ScrollSource::EdgeReturn } else { crate::ScrollSource::None }
                        } else {
                            let decay = (-SCROLL_DECAY * dt).exp();
                            *offset += f64::from(v * (1.0 - decay) / SCROLL_DECAY);
                            v *= decay;
                            if (*offset - before).abs() > 1e-6 { crate::ScrollSource::Momentum } else { crate::ScrollSource::None }
                        };
                        let moving = v.abs() > 1.0 || *offset < 0.0 || *offset > max;
                        let _ = mode::step(&mut st.mode, ModeEvent::Coast { moving: v0.abs() > 1.0, source });
                        if horizontal { st.x.set_coast(v); } else if v != 0.0 { st.mode = st.mode.to_momentum(v); } else { st.halt(); }
                        moving
                    };
                    self.step_touch(TouchEvent::Settled { moving });
                    active = Some(moving);
                }
            }
        }
        active
    }
}

#[cfg(test)]
#[path = "touch_tests.rs"]
mod tests;
