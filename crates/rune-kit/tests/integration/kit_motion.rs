//! Theme-level hover and press motion defaults via `Kit::motion`.

use std::hash::{BuildHasher, RandomState};

use rune::{Anim, Curve, Cx, El, Env, Runtime, TextEdit, col, el, row};
use rune_kit::{
    Accordion, AccordionItem, Button, Checkbox, FileNode, FileTree, IconButton, Kit, Motion,
    RadioItem, SegmentedControl, Select, SelectAppearance, SidebarRow, Slider, Switch, Table,
    TableColumn, TableState, TextInput, Toast, Tree, TreeItem, TreeModel,
};

const SLOW_IN: Curve = Curve::linear(0.2);
const SLOW_OUT: Curve = Curve::linear(0.4);
const PRESS_DOWN: Curve = Curve::linear(0.2);
const PRESS_UP: Curve = Curve::linear(0.4);

#[derive(Clone, Copy, PartialEq, Eq)]
enum MotionMode {
    Default,
    HoverOnly,
    PressOnly,
    Both,
    Instant,
    Long,
}

struct App {
    mode: MotionMode,
    input: TextEdit,
    table: TableState,
    tree: TreeModel<FileNode<()>>,
}

impl App {
    fn new(mode: MotionMode) -> Self {
        let tree = FileTree::new().build([("alpha.rs", ()), ("beta.rs", ())]);
        Self { mode, input: TextEdit::new(""), table: TableState::new(), tree }
    }
}

fn kit_for(mode: MotionMode) -> Kit {
    let mut k = Kit::neutral(true);
    match mode {
        MotionMode::Default => {}
        MotionMode::HoverOnly => {
            k.motion = k.motion.hover_motion(SLOW_IN, SLOW_OUT);
        }
        MotionMode::PressOnly => {
            k.motion = k.motion.press_motion(PRESS_DOWN, PRESS_UP);
        }
        MotionMode::Both => {
            k.motion = k.motion.hover_motion(SLOW_IN, SLOW_OUT).press_motion(PRESS_DOWN, PRESS_UP);
        }
        MotionMode::Instant => {
            k.motion = k.motion.hover_motion(Anim::INSTANT, Anim::INSTANT).press_motion(Anim::INSTANT, Anim::INSTANT);
        }
        MotionMode::Long => {
            k.motion = k.motion
                .hover_motion(Curve::linear(0.6), Curve::linear(0.6))
                .press_motion(Curve::linear(0.6), Curve::linear(0.6));
        }
    }
    k
}

fn hover_of(rt: &Runtime<App>, key: &str) -> f32 {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}")).hover
}

fn press_of(rt: &Runtime<App>, key: &str) -> f32 {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}")).press
}

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
}

fn core_controls_view(s: &App, _cx: &mut Cx<App>) -> El {
    let k = kit_for(s.mode);
    let mut layer = Vec::new();
    let inline_select = Select::new("sel-inline", &["One", "Two"])
        .label("Inline")
        .current(Some("One"))
        .on_toggle(|_: &mut App| {})
        .build(&k, &mut layer)
        .key("select-inline");
    let field_select = Select::new("sel-field", &["One", "Two"])
        .label("Field")
        .appearance(SelectAppearance::Field)
        .current(Some("One"))
        .on_toggle(|_: &mut App| {})
        .build(&k, &mut layer)
        .key("select-field");
    let left = col()
        .gap(8.0)
        .w(240.0)
        .child(Button::new("Save").on_press(|_: &mut App| {}).build(&k).key("btn"))
        .child(IconButton::new(k.glyph.close).label("Close").on_press(|_: &mut App| {}).build(&k).key("icon-btn"))
        .child(
            SegmentedControl::new(&["One", "Two"], Some(0))
                .on_pick(|_: &mut App, _, _| {})
                .build(&k)
                .key("seg"),
        )
        .child(Checkbox::new(false).label("Check").on_toggle(|_: &mut App| {}).build(&k).key("cb"))
        .child(RadioItem::new("Choice", false).on_select(|_: &mut App| {}).build(&k).key("radio"))
        .child(Switch::new(false).label("Toggle").on_toggle(|_: &mut App| {}).build(&k).key("sw"))
        .child(Slider::new("vol", 0.5).label("Volume").on_change(|_: &mut App, _, _| {}).build(&k).key("slider"))
        .child(inline_select)
        .child(field_select);
    let right = col()
        .gap(8.0)
        .w(240.0)
        .child(
            Accordion::new()
                .item(AccordionItem::new("Details").on_toggle(|_: &mut App| {}))
                .build(&k)
                .key("acc"),
        )
        .child(
            TextInput::new(&s.input)
                .label("Input")
                .on_edit(|s: &mut App| &mut s.input)
                .build(&k)
                .key("input"),
        )
        .child(Toast::new("toast-1", "Saved").on_close(|_: &mut App| {}).build(&k).key("toast"))
        .child(
            SidebarRow::new("Inbox")
                .on_select(|_: &mut App, _| {})
                .build(&k)
                .key("sidebar-row"),
        )
        .child(
            k.interactive(el().key("custom").size(80.0, 28.0).on_press(|_: &mut App| {})),
        )
        .child(
            Table::new(|s: &App| &s.table, |s: &mut App| &mut s.table, |_| 1, |_, _| "r0".into())
                .column(TableColumn::new("name", "Name").sortable())
                .cell(|_, _, _| rune_kit::Cell::text("Row 0"))
                .build(s, &k)
                .h(80.0)
                .key("tbl-wrap"),
        )
        .child(
            Tree::new(|s: &App| &s.tree, |s: &mut App| &mut s.tree, |n| n.name.as_str())
                .row(|n, r| TreeItem::file(n, r))
                .build(s, &k)
                .h(80.0)
                .key("tree-wrap"),
        );
    row().w_full().h_full().padding(12.0).gap(16.0).child(left).child(right)
}

fn find_key_path(rt: &Runtime<App>, path_suffix: &str) -> (String, (f32, f32)) {
    let e = rt
        .elements()
        .into_iter()
        .find(|e| e.path.ends_with(path_suffix))
        .unwrap_or_else(|| panic!("no element ending with {path_suffix}"));
    let c = (e.rect[0] + e.rect[2] * 0.5, e.rect[1] + e.rect[3] * 0.5);
    (e.path, c)
}

fn hover_by_path(rt: &Runtime<App>, path: &str) -> f32 {
    rt.elements().into_iter().find(|e| e.path == path).unwrap().hover
}

fn press_by_path(rt: &Runtime<App>, path: &str) -> f32 {
    rt.elements().into_iter().find(|e| e.path == path).unwrap().press
}

#[test]
fn defaults_are_none_and_hash_distinguishes_curves() {
    for dark in [false, true] {
        let k = Kit::neutral(dark);
        assert_eq!(k.motion.hover, None);
        assert_eq!(k.motion.press, None);
    }
    let def = Motion::default();
    assert_eq!(def.hover, None);
    assert_eq!(def.press, None);

    let s = RandomState::new();
    let a = def.hover_motion(SLOW_IN, SLOW_OUT);
    let b = def.hover_motion(SLOW_IN, SLOW_OUT);
    let c = def.hover_motion(Anim::INSTANT, SLOW_OUT);
    let d = def.press_motion(PRESS_DOWN, PRESS_UP);
    assert_eq!(s.hash_one(a), s.hash_one(b));
    assert_ne!(s.hash_one(def), s.hash_one(a));
    assert_ne!(s.hash_one(a), s.hash_one(c));
    assert_ne!(s.hash_one(def), s.hash_one(d));
}

#[test]
fn default_kit_keeps_existing_hover_and_press_timing() {
    let mut rt = Runtime::new(App::new(MotionMode::Default), core_controls_view, (600.0, 600.0), Env::new(true, 1.0));
    settle(&mut rt);
    let (x, y) = rt.find("btn").unwrap();

    // Default hover enter is instant on the first frame.
    rt.pointer_move(x, y);
    rt.frame(1.0 / 120.0);
    assert_eq!(hover_of(&rt, "btn"), 1.0, "default hover enter stays instant");

    // Default press down uses tok::motion::INSTANT (0.1 s spring): mid-flight on a short step.
    rt.pointer_down(x, y);
    rt.frame(0.02);
    let p_down = press_of(&rt, "btn");
    assert!(p_down > 0.0 && p_down < 1.0, "default press down springs on INSTANT: {p_down}");
    rt.frame(0.3);
    assert!(press_of(&rt, "btn") > 0.99);

    // Default press release uses SNAPPY (0.18 s spring): mid-flight after 0.04 s, settled by 0.4 s.
    rt.pointer_up(x, y);
    rt.frame(0.04);
    let p_up = press_of(&rt, "btn");
    assert!(p_up > 0.1 && p_up < 0.95, "default press release springs on SNAPPY: {p_up}");

    // Default hover exit fades over 0.1 s ease-out: mid-flight on the first frame, settled by 0.2 s.
    rt.pointer_move(590.0, 590.0);
    rt.frame(0.03);
    let h_exit = hover_of(&rt, "btn");
    assert!(h_exit > 0.1 && h_exit < 0.95, "default hover exit fades over 0.1 s: {h_exit}");
    rt.frame(0.2);
    assert_eq!(hover_of(&rt, "btn"), 0.0);
}

#[test]
fn kit_motion_hover_applies_independent_enter_and_exit_across_controls() {
    let mut rt = Runtime::new(App::new(MotionMode::HoverOnly), core_controls_view, (600.0, 600.0), Env::new(true, 1.0));
    settle(&mut rt);

    // Representative kit controls whose hover overlays come from Kit::motion.hover.
    let targets = [
        "/btn",
        "/icon-btn",
        "/seg/segment1",
        "/cb/control",
        "/radio/stack/control",
        "/sw/control",
        "/slider/vol/knob",
        "/select-inline",
        "/select-field",
        "/acc/item0/trigger",
        "/input",
        "/toast/top/close",
        "/custom",
        "/tbl-wrap/viewport/rows/r0",
        "/tree-wrap/viewport/rows/alpha.rs",
    ];

    for suffix in targets {
        let (path, (x, y)) = find_key_path(&rt, suffix);
        // Move off first and settle.
        rt.pointer_move(595.0, 595.0);
        rt.frame(0.5);
        assert_eq!(hover_by_path(&rt, &path), 0.0, "{suffix} starts unhovered");

        // Enter: 0.2 s linear -> ~0.5 at 0.1 s (NOT default instant 1.0!).
        rt.pointer_move(x, y);
        rt.frame(0.1);
        let mid_in = hover_by_path(&rt, &path);
        assert!(
            (mid_in - 0.5).abs() < 0.12,
            "{suffix} hover enter follows 0.2 s linear midpoint: got {mid_in}"
        );
        rt.frame(0.15);
        assert!((hover_by_path(&rt, &path) - 1.0).abs() < 1e-4, "{suffix} reaches 1.0");

        // Exit: 0.4 s linear -> ~0.5 at 0.2 s (NOT default 0.1 s ease-out which is 0.0 by 0.2 s!).
        rt.pointer_move(595.0, 595.0);
        rt.frame(0.2);
        let mid_out = hover_by_path(&rt, &path);
        assert!(
            (mid_out - 0.5).abs() < 0.12,
            "{suffix} hover exit follows 0.4 s linear midpoint: got {mid_out}"
        );
        rt.frame(0.25);
        assert!(hover_by_path(&rt, &path) < 1e-4, "{suffix} settles back to 0.0");
    }

    // Setting motion.hover alone leaves press at its runtime default (INSTANT down, SNAPPY up).
    let (x, y) = rt.find("btn").unwrap();
    rt.pointer_move(x, y);
    rt.frame(0.3);
    rt.pointer_down(x, y);
    rt.frame(0.1);
    assert!(press_of(&rt, "btn") > 0.85, "press down stays on default INSTANT spring");
    rt.pointer_up(x, y);
    rt.frame(0.3);
    assert!(press_of(&rt, "btn") < 0.05, "press release stays on default SNAPPY spring");
}

#[test]
fn kit_motion_press_applies_independent_down_and_release_across_controls() {
    let mut rt = Runtime::new(App::new(MotionMode::PressOnly), core_controls_view, (600.0, 600.0), Env::new(true, 1.0));
    settle(&mut rt);

    let targets = [
        "/btn",
        "/icon-btn",
        "/seg/segment1",
        "/select-inline",
        "/toast/top/close",
        "/sidebar-row",
        "/custom",
    ];

    for suffix in targets {
        let (path, (x, y)) = find_key_path(&rt, suffix);
        rt.pointer_move(x, y);
        rt.frame(0.1);
        // Hover remains at default (instant enter).
        assert_eq!(hover_by_path(&rt, &path), 1.0, "{suffix} keeps default instant hover enter");

        // Press down: 0.2 s linear -> ~0.5 at 0.1 s.
        rt.pointer_down(x, y);
        rt.frame(0.1);
        let mid_down = press_by_path(&rt, &path);
        assert!(
            (mid_down - 0.5).abs() < 0.12,
            "{suffix} press down follows 0.2 s linear midpoint: got {mid_down}"
        );
        rt.frame(0.15);
        assert!((press_by_path(&rt, &path) - 1.0).abs() < 1e-4, "{suffix} press settles at 1.0");

        // Press up: 0.4 s linear -> ~0.5 at 0.2 s.
        rt.pointer_up(x, y);
        rt.frame(0.2);
        let mid_up = press_by_path(&rt, &path);
        assert!(
            (mid_up - 0.5).abs() < 0.12,
            "{suffix} press release follows 0.4 s linear midpoint: got {mid_up}"
        );
        rt.frame(0.25);
        assert!(press_by_path(&rt, &path) < 1e-4, "{suffix} press settles at 0.0");
    }
}

fn overrides_view(s: &App, _cx: &mut Cx<App>) -> El {
    let k = kit_for(s.mode);
    let btn_hover_override = Button::new("Hover Override")
        .on_press(|_: &mut App| {})
        .build(&k)
        .key("btn-hover-ov")
        .hover_motion(Anim::INSTANT, Anim::INSTANT);
    let btn_press_override = Button::new("Press Override")
        .on_press(|_: &mut App| {})
        .build(&k)
        .key("btn-press-ov")
        .press_motion(Anim::INSTANT, Anim::INSTANT);
    let pre_override = k.interactive(
        el().key("pre-ov")
            .size(100.0, 28.0)
            .hover_motion(Anim::INSTANT, Anim::INSTANT)
            .press_motion(Anim::INSTANT, Anim::INSTANT)
            .on_press(|_: &mut App| {}),
    );
    let post_override = k
        .interactive(el().key("post-ov").size(100.0, 28.0).on_press(|_: &mut App| {}))
        .hover_motion(Anim::INSTANT, Anim::INSTANT)
        .press_motion(Anim::INSTANT, Anim::INSTANT);
    let within_subtree = col()
        .gap(8.0)
        .hover_motion_within(Anim::INSTANT, Anim::INSTANT)
        .child(Button::new("Within Btn").on_press(|_: &mut App| {}).build(&k).key("within-btn"))
        .child(
            SegmentedControl::new(&["A", "B"], Some(0))
                .on_pick(|_: &mut App, _, _| {})
                .build(&k)
                .key("within-seg"),
        )
        .child(Switch::new(false).on_toggle(|_: &mut App| {}).build(&k).key("within-sw"));
    let seg_own_within = SegmentedControl::new(&["X", "Y"], Some(0))
        .on_pick(|_: &mut App, _, _| {})
        .build(&k)
        .key("seg-own-within")
        .hover_motion_within(Anim::INSTANT, Anim::INSTANT);
    let sidebar = SidebarRow::new("Sidebar").on_select(|_: &mut App, _| {}).build(&k).key("sb");

    col()
        .w_full()
        .h_full()
        .padding(12.0)
        .gap(8.0)
        .child(btn_hover_override)
        .child(btn_press_override)
        .child(pre_override)
        .child(post_override)
        .child(within_subtree)
        .child(seg_own_within)
        .child(sidebar)
}

#[test]
fn explicit_element_and_subtree_overrides_win_over_kit_motion() {
    let mut rt = Runtime::new(App::new(MotionMode::Both), overrides_view, (400.0, 500.0), Env::new(true, 1.0));
    settle(&mut rt);

    // 1. Button with explicit hover_motion(INSTANT): hover snaps, press still uses kit's 0.2/0.4 s.
    let (x, y) = rt.find("btn-hover-ov").unwrap();
    rt.pointer_move(x, y);
    rt.frame(1.0 / 120.0);
    assert_eq!(hover_of(&rt, "btn-hover-ov"), 1.0, "explicit hover_motion snaps enter");
    rt.pointer_down(x, y);
    rt.frame(0.1);
    assert!((press_of(&rt, "btn-hover-ov") - 0.5).abs() < 0.12, "press still uses kit default");
    rt.pointer_up(x, y);
    rt.pointer_move(390.0, 490.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(hover_of(&rt, "btn-hover-ov"), 0.0, "explicit hover_motion snaps exit");

    // 2. Button with explicit press_motion(INSTANT): press snaps, hover still uses kit's 0.2/0.4 s.
    let (x, y) = rt.find("btn-press-ov").unwrap();
    rt.pointer_move(x, y);
    rt.frame(0.1);
    assert!((hover_of(&rt, "btn-press-ov") - 0.5).abs() < 0.12, "hover still uses kit default");
    rt.frame(0.15);
    rt.pointer_down(x, y);
    rt.frame(1.0 / 120.0);
    assert_eq!(press_of(&rt, "btn-press-ov"), 1.0, "explicit press_motion snaps down");
    rt.pointer_up(x, y);
    rt.frame(1.0 / 120.0);
    assert_eq!(press_of(&rt, "btn-press-ov"), 0.0, "explicit press_motion snaps up");

    // 3. Kit::interactive preserves explicit hover_motion and press_motion set before OR after it.
    for key in ["pre-ov", "post-ov"] {
        let (x, y) = rt.find(key).unwrap();
        rt.pointer_move(x, y);
        rt.frame(1.0 / 120.0);
        assert_eq!(hover_of(&rt, key), 1.0, "{key} hover enter snaps");
        rt.pointer_down(x, y);
        rt.frame(1.0 / 120.0);
        assert_eq!(press_of(&rt, key), 1.0, "{key} press down snaps");
        rt.pointer_up(x, y);
        rt.frame(1.0 / 120.0);
        assert_eq!(press_of(&rt, key), 0.0, "{key} press up snaps");
        rt.pointer_move(390.0, 490.0);
        rt.frame(1.0 / 120.0);
        assert_eq!(hover_of(&rt, key), 0.0, "{key} hover exit snaps");
    }

    // 4. Enclosing hover_motion_within overrides kit.motion.hover on single and multi-part controls.
    for suffix in ["/within-btn", "/within-seg/segment1", "/within-sw/control", "/seg-own-within/segment1"] {
        let (path, (x, y)) = find_key_path(&rt, suffix);
        rt.pointer_move(x, y);
        rt.frame(1.0 / 120.0);
        assert_eq!(hover_by_path(&rt, &path), 1.0, "{suffix} hover_motion_within snaps enter");
        rt.pointer_move(390.0, 490.0);
        rt.frame(1.0 / 120.0);
        assert_eq!(hover_by_path(&rt, &path), 0.0, "{suffix} hover_motion_within snaps exit");
    }

    // 5. SidebarRow keeps its explicit instant hover while taking kit.motion.press.
    let (x, y) = rt.find("sb").unwrap();
    rt.pointer_move(x, y);
    rt.frame(1.0 / 120.0);
    assert_eq!(hover_of(&rt, "sb"), 1.0, "SidebarRow keeps explicit instant hover enter");
    rt.pointer_down(x, y);
    rt.frame(0.1);
    assert!((press_of(&rt, "sb") - 0.5).abs() < 0.12, "SidebarRow follows kit.motion.press");
    rt.pointer_up(x, y);
    rt.pointer_move(390.0, 490.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(hover_of(&rt, "sb"), 0.0, "SidebarRow keeps explicit instant hover exit");
}

#[test]
fn reduced_motion_respects_instant_and_caps_long_kit_curves() {
    // 1. Instant curves stay instant under Reduce Motion (no added fade).
    let mut inst = Runtime::new(App::new(MotionMode::Instant), core_controls_view, (600.0, 600.0), Env::new(true, 1.0));
    inst.set_system_reduced_motion(true);
    settle(&mut inst);
    let (x, y) = inst.find("btn").unwrap();
    inst.pointer_move(x, y);
    inst.frame(1.0 / 120.0);
    assert_eq!(hover_of(&inst, "btn"), 1.0, "instant hover enter stays instant under Reduce Motion");
    inst.pointer_down(x, y);
    inst.frame(1.0 / 120.0);
    assert_eq!(press_of(&inst, "btn"), 1.0, "instant press down stays instant under Reduce Motion");
    inst.pointer_up(x, y);
    inst.frame(1.0 / 120.0);
    assert_eq!(press_of(&inst, "btn"), 0.0, "instant press up stays instant under Reduce Motion");
    inst.pointer_move(595.0, 595.0);
    inst.frame(1.0 / 120.0);
    assert_eq!(hover_of(&inst, "btn"), 0.0, "instant hover exit stays instant under Reduce Motion");

    // 2. Long 0.6 s curves are reduced under Reduce Motion and settle well before 0.6 s.
    let mut long = Runtime::new(App::new(MotionMode::Long), core_controls_view, (600.0, 600.0), Env::new(true, 1.0));
    long.set_system_reduced_motion(true);
    settle(&mut long);
    let (x, y) = long.find("btn").unwrap();
    long.pointer_move(x, y);
    long.frame(0.25);
    assert!((hover_of(&long, "btn") - 1.0).abs() < 1e-4, "0.6 s hover enter reduced under Reduce Motion");
    long.pointer_down(x, y);
    long.frame(0.25);
    assert!((press_of(&long, "btn") - 1.0).abs() < 1e-4, "0.6 s press down reduced under Reduce Motion");
    long.pointer_up(x, y);
    long.frame(0.25);
    assert!(press_of(&long, "btn") < 1e-4, "0.6 s press up reduced under Reduce Motion");
    long.pointer_move(595.0, 595.0);
    long.frame(0.25);
    assert!(hover_of(&long, "btn") < 1e-4, "0.6 s hover exit reduced under Reduce Motion");
}
