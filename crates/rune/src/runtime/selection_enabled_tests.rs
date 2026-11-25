//! `El::selection_enabled`: a `false` subtree's selectable texts don't select by drag,
//! double-click, Cmd+A or `Tx::select_*`, lose the I-beam `El::selectable` gave them and leave
//! their selection group; a descendant's `true` brings them back; the switch reaches memos and
//! unbuilt virtual rows, and a selection it turns off clears.
use super::*;
use crate::runtime::selectable::virtual_selection::collect_virtual_row_selection;
use crate::{Cursor, Cx, El, Env, GrowthSpec, Key, KeyEvent, Mods, ScrollSpec, TextStyle, col, el, memo, text};

fn style() -> TextStyle { TextStyle::new(14.0, 400.0).lh(20.0) }

fn t(content: &str, key: &str) -> El {
    text(content.to_owned()).key(key.to_owned()).font(style()).w(200.0).selectable(true)
}

#[derive(Default)]
struct S {
    /// The root's `selection_enabled`, when it sets one.
    on: Option<bool>,
    /// The id opts back in with `selection_enabled(true)`.
    id_on: bool,
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    let id = if s.id_on { t("ID 4f2a", "id").selection_enabled(true) } else { t("ID 4f2a", "id") };
    let root = col().key("root").size(300.0, 300.0).padding(10.0).gap(4.0).selection_group(true)
        .child(t("alpha one", "a"))
        .child(id)
        .child(memo("m", 0u8, || col().child(t("memo body", "mb"))))
        .child(el().key("blank").size(200.0, 60.0));
    match s.on {
        Some(on) => root.selection_enabled(on),
        None => root,
    }
}

fn runtime(on: Option<bool>, id_on: bool) -> Runtime<S> {
    let mut rt = Runtime::new(S { on, id_on }, view, (300.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn rect<St: 'static>(rt: &Runtime<St>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("missing {key}")).rect
}

fn select_all<St: 'static>(rt: &mut Runtime<St>) {
    rt.key(KeyEvent { key: Key::Command('a'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(0.0);
}

fn set<St: 'static>(rt: &mut Runtime<St>, f: impl FnOnce(&mut St)) {
    rt.update(|s, _| f(s));
    rt.frame(0.0);
}

fn cursor_over<St: 'static>(rt: &mut Runtime<St>, key: &str) -> Cursor {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing {key}"));
    rt.pointer_move(x, y);
    rt.frame(0.0);
    rt.cursor()
}

/// Drags from the start of `key`'s line to past its end.
fn drag_across(rt: &mut Runtime<S>, key: &str) -> Option<String> {
    let r = rect(rt, key);
    let y = r[1] + r[3] * 0.5;
    rt.pointer_move(r[0] + 1.0, y);
    rt.pointer_down(r[0] + 1.0, y);
    rt.frame(0.0);
    rt.pointer_move(r[0] + r[2] - 2.0, y);
    rt.frame(0.0);
    rt.pointer_up(r[0] + r[2] - 2.0, y);
    rt.frame(0.0);
    rt.selected_text()
}

/// Double-clicks the first word of `key`.
fn double_click(rt: &mut Runtime<S>, key: &str) -> Option<String> {
    let r = rect(rt, key);
    let (x, y) = (r[0] + 8.0, r[1] + r[3] * 0.5);
    rt.pointer_move(x, y);
    for _ in 0..2 {
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
    }
    rt.frame(0.0);
    rt.selected_text()
}

#[test]
fn selectable_text_selects_by_default() {
    let mut rt = runtime(None, false);
    assert_eq!(cursor_over(&mut rt, "a"), Cursor::Text);
    assert_eq!(drag_across(&mut rt, "a").as_deref(), Some("alpha one"));
    let mut rt = runtime(None, false);
    assert!(double_click(&mut rt, "a").is_some_and(|w| w.starts_with("alpha")));
    let mut rt = runtime(None, false);
    select_all(&mut rt);
    assert_eq!(rt.selected_text().as_deref(), Some("alpha one\nID 4f2a\nmemo body"));
}

#[test]
fn an_off_subtree_selects_nothing() {
    let mut rt = runtime(Some(false), false);
    assert_ne!(cursor_over(&mut rt, "a"), Cursor::Text, "the I-beam selectable gave goes");
    assert_eq!(drag_across(&mut rt, "a"), None);
    assert_eq!(double_click(&mut rt, "a"), None);
    select_all(&mut rt);
    assert_eq!(rt.selected_text(), None);
    rt.update(|_, tx| tx.select_all_text("root"));
    rt.frame(0.0);
    assert_eq!(rt.selected_text(), None);
    rt.update(|_, tx| tx.select_all_text("a"));
    rt.frame(0.0);
    assert_eq!(rt.selected_text(), None);
    rt.update(|_, tx| tx.select_text("a", 0..5));
    rt.frame(0.0);
    assert_eq!(rt.selected_text(), None);
}

#[test]
fn a_descendant_opts_back_in_and_its_group_skips_the_rest() {
    let mut rt = runtime(Some(false), true);
    assert_eq!(cursor_over(&mut rt, "id"), Cursor::Text);
    assert_ne!(cursor_over(&mut rt, "a"), Cursor::Text);
    select_all(&mut rt);
    assert_eq!(rt.selected_text().as_deref(), Some("ID 4f2a"), "the group holds only the opted-in text");
    rt.update(|_, tx| tx.select_all_text("root"));
    rt.frame(0.0);
    assert_eq!(rt.selected_text().as_deref(), Some("ID 4f2a"));
    let mut rt = runtime(Some(false), true);
    assert_eq!(drag_across(&mut rt, "id").as_deref(), Some("ID 4f2a"));
}

#[test]
fn turning_it_back_on_restores_selection() {
    let mut rt = runtime(Some(false), false);
    set(&mut rt, |s| s.on = Some(true));
    select_all(&mut rt);
    assert_eq!(rt.selected_text().as_deref(), Some("alpha one\nID 4f2a\nmemo body"));
}

#[test]
fn a_memo_follows_its_ancestors_switch() {
    let mut rt = runtime(None, true);
    select_all(&mut rt);
    assert_eq!(rt.selected_text().as_deref(), Some("alpha one\nID 4f2a\nmemo body"));
    rt.update(|_, tx| tx.clear_text_selection());
    rt.frame(0.0);

    // The memo's version doesn't change: only what it inherits does.
    set(&mut rt, |s| s.on = Some(false));
    select_all(&mut rt);
    assert_eq!(rt.selected_text().as_deref(), Some("ID 4f2a"));
    rt.update(|_, tx| tx.clear_text_selection());
    rt.frame(0.0);

    set(&mut rt, |s| s.on = None);
    select_all(&mut rt);
    assert_eq!(rt.selected_text().as_deref(), Some("alpha one\nID 4f2a\nmemo body"));
}

#[test]
fn turning_it_off_clears_a_held_selection() {
    let mut rt = runtime(None, false);
    select_all(&mut rt);
    assert!(rt.selected_text().is_some());
    set(&mut rt, |s| s.on = Some(false));
    assert_eq!(rt.selected_text(), None);

    let mut rt = runtime(None, false);
    assert_eq!(drag_across(&mut rt, "a").as_deref(), Some("alpha one"));
    set(&mut rt, |s| s.on = Some(false));
    assert_eq!(rt.selected_text(), None);
}

fn cursor_view(_: &(), _: &mut Cx<()>) -> El {
    col().key("root").size(300.0, 200.0).padding(10.0).gap(4.0).selection_enabled(false)
        .child(t("own cursor", "own").cursor(Cursor::Text))
        .child(t("given cursor", "given"))
        .child(t("hand", "hand").cursor(Cursor::Pointer))
        .child(mixed("mixed"))
        .child(col().selection_enabled(true).child(mixed("mixed_on")))
}

/// A selectable text whose paragraphs differ in direction: `dir_by_paragraph` makes it a column
/// of one text per paragraph.
fn mixed(key: &str) -> El {
    text("Hello there\n\u{645}\u{631}\u{62d}\u{628}\u{627}".to_owned()).key(key.to_owned()).font(style()).w(200.0).selectable(true).dir_by_paragraph()
}

fn path_cursor(rt: &Runtime<()>, suffix: &str) -> Option<Cursor> {
    rt.tree.nodes.iter().find(|n| n.path.ends_with(suffix)).unwrap_or_else(|| panic!("missing {suffix}")).handlers.cursor
}

/// The cursor a node keeps, read off the node: a text that no longer selects isn't a hit target
/// by itself, so the pointer can't show it.
fn node_cursor(rt: &Runtime<()>, key: &str) -> Option<Cursor> {
    rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("missing {key}")).handlers.cursor
}

#[test]
fn only_the_cursor_selectable_gave_goes() {
    let mut rt = Runtime::new((), cursor_view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(node_cursor(&rt, "own"), Some(Cursor::Text), "set with El::cursor, so it stays");
    assert_eq!(node_cursor(&rt, "given"), None);
    assert_eq!(node_cursor(&rt, "hand"), Some(Cursor::Pointer));
    // Each paragraph of a mixed-direction text drops the I-beam, and so does their column.
    assert_eq!(path_cursor(&rt, "/mixed"), None);
    assert_eq!(path_cursor(&rt, "/mixed/p0"), None);
    assert_eq!(path_cursor(&rt, "/mixed/p1"), None);
    // Opted back in, the paragraphs keep it.
    assert_eq!(path_cursor(&rt, "/mixed_on/p0"), Some(Cursor::Text));
    assert_eq!(path_cursor(&rt, "/mixed_on/p1"), Some(Cursor::Text));
}

#[derive(Default)]
struct Caret { on: bool }

fn caret_view(s: &Caret, _: &mut Cx<Caret>) -> El {
    col().key("root").size(300.0, 200.0).padding(10.0).selection_enabled(s.on)
        .child(col().key("doc").selection_group(true).caret_browsing(true).child(t("caret text", "c")))
}

#[test]
fn a_caret_browsing_group_in_an_off_subtree_takes_no_focus() {
    let mut rt = Runtime::new(Caret { on: true }, caret_view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let focusable = |rt: &Runtime<Caret>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("doc")).unwrap().focusable;
    assert!(focusable(&rt));
    set(&mut rt, |s| s.on = false);
    assert!(!focusable(&rt));
}

const ROWS: usize = 40;
const ROW_H: f32 = 56.0;

#[derive(Default)]
struct V {
    on: Option<bool>,
    /// Each row's header opts back in.
    heads: bool,
}

fn v_row(s: &V, i: usize) -> El {
    let head = text(format!("Row {i:02} header")).key("head").font(style()).w_full().selectable(true);
    let head = if s.heads { head.selection_enabled(true) } else { head };
    let body = text(format!("Body {i:02}")).key("body").font(style()).w_full().selectable(true);
    col().w_full().h(ROW_H).padding(6.0).gap(4.0).children([head, body])
}

fn v_view(s: &V, _: &mut Cx<V>) -> El {
    let root = col().w_full().h_full().child(
        col().key("scroll").w_full().grow(1.0).scroll(ScrollSpec { stick_bottom: false }).child(
            col()
                .key("timeline")
                .w_full()
                .growth(GrowthSpec::default())
                .selection_group(true)
                .virtual_rows(ROWS, ROW_H, |_: &V, i| format!("r{i}").into(), v_row),
        ),
    );
    match s.on {
        Some(on) => root.selection_enabled(on),
        None => root,
    }
}

fn timeline(on: Option<bool>, heads: bool) -> Runtime<V> {
    let mut rt = Runtime::new(V { on, heads }, v_view, (1280.0, 420.0), Env::new(true, 2.0));
    rt.frame(0.0);
    while rt.frame(1.0 / 120.0) {}
    rt
}

fn scroll_down(rt: &mut Runtime<V>) {
    for _ in 0..6 {
        rt.wheel(640.0, 210.0, -160.0);
        rt.frame(1.0 / 60.0);
    }
    for _ in 0..20 {
        if !rt.frame(1.0 / 60.0) { break; }
    }
}

#[test]
fn virtual_rows_in_an_off_subtree_select_nothing() {
    let mut rt = timeline(Some(false), false);
    select_all(&mut rt);
    assert_eq!(rt.selected_text(), None);
    rt.update(|_, tx| tx.select_all_text("timeline"));
    rt.frame(0.0);
    assert_eq!(rt.selected_text(), None);
    scroll_down(&mut rt);
    rt.update(|_, tx| tx.select_all_text("timeline"));
    rt.frame(0.0);
    assert_eq!(rt.selected_text(), None);
}

#[test]
fn unbuilt_virtual_rows_keep_only_their_opted_in_texts() {
    let expected = (0..ROWS).map(|i| format!("Row {i:02} header")).collect::<Vec<_>>().join("\n");
    let mut rt = timeline(Some(false), true);
    rt.update(|_, tx| tx.select_all_text("timeline"));
    rt.frame(0.0);
    assert_eq!(rt.selected_text().as_deref(), Some(expected.as_str()));
    // Scrolling remounts the window with the selection's end rows pinned offscreen.
    scroll_down(&mut rt);
    assert_eq!(rt.selected_text().as_deref(), Some(expected.as_str()));
}

#[test]
fn an_off_row_collects_only_the_texts_it_opts_back_in() {
    let row = col().key("r0")
        .child(text("body").key("body").selectable(true))
        .child(col().selection_enabled(true).child(text("id").key("id").selectable(true)));
    let (mut texts, mut providers) = (Vec::new(), Vec::new());
    collect_virtual_row_selection(&row, "/list", "r0", 0, None, false, true, &mut texts, &mut providers);
    assert_eq!(texts.iter().map(|t| t.key.as_deref()).collect::<Vec<_>>(), [Some("id")]);
    texts.clear();
    collect_virtual_row_selection(&row, "/list", "r0", 0, None, false, false, &mut texts, &mut providers);
    assert_eq!(texts.iter().map(|t| t.key.as_deref()).collect::<Vec<_>>(), [Some("body"), Some("id")]);
}
