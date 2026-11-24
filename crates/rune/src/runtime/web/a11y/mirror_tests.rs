//! Headless tests for web DOM accessibility mirror container roles, disabled/busy states,
//! treeitems with nested actions, range controls (`slider`/`spinbutton`), and focus proxy parity.

use serde_json::{Value, json};

use crate::runtime::web_a11y::A11yMirror;
use crate::{Cx, El, Env, Runtime, col, el, row, text};

fn items_json<S: 'static>(rt: &Runtime<S>) -> Vec<Value> {
    serde_json::from_str(&rt.a11y_json()).expect("valid a11y_json")
}

fn find_key<'a>(items: &'a [Value], key: &str) -> &'a Value {
    let suffix = format!("/{key}");
    items
        .iter()
        .find(|v| v["p"].as_str().is_some_and(|p| p.ends_with(&suffix)))
        .unwrap_or_else(|| panic!("missing item ending with {suffix}: {items:?}"))
}

fn index_key(items: &[Value], key: &str) -> usize {
    let suffix = format!("/{key}");
    items
        .iter()
        .position(|v| v["p"].as_str().is_some_and(|p| p.ends_with(&suffix)))
        .unwrap_or_else(|| panic!("missing item ending with {suffix}: {items:?}"))
}

#[test]
fn container_roles_preserve_hierarchy_and_labels() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col()
            .size(480.0, 600.0)
            .gap(4.0)
            .child(
                row()
                    .key("bar")
                    .role("menubar")
                    .label("Main menu")
                    .size(400.0, 28.0)
                    .child(
                        col()
                            .key("file-menu")
                            .role("menu")
                            .label("File")
                            .size(160.0, 60.0)
                            .child(
                                row()
                                    .key("mi-open")
                                    .role("menuitem")
                                    .label("Open")
                                    .size(140.0, 24.0)
                                    .on_press(|_: &mut ()| {}),
                            ),
                    ),
            )
            .child(
                row()
                    .key("tools")
                    .role("toolbar")
                    .label("Formatting")
                    .size(400.0, 32.0)
                    .child(
                        row()
                            .key("bold")
                            .role("button")
                            .label("Bold")
                            .size(60.0, 24.0)
                            .on_press(|_: &mut ()| {}),
                    ),
            )
            .child(
                col()
                    .key("panel")
                    .role("tabpanel")
                    .label("Day")
                    .size(400.0, 60.0)
                    .child(text("Monday").key("mon")),
            )
            .child(
                col()
                    .key("grid")
                    .role("grid")
                    .label("Metrics")
                    .size(400.0, 80.0)
                    .child(
                        col()
                            .key("body-rows")
                            .role("rowgroup")
                            .size(400.0, 40.0)
                            .child(
                                row()
                                    .key("r0")
                                    .role("row")
                                    .size(400.0, 24.0)
                                    .child(
                                        row()
                                            .key("c0")
                                            .role("gridcell")
                                            .size(120.0, 24.0)
                                            .child(text("99 ms").key("c0-val")),
                                    )
                                    .child(text("OK").key("c1").role("gridcell").size(80.0, 24.0)),
                            ),
                    ),
            )
            .child(
                col()
                    .key("tgrid")
                    .role("treegrid")
                    .label("Tasks")
                    .size(400.0, 40.0)
                    .child(
                        row()
                            .key("tg-row")
                            .role("row")
                            .expanded(true)
                            .level(1)
                            .size(400.0, 24.0)
                            .child(text("Build").key("tg-cell")),
                    ),
            )
            .child(
                col()
                    .key("settings-form")
                    .role("form")
                    .label("Preferences")
                    .size(400.0, 60.0)
                    .child(
                        row()
                            .key("filter-box")
                            .role("search")
                            .label("Filter settings")
                            .size(200.0, 28.0)
                            .child(text("Search").key("search-hint")),
                    ),
            )
    }

    let mut rt = Runtime::new((), view, (480.0, 600.0), Env::new(false, 1.0));
    rt.frame(0.0);
    let items = items_json(&rt);

    let bar_i = index_key(&items, "bar");
    assert_eq!((&items[bar_i]["k"], &items[bar_i]["g"], &items[bar_i]["o"], &items[bar_i]["t"]),
        (&json!("group"), &json!("div"), &json!("menubar"), &json!("Main menu")));

    let menu_i = index_key(&items, "file-menu");
    assert_eq!((&items[menu_i]["o"], &items[menu_i]["t"], &items[menu_i]["a"]),
        (&json!("menu"), &json!("File"), &json!(bar_i)));
    assert_eq!(find_key(&items, "mi-open")["a"], json!(menu_i));

    let tools_i = index_key(&items, "tools");
    assert_eq!((&items[tools_i]["o"], &items[tools_i]["t"]), (&json!("toolbar"), &json!("Formatting")));
    assert_eq!(find_key(&items, "bold")["a"], json!(tools_i));

    let panel_i = index_key(&items, "panel");
    assert_eq!((&items[panel_i]["o"], &items[panel_i]["t"]), (&json!("tabpanel"), &json!("Day")));
    assert_eq!(find_key(&items, "mon")["a"], json!(panel_i));

    let grid_i = index_key(&items, "grid");
    let rg_i = index_key(&items, "body-rows");
    let r0_i = index_key(&items, "r0");
    let c0_i = index_key(&items, "c0");
    assert_eq!((&items[grid_i]["o"], &items[grid_i]["t"]), (&json!("grid"), &json!("Metrics")));
    assert_eq!((&items[rg_i]["o"], &items[rg_i]["a"]), (&json!("rowgroup"), &json!(grid_i)));
    assert_eq!((&items[r0_i]["o"], &items[r0_i]["a"]), (&json!("row"), &json!(rg_i)));
    assert_eq!((&items[c0_i]["o"], &items[c0_i]["a"]), (&json!("gridcell"), &json!(r0_i)));
    assert_eq!(find_key(&items, "c0-val")["a"], json!(c0_i));
    let c1 = find_key(&items, "c1");
    assert_eq!((&c1["k"], &c1["o"], &c1["t"], &c1["a"]), (&json!("text"), &json!("gridcell"), &json!("OK"), &json!(r0_i)));

    let tgrid_i = index_key(&items, "tgrid");
    let tgrow_i = index_key(&items, "tg-row");
    assert_eq!((&items[tgrid_i]["o"], &items[tgrid_i]["t"]), (&json!("treegrid"), &json!("Tasks")));
    assert_eq!((&items[tgrow_i]["o"], &items[tgrow_i]["x"], &items[tgrow_i]["l"], &items[tgrow_i]["a"]),
        (&json!("row"), &json!(true), &json!(1), &json!(tgrid_i)));

    let form_i = index_key(&items, "settings-form");
    let search_i = index_key(&items, "filter-box");
    assert_eq!((&items[form_i]["o"], &items[form_i]["t"]), (&json!("form"), &json!("Preferences")));
    assert_eq!((&items[search_i]["o"], &items[search_i]["t"], &items[search_i]["a"]),
        (&json!("search"), &json!("Filter settings"), &json!(form_i)));
}

#[test]
fn disabled_controls_without_on_press_emit_aria_disabled_and_reject_a11y_press() {
    #[derive(Default)]
    struct State {
        disabled: bool,
        presses: usize,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        col()
            .size(360.0, 360.0)
            .gap(4.0)
            .child(
                row()
                    .key("btn")
                    .role("button")
                    .label("Save")
                    .disabled(s.disabled)
                    .size(100.0, 28.0)
                    .on_press(|s: &mut State| s.presses += 1),
            )
            .child(
                col()
                    .key("menu")
                    .role("menu")
                    .label("Actions")
                    .disabled(s.disabled)
                    .size(160.0, 60.0)
                    .child(row().key("mi-off").role("menuitem").label("Delete").disabled(true).size(140.0, 24.0))
                    .child(row().key("mi-chk").role("menuitemcheckbox").label("Auto").checked(true).disabled(true).size(140.0, 24.0)),
            )
            .child(
                row()
                    .key("tabs")
                    .role("tablist")
                    .label("Views")
                    .size(200.0, 28.0)
                    .child(row().key("tab-off").role("tab").label("History").focus_selected(false).disabled(true).size(80.0, 24.0)),
            )
            .child(row().key("chk-off").role("checkbox").label("Locked").checked(false).disabled(true).size(120.0, 24.0))
            .child(row().key("sw-off").role("switch").label("Airplane").checked(true).disabled(true).size(120.0, 24.0))
            .child(row().key("opt-off").role("option").label("Beta").disabled(true).size(120.0, 24.0))
            .child(
                row()
                    .key("disabled-link")
                    .href("https://example.com/")
                    .label("Docs")
                    .disabled(true)
                    .size(100.0, 24.0),
            )
    }

    let mut rt = Runtime::new(State { disabled: true, presses: 0 }, view, (360.0, 360.0), Env::new(false, 1.0));
    rt.frame(0.0);
    let items = items_json(&rt);

    let btn = find_key(&items, "btn");
    assert_eq!((&btn["k"], btn.get("dis")), (&json!("button"), Some(&json!(true))));
    let btn_path = btn["p"].as_str().unwrap().to_owned();
    assert!(!rt.web_a11y_action(&json!({"kind": "a11y_press", "path": btn_path})),
        "disabled button must reject a11y_press");
    rt.frame(0.0);
    assert_eq!(rt.state.presses, 0);

    let menu_i = index_key(&items, "menu");
    assert_eq!(items[menu_i].get("dis"), Some(&json!(true)));

    let mi_off = find_key(&items, "mi-off");
    assert_eq!((&mi_off["o"], mi_off.get("dis"), &mi_off["a"]), (&json!("menuitem"), Some(&json!(true)), &json!(menu_i)));
    let mi_chk = find_key(&items, "mi-chk");
    assert_eq!((&mi_chk["o"], &mi_chk["c"], mi_chk.get("dis")), (&json!("menuitemcheckbox"), &json!("true"), Some(&json!(true))));

    assert_eq!((&find_key(&items, "tab-off")["o"], find_key(&items, "tab-off").get("dis")), (&json!("tab"), Some(&json!(true))));
    assert_eq!((&find_key(&items, "chk-off")["o"], find_key(&items, "chk-off").get("dis")), (&json!("checkbox"), Some(&json!(true))));
    assert_eq!((&find_key(&items, "sw-off")["o"], find_key(&items, "sw-off").get("dis")), (&json!("switch"), Some(&json!(true))));
    assert_eq!((&find_key(&items, "opt-off")["o"], find_key(&items, "opt-off").get("dis")), (&json!("option"), Some(&json!(true))));
    assert_eq!((&find_key(&items, "disabled-link")["k"], find_key(&items, "disabled-link").get("dis")), (&json!("link"), Some(&json!(true))));

    // Re-enabling clears "dis" and allows a11y_press.
    rt.update(|s, _| s.disabled = false);
    rt.frame(0.0);
    let after = items_json(&rt);
    assert_eq!(find_key(&after, "btn").get("dis"), None);
    assert_eq!(find_key(&after, "menu").get("dis"), None);
    assert!(rt.web_a11y_action(&json!({"kind": "a11y_press", "path": btn_path})));
    rt.frame(0.0);
    assert_eq!(rt.state.presses, 1);
}

#[test]
fn tree_preserves_busy_treeitem_and_nested_row_action_as_sibling() {
    fn view(busy: &bool, cx: &mut Cx<bool>) -> El {
        cx.web_focus();
        col()
            .size(320.0, 200.0)
            .role("tree")
            .label("Files")
            .busy(*busy)
            .child(
                row()
                    .key("src")
                    .h(30.0)
                    .w(240.0)
                    .role("treeitem")
                    .label("src")
                    .level(1)
                    .expanded(true)
                    .busy(*busy)
                    .focusable(true)
                    .on_press(|_: &mut bool| {}),
            )
            .child(
                row()
                    .key("main-rs")
                    .h(30.0)
                    .w(240.0)
                    .role("treeitem")
                    .label("main.rs")
                    .level(2)
                    .focus_selected(true)
                    .focusable(true)
                    .on_press(|_: &mut bool| {})
                    .child(text("main.rs").key("main-text"))
                    .child(
                        el()
                            .key("preview")
                            .size(24.0, 24.0)
                            .role("button")
                            .label("Preview")
                            .focusable(true)
                            .on_press(|_: &mut bool| {}),
                    ),
            )
    }

    let mut rt = Runtime::new(true, view, (320.0, 200.0), Env::new(false, 1.0));
    rt.frame(0.0);
    let items = items_json(&rt);

    let tree_i = items.iter().position(|v| v.get("o") == Some(&json!("tree"))).expect("tree container");
    assert_eq!((&items[tree_i]["t"], items[tree_i].get("b")), (&json!("Files"), Some(&json!(true))));

    let src = find_key(&items, "src");
    assert_eq!((&src["o"], &src["t"], &src["x"], &src["l"], src.get("b"), &src["a"]),
        (&json!("treeitem"), &json!("src"), &json!(true), &json!(1), Some(&json!(true)), &json!(tree_i)));

    let main = find_key(&items, "main-rs");
    assert_eq!((&main["o"], &main["t"], &main["se"], &main["l"], &main["a"]),
        (&json!("treeitem"), &json!("main.rs"), &json!(true), &json!(2), &json!(tree_i)));

    let preview = find_key(&items, "preview");
    assert_eq!((&preview["k"], &preview["t"], &preview["a"]), (&json!("button"), &json!("Preview"), &json!(tree_i)));
    assert!(items.iter().all(|v| v["k"] != "text"), "row text inside treeitem must not duplicate beside it: {items:?}");

    // Clearing busy removes "b" via incremental patch too.
    let mut mirror = A11yMirror::default();
    let _ = rt.core().web_a11y_patch(&mut mirror).expect("initial patch");
    rt.update(|b, _| *b = false);
    rt.frame(0.0);
    let patch: Value = serde_json::from_str(rt.core().web_a11y_patch(&mut mirror).expect("busy clear patch")).unwrap();
    assert_eq!(patch["p"], json!(1));
    let upserts = patch["u"].as_array().expect("upserts");
    assert!(upserts.iter().any(|v| v["p"].as_str().is_some_and(|p| p.ends_with("/src")) && v.get("b").is_none()));
}

#[test]
fn slider_and_spinbutton_export_range_value_text_and_focus_proxy_states() {
    struct State {
        volume: f64,
        retries: f64,
        locked: bool,
        busy: bool,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        col()
            .size(360.0, 260.0)
            .gap(8.0)
            .child(
                el()
                    .key("vol")
                    .role("slider")
                    .label("Volume")
                    .range_value(s.volume, 0.0, 100.0)
                    .a11y_value(format!("{}%", s.volume.round()))
                    .disabled(s.locked)
                    .busy(s.busy)
                    .focusable(true)
                    .size(180.0, 24.0),
            )
            .child(
                el()
                    .key("retries")
                    .role("spinbutton")
                    .label("Retries")
                    .range_value(s.retries, 1.0, 10.0)
                    .a11y_value(format!("{} retries", s.retries.round()))
                    .focusable(true)
                    .size(120.0, 28.0),
            )
            .child(
                el()
                    .key("nan-slider")
                    .role("slider")
                    .label("Bad range")
                    .range_value(f64::NAN, 0.0, 1.0)
                    .size(120.0, 24.0),
            )
            .child(
                el()
                    .key("sec-slider")
                    .role("slider")
                    .label("Secret")
                    .range_value(42.0, 0.0, 100.0)
                    .a11y_value("42")
                    .secure()
                    .focusable(true)
                    .size(120.0, 24.0),
            )
    }

    let mut rt = Runtime::new(
        State { volume: 35.0, retries: 3.0, locked: false, busy: true },
        view,
        (360.0, 260.0),
        Env::new(false, 1.0),
    );
    rt.frame(0.0);
    let items = items_json(&rt);

    let vol = find_key(&items, "vol");
    assert_eq!((&vol["k"], &vol["o"], &vol["t"], &vol["rg"], &vol["vt"], vol.get("v"), vol.get("dis"), vol.get("b")),
        (&json!("button"), &json!("slider"), &json!("Volume"), &json!([0.0, 100.0, 35.0]), &json!("35%"), None, None, Some(&json!(true))));

    let retries = find_key(&items, "retries");
    assert_eq!((&retries["k"], &retries["o"], &retries["t"], &retries["rg"], &retries["vt"]),
        (&json!("button"), &json!("spinbutton"), &json!("Retries"), &json!([1.0, 10.0, 3.0]), &json!("3 retries")));

    let nan_slider = find_key(&items, "nan-slider");
    assert_eq!(nan_slider.get("rg"), None, "non-finite range must be omitted");

    assert!(items.iter().all(|v| v["p"].as_str().is_none_or(|p| !p.ends_with("/sec-slider"))),
        "secure slider must not appear in the DOM mirror");

    // Focus proxy carries rg, v, dis, and b, and omits rg/v on secure controls.
    let mut mirror = A11yMirror::default();
    rt.update(|_, tx| tx.focus("vol", true));
    rt.frame(0.0);
    let f_vol: Value = serde_json::from_str(rt.core().web_a11y_focus(&mut mirror).expect("focus vol")).unwrap();
    assert_eq!((&f_vol["o"], &f_vol["t"], &f_vol["rg"], &f_vol["v"], f_vol.get("dis"), f_vol.get("b")),
        (&json!("slider"), &json!("Volume"), &json!([0.0, 100.0, 35.0]), &json!("35%"), None, Some(&json!(true))));

    rt.update(|s, _| { s.volume = 70.0; s.locked = true; s.busy = false; });
    rt.frame(0.0);
    let f_vol2: Value = serde_json::from_str(rt.core().web_a11y_focus(&mut mirror).expect("focus vol updated")).unwrap();
    assert_eq!((&f_vol2["rg"], &f_vol2["v"], f_vol2.get("dis"), f_vol2.get("b")),
        (&json!([0.0, 100.0, 70.0]), &json!("70%"), Some(&json!(true)), None));

    rt.update(|_, tx| tx.focus("sec-slider", true));
    rt.frame(0.0);
    let f_sec: Value = serde_json::from_str(rt.core().web_a11y_focus(&mut mirror).expect("focus sec")).unwrap();
    assert_eq!((f_sec.get("rg"), f_sec.get("v")), (None, None), "secure focus proxy must omit range and value");
}

#[test]
fn plain_controls_omit_disabled_busy_and_range_keys() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col()
            .size(200.0, 100.0)
            .child(text("Hello").key("hello"))
            .child(row().key("ok").role("button").label("OK").size(60.0, 28.0).on_press(|_: &mut ()| {}))
    }

    let mut rt = Runtime::new((), view, (200.0, 100.0), Env::new(false, 1.0));
    rt.frame(0.0);
    let items = items_json(&rt);
    for item in &items {
        assert_eq!(item.get("dis"), None);
        assert_eq!(item.get("b"), None);
        assert_eq!(item.get("rg"), None);
        assert_eq!(item.get("vt"), None);
        assert_eq!(item.get("dir"), None);
    }
}

#[test]
fn live_text_updates_web_a11y_patch_snapshot_focus_and_schedule_without_commit() {
    struct State {
        clock: crate::LiveText,
        status: crate::LiveText,
        count: crate::LiveText,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().size(320.0, 200.0)
            .child(text("00:00").key("clock").live_text(&s.clock))
            .child(
                col().key("live-box").live_region(crate::LiveRegion::Polite).size(240.0, 30.0)
                    .child(text("idle").key("status").live_text(&s.status)),
            )
            .child(
                row().key("btn").role("button").focusable(true).size(120.0, 30.0)
                    .on_press(|_: &mut State| {})
                    .child(text("0").key("count").live_text(&s.count)),
            )
    }

    let clock = crate::LiveText::new("00:00");
    let status = crate::LiveText::new("idle");
    let count = crate::LiveText::new("0");
    let mut rt = Runtime::new(
        State { clock: clock.clone(), status: status.clone(), count: count.clone() },
        view,
        (320.0, 200.0),
        Env::new(false, 1.0),
    );
    rt.update(|_, tx| tx.focus("btn", true));
    rt.frame(0.0);

    let mut patch_mirror = A11yMirror::default();
    let mut snap_mirror = A11yMirror::default();
    let _ = rt.core().web_a11y_patch(&mut patch_mirror).expect("initial patch");
    let _ = rt.core().web_a11y(&mut snap_mirror).expect("initial snapshot");
    let f0: Value = serde_json::from_str(rt.core().web_a11y_focus(&mut patch_mirror).expect("initial focus")).unwrap();
    assert_eq!(f0["t"], json!("0"));
    assert!(rt.core().web_a11y_patch(&mut patch_mirror).is_none());
    assert!(rt.core().web_a11y(&mut snap_mirror).is_none());
    assert!(rt.core().web_a11y_focus(&mut patch_mirror).is_none());

    let commits = rt.stats.commits;

    // Updating plain LiveText marks the mirror dirty and emits a compact patch without a commit.
    clock.set("00:01");
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.stats.commits, commits, "LiveText::set must not commit");
    let ev = rt.core().web_a11y_frame_event(&mut patch_mirror, crate::time::now(), false);
    assert!(
        matches!(ev, crate::runtime::web_a11y::schedule::ScheduleEvent::Frame { dirty: true, scrolling: false, .. }),
        "expected dirty frame event after LiveText change, got {ev:?}",
    );
    let patch: Value = serde_json::from_str(rt.core().web_a11y_patch(&mut patch_mirror).expect("clock patch")).unwrap();
    let upserts = patch["u"].as_array().expect("upserts");
    let clock_upsert = upserts.iter().find(|v| v["p"].as_str().is_some_and(|p| p.ends_with("/clock")))
        .unwrap_or_else(|| panic!("missing clock upsert: {patch}"));
    assert_eq!(clock_upsert["t"], json!("00:01"));
    assert_eq!(clock_upsert.get("lv"), None, "plain LiveText must carry no aria-live");

    let snap: Vec<Value> = serde_json::from_str(rt.core().web_a11y(&mut snap_mirror).expect("clock snapshot")).unwrap();
    assert_eq!(find_key(&snap, "clock")["t"], json!("00:01"));
    assert!(rt.core().web_a11y_focus(&mut patch_mirror).is_none(), "unrelated LiveText must not re-emit focus proxy");

    // Idle frame emits nothing.
    rt.frame(2.0 / 60.0);
    assert!(rt.core().web_a11y_patch(&mut patch_mirror).is_none());
    assert!(rt.core().web_a11y(&mut snap_mirror).is_none());
    let idle_ev = rt.core().web_a11y_frame_event(&mut patch_mirror, crate::time::now(), false);
    assert!(
        matches!(idle_ev, crate::runtime::web_a11y::schedule::ScheduleEvent::Frame { dirty: false, .. }),
        "idle frame must not be dirty: {idle_ev:?}",
    );

    // LiveText inside a LiveRegion::Polite container patches in place.
    status.set("working for 12s");
    rt.frame(3.0 / 60.0);
    assert_eq!(rt.stats.commits, commits);
    let patch: Value = serde_json::from_str(rt.core().web_a11y_patch(&mut patch_mirror).expect("status patch")).unwrap();
    let upserts = patch["u"].as_array().expect("status upserts");
    let status_upsert = upserts.iter().find(|v| v["p"].as_str().is_some_and(|p| p.ends_with("/status")))
        .unwrap_or_else(|| panic!("missing status upsert: {patch}"));
    assert_eq!(status_upsert["t"], json!("working for 12s"));

    // Focused button whose label comes from a child LiveText updates both the mirror and focus proxy.
    count.set("42");
    rt.frame(4.0 / 60.0);
    assert_eq!(rt.stats.commits, commits);
    let patch: Value = serde_json::from_str(rt.core().web_a11y_patch(&mut patch_mirror).expect("btn patch")).unwrap();
    let upserts = patch["u"].as_array().expect("btn upserts");
    let btn_upsert = upserts.iter().find(|v| v["p"].as_str().is_some_and(|p| p.ends_with("/btn")))
        .unwrap_or_else(|| panic!("missing btn upsert: {patch}"));
    assert_eq!(btn_upsert["t"], json!("42"));
    let f1: Value = serde_json::from_str(rt.core().web_a11y_focus(&mut patch_mirror).expect("focus proxy update")).unwrap();
    assert_eq!(f1["t"], json!("42"));

    // Per-frame LiveText ticks in app mode coalesce into one off-rAF idle flush without timer churn.
    let mut seq = crate::machine::Seq::default();
    let mut sched = crate::runtime::web_a11y::schedule::ScheduleState::default();
    let t0 = crate::time::now();
    clock.set("00:02");
    rt.frame(5.0 / 60.0);
    let ev1 = rt.core().web_a11y_frame_event(&mut patch_mirror, t0, false);
    let eff1: Vec<_> = crate::runtime::web_a11y::schedule::step(&mut seq, &mut sched, ev1).into_iter().collect();
    assert_eq!(eff1.len(), 1);
    let crate::runtime::web_a11y::schedule::ScheduleEffect::ScheduleIdle { attempt, .. } = eff1[0] else {
        panic!("expected ScheduleIdle, got {eff1:?}");
    };
    clock.set("00:03");
    rt.frame(6.0 / 60.0);
    let ev2 = rt.core().web_a11y_frame_event(&mut patch_mirror, t0 + std::time::Duration::from_millis(16), false);
    let eff2: Vec<_> = crate::runtime::web_a11y::schedule::step(&mut seq, &mut sched, ev2).into_iter().collect();
    assert!(eff2.is_empty(), "subsequent LiveText ticks while IdlePending must coalesce: {eff2:?}");
    let idle_ev = rt.core().web_a11y_idle_event(attempt, t0 + std::time::Duration::from_millis(20));
    let eff3: Vec<_> = crate::runtime::web_a11y::schedule::step(&mut seq, &mut sched, idle_ev).into_iter().collect();
    assert_eq!(eff3, vec![crate::runtime::web_a11y::schedule::ScheduleEffect::FlushNow]);
    let coalesced: Value = serde_json::from_str(rt.core().web_a11y_patch(&mut patch_mirror).expect("coalesced patch")).unwrap();
    let coalesced_clock = coalesced["u"].as_array().unwrap().iter()
        .find(|v| v["p"].as_str().is_some_and(|p| p.ends_with("/clock")))
        .unwrap_or_else(|| panic!("missing coalesced clock upsert: {coalesced}"));
    assert_eq!(coalesced_clock["t"], json!("00:03"));
}

#[test]
fn align_name_uses_logical_start_center_and_end() {
    use crate::runtime::web_a11y::align_name;
    assert_eq!(align_name(crate::Align::Start), "start");
    assert_eq!(align_name(crate::Align::Center), "center");
    assert_eq!(align_name(crate::Align::End), "end");
}

#[test]
fn rtl_root_emits_dir_only_on_flips_and_logical_text_alignment() {
    use crate::{Align, Direction};

    fn view(rtl_root: &bool, _: &mut Cx<bool>) -> El {
        let dir = if *rtl_root { Direction::Rtl } else { Direction::Ltr };
        col()
            .dir(dir)
            .size(360.0, 260.0)
            .child(text("مرحبا بالعالم").key("rtl-start").align(Align::Start))
            .child(text("نهاية").key("rtl-end").align(Align::End))
            // Unmirrored intermediate container flips direction: its text child must still
            // emit explicit dir relative to the mirror root.
            .child(
                col()
                    .dir(Direction::Ltr)
                    .child(text("README.md").key("ltr-island").align(Align::Start)),
            )
            // Mirrored container flips to LTR; its LTR child inherits, while its inner RTL
            // child flips back to rtl.
            .child(
                row()
                    .key("ltr-bar")
                    .role("toolbar")
                    .label("Commands")
                    .dir(Direction::Ltr)
                    .size(200.0, 32.0)
                    .child(
                        el()
                            .key("run-btn")
                            .role("button")
                            .label("Run")
                            .size(60.0, 28.0)
                            .on_press(|_: &mut bool| {}),
                    )
                    .child(text("إرسال").key("inner-rtl").dir(Direction::Rtl)),
            )
            // Direction::Auto resolves from the first strong character.
            .child(text("Hello auto").key("auto-ltr").dir(Direction::Auto))
    }

    let mut rt = Runtime::new(true, view, (360.0, 260.0), Env::new(false, 1.0));
    rt.frame(0.0);

    let mut mirror = A11yMirror::default();
    let first: Value = serde_json::from_str(
        rt.core().web_a11y_patch(&mut mirror).expect("initial RTL patch"),
    ).unwrap();
    assert_eq!(first["dir"], "rtl");
    assert_eq!(first.get("lang"), None, "lang must be omitted when no language is set");
    assert!(rt.core().web_a11y_patch(&mut mirror).is_none(), "idle frame emits no patch");

    let items = items_json(&rt);
    let rtl_start = find_key(&items, "rtl-start");
    assert_eq!(rtl_start.get("dir"), None, "inherited RTL text omits redundant dir");
    assert_eq!(rtl_start["f"][5], "start");
    let rtl_end = find_key(&items, "rtl-end");
    assert_eq!(rtl_end.get("dir"), None);
    assert_eq!(rtl_end["f"][5], "end");
    let ltr_island = find_key(&items, "ltr-island");
    assert_eq!(ltr_island["dir"], "ltr", "text inside unmirrored LTR wrapper flips from RTL root");
    let ltr_bar = find_key(&items, "ltr-bar");
    assert_eq!(ltr_bar["dir"], "ltr");
    let run_btn = find_key(&items, "run-btn");
    assert_eq!(run_btn.get("dir"), None, "LTR button inside LTR toolbar inherits dir");
    let inner_rtl = find_key(&items, "inner-rtl");
    assert_eq!(inner_rtl["dir"], "rtl", "RTL text inside LTR toolbar flips back to rtl");
    let auto_ltr = find_key(&items, "auto-ltr");
    assert_eq!(auto_ltr["dir"], "ltr", "Direction::Auto Latin text flips from RTL root");

    // Flipping the root back to LTR updates root dir and clears redundant child dir attributes.
    rt.update(|rtl, _| *rtl = false);
    rt.frame(0.0);
    let flipped: Value = serde_json::from_str(
        rt.core().web_a11y_patch(&mut mirror).expect("LTR flip patch"),
    ).unwrap();
    assert_eq!(flipped["dir"], "ltr");
    let items_ltr = items_json(&rt);
    assert_eq!(find_key(&items_ltr, "ltr-island").get("dir"), None);
    assert_eq!(find_key(&items_ltr, "ltr-bar").get("dir"), None);
    assert_eq!(find_key(&items_ltr, "inner-rtl")["dir"], "rtl");
}

#[test]
fn root_lang_tracks_active_text_system_language_and_clears_cleanly() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().size(240.0, 80.0).child(text("مرحبا").key("hello"))
    }

    let mut rt = Runtime::new((), view, (240.0, 80.0), Env::new(false, 1.0));
    rt.frame(0.0);
    let mut mirror = A11yMirror::default();

    let initial: Value = serde_json::from_str(
        rt.core().web_a11y_patch(&mut mirror).expect("initial patch"),
    ).unwrap();
    assert_eq!(initial["dir"], "ltr");
    assert_eq!(initial.get("lang"), None);
    assert!(rt.core().web_a11y_patch(&mut mirror).is_none());

    // Setting language on the text system triggers a compact root-only lang patch.
    let ar = parley::fontique::Language::parse("ar-EG").ok();
    rt.core_mut().text.set_language(ar);
    let set_ar: Value = serde_json::from_str(
        rt.core().web_a11y_patch(&mut mirror).expect("lang set patch"),
    ).unwrap();
    assert_eq!(set_ar["lang"], "ar-EG");
    assert_eq!(set_ar.get("dir"), None, "unchanged root dir is omitted");
    assert_eq!(set_ar.get("u"), None, "unchanged items are omitted");
    assert!(rt.core().web_a11y_patch(&mut mirror).is_none(), "repeated read emits nothing");

    // Changing language emits only the new BCP 47 tag.
    let he = parley::fontique::Language::parse("he-IL").ok();
    rt.core_mut().text.set_language(he);
    let set_he: Value = serde_json::from_str(
        rt.core().web_a11y_patch(&mut mirror).expect("lang change patch"),
    ).unwrap();
    assert_eq!(set_he["lang"], "he-IL");

    // Clearing language emits empty string so bridge.js removes the lang attribute.
    rt.core_mut().text.set_language(None);
    let cleared: Value = serde_json::from_str(
        rt.core().web_a11y_patch(&mut mirror).expect("lang clear patch"),
    ).unwrap();
    assert_eq!(cleared["lang"], "");
    assert!(rt.core().web_a11y_patch(&mut mirror).is_none());
}

#[test]
fn inline_text_runs_do_not_merge_across_direction_flips() {
    use crate::Direction;

    fn view(_: &(), _: &mut Cx<()>) -> El {
        row()
            .size(300.0, 40.0)
            .child(text("Open ").key("a").dir(Direction::Ltr))
            .child(text("مرحبا").key("b").dir(Direction::Rtl))
    }

    let mut rt = Runtime::new((), view, (300.0, 40.0), Env::new(false, 1.0));
    rt.frame(0.0);
    let items = items_json(&rt);
    let a = find_key(&items, "a");
    let b = find_key(&items, "b");
    assert_eq!(a.get("dir"), None);
    assert_eq!(b["dir"], "rtl");
}
