//! `El::caret_browsing`: the arrow keys move selectable text's caret, Shift extends the
//! selection, and at a text's edge the caret steps into the next text of its selection group.
use crate::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, TextStyle, col, text};

fn view(browse: &bool, _: &mut Cx<bool>) -> El {
    let t = |key: &str, s: &str| text(s).key(key).font(TextStyle::new(14.0, 400.0)).w_full().selectable(true).caret_browsing(*browse);
    col().key("root").w(300.0).padding(10.0).selection_group(true)
        .child(t("a", "one two"))
        .child(t("b", "three"))
}

fn runtime(browse: bool) -> Runtime<bool> {
    let mut rt = Runtime::new(browse, view, (320.0, 120.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.update(|_, tx| tx.select_text("a", 2..2));
    rt.frame(1.0 / 60.0);
    rt
}

fn key(rt: &mut Runtime<bool>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    rt.frame(1.0 / 60.0);
}

fn at(rt: &Runtime<bool>) -> (String, std::ops::Range<usize>) {
    let (path, range) = rt.text_selection().expect("a selection");
    (path.rsplit('/').next().unwrap_or(path).to_owned(), range)
}

#[test]
fn arrows_move_the_caret_and_shift_extends_across_texts() {
    let shift = Mods { shift: true, ..Mods::default() };
    let mut rt = runtime(true);
    key(&mut rt, Key::Right, Mods::default());
    assert_eq!(at(&rt), ("a".to_owned(), 3..3), "one character right");
    assert!(rt.core().browse_caret().is_some(), "an empty selection draws the caret");
    key(&mut rt, Key::End, Mods::default());
    assert_eq!(at(&rt), ("a".to_owned(), 7..7), "the line's end");
    key(&mut rt, Key::Right, Mods::default());
    assert_eq!(at(&rt), ("b".to_owned(), 0..0), "past the text's end into the next one");
    key(&mut rt, Key::Left, shift);
    key(&mut rt, Key::Left, shift);
    assert_eq!(at(&rt), ("b".to_owned(), 0..0), "the anchor stays in b");
    assert_eq!(rt.core().selection_ranges().len(), 1, "Shift reached back into a");
    let (j, a, b) = rt.core().selection_ranges()[0];
    assert_eq!((rt.core().content_of(j), a, b), ("one two", 6, 7));
    assert!(rt.core().browse_caret().is_none(), "a selection hides the caret");
    key(&mut rt, Key::Left, Mods::default());
    assert_eq!(at(&rt), ("a".to_owned(), 6..6), "Left collapses to the selection's start");
    let doc_end = Mods { cmd: true, ..Mods::default() };
    key(&mut rt, Key::End, doc_end);
    assert_eq!(at(&rt), ("b".to_owned(), 5..5), "the group's end");
}

#[test]
fn text_without_it_leaves_the_keys_alone() {
    let mut rt = runtime(false);
    key(&mut rt, Key::Right, Mods::default());
    assert_eq!(at(&rt), ("a".to_owned(), 2..2));
    assert!(rt.core().browse_caret().is_none());
}

#[test]
fn up_and_down_step_between_texts_at_the_caret_column() {
    let mut rt = runtime(true);
    key(&mut rt, Key::Down, Mods::default());
    let (path, range) = at(&rt);
    assert_eq!(path, "b", "down from a's only line into b");
    assert!(range.start == range.end && range.start <= 3, "near a's column 2, got {range:?}");
    key(&mut rt, Key::Up, Mods::default());
    assert_eq!(at(&rt).0, "a", "and back up");
}
