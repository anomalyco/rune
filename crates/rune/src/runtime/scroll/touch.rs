//! Touch scrolling: a finger pans the scroll viewport under it, rubber-bands past its ends,
//! and flings it on with momentum, on every platform that reports touches (`runtime/input/touch.rs`
//! turns touches into presses, drags and these pans).
use super::*;
use crate::machine::Effects;

/// What a touch holds: the viewport its finger went down on, one it pans, or one it let go
/// that is still settling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum TouchScroll {
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
pub(super) enum TouchEvent {
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
pub(super) enum TouchEffect {
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
pub(super) fn step(state: &mut Option<TouchScroll>, event: TouchEvent) -> Effects<TouchEffect> {
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

impl<S: 'static> Runtime<S> {
    /// A touch went down: it stops the momentum of the viewports under it and holds the
    /// vertical one until the pan picks its axis (`touch_scroll_pan`). A horizontal one still
    /// settling from an earlier touch (momentum or its edge return) holds where the finger
    /// caught it instead.
    pub(super) fn touch_scroll_start(&mut self, x: f32, y: f32) -> Option<NodeId> {
        let across = self.scroll_at_axis(x, y, true).and_then(|id| self.scrolls.get(&id).map(|st| (id, st.x.offset)));
        let vertical = self.scroll_at(x, y);
        let held = vertical.and_then(|id| self.scrolls.get(&id).map(|st| (id, st.offset)));
        self.step_touch(TouchEvent::Down { across, vertical: held });
        vertical
    }

    /// The touch that went down at (x, y) started panning, mostly horizontally or not: a
    /// horizontal pan takes the innermost horizontal viewport there, and any other pan stays
    /// with the vertical one. Returns the viewport and whether it moves horizontally.
    pub(super) fn touch_scroll_pan(&mut self, x: f32, y: f32, horizontal: bool, vertical: Option<NodeId>) -> (Option<NodeId>, bool) {
        let across = horizontal
            .then(|| self.scroll_at_axis(x, y, true))
            .flatten()
            .and_then(|id| self.scrolls.get(&id).map(|st| (id, st.x.offset)));
        let held = vertical.and_then(|id| self.scrolls.get(&id).map(|st| (id, st.offset)));
        self.step_touch(TouchEvent::Pan { horizontal, across, vertical: held });
        match across {
            Some((id, _)) => (Some(id), true),
            None => (vertical, false),
        }
    }

    /// The pan moved `d` points along its axis from where the touch went down.
    pub(super) fn touch_scroll_move(&mut self, id: NodeId, d: f32) {
        self.step_touch(TouchEvent::Move { id, d });
    }

    /// The touch lifted from viewport `id` (None when it held none) with `velocity` along its
    /// axis. Whatever it held lets go, a viewport it caught without panning included.
    pub(super) fn touch_scroll_end(&mut self, id: Option<NodeId>, velocity: f32) {
        self.step_touch(TouchEvent::Up { id, velocity });
    }

    /// Own the touched viewport's panned axis through momentum and its edge return. Other
    /// viewports use the native wheel path, including native clamping and stick-to-bottom.
    pub(super) fn step_touch_scroll(&mut self, id: NodeId, overflow: f64, dt: f32, horizontal: bool) -> Option<bool> {
        let effects = step(&mut self.touch.scroll, TouchEvent::Tick { id, horizontal, overflow, dt });
        self.apply_touch(effects)
    }

    /// Step the pan and carry out what the transition asked for.
    fn step_touch(&mut self, event: TouchEvent) {
        let effects = step(&mut self.touch.scroll, event);
        let _ = self.apply_touch(effects);
    }

    /// Carry out what a touch transition asked for on the scroll viewports: the drag's and the
    /// coast's math lives here, and a coast reports whether its viewport is still moving.
    fn apply_touch(&mut self, effects: Effects<TouchEffect>) -> Option<bool> {
        let mut active = None;
        for effect in effects {
            match effect {
                TouchEffect::Halt(id) => {
                    if let Some(st) = self.scrolls.get_mut(&id) { st.halt(); }
                }
                TouchEffect::StopAcross(id) => {
                    if let Some(st) = self.scrolls.get_mut(&id) { st.x.v = 0.0; }
                }
                TouchEffect::Fling { id, horizontal, velocity } => {
                    let Some(st) = self.scrolls.get_mut(&id) else { continue };
                    let v = f64::from(velocity.clamp(-8000.0, 8000.0));
                    if horizontal { st.x.v = st.x.toward(v) as f32 } else { st.mode = ScrollMode::Momentum { v: -v as f32 } }
                }
                TouchEffect::Pan { id, origin, horizontal, d } => {
                    let Some(st) = self.scrolls.get_mut(&id) else { continue };
                    let toward = if horizontal { st.x.toward(f64::from(d)) } else { -f64::from(d) };
                    let (offset, max) = st.axis(horizontal);
                    let to = origin + toward;
                    let edge = to.clamp(0.0, max);
                    let over = to - edge;
                    *offset = edge + over * 0.55 / (1.0 + over.abs() * 0.55 / 300.0);
                    if !horizontal { st.mode = ScrollMode::Rest; }
                    st.direct = true;
                    self.scroll_cancels_press(id);
                }
                TouchEffect::Hold { id, horizontal, overflow } => {
                    let Some(st) = self.scrolls.get_mut(&id) else { continue };
                    if horizontal { st.x.max = overflow.max(0.0); } else { st.max = overflow.max(0.0); }
                    st.scrolling = true;
                    st.by_hand = true;
                    active = Some(false);
                }
                TouchEffect::Coast { id, horizontal, overflow, dt } => {
                    let moving = {
                        let Some(st) = self.scrolls.get_mut(&id) else { continue };
                        if horizontal { st.x.max = overflow.max(0.0); } else { st.max = overflow.max(0.0); }
                        let mut v = if horizontal { st.x.v } else { st.coast() };
                        st.scrolling = v.abs() > 1.0;
                        st.by_hand |= st.scrolling;
                        st.direct = false;
                        let (offset, max) = st.axis(horizontal);
                        let edge = offset.clamp(0.0, max);
                        if *offset != edge {
                            (*offset, v) = rebound(*offset, v, edge, dt);
                            if (*offset - edge).abs() < 0.1 && v.abs() < 1.0 { *offset = edge; v = 0.0; }
                        } else {
                            let decay = (-SCROLL_DECAY * dt).exp();
                            *offset += f64::from(v * (1.0 - decay) / SCROLL_DECAY);
                            v *= decay;
                        }
                        let moving = v.abs() > 1.0 || *offset < 0.0 || *offset > max;
                        if horizontal { st.x.v = v; } else if v != 0.0 { st.mode = ScrollMode::Momentum { v }; } else { st.halt(); }
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
mod tests {
    use super::*;

    #[test]
    fn a_touch_pan_moves_the_axis_its_first_direction_picks() {
        fn view(_: &(), _: &mut Cx<()>) -> El {
            let tiles = (0..10).fold(crate::row(), |r, i| r.child(crate::el().key(format!("tile{i}")).w(100.0).h(100.0)));
            let rows = (0..20).fold(crate::col().w_full().child(crate::col().key("strip").w_full().h(100.0).scroll_x().child(tiles)),
                |c, i| c.child(crate::el().key(format!("row{i}")).w_full().h(40.0)));
            crate::col().w_full().h_full().child(crate::col().key("page").w_full().h(300.0)
                .scroll(crate::ScrollSpec { stick_bottom: false }).child(rows))
        }
        let mut rt = Runtime::new((), view, (300.0, 300.0), Env::default());
        rt.frame(0.0);
        let id = |rt: &Runtime<()>, key: &str| rt.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap().id;
        let (page, strip) = (id(&rt, "page"), id(&rt, "strip"));

        let vertical = rt.touch_scroll_start(150.0, 50.0);
        assert_eq!(vertical, Some(page));
        assert_eq!(rt.touch_scroll_pan(150.0, 50.0, true, vertical), (Some(strip), true));
        rt.touch_scroll_move(strip, -50.0);
        assert_eq!((rt.scrolls[&strip].x.offset, rt.scrolls[&page].offset), (50.0, 0.0));
        rt.touch_scroll_end(Some(strip), 0.0);

        // A vertical pan over the strip passes through to the page.
        let vertical = rt.touch_scroll_start(150.0, 50.0);
        assert_eq!(rt.touch_scroll_pan(150.0, 50.0, false, vertical), (Some(page), false));
        rt.touch_scroll_move(page, -30.0);
        assert_eq!((rt.scrolls[&strip].x.offset, rt.scrolls[&page].offset), (50.0, 30.0));
        rt.touch_scroll_end(Some(page), 0.0);

        // A touch on the strip while it springs back past its left edge holds it there, until a
        // tap lets it go or a sideways pan carries on from where it was caught.
        let held = |rt: &Runtime<()>| rt.touch.scroll.map(|t| (t.id(), t.horizontal(), !t.coasting(), t.origin()));
        rt.scrolls.get_mut(&strip).unwrap().x.offset = -20.0;
        rt.touch.scroll = Some(TouchScroll::Coasting { id: strip, horizontal: true });
        let vertical = rt.touch_scroll_start(150.0, 50.0);
        assert_eq!(held(&rt), Some((strip, true, true, Some(-20.0))), "caught, not handed to the page");
        rt.touch_scroll_end(vertical, 0.0);
        assert_eq!(held(&rt), Some((strip, true, false, None)), "a tap lets it go back");
        let vertical = rt.touch_scroll_start(150.0, 50.0);
        assert_eq!(rt.touch_scroll_pan(150.0, 50.0, true, vertical), (Some(strip), true));
        rt.touch_scroll_move(strip, 10.0);
        assert!(rt.scrolls[&strip].x.offset < -20.0 * 0.55, "still rubber-banding: {}", rt.scrolls[&strip].x.offset);
        rt.touch_scroll_end(Some(strip), 0.0);

        // Caught there, a vertical pan lets the strip go and moves the page.
        rt.touch_scroll_start(150.0, 50.0);
        assert_eq!(rt.touch_scroll_pan(150.0, 50.0, false, vertical), (Some(page), false));
        assert_eq!(held(&rt).map(|h| (h.0, h.1, h.2)), Some((page, false, true)));
    }

    #[test]
    fn edge_return_is_frame_rate_independent() {
        let one = rebound(-50.0, -400.0, 0.0, 0.5);
        let mut many = (-50.0, -400.0);
        for _ in 0..60 { many = rebound(many.0, many.1, 0.0, 0.5 / 60.0); }
        assert!((one.0 - many.0).abs() < 0.001);
        assert!((one.1 - many.1).abs() < 0.01);
        assert!(one.0 < 0.0 && one.0 > -1.0);
    }

    fn run(state: &mut Option<TouchScroll>, event: TouchEvent) -> Vec<TouchEffect> {
        step(state, event).into_iter().collect()
    }

    #[test]
    fn a_down_holds_the_viewport_under_it_and_stops_its_motion() {
        let (page, strip) = (NodeId(1), NodeId(2));
        let mut state = None;
        let effects = run(&mut state, TouchEvent::Down { across: Some((strip, 5.0)), vertical: Some((page, 7.0)) });
        assert_eq!(state, Some(TouchScroll::Held { id: page, origin: 7.0, horizontal: false }));
        assert_eq!(effects, vec![TouchEffect::StopAcross(strip), TouchEffect::Halt(page)]);

        // A horizontal viewport still settling is caught where the finger lands on it.
        let mut state = Some(TouchScroll::Coasting { id: strip, horizontal: true });
        run(&mut state, TouchEvent::Down { across: Some((strip, -20.0)), vertical: Some((page, 7.0)) });
        assert_eq!(state, Some(TouchScroll::Held { id: strip, origin: -20.0, horizontal: true }), "caught, not handed to the page");

        // A touch outside every viewport leaves the settling one alone.
        let mut state = Some(TouchScroll::Coasting { id: strip, horizontal: false });
        run(&mut state, TouchEvent::Down { across: None, vertical: None });
        assert_eq!(state, Some(TouchScroll::Coasting { id: strip, horizontal: false }));
    }

    #[test]
    fn a_pan_turns_a_held_viewport_into_a_drag_with_the_offset_it_was_caught_at() {
        let (page, strip) = (NodeId(1), NodeId(2));
        let mut state = Some(TouchScroll::Held { id: page, origin: 7.0, horizontal: false });
        run(&mut state, TouchEvent::Pan { horizontal: false, across: None, vertical: Some((page, 9.0)) });
        assert_eq!(state, Some(TouchScroll::Dragging { id: page, origin: 7.0, horizontal: false }), "keeps the offset the finger went down at");

        // A horizontal pan takes the horizontal viewport under the finger, from its offset now.
        let mut state = Some(TouchScroll::Held { id: page, origin: 7.0, horizontal: false });
        run(&mut state, TouchEvent::Pan { horizontal: true, across: Some((strip, 4.0)), vertical: Some((page, 9.0)) });
        assert_eq!(state, Some(TouchScroll::Dragging { id: strip, origin: 4.0, horizontal: true }));

        // A finger down on a horizontal viewport with no vertical one still pans it.
        let mut state = None;
        run(&mut state, TouchEvent::Pan { horizontal: true, across: Some((strip, 4.0)), vertical: None });
        assert_eq!(state, Some(TouchScroll::Dragging { id: strip, origin: 4.0, horizontal: true }));

        // A vertical pan lets a caught horizontal viewport go to the vertical one.
        let mut state = Some(TouchScroll::Held { id: strip, origin: -20.0, horizontal: true });
        run(&mut state, TouchEvent::Pan { horizontal: false, across: Some((strip, -20.0)), vertical: Some((page, 9.0)) });
        assert_eq!(state, Some(TouchScroll::Dragging { id: page, origin: 9.0, horizontal: false }));
        // With no vertical viewport under it, the horizontal one is left settling.
        let mut state = Some(TouchScroll::Held { id: strip, origin: -20.0, horizontal: true });
        run(&mut state, TouchEvent::Pan { horizontal: false, across: None, vertical: None });
        assert_eq!(state, Some(TouchScroll::Coasting { id: strip, horizontal: true }));
    }

    #[test]
    fn a_lift_starts_the_coast_and_flings_only_the_viewport_it_held() {
        let (page, strip) = (NodeId(1), NodeId(2));
        let mut state = Some(TouchScroll::Dragging { id: page, origin: 0.0, horizontal: false });
        let effects = run(&mut state, TouchEvent::Up { id: Some(page), velocity: -1200.0 });
        assert_eq!(state, Some(TouchScroll::Coasting { id: page, horizontal: false }));
        assert_eq!(effects, vec![TouchEffect::Fling { id: page, horizontal: false, velocity: -1200.0 }]);

        // A lift from another viewport still lets this one go, without a fling.
        let mut state = Some(TouchScroll::Dragging { id: page, origin: 0.0, horizontal: false });
        assert_eq!(run(&mut state, TouchEvent::Up { id: Some(strip), velocity: 0.0 }), vec![]);
        assert_eq!(state, Some(TouchScroll::Coasting { id: page, horizontal: false }));
    }

    #[test]
    fn a_move_and_a_frame_tick_go_through_step() {
        let page = NodeId(1);
        let mut state = Some(TouchScroll::Dragging { id: page, origin: 5.0, horizontal: false });
        assert_eq!(
            run(&mut state, TouchEvent::Move { id: page, d: -30.0 }),
            vec![TouchEffect::Pan { id: page, origin: 5.0, horizontal: false, d: -30.0 }]
        );
        assert_eq!(
            run(&mut state, TouchEvent::Tick { id: page, horizontal: false, overflow: 40.0, dt: 1.0 / 60.0 }),
            vec![TouchEffect::Hold { id: page, horizontal: false, overflow: 40.0 }],
            "a held viewport only has its bounds set"
        );
        let mut state = Some(TouchScroll::Coasting { id: page, horizontal: false });
        assert_eq!(
            run(&mut state, TouchEvent::Tick { id: page, horizontal: false, overflow: 40.0, dt: 1.0 / 60.0 }),
            vec![TouchEffect::Coast { id: page, horizontal: false, overflow: 40.0, dt: 1.0 / 60.0 }],
            "a coasting one is stepped"
        );

        // A move of a viewport the finger no longer drags, or a tick for another axis, asks for nothing.
        let mut state = Some(TouchScroll::Held { id: page, origin: 5.0, horizontal: false });
        assert_eq!(run(&mut state, TouchEvent::Move { id: page, d: -30.0 }), vec![]);
        assert_eq!(run(&mut state, TouchEvent::Tick { id: page, horizontal: true, overflow: 40.0, dt: 0.1 }), vec![]);
    }

    #[test]
    fn a_coast_reaches_rest_and_nothing_else_is_cleared() {
        let page = NodeId(1);
        let mut state = Some(TouchScroll::Coasting { id: page, horizontal: false });
        run(&mut state, TouchEvent::Settled { moving: true });
        assert_eq!(state, Some(TouchScroll::Coasting { id: page, horizontal: false }), "still moving");
        run(&mut state, TouchEvent::Settled { moving: false });
        assert_eq!(state, None, "came to rest");

        let mut state = Some(TouchScroll::Dragging { id: page, origin: 0.0, horizontal: false });
        run(&mut state, TouchEvent::Settled { moving: false });
        assert_eq!(state, Some(TouchScroll::Dragging { id: page, origin: 0.0, horizontal: false }), "a drag never settles itself");
    }
}
