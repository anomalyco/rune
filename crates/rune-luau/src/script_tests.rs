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



/// The eight added WAI-ARIA 1.2 roles map onto Rune's AccessKit roles and unknown roles still fail.
#[test]
fn extended_aria_roles_reach_accessibility_tree_and_unknown_role_errors() {
    let src = r#"
local ui = require("@rune/ui")
return {
    render = function()
        return ui.col({ w = 300, h = 400 }, {
            ui.box({ key = "r_slider", role = "slider", label = "Volume", w = 100, h = 20 }),
            ui.box({ key = "r_spin", role = "spinbutton", label = "Count", w = 100, h = 20 }),
            ui.box({ key = "r_textbox", role = "textbox", label = "Notes", w = 100, h = 20 }),
            ui.box({ key = "r_progress", role = "progressbar", label = "Loading", w = 100, h = 20 }),
            ui.box({ key = "r_dialog", role = "dialog", label = "Confirm", w = 100, h = 20 }),
            ui.box({ key = "r_tablist", role = "tablist", label = "Sections", w = 100, h = 20 }),
            ui.box({ key = "r_combobox", role = "combobox", label = "Country", w = 100, h = 20 }),
            ui.box({ key = "r_tooltip", role = "tooltip", label = "Hint", w = 100, h = 20 }),
        })
    end,
}
"#;
    let script = Script::new("roles", Sources::single(src));
    assert!(script.error().is_none(), "start: {:?}", script.error());
    let mut rt = Runtime::new(script, |s: &Script, cx: &mut Cx<Script>| s.view(cx), (320.0, 420.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(rt.state.log().is_empty(), "unexpected logs: {:?}", rt.state.log());

    let elements = rt.elements();
    let tree = rt.accessibility_json();
    // `textbox` without a native text-input wraps a keyed box as a `group` in AccessKit while
    // preserving the Rune element's `"textbox"` role.
    let expected = [
        ("r_slider", "slider", "slider"),
        ("r_spin", "spinbutton", "spin-button"),
        ("r_textbox", "textbox", "group"),
        ("r_progress", "progressbar", "progress-indicator"),
        ("r_dialog", "dialog", "dialog"),
        ("r_tablist", "tablist", "tab-list"),
        ("r_combobox", "combobox", "combo-box"),
        ("r_tooltip", "tooltip", "tooltip"),
    ];
    let role_in_a11y = |key: &str| -> Option<&str> {
        let mut stack = vec![&tree];
        while let Some(node) = stack.pop() {
            if node.get("key").and_then(|k| k.as_str()) == Some(key) {
                return node.get("role").and_then(|r| r.as_str());
            }
            if let Some(children) = node.get("children").and_then(|c| c.as_array()) {
                stack.extend(children);
            }
        }
        None
    };
    for (key, rune_role, a11y_role) in expected {
        let el = elements.iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("missing element {key}"));
        assert_eq!(el.role, rune_role, "element {key} Rune role");
        assert_eq!(role_in_a11y(key), Some(a11y_role), "a11y role for {key} in {tree}");
    }

    let bad = Script::new(
        "bad_role",
        Sources::single(r#"local ui = require("@rune/ui") return { render = function() return ui.box({ role = "widget" }) end }"#),
    );
    let _ = bad.render_uncached::<()>("render", true);
    assert!(
        bad.log().iter().any(|l| l.message.contains("unknown role widget")),
        "unknown role must log an error, got {:?}",
        bad.log()
    );
}

/// `matched` inside `view:Memo` still records and animates shared-element flights.
#[test]
fn matched_animates_inside_view_memo() {
    let src = r#"
local ui = require("@rune/ui")
local side = 0
return {
    render = function(view)
        return ui.col({ w = 300, h = 120, gap = 8 }, {
            ui.row({ key = "swap", w = 80, h = 24, onPress = function() side = 1 - side end }, { "Swap" }),
            view:Memo("pair", side, function()
                local pill = ui.box({ key = "pill", matched = "memo-pill", w = 50, h = 30, fill = "indicator" })
                return ui.row({ w = 300, h = 40, justify = "between" }, {
                    ui.box({ key = "left", w = 60, h = 40 }, { side == 0 and pill }),
                    ui.box({ key = "right", w = 60, h = 40 }, { side == 1 and pill }),
                })
            end),
        })
    end,
}
"#;
    let script = Script::new("memo_matched", Sources::single(src));
    let mut rt = Runtime::new(script, |s: &Script, cx: &mut Cx<Script>| s.view(cx), (320.0, 140.0), Env::new(true, 1.0));
    rt.poll = Some(|s, now, tx| s.poll(now, tx));
    rt.frame(0.0);
    let start_x = rt.presented("pill").expect("pill start").0[0];
    let dest_x = rt.presented("right").expect("right slot").0[0];

    let (bx, by) = rt.find("swap").expect("swap");
    rt.pointer_down(bx, by);
    rt.pointer_up(bx, by);
    rt.frame(0.016);
    assert!(!rt.matched_handovers().is_empty(), "matched inside view:Memo should record handover");
    rt.frame(0.05);
    let mid_x = rt.presented("pill").expect("pill mid").0[0];
    assert!(mid_x > start_x + 1.0 && mid_x < dest_x - 1.0, "matched inside view:Memo should fly: {start_x} -> {mid_x} -> {dest_x}");
}
