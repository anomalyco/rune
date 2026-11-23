//! Scheduler clock synchronization when `Script::poll` returns early with no task due.

use rune::{Cx, Env, Runtime};

use crate::{Script, Sources};

/// Headless virtual-clock frames with no task due still advance the scheduler clock, so a press
/// handler calling `task.wait(0.2)` waits 0.2 s from the press frame instead of resuming at once
/// against a stale clock.
#[test]
fn task_wait_after_idle_virtual_clock_frames_counts_from_press_frame() {
    let src = r#"
local ui = require("@rune/ui")
local started = 0
local finished = 0
local waited = -1
return {
    render = function()
        return ui.col({}, {
            ui.row({
                key = "btn",
                w = 100,
                h = 40,
                onPress = function()
                    started += 1
                    waited = task.wait(0.2)
                    finished += 1
                end,
            }, { ui.text("Go") }),
        })
    end,
    state = function()
        return { started = started, finished = finished, waited = waited }
    end,
}
"#;
    let script = Script::new("wait_after_idle", Sources::single(src));
    assert!(script.error().is_none(), "start: {:?}", script.error());

    let mut rt = Runtime::new(script, |s: &Script, cx: &mut Cx<Script>| s.view(cx), (200.0, 100.0), Env::new(true, 1.0));
    rt.poll = Some(|s, now, tx| s.poll(now, tx));
    rt.frame(0.0);

    // Advance 2.0 s of virtual time with no task due; idle polls must not enter the VM.
    let calls_before = rt.state.metrics().vm_calls;
    for _ in 0..20 {
        rt.frame(0.1);
    }
    assert_eq!(rt.state.metrics().vm_calls, calls_before, "idle polls must not enter the VM");

    let (x, y) = rt.find("btn").expect("button laid out");
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.01);

    let after_press = rt.state.query("state", []).expect("query state");
    assert_eq!(after_press.get("started").as_i64(), Some(1), "press handler started");
    assert_eq!(after_press.get("finished").as_i64(), Some(0), "task.wait(0.2) is still parked right after press");

    // 0.15 s later on the virtual clock: still before the 0.2 s deadline.
    for _ in 0..3 {
        rt.frame(0.05);
    }
    let before_due = rt.state.query("state", []).expect("query state");
    assert_eq!(before_due.get("finished").as_i64(), Some(0), "task must not resume before 0.2 s has passed");

    // 0.21 s after the press: past the 0.2 s deadline, so the task resumes.
    rt.frame(0.06);
    let after_due = rt.state.query("state", []).expect("query state");
    assert_eq!(after_due.get("finished").as_i64(), Some(1), "task resumes once 0.2 s has passed");
    let waited = after_due.get("waited").as_f64().expect("waited seconds");
    assert!((0.19..=0.25).contains(&waited), "expected ~0.21 s waited, got {waited}");
}

/// After an earlier task finishes and the script sits idle on a virtual clock, a later
/// `task.wait` and `task.delay` count from the current frame rather than from the earlier task.
#[test]
fn task_wait_and_delay_after_earlier_task_count_from_current_frame() {
    let src = r#"
local first_done = 0
local second_done = 0
local delayed_done = 0
return {
    first = function()
        task.wait(0.1)
        first_done += 1
    end,
    second = function()
        task.delay(0.2, function()
            delayed_done += 1
        end)
        task.wait(0.2)
        second_done += 1
    end,
    state = function()
        return { first = first_done, second = second_done, delayed = delayed_done }
    end,
}
"#;
    let script = Script::new("wait_after_task", Sources::single(src));
    assert!(script.error().is_none(), "start: {:?}", script.error());

    let mut rt = Runtime::new(script, |_: &Script, _: &mut Cx<Script>| rune::el(), (100.0, 100.0), Env::new(true, 1.0));
    rt.poll = Some(|s, now, tx| s.poll(now, tx));
    rt.frame(0.0);

    // Run and complete an initial task around t = 0.2 s.
    rt.state.call("first", []).expect("call first");
    rt.frame(0.2);
    assert_eq!(rt.state.query("state", []).expect("state").get("first").as_i64(), Some(1));

    // Sit idle for 2.0 s of virtual time (now = 2.2 s) with no tasks due.
    for _ in 0..20 {
        rt.frame(0.1);
    }

    // Start a second task waiting 0.2 s; its deadline must be ~2.4 s, not ~0.4 s.
    rt.state.call("second", []).expect("call second");
    rt.frame(0.1);
    let mid = rt.state.query("state", []).expect("state");
    assert_eq!(mid.get("second").as_i64(), Some(0), "second task.wait(0.2) must not resume at +0.1 s");
    assert_eq!(mid.get("delayed").as_i64(), Some(0), "task.delay(0.2) must not resume at +0.1 s");

    rt.frame(0.15);
    let done = rt.state.query("state", []).expect("state");
    assert_eq!(done.get("second").as_i64(), Some(1), "second task.wait(0.2) resumes after +0.25 s");
    assert_eq!(done.get("delayed").as_i64(), Some(1), "task.delay(0.2) resumes after +0.25 s");
}
