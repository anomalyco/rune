//! Touchscreen and pen input, as a browser handles it: the first finger is the primary
//! button. It presses where it lands, so a tap clicks; dragging an element that drags
//! (`El::on_drag`: a slider, a scroll bar) drags it; any other drag pans the scroll viewport
//! under the finger (`runtime/scroll/touch.rs`), flinging it on with momentum, and cancels
//! the press. A finger held still for [`LONG_PRESS`] is a secondary click there, as Chromium's
//! long press sends `contextmenu`: the press it began lets go without a click and the menu opens
//! under the finger, which then lifts on nothing. When a second finger lands with an
//! `El::on_magnify` handler under the fingers' midpoint, the first finger's pending press,
//! hold, drag or pan cancels cleanly without a click or fling, and two-finger pinches route to
//! `on_magnify` through [`touch_machine::step_touch`]; otherwise further fingers are ignored.
use crate::machine::{Attempt, Effects, Seq};

use super::touch_machine::{
    Finger, Mode, SlopResolution, TouchEffect, TouchInput, TouchState, TwoFingerTargets, step_touch,
};
use super::*;
/// A finger held still this long (seconds) opens the menu under it (Chromium's
/// `long_press_time_in_ms`).
const LONG_PRESS: f64 = 0.5;
/// Fallback hold deadline (seconds) behind the browser's live `contextmenu` event under
/// `TouchHoldPolicy::HostContextMenu`: clears Android's longest Touch & Hold accessibility delay
/// (1500 ms + 100 ms tap timeout) while still guaranteeing a long press if a browser never sends
/// `contextmenu` (spoofed UA, embedded WebView, or kiosk).
const HOST_HOLD_FALLBACK: f64 = 1.7;

/// A host's touch phase, as winit reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchPhase {
    Started,
    Moved,
    Ended,
    /// The system took the touch (a gesture of its own): nothing it pressed is clicked.
    Cancelled,
}

/// How a touch hold timer turns into a secondary press.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
pub(crate) enum TouchHoldPolicy {
    /// Fire the secondary press directly when `clock >= due` (native hosts and headless default).
    #[default]
    DirectTimer,
    /// When `clock >= due`, wait one low-priority host turn (`TouchHoldCommand::Confirm`) so any
    /// queued release or move drains before confirming the hold (web iOS/iPadOS, Windows, macOS).
    ConfirmingTimer,
    /// Trust the browser's live `contextmenu` event (`HoldEvent::HostHold`) while arming a
    /// `HOST_HOLD_FALLBACK` confirming timer behind it so a wrong platform guess still gets a
    /// long press and no hold ever fires twice (web Android, Linux, ChromeOS).
    HostContextMenu,
}

/// Host-side command emitted when a web touch hold enters or leaves confirmation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TouchHoldCommand {
    /// Schedule a low-priority confirmation turn for `attempt`.
    Confirm(Attempt),
    /// Cancel any pending confirmation turn.
    Cancel,
}

/// Long-press state for the active finger, stepped by [`step_hold`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(in crate::runtime) enum HoldState {
    /// No hold timer is armed (no finger down, or the finger moved to drag/pan/window-drag).
    #[default]
    Idle,
    /// Finger is still within `TOUCH_SLOP`; hold is due at `due` on the runtime clock.
    Armed { attempt: Attempt, due: f64 },
    /// Under `ConfirmingTimer`, `due` elapsed and the host is draining queued input on a
    /// low-priority turn before confirming `attempt`.
    Confirming { attempt: Attempt },
    /// A long press fired for this finger.
    Fired,
}

impl HoldState {
    /// Next wake deadline on the runtime clock while a hold timer is armed; `None` otherwise.
    #[inline]
    pub(in crate::runtime) fn deadline(self) -> Option<f64> {
        match self {
            HoldState::Armed { due, .. } => Some(due),
            HoldState::Idle | HoldState::Confirming { .. } | HoldState::Fired => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::runtime) enum HoldEvent {
    /// A hold-eligible touch began at runtime clock `now` with event timestamp `event_time`.
    Start { now: f64, event_time: f64, policy: TouchHoldPolicy },
    /// A frame ticked at runtime clock `now`.
    Tick { now: f64, policy: TouchHoldPolicy },
    /// The host's low-priority confirmation turn ran for `attempt`.
    Confirm { attempt: Attempt },
    /// The browser dispatched a live touch `contextmenu` event for the active finger.
    HostHold,
    /// The finger moved past `TOUCH_SLOP`, lifted, or was cancelled.
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::runtime) enum HoldEffect {
    /// Schedule a wake at `due` on the runtime clock.
    WakeAt(f64),
    /// Ask the host to yield a low-priority turn and reply with `HoldEvent::Confirm { attempt }`.
    ConfirmHold(Attempt),
    /// Cancel any pending host confirmation turn.
    CancelConfirm,
    /// Fire the secondary press now.
    Fire,
}

/// Hold timeout duration for `policy` (`LONG_PRESS` for direct/confirming timers,
/// `HOST_HOLD_FALLBACK` behind the browser's live `contextmenu` under `HostContextMenu`).
#[inline]
pub(in crate::runtime) fn hold_duration(policy: TouchHoldPolicy) -> f64 {
    match policy {
        TouchHoldPolicy::DirectTimer | TouchHoldPolicy::ConfirmingTimer => LONG_PRESS,
        TouchHoldPolicy::HostContextMenu => HOST_HOLD_FALLBACK,
    }
}

/// Computes the hold deadline on the runtime clock. Under web policies (`ConfirmingTimer` and
/// `HostContextMenu`), subtracts `event_processing_delay = now - event_time` only when
/// `0 < delay < timeout`, matching Chromium's `TimeoutGestureHandler::Start`
/// (`kCompensateGestureDetectorTimeouts` with `kCompensateGestureTimeoutsForLongDelayedSequences = false`):
/// a malformed/future timestamp or a press delayed by `>= timeout` arms a full `timeout` instead
/// of firing immediately.
pub(in crate::runtime) fn compensated_hold_due(now: f64, event_time: f64, policy: TouchHoldPolicy) -> f64 {
    let timeout = hold_duration(policy);
    if policy == TouchHoldPolicy::DirectTimer || !event_time.is_finite() {
        return now + timeout;
    }
    let delay = now - event_time;
    if delay > 0.0 && delay < timeout {
        now + (timeout - delay)
    } else {
        now + timeout
    }
}

/// Pure transition function for touch long-press recognition.
pub(in crate::runtime) fn step_hold(seq: &mut Seq, state: &mut HoldState, event: HoldEvent) -> Effects<HoldEffect> {
    match event {
        HoldEvent::Start { now, event_time, policy } => {
            let cancel = matches!(*state, HoldState::Confirming { .. });
            let attempt = seq.next();
            let due = compensated_hold_due(now, event_time, policy);
            *state = HoldState::Armed { attempt, due };
            if cancel {
                Effects::two(HoldEffect::CancelConfirm, HoldEffect::WakeAt(due))
            } else {
                Effects::one(HoldEffect::WakeAt(due))
            }
        }
        HoldEvent::Tick { now, policy } => match *state {
            HoldState::Armed { attempt, due } if now >= due => match policy {
                TouchHoldPolicy::DirectTimer => {
                    *state = HoldState::Fired;
                    Effects::one(HoldEffect::Fire)
                }
                TouchHoldPolicy::ConfirmingTimer | TouchHoldPolicy::HostContextMenu => {
                    *state = HoldState::Confirming { attempt };
                    Effects::one(HoldEffect::ConfirmHold(attempt))
                }
            },
            _ => Effects::none(),
        },
        HoldEvent::Confirm { attempt } => match *state {
            HoldState::Confirming { attempt: expected } if expected == attempt => {
                *state = HoldState::Fired;
                Effects::one(HoldEffect::Fire)
            }
            _ => Effects::none(),
        },
        HoldEvent::HostHold => match *state {
            HoldState::Armed { .. } => {
                *state = HoldState::Fired;
                Effects::one(HoldEffect::Fire)
            }
            HoldState::Confirming { .. } => {
                *state = HoldState::Fired;
                Effects::two(HoldEffect::CancelConfirm, HoldEffect::Fire)
            }
            HoldState::Idle | HoldState::Fired => Effects::none(),
        },
        HoldEvent::Cancel => {
            let cancel = matches!(*state, HoldState::Confirming { .. });
            *state = HoldState::Idle;
            if cancel {
                Effects::one(HoldEffect::CancelConfirm)
            } else {
                Effects::none()
            }
        }
    }
}

/// What touches are doing: the touch contact state machine, its hold timer sub-machine, and
/// the viewport a touch holds or flung.
#[derive(Default)]
pub(in crate::runtime) struct Touch {
    pub(in crate::runtime) scroll: Option<scroll_touch::TouchScroll>,
    state: TouchState,
    /// True while inside `touch_down` or `touch_up`, so synthetic pointer moves/leaves emitted
    /// by touch handling never start a mouse link hover.
    in_touch: bool,
    hold_seq: Seq,
    hold: HoldState,
    policy: TouchHoldPolicy,
    hold_command: Option<TouchHoldCommand>,
    /// [`Drag::data`] kept across an `El::on_edge_drag` gesture's phases.
    pub(in crate::runtime) edge_data: f32,
    /// Active `El::on_edge_drag` target and fallback handler, kept in lockstep with `Mode::Edge`.
    pub(in crate::runtime) active_edge: Option<edge_drag::ActiveEdge>,
    /// Fallback handlers for cancelled `Action::EdgeDrag` `Phase::Up` events whose element left.
    edge_gone: Vec<(String, crate::el::DragFn)>,
}

impl Touch {
    /// Next wake deadline on the runtime clock for an armed touch hold, or `None` when idle.
    #[inline]
    pub(in crate::runtime) fn hold_deadline(&self) -> Option<f64> {
        self.hold.deadline()
    }

    #[inline]
    pub(super) fn state_mut(&mut self) -> &mut TouchState {
        &mut self.state
    }

    #[inline]
    pub(in crate::runtime) fn push_edge_gone(&mut self, path: String, f: crate::el::DragFn) {
        self.edge_gone.push((path, f));
    }

    #[inline]
    pub(in crate::runtime) fn take_edge_gone(&mut self, path: &str) -> Option<crate::el::DragFn> {
        self.edge_gone.iter().position(|g| g.0 == path).map(|i| self.edge_gone.remove(i).1)
    }

    pub(in crate::runtime) fn clear_edge_state(&mut self) {
        self.active_edge = None;
        self.edge_gone.clear();
    }

    #[cfg(test)]
    #[inline]
    pub(super) fn single(&self) -> Option<&Finger> {
        self.state.single()
    }

    #[cfg(test)]
    #[inline]
    pub(super) fn state(&self) -> TouchState {
        self.state
    }
}

impl capture::PointerState {
    /// Whether any tracked finger is down: a host ignores the mouse events its system makes up from it.
    #[inline]
    pub(in crate::runtime) fn touching(&self) -> bool {
        self.touch.state.is_active()
    }

    /// Whether a touch finger is active or a touch down/up event is being processed now.
    #[inline]
    pub(in crate::runtime) fn is_touch_input(&self) -> bool {
        self.touch.state.is_active() || self.touch.in_touch
    }

    /// Next wake deadline on the runtime clock for an armed touch hold, or `None` when idle.
    #[inline]
    pub(in crate::runtime) fn touch_hold_deadline(&self) -> Option<f64> {
        self.touch.hold_deadline()
    }
}

impl RuntimeCore {
    /// Configures how touch holds turn into secondary presses (the web host selects
    /// `ConfirmingTimer`; native and headless keep `DirectTimer`).
    #[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
    #[inline]
    pub(crate) fn set_touch_hold_policy(&mut self, policy: TouchHoldPolicy) {
        self.pointer.touch.policy = policy;
    }

    /// Takes the latest host-side confirmation command produced by the touch hold state machine.
    #[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
    #[inline]
    pub(crate) fn take_touch_hold_command(&mut self) -> Option<TouchHoldCommand> {
        self.pointer.touch.hold_command.take()
    }

    /// One touch event at (x, y) in window points, `time` in seconds on any steady clock (on
    /// Web, mapped onto the runtime clock from `PointerEvent.timeStamp`). A host with touches
    /// calls this for every one; `id` tells fingers apart.
    pub(crate) fn touch(&mut self, app: &mut dyn AppDriver, id: u64, phase: TouchPhase, x: f32, y: f32, time: f64) {
        match phase {
            TouchPhase::Started => self.touch_down(app, id, x, y, time),
            TouchPhase::Moved => self.touch_move(app, id, x, y, time),
            TouchPhase::Ended | TouchPhase::Cancelled => self.touch_up(app, id, x, y, time, phase == TouchPhase::Cancelled),
        }
    }

    /// Whether a finger is down: a host ignores the mouse events its system makes up from it.
    #[inline]
    pub(crate) fn touching(&self) -> bool {
        self.pointer.touching()
    }

    /// Whether the active touch already entered `Mode::Menu` (long press), so lifting it does
    /// nothing and must not activate DOM focus.
    #[inline]
    #[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
    pub(crate) fn touch_in_menu(&self) -> bool {
        self.pointer.touch.state.in_menu()
    }

    fn apply_hold_effects(&mut self, effects: Effects<HoldEffect>) -> bool {
        let mut fire = false;
        for effect in effects {
            match effect {
                // `hold_deadline()` exposes `HoldState::Armed` directly to `wake_deadline()`,
                // so cancelling or lifting a finger leaves no stale wake behind.
                HoldEffect::WakeAt(_) => {}
                HoldEffect::ConfirmHold(attempt) => {
                    self.pointer.touch.hold_command = Some(TouchHoldCommand::Confirm(attempt));
                }
                HoldEffect::CancelConfirm => {
                    self.pointer.touch.hold_command = Some(TouchHoldCommand::Cancel);
                }
                HoldEffect::Fire => fire = true,
            }
        }
        fire
    }

    /// Cancels any armed hold timer or pending host confirmation without dropping an active
    /// `Mode::Menu` touch whose finger is still down.
    pub(in crate::runtime) fn cancel_touch_hold(&mut self) {
        let effects = step_hold(&mut self.pointer.touch.hold_seq, &mut self.pointer.touch.hold, HoldEvent::Cancel);
        let _ = self.apply_hold_effects(effects);
    }

    fn touch_down(&mut self, app: &mut dyn AppDriver, id: u64, x: f32, y: f32, time: f64) {
        self.pointer.touch.in_touch = true;
        self.touch_down_inner(app, id, x, y, time);
        self.pointer.touch.in_touch = false;
    }

    /// Queries which two-finger gesture channels have a live, allowed handler (`on_wheel`,
    /// `on_magnify`, or `on_rotate`) under window point `mid`. Plain scroll viewports without an
    /// `on_wheel` handler do not match, keeping two-finger touches ignored over scroll lists.
    fn two_finger_targets(&self, mid: (f32, f32)) -> TwoFingerTargets {
        let mut targets = TwoFingerTargets::default();
        let mut at = self.chain_start(mid.0, mid.1);
        while let Some(i) = at {
            if self.subtree_exiting(i) || !self.allowed(i) {
                break;
            }
            if let Some(g) = self.tree.nodes[i].handlers.gestures.as_deref() {
                targets.wheel |= g.wheel.is_some();
                targets.magnify |= g.magnify.is_some();
                targets.rotate |= g.rotate.is_some();
                if targets.wheel && targets.magnify && targets.rotate {
                    break;
                }
            }
            at = self.tree.nodes[i].parent;
        }
        targets
    }

    fn apply_touch_effects(&mut self, app: &dyn ViewDriver, effects: Effects<TouchEffect>) {
        for effect in effects {
            match effect {
                TouchEffect::CancelHold => self.cancel_touch_hold(),
                TouchEffect::StartWindowDrag => {
                    self.host.titlebar.clear();
                    self.host.titlebar.action = Some(TitlebarAction::Drag);
                }
                TouchEffect::ClearTitlebar => self.host.titlebar.clear(),
                TouchEffect::BeginPan { origin, horizontal, vertical } => {
                    let _ = self.touch_scroll_pan(origin.0, origin.1, horizontal, vertical);
                }
                TouchEffect::PanMove { scroll, along } => self.touch_scroll_move(scroll, along),
                TouchEffect::EndScroll { scroll, velocity } => self.touch_scroll_end(scroll, velocity),
                TouchEffect::PointerMove((x, y)) => self.pointer_move(app, x, y),
                TouchEffect::PointerUp((x, y)) => self.pointer_up(app, x, y),
                TouchEffect::PointerLeave => self.pointer_leave(),
                TouchEffect::ReleaseWithoutClick => {
                    self.pointer.note_button_up(crate::PointerButton::Primary);
                    self.pointer_leave();
                }
                TouchEffect::CancelPointer => self.cancel_pointer(),
                TouchEffect::EdgeDrag { node, edge, phase, origin, pos, cancelled } => {
                    self.queue_edge_drag(node, edge, phase, origin, pos, cancelled);
                }
                TouchEffect::CommitHold { origin: (x, y) } => {
                    if !self.cancel_drag() { self.drop_press(); }
                    self.secondary_down(x, y);
                    self.secondary_up(app, x, y);
                }
                TouchEffect::EmitTouchWheel { dx, dy, mid, hit, phase } => {
                    self.touch_wheel_event_at(
                        crate::WheelEvent::new(dx, dy).at(mid.0, mid.1).phase(phase).precise(true).touch(true),
                        hit,
                    );
                }
                TouchEffect::EmitMagnify { delta, mid, hit, phase } => {
                    self.magnify_event_at(
                        crate::MagnifyEvent::new(delta).at(mid.0, mid.1).phase(phase),
                        hit,
                    );
                }
                TouchEffect::EmitRotate { degrees, mid, hit, phase } => {
                    self.rotate_event_at(
                        crate::RotateEvent::new(degrees).at(mid.0, mid.1).phase(phase),
                        hit,
                    );
                }
            }
        }
    }

    fn touch_down_inner(&mut self, app: &mut dyn AppDriver, id: u64, x: f32, y: f32, time: f64) {
        if self.pointer.touch.state.is_active() {
            if let Some(mid) = self.pointer.touch.state.second_down_midpoint(id, (x, y)) {
                let targets = self.two_finger_targets(mid);
                let effects = step_touch(
                    &mut self.pointer.touch.state,
                    TouchInput::SecondDown { id, pos: (x, y), targets },
                );
                self.apply_touch_effects(app, effects);
            }
            return;
        }
        let edge = self.edge_drag_candidates(x, y);
        let has_edge = edge_drag::any_candidate(edge);
        let scroll = self.touch_scroll_start(x, y);
        self.pointer_move(app, x, y);
        self.press_down(app, x, y, true);
        let now = self.now_clock();
        let policy = self.pointer.touch.policy;
        let (mode, start_hold) = if self.host.titlebar.resize.is_some() {
            self.touch_scroll_end(scroll, 0.0);
            (Mode::WindowDragging, false)
        } else {
            match self.host.titlebar.action {
                Some(TitlebarAction::Drag) => {
                    // Hold the drag until the finger crosses `TOUCH_SLOP`, so taps, double-taps
                    // and long-press window menus on bare `.window_drag()` regions still work.
                    self.host.titlebar.action = None;
                    self.touch_scroll_end(scroll, 0.0);
                    (Mode::WindowDragPending, true)
                }
                Some(TitlebarAction::DoubleClick) => {
                    self.touch_scroll_end(scroll, 0.0);
                    (Mode::WindowDragging, false)
                }
                // `El::touch_drags_at_once`: a drag from the first move, and no long press to cut
                // it, unless an edge swipe may still take the touch.
                None if !has_edge && self.press_drags_at_once() => {
                    self.touch_scroll_end(scroll, 0.0);
                    (Mode::Drag, false)
                }
                // An at-once drag in an edge zone waits for the slop, but still has no long press.
                None => (Mode::Pending { scroll, edge }, !(has_edge && self.press_drags_at_once())),
            }
        };
        let hold_event = if start_hold {
            HoldEvent::Start { now, event_time: time, policy }
        } else {
            HoldEvent::Cancel
        };
        let hold_effects = step_hold(&mut self.pointer.touch.hold_seq, &mut self.pointer.touch.hold, hold_event);
        let _ = self.apply_hold_effects(hold_effects);
        let finger = Finger { id, origin: (x, y), pos: (x, y), last: 0.0, at: time, velocity: 0.0, mode };
        let effects = step_touch(&mut self.pointer.touch.state, TouchInput::StartSingle(finger));
        self.apply_touch_effects(app, effects);
    }

    fn touch_move(&mut self, app: &dyn ViewDriver, id: u64, x: f32, y: f32, time: f64) {
        let slop = self.pointer.touch.state.single_slop_query(id, (x, y)).map(|(origin, horizontal_bias, _)| {
            SlopResolution {
                press_drags: self.press_drags(),
                across: horizontal_bias.then(|| self.scroll_at_axis(origin.0, origin.1, true)).flatten(),
            }
        });
        let effects = step_touch(&mut self.pointer.touch.state, TouchInput::Move { id, pos: (x, y), time, slop });
        self.apply_touch_effects(app, effects);
    }

    fn touch_up(&mut self, app: &dyn ViewDriver, id: u64, x: f32, y: f32, time: f64, cancelled: bool) {
        self.pointer.touch.in_touch = true;
        self.touch_up_inner(app, id, x, y, time, cancelled);
        self.pointer.touch.in_touch = false;
    }

    fn touch_up_inner(&mut self, app: &dyn ViewDriver, id: u64, x: f32, y: f32, time: f64, cancelled: bool) {
        let effects = step_touch(&mut self.pointer.touch.state, TouchInput::End { id, pos: (x, y), time, cancelled });
        self.apply_touch_effects(app, effects);
    }

    /// A finger still within the slop when its long press is due: under `DirectTimer` it fires
    /// now; under `ConfirmingTimer` it enters `Confirming` and asks the host to confirm once
    /// waiting input has drained.
    pub(in crate::runtime) fn fire_touch_hold(&mut self, app: &dyn ViewDriver) {
        if !self.pointer.touch.state.hold_eligible() {
            return;
        }
        let policy = self.pointer.touch.policy;
        let effects = step_hold(&mut self.pointer.touch.hold_seq, &mut self.pointer.touch.hold, HoldEvent::Tick { now: self.clock, policy });
        if self.apply_hold_effects(effects) {
            self.commit_touch_hold(app);
        }
    }

    /// Completes a pending `ConfirmingTimer` hold after the host drained queued input; stale or
    /// cancelled attempts do nothing and return `false`.
    #[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
    pub(crate) fn confirm_touch_hold(&mut self, app: &dyn ViewDriver, attempt: Attempt) -> bool {
        if !self.pointer.touch.state.hold_eligible() {
            let effects = step_hold(&mut self.pointer.touch.hold_seq, &mut self.pointer.touch.hold, HoldEvent::Cancel);
            let _ = self.apply_hold_effects(effects);
            return false;
        }
        let effects = step_hold(&mut self.pointer.touch.hold_seq, &mut self.pointer.touch.hold, HoldEvent::Confirm { attempt });
        if self.apply_hold_effects(effects) {
            self.commit_touch_hold(app);
            return true;
        }
        false
    }

    /// Handles a browser-dispatched live touch `contextmenu` event for finger `id`. Fires the
    /// hold once while the finger is still pending within `TOUCH_SLOP`, cancels any pending
    /// fallback timer/confirmation, and ignores duplicate or post-lift events.
    #[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
    pub(crate) fn host_touch_hold(&mut self, app: &dyn ViewDriver, id: u64) -> bool {
        if !self.pointer.touch.state.single().is_some_and(|f| f.id == id && f.mode.hold_eligible()) || self.press_drags_at_once() {
            return false;
        }
        let effects = step_hold(&mut self.pointer.touch.hold_seq, &mut self.pointer.touch.hold, HoldEvent::HostHold);
        if self.apply_hold_effects(effects) {
            self.commit_touch_hold(app);
            return true;
        }
        false
    }

    /// Turns the held press into a secondary click where it went down: the press it began lets go
    /// without a click (a drag it started hears a cancelled `Phase::Up`), and a secondary click
    /// lands where it went down.
    fn commit_touch_hold(&mut self, app: &dyn ViewDriver) {
        let effects = step_touch(&mut self.pointer.touch.state, TouchInput::HoldFired);
        self.apply_touch_effects(app, effects);
    }

    /// Whether the held press is on an `El::touch_drags_at_once` element.
    fn press_drags_at_once(&self) -> bool {
        let Some(path) = self.pointer.press.path() else { return false };
        self.live_index(path).is_some_and(|i| self.tree.nodes[i].handlers.touch_at_once && self.tree.nodes[i].handlers.on_drag.is_some())
    }

    /// Whether the held press is on an element that drags, so a finger drags it, not the page.
    fn press_drags(&self) -> bool {
        let Some(path) = self.pointer.press.path() else { return false };
        self.live_index(path).is_some_and(|i| self.tree.nodes[i].handlers.on_drag.is_some() && !self.drags_out(&self.tree.nodes[i]))
    }
}

#[cfg(test)]
#[path = "touch_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "touch_pinch_tests.rs"]
mod pinch_tests;

#[cfg(test)]
#[path = "touch_multi_tests.rs"]
mod multi_tests;
