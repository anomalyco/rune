use std::time::Duration;

use super::*;
use crate::el::{col, row};
use crate::runtime::Runtime;
use crate::{Cx, El, Env, GrowthSpec, ScrollSpec};

fn list(key: &'static str, rows: usize) -> El {
    col().size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).child(
        col().key(key).w_full().growth(GrowthSpec::default()).virtualize(20.0)
            .children((0..rows).map(|i| row().key(format!("r{i}")).w_full().h(20.0))),
    )
}

#[test]
fn a_virtualize_list_that_builds_far_more_rows_than_it_mounts_warns_once() {
    let _clock = FakeStallClock::new();
    take_log();
    fn view(_: &u32, _: &mut Cx<u32>) -> El { list("overbuilt-rows", 1000) }
    let mut rt = Runtime::new(0u32, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|n, _| *n += 1);
    rt.frame(1.0 / 60.0);
    let log = take_log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].contains("overbuilt-rows") && log[0].contains("virtual_rows") && log[0].contains("built 1000 rows"), "{}", log[0]);
}

#[test]
fn a_short_virtualize_list_stays_quiet() {
    let _clock = FakeStallClock::new();
    take_log();
    fn view(_: &u32, _: &mut Cx<u32>) -> El { list("short-rows", 150) }
    let mut rt = Runtime::new(0u32, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(take_log(), Vec::<String>::new());
}

#[test]
fn reveal_selected_on_virtual_rows_warns_once() {
    let _clock = FakeStallClock::new();
    take_log();
    fn view(_: &u32, _: &mut Cx<u32>) -> El {
        col().size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).child(
            col().key("vr-reveal").w_full().growth(GrowthSpec::default()).reveal_selected(true)
                .virtual_rows(100, 20.0, |_: &u32, i| format!("r{i}").into(), |n: &u32, i| {
                    row().w_full().h(20.0).focus_selected(i == *n as usize)
                }),
        )
    }
    let mut rt = Runtime::new(0u32, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|n, _| *n = 60);
    rt.frame(1.0 / 60.0);
    let log = take_log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(
        log[0].contains("vr-reveal")
            && log[0].contains("scroll_into_view")
            && log[0].contains("tests.rs")
            && !log[0].contains("virtual_rows.rs"),
        "{}",
        log[0]
    );
}

#[test]
fn virtual_rows_without_reveal_selected_and_virtualize_with_reveal_selected_stay_quiet() {
    let _clock = FakeStallClock::new();
    take_log();
    fn vr_view(_: &u32, _: &mut Cx<u32>) -> El {
        col().size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).child(
            col().key("vr-quiet").w_full().growth(GrowthSpec::default())
                .virtual_rows(100, 20.0, |_: &u32, i| format!("r{i}").into(), |n: &u32, i| {
                    row().w_full().h(20.0).focus_selected(i == *n as usize)
                }),
        )
    }
    let mut rt = Runtime::new(0u32, vr_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|n, _| *n = 60);
    rt.frame(1.0 / 60.0);
    assert_eq!(take_log(), Vec::<String>::new());

    fn vz_view(n: &u32, _: &mut Cx<u32>) -> El {
        let sel = *n as usize;
        col().size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).child(
            col().key("vz-reveal").w_full().growth(GrowthSpec::default()).virtualize(20.0).reveal_selected(true)
                .children((0..50).map(|i| row().key(format!("r{i}")).w_full().h(20.0).focus_selected(i == sel))),
        )
    }
    let mut rt = Runtime::new(0u32, vz_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|n, _| *n = 40);
    rt.frame(1.0 / 60.0);
    assert_eq!(take_log(), Vec::<String>::new());
}

#[test]
fn a_callback_blocked_past_its_cpu_time_warns_once_per_site() {
    let _clock = FakeStallClock::new();
    take_log();
    for _ in 0..2 {
        let stall = Stall::start();
        FakeStallClock::advance(30.0, 0.0, Some(0.0));
        stall.end(|| ("test-blocked".into(), "`poll`".into()));
    }
    let log = take_log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].starts_with("rune: `poll` blocked ") && log[0].contains("on the main thread: move IO off it with `rune::Latest` or a thread"), "{}", log[0]);
}

#[test]
fn a_callback_that_computes_is_not_a_stall() {
    let _clock = FakeStallClock::new();
    take_log();
    let stall = Stall::start();
    FakeStallClock::advance(30.0, 30.0, Some(0.0));
    stall.end(|| ("test-computing".into(), "`poll`".into()));
    let quick = Stall::start();
    quick.end(|| ("test-quick".into(), "`poll`".into()));
    assert_eq!(take_log(), Vec::<String>::new());
}

#[test]
fn a_blocking_poll_names_poll() {
    let _clock = FakeStallClock::new();
    take_log();
    fn view(_: &u32, _: &mut Cx<u32>) -> El { col() }
    let mut rt = Runtime::new(0u32, view, (100.0, 100.0), Env::new(true, 1.0));
    rt.poll = Some(|_, _, _| { FakeStallClock::advance(30.0, 0.0, Some(0.0)); false });
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    let log = take_log();
    assert!(log.len() == 1 && log[0].starts_with("rune: `poll` blocked "), "{log:?}");
}

#[test]
fn a_blocking_update_names_update() {
    let _clock = FakeStallClock::new();
    take_log();
    fn view(_: &u32, _: &mut Cx<u32>) -> El { col() }
    let mut rt = Runtime::new(0u32, view, (100.0, 100.0), Env::new(true, 1.0));
    rt.update(|_, _| FakeStallClock::advance(30.0, 0.0, Some(0.0)));
    rt.update(|_, _| FakeStallClock::advance(30.0, 0.0, Some(0.0)));
    let log = take_log();
    assert!(log.len() == 1 && log[0].starts_with("rune: an `update` closure blocked "), "{log:?}");
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[test]
fn a_first_blocking_callback_before_cpu_rate_is_known_still_warns() {
    take_log();
    let stall = Stall::start();
    std::thread::sleep(Duration::from_millis(30));
    stall.end(|| ("first-blocked-real".into(), "`init`".into()));
    let log = take_log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].starts_with("rune: `init` blocked "), "{}", log[0]);
}

#[test]
fn fast_updates_do_not_probe_the_run_queue() {
    fn view(_: &u32, _: &mut Cx<u32>) -> El { col() }
    let mut rt = Runtime::new(0u32, view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let _ = runq::take_probe_calls();
    let mut slow = 0;
    for _ in 0..100 {
        let started = crate::time::now();
        rt.update(|n, _| *n += 1);
        if started.elapsed().as_secs_f64() * 1000.0 > STALL_MS { slow += 1; }
    }
    assert!(slow < 100, "at least one update finished within STALL_MS");
    assert!(runq::take_probe_calls() <= slow, "updates within STALL_MS must not probe the run queue");

    if let Some(mark) = crate::frame::profile::thread_cpu_mark() {
        end_slow(STALL_MS, mark, &mut || ("test-slow-probe".into(), "`update`".into()));
        assert_eq!(runq::take_probe_calls(), 1, "judging a slow callback consults the load probe");
    }
}

#[test]
fn a_slow_callback_without_a_cpu_reading_does_not_invent_a_stall() {
    let _clock = FakeStallClock::new();
    take_log();
    let stall = Stall::start();
    FakeStallClock::advance_without_cpu(15.0);
    stall.end(|| ("test-no-cpu".into(), "`poll`".into()));
    assert_eq!(take_log(), Vec::<String>::new());
}

#[test]
fn rows_of_one_list_share_a_handler_site() {
    assert_eq!(handler_site("on_press", "#0/list/r12"), handler_site("on_press", "#0/list/r7"));
    assert_ne!(handler_site("on_press", "#0/list/r12"), handler_site("on_hover", "#0/list/r12"));
}

fn titled<S: 'static>(state: S, view: fn(&S, &mut Cx<S>) -> El, title: &str, poll: fn(&mut S, f64, &mut crate::Tx) -> bool) -> Runtime<S> {
    let mut rt = Runtime::new(state, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.host.window_title.start(title);
    rt.poll = Some(poll);
    rt
}

fn still(_: &u32, _: &mut Cx<u32>) -> El {
    col().size(200.0, 200.0).child(row().size(20.0, 20.0).fill(crate::Color::WHITE))
}

#[test]
fn a_poll_that_always_says_changed_but_changes_nothing_warns_after_30_frames() {
    let _clock = FakeStallClock::new();
    take_log();
    let mut rt = titled(0u32, still, "poll-noop", |n, _, _| { *n += 1; true });
    for _ in 0..29 { rt.frame(1.0 / 60.0); }
    assert_eq!(take_log(), Vec::<String>::new(), "not before 30 frames that change nothing");
    for _ in 0..40 { rt.frame(1.0 / 60.0); }
    let log = take_log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].starts_with("rune: `poll` of window \"poll-noop\" returned true but nothing changed")
        && log[0].contains("return true only when visible state changed"), "{}", log[0]);
}

#[test]
fn a_frame_whose_poll_does_not_say_changed_breaks_the_run() {
    let _clock = FakeStallClock::new();
    take_log();
    // 29 quiet identical frames whose `poll` returns true, one whose `poll` returns false, one
    // more that returns true: the false frame ends the run, so 30 in a row never happened.
    let mut rt = titled(0u32, still, "poll-gap", |n, _, _| { *n += 1; *n != 31 });
    for _ in 0..32 { rt.frame(1.0 / 60.0); }
    assert_eq!(take_log(), Vec::<String>::new(), "a frame whose `poll` returned false breaks the run");
    // The run starts over on the other side of the gap: 29 more true frames stay quiet, the
    // 30th warns as before.
    for _ in 0..28 { rt.frame(1.0 / 60.0); }
    assert_eq!(take_log(), Vec::<String>::new(), "not before 30 frames in a row after the gap");
    rt.frame(1.0 / 60.0);
    let log = take_log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].contains("returned true but nothing changed (30 frames in a row)"), "{}", log[0]);
}

#[test]
fn a_poll_that_changes_what_shows_stays_quiet() {
    let _clock = FakeStallClock::new();
    take_log();
    fn view(n: &u32, _: &mut Cx<u32>) -> El {
        col().size(200.0, 200.0).child(crate::el::text(format!("tick {n}")))
    }
    let mut rt = titled(0u32, view, "poll-real", |n, _, tx| { *n += 1; tx.wake_after(Duration::from_millis(16)); true });
    for _ in 0..300 { rt.frame(1.0 / 60.0); }
    assert_eq!(take_log(), Vec::<String>::new());
}

#[test]
fn an_endless_16_ms_wake_after_loop_on_an_idle_window_warns() {
    let _clock = FakeStallClock::new();
    take_log();
    let mut rt = titled(0u32, still, "idle-loop", |_, _, tx| { tx.wake_after(Duration::from_millis(16)); false });
    for _ in 0..100 { rt.frame(1.0 / 60.0); }
    assert_eq!(take_log(), Vec::<String>::new(), "not before 120 frames");
    for _ in 0..100 { rt.frame(1.0 / 60.0); }
    let log = take_log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].starts_with("rune: idle window \"idle-loop\" still drawing: check `wake_after` loops (120 frames in ")
        && log[0].contains("woken by a `wake_after` loop)"), "{}", log[0]);
}

#[test]
fn a_wake_after_loop_that_always_says_changed_names_poll_too() {
    let _clock = FakeStallClock::new();
    take_log();
    let mut rt = titled(0u32, still, "idle-poll-loop", |_, _, tx| { tx.wake_after(Duration::from_millis(16)); true });
    for _ in 0..200 { rt.frame(1.0 / 60.0); }
    let log = take_log();
    assert_eq!(log.len(), 2, "{log:?}");
    assert!(log[0].contains("`poll` of window \"idle-poll-loop\" returned true"), "{}", log[0]);
    assert!(log[1].contains("woken by a `wake_after` loop whose `poll` returns true"), "{}", log[1]);
}

#[test]
fn a_wake_after_loop_that_stops_stays_quiet() {
    let _clock = FakeStallClock::new();
    take_log();
    let mut rt = titled(0u32, still, "loop-stops", |n, _, tx| {
        *n += 1;
        if *n < 100 { tx.wake_after(Duration::from_millis(16)); }
        false
    });
    for _ in 0..400 { rt.frame(1.0 / 60.0); }
    assert_eq!(take_log(), Vec::<String>::new());
}

#[test]
fn a_wake_after_loop_under_running_motion_stays_quiet() {
    let _clock = FakeStallClock::new();
    take_log();
    fn view(_: &u32, _: &mut Cx<u32>) -> El {
        col().size(200.0, 200.0).child(row().size(20.0, 20.0).fill(crate::Color::WHITE)
            .looping(rune_motion::LoopSpec::opacity(1.0, 0.0, 0.2, 1.0)))
    }
    let mut rt = titled(0u32, view, "loop-moving", |_, _, tx| { tx.wake_after(Duration::from_millis(16)); false });
    for _ in 0..300 { rt.frame(1.0 / 60.0); }
    assert_eq!(take_log(), Vec::<String>::new());

    fn spring(x: &f32, _: &mut Cx<f32>) -> El {
        col().size(200.0, 200.0).child(row().key("box").size(20.0, 20.0).absolute(*x, 0.0).fill(crate::Color::WHITE)
            .motion(rune_motion::Spring::new(3.0, 0.0)))
    }
    let mut rt = titled(0.0f32, spring, "spring-moving", |_, clock, tx| {
        if clock < 2.5 { tx.wake_after(Duration::from_millis(16)); }
        false
    });
    rt.frame(0.0);
    rt.update(|x, _| *x = 150.0);
    let moving = (0..150).filter(|_| rt.frame(1.0 / 60.0)).count();
    assert!(moving >= 140, "the spring runs through the loop: {moving}");
    for _ in 0..100 { rt.frame(1.0 / 60.0); }
    assert_eq!(take_log(), Vec::<String>::new());
}

#[test]
fn a_slow_launch_names_its_slowest_part_and_rune_s_own_time_stays_quiet() {
    assert_eq!(launch::report(5.0, 40.0, 20.0, 400.0), None, "Rune's own window and GPU time doesn't count");
    let message = launch::report(10.0, 180.0, 12.0, 260.0).unwrap();
    assert_eq!(message, "first frame took 260 ms (setup 10 ms, init 180 ms, first view 12 ms, window and GPU 58 ms); \
        the slowest is `init`: start slow work from `on_ready`");
    assert!(launch::report(150.0, 0.0, 0.0, 200.0).unwrap().contains("the slowest is setup in `main` before `run`"));
}

#[test]
fn a_memo_over_a_differently_keyed_root_warns_once_per_site() {
    let _clock = FakeStallClock::new();
    take_log();
    fn view(_: &u32, _: &mut Cx<u32>) -> El {
        let same = crate::memo("same", 0, || col().key("same"));
        let renamed = crate::memo("outer", 0, || col().key("inner"));
        col().child(same).child(renamed)
    }
    let mut rt = Runtime::new(0u32, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|n, _| *n += 1);
    rt.frame(1.0 / 60.0);
    let log = take_log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].contains("\"outer\"") && log[0].contains("\"inner\""), "{}", log[0]);
    assert!(rt.find("outer").is_some() && rt.find("inner").is_none());
}

#[test]
fn a_memo_id_reused_in_one_build_warns_once_per_site() {
    let _clock = FakeStallClock::new();
    take_log();
    let dup_line = line!() + 4;
    fn view(_: &u32, _: &mut Cx<u32>) -> El {
        col().children([
            col().key("a").child(crate::memo("dup", 0, || col().key("dup"))),
            col().key("b").child(crate::memo("dup", 0, || col().key("dup"))),
        ])
    }
    let mut rt = Runtime::new(0u32, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|n, _| *n += 1);
    rt.frame(1.0 / 60.0);
    let log = take_log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(
        log[0].contains("\"dup\"")
            && log[0].contains("uncached")
            && log[0].contains(&format!("{}:{dup_line}:", file!()))
            && !log[0].contains("memo.rs")
            && log[0].contains("RUNE_PERF_WARN=0"),
        "{}",
        log[0]
    );
}

#[test]
fn a_memo_id_reused_in_a_loop_warns_once_and_distinct_sites_warn_separately() {
    let _clock = FakeStallClock::new();
    take_log();
    fn view(_: &u32, _: &mut Cx<u32>) -> El {
        let mut children: Vec<El> = (0..4)
            .map(|i| col().key(format!("r{i}")).child(crate::memo("loop-dup", 0, || col())))
            .collect();
        children.push(col().key("extra").child(crate::memo("loop-dup", 0, || col().key("other"))));
        col().children(children)
    }
    let mut rt = Runtime::new(0u32, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|n, _| *n += 1);
    rt.frame(1.0 / 60.0);
    let log = take_log();
    // 1 warning for the loop call site, 1 for the extra call site's id collision, and 1 for the
    // extra call site's root key replacement (proving memo_id_reused and memo_key_replaced use
    // distinct deduplication keys).
    assert_eq!(log.len(), 3, "{log:?}");
    assert_eq!(log.iter().filter(|m| m.contains("already used in this build")).count(), 2, "{log:?}");
    assert_eq!(log.iter().filter(|m| m.contains("replaced its root's key")).count(), 1, "{log:?}");
}

#[test]
fn memo_id_reused_respects_rune_perf_warn_zero() {
    use std::ffi::OsStr;
    assert!(enabled_for(None));
    assert!(enabled_for(Some(OsStr::new("1"))));
    assert!(!enabled_for(Some(OsStr::new("0"))));

    let _clock = FakeStallClock::new();
    take_log();
    fn view(_: &u32, _: &mut Cx<u32>) -> El {
        col().children([
            col().key("a").child(crate::memo("silenced-dup", 0, || col())),
            col().key("b").child(crate::memo("silenced-dup", 0, || col())),
        ])
    }
    let mut rt = Runtime::new(0u32, view, (200.0, 200.0), Env::new(true, 1.0));
    {
        let _off = FakeEnabled::new(enabled_for(Some(OsStr::new("0"))));
        rt.frame(0.0);
        assert_eq!(take_log(), Vec::<String>::new(), "RUNE_PERF_WARN=0 silences memo_id_reused");
    }
    {
        let _on = FakeEnabled::new(true);
        rt.update(|n, _| *n += 1);
        rt.frame(1.0 / 60.0);
        let log = take_log();
        assert_eq!(log.len(), 1, "being silenced once does not suppress future enabled runs: {log:?}");
        assert!(log[0].contains("\"silenced-dup\""), "{}", log[0]);
    }
}

#[test]
fn equal_memo_ids_under_distinct_enclosing_memos_stay_quiet_and_same_scope_warns() {
    let _clock = FakeStallClock::new();
    take_log();
    fn quiet_view(_: &u32, _: &mut Cx<u32>) -> El {
        col().children([
            crate::memo("first", 0, || col().child(crate::memo("header", 0, || col()))),
            crate::memo("second", 0, || col().child(crate::memo("header", 0, || col()))),
        ])
    }
    let mut rt = Runtime::new(0u32, quiet_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|n, _| *n += 1);
    rt.frame(1.0 / 60.0);
    assert_eq!(take_log(), Vec::<String>::new());

    let nested_line = line!() + 5;
    fn nested_dup_view(_: &u32, _: &mut Cx<u32>) -> El {
        crate::memo("outer-scope", 0, || {
            col().children([
                col().key("a").child(crate::memo("header", 0, || col())),
                col().key("b").child(crate::memo("header", 0, || col())),
            ])
        })
    }
    let mut rt = Runtime::new(0u32, nested_dup_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let log = take_log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(
        log[0].contains("\"outer-scope/header\"")
            && log[0].contains(&format!("{}:{nested_line}:", file!())),
        "{}",
        log[0]
    );
}

#[test]
fn memo_with_hash_and_region_id_reuse_warn_at_caller_sites() {
    let _clock = FakeStallClock::new();
    take_log();
    let hash_line = line!() + 5;
    let region_line = line!() + 6;
    fn view(_: &u32, _: &mut Cx<u32>) -> El {
        col().children([
            col().key("h1").child(crate::memo::memo_with_hash("hash-dup", 1, || col())),
            col().key("h2").child(crate::memo::memo_with_hash("hash-dup", 1, || col())),
            col().key("r1").child(crate::memo::region("region-dup", || col())),
            col().key("r2").child(crate::memo::region("region-dup", || col())),
        ])
    }
    let mut rt = Runtime::new(0u32, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let log = take_log();
    assert_eq!(log.len(), 2, "{log:?}");
    assert!(
        log[0].contains("\"hash-dup\"")
            && log[0].contains(&format!("{}:{hash_line}:", file!()))
            && !log[0].contains("memo.rs"),
        "{}",
        log[0]
    );
    assert!(
        log[1].contains("\"region-dup\"")
            && log[1].contains(&format!("{}:{region_line}:", file!()))
            && !log[1].contains("region.rs")
            && !log[1].contains("memo.rs"),
        "{}",
        log[1]
    );
}
