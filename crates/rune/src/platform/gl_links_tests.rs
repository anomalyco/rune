use std::time::Duration;
use crate::machine::{Attempt, Seq};
use super::*;

struct Harness {
    state: GlLinkState,
    seq: Seq,
    scheduled_polls: usize,
    last_attempt: Option<Attempt>,
    last_delay: Option<Duration>,
    built_classic: usize,
    delivered: usize,
    failed: Vec<String>,
    aborted: usize,
}

impl Harness {
    fn new() -> Self {
        Self {
            state: GlLinkState::default(),
            seq: Seq::default(),
            scheduled_polls: 0,
            last_attempt: None,
            last_delay: None,
            built_classic: 0,
            delivered: 0,
            failed: Vec::new(),
            aborted: 0,
        }
    }

    fn send(&mut self, event: GlLinkEvent) -> Vec<GlLinkEffect> {
        let effects: Vec<_> = self.state.step(&mut self.seq, event).into_iter().collect();
        for effect in &effects {
            match effect {
                GlLinkEffect::SchedulePoll { attempt, delay } => {
                    self.scheduled_polls += 1;
                    self.last_attempt = Some(*attempt);
                    self.last_delay = Some(*delay);
                }
                GlLinkEffect::BuildClassic => self.built_classic += 1,
                GlLinkEffect::Deliver => self.delivered += 1,
                GlLinkEffect::Fail(reason) => self.failed.push(reason.clone()),
                GlLinkEffect::Abort => self.aborted += 1,
            }
        }
        effects
    }
}

#[test]
fn deferred_links_poll_across_turns_then_stop_polling_when_ready() {
    let t0 = crate::time::now();
    let mut h = Harness::new();

    // Submission yields a turn (Duration::ZERO) before the first COMPLETION_STATUS_KHR poll.
    h.send(GlLinkEvent::Submitted { phase: LinkPhase::Windfoil, pending: 1, now: t0 });
    assert!(h.state.is_polling());
    assert_eq!(h.scheduled_polls, 1);
    assert_eq!(h.last_delay, Some(Duration::ZERO));
    let a1 = h.last_attempt.unwrap();

    // First poll: still compiling -> reschedules after GL_LINK_POLL_INTERVAL.
    h.send(GlLinkEvent::PollDue {
        attempt: a1,
        now: t0 + Duration::from_millis(1),
        outcome: LinkPollOutcome { pending: 1, error: None, context_lost: false },
    });
    assert!(h.state.is_polling());
    assert_eq!(h.scheduled_polls, 2);
    assert_eq!(h.last_delay, Some(GL_LINK_POLL_INTERVAL));
    let a2 = h.last_attempt.unwrap();
    assert_ne!(a1, a2);

    // Stale attempt is ignored and schedules nothing.
    let before = h.scheduled_polls;
    let stale = h.send(GlLinkEvent::PollDue {
        attempt: a1,
        now: t0 + Duration::from_millis(9),
        outcome: LinkPollOutcome { pending: 1, error: None, context_lost: false },
    });
    assert!(stale.is_empty());
    assert_eq!(h.scheduled_polls, before);

    // Second poll: link finished cleanly -> Deliver, Classic never built, no more polls.
    h.send(GlLinkEvent::PollDue {
        attempt: a2,
        now: t0 + Duration::from_millis(9),
        outcome: LinkPollOutcome { pending: 0, error: None, context_lost: false },
    });
    assert_eq!(h.state, GlLinkState::Ready);
    assert!(!h.state.is_polling());
    assert_eq!(h.delivered, 1);
    assert_eq!(h.built_classic, 0);
    assert_eq!(h.scheduled_polls, 2);

    // Idle page: further events schedule zero polls.
    let polls_when_ready = h.scheduled_polls;
    assert!(h.send(GlLinkEvent::PollDue {
        attempt: a2,
        now: t0 + Duration::from_millis(17),
        outcome: LinkPollOutcome::default(),
    }).is_empty());
    assert!(h.send(GlLinkEvent::Quit).is_empty());
    assert_eq!(h.scheduled_polls, polls_when_ready);
}

#[test]
fn zero_pending_links_deliver_immediately_with_zero_scheduled_polls() {
    let t0 = crate::time::now();
    let mut h = Harness::new();

    h.send(GlLinkEvent::Submitted { phase: LinkPhase::Windfoil, pending: 0, now: t0 });
    assert_eq!(h.state, GlLinkState::Ready);
    assert!(!h.state.is_polling());
    assert_eq!(h.delivered, 1);
    assert_eq!(h.scheduled_polls, 0, "idle/already-ready submission must schedule zero polls");
}

#[test]
fn context_loss_while_pending_fails_cleanly_and_stops_polling() {
    let t0 = crate::time::now();
    let mut h = Harness::new();

    h.send(GlLinkEvent::Submitted { phase: LinkPhase::Windfoil, pending: 1, now: t0 });
    let a1 = h.last_attempt.unwrap();
    assert_eq!(h.scheduled_polls, 1);

    h.send(GlLinkEvent::PollDue {
        attempt: a1,
        now: t0 + Duration::from_millis(4),
        outcome: LinkPollOutcome { pending: 0, error: None, context_lost: true },
    });
    assert_eq!(h.state, GlLinkState::ContextLost);
    assert!(!h.state.is_polling());
    assert_eq!(h.delivered, 0);
    assert_eq!(h.built_classic, 0, "context loss must not retry Classic");
    assert_eq!(h.failed.len(), 1);
    assert!(h.failed[0].contains("context lost"), "unexpected error: {}", h.failed[0]);

    // Nothing else is scheduled after context loss.
    let polls_after = h.scheduled_polls;
    assert!(h.send(GlLinkEvent::PollDue {
        attempt: a1,
        now: t0 + Duration::from_millis(12),
        outcome: LinkPollOutcome { pending: 1, error: None, context_lost: false },
    }).is_empty());
    assert_eq!(h.scheduled_polls, polls_after);
}

#[test]
fn early_pipeline_use_success_and_failure_resolve_on_next_poll_without_extra_polls() {
    let t0 = crate::time::now();

    // Case 1: CommandEncoder::set_pipeline_inner finalized the pending program synchronously
    // and it succeeded; poll_deferred_links prunes it and reports pending=0, error=None.
    let mut ok_h = Harness::new();
    ok_h.send(GlLinkEvent::Submitted { phase: LinkPhase::Windfoil, pending: 1, now: t0 });
    let a1 = ok_h.last_attempt.unwrap();
    ok_h.send(GlLinkEvent::PollDue {
        attempt: a1,
        now: t0 + Duration::from_millis(2),
        outcome: LinkPollOutcome { pending: 0, error: None, context_lost: false },
    });
    assert_eq!(ok_h.state, GlLinkState::Ready);
    assert_eq!(ok_h.delivered, 1);
    assert_eq!(ok_h.scheduled_polls, 1);

    // Case 2: Early use finalized a pending Windfoil program and it FAILED to link;
    // poll_deferred_links surfaces that recorded failure and triggers Classic fallback.
    let mut fail_h = Harness::new();
    fail_h.send(GlLinkEvent::Submitted { phase: LinkPhase::Windfoil, pending: 1, now: t0 });
    let a1 = fail_h.last_attempt.unwrap();
    fail_h.send(GlLinkEvent::PollDue {
        attempt: a1,
        now: t0 + Duration::from_millis(2),
        outcome: LinkPollOutcome {
            pending: 0,
            error: Some("link error on early use".into()),
            context_lost: false,
        },
    });
    assert_eq!(fail_h.state, GlLinkState::AwaitingClassic);
    assert_eq!(fail_h.built_classic, 1);
    assert_eq!(fail_h.scheduled_polls, 1);
}

#[test]
fn windfoil_link_rejection_falls_back_to_classic_and_classic_rejection_fails_startup() {
    let t0 = crate::time::now();
    let mut h = Harness::new();

    // Windfoil submitted, then driver rejects it at link time.
    h.send(GlLinkEvent::Submitted { phase: LinkPhase::Windfoil, pending: 1, now: t0 });
    let a1 = h.last_attempt.unwrap();
    h.send(GlLinkEvent::PollDue {
        attempt: a1,
        now: t0 + Duration::from_millis(10),
        outcome: LinkPollOutcome {
            pending: 0,
            error: Some("Windfoil fragment too complex".into()),
            context_lost: false,
        },
    });
    assert_eq!(h.state, GlLinkState::AwaitingClassic);
    assert_eq!(h.built_classic, 1);
    assert!(h.failed.is_empty());

    // Classic submitted and polls asynchronously.
    let t1 = t0 + Duration::from_millis(11);
    h.send(GlLinkEvent::Submitted { phase: LinkPhase::Classic, pending: 1, now: t1 });
    assert!(h.state.is_polling());
    let a2 = h.last_attempt.unwrap();

    // Classic ALSO rejected -> terminal Failed with the #1833 error message, zero further polls.
    h.send(GlLinkEvent::PollDue {
        attempt: a2,
        now: t1 + Duration::from_millis(8),
        outcome: LinkPollOutcome {
            pending: 0,
            error: Some("Classic GLSL compile error".into()),
            context_lost: false,
        },
    });
    assert!(matches!(h.state, GlLinkState::Failed { .. }));
    assert!(!h.state.is_polling());
    assert_eq!(h.delivered, 0);
    assert_eq!(h.failed, vec!["the GPU rejected Rune's shader: Classic GLSL compile error"]);

    let polls = h.scheduled_polls;
    assert!(h.send(GlLinkEvent::PollDue {
        attempt: a2,
        now: t1 + Duration::from_millis(16),
        outcome: LinkPollOutcome::default(),
    }).is_empty());
    assert_eq!(h.scheduled_polls, polls);
}

#[test]
fn windfoil_rejection_then_classic_success_delivers_once() {
    let t0 = crate::time::now();
    let mut h = Harness::new();

    h.send(GlLinkEvent::Submitted { phase: LinkPhase::Windfoil, pending: 1, now: t0 });
    let a1 = h.last_attempt.unwrap();
    h.send(GlLinkEvent::PollDue {
        attempt: a1,
        now: t0 + Duration::from_millis(8),
        outcome: LinkPollOutcome {
            pending: 0,
            error: Some("outline shader rejected".into()),
            context_lost: false,
        },
    });
    assert_eq!(h.built_classic, 1);

    let t1 = t0 + Duration::from_millis(9);
    h.send(GlLinkEvent::Submitted { phase: LinkPhase::Classic, pending: 1, now: t1 });
    let a2 = h.last_attempt.unwrap();
    h.send(GlLinkEvent::PollDue {
        attempt: a2,
        now: t1 + Duration::from_millis(8),
        outcome: LinkPollOutcome { pending: 0, error: None, context_lost: false },
    });
    assert_eq!(h.state, GlLinkState::Ready);
    assert_eq!(h.delivered, 1);
    assert!(h.failed.is_empty());
    assert!(!h.state.is_polling());
}

#[test]
fn page_quit_while_pending_aborts_immediately_and_schedules_nothing() {
    let t0 = crate::time::now();
    let mut h = Harness::new();

    h.send(GlLinkEvent::Submitted { phase: LinkPhase::Windfoil, pending: 1, now: t0 });
    let a1 = h.last_attempt.unwrap();
    assert_eq!(h.scheduled_polls, 1);

    h.send(GlLinkEvent::Quit);
    assert_eq!(h.state, GlLinkState::Aborted);
    assert!(!h.state.is_polling());
    assert_eq!(h.aborted, 1);
    assert_eq!(h.delivered, 0);
    assert!(h.failed.is_empty());

    // Any in-flight timer firing after quit is ignored and schedules zero further polls.
    assert!(h.send(GlLinkEvent::PollDue {
        attempt: a1,
        now: t0 + Duration::from_millis(8),
        outcome: LinkPollOutcome { pending: 1, error: None, context_lost: false },
    }).is_empty());
    assert_eq!(h.scheduled_polls, 1);
}

#[test]
fn link_timeout_stops_polling_and_caps_final_delay_to_deadline() {
    let t0 = crate::time::now();
    let mut h = Harness::new();

    h.send(GlLinkEvent::Submitted { phase: LinkPhase::Windfoil, pending: 1, now: t0 });
    let a1 = h.last_attempt.unwrap();

    // 3 ms before deadline: next poll delay is clamped to 3 ms rather than overshooting by 8 ms.
    h.send(GlLinkEvent::PollDue {
        attempt: a1,
        now: t0 + GL_LINK_TIMEOUT - Duration::from_millis(3),
        outcome: LinkPollOutcome { pending: 1, error: None, context_lost: false },
    });
    assert_eq!(h.last_delay, Some(Duration::from_millis(3)));
    let a2 = h.last_attempt.unwrap();

    // At deadline, still pending -> TimedOut, Fail, zero further polls.
    h.send(GlLinkEvent::PollDue {
        attempt: a2,
        now: t0 + GL_LINK_TIMEOUT,
        outcome: LinkPollOutcome { pending: 1, error: None, context_lost: false },
    });
    assert_eq!(h.state, GlLinkState::TimedOut);
    assert!(!h.state.is_polling());
    assert_eq!(h.failed.len(), 1);
    assert!(h.failed[0].contains("timed out"), "unexpected error: {}", h.failed[0]);

    let polls = h.scheduled_polls;
    assert!(h.send(GlLinkEvent::PollDue {
        attempt: a2,
        now: t0 + GL_LINK_TIMEOUT + GL_LINK_POLL_INTERVAL,
        outcome: LinkPollOutcome { pending: 1, error: None, context_lost: false },
    }).is_empty());
    assert_eq!(h.scheduled_polls, polls);
}

#[test]
fn sync_submit_failure_falls_back_or_fails_with_zero_scheduled_polls() {
    let t0 = crate::time::now();
    let mut h = Harness::new();

    // Synchronous Windfoil failure (e.g. WGSL/Naga error) asks for Classic with zero polls.
    h.send(GlLinkEvent::SubmitFailed {
        phase: LinkPhase::Windfoil,
        reason: "windfoil validation failed".into(),
    });
    assert_eq!(h.state, GlLinkState::AwaitingClassic);
    assert_eq!(h.built_classic, 1);
    assert_eq!(h.scheduled_polls, 0);

    // Synchronous Classic failure fails startup immediately with zero polls.
    h.send(GlLinkEvent::SubmitFailed {
        phase: LinkPhase::Classic,
        reason: "the GPU rejected Rune's shader: classic validation failed".into(),
    });
    assert!(matches!(h.state, GlLinkState::Failed { .. }));
    assert_eq!(h.failed, vec!["the GPU rejected Rune's shader: classic validation failed"]);
    assert_eq!(h.scheduled_polls, 0);

    // Further events after terminal failure do nothing.
    assert!(h.send(GlLinkEvent::Submitted {
        phase: LinkPhase::Classic,
        pending: 1,
        now: t0,
    }).is_empty());
    assert_eq!(h.scheduled_polls, 0);
}

#[test]
fn vendored_gles_backend_drops_eager_compile_status_and_supports_deferred_links() {
    // Structural check on the vendored GLES backend: compile_shader must not synchronously query
    // COMPILE_STATUS or shader info logs (those run only inside finalize_program when LINK_STATUS
    // fails), eliminating 4 synchronous status/log queries per 2-stage program on every driver
    // (and all synchronous queries during submission when deferred linking is enabled).
    let device_rs = include_str!("../../../../vendor/wgpu-hal/src/gles/device.rs");
    let compile_fn_start = device_rs
        .find("unsafe fn compile_shader(")
        .expect("compile_shader present in vendored gles/device.rs");
    let create_shader_start = device_rs[compile_fn_start..]
        .find("fn create_shader(")
        .expect("create_shader follows compile_shader");
    let compile_body = &device_rs[compile_fn_start..compile_fn_start + create_shader_start];
    assert!(
        !compile_body.contains("get_shader_compile_status"),
        "compile_shader must not eagerly query GL_COMPILE_STATUS"
    );
    assert!(
        !compile_body.contains("get_shader_info_log"),
        "compile_shader must not eagerly query shader info logs"
    );
    assert!(
        device_rs.contains("pub fn poll_deferred_links(")
            && device_rs.contains("get_program_completion_status")
            && device_rs.contains("is_context_lost()"),
        "vendored gles/device.rs must expose non-blocking poll_deferred_links with context-loss guard"
    );
}
