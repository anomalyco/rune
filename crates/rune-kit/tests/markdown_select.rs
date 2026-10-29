//! `Markdown::selectable`: one selection across a message's paragraphs, list items and code
//! blocks, copied as the text drawn (no list markers or fences), with links still opening on a
//! click, inside memo'd rows and while the message streams in.
#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]
#![cfg(feature = "markdown")]

use rune::runtime::ElementInfo;
use rune::{Cursor, Cx, El, Env, Key, KeyEvent, Mods, Runtime, col, memo};
use rune_kit::{Kit, Markdown};

#[derive(Default)]
struct S { msgs: Vec<String>, selectable: bool, opened: Vec<String> }

const FIRST: &str = "Hello **world** and [link](https://x.dev/a).\n\n- alpha\n- beta\n\n```\nfn main() {}\n```\n";

fn view(s: &S, _: &mut Cx<S>) -> El {
    let k = Kit::neutral(true);
    col().w_full().gap(24.0).padding(10.0).children(s.msgs.iter().enumerate().map(|(i, m)| {
        memo(format!("m{i}"), (m, s.selectable), || {
            Markdown::new(m.clone()).selectable(s.selectable).on_link(|s: &mut S, url| s.opened.push(url.to_owned())).build(&k)
        })
    }))
}

fn runtime(selectable: bool) -> Runtime<S> {
    runtime_with(&[FIRST, "Second message"], selectable)
}

fn runtime_with(msgs: &[&str], selectable: bool) -> Runtime<S> {
    let s = S { msgs: msgs.iter().map(|m| m.to_string()).collect(), selectable, opened: Vec::new() };
    let mut rt = Runtime::new(s, view, (480.0, 800.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt
}

fn find(rt: &Runtime<S>, path_end: &str) -> ElementInfo {
    rt.elements().into_iter().find(|e| e.path.ends_with(path_end)).unwrap_or_else(|| panic!("{path_end}"))
}

/// Centres (points) of the glyphs drawn inside the element whose path ends `path_end`.
fn glyphs(rt: &Runtime<S>, path_end: &str) -> Vec<(f32, f32)> {
    let e = find(rt, path_end);
    let mut out: Vec<(f32, f32)> = rt.instances.iter()
        .filter(|i| i.params[1] == rune::render::KIND_GLYPH)
        .map(|i| ((i.rect[0] + i.rect[2] / 2.0) / 2.0, (i.rect[1] + i.rect[3] / 2.0) / 2.0))
        .filter(|&(x, y)| x >= e.rect[0] && x <= e.rect[0] + e.rect[2] && y >= e.rect[1] && y <= e.rect[1] + e.rect[3])
        .collect();
    out.sort_by(|a, b| ((a.1 / 10.0).round(), a.0).partial_cmp(&((b.1 / 10.0).round(), b.0)).unwrap());
    out
}

fn tick(rt: &mut Runtime<S>) { rt.frame(1.0 / 60.0); }

fn drag(rt: &mut Runtime<S>, (x0, y0): (f32, f32), (x1, y1): (f32, f32)) {
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

fn before(g: (f32, f32)) -> (f32, f32) { (g.0 - 3.0, g.1) }

fn command(rt: &mut Runtime<S>, c: char) {
    rt.key(KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, ..Mods::default() } });
    tick(rt);
}

#[test]
fn a_drag_selects_from_a_paragraph_into_a_code_block_and_copies_the_drawn_text() {
    let mut rt = runtime(true);
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    let para = glyphs(&rt, "m0/b0");
    rt.pointer_move(para[1].0, para[1].1);
    tick(&mut rt);
    assert_eq!(rt.cursor(), Cursor::Text, "an I-beam over the text");
    let code = glyphs(&rt, "m0/b2");
    drag(&mut rt, before(para[1]), before(code[2]));
    assert_eq!(rt.selected_text().as_deref(), Some("ello world and link.\nalpha\nbeta\nfn "));
    command(&mut rt, 'c');
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some("ello world and link.\nalpha\nbeta\nfn "));
    command(&mut rt, 'a');
    assert_eq!(rt.selected_text().as_deref(), Some("Hello world and link.\nalpha\nbeta\nfn main() {}"), "the message, not the next one");
}

#[test]
fn a_link_opens_on_a_click() {
    let mut rt = runtime(true);
    let link = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("https://x.dev/a")).expect("the link");
    let at = (link.rect[0] + link.rect[2] / 2.0, link.rect[1] + link.rect[3] / 2.0);
    rt.pointer_move(at.0, at.1);
    tick(&mut rt);
    assert_eq!(rt.cursor(), Cursor::Pointer);
    rt.pointer_down(at.0, at.1);
    rt.pointer_up(at.0, at.1);
    tick(&mut rt);
    assert_eq!(rt.state.opened, ["https://x.dev/a"]);
    assert_eq!(rt.selected_text(), None);
}

#[test]
fn a_selection_survives_its_message_and_the_next_streaming_in() {
    let mut rt = runtime(true);
    let para = glyphs(&rt, "m0/b0");
    let alpha = glyphs(&rt, "m0/b1/i0/#1/b0");
    drag(&mut rt, before(para[1]), before(alpha[2]));
    assert_eq!(rt.selected_text().as_deref(), Some("ello world and link.\nal"));
    for more in [" streams", " in\n\nand", " on"] {
        rt.state.msgs[1].push_str(more);
        rt.state.msgs[0].push_str(more);
        rt.invalidate();
        tick(&mut rt);
        assert_eq!(rt.selected_text().as_deref(), Some("ello world and link.\nal"), "after {more:?}");
    }
    // In the streaming message itself, once the growing first one has pushed it down.
    rt.frame(1.0);
    let second = glyphs(&rt, "m1/b0");
    drag(&mut rt, before(second[0]), before(second[6]));
    assert_eq!(rt.selected_text().as_deref(), Some("Second "));
    rt.state.msgs[1].push_str(" more words");
    rt.invalidate();
    tick(&mut rt);
    assert_eq!(rt.selected_text().as_deref(), Some("Second "));
}

#[test]
fn without_selectable_nothing_selects() {
    let mut rt = runtime(false);
    let para = glyphs(&rt, "m0/b0");
    rt.pointer_move(para[1].0, para[1].1);
    tick(&mut rt);
    assert_eq!(rt.cursor(), Cursor::Default);
    let code = glyphs(&rt, "m0/b2");
    drag(&mut rt, before(para[1]), before(code[2]));
    assert_eq!(rt.selected_text(), None);
}

#[test]
fn inline_code_copies_without_its_padding() {
    let mut rt = runtime_with(&["Run `cargo test` now"], true);
    let g = glyphs(&rt, "m0/b0");
    let a = (g[4].0 - 1.0, g[4].1);
    for _ in 0..2 {
        rt.pointer_move(a.0, a.1);
        rt.pointer_down(a.0, a.1);
        rt.pointer_up(a.0, a.1);
        tick(&mut rt);
    }
    assert_eq!(rt.selected_text().as_deref(), Some("cargo"), "a double-click's word stops at the padding");
    command(&mut rt, 'a');
    assert_eq!(rt.selected_text().as_deref(), Some("Run cargo test now"));
    assert_eq!(find(&rt, "m0/b0").selected.as_deref(), Some("Run cargo test now"));
}

#[test]
fn highlighted_code_copies_as_plain_text() {
    let code = "def f(x):\n    return f\"{x!r}\"  # done";
    let mut rt = runtime_with(&[&format!("```python\n{code}\n```\n")], true);
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    let g = glyphs(&rt, "m0/b0");
    for _ in 0..2 {
        rt.pointer_move(g[1].0, g[1].1);
        rt.pointer_down(g[1].0, g[1].1);
        rt.pointer_up(g[1].0, g[1].1);
        tick(&mut rt);
    }
    assert_eq!(rt.selected_text().as_deref(), Some("def"), "a double-click picks the keyword's word");
    command(&mut rt, 'a');
    command(&mut rt, 'c');
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some(code));
}

/// A tab draws to the next four-column stop and copies as the tab.
#[test]
fn tabs_line_up_and_copy_as_tabs() {
    // Each tabbed line, then the same line indented with spaces.
    let code = "\tif x {\n    if x {\n\t\treturn\n        return\n  \tgo\n    go\nab\tc\nab  c";
    let mut rt = runtime_with(&[&format!("```go\n{code}\n```\n")], true);
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    let mut lines: Vec<Vec<f32>> = Vec::new();
    let mut y = f32::NAN;
    for (gx, gy) in glyphs(&rt, "m0/b0") {
        if y.is_nan() || (gy - y).abs() > 2.0 {
            lines.push(Vec::new());
            y = gy;
        }
        lines.last_mut().unwrap().push(gx);
    }
    assert_eq!(lines.len(), 8);
    for pair in lines.chunks(2) {
        assert_eq!(pair[0], pair[1]);
    }
    let first = glyphs(&rt, "m0/b0")[0];
    drag(&mut rt, before(first), first);
    command(&mut rt, 'a');
    command(&mut rt, 'c');
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some(code));
}
