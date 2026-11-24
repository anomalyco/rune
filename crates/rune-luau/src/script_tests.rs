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

/// `onScroll` delivers `kind = "scroll"`, `key`, `x`, `y`, `maxX`, `maxY`, and clamped `fy`,
/// deduplicates unchanged `(offset, max)` across re-renders, and keeps idle scripts out of the VM.
#[test]
fn on_scroll_delivers_offsets_and_deduplicates_rerenders() {
    let src = r#"
local ui = require("@rune/ui")
local calls = 0
local bumps = 0
local last = nil
return {
    render = function()
        local children = {}
        for i = 1, 10 do
            children[i] = ui.box({ key = `row-{i}`, w = 200, h = 50 })
        end
        return ui.col({ w = 200, h = 140 }, {
            ui.row({ key = "bump", w = 200, h = 40, onPress = function() bumps += 1 end }, { ui.text(`{bumps}`) }),
            ui.col({
                key = "feed",
                w = 200,
                h = 100,
                scrollY = true,
                onScroll = function(e)
                    calls += 1
                    last = e
                end,
            }, children),
        })
    end,
    state = function()
        return { calls = calls, bumps = bumps, last = last }
    end,
}
"#;
    let script = Script::new("on_scroll_basic", Sources::single(src));
    assert!(script.error().is_none(), "start: {:?}", script.error());
    let mut rt = Runtime::new(script, |s: &Script, cx: &mut Cx<Script>| s.view(cx), (200.0, 140.0), Env::new(true, 1.0));
    rt.poll = Some(|s, now, tx| s.poll(now, tx));
    rt.frame(0.0);
    // Next frame delivers the initial layout scroll snapshot via poll.
    rt.frame(0.016);
    assert!(rt.state.log().is_empty(), "unexpected logs: {:?}", rt.state.log());

    let s0 = rt.state.query("state", []).expect("state");
    assert_eq!(s0.get("calls").as_i64(), Some(1), "initial layout delivers once");
    assert_eq!(s0.get("last").get("kind").as_str(), Some("scroll"));
    assert_eq!(s0.get("last").get("key").as_str(), Some("feed"));
    assert_eq!(s0.get("last").get("x").as_f64(), Some(0.0));
    assert_eq!(s0.get("last").get("y").as_f64(), Some(0.0));
    assert_eq!(s0.get("last").get("maxX").as_f64(), Some(0.0));
    assert_eq!(s0.get("last").get("maxY").as_f64(), Some(400.0));
    assert_eq!(s0.get("last").get("fy").as_f64(), Some(0.0));

    // Idle frames must not re-fire onScroll or enter the VM.
    let vm_idle = rt.state.metrics().vm_calls;
    for _ in 0..10 {
        assert!(!rt.frame(0.016), "idle scroll view must not ask for frames");
    }
    assert_eq!(rt.state.metrics().vm_calls, vm_idle, "idle frames must not enter VM");

    // An unrelated re-render attaches a new on_scroll_offset closure with unchanged (offset, max):
    // deduplication must prevent onScroll from re-firing.
    let (bx, by) = rt.find("bump").expect("bump button");
    rt.pointer_down(bx, by);
    rt.pointer_up(bx, by);
    rt.frame(0.016);
    rt.frame(0.016);
    let s1 = rt.state.query("state", []).expect("state after bump");
    assert_eq!(s1.get("bumps").as_i64(), Some(1));
    assert_eq!(s1.get("calls").as_i64(), Some(1), "unchanged re-render must not re-fire onScroll");

    // Scrolling the viewport delivers the updated y and clamped fy.
    rt.update(|_, tx| {
        tx.scroll_to_y("feed", 100.0);
    });
    rt.frame(0.016);
    rt.frame(0.016);
    let s2 = rt.state.query("state", []).expect("state after scroll");
    assert_eq!(s2.get("calls").as_i64(), Some(2));
    assert_eq!(s2.get("last").get("y").as_f64(), Some(100.0));
    assert_eq!(s2.get("last").get("maxY").as_f64(), Some(400.0));
    let fy = s2.get("last").get("fy").as_f64().expect("fy");
    assert!((fy - 0.25).abs() < 1e-4, "expected fy=0.25, got {fy}");

    // Scrolling to the bottom clamps fy to 1.0 and settles idle.
    rt.update(|_, tx| {
        tx.scroll_to_y("feed", 400.0);
    });
    rt.frame(0.016);
    rt.frame(0.016);
    let s3 = rt.state.query("state", []).expect("state at bottom");
    assert_eq!(s3.get("calls").as_i64(), Some(3));
    assert_eq!(s3.get("last").get("y").as_f64(), Some(400.0));
    assert_eq!(s3.get("last").get("fy").as_f64(), Some(1.0));

    let vm_settled = rt.state.metrics().vm_calls;
    for _ in 0..30 {
        if !rt.frame(0.016) {
            break;
        }
    }
    for _ in 0..10 {
        assert!(!rt.frame(0.016), "settled scroll view must not ask for frames");
    }
    assert_eq!(rt.state.metrics().vm_calls, vm_settled, "settled frames must not enter VM");
}

/// When `maxY == 0`, `fy` is `0`, keyless scroll views omit `key`, and horizontal `scrollX`
/// reports `x` and `maxX`.
#[test]
fn on_scroll_zero_max_fy_keyless_and_horizontal() {
    let src = r#"
local ui = require("@rune/ui")
local flat = nil
local horiz = nil
return {
    render = function()
        return ui.col({ w = 200, h = 200 }, {
            ui.col({
                w = 200,
                h = 100,
                scrollY = true,
                onScroll = function(e) flat = e end,
            }, { ui.box({ w = 200, h = 40 }) }),
            ui.row({
                key = "strip",
                w = 200,
                h = 100,
                scrollX = true,
                onScroll = function(e) horiz = e end,
            }, { ui.box({ w = 500, h = 100 }) }),
        })
    end,
    state = function()
        return { flat = flat, horiz = horiz }
    end,
}
"#;
    let script = Script::new("on_scroll_edges", Sources::single(src));
    assert!(script.error().is_none(), "start: {:?}", script.error());
    let mut rt = Runtime::new(script, |s: &Script, cx: &mut Cx<Script>| s.view(cx), (200.0, 200.0), Env::new(true, 1.0));
    rt.poll = Some(|s, now, tx| s.poll(now, tx));
    rt.frame(0.0);
    rt.frame(0.016);

    let s0 = rt.state.query("state", []).expect("state");
    assert_eq!(s0.get("flat").get("kind").as_str(), Some("scroll"));
    assert_eq!(s0.get("flat").get("key"), &crate::Data::Nil, "keyless scroll view omits key");
    assert_eq!(s0.get("flat").get("maxY").as_f64(), Some(0.0));
    assert_eq!(s0.get("flat").get("fy").as_f64(), Some(0.0), "fy must be 0 when maxY == 0");

    assert_eq!(s0.get("horiz").get("key").as_str(), Some("strip"));
    assert_eq!(s0.get("horiz").get("maxX").as_f64(), Some(300.0));
    assert_eq!(s0.get("horiz").get("maxY").as_f64(), Some(0.0));
    assert_eq!(s0.get("horiz").get("fy").as_f64(), Some(0.0));

    rt.update(|_, tx| {
        tx.scroll_to_x("strip", 120.0);
    });
    rt.frame(0.016);
    rt.frame(0.016);
    let s1 = rt.state.query("state", []).expect("state after scroll_x");
    assert_eq!(s1.get("horiz").get("x").as_f64(), Some(120.0));
    assert_eq!(s1.get("horiz").get("maxX").as_f64(), Some(300.0));
    assert_eq!(s1.get("horiz").get("fy").as_f64(), Some(0.0));
}

/// Multiple scroll moves before `poll` coalesce per viewport (newest `ScrollPos` wins), returning
/// to the last delivered offset before `poll` drops the pending event, and `Script::reload` resets
/// scroll tracking.
#[test]
fn on_scroll_coalesces_moves_before_poll_and_clears_on_reload() {
    let src = r#"
local ui = require("@rune/ui")
local calls = 0
local lastY = -1
return {
    render = function()
        local children = {}
        for i = 1, 10 do
            children[i] = ui.box({ key = `r-{i}`, w = 200, h = 50 })
        end
        return ui.col({ w = 200, h = 100 }, {
            ui.col({
                key = "scroller",
                w = 200,
                h = 100,
                scrollY = true,
                onScroll = function(e)
                    calls += 1
                    lastY = e.y
                end,
            }, children),
        })
    end,
    state = function()
        return { calls = calls, lastY = lastY }
    end,
}
"#;
    let script = Script::new("on_scroll_coalesce", Sources::single(src));
    assert!(script.error().is_none(), "start: {:?}", script.error());
    let mut rt = Runtime::new(script, |s: &Script, cx: &mut Cx<Script>| s.view(cx), (200.0, 100.0), Env::new(true, 1.0));
    // Drive poll manually so we can perform multiple scroll moves before a single poll.
    rt.frame(0.0);
    assert!(rt.update_if(|s, tx| s.poll(0.016, tx)), "initial layout scroll queued");
    rt.frame(0.016);
    assert_eq!(rt.state.query("state", []).expect("s0").get("calls").as_i64(), Some(1));

    // Move twice before polling: only the newest position (180) must be delivered in one call.
    rt.update(|_, tx| { tx.scroll_to_y("scroller", 60.0); });
    rt.frame(0.016);
    rt.update(|_, tx| { tx.scroll_to_y("scroller", 180.0); });
    rt.frame(0.016);
    assert!(rt.update_if(|s, tx| s.poll(0.048, tx)));
    let s1 = rt.state.query("state", []).expect("s1");
    assert_eq!(s1.get("calls").as_i64(), Some(2), "two moves before poll must coalesce into one callback");
    assert_eq!(s1.get("lastY").as_f64(), Some(180.0), "newest scroll position wins");

    // Moving away and back to 180 before poll cancels the pending notification.
    rt.update(|_, tx| { tx.scroll_to_y("scroller", 240.0); });
    rt.frame(0.016);
    rt.update(|_, tx| { tx.scroll_to_y("scroller", 180.0); });
    rt.frame(0.016);
    assert!(!rt.update_if(|s, tx| s.poll(0.080, tx)), "net-zero scroll before poll must not wake script");
    assert_eq!(rt.state.query("state", []).expect("s2").get("calls").as_i64(), Some(2));

    // Reloading clears per-viewport tracking so the fresh VM receives the current scroll state.
    rt.state.reload(Sources::single(src)).expect("reload");
    rt.invalidate();
    rt.frame(0.016);
    assert!(rt.update_if(|s, tx| s.poll(0.096, tx)), "reloaded script receives fresh scroll state");
    let s3 = rt.state.query("state", []).expect("s3");
    assert_eq!(s3.get("calls").as_i64(), Some(1));
    assert_eq!(s3.get("lastY").as_f64(), Some(180.0));
}

/// `onScroll` works on `ui.virtualList` and rejects non-function values on both containers and
/// virtual lists.
#[test]
fn on_scroll_works_on_virtual_list_and_validates_type() {
    let src = r#"
local ui = require("@rune/ui")
local last = nil
local calls = 0
return {
    render = function()
        return ui.col({ w = 200, h = 100 }, {
            ui.virtualList({
                key = "vlist",
                w = 200,
                h = 100,
                count = 30,
                estimatedRowHeight = 30,
                renderRow = function(i)
                    return ui.box({ w = 200, h = 30 }, { ui.text(`row {i}`) })
                end,
                onScroll = function(e)
                    calls += 1
                    last = e
                end,
            }),
        })
    end,
    state = function()
        return { calls = calls, last = last }
    end,
}
"#;
    let script = Script::new("on_scroll_vlist", Sources::single(src));
    assert!(script.error().is_none(), "start: {:?}", script.error());
    let mut rt = Runtime::new(script, |s: &Script, cx: &mut Cx<Script>| s.view(cx), (200.0, 100.0), Env::new(true, 1.0));
    rt.poll = Some(|s, now, tx| s.poll(now, tx));
    rt.frame(0.0);
    rt.frame(0.016);
    assert!(rt.state.log().is_empty(), "unexpected logs: {:?}", rt.state.log());
    let s0 = rt.state.query("state", []).expect("s0");
    assert_eq!(s0.get("calls").as_i64(), Some(1));
    assert_eq!(s0.get("last").get("key").as_str(), Some("vlist"));
    assert!(s0.get("last").get("maxY").as_f64().unwrap_or(0.0) > 0.0);

    rt.update(|_, tx| { tx.scroll_to_y("vlist", 150.0); });
    rt.frame(0.016);
    rt.frame(0.016);
    let s1 = rt.state.query("state", []).expect("s1");
    assert_eq!(s1.get("calls").as_i64(), Some(2));
    assert_eq!(s1.get("last").get("y").as_f64(), Some(150.0));
    assert!(s1.get("last").get("fy").as_f64().unwrap_or(0.0) > 0.0);

    // Non-function onScroll on a container or virtualList reports a type error.
    for (name, bad_src) in [
        ("bad_col", r#"local ui = require("@rune/ui") return { render = function() return ui.col({ scrollY = true, onScroll = 42 }) end }"#),
        ("bad_vlist", r#"local ui = require("@rune/ui") return { render = function() return ui.virtualList({ count = 1, renderRow = function() return ui.box({}) end, onScroll = "nope" }) end }"#),
    ] {
        let bad = Script::new(name, Sources::single(bad_src));
        let _ = bad.render_uncached::<()>("render", true);
        assert!(
            bad.log().iter().any(|l| l.message.contains("onScroll must be a function")),
            "{name} must log type error, got {:?}",
            bad.log()
        );
    }
}

/// `ScrollState` bounds both its last-delivered and pending maps at `TRACK_CAP`.
#[test]
fn scroll_state_respects_track_cap() {
    let mut state = crate::scroll::ScrollState::default();
    for i in 0..(crate::scroll::TRACK_CAP + 32) {
        let path: Box<str> = format!("root/scroll-{i}").into();
        let snap = crate::scroll::ScrollSnapshot { offset: [0.0, i as f32], max: [0.0, 1000.0] };
        state.record_last(path, snap);
    }
    assert_eq!(state.last_len(), crate::scroll::TRACK_CAP);
    assert_eq!(state.pending_len(), 0);
}

/// `view.reducedMotion` and `NodeArgs::reduced_motion()` follow `rt.set_system_reduced_motion`,
/// rebuild nested `view:Memo` and `ui.each` subtrees when reduced motion flips, and hit memo
/// without entering the VM when unchanged.
#[test]
fn reduced_motion_reaches_view_node_args_and_invalidates_memos() {
    use std::cell::Cell;
    use std::rc::Rc;

    let seen_host = Rc::new(Cell::new(None::<bool>));
    let seen_host_clone = seen_host.clone();
    let probe = crate::HostNode::new("motionProbe", move |args| {
        seen_host_clone.set(Some(args.reduced_motion()));
        Ok(rune::text(if args.reduced_motion() { "reduced" } else { "full" }))
    });

    let src = r#"
local ui = require("@rune/ui")
local memoBuilds = 0
local eachBuilds = 0
local lastReduced = nil
local items = { { id = "a", v = 1 } }
return {
    render = function(view)
        lastReduced = view.reducedMotion
        return ui.col({ w = 200, h = 100 }, {
            view:Memo("card", 1, function()
                memoBuilds += 1
                return ui.motionProbe({ key = "probe" })
            end),
            ui.col({}, ui.each(items, "id", "v", function()
                eachBuilds += 1
                return ui.text(if view.reducedMotion then "rm-on" else "rm-off")
            end)),
        })
    end,
    state = function()
        return { reduced = lastReduced, memoBuilds = memoBuilds, eachBuilds = eachBuilds }
    end,
}
"#;
    let script = Script::with_options("reduced_motion", Sources::single(src), crate::ScriptOptions::new().node(probe));
    assert!(script.error().is_none(), "start: {:?}", script.error());
    let mut rt = Runtime::new(script, |s: &Script, cx: &mut Cx<Script>| s.view(cx), (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(rt.state.log().is_empty(), "unexpected logs: {:?}", rt.state.log());

    let s0 = rt.state.query("state", []).expect("s0");
    assert_eq!(s0.get("reduced"), &crate::Data::Bool(false));
    assert_eq!(s0.get("memoBuilds").as_i64(), Some(1));
    assert_eq!(s0.get("eachBuilds").as_i64(), Some(1));
    assert_eq!(seen_host.get(), Some(false));

    // `query` bumps script version, so let one commit settle before checking steady memo reuse.
    rt.invalidate();
    rt.frame(0.016);
    let calls_before = rt.state.metrics().vm_calls;
    rt.invalidate();
    rt.frame(0.016);
    assert_eq!(rt.state.metrics().vm_calls, calls_before, "unchanged reduced motion must hit memo");

    // Enabling system reduced motion invalidates the surface, view:Memo, ui.each, and NodeArgs.
    rt.set_system_reduced_motion(true);
    rt.frame(0.016);
    let s1 = rt.state.query("state", []).expect("s1");
    assert_eq!(s1.get("reduced"), &crate::Data::Bool(true));
    assert_eq!(s1.get("memoBuilds").as_i64(), Some(2), "view:Memo must rebuild on reducedMotion flip");
    assert_eq!(s1.get("eachBuilds").as_i64(), Some(2), "ui.each must rebuild on reducedMotion flip");
    assert_eq!(seen_host.get(), Some(true), "NodeArgs::reduced_motion() must reflect true");

    // Staying in reduced motion reuses memo without entering the VM.
    rt.invalidate();
    rt.frame(0.016);
    let calls_rm = rt.state.metrics().vm_calls;
    rt.invalidate();
    rt.frame(0.016);
    assert_eq!(rt.state.metrics().vm_calls, calls_rm, "steady reduced motion must hit memo");

    // Turning system reduced motion back off rebuilds once more.
    rt.set_system_reduced_motion(false);
    rt.frame(0.016);
    let s2 = rt.state.query("state", []).expect("s2");
    assert_eq!(s2.get("reduced"), &crate::Data::Bool(false));
    assert_eq!(s2.get("memoBuilds").as_i64(), Some(3));
    assert_eq!(s2.get("eachBuilds").as_i64(), Some(3));
    assert_eq!(seen_host.get(), Some(false));
}

/// A view's per-commit `cx.reduced_motion(true)` override reaches `view.reducedMotion`,
/// `NodeArgs::reduced_motion()`, and invalidates nested `view:Memo` subtrees.
#[test]
fn cx_reduced_motion_override_updates_view_and_node_args() {
    use std::cell::Cell;
    use std::rc::Rc;

    struct Host {
        script: Script,
        override_reduced: bool,
    }

    let seen_host = Rc::new(Cell::new(None::<bool>));
    let seen_host_clone = seen_host.clone();
    let probe = crate::HostNode::new("motionProbe", move |args| {
        seen_host_clone.set(Some(args.reduced_motion()));
        Ok(rune::text("ok"))
    });

    let src = r#"
local ui = require("@rune/ui")
local memoBuilds = 0
local lastReduced = nil
return {
    render = function(view)
        lastReduced = view.reducedMotion
        return view:Memo("root", 1, function()
            memoBuilds += 1
            return ui.motionProbe({ key = "probe" })
        end)
    end,
    state = function()
        return { reduced = lastReduced, memoBuilds = memoBuilds }
    end,
}
"#;
    let script = Script::with_options("cx_reduced", Sources::single(src), crate::ScriptOptions::new().node(probe));
    let mut rt = Runtime::new(
        Host { script, override_reduced: false },
        |s: &Host, cx: &mut Cx<Host>| {
            cx.reduced_motion(s.override_reduced);
            s.script.view(cx)
        },
        (200.0, 100.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    assert_eq!(rt.state.script.query("state", []).expect("s0").get("reduced"), &crate::Data::Bool(false));
    assert_eq!(seen_host.get(), Some(false));

    rt.update(|s, _| s.override_reduced = true);
    rt.frame(0.016);
    let s1 = rt.state.script.query("state", []).expect("s1");
    assert_eq!(s1.get("reduced"), &crate::Data::Bool(true));
    assert_eq!(s1.get("memoBuilds").as_i64(), Some(2));
    assert_eq!(seen_host.get(), Some(true));
}
