//! Integration tests for `Markdown::hard_breaks`: copy_source(true/false), selection drag
//! geometry across soft-break lines, AccessKit tree + web mirror parity with CommonMark hard
//! breaks, and edge cases (empty, RTL, large multiline message).

use rune::render::{KIND_GLYPH, KIND_RECT};
use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, col};
use rune_kit::{Kit, Markdown};

fn tick<T: 'static>(rt: &mut Runtime<T>) {
    rt.frame(1.0 / 60.0);
}

fn copy<T: 'static>(rt: &mut Runtime<T>, clipboard: &rune::clipboard::MemoryClipboard, f: impl FnOnce(&mut rune::Tx)) -> String {
    rt.update(|_, tx| f(tx));
    tick(rt);
    rt.key(KeyEvent { key: Key::Command('c'), mods: Mods { cmd: true, ..Mods::default() } });
    tick(rt);
    clipboard.text().map(|t| t.to_string()).unwrap_or_default()
}

fn find<T: 'static>(rt: &Runtime<T>, suffix: &str) -> rune::runtime::ElementInfo {
    rt.elements()
        .into_iter()
        .find(|e| e.path.ends_with(suffix))
        .unwrap_or_else(|| panic!("missing element ending with {suffix:?}"))
}

fn glyphs<T: 'static>(rt: &Runtime<T>, suffix: &str, scale: f32) -> Vec<(f32, f32)> {
    let e = find(rt, suffix);
    let mut out: Vec<(f32, f32)> = rt
        .instances
        .iter()
        .filter(|i| i.params[1] == KIND_GLYPH)
        .map(|i| ((i.rect[0] + i.rect[2] / 2.0) / scale, (i.rect[1] + i.rect[3] / 2.0) / scale))
        .filter(|&(x, y)| x >= e.rect[0] && x <= e.rect[0] + e.rect[2] && y >= e.rect[1] && y <= e.rect[1] + e.rect[3])
        .collect();
    out.sort_by(|a, b| (a.1 / 10.0).round().total_cmp(&(b.1 / 10.0).round()).then_with(|| a.0.total_cmp(&b.0)));
    out
}

fn drag<T: 'static>(rt: &mut Runtime<T>, (x0, y0): (f32, f32), (x1, y1): (f32, f32)) {
    rt.pointer_move(x0, y0);
    rt.pointer_down(x0, y0);
    tick(rt);
    for k in 1..=6 {
        let f = k as f32 / 6.0;
        rt.pointer_move(x0 + (x1 - x0) * f, y0 + (y1 - y0) * f);
        tick(rt);
    }
    rt.pointer_up(x1, y1);
    tick(rt);
}

#[test]
fn copy_source_true_copies_original_and_false_copies_newline() {
    const SRC: &str = "**First**\n*Second*\n\n- alpha\n  beta\n\n> quote 1\n> quote 2\n\n```rust\nfn f() {}\n```\n";

    struct S {
        hard_breaks: bool,
        copy_source: bool,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let k = Kit::neutral(true);
        col().w_full().padding(10.0).child(
            Markdown::new(SRC)
                .hard_breaks(s.hard_breaks)
                .selectable(true)
                .copy_source(s.copy_source)
                .build(&k)
                .key("msg"),
        )
    }
    let make = |hard_breaks: bool, copy_source: bool| {
        let mut rt = Runtime::new(S { hard_breaks, copy_source }, view, (480.0, 800.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let clipboard = rune::clipboard::memory();
        rt.use_clipboard(&clipboard);
        (rt, clipboard)
    };

    // hard_breaks(true) + copy_source(true): original source verbatim (no synthetic trailing spaces).
    let (mut rt_src, cb_src) = make(true, true);
    assert_eq!(copy(&mut rt_src, &cb_src, |tx| tx.select_all_text("msg")), SRC);
    assert_eq!(copy(&mut rt_src, &cb_src, |tx| tx.select_all_text("msg/b0")), "**First**\n*Second*");
    assert_eq!(copy(&mut rt_src, &cb_src, |tx| tx.select_all_text("msg/b1")), "- alpha\n  beta");
    // Partial selection inside one block copies drawn text with the newline.
    assert_eq!(copy(&mut rt_src, &cb_src, |tx| tx.select_text("msg/b0", 1..9)), "irst\nSec");

    // hard_breaks(true) + copy_source(false): drawn text has newline at every soft break.
    let (mut rt_drawn, cb_drawn) = make(true, false);
    assert_eq!(
        copy(&mut rt_drawn, &cb_drawn, |tx| tx.select_all_text("msg")),
        "First\nSecond\n\n- alpha\nbeta\n\nquote 1\nquote 2\n\nfn f() {}"
    );

    // Default hard_breaks(false) + copy_source(false): CommonMark spaces at soft breaks.
    let (mut rt_def, cb_def) = make(false, false);
    assert_eq!(
        copy(&mut rt_def, &cb_def, |tx| tx.select_all_text("msg")),
        "First Second\n\n- alpha beta\n\nquote 1 quote 2\n\nfn f() {}"
    );
}

#[test]
fn selection_geometry_and_drag_across_soft_break_lines() {
    let mut rt = Runtime::new(
        (),
        |_: &(), _: &mut Cx<()>| {
            let k = Kit::neutral(true);
            col().w_full().padding(10.0).child(
                Markdown::new("First line\nSecond line")
                    .hard_breaks(true)
                    .selectable(true)
                    .build(&k)
                    .key("msg"),
            )
        },
        (480.0, 300.0),
        Env::new(true, 2.0),
    );
    rt.set_srgb_blend(false);
    rt.frame(0.0);
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);

    let gs = glyphs(&rt, "msg/b0", 2.0);
    // Split glyphs into visual lines by Y.
    let mut lines: Vec<Vec<(f32, f32)>> = Vec::new();
    for g in gs {
        if lines.last().is_none_or(|l: &Vec<(f32, f32)>| (g.1 - l[0].1).abs() > 6.0) {
            lines.push(vec![g]);
        } else {
            lines.last_mut().unwrap().push(g);
        }
    }
    assert_eq!(lines.len(), 2, "soft break under hard_breaks(true) must lay out on two visual lines: {lines:?}");
    assert!(lines[1][0].1 > lines[0][0].1 + 10.0, "second line sits below first line");

    // Drag from 'i' of "First line" to inside "Second line".
    let start = (lines[0][1].0 - 2.0, lines[0][1].1);
    let end = (lines[1][4].0 - 2.0, lines[1][4].1);
    drag(&mut rt, start, end);
    let sel = rt.selected_text().expect("selection active across soft break");
    assert!(sel.contains('\n'), "dragged selection across soft break contains newline: {sel:?}");
    assert!(sel.starts_with("irst line\n"), "starts on line 0: {sel:?}");
    assert!(sel.ends_with("Seco"), "ends on line 1: {sel:?}");

    // Selection geometry paints highlight rects on BOTH visual lines.
    let sel_color = Kit::neutral(true).selection().premul(1.0);
    let sel_rects: Vec<[f32; 4]> = rt
        .instances
        .iter()
        .filter(|i| i.params[1] == KIND_RECT && i.fill.iter().zip(sel_color).all(|(a, b)| (a - b).abs() < 0.02))
        .map(|i| [i.rect[0] / 2.0, i.rect[1] / 2.0, i.rect[2] / 2.0, i.rect[3] / 2.0])
        .collect();
    assert!(sel_rects.len() >= 2, "selection highlight spans both lines: {sel_rects:?}");
    let min_y = sel_rects.iter().map(|r| r[1]).fold(f32::INFINITY, f32::min);
    let max_y = sel_rects.iter().map(|r| r[1]).fold(f32::NEG_INFINITY, f32::max);
    assert!(max_y - min_y > 10.0, "selection rects sit on two distinct lines: {sel_rects:?}");

    // Copy dragged selection, then Select All + Copy round trip.
    rt.key(KeyEvent { key: Key::Command('c'), mods: Mods { cmd: true, ..Mods::default() } });
    tick(&mut rt);
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some(sel.as_str()));

    rt.key(KeyEvent { key: Key::Command('a'), mods: Mods { cmd: true, ..Mods::default() } });
    tick(&mut rt);
    rt.key(KeyEvent { key: Key::Command('c'), mods: Mods { cmd: true, ..Mods::default() } });
    tick(&mut rt);
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some("First line\nSecond line"));
}

#[test]
fn accessibility_tree_and_web_mirror_match_hard_break() {
    let run = |src: &'static str, hard_breaks: bool| {
        let mut rt = Runtime::new(
            (src, hard_breaks),
            |&(src, hard_breaks): &(&'static str, bool), _: &mut Cx<(&'static str, bool)>| {
                let k = Kit::neutral(true);
                col().w(480.0).child(
                    Markdown::new(src)
                        .hard_breaks(hard_breaks)
                        .selectable(true)
                        .build(&k)
                        .key("msg"),
                )
            },
            (480.0, 300.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.0);
        rt
    };

    let rt_soft = run("First line\nSecond line", true);
    let rt_hard = run("First line  \nSecond line", false);

    // Web accessibility mirror JSON must be identical to a CommonMark hard break.
    assert_eq!(rt_soft.a11y_json(), rt_hard.a11y_json());

    // AccessKit tree roles, values, and bounds must match a CommonMark hard break.
    type NodeSummary = (rune::accesskit::Role, Option<String>, Option<String>, Option<[i32; 4]>);
    let summarize = |rt: &Runtime<(&'static str, bool)>| {
        let tree = rt.accessibility_tree();
        let mut nodes: Vec<NodeSummary> = tree
            .nodes
            .into_iter()
            .map(|(_, n)| {
                let bounds = n.bounds().map(|b| [b.x0.round() as i32, b.y0.round() as i32, b.x1.round() as i32, b.y1.round() as i32]);
                (n.role(), n.label().map(str::to_owned), n.value().map(str::to_owned), bounds)
            })
            .collect();
        nodes.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
        nodes
    };
    let soft_nodes = summarize(&rt_soft);
    let hard_nodes = summarize(&rt_hard);
    assert_eq!(soft_nodes, hard_nodes);
    assert!(
        soft_nodes.iter().filter(|(r, ..)| *r == rune::accesskit::Role::TextRun).count() >= 2,
        "AccessKit tree should expose separate TextRuns for the two lines: {soft_nodes:?}"
    );
}

#[test]
fn edge_cases_empty_rtl_and_large_chat_message() {
    let run = |src: String, hard_breaks: bool| {
        let mut rt = Runtime::new(
            (src, hard_breaks),
            |(src, hard_breaks): &(String, bool), _: &mut Cx<(String, bool)>| {
                let k = Kit::neutral(true);
                col().w(480.0).child(Markdown::new(src.clone()).hard_breaks(*hard_breaks).build(&k).key("md"))
            },
            (480.0, 4000.0),
            Env::new(true, 1.0),
        );
        rt.frame(0.0);
        rt
    };

    // Empty, bare newline, trailing newline.
    for empty in ["", "\n", "   \n  "] {
        let rt = run(empty.into(), true);
        assert!(rt.elements().iter().all(|e| !e.path.ends_with("md/b0")), "empty source {empty:?} creates no blocks");
    }
    let trailing = run("Hello\n".into(), true);
    assert_eq!(find(&trailing, "md/b0").text.as_deref(), Some("Hello"));

    // Right-to-left Arabic chat message: two lines with hard_breaks(true), one with false.
    let rtl = "السطر الأول\nالسطر الثاني".to_owned();
    let rtl_off = run(rtl.clone(), false);
    let rtl_on = run(rtl, true);
    let h_off = find(&rtl_off, "md/b0").rect[3];
    let h_on = find(&rtl_on, "md/b0").rect[3];
    assert!(h_on > h_off * 1.5, "RTL soft break lays out on two lines: off={h_off} on={h_on}");

    // Large 120-line chat message.
    let huge = (0..120).map(|i| format!("Chat line {i}")).collect::<Vec<_>>().join("\n");
    let huge_rt = run(huge, true);
    let huge_el = find(&huge_rt, "md/b0");
    assert!(huge_el.rect[3] > 1_800.0, "120 lines should stack vertically: {:?}", huge_el.rect);
}
