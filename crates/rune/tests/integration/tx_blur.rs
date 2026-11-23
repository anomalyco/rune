//! `Tx::blur` drops keyboard focus from a handler.
use rune::tok::TextStyle;
use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, TextEdit, col, text_input};

#[derive(Default)]
struct S {
    field: TextEdit,
    submits: u32,
    keys: u32,
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    col().size(200.0, 100.0).child(
        text_input(&s.field, TextStyle::new(14.0, 400.0)).key("field").w(150.0)
            .on_edit(|s: &mut S| &mut s.field)
            .on_submit(|s: &mut S, tx| { s.submits += 1; tx.blur(); }),
    )
}

fn focused(rt: &Runtime<S>) -> bool {
    rt.elements().iter().any(|e| e.key.as_deref() == Some("field") && e.focused)
}

fn runtime() -> Runtime<S> {
    let mut s = S::default();
    s.field.focused = true;
    let mut rt = Runtime::new(s, view, (200.0, 100.0), Env::default());
    rt.on_key = Some(|s, _, _| { s.keys += 1; true });
    rt.frame(0.0);
    assert!(focused(&rt), "the model asked for focus");
    rt
}

fn press(rt: &mut Runtime<S>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
}

#[test]
fn a_submit_handler_blurs_its_field() {
    let mut rt = runtime();
    press(&mut rt, Key::Enter);
    assert_eq!(rt.state.submits, 1);
    assert!(!focused(&rt), "nothing has focus");
    assert!(!rt.state.field.focused, "the model hears it through on_edit");
    rt.frame(1.0 / 60.0);
    assert!(!focused(&rt), "and focus stays dropped");
    press(&mut rt, Key::Text("z".into()));
    assert_eq!(rt.state.field.text.as_str(), "", "typing no longer reaches the field");
    assert_eq!(rt.state.keys, 1, "keys go to the window");
}

#[test]
fn an_update_blurs_and_asks_for_a_frame() {
    let mut rt = runtime();
    assert!(rt.update_if(|_, tx| { tx.blur(); false }), "a blur that drops focus is frame work");
    rt.frame(1.0 / 60.0);
    assert!(!focused(&rt));
    assert!(!rt.update_if(|_, tx| { tx.blur(); false }), "with nothing focused it's nothing");
}

struct Inline {
    title: Option<TextEdit>,
    other: TextEdit,
    other_accesses: u32,
    submits: u32,
    keys: u32,
}

fn inline_view(s: &Inline, _: &mut Cx<Inline>) -> El {
    let style = TextStyle::new(14.0, 400.0);
    let mut root = col().size(200.0, 140.0);
    if let Some(title) = &s.title {
        root = root.child(
            text_input(title, style).single_line().key("title").size(150.0, 30.0)
                .on_edit(|s: &mut Inline| s.title.as_mut().expect("removed title's on_edit called"))
                .on_submit(|s: &mut Inline, tx| { s.submits += 1; s.title = None; tx.blur(); }),
        );
    }
    root.child(
        text_input(&s.other, style).single_line().key("other").size(150.0, 30.0)
            .on_edit(|s: &mut Inline| { s.other_accesses += 1; &mut s.other }),
    )
}

fn inline_runtime() -> Runtime<Inline> {
    let mut title = TextEdit::new("draft");
    title.focused = true;
    let mut rt = Runtime::new(
        Inline { title: Some(title), other: TextEdit::default(), other_accesses: 0, submits: 0, keys: 0 },
        inline_view,
        (200.0, 140.0),
        Env::default(),
    );
    rt.on_key = Some(|s, _, _| { s.keys += 1; true });
    rt.frame(0.0);
    assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("title") && e.focused));
    rt.state.other_accesses = 0;
    rt
}

#[test]
fn a_submit_that_removes_its_field_and_blurs_skips_removed_and_unrelated_accessors() {
    let mut rt = inline_runtime();
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.submits, 1);
    assert!(rt.state.title.is_none());
    assert!(!rt.elements().iter().any(|e| e.focused), "nothing takes focus after Tx::blur");
    assert!(!rt.state.other.focused);
    assert_eq!(rt.state.other_accesses, 0, "blurring title must not call other's on_edit");
}

#[test]
fn an_update_that_removes_its_field_and_blurs_skips_removed_and_unrelated_accessors() {
    let mut rt = inline_runtime();
    rt.update(|s, tx| { s.title = None; tx.blur(); });
    rt.frame(1.0 / 60.0);
    assert!(rt.state.title.is_none());
    assert!(!rt.elements().iter().any(|e| e.focused));
    assert!(!rt.state.other.focused);
    assert_eq!(rt.state.other_accesses, 0, "blurring title must not call other's on_edit");
}

#[test]
fn a_same_frame_key_after_submit_blur_reaches_the_window() {
    let mut rt = inline_runtime();
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.key(KeyEvent { key: Key::Text("x".into()), mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.submits, 1);
    assert!(rt.state.title.is_none());
    assert_eq!(rt.state.other.text.as_str(), "");
    assert_eq!(rt.state.keys, 1, "a key queued right after submit+blur reaches the window");
}

#[test]
fn escape_and_pointer_blur_do_not_call_unrelated_fields_on_edit() {
    let mut rt = inline_runtime();
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.title.as_ref().unwrap().focused);
    assert_eq!(rt.state.other_accesses, 0, "Escape blur must not call other's on_edit");

    rt.update(|s, _| s.title.as_mut().unwrap().focused = true);
    rt.frame(1.0 / 60.0);
    rt.state.other_accesses = 0;
    rt.pointer_down(190.0, 130.0);
    rt.pointer_up(190.0, 130.0);
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.title.as_ref().unwrap().focused);
    assert_eq!(rt.state.other_accesses, 0, "pointer blur must not call other's on_edit");
}
