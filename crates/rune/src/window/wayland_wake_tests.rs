use std::cell::Cell;
use std::rc::Rc;

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::{Effect, Event, State, about_to_wait, step};
use crate::window::{AnyHost, WakeEvent};

struct MockHost {
    live_layer: Rc<Cell<bool>>,
    pending_events: Rc<Cell<bool>>,
}

impl MockHost {
    fn window() -> (Box<dyn AnyHost>, HostCtl) {
        Self::new(false)
    }

    fn layer_overlay(live: bool) -> (Box<dyn AnyHost>, HostCtl) {
        Self::new(live)
    }

    fn new(live: bool) -> (Box<dyn AnyHost>, HostCtl) {
        let live_layer = Rc::new(Cell::new(live));
        let pending_events = Rc::new(Cell::new(false));
        let ctl = HostCtl {
            live_layer: live_layer.clone(),
            pending_events: pending_events.clone(),
            dirty: Cell::new(false),
            frames: Cell::new(0),
        };
        let host = Box::new(Self { live_layer, pending_events });
        (host, ctl)
    }
}

struct HostCtl {
    live_layer: Rc<Cell<bool>>,
    pending_events: Rc<Cell<bool>>,
    dirty: Cell<bool>,
    frames: Cell<usize>,
}

impl HostCtl {
    fn simulate_turn_dispatch(&self) {
        let _drained = self.pending_events.replace(false);
        if self.dirty.replace(false) {
            self.frames.set(self.frames.get() + 1);
        }
    }
}

impl AnyHost for MockHost {
    fn resumed(&mut self, _: &ActiveEventLoop) {}
    fn user_event(&mut self, _: &ActiveEventLoop, _: WakeEvent) {}
    fn owns(&self, _: WindowId) -> bool { false }
    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
    fn has_live_wayland_layer(&self) -> bool { self.live_layer.get() }
    fn has_pending_platform_events(&self) -> bool { self.pending_events.get() }
}

/// Counts every side effect emitted by `wayland_wake` across simulated event-loop turns.
#[derive(Default)]
struct Harness {
    state: State,
    wake_on_read: bool,
    set_wake_calls: Vec<bool>,
    wakes_sent: usize,
    wake_in_flight: bool,
}

impl Harness {
    fn apply(&mut self, effects: crate::machine::Effects<Effect>) {
        for effect in effects {
            match effect {
                Effect::SetWakeOnRead(on) => {
                    self.wake_on_read = on;
                    self.set_wake_calls.push(on);
                }
                Effect::SendWake => {
                    self.wakes_sent += 1;
                    self.wake_in_flight = true;
                }
            }
        }
    }

    fn deliver_wake_if_queued(&mut self) {
        if self.wake_in_flight {
            self.wake_in_flight = false;
            let effects = step(&mut self.state, Event::WakeReceived);
            assert!(effects.is_empty(), "WakeReceived must be a pure state transition");
        }
    }

    fn end_turn(&mut self, hosts: &[Option<Box<dyn AnyHost>>]) {
        let effects = about_to_wait(&mut self.state, hosts);
        self.apply(effects);
    }
}

#[test]
fn window_with_no_layer_overlay_never_sets_wake_flag_or_wakes() {
    let (window, ctl) = MockHost::window();
    let hosts = vec![Some(window)];
    let mut h = Harness::default();

    for _ in 0..20 {
        ctl.simulate_turn_dispatch();
        h.end_turn(&hosts);
    }

    assert_eq!(h.state, State::Inactive);
    assert!(!h.wake_on_read);
    assert!(h.set_wake_calls.is_empty(), "plain window must never toggle set_wake_on_wayland_read");
    assert_eq!(h.wakes_sent, 0);
    assert_eq!(ctl.frames.get(), 0);
}

#[test]
fn idle_overlay_with_no_pending_events_causes_zero_wakes_and_zero_frames() {
    let (window, win_ctl) = MockHost::window();
    let (overlay, ov_ctl) = MockHost::layer_overlay(true);
    let hosts = vec![Some(window), Some(overlay)];
    let mut h = Harness::default();

    // First turn after the layer surface becomes live enables wake_on_wayland_read once.
    ov_ctl.simulate_turn_dispatch();
    h.end_turn(&hosts);
    assert_eq!(h.state, State::Watching);
    assert!(h.wake_on_read);
    assert_eq!(h.set_wake_calls, vec![true]);
    assert_eq!(h.wakes_sent, 0);
    assert_eq!(ov_ctl.frames.get(), 0);

    // Subsequent idle turns emit no flag writes, no wakes, and draw no frames.
    for _ in 0..20 {
        win_ctl.simulate_turn_dispatch();
        ov_ctl.simulate_turn_dispatch();
        h.end_turn(&hosts);
    }

    assert_eq!(h.state, State::Watching);
    assert_eq!(h.set_wake_calls, vec![true]);
    assert_eq!(h.wakes_sent, 0, "idle overlay must cause zero wakes");
    assert_eq!(ov_ctl.frames.get(), 0, "idle overlay must draw zero frames");
    assert_eq!(win_ctl.frames.get(), 0);
}

#[test]
fn pending_event_wakes_loop_once_coalesces_while_in_flight_and_returns_to_idle() {
    let (overlay, ov_ctl) = MockHost::layer_overlay(true);
    let hosts = vec![Some(overlay)];
    let mut h = Harness::default();

    // Become live and idle.
    h.end_turn(&hosts);
    assert_eq!(h.state, State::Watching);
    assert_eq!(h.wakes_sent, 0);

    // Mid-turn read (e.g. Mesa WSI acquire/present) leaves an event queued at the end of the turn.
    ov_ctl.pending_events.set(true);
    h.end_turn(&hosts);
    assert_eq!(h.state, State::WakeQueued);
    assert_eq!(h.wakes_sent, 1, "pending event must wake the loop once");

    // Another about_to_wait before WakeEvent::LayerPending arrives coalesces rather than spamming.
    h.end_turn(&hosts);
    assert_eq!(h.state, State::WakeQueued);
    assert_eq!(h.wakes_sent, 1, "in-flight wake must coalesce");

    // Next turn: WakeEvent::LayerPending is received, then about_to_wait dispatches the queue.
    h.deliver_wake_if_queued();
    assert_eq!(h.state, State::Watching);
    ov_ctl.simulate_turn_dispatch();
    h.end_turn(&hosts);
    assert_eq!(h.state, State::Watching);
    assert_eq!(h.wakes_sent, 1, "draining the pending event must not wake again");
    assert_eq!(ov_ctl.frames.get(), 0, "a non-dirtying protocol event draws zero frames");

    // Further idle turns remain completely quiet.
    for _ in 0..10 {
        ov_ctl.simulate_turn_dispatch();
        h.end_turn(&hosts);
    }
    assert_eq!(h.wakes_sent, 1);
    assert_eq!(ov_ctl.frames.get(), 0);
}

#[test]
fn hiding_all_layer_overlays_and_exiting_clear_wake_flag_once() {
    let (overlay, ov_ctl) = MockHost::layer_overlay(true);
    let hosts = vec![Some(overlay)];
    let mut h = Harness::default();

    // Show overlay with an event already pending right after LayerSurface::new.
    ov_ctl.pending_events.set(true);
    h.end_turn(&hosts);
    assert_eq!(h.state, State::WakeQueued);
    assert_eq!(h.set_wake_calls, vec![true]);
    assert_eq!(h.wakes_sent, 1);

    // Hide the overlay before the queued wake arrives: flag clears immediately and stale wake is ignored.
    ov_ctl.live_layer.set(false);
    ov_ctl.pending_events.set(false);
    h.end_turn(&hosts);
    assert_eq!(h.state, State::Inactive);
    assert!(!h.wake_on_read);
    assert_eq!(h.set_wake_calls, vec![true, false]);

    h.deliver_wake_if_queued();
    assert_eq!(h.state, State::Inactive);

    // Idle turns after hiding never touch the flag again.
    for _ in 0..5 {
        h.end_turn(&hosts);
    }
    assert_eq!(h.set_wake_calls, vec![true, false]);

    // Show again, then exit the loop while live: Exiting clears the flag once.
    ov_ctl.live_layer.set(true);
    h.end_turn(&hosts);
    assert_eq!(h.state, State::Watching);
    assert_eq!(h.set_wake_calls, vec![true, false, true]);

    let exit_effects = step(&mut h.state, Event::Exiting);
    h.apply(exit_effects);
    assert_eq!(h.state, State::Inactive);
    assert!(!h.wake_on_read);
    assert_eq!(h.set_wake_calls, vec![true, false, true, false]);

    // Exiting while already inactive emits nothing.
    let exit_again = step(&mut h.state, Event::Exiting);
    assert!(exit_again.is_empty());
}
