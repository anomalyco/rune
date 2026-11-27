//! `App::on_frame_stage`, `App::on_window_key`, and `App::on_window_focus`: zero-cost unset
//! defaults, `WindowSpec` inheritance/override, balanced nesting, re-entrancy & panic safety,
//! sequence correlation with `FrameEncoded`, and real `Runtime` / `rune-text` stage emission.

use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::frame::encoded::{FrameReports, OnFrameEncoded};
use crate::{App, AppSpec, Env, FrameOutcome, Runtime, WindowSpec, el, text};

fn spec() -> AppSpec<()> {
    AppSpec::new("stage-test", (160.0, 120.0), || (), |_, _| el())
}

#[test]
fn unset_hooks_are_none_and_do_not_activate_stage_tracking() {
    let app = App::new(spec());
    assert!(app.observability.on_frame_stage.is_none());
    assert!(app.observability.on_window_key.is_none());
    assert!(app.observability.on_window_focus.is_none());
    let win = WindowSpec::new("plain", spec());
    assert!(win.app.observability.on_frame_stage.is_none());
    assert!(win.app.observability.on_window_key.is_none());
    assert!(win.app.observability.on_window_focus.is_none());

    let mut obs = HostObservability::new(WindowObservability::default(), Some("plain"));
    assert!(!obs.wants_stages());
    assert!(!active());
    {
        let _scope = obs.scope_stages(Some(0));
        assert!(!active(), "unset on_frame_stage must leave TLS flag false");
        let _guard = StageGuard::enter(FrameStage::Redraw);
    }
    obs.report_key(true, false, false, None);
    obs.report_focus(true);
    let reports = FrameReports::with_presented_seq(None, false, Some("plain"));
    assert_eq!(reports.next_seq(), None);
}

#[test]
fn window_spec_inherits_and_overrides_observability_hooks() {
    let log: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
    let a1 = log.clone();
    let a2 = log.clone();
    let a3 = log.clone();
    let mut w1 = WindowSpec::from_app(
        "main",
        App::new(spec())
            .on_frame_stage(move |_| a1.borrow_mut().push("app-stage"))
            .on_window_key(move |_| a2.borrow_mut().push("app-key"))
            .on_window_focus(move |_| a3.borrow_mut().push("app-focus")),
    );
    let mut obs1 = HostObservability::new(w1.app.observability.take(), Some("main"));
    {
        let _s = obs1.scope_stages(Some(0));
        emit(FrameStage::Acquire, StageBoundary::Begin);
    }
    obs1.report_key(true, false, true, None);
    obs1.report_focus(true);
    assert_eq!(*log.borrow(), ["app-stage", "app-key", "app-focus"]);

    let wlog: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
    let b1 = wlog.clone();
    let b2 = wlog.clone();
    let b3 = wlog.clone();
    let mut w2 = WindowSpec::from_app(
        "inspector",
        App::new(spec())
            .on_frame_stage(|_| unreachable!())
            .on_window_key(|_| unreachable!())
            .on_window_focus(|_| unreachable!()),
    )
    .on_frame_stage(move |_| b1.borrow_mut().push("win-stage"))
    .on_window_key(move |_| b2.borrow_mut().push("win-key"))
    .on_window_focus(move |_| b3.borrow_mut().push("win-focus"));
    let mut obs2 = HostObservability::new(w2.app.observability.take(), Some("inspector"));
    {
        let _s = obs2.scope_stages(Some(0));
        emit(FrameStage::Acquire, StageBoundary::Begin);
    }
    obs2.report_key(false, true, false, None);
    obs2.report_focus(false);
    assert_eq!(*wlog.borrow(), ["win-stage", "win-key", "win-focus"]);
}

#[test]
fn window_key_and_focus_report_non_sensitive_metadata_and_timestamps() {
    let keys = Rc::new(RefCell::new(Vec::<WindowKeyEvent>::new()));
    let focuses = Rc::new(RefCell::new(Vec::<WindowFocusEvent>::new()));
    let k_sink = keys.clone();
    let f_sink = focuses.clone();
    let mut app = App::new(spec())
        .on_window_key(move |e| k_sink.borrow_mut().push(e.clone()))
        .on_window_focus(move |e| f_sink.borrow_mut().push(e.clone()));
    let mut obs = HostObservability::new(app.observability.take(), Some("editor"));

    let os_time = crate::time::now();
    obs.report_key(true, false, true, Some(os_time));
    obs.report_key(true, true, true, Some(os_time));
    obs.report_key(false, false, false, None);
    obs.report_focus(true);
    obs.report_focus(false);

    let keys = keys.borrow();
    assert_eq!(keys.len(), 3);
    assert_eq!(keys[0].window(), Some("editor"));
    assert!((keys[0].pressed(), keys[0].repeat(), keys[0].has_focus()) == (true, false, true));
    assert_eq!(keys[0].input_time(), Some(os_time));
    assert!(keys[0].at() >= os_time);
    assert_eq!((keys[1].pressed(), keys[1].repeat(), keys[1].has_focus()), (true, true, true));
    assert_eq!((keys[2].pressed(), keys[2].repeat(), keys[2].has_focus(), keys[2].input_time()), (false, false, false, None));

    let focuses = focuses.borrow();
    assert_eq!(focuses.len(), 2);
    assert_eq!((focuses[0].window(), focuses[0].focused()), (Some("editor"), true));
    assert_eq!((focuses[1].window(), focuses[1].focused()), (Some("editor"), false));
    assert!(focuses[1].at() >= focuses[0].at());
}

#[test]
fn stage_guards_nest_cleanly_guard_reentrancy_and_recover_from_panic() {
    let seen = Rc::new(RefCell::new(Vec::<(FrameStage, StageBoundary, Option<u64>)>::new()));
    let sink = seen.clone();
    let mut app = App::new(spec()).on_frame_stage(move |e| {
        sink.borrow_mut().push((e.stage(), e.boundary(), e.seq()));
        // Re-entering emit from inside the callback must be ignored, never double-borrow or recurse.
        emit(FrameStage::Submit, StageBoundary::Begin);
    });
    let obs = HostObservability::new(app.observability.take(), Some("main"));

    {
        let _scope = obs.scope_stages(Some(7));
        let mut redraw = StageGuard::enter(FrameStage::Redraw);
        {
            let _dl = StageGuard::enter(FrameStage::DisplayLink);
        }
        {
            let _acq = StageGuard::enter(FrameStage::Acquire);
        }
        redraw.end();
        redraw.end(); // idempotent
    }
    assert!(!active(), "TLS active flag must clear after scope drops");
    assert_eq!(
        *seen.borrow(),
        [
            (FrameStage::Redraw, StageBoundary::Begin, Some(7)),
            (FrameStage::DisplayLink, StageBoundary::Begin, Some(7)),
            (FrameStage::DisplayLink, StageBoundary::End, Some(7)),
            (FrameStage::Acquire, StageBoundary::Begin, Some(7)),
            (FrameStage::Acquire, StageBoundary::End, Some(7)),
            (FrameStage::Redraw, StageBoundary::End, Some(7)),
        ]
    );

    // Panic recovery: hook returns to HostObservability and TLS resets.
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _scope = obs.scope_stages(Some(8));
        let _redraw = StageGuard::enter(FrameStage::Redraw);
        panic!("simulated stage panic");
    }));
    assert!(panicked.is_err());
    assert!(!active(), "TLS active flag must reset after panic unwind");
    // Redraw::End ran during unwind AND subsequent frames still receive events.
    {
        let _scope = obs.scope_stages(Some(9));
        let _p = StageGuard::enter(FrameStage::Present);
    }
    let tail: Vec<_> = seen.borrow().iter().skip(6).copied().collect();
    assert_eq!(
        tail,
        [
            (FrameStage::Redraw, StageBoundary::Begin, Some(8)),
            (FrameStage::Redraw, StageBoundary::End, Some(8)),
            (FrameStage::Present, StageBoundary::Begin, Some(9)),
            (FrameStage::Present, StageBoundary::End, Some(9)),
        ]
    );
}

#[test]
fn stage_seq_correlates_with_frame_encoded_and_closes_before_encoded_fires() {
    let timeline = Rc::new(RefCell::new(Vec::<String>::new()));
    let s_log = timeline.clone();
    let e_log = timeline.clone();
    let mut app = App::new(spec()).on_frame_stage(move |e| {
        s_log.borrow_mut().push(format!("stage:{:?}:{:?}:{:?}", e.stage(), e.boundary(), e.seq()));
    });
    let obs = HostObservability::new(app.observability.take(), Some("main"));
    let enc_hook: OnFrameEncoded = Box::new(move |f| {
        e_log.borrow_mut().push(format!("encoded:{:?}:{}", f.outcome(), f.seq()));
    });
    let mut reports = FrameReports::with_presented_seq(Some(enc_hook), obs.wants_stages(), Some("main"));

    // Frame 0: Drawn
    {
        let seq = reports.next_seq();
        let _scope = obs.scope_stages(seq);
        let mut redraw = StageGuard::enter(FrameStage::Redraw);
        { let _rt = StageGuard::enter(FrameStage::Runtime); }
        let stamp = reports.stamp();
        redraw.end();
        let drawn_seq = reports.drawn(stamp, None, crate::time::now(), 1.0);
        assert_eq!(drawn_seq, Some(0));
    }
    // Frame 1: Unchanged
    {
        let seq = reports.next_seq();
        let _scope = obs.scope_stages(seq);
        let mut redraw = StageGuard::enter(FrameStage::Redraw);
        redraw.end();
        reports.unchanged(None);
    }
    // Frame 2: Skipped
    {
        let seq = reports.next_seq();
        let _scope = obs.scope_stages(seq);
        let mut redraw = StageGuard::enter(FrameStage::Redraw);
        redraw.end();
        reports.skipped();
    }
    assert_eq!(
        *timeline.borrow(),
        [
            "stage:Redraw:Begin:Some(0)",
            "stage:Runtime:Begin:Some(0)",
            "stage:Runtime:End:Some(0)",
            "stage:Redraw:End:Some(0)",
            "encoded:Drawn:0",
            "stage:Redraw:Begin:Some(1)",
            "stage:Redraw:End:Some(1)",
            "encoded:Unchanged:1",
            "stage:Redraw:Begin:Some(2)",
            "stage:Redraw:End:Some(2)",
            "encoded:Skipped:2",
        ]
    );

    // Stage-only (without on_frame_encoded) still tracks seq without stamping encode clocks.
    let mut stage_only_reports = FrameReports::with_presented_seq(None, obs.wants_stages(), Some("main"));
    assert!(!stage_only_reports.wanted());
    assert_eq!(stage_only_reports.stamp(), None);
    assert_eq!(stage_only_reports.next_seq(), Some(0));
    assert_eq!(stage_only_reports.drawn(None, None, crate::time::now(), 0.0), Some(0));
    assert_eq!(stage_only_reports.next_seq(), Some(1));
    stage_only_reports.unchanged(None);
    assert_eq!(stage_only_reports.next_seq(), Some(2));
    stage_only_reports.skipped();
    assert_eq!(stage_only_reports.next_seq(), Some(3));
}

#[test]
fn runtime_frame_text_warm_glyph_atlas_and_system_fallback_emit_balanced_stages() {
    let events = Rc::new(RefCell::new(Vec::<FrameStageEvent>::new()));
    let sink = events.clone();
    let mut app = App::new(spec()).on_frame_stage(move |e| sink.borrow_mut().push(e.clone()));
    let obs = HostObservability::new(app.observability.take(), Some("main"));

    // 1. Startup TextSystem construction emits TextWarm with seq == None.
    let mut rt = {
        let _startup = obs.scope_stages(None);
        Runtime::new(0u32, |count, _| el().child(text(format!("hello {count}"))), (200.0, 80.0), Env::new(true, 1.0))
    };
    let startup_warm: Vec<_> = events.borrow().iter()
        .filter(|e| e.stage() == FrameStage::TextWarm)
        .map(|e| (e.boundary(), e.seq(), e.window().map(str::to_owned)))
        .collect();
    assert!(
        startup_warm.len() >= 2
            && startup_warm[0] == (StageBoundary::Begin, None, Some("main".into()))
            && startup_warm.last() == Some(&(StageBoundary::End, None, Some("main".into()))),
        "TextSystem startup must emit balanced TextWarm with seq=None: {startup_warm:?}"
    );

    // 2. First runtime frame emits View, Commit, Layout, Text, Paint, Apply, and GlyphAtlas.
    events.borrow_mut().clear();
    {
        let _frame = obs.scope_stages(Some(0));
        let _rt_guard = StageGuard::enter(FrameStage::Runtime);
        rt.frame(0.0);
    }
    let frame0 = events.borrow().clone();
    assert_balanced(&frame0, Some(0));
    for required in [
        FrameStage::Runtime,
        FrameStage::View,
        FrameStage::Commit,
        FrameStage::Layout,
        FrameStage::Text,
        FrameStage::Paint,
        FrameStage::GlyphAtlas,
        FrameStage::Apply,
    ] {
        assert!(
            frame0.iter().any(|e| e.stage() == required && e.boundary() == StageBoundary::Begin),
            "frame 0 must emit {required:?}: {frame0:?}"
        );
    }

    // 3. Cached second frame (same text, no new glyphs) does not emit GlyphAtlas during Paint.
    events.borrow_mut().clear();
    {
        let _frame = obs.scope_stages(Some(1));
        let _rt_guard = StageGuard::enter(FrameStage::Runtime);
        rt.frame(0.016);
    }
    let frame1 = events.borrow().clone();
    assert_balanced(&frame1, Some(1));
    assert!(
        !frame1.iter().any(|e| e.stage() == FrameStage::GlyphAtlas),
        "steady cached frame must not emit spurious GlyphAtlas misses: {frame1:?}"
    );

    // 4. Idle prepare_text warming emits TextWarm with seq == None.
    events.borrow_mut().clear();
    {
        let _idle = obs.scope_stages(None);
        let _added = rt.prepare_text(|_, _| el().child(text("warmed transcript line")), (200.0, 80.0));
    }
    let idle_events = events.borrow().clone();
    assert_balanced(&idle_events, None);
    assert!(
        idle_events.iter().any(|e| e.stage() == FrameStage::TextWarm && e.boundary() == StageBoundary::Begin),
        "prepare_text must emit TextWarm: {idle_events:?}"
    );

    // 5. System font fallback emits SystemFontFallback even after first-frame marks are closed.
    let _ = crate::marks::presented();
    events.borrow_mut().clear();
    {
        let _fallback_scope = obs.scope_stages(Some(2));
        let mut ts = crate::text::TextSystem::new(&[b""], ("Rune Missing Family", "Rune Missing Mono"));
        // Also trigger load_fallbacks with a non-Latin character (CJK).
        let _ = ts.layout_rich("\u{6F22}\u{5B57}", &[], crate::TextStyle::new(14.0, 400.0), 1.0, None);
    }
    let fb_events = events.borrow().clone();
    assert_balanced(&fb_events, Some(2));
    assert!(
        fb_events.iter().any(|e| e.stage() == FrameStage::SystemFontFallback && e.boundary() == StageBoundary::Begin),
        "system-font fallback must emit SystemFontFallback after first-frame marks close: {fb_events:?}"
    );
}

#[test]
fn headless_run_emits_correlated_frame_stages_and_frame_encoded() {
    if !crate::render::Gpu::test_adapter_available() {
        return;
    }
    let _turn = crate::tooling::headless_run::test_turn();
    let stages = Rc::new(RefCell::new(Vec::<FrameStageEvent>::new()));
    let encoded = Rc::new(RefCell::new(Vec::<(u64, FrameOutcome)>::new()));
    let s_sink = stages.clone();
    let e_sink = encoded.clone();
    let app = App::new(AppSpec::new("stage-headless", (32.0, 24.0), || (), |_, _| el().child(text("hi"))))
        .on_frame_stage(move |e| s_sink.borrow_mut().push(e.clone()))
        .on_frame_encoded(move |f| e_sink.borrow_mut().push((f.seq(), f.outcome())));
    let snap = std::env::temp_dir().join(format!("rune-stage-headless-{}.png", std::process::id()));
    crate::tooling::headless(
        app,
        crate::tooling::Args { snap: Some(snap.clone()), at: Some(0.05), fps: 30.0, scale: Some(1.0), ..Default::default() },
    );
    let _ = std::fs::remove_file(snap);

    let encoded = encoded.borrow();
    assert!(!encoded.is_empty());
    let stages = stages.borrow();
    assert!(stages.iter().any(|e| e.stage() == FrameStage::TextWarm && e.seq().is_none()));
    for &(seq, outcome) in encoded.iter() {
        assert_eq!(outcome, FrameOutcome::Drawn);
        let frame_events: Vec<_> = stages.iter().filter(|e| e.seq() == Some(seq)).cloned().collect();
        assert_balanced(&frame_events, Some(seq));
        for required in [FrameStage::Redraw, FrameStage::Runtime, FrameStage::Upload, FrameStage::Encode, FrameStage::Submit] {
            assert!(
                frame_events.iter().any(|e| e.stage() == required && e.boundary() == StageBoundary::Begin),
                "headless frame {seq} missing {required:?}: {frame_events:?}"
            );
        }
    }
}

fn assert_balanced(events: &[FrameStageEvent], expected_seq: Option<u64>) {
    let mut stack = Vec::new();
    for e in events {
        assert_eq!(e.seq(), expected_seq, "unexpected seq in {e:?}");
        match e.boundary() {
            StageBoundary::Begin => stack.push(e.stage()),
            StageBoundary::End => {
                let open = stack.pop();
                assert_eq!(open, Some(e.stage()), "unbalanced stage close {e:?} (open={open:?})");
            }
        }
    }
    assert!(stack.is_empty(), "unclosed stages: {stack:?}");
}
