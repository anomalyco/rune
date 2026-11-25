//! Tests for opt-in `reveal_fps` wake-rate capping on `El::reveal` word fades.
use super::*;
use crate::{Cycle, OverlayOptions, OverlaySpec, ScrollSpec, Spring, TextStyle, col, el, text};

#[derive(Clone, Default)]
struct StreamState {
    content: String,
    reveal: f32,
    reduced: bool,
    paused_node: bool,
    time_scale: f32,
    spinner: SpinnerMode,
    box_shifted: bool,
    scroll_rows: usize,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum SpinnerMode {
    #[default]
    None,
    /// Continuous display-rate opacity cycle (`Cycle::DISPLAY_RATE`).
    Continuous,
    /// Stepped 15 Hz opacity cycle (`fps: 15.0`).
    Stepped,
}

impl StreamState {
    fn new(content: &str, reveal: f32) -> Self {
        Self {
            content: content.to_owned(),
            reveal,
            reduced: false,
            paused_node: false,
            time_scale: 1.0,
            spinner: SpinnerMode::None,
            box_shifted: false,
            scroll_rows: 0,
        }
    }
}

const SPINNER_STOPS: &[(f32, f32)] = &[(0.0, 0.2), (0.5, 1.0), (1.0, 0.2)];

fn stream_view(s: &StreamState, cx: &mut Cx<StreamState>) -> El {
    cx.reduced_motion(s.reduced);
    cx.time_scale(s.time_scale);
    let mut t = text(s.content.clone())
        .key("t")
        .w_full()
        .wrap()
        .font(TextStyle::new(14.0, 400.0))
        .reveal(s.reveal);
    if s.paused_node {
        t = t.time_scale(0.0);
    }
    let mut root = col().key("root").w(300.0).h(240.0).child(t);
    match s.spinner {
        SpinnerMode::None => {}
        SpinnerMode::Continuous => {
            root = root.child(
                el().key("spin")
                    .size(16.0, 16.0)
                    .fill(crate::Color::WHITE)
                    .a11y_hidden()
                    .cycle(Cycle { period: 1.0, phase: 0.0, stops: SPINNER_STOPS, still: 0.5, fps: Cycle::DISPLAY_RATE }),
            );
        }
        SpinnerMode::Stepped => {
            root = root.child(
                el().key("spin")
                    .size(16.0, 16.0)
                    .fill(crate::Color::WHITE)
                    .a11y_hidden()
                    .cycle(Cycle { period: 1.0, phase: 0.0, stops: SPINNER_STOPS, still: 0.5, fps: 15.0 }),
            );
        }
    }
    if s.box_shifted {
        root = root.child(
            el().key("mover")
                .size(20.0, 20.0)
                .ml(80.0)
                .fill(crate::Color::WHITE)
                .motion(Spring::SMOOTH),
        );
    } else {
        root = root.child(
            el().key("mover")
                .size(20.0, 20.0)
                .ml(0.0)
                .fill(crate::Color::WHITE)
                .motion(Spring::SMOOTH),
        );
    }
    if s.scroll_rows > 0 {
        let list = col()
            .key("scroller")
            .w(280.0)
            .h(80.0)
            .scroll(ScrollSpec::default())
            .children((0..s.scroll_rows).map(|i| el().key(format!("row-{i}")).w_full().h(30.0)));
        root = root.child(list);
    }
    root
}

fn text_id(rt: &Runtime<StreamState>) -> NodeId {
    rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("t")).unwrap().id
}

fn last_word_alpha(rt: &Runtime<StreamState>) -> f32 {
    let id = text_id(rt);
    let byte = rt.state.content.len().saturating_sub(1) as u32;
    let now = rt.scene.store().node_time(id);
    rt.motion
        .reveals_map()
        .get(&id)
        .and_then(RevealPhase::fade)
        .map_or(1.0, |fade| fade.alpha(byte, now))
}

#[test]
fn reveal_fps_normalizes_and_clamps_on_runtime_and_overlay() {
    let mut rt = Runtime::new(StreamState::new("hello", 0.2), stream_view, (300.0, 240.0), Env::new(true, 1.0));
    assert_eq!(rt.reveal_fps(), 0.0, "default is uncapped display rate");

    for raw in [0.0, -0.0, -30.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        rt.set_reveal_fps(raw);
        assert_eq!(rt.reveal_fps(), 0.0, "raw {raw:?} normalizes to 0.0");
    }
    for raw in [f32::MIN_POSITIVE, 0.01, 0.5, 0.99, 1.0] {
        rt.set_reveal_fps(raw);
        assert_eq!(rt.reveal_fps(), 1.0, "small positive {raw} clamps to 1.0 Hz");
    }
    for raw in [24.0, 30.0, 40.0, 60.0, 120.0, 100_000.0] {
        rt.set_reveal_fps(raw);
        assert_eq!(rt.reveal_fps(), raw, "finite positive >= 1.0 is preserved");
    }

    let overlay_default = OverlaySpec::new("o", (200.0, 100.0), |_: &(), _| el())
        .options(OverlayOptions::default());
    assert_eq!(overlay_default.runtime.reveal_fps, 0.0);

    let overlay_capped = OverlaySpec::new("o", (200.0, 100.0), |_: &(), _| el())
        .options(OverlayOptions::default())
        .reveal_fps(30.0);
    assert_eq!(overlay_capped.runtime.reveal_fps, 30.0);

    let overlay_clamped = OverlaySpec::new("o", (200.0, 100.0), |_: &(), _| el())
        .options(OverlayOptions::default())
        .reveal_fps(0.25);
    assert_eq!(overlay_clamped.runtime.reveal_fps, 1.0);

    let overlay_nan = OverlaySpec::new("o", (200.0, 100.0), |_: &(), _| el())
        .options(OverlayOptions::default())
        .reveal_fps(f32::NAN);
    assert_eq!(overlay_nan.runtime.reveal_fps, 0.0);
}

#[test]
fn default_reveal_fps_zero_keeps_display_rate_and_anim_source_bit_identical() {
    let mut rt = Runtime::new(StreamState::new("start", 0.2), stream_view, (300.0, 240.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.reveal_fps(), 0.0);
    assert_eq!(rt.wake_deadline(), None);

    rt.update(|s, _| s.content = "start one two three four five six".to_owned());
    let first = rt.frame(0.0);
    assert!(first, "default reveal_fps(0.0) keeps animating == true");
    assert_ne!(rt.motion.anim_sources & AnimSource::REVEAL, 0, "AnimSource::REVEAL must be set");
    assert_eq!(rt.wake_deadline(), None, "uncapped reveal arms no timer wake");

    let mut uncapped_frames = 1usize;
    while rt.frame(1.0 / 120.0) {
        assert_ne!(rt.motion.anim_sources & AnimSource::REVEAL, 0);
        assert_eq!(rt.wake_deadline(), None);
        uncapped_frames += 1;
        assert!(uncapped_frames < 200, "reveal must settle");
    }
    assert!(uncapped_frames >= 24, "0.2 s reveal at 120 Hz takes >= 24 frames, got {uncapped_frames}");
    assert_eq!(rt.motion.anim_sources & AnimSource::REVEAL, 0);
    assert_eq!(rt.wake_deadline(), None);
    assert!(!rt.motion.has_fading_reveals(&rt.scene));
    assert_eq!(last_word_alpha(&rt), 1.0);
}

#[test]
fn capped_reveal_fps_paces_reveal_only_tail_on_timer_wakes_and_clears_when_done() {
    let mut uncapped = Runtime::new(StreamState::new("start", 0.2), stream_view, (300.0, 240.0), Env::new(true, 1.0));
    uncapped.frame(0.0);
    uncapped.update(|s, _| s.content = "start one two three four five six".to_owned());
    uncapped.frame(0.0);
    let mut uncapped_frames = 1usize;
    while uncapped.frame(1.0 / 120.0) {
        uncapped_frames += 1;
    }

    let mut rt = Runtime::new(StreamState::new("start", 0.2), stream_view, (300.0, 240.0), Env::new(true, 1.0));
    rt.set_reveal_fps(30.0);
    rt.frame(0.0);
    assert_eq!(rt.wake_deadline(), None);

    rt.update(|s, _| s.content = "start one two three four five six".to_owned());
    let animating = rt.frame(0.0);
    assert!(!animating, "reveal-only tail with reveal_fps(30.0) must release display link");
    assert_eq!(rt.motion.anim_sources & AnimSource::REVEAL, 0, "REVEAL source stays unset when timer-paced alone");
    let due = rt.wake_deadline().expect("reveal_fps(30.0) arms a timer wake");
    assert!((due - (rt.clock + 1.0 / 30.0)).abs() < 1e-9, "deadline {due} vs {}", rt.clock + 1.0 / 30.0);
    assert_eq!(rt.core().waiting_work(), Some("a reveal"));

    let commits_before = rt.stats.commits;
    let mut prev_alpha = last_word_alpha(&rt);
    let mut saw_progress = false;
    let mut capped_frames = 1usize;
    while let Some(next_due) = rt.wake_deadline() {
        let dt = (next_due - rt.clock).max(0.0);
        assert!((dt - 1.0 / 30.0).abs() < 1e-9, "step interval {dt}");
        let anim = rt.frame(dt);
        assert!(!anim, "reveal-only wake must not re-engage display link");
        capped_frames += 1;
        let alpha = last_word_alpha(&rt);
        assert!(alpha >= prev_alpha - 1e-6, "word alpha must advance monotonically: {prev_alpha} -> {alpha}");
        if alpha > prev_alpha + 1e-4 {
            saw_progress = true;
        }
        prev_alpha = alpha;
        assert!(capped_frames < 50, "capped reveal must settle");
    }
    assert!(saw_progress, "word alpha must advance across 1/30 s steps");
    assert_eq!(prev_alpha, 1.0, "all words reach full alpha");
    assert_eq!(rt.stats.commits, commits_before, "stepping a reveal must perform zero view rebuilds");
    assert_eq!(rt.wake_deadline(), None, "settled reveal clears wake_deadline");
    assert_eq!(rt.core().waiting_work(), None);
    assert!(!rt.motion.has_fading_reveals(&rt.scene));
    assert!(
        capped_frames * 2 < uncapped_frames,
        "30 Hz capped reveal ({capped_frames} frames) must wake far fewer times than 120 Hz ({uncapped_frames} frames)"
    );
    // An idle frame after settling schedules nothing.
    assert!(!rt.frame(1.0 / 30.0));
    assert_eq!(rt.wake_deadline(), None);
}

#[test]
fn capped_reveal_fps_supports_custom_40hz_huge_rate_and_time_scale() {
    // 40 Hz caller preference.
    let mut rt = Runtime::new(StreamState::new("a", 0.2), stream_view, (300.0, 240.0), Env::new(true, 1.0));
    rt.set_reveal_fps(40.0);
    rt.frame(0.0);
    rt.update(|s, _| s.content = "a b c d".to_owned());
    assert!(!rt.frame(0.0));
    let due = rt.wake_deadline().expect("40 Hz wake");
    assert!((due - (rt.clock + 1.0 / 40.0)).abs() < 1e-9);

    // Huge rate finishes cleanly without stalling.
    let mut huge = Runtime::new(StreamState::new("a", 0.1), stream_view, (300.0, 240.0), Env::new(true, 1.0));
    huge.set_reveal_fps(100_000.0);
    huge.frame(0.0);
    huge.update(|s, _| s.content = "a b c".to_owned());
    assert!(!huge.frame(0.0));
    assert!(huge.wake_deadline().is_some());
    huge.frame(0.5);
    assert_eq!(huge.wake_deadline(), None);
    assert_eq!(last_word_alpha(&huge), 1.0);

    // Non-unit time_scale keeps wall wake at 1/30 s while scaling animation time.
    let mut scaled = Runtime::new(StreamState::new("a", 0.2), stream_view, (300.0, 240.0), Env::new(true, 1.0));
    scaled.set_reveal_fps(30.0);
    scaled.frame(0.0);
    scaled.update(|s, _| {
        s.time_scale = 0.5;
        s.content = "a b c".to_owned();
    });
    assert!(!scaled.frame(0.0));
    let due = scaled.wake_deadline().expect("scaled wake");
    assert!((due - (scaled.clock + 1.0 / 30.0)).abs() < 1e-9, "wall wake interval stays 1/30 s");
    let t0 = scaled.t;
    scaled.frame(1.0 / 30.0);
    assert!((scaled.t - (t0 + 0.5 / 30.0)).abs() < 1e-9, "animation time advances by dt * time_scale");
}

#[test]
fn concurrent_tracks_scroll_or_loop_ride_at_full_rate_and_block_cycle_fast_paths() {
    // 1. Concurrent TRACKS (spring move).
    let mut rt = Runtime::new(StreamState::new("a", 0.2), stream_view, (300.0, 240.0), Env::new(true, 1.0));
    rt.set_reveal_fps(30.0);
    rt.frame(0.0);
    rt.update(|s, _| {
        s.content = "a b c d e".to_owned();
        s.box_shifted = true;
    });
    assert!(rt.frame(0.0), "concurrent spring keeps animating == true");
    assert_ne!(rt.motion.anim_sources & AnimSource::TRACKS, 0);
    assert_ne!(rt.motion.anim_sources & AnimSource::REVEAL, 0, "REVEAL rides along with TRACKS");
    assert_eq!(rt.wake_deadline(), None, "active display-rate motion needs no reveal timer");

    // 2. Concurrent SCROLL (wheel momentum / glide).
    let mut scroll_rt = Runtime::new(
        StreamState { scroll_rows: 20, ..StreamState::new("a", 0.2) },
        stream_view,
        (300.0, 240.0),
        Env::new(true, 1.0),
    );
    scroll_rt.set_reveal_fps(30.0);
    scroll_rt.frame(0.0);
    scroll_rt.update(|s, tx| {
        s.content = "a b c d e".to_owned();
        tx.scroll_to("scroller", 0.0, 120.0);
    });
    assert!(scroll_rt.frame(0.0), "concurrent scroll glide keeps animating == true");
    assert_ne!(scroll_rt.motion.anim_sources & AnimSource::SCROLL, 0);
    assert_ne!(scroll_rt.motion.anim_sources & AnimSource::REVEAL, 0, "REVEAL rides along with SCROLL");

    // 3. Concurrent continuous LOOP (Cycle::DISPLAY_RATE): rides at full rate and blocks loop_step.
    let mut loop_rt = Runtime::new(
        StreamState { spinner: SpinnerMode::Continuous, ..StreamState::new("a", 0.2) },
        stream_view,
        (300.0, 240.0),
        Env::new(true, 1.0),
    );
    loop_rt.set_reveal_fps(30.0);
    loop_rt.wall_start = Some(crate::time::now());
    loop_rt.frame(0.0);
    loop_rt.update(|s, _| s.content = "a b c d e".to_owned());
    assert!(loop_rt.frame(0.0), "continuous loop keeps animating == true");
    assert_ne!(loop_rt.motion.anim_sources & AnimSource::LOOP, 0);
    assert_ne!(loop_rt.motion.anim_sources & AnimSource::REVEAL, 0, "REVEAL rides along with LOOP");
    assert_eq!(
        loop_rt.core_mut().loop_step(1.0 / 120.0),
        Err("a reveal"),
        "loop_step must refuse cycle-only fast path while a reveal is fading"
    );

    // Once the reveal finishes, continuous spinner can take loop_step again.
    for _ in 0..40 {
        loop_rt.frame(1.0 / 60.0);
    }
    assert!(!loop_rt.motion.has_fading_reveals(&loop_rt.scene));
    assert_eq!(loop_rt.core().waiting_work(), None);
    assert!(
        loop_rt.core_mut().loop_step(1.0 / 120.0).is_ok(),
        "loop_step resumes once reveal finishes"
    );

    // 4. Concurrent stepped Cycle (15 Hz): neither loop_step nor paint_cycle_tick can skip the reveal.
    let mut stepped_rt = Runtime::new(
        StreamState { spinner: SpinnerMode::Stepped, ..StreamState::new("a", 0.2) },
        stream_view,
        (300.0, 240.0),
        Env::new(true, 1.0),
    );
    stepped_rt.set_reveal_fps(30.0);
    stepped_rt.frame(0.0);
    stepped_rt.update(|s, _| s.content = "a b c d e".to_owned());
    assert!(!stepped_rt.frame(0.0));
    assert_eq!(stepped_rt.core().waiting_work(), Some("a reveal"));
    // Advance test clock past the wake deadline and verify paint_cycle_tick refuses.
    let due = stepped_rt.wake_deadline().expect("wake armed");
    stepped_rt.clock = due + 0.05;
    assert!(!stepped_rt.core_mut().paint_cycle_tick(), "paint_cycle_tick must not skip a pending reveal");
    assert!(stepped_rt.core_mut().loop_step(1.0 / 30.0).is_err(), "loop_step must not skip a pending reveal");
}

#[test]
fn reveal_fps_respects_reduced_motion_paused_time_scale_hidden_and_empty_text() {
    // 1. reveal(0.0) under reduced motion: instant, zero wakes.
    let mut zero_rm = Runtime::new(
        StreamState { reduced: true, ..StreamState::new("a", 0.0) },
        stream_view,
        (300.0, 240.0),
        Env::new(true, 1.0),
    );
    zero_rm.set_reveal_fps(30.0);
    zero_rm.frame(0.0);
    zero_rm.update(|s, _| s.content = "a b c d".to_owned());
    assert!(!zero_rm.frame(0.0));
    assert_eq!(zero_rm.wake_deadline(), None);
    assert_eq!(last_word_alpha(&zero_rm), 1.0);

    // 2. Positive reveal(0.30) under reduced motion caps at <= 0.15 s and clears wake_deadline.
    let mut pos_rm = Runtime::new(
        StreamState { reduced: true, ..StreamState::new("a", 0.30) },
        stream_view,
        (300.0, 240.0),
        Env::new(true, 1.0),
    );
    pos_rm.set_reveal_fps(30.0);
    pos_rm.frame(0.0);
    pos_rm.update(|s, _| s.content = "a b c d e f".to_owned());
    assert!(!pos_rm.frame(0.0));
    assert!(pos_rm.wake_deadline().is_some());
    let start = pos_rm.clock;
    while let Some(due) = pos_rm.wake_deadline() {
        pos_rm.frame((due - pos_rm.clock).max(0.0));
        assert!(pos_rm.clock - start <= 0.20, "reduced-motion fade must cap at <= 0.15 s");
    }
    assert_eq!(pos_rm.wake_deadline(), None);
    assert_eq!(last_word_alpha(&pos_rm), 1.0);

    // 3. Paused runtime time_scale(0.0): arms no wake and does not block resting.
    let mut paused = Runtime::new(
        StreamState { time_scale: 0.0, ..StreamState::new("a", 0.2) },
        stream_view,
        (300.0, 240.0),
        Env::new(true, 1.0),
    );
    paused.set_reveal_fps(30.0);
    paused.frame(0.0);
    paused.update(|s, _| s.content = "a b c d".to_owned());
    assert!(!paused.frame(0.0));
    assert_eq!(paused.wake_deadline(), None);
    assert_eq!(paused.core().waiting_work(), None);

    // 4. Paused text node (El::time_scale(0.0)): arms no wake.
    let mut node_paused = Runtime::new(
        StreamState { paused_node: true, ..StreamState::new("a", 0.2) },
        stream_view,
        (300.0, 240.0),
        Env::new(true, 1.0),
    );
    node_paused.set_reveal_fps(30.0);
    node_paused.frame(0.0);
    node_paused.update(|s, _| s.content = "a b c d".to_owned());
    assert!(!node_paused.frame(0.0));
    assert_eq!(node_paused.wake_deadline(), None);
    assert_eq!(node_paused.core().waiting_work(), None);

    // 5. Hidden window (!live_visible()): arms no wake.
    let mut hidden = Runtime::new(StreamState::new("a", 0.2), stream_view, (300.0, 240.0), Env::new(true, 1.0));
    hidden.set_reveal_fps(30.0);
    hidden.frame(0.0);
    hidden.set_live_visible(false);
    hidden.update(|s, _| s.content = "a b c d".to_owned());
    assert!(!hidden.frame(0.0));
    assert_eq!(hidden.wake_deadline(), None);

    // 6. Empty text: arms no wake.
    let mut empty = Runtime::new(StreamState::new("", 0.2), stream_view, (300.0, 240.0), Env::new(true, 1.0));
    empty.set_reveal_fps(30.0);
    assert!(!empty.frame(0.0));
    assert_eq!(empty.wake_deadline(), None);
}

#[test]
fn capped_reveal_streaming_passes_motion_gate() {
    let mut rt = Runtime::new(StreamState::new("", 0.18), stream_view, (300.0, 240.0), Env::new(true, 1.0));
    rt.set_reveal_fps(30.0);
    let mut run = crate::motion_gate::Run::new(rt, 30.0);
    run.idle(0.1);
    run.step();
    run.act("chunk-1", |rt| {
        rt.update(|s, _| s.content = "Streaming words arrive calmly at thirty frames per second.".to_owned());
    });
    run.play(0.5);
    run.act("chunk-2", |rt| {
        rt.update(|s, _| s.content.push_str(" Then a second chunk fades in and settles cleanly to rest."));
    });
    run.play(0.6);
    run.quiet();
    run.play(0.2);
    let findings = crate::motion_gate::check(&run.rec);
    assert!(findings.is_empty(), "capped reveal streaming must pass motion_gate cleanly: {findings:?}");
    assert_eq!(run.rt.wake_deadline(), None);
}
