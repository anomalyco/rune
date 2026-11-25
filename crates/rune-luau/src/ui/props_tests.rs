//! Tests for `ui.*` layout, transform, outline, clip, line-clamp, and matched-flight props.

use std::cell::RefCell;
use std::rc::Rc;

use rune::{Cx, Env, Runtime};

use crate::{LogLevel, Script, ScriptOptions, Sources};

fn render_runtime(src: &str, viewport: (f32, f32)) -> (Runtime<Script>, Rc<RefCell<Vec<(LogLevel, String)>>>) {
    let logged = Rc::new(RefCell::new(Vec::new()));
    let sink = logged.clone();
    let options = ScriptOptions::new().on_log(move |level, msg| sink.borrow_mut().push((level, msg.to_owned())));
    let script = Script::with_options("props_test", Sources::single(src), options);
    assert!(script.error().is_none(), "start error: {:?}", script.error());
    let mut rt = Runtime::new(script, |s: &Script, cx: &mut Cx<Script>| s.view(cx), viewport, Env::new(true, 1.0));
    rt.poll = Some(|s, now, tx| s.poll(now, tx));
    rt.frame(0.0);
    (rt, logged)
}

fn render_errors(src: &str) -> Vec<String> {
    let (rt, logged) = render_runtime(src, (300.0, 200.0));
    drop(rt);
    logged.borrow().iter().filter(|(l, _)| *l == LogLevel::Error).map(|(_, m)| m.clone()).collect()
}

fn rect_of(rt: &Runtime<Script>, key: &str) -> [f32; 4] {
    rt.presented(key).unwrap_or_else(|| panic!("missing element {key:?}")).0
}

#[test]
fn max_lines_clamps_wrapped_text_and_zero_disables_clamping() {
    let src = r#"
local ui = require("@rune/ui")
local body = "one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen"
local CLAMPED = table.freeze({ w = 120, maxLines = 2 })
return {
    render = function()
        return ui.col({ w = 200, gap = 8 }, {
            ui.text(body, { key = "unclamped", w = 120, wrap = true }),
            ui.text(body, { key = "clamped", w = 120, maxLines = 2 }),
            ui.text(body, { key = "zero", w = 120, maxLines = 0 }),
            ui.text(body, CLAMPED, { key = "override_zero", maxLines = 0 }),
            ui.text("مرحبا بكم في عالم التصميم الحديث للنصوص العربية الطويلة جدا هنا", { key = "rtl_full", w = 100, dir = "rtl", wrap = true }),
            ui.text("مرحبا بكم في عالم التصميم الحديث للنصوص العربية الطويلة جدا هنا", { key = "rtl_clamped", w = 100, dir = "rtl", maxLines = 1 }),
        })
    end,
}
"#;
    let (rt, logged) = render_runtime(src, (240.0, 400.0));
    assert!(logged.borrow().is_empty(), "unexpected logs: {:?}", logged.borrow());

    let h_unclamped = rect_of(&rt, "unclamped")[3];
    let h_clamped = rect_of(&rt, "clamped")[3];
    let h_zero = rect_of(&rt, "zero")[3];
    let h_override = rect_of(&rt, "override_zero")[3];
    assert!(h_clamped > 0.0 && h_clamped < h_unclamped, "expected maxLines=2 ({h_clamped}) < unclamped ({h_unclamped})");
    assert!((h_zero - h_unclamped).abs() < 0.5, "maxLines=0 ({h_zero}) should match unclamped ({h_unclamped})");
    assert!((h_override - h_unclamped).abs() < 0.5, "override maxLines=0 ({h_override}) should unclamp frozen style");

    let h_rtl_full = rect_of(&rt, "rtl_full")[3];
    let h_rtl_clamped = rect_of(&rt, "rtl_clamped")[3];
    assert!(h_rtl_clamped > 0.0 && h_rtl_clamped < h_rtl_full, "RTL maxLines=1 ({h_rtl_clamped}) < unclamped ({h_rtl_full})");
}

#[test]
fn max_lines_rejects_negative_fractional_non_finite_and_non_numeric_values() {
    for expr in ["-1", "1.5", "0/0", "math.huge", "\"2\"", "true"] {
        let src = format!(
            "local ui = require(\"@rune/ui\")\nreturn {{ render = function() return ui.text(\"hi\", {{ maxLines = {expr} }}) end }}"
        );
        let errs = render_errors(&src);
        assert!(errs.iter().any(|e| e.contains("maxLines")), "expected maxLines error for {expr}, got {errs:?}");
    }
}

#[test]
fn matched_flies_between_parents_while_unmatched_jumps_and_reduced_motion_lands_at_once() {
    let src = r#"
local ui = require("@rune/ui")
local right = false
return {
    render = function()
        local matched_card = ui.box({ key = "m", matched = "card-1", w = 60, h = 40, fill = "indicator" })
        local plain_card = ui.box({ key = "u", w = 60, h = 40, fill = "control-off" })
        return ui.col({ w = 300, h = 160, gap = 8 }, {
            ui.row({ key = "toggle", w = 80, h = 24, onPress = function() right = not right end }, { "Move" }),
            ui.row({ w = 300, h = 50, justify = "between" }, {
                ui.box({ key = "m_left", w = 60, h = 40 }, { not right and matched_card }),
                ui.box({ key = "m_right", w = 60, h = 40 }, { right and matched_card }),
            }),
            ui.row({ w = 300, h = 50, justify = "between" }, {
                ui.box({ key = "u_left", w = 60, h = 40 }, { not right and plain_card }),
                ui.box({ key = "u_right", w = 60, h = 40 }, { right and plain_card }),
            }),
        })
    end,
}
"#;
    let (mut rt, logged) = render_runtime(src, (320.0, 200.0));
    assert!(logged.borrow().is_empty(), "unexpected logs: {:?}", logged.borrow());
    let start_x = rect_of(&rt, "m")[0];
    let dest_x = rect_of(&rt, "m_right")[0];
    assert!((dest_x - start_x) > 100.0, "expected distinct left/right columns: {start_x} -> {dest_x}");

    // Press the toggle button so the commit is input-driven and matched flight starts.
    let (bx, by) = rt.find("toggle").expect("toggle button");
    rt.pointer_down(bx, by);
    rt.pointer_up(bx, by);
    rt.frame(0.016);
    assert!(!rt.matched_handovers().is_empty(), "expected matched handover on move");
    rt.frame(0.05);

    let mid_m = rect_of(&rt, "m")[0];
    let mid_u = rect_of(&rt, "u")[0];
    assert!(mid_m > start_x + 1.0 && mid_m < dest_x - 1.0, "matched node should be mid-flight: start={start_x}, mid={mid_m}, dest={dest_x}");
    assert!((mid_u - dest_x).abs() < 0.5, "unmatched node should jump straight to dest={dest_x}, got {mid_u}");

    // Settle, turn on Reduce Motion, and toggle back: matched must land immediately.
    for _ in 0..120 {
        if !rt.frame(0.016) {
            break;
        }
    }
    rt.set_system_reduced_motion(true);
    rt.pointer_down(bx, by);
    rt.pointer_up(bx, by);
    rt.frame(0.016);
    let reduced_x = rect_of(&rt, "m")[0];
    assert!((reduced_x - start_x).abs() < 0.5, "reduced motion must land at start={start_x} immediately, got {reduced_x}");
}

#[test]
fn matched_crop_keeps_child_unscaled_during_flight_and_validates_props() {
    let src = r#"
local ui = require("@rune/ui")
local wide = false
return {
    render = function()
        local badge = ui.box({ key = "pill", matched = "pill", matchedCrop = true, w = if wide then 200 else 60, h = 40, fill = "indicator" }, {
            ui.box({ key = "inner", w = 20, h = 20, fill = "bg" }),
        })
        return ui.col({ w = 300, h = 120, gap = 8 }, {
            ui.row({ key = "go", w = 60, h = 24, onPress = function() wide = true end }, { "Go" }),
            ui.row({ w = 300, h = 50 }, {
                ui.box({ key = "slot_a", w = 80, h = 50 }, { not wide and badge }),
                ui.box({ key = "slot_b", w = 220, h = 50 }, { wide and badge }),
            }),
        })
    end,
}
"#;
    let (mut rt_crop, logged) = render_runtime(src, (320.0, 140.0));
    assert!(logged.borrow().is_empty(), "unexpected logs: {:?}", logged.borrow());
    let (gx, gy) = rt_crop.find("go").expect("go");
    rt_crop.pointer_down(gx, gy);
    rt_crop.pointer_up(gx, gy);
    rt_crop.frame(0.016);
    rt_crop.frame(0.04);
    let els_crop = rt_crop.elements();
    let pill_crop = els_crop.iter().find(|e| e.key.as_deref() == Some("pill")).expect("pill with crop");
    let inner_crop = els_crop.iter().find(|e| e.key.as_deref() == Some("inner")).expect("inner with crop");
    assert!(
        (inner_crop.rect[0] - pill_crop.rect[0]).abs() < 0.5 && (inner_crop.rect[2] - 20.0).abs() < 0.5,
        "matchedCrop=true pins child to moving frame leading edge: pill={:?}, inner={:?}",
        pill_crop.rect, inner_crop.rect
    );

    // Without `matchedCrop`, cover mode centres the wider final content on the moving frame,
    // sliding its leading edge left of the moving frame while the pill widens.
    let src_cover = src.replace("matchedCrop = true, ", "");
    let (mut rt_cover, logged_cover) = render_runtime(&src_cover, (320.0, 140.0));
    assert!(logged_cover.borrow().is_empty(), "unexpected logs: {:?}", logged_cover.borrow());
    let (gx, gy) = rt_cover.find("go").expect("go");
    rt_cover.pointer_down(gx, gy);
    rt_cover.pointer_up(gx, gy);
    rt_cover.frame(0.016);
    rt_cover.frame(0.04);
    let els_cover = rt_cover.elements();
    let pill_cover = els_cover.iter().find(|e| e.key.as_deref() == Some("pill")).expect("pill without crop");
    let inner_cover = els_cover.iter().find(|e| e.key.as_deref() == Some("inner")).expect("inner without crop");
    assert!(
        inner_cover.rect[0] < pill_cover.rect[0] - 10.0,
        "default cover should centre wider content past leading edge: pill={:?}, inner={:?}",
        pill_cover.rect, inner_cover.rect
    );

    for bad in [r#"matched = """#, "matched = 42", r#"matchedCrop = "yes""#] {
        let src = format!("local ui = require(\"@rune/ui\")\nreturn {{ render = function() return ui.box({{ {bad} }}) end }}");
        let errs = render_errors(&src);
        assert!(!errs.is_empty(), "expected prop error for `{bad}`");
    }
}

#[test]
fn percentage_sizes_equal_columns_and_aspect_ratio_layout() {
    let src = r#"
local ui = require("@rune/ui")
local CELL = table.freeze({ w = "50%", h = 20 })
return {
    render = function()
        return ui.col({ w = 300, h = 300, gap = 0 }, {
            ui.box({ key = "parent", w = 200, h = 100 }, {
                ui.box({ key = "pct", w = "40%", h = "60%" }),
            }),
            ui.box({ key = "zero_wrap", w = 200, h = 50 }, {
                ui.box({ key = "zero_pct", w = "0%", h = "100%" }),
            }),
            ui.box({ key = "frozen_wrap", w = 200, h = 20 }, {
                ui.box(CELL, { key = "frozen_pct" }),
            }),
            ui.box({ key = "grid", w = 300, cols = 3 }, {
                ui.box({ key = "c1", h = 20 }),
                ui.box({ key = "c2", h = 20 }),
                ui.box({ key = "c3", h = 20 }),
            }),
            ui.box({ key = "ratio", w = 100, aspect = 2 }),
        })
    end,
}
"#;
    let (rt, logged) = render_runtime(src, (320.0, 320.0));
    assert!(logged.borrow().is_empty(), "unexpected logs: {:?}", logged.borrow());

    let pct = rect_of(&rt, "pct");
    assert!((pct[2] - 80.0).abs() < 0.5 && (pct[3] - 60.0).abs() < 0.5, "40%x60% of 200x100 => 80x60, got {pct:?}");
    let z = rect_of(&rt, "zero_pct");
    assert!(z[2].abs() < 0.5 && (z[3] - 50.0).abs() < 0.5, "0%x100% of 200x50 => 0x50, got {z:?}");
    let fz = rect_of(&rt, "frozen_pct");
    assert!((fz[2] - 100.0).abs() < 0.5, "frozen 50% of 200 => 100, got {fz:?}");

    let (c1, c2, c3) = (rect_of(&rt, "c1"), rect_of(&rt, "c2"), rect_of(&rt, "c3"));
    assert!((c1[2] - 100.0).abs() < 0.5 && (c2[2] - 100.0).abs() < 0.5 && (c3[2] - 100.0).abs() < 0.5, "cols=3 widths: {c1:?} {c2:?} {c3:?}");
    assert!((c2[0] - 100.0).abs() < 0.5 && (c3[0] - 200.0).abs() < 0.5, "cols=3 positions: {c1:?} {c2:?} {c3:?}");

    let ratio = rect_of(&rt, "ratio");
    assert!((ratio[2] - 100.0).abs() < 0.5 && (ratio[3] - 50.0).abs() < 0.5, "w=100 aspect=2 => h=50, got {ratio:?}");
}

#[test]
fn transforms_press_scale_outline_raise_and_clip_props() {
    let src = r##"
local ui = require("@rune/ui")
local STYLE = table.freeze({
    scale = 0.8,
    rotate = 30,
    origin = { x = 0, y = 0 },
    offset = { x = 10, y = 20 },
    outline = "indicator",
    outlineWidth = 2,
    clip = 12,
    raise = true,
})
return {
    render = function()
        return ui.col({ w = 300, h = 300 }, {
            ui.box({
                key = "xform",
                w = 100,
                h = 40,
                scale = 0.5,
                rotate = 90,
                origin = { x = 0, y = 1 },
                offset = { x = 15, y = -10 },
                outline = ui.hex("#009fff"),
                outlineWidth = 2,
            }),
            ui.box(STYLE, {
                key = "overridden",
                w = 40,
                h = 40,
                scale = 1.25,
                rotate = -45,
                origin = { x = 1 },
                offset = { y = 5 },
                outlineWidth = 3,
                clip = false,
                raise = false,
            }),
            ui.row({ key = "press_btn", w = 80, h = 40, pressScale = 0.9, onPress = function() end }, { "Press" }),
            ui.box({ key = "clip_round", w = 40, h = 40, clip = 12 }, {
                ui.box({ key = "clip_round_child", w = 120, h = 120 }),
            }),
            ui.box({ key = "clip_bool", w = 40, h = 40, clip = true }, {
                ui.box({ key = "clip_bool_child", w = 120, h = 120 }),
            }),
            ui.box({ key = "clip_zero", w = 40, h = 40, clip = 0, origin = {}, offset = {} }),
            ui.row({ key = "offset_row", w = 120, h = 40 }, {
                ui.box({ key = "base_box", w = 40, h = 40 }),
                ui.box({ key = "shifted_box", w = 40, h = 40, offset = { x = 15, y = -10 }, hoverTransition = "instant" }),
            }),
            ui.row({ key = "stack", w = 120, h = 40 }, {
                ui.box({ key = "raised_a", w = 40, h = 40, fill = "indicator", raise = true, offset = { x = 40 } }),
                ui.box({ key = "plain_b", w = 40, h = 40, fill = "control-off" }),
            }),
        })
    end,
}
"##;
    let (mut rt, logged) = render_runtime(src, (320.0, 320.0));
    assert!(logged.borrow().is_empty(), "unexpected logs: {:?}", logged.borrow());

    let pose = rt.presented_pose("xform").expect("xform pose");
    assert!((pose.scale() - 0.5).abs() < 0.01, "scale=0.5, got {}", pose.scale());
    assert!((pose.rotation() - 90.0_f32.to_radians()).abs() < 0.01, "rotate=90 deg, got {}", pose.rotation());

    let over = rt.presented_pose("overridden").expect("overridden pose");
    assert!((over.scale() - 1.25).abs() < 0.01, "override scale=1.25, got {}", over.scale());
    assert!((over.rotation() - (-45.0_f32).to_radians()).abs() < 0.01, "override rotate=-45 deg, got {}", over.rotation());

    // `pressScale = 0.9` shrinks while pointer is held down and springs back on release.
    let (px, py) = rt.find("press_btn").expect("press_btn");
    rt.pointer_down(px, py);
    for _ in 0..20 {
        rt.frame(0.016);
    }
    let held_scale = rt.presented_pose("press_btn").expect("press_btn pose").scale();
    assert!(held_scale < 0.95, "pressScale=0.9 should shrink button while held, got {held_scale}");
    rt.pointer_up(px, py);
    for _ in 0..60 {
        rt.frame(0.016);
    }
    let rest_scale = rt.presented_pose("press_btn").expect("press_btn pose").scale();
    assert!((rest_scale - 1.0).abs() < 0.02, "pressScale should recover to 1.0, got {rest_scale}");

    // Both `clip = 12` and `clip = true` clip overflowing children to 40x40.
    let els = rt.elements();
    let vis = |k: &str| els.iter().find(|e| e.key.as_deref() == Some(k)).expect(k).visible;
    assert!((vis("clip_round_child")[2] - 40.0).abs() < 0.5 && (vis("clip_round_child")[3] - 40.0).abs() < 0.5);
    assert!((vis("clip_bool_child")[2] - 40.0).abs() < 0.5 && (vis("clip_bool_child")[3] - 40.0).abs() < 0.5);

    // `offset = { x = 15, y = -10 }` shifts the drawn rect by (+15, -10) from its layout slot.
    let base_r = rect_of(&rt, "base_box");
    let shifted_r = rect_of(&rt, "shifted_box");
    assert!((shifted_r[0] - (base_r[0] + 40.0 + 15.0)).abs() < 0.5, "offset.x +15: base={base_r:?}, shifted={shifted_r:?}");
    assert!((shifted_r[1] - (base_r[1] - 10.0)).abs() < 0.5, "offset.y -10: base={base_r:?}, shifted={shifted_r:?}");

    // `raised_a` comes before `plain_b` in tree order but overlaps it via `offset = { x = 40 }`;
    // `raise = true` puts `raised_a` on top for pointer picking.
    let b_rect = rect_of(&rt, "plain_b");
    let hit = rt.element_at(b_rect[0] + 20.0, b_rect[1] + 20.0).expect("hit overlapping stack");
    assert_eq!(hit.key(), Some("raised_a"), "raise=true should place earlier sibling on top");
}

#[test]
fn new_props_validate_types_and_ranges() {
    let cases = [
        ("pressScale = 0", "pressScale"),
        ("pressScale = -0.5", "pressScale"),
        ("pressScale = \"0.9\"", "pressScale"),
        ("scale = -1", "scale"),
        ("scale = 0/0", "scale"),
        ("rotate = math.huge", "rotate"),
        ("rotate = \"45deg\"", "rotate"),
        ("origin = 0.5", "origin"),
        ("origin = { x = \"left\" }", "origin.x"),
        ("origin = { y = 0/0 }", "origin.y"),
        ("offset = \"10px\"", "offset"),
        ("offset = { x = math.huge }", "offset.x"),
        ("outline = 123", "outline"),
        ("outlineWidth = -2", "outlineWidth"),
        ("raise = \"yes\"", "raise"),
        ("clip = -4", "clip"),
        ("clip = 0/0", "clip"),
        ("clip = \"round\"", "clip"),
        ("cols = 0", "cols"),
        ("cols = -2", "cols"),
        ("cols = 1.5", "cols"),
        ("aspect = 0", "aspect"),
        ("aspect = -1.5", "aspect"),
        ("w = \"-10%\"", "w"),
        ("w = \"abc%\"", "w"),
        ("w = \"%\"", "w"),
        ("h = \"NaN%\"", "h"),
    ];
    for (prop, expected) in cases {
        let src = format!("local ui = require(\"@rune/ui\")\nreturn {{ render = function() return ui.box({{ {prop} }}) end }}");
        let errs = render_errors(&src);
        assert!(errs.iter().any(|e| e.contains(expected)), "prop `{prop}` should error naming `{expected}`, got {errs:?}");
    }
}

#[test]
fn cursor_names_the_pointers_shape_over_a_node_and_rejects_an_unknown_name() {
    let src = r#"
local ui = require("@rune/ui")
return {
    render = function()
        return ui.col({ w = 200, h = 120 }, {
            ui.row({ key = "field", w = 200, h = 40, cursor = "text", onPress = function() end }, { "Field" }),
            ui.row({ key = "button", w = 200, h = 40, onPress = function() end }, { "Button" }),
        })
    end,
}
"#;
    let (mut rt, logged) = render_runtime(src, (300.0, 200.0));
    assert!(logged.borrow().iter().all(|(l, _)| *l != LogLevel::Error), "{:?}", logged.borrow());
    let [x, y, w, h] = rect_of(&rt, "field");
    rt.pointer_move(x + w / 2.0, y + h / 2.0);
    assert_eq!(rt.cursor(), rune::Cursor::Text, "the node's own cursor");
    let [x, y, w, h] = rect_of(&rt, "button");
    rt.pointer_move(x + w / 2.0, y + h / 2.0);
    assert_eq!(rt.cursor(), rune::Cursor::Default, "a node that names none");

    let errors = render_errors(r#"
local ui = require("@rune/ui")
return { render = function() return ui.row({ cursor = "hand" }, { "x" }) end }
"#);
    assert!(errors.iter().any(|e| e.contains("unknown cursor hand")), "{errors:?}");
}

#[test]
fn enter_exit_and_layout_decode_and_bad_strings_are_errors() {
    let src = r#"
local ui = require("@rune/ui")
local STYLE = table.freeze({ enter = "fade slide-up:24 spring:0.25:0.1", exit = "fade scale" })
return {
    render = function()
        return ui.col({ layout = true }, {
            ui.box({ key = "a", enter = "slide-left:12", exit = "none" }),
            ui.box(STYLE, { key = "b" }),
            ui.text("c", { enter = "fade spring:0.2" }),
        })
    end,
}
"#;
    let (_rt, logged) = render_runtime(src, (300.0, 200.0));
    assert!(logged.borrow().is_empty(), "unexpected logs: {:?}", logged.borrow());

    for (props, wanted) in [("enter = \"wobble\"", "enter"), ("exit = \"spring:0\"", "exit"), ("enter = 3", "enter"), ("layout = 1", "layout")] {
        let src = format!("local ui = require(\"@rune/ui\")\nreturn {{ render = function() return ui.box({{ {props} }}) end }}");
        let errs = render_errors(&src);
        assert!(errs.iter().any(|e| e.contains(wanted)), "expected {wanted} error for {props}, got {errs:?}");
    }
}

#[test]
fn enter_and_exit_play_and_reduced_motion_keeps_only_none() {
    let src = r#"
local ui = require("@rune/ui")
local shown = false
return {
    render = function()
        return ui.col({ w = 300, h = 200, gap = 8 }, {
            ui.row({ key = "toggle", w = 80, h = 24, onPress = function() shown = not shown end }, { "Toggle" }),
            ui.box({ key = "s_slot", w = 60, h = 40 }, {
                shown and ui.box({ key = "s", w = 60, h = 40, fill = "indicator", enter = "slide-up:24 spring:0.4", exit = "slide-up:24 spring:0.4" }),
            }),
            ui.box({ key = "n_slot", w = 60, h = 40 }, {
                shown and ui.box({ key = "n", w = 60, h = 40, fill = "indicator", enter = "none", exit = "none" }),
            }),
        })
    end,
}
"#;
    let (mut rt, logged) = render_runtime(src, (320.0, 220.0));
    assert!(logged.borrow().is_empty(), "unexpected logs: {:?}", logged.borrow());
    let rest = rect_of(&rt, "s_slot")[1];
    let (bx, by) = rt.find("toggle").expect("toggle button");
    let toggle = |rt: &mut Runtime<Script>| {
        rt.pointer_down(bx, by);
        rt.pointer_up(bx, by);
        rt.frame(0.016);
        rt.frame(0.05);
    };
    let settle = |rt: &mut Runtime<Script>| {
        for _ in 0..200 {
            if !rt.frame(0.016) {
                break;
            }
        }
    };
    let shows = |rt: &Runtime<Script>, key: &str| rt.presented(key).is_some_and(|(_, opacity)| opacity > 0.0);

    // An entrance sliding up starts below its place and rises; `none` shows whole at once.
    toggle(&mut rt);
    let y = rect_of(&rt, "s")[1];
    assert!(y > rest + 1.0 && y <= rest + 24.0, "slide-up entrance should start below rest={rest}, got {y}");
    assert_eq!(rt.presented("n").map(|(_, o)| o), Some(1.0), "enter = none shows at once");
    settle(&mut rt);
    assert!((rect_of(&rt, "s")[1] - rest).abs() < 0.5, "the entrance lands at rest");

    // An exit sliding up rises out; `none` is gone at once.
    toggle(&mut rt);
    let y = rect_of(&rt, "s")[1];
    assert!(y < rest - 1.0, "slide-up exit should rise above rest={rest}, got {y}");
    assert!(!shows(&rt, "n"), "exit = none goes at once");
    settle(&mut rt);

    // Under Reduce Motion the slides are dropped, and `none` still holds.
    rt.set_system_reduced_motion(true);
    toggle(&mut rt);
    let y = rect_of(&rt, "s")[1];
    assert!((y - rest).abs() < 0.5, "reduced motion drops the slide-up entrance: rest={rest}, got {y}");
    // Rune would reduce a kept slide to no motion at all; dropped, the node takes the default
    // transition's reduced fade instead.
    let opacity = rt.presented("s").map_or(0.0, |(_, o)| o);
    assert!(opacity < 1.0, "the dropped entrance falls back to the default fade, got opacity {opacity}");
    assert_eq!(rt.presented("n").map(|(_, o)| o), Some(1.0), "enter = none still shows at once");
    settle(&mut rt);
    toggle(&mut rt);
    if let Some(([_, y, _, _], _)) = rt.presented("s") {
        assert!((y - rest).abs() < 0.5, "reduced motion drops the slide-up exit: rest={rest}, got {y}");
    }
    assert!(!shows(&rt, "n"), "exit = none still goes at once");
}

#[test]
fn shrink_and_truncate_preserve_explicit_min_w_regardless_of_prop_order() {
    let src = r#"
local ui = require("@rune/ui")
local LONG = "A very long single-line label that exceeds three hundred points when laid out at default size"
local BASE = table.freeze({ w = 300, minW = 160, shrink = true })
return {
    render = function()
        return ui.col({ w = 400, h = 400 }, {
            ui.row({ w = 100 }, { ui.box({ key = "shrink_min", w = 300, h = 20, minW = 160, shrink = true }) }),
            ui.row({ w = 200 }, { ui.box({ key = "shrink_mid", w = 300, h = 20, minW = 160, shrink = true }) }),
            ui.row({ w = 100 }, { ui.box({ key = "shrink_auto", w = 300, h = 20, shrink = true }) }),
            ui.row({ w = 100 }, { ui.box(BASE, { key = "shrink_zero", h = 20, minW = 0 }) }),
            ui.row({ w = 100 }, { ui.text(LONG, { key = "trunc_min", minW = 160, truncate = true }) }),
            ui.row({ w = 100 }, { ui.text(LONG, { key = "fade_min", minW = 160, fadeOut = true }) }),
            ui.row({ w = 100 }, { ui.text(LONG, { key = "trunc_auto", truncate = true }) }),
        })
    end,
}
"#;
    let (rt, logged) = render_runtime(src, (400.0, 400.0));
    assert!(logged.borrow().is_empty(), "unexpected logs: {:?}", logged.borrow());
    assert_eq!(rect_of(&rt, "shrink_min")[2], 160.0, "minW=160 + shrink=true stops at 160");
    assert_eq!(rect_of(&rt, "shrink_mid")[2], 200.0, "minW=160 + shrink=true shrinks from 300 to 200");
    assert_eq!(rect_of(&rt, "shrink_auto")[2], 100.0, "shrink=true without minW shrinks to 100");
    assert_eq!(rect_of(&rt, "shrink_zero")[2], 100.0, "later minW=0 overrides BASE minW=160");
    assert_eq!(rect_of(&rt, "trunc_min")[2], 160.0, "minW=160 + truncate=true stops at 160");
    assert_eq!(rect_of(&rt, "fade_min")[2], 160.0, "minW=160 + fadeOut=true stops at 160");
    assert_eq!(rect_of(&rt, "trunc_auto")[2], 100.0, "truncate=true without minW shrinks to 100");
}
