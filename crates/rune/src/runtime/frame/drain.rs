//! Bounded same-turn follow-up drain: a turn does not end with work that it itself queued still
//! waiting; it repeats follow-up rounds while due work remains, at most [`MAX_POLL_ROUNDS`]
//! times, and then stops, logging a debug-only warning once and scheduling a continuation wake
//! so a self-requeueing handler or poll cannot spin the host.
//!
//! Shared by hidden and covered wakes (draining input that `poll` or a handler queued without
//! drawing) and by the visible frame loop for same-turn `poll <-> handlers` follow-up rounds.

use super::EndOfFrameRequests;
use crate::machine::Effects;
use crate::runtime::*;

/// Maximum follow-up rounds a single turn may run to drain work that it itself queued
/// (matching [`super::mount::MAX_MOUNT_ROUNDS`] = 4), so a self-requeueing callback cannot
/// spin a window.
pub(in crate::runtime) const MAX_POLL_ROUNDS: usize = super::mount::MAX_MOUNT_ROUNDS;

/// Explicit state of a bounded same-turn follow-up drain.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::runtime) enum DrainState {
    /// Initial pass has not yet triggered a follow-up round.
    #[default]
    Active,
    /// Follow-up round `rounds` (`1..=MAX_POLL_ROUNDS`) is running.
    Draining { rounds: usize },
    /// No same-turn work remained, or [`MAX_POLL_ROUNDS`] was reached.
    Done { capped: bool },
}

/// Observation after a turn's initial step or after a follow-up round.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::runtime) enum DrainEvent {
    /// `more` is whether the turn left same-turn work waiting.
    StepDone { more: bool },
}

/// Effect emitted by [`step_drain`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::runtime) enum DrainEffect {
    /// Run one bounded follow-up round (`round` is 1-based, up to [`MAX_POLL_ROUNDS`]).
    FollowUp { round: usize },
    /// [`MAX_POLL_ROUNDS`] was exhausted with work still waiting: stop and defer continuation.
    Capped,
}

/// Outcome of [`drain_turn`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::runtime) struct DrainOutcome {
    /// Number of follow-up rounds executed (`0..=MAX_POLL_ROUNDS`).
    pub rounds: usize,
    /// `true` when [`MAX_POLL_ROUNDS`] was reached with work still waiting.
    pub capped: bool,
}

/// Advances [`DrainState`] after a round finishes: emits [`DrainEffect::FollowUp`] while `more`
/// holds and fewer than [`MAX_POLL_ROUNDS`] follow-up rounds have run; emits
/// [`DrainEffect::Capped`] if `more` still holds at the cap, then transitions to
/// [`DrainState::Done`].
pub(in crate::runtime) fn step_drain(
    state: &mut DrainState,
    event: DrainEvent,
) -> Effects<DrainEffect> {
    let DrainEvent::StepDone { more } = event;
    let ran = match *state {
        DrainState::Active => 0,
        DrainState::Draining { rounds } => rounds,
        DrainState::Done { .. } => return Effects::none(),
    };
    if !more {
        *state = DrainState::Done { capped: false };
        Effects::none()
    } else if ran < MAX_POLL_ROUNDS {
        let round = ran + 1;
        *state = DrainState::Draining { rounds: round };
        Effects::one(DrainEffect::FollowUp { round })
    } else {
        *state = DrainState::Done { capped: true };
        Effects::one(DrainEffect::Capped)
    }
}

/// Runs follow-up rounds while `has_more` reports same-turn work queued by the current turn,
/// bounded to [`MAX_POLL_ROUNDS`]. When the cap is reached with work still waiting, logs the
/// debug-only cap warning once and returns `DrainOutcome { capped: true, .. }` so the caller
/// schedules a continuation without spinning.
pub(in crate::runtime) fn drain_turn<C>(
    ctx: &mut C,
    mut has_more: impl FnMut(&C) -> bool,
    mut run_round: impl FnMut(&mut C, usize),
) -> DrainOutcome {
    let mut state = DrainState::Active;
    let mut rounds = 0;
    loop {
        let effects = step_drain(&mut state, DrainEvent::StepDone { more: has_more(ctx) });
        match effects.into_iter().next() {
            Some(DrainEffect::FollowUp { round }) => {
                rounds = round;
                run_round(ctx, round);
            }
            Some(DrainEffect::Capped) => {
                crate::tooling::perf_warn::poll_round_cap(MAX_POLL_ROUNDS);
                return DrainOutcome { rounds, capped: true };
            }
            None => return DrainOutcome { rounds, capped: false },
        }
    }
}

impl RuntimeCore {
    /// Runs a batch of queued actions inside `frame_step`, preserving split-commit routing
    /// boundaries and tracking whether any non-`live_only`, non-layout-observer handler ran.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn run_action_queue(
        &mut self,
        app: &mut dyn AppDriver,
        queue: ActionQueue,
        tx: &mut Tx,
        ran: &mut bool,
        ran_layout: &mut bool,
        why: &mut u16,
        pointer_hover: &mut bool,
        handler_ran: &mut bool,
        drag_cancelled: &mut bool,
        #[cfg(debug_assertions)] ran_other: &mut bool,
    ) {
        for Queued { action, at, input } in queue {
            if *drag_cancelled {
                match &action {
                    Action::Drag(_, d, _) | Action::DragRepeat(_, d, _) if d.phase != Phase::Down => continue,
                    Action::Drag(_, d, _) if d.phase == Phase::Down => *drag_cancelled = false,
                    _ => {}
                }
            }
            if matches!(action, Action::Key { stage: false, .. } | Action::Physical(crate::PhysicalInput::Key { pressed: true, .. }, _)) {
                crate::marks::input_handled(true);
            }
            *pointer_hover |= matches!(action, Action::Hover(_));
            // The user's own key or press takes over from a `Tx::focus_when_shown` still waiting.
            if matches!(action, Action::Key { stage: false, .. } | Action::Physical(crate::PhysicalInput::Key { pressed: true, .. }, _) | Action::Press(..) | Action::Focus(..)) {
                self.kbd.focus_owed = None;
            }
            // State may have requested focus on a newly mounted field. Resolve the new tree before
            // routing this frame's text, not after it has gone to the previous field.
            let routed = matches!(&action, Action::Key { .. } | Action::KeyUp(_) | Action::Physical(..) | Action::Ime(_) | Action::FinishIme | Action::Paste(..) | Action::Files(_) | Action::SecondaryRepost { .. } | Action::WheelRepost(_));
            if routed {
                self.latch_playground(app, tx);
                self.latch_focus(tx);
                if let Some(picked) = tx.field_menu_resolution.take() {
                    self.latch_field_menu_resolution(app, picked, tx);
                }
                if let Some(picked) = tx.popup_menu_resolution.take() {
                    self.latch_popup_menu_resolution(app, picked, tx);
                }
            }
            // Both callbacks see ordinary &mut S. Observe the poll's tree now, without drawing it
            // or advancing time. A key already needed this same routing boundary.
            let split = self.motion.motion_needs_cause() && *why & CommitWhy::POLL != 0 && (input || routed);
            if split || (*ran || self.commit.dirty || !tx.scroll_to.is_empty()) && routed {
                if split {
                    let deferred = EndOfFrameRequests::take_from(tx);
                    self.latch_requests(app, tx);
                    deferred.restore_into(tx);
                }
                if *ran || self.commit.dirty {
                    self.motion.capture_flights(&self.tree.nodes, &mut self.paint, &mut self.text, self.env.scale, self.viewport, self.t, tx);
                    if self.commit.is_settling() { tx.settle(); }
                    self.settled |= tx.commit.lands();
                    #[cfg(debug_assertions)]
                    if *ran_other || self.commit.dirty || *ran_layout { self.frame.clear_live_check(); }
                    self.capture_keeps(tx);
                    tx.unprompted = self.motion.motion_needs_cause() && *why & CommitWhy::INPUT == 0 && !tx.asked_motion;
                    if split && tx.unprompted { tx.commit = tx.commit.without_input(); }
                    if routed { *why |= CommitWhy::ROUTE; }
                    if self.commit.dirty { *why |= CommitWhy::DIRTY; }
                    self.commit_for(app, tx, std::mem::take(why));
                    #[cfg(debug_assertions)]
                    { let check = self.frame.take_live_check(); self.compare_live_check(check); *ran_other = false; }
                    self.apply_keeps(app);
                    self.finish_deferred_field_menu(app, tx, None);
                    self.finish_deferred_popup_menu(app, tx, None);
                    *ran = false;
                }
                if split {
                    self.finish_scroll_requests(app, tx, false);
                    tx.commit = Commit::Animate;
                    tx.unprompted = false;
                    tx.asked_motion = false;
                    tx.live_growth.clear(); tx.tail_rooms.clear();
                    tx.motion = None;
                    tx.color_motion = None;
                    tx.follow_edge = false;
                    tx.unclipped_flights = false;
                    *ran_layout = false;
                } else if !tx.scroll_to.is_empty() {
                    self.drop_scroll_holds();
                    for r in tx.scroll_to.drain(..) { self.reveal_row(app, &r, tx.commit.snaps()); }
                }
            }
            let _input = crate::input::time::scope(at);
            if matches!(action, Action::Key { .. } | Action::Physical(..)) && let Some(request) = tx.select.take() {
                let from_change = std::mem::take(&mut tx.select_from_change);
                self.apply_select_request(app, request, from_change);
            }
            if matches!(action, Action::Layout(..) | Action::ScreenLayout(..) | Action::DrawnLayout(_, _, false) | Action::VisibleRows(..)) {
                *ran_layout |= self.run_action(app, action, tx);
            } else {
                let observer = matches!(action, Action::DrawnLayout(..) | Action::Visible(..) | Action::AtBottom(..) | Action::FarFromBottom(..));
                #[cfg(debug_assertions)]
                let checks = self.frame.live_checks();
                let r = self.run_action(app, action, tx);
                #[cfg(debug_assertions)]
                let non_live = r && checks == self.frame.live_checks();
                #[cfg(not(debug_assertions))]
                let non_live = r;
                #[cfg(debug_assertions)]
                { *ran_other |= non_live; }
                if !observer {
                    *handler_ran |= non_live;
                }
                *ran |= r;
                if r { *why |= if input { CommitWhy::INPUT } else { CommitWhy::DIRTY }; }
            }
            self.run_pick_reads(app, tx);
            *drag_cancelled |= self.flush_cancel_pointer(
                app,
                tx,
                ran,
                why,
                handler_ran,
                #[cfg(debug_assertions)]
                ran_other,
            );
        }
    }

    /// Runs bounded same-frame follow-up `poll <-> handlers` rounds through [`drain_turn`]
    /// (at most [`MAX_POLL_ROUNDS`]) before the main commit, preserving `CommitWhy::INPUT` and
    /// the frame's `polled_at()` clock.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn run_poll_follow_ups(
        &mut self,
        app: &mut dyn AppDriver,
        tx: &mut Tx,
        ran: &mut bool,
        ran_layout: &mut bool,
        why: &mut u16,
        pointer_hover: &mut bool,
        handler_ran: &mut bool,
        frame_had_input: &mut bool,
        #[cfg(debug_assertions)] ran_other: &mut bool,
    ) {
        let poll_clock = self.polled_at();
        struct Ctx<'a> {
            rt: &'a mut RuntimeCore,
            app: &'a mut dyn AppDriver,
            tx: &'a mut Tx,
            ran: &'a mut bool,
            ran_layout: &'a mut bool,
            why: &'a mut u16,
            pointer_hover: &'a mut bool,
            handler_ran: &'a mut bool,
            frame_had_input: &'a mut bool,
            #[cfg(debug_assertions)]
            ran_other: &'a mut bool,
        }
        let mut ctx = Ctx {
            rt: self,
            app,
            tx,
            ran,
            ran_layout,
            why,
            pointer_hover,
            handler_ran,
            frame_had_input,
            #[cfg(debug_assertions)]
            ran_other,
        };
        let outcome = drain_turn(
            &mut ctx,
            |c| {
                *c.handler_ran
                    || c.tx.request_poll
                    || !c.tx.keys.is_empty()
                    || (c.rt.frame.queue.has_input() && !c.rt.files_held())
            },
            |c, _| {
                let need_poll = std::mem::take(c.handler_ran) || std::mem::take(&mut c.tx.request_poll);
                if need_poll {
                    let mut one = c.rt.transaction(c.app);
                    one.clock = poll_clock;
                    one.input_waiting = *c.frame_had_input || c.rt.frame.queue.has_input();
                    if c.rt.frame.record_poll(crate::tooling::perf_warn::timed("poll", || {
                        c.app.poll(poll_clock, &mut one)
                    })) {
                        *c.ran = true;
                        *c.why |= CommitWhy::POLL;
                        c.rt.commit.watch.meter.poll();
                    }
                    merge(c.tx, one);
                }
                for ev in std::mem::take(&mut c.tx.keys) {
                    c.rt.key(ev);
                }
                let mut drag_cancelled = c.rt.flush_cancel_pointer(
                    c.app,
                    c.tx,
                    c.ran,
                    c.why,
                    c.handler_ran,
                    #[cfg(debug_assertions)]
                    c.ran_other,
                );
                if !c.rt.frame.queue.is_empty() && !c.rt.files_held() {
                    *c.frame_had_input |= c.rt.frame.queue.has_input();
                    let ended: Vec<_> = c.rt.pointer.drag_ends.borrow_mut().drain(..).collect();
                    let q = order_drag_ends(
                        std::mem::take(&mut c.rt.frame.queue),
                        ended,
                        &mut c.rt.pointer.drags_open,
                    );
                    let q = c.rt.hold_files(q, *c.ran || c.rt.commit.dirty);
                    c.rt.run_action_queue(
                        c.app,
                        q,
                        c.tx,
                        c.ran,
                        c.ran_layout,
                        c.why,
                        c.pointer_hover,
                        c.handler_ran,
                        &mut drag_cancelled,
                        #[cfg(debug_assertions)]
                        c.ran_other,
                    );
                }
            },
        );
        if outcome.capped {
            self.frame.set_poll_capped(true);
            if *handler_ran || std::mem::take(&mut tx.request_poll) {
                self.schedule_wakes_from(poll_clock, [std::time::Duration::ZERO]);
            }
        }
    }
}
