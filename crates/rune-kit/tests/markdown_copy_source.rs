//! `Markdown::copy_source`: copying a selection copies the Markdown source of the top-level
//! blocks it touches, the whole source verbatim for all of them, and the drawn text for part
//! of one block. And an app's `on_key` sees Cmd+A before a selectable note does.
#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]
#![cfg(feature = "markdown")]

use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, col};
use rune_kit::{Kit, Markdown};

const SRC: &str = "# Title\n\nHello **world** and [link](https://x.dev/a).\n\n- alpha\n- [x] beta\n\n```rust\nfn main() {}\n```\n\nLast *line*.\n";

#[derive(Default)]
struct S { source: bool }

fn view(s: &S, _: &mut Cx<S>) -> El {
    let k = Kit::neutral(true);
    col().w_full().padding(10.0).child(Markdown::new(SRC).selectable(true).copy_source(s.source).build(&k).key("note"))
}

fn runtime(source: bool) -> (Runtime<S>, rune::clipboard::MemoryClipboard) {
    let mut rt = Runtime::new(S { source }, view, (480.0, 800.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    (rt, clipboard)
}

fn tick<T: 'static>(rt: &mut Runtime<T>) { rt.frame(1.0 / 60.0); }

fn copy<T: 'static>(rt: &mut Runtime<T>, clipboard: &rune::clipboard::MemoryClipboard, f: impl FnOnce(&mut rune::Tx)) -> String {
    rt.update(|_, tx| f(tx));
    tick(rt);
    rt.key(KeyEvent { key: Key::Command('c'), mods: Mods { cmd: true, ..Mods::default() } });
    tick(rt);
    clipboard.text().map(|t| t.to_string()).unwrap_or_default()
}

#[test]
fn copying_takes_the_source_of_whole_blocks() {
    let (mut rt, clipboard) = runtime(true);
    assert_eq!(copy(&mut rt, &clipboard, |tx| tx.select_all_text("note")), SRC, "everything: the source verbatim");
    assert_eq!(rt.selected_text().as_deref().map(|t| t.starts_with("Title\nHello world")), Some(true), "selected as drawn");
    assert_eq!(copy(&mut rt, &clipboard, |tx| tx.select_all_text("note/b1")), "Hello **world** and [link](https://x.dev/a).");
    assert_eq!(copy(&mut rt, &clipboard, |tx| tx.select_all_text("note/b2")), "- alpha\n- [x] beta", "a whole list");
    assert_eq!(copy(&mut rt, &clipboard, |tx| tx.select_all_text("note/b3")), "```rust\nfn main() {}\n```");
    // Part of a paragraph or of a list: as drawn.
    assert_eq!(copy(&mut rt, &clipboard, |tx| tx.select_text("note/b1", 6..11)), "world");
    let alpha = rt.elements().into_iter().find(|e| e.path.ends_with("note/b2/i0/#1/b0")).expect("alpha").path;
    assert_eq!(copy(&mut rt, &clipboard, |tx| tx.select_all_text(alpha)), "alpha");
}

#[test]
fn a_drag_across_blocks_copies_each_whole() {
    let (mut rt, clipboard) = runtime(true);
    let find = |rt: &Runtime<S>, end: &str| rt.elements().into_iter().find(|e| e.path.ends_with(end)).unwrap_or_else(|| panic!("{end}"));
    let (para, last) = (find(&rt, "note/b1"), find(&rt, "note/b4"));
    let (x0, y0) = (para.rect[0] + 30.0, para.rect[1] + para.rect[3] / 2.0);
    let (x1, y1) = (last.rect[0] + 10.0, last.rect[1] + last.rect[3] / 2.0);
    rt.pointer_move(x0, y0);
    rt.pointer_down(x0, y0);
    tick(&mut rt);
    for k in 1..=6 {
        let f = k as f32 / 6.0;
        rt.pointer_move(x0 + (x1 - x0) * f, y0 + (y1 - y0) * f);
        tick(&mut rt);
    }
    rt.pointer_up(x1, y1);
    tick(&mut rt);
    assert!(rt.selected_text().is_some_and(|t| t.contains("alpha") && !t.starts_with("Hello")), "{:?}", rt.selected_text());
    rt.key(KeyEvent { key: Key::Command('c'), mods: Mods { cmd: true, ..Mods::default() } });
    tick(&mut rt);
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some(&SRC[9..]).map(|s| s.trim_end()));
}

#[test]
fn without_copy_source_copying_takes_the_drawn_text() {
    let (mut rt, clipboard) = runtime(false);
    let all = copy(&mut rt, &clipboard, |tx| tx.select_all_text("note"));
    assert_eq!(all, "Title\nHello world and link.\nalpha\nbeta\nfn main() {}\nLast line.");
}

#[derive(Default)]
struct Keys { takes: bool, keys: Vec<Key> }

fn short(_: &Keys, _: &mut Cx<Keys>) -> El {
    col().w_full().padding(10.0).child(Markdown::new("a\n\nb").selectable(true).build(&Kit::neutral(true)).key("note"))
}

#[test]
fn an_apps_on_key_sees_cmd_a_before_a_clicked_note() {
    for takes in [true, false] {
        let mut rt = Runtime::new(Keys { takes, keys: Vec::new() }, short, (300.0, 200.0), Env::new(true, 2.0));
        rt.on_key = Some(|s, ev, _| { s.keys.push(ev.key.clone()); s.takes && ev.key == Key::Command('a') });
        rt.frame(0.0);
        let a = rt.elements().into_iter().find(|e| e.path.ends_with("note/b0")).expect("the first paragraph");
        let (x, y) = (a.rect[0] + 3.0, a.rect[1] + a.rect[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        rt.key(KeyEvent { key: Key::Command('a'), mods: Mods { cmd: true, ..Mods::default() } });
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.keys, [Key::Command('a')], "takes={takes}");
        let want = if takes { None } else { Some("a\nb") };
        assert_eq!(rt.selected_text().as_deref(), want, "takes={takes}: passed on, the note selects all");
    }
}

#[test]
fn inline_code_rule_and_crlf_blocks_copy_cleanly() {
    #[derive(Default)]
    struct Doc { src: &'static str }
    fn doc_view(s: &Doc, _: &mut Cx<Doc>) -> El {
        col().w_full().padding(10.0).child(Markdown::new(s.src).selectable(true).copy_source(true).build(&Kit::neutral(true)).key("note"))
    }
    let run = |src: &'static str| {
        let mut rt = Runtime::new(Doc { src }, doc_view, (480.0, 800.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let clipboard = rune::clipboard::memory();
        rt.use_clipboard(&clipboard);
        (rt, clipboard)
    };

    // Document starting and ending with `---` rules returns the full source verbatim on Select All.
    let ruled = "---\n\nHello **world**\n\n---\n";
    let (mut rt, clipboard) = run(ruled);
    assert_eq!(copy(&mut rt, &clipboard, |tx| tx.select_all_text("note")), ruled);

    // Double-clicking a single inline-code word selects the whole block despite uncopied padding.
    let inline = "`code`\n";
    let (mut rt, clipboard) = run(inline);
    let b0 = rt.elements().into_iter().find(|e| e.path.ends_with("note/b0")).expect("b0");
    let (x, y) = (b0.rect[0] + 12.0, b0.rect[1] + b0.rect[3] / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    tick(&mut rt);
    rt.key(KeyEvent { key: Key::Command('c'), mods: Mods { cmd: true, ..Mods::default() } });
    tick(&mut rt);
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some(inline));

    // CRLF code and diff blocks strip `\r` from drawn text while preserving source copy.
    let crlf = "```rust\r\nfn main() {}\r\n```\r\n\r\n```diff\r\n@@ -1 +1 @@\r\n-old\r\n+new\r\n```\r\n";
    let (mut rt, clipboard) = run(crlf);
    assert_eq!(copy(&mut rt, &clipboard, |tx| tx.select_all_text("note")), crlf);
    assert_eq!(rt.selected_text().as_deref(), Some("fn main() {}\n@@ -1 +1 @@\n-old\n+new"));
}
