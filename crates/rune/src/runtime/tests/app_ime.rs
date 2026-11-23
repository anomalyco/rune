use super::*;
use crate::{Key, KeyEvent, Mods, TextEdit, TextStyle, text_input};

#[derive(Default)]
struct Editor {
    /// What `App::on_ime` and `on_key` heard, in order.
    log: Vec<String>,
    field: TextEdit,
    /// The app's editor draws its caret (and so has the input method).
    caret: bool,
}

fn view(s: &Editor, _: &mut Cx<Editor>) -> El {
    col().size(300.0, 200.0).children([
        text_input(&s.field, TextStyle::new(14.0, 400.0)).key("field").size(200.0, 30.0).on_edit(|s: &mut Editor| &mut s.field),
        el().key("page").size(300.0, 100.0)
            .on_key_up(|s: &mut Editor, ev, _| { s.log.push(format!("up {:?}", ev.key)); true })
            .children(s.caret.then(|| el().key("caret").absolute(40.0, 20.0).size(2.0, 18.0).ime_cursor())),
    ])
}

fn editor() -> Runtime<Editor> {
    let mut rt = Runtime::new(Editor { caret: true, ..Default::default() }, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.hooks.on_ime = Some(|s, ev, _| { s.log.push(format!("{ev:?}")); true });
    rt.on_key = Some(|s, ev, _| { s.log.push(format!("key {:?}", ev.key)); true });
    rt.frame(0.0);
    rt
}

fn press(rt: &mut Runtime<Editor>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    rt.frame(0.0);
}

fn release(rt: &mut Runtime<Editor>, key: Key) {
    rt.key_up(KeyEvent { key, mods: Mods::default() });
    rt.frame(0.0);
}

#[test]
fn an_ime_cursor_turns_the_input_method_on_at_its_rect_unless_a_field_has_focus() {
    let mut rt = editor();
    assert_eq!(rt.ime_area, Some([40.0, 50.0, 2.0, 18.0]), "the caret's drawn rect, below the 30 pt field");
    rt.update(|s, _| s.field.focused = true);
    rt.frame(0.0);
    let field = rt.ime_area.expect("the field's caret");
    assert!(field[1] < 30.0, "a focused field's caret wins: {field:?}");
    rt.update(|s, _| { s.field.focused = false; s.caret = false; });
    rt.frame(0.0);
    assert_eq!(rt.ime_area, None, "no caret, no input method");
}

#[test]
fn the_app_hears_the_composition_and_typing_arrives_once() {
    let mut rt = editor();
    press(&mut rt, Key::Text("a".into()));
    rt.ime(Ime::Enabled);
    rt.ime(Ime::Preedit("か".into(), Some((0, 3)), Vec::new()));
    rt.frame(0.0);
    press(&mut rt, Key::Enter);
    press(&mut rt, Key::Escape);
    rt.ime(Ime::Commit("家".into()));
    rt.frame(0.0);
    press(&mut rt, Key::Enter);
    assert_eq!(rt.state.log, [
        "key Text(\"a\")", "Enabled", "Preedit(\"か\", Some((0, 3)), [])", "Commit(\"家\")", "key Enter",
    ], "plain typing is a key; Return and Escape mid-composition are the input method's");
}

#[test]
fn a_focused_field_takes_the_input_method_but_the_apps_composition_ends_in_the_app() {
    let mut rt = editor();
    rt.ime(Ime::Preedit("か".into(), None, Vec::new()));
    rt.frame(0.0);
    rt.update(|s, _| s.field.focused = true);
    rt.frame(0.0);
    // Turning the input method over to the field ends the app's composition.
    rt.ime(Ime::Preedit(String::new(), None, Vec::new()));
    rt.ime(Ime::Disabled);
    rt.ime(Ime::Preedit("ね".into(), None, Vec::new()));
    rt.ime(Ime::Commit("猫".into()));
    rt.frame(0.0);
    assert_eq!(rt.state.log, ["Preedit(\"か\", None, [])", "Preedit(\"\", None, [])", "Disabled"]);
    assert_eq!(rt.state.field.text, "猫", "the field's own composition stays the field's");
}

#[test]
fn without_on_ime_an_ime_cursor_hears_nothing() {
    let mut rt = editor();
    rt.hooks.on_ime = None;
    rt.ime(Ime::Preedit("か".into(), None, Vec::new()));
    rt.frame(0.0);
    press(&mut rt, Key::Enter);
    assert_eq!(rt.state.log, ["key Enter"], "nothing composing holds Return back");
}

#[test]
fn ime_disabled_ends_app_composition_and_restores_keys_and_releases() {
    let mut rt = editor();
    rt.ime(Ime::Preedit("^".into(), Some((0, 1)), Vec::new()));
    rt.frame(0.0);
    // Caret disappears mid-composition; the platform turns IME off with Ime::Disabled alone.
    rt.update(|s, _| s.caret = false);
    rt.frame(0.0);
    assert_eq!(rt.ime_area, None);
    rt.ime(Ime::Disabled);
    rt.frame(0.0);
    release(&mut rt, Key::Text("a".into()));
    press(&mut rt, Key::Enter);
    release(&mut rt, Key::Enter);
    press(&mut rt, Key::Escape);
    release(&mut rt, Key::Escape);
    assert_eq!(rt.state.log, [
        r#"Preedit("^", Some((0, 1)), [])"#,
        r#"Preedit("", None, [])"#,
        "Disabled",
        r#"up Text("a")"#,
        "key Enter",
        "up Enter",
        "key Escape",
        "up Escape",
    ]);
}

#[test]
fn ime_disabled_without_empty_preedit_unblocks_keys_and_field_handoff() {
    let mut rt = editor();
    // Switching input source sends Ime::Disabled alone while the caret is still present.
    rt.ime(Ime::Preedit("^".into(), None, Vec::new()));
    rt.frame(0.0);
    rt.ime(Ime::Disabled);
    rt.frame(0.0);
    press(&mut rt, Key::Enter);
    release(&mut rt, Key::Enter);
    assert_eq!(rt.state.log, [
        r#"Preedit("^", None, [])"#,
        r#"Preedit("", None, [])"#,
        "Disabled",
        "key Enter",
        "up Enter",
    ]);

    // Focusing a text field when the platform sends only Ime::Disabled still hands off cleanly.
    rt.state.log.clear();
    rt.ime(Ime::Preedit("か".into(), None, Vec::new()));
    rt.frame(0.0);
    rt.update(|s, _| s.field.focused = true);
    rt.frame(0.0);
    rt.ime(Ime::Disabled);
    rt.ime(Ime::Preedit("ね".into(), None, Vec::new()));
    rt.ime(Ime::Commit("猫".into()));
    rt.frame(0.0);
    assert_eq!(rt.state.log, [
        r#"Preedit("か", None, [])"#,
        r#"Preedit("", None, [])"#,
        "Disabled",
    ]);
    assert_eq!(rt.state.field.text, "猫");
}

#[test]
fn ime_disabled_after_commit_keeps_confirm_release_suppressed() {
    let mut rt = editor();
    rt.ime(Ime::Preedit("か".into(), None, Vec::new()));
    rt.frame(0.0);
    // Return confirms the composition; Windows sends Preedit(""), Commit, then Disabled.
    press(&mut rt, Key::Enter);
    rt.ime(Ime::Preedit(String::new(), None, Vec::new()));
    rt.ime(Ime::Commit("家".into()));
    rt.ime(Ime::Disabled);
    rt.frame(0.0);
    release(&mut rt, Key::Enter);
    press(&mut rt, Key::Enter);
    release(&mut rt, Key::Enter);
    assert_eq!(rt.state.log, [
        r#"Preedit("か", None, [])"#,
        r#"Preedit("", None, [])"#,
        r#"Commit("家")"#,
        "Disabled",
        "key Enter",
        "up Enter",
    ]);
}

#[test]
fn empty_preedit_without_composition_does_not_snap_scroll_back_to_caret() {
    struct Page {
        field: TextEdit,
    }
    fn page_view(s: &Page, _: &mut Cx<Page>) -> El {
        col().key("viewport").size(300.0, 120.0).scroll(crate::ScrollSpec { stick_bottom: false }).child(
            text_input(&s.field, TextStyle::new(14.0, 400.0).lh(20.0))
                .key("field")
                .label("Field")
                .w_full()
                .on_edit(|s: &mut Page| &mut s.field),
        )
    }
    let scroll_y = |rt: &Runtime<Page>| {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some("field")).unwrap().scrolled[1]
    };

    let mut field = TextEdit::new((1..=80).map(|i| format!("Line {i}\n")).collect::<String>());
    field.focused = true;
    field.set_caret(0);
    let mut rt = Runtime::new(Page { field }, page_view, (300.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(scroll_y(&rt), 0.0);

    // Wheel-scroll far past the caret on line 1.
    rt.wheel(150.0, 60.0, -400.0);
    rt.frame(1.0 / 60.0);
    let scrolled = scroll_y(&rt);
    assert!(scrolled > 200.0, "wheel scrolled past the caret: {scrolled}");

    // Wayland's text-input protocol replies to set_ime_cursor_area with an empty preedit.
    // While nothing is composing, it is a no-op and must leave the scroll offset alone.
    let commits_before = rt.core().commit.commits;
    rt.ime(Ime::Preedit(String::new(), None, Vec::new()));
    rt.frame(1.0 / 60.0);
    assert_eq!(scroll_y(&rt), scrolled, "empty preedit outside composition must not trigger caret_follow");
    assert_eq!(rt.core().commit.commits, commits_before, "empty preedit outside composition must not commit");

    // A real composition still brings the caret back into view, and an empty preedit still cancels it.
    rt.ime(Ime::Preedit("か".into(), Some((0, 3)), Vec::new()));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.field.preedit(), Some(("か", Some((0, 3)))));
    assert!(scroll_y(&rt) < 20.0, "active composition follows the caret back into view");

    rt.ime(Ime::Preedit(String::new(), None, Vec::new()));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.field.preedit(), None, "empty preedit cancels an open composition");
}

#[test]
fn empty_commit_without_composition_keeps_scroll_and_double_click_while_ending_open_composition() {
    struct Page {
        field: TextEdit,
    }
    fn page_view(s: &Page, _: &mut Cx<Page>) -> El {
        col().key("viewport").size(300.0, 120.0).scroll(crate::ScrollSpec { stick_bottom: false }).child(
            text_input(&s.field, TextStyle::new(14.0, 400.0).lh(20.0))
                .key("field")
                .label("Field")
                .w_full()
                .on_edit(|s: &mut Page| &mut s.field),
        )
    }
    let scroll_y = |rt: &Runtime<Page>| {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some("field")).unwrap().scrolled[1]
    };

    let text: String = (1..=80).map(|i| format!("alpha line {i}\n")).collect();
    let mut field = TextEdit::new(text.clone());
    field.focused = true;
    field.set_caret(0);
    let mut rt = Runtime::new(Page { field }, page_view, (300.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(scroll_y(&rt), 0.0);

    // 1. An empty commit arriving between the two clicks of a double-click must not clear
    // the click count.
    let field_rect = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("field")).unwrap().rect;
    let pt = (field_rect[0] + 12.0, field_rect[1] + 10.0);
    rt.pointer_down(pt.0, pt.1);
    rt.pointer_up(pt.0, pt.1);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.field.anchor, rt.state.field.focus, "first click places a caret");

    rt.ime(Ime::Commit(String::new()));
    rt.frame(1.0 / 60.0);

    rt.pointer_down(pt.0, pt.1);
    rt.pointer_up(pt.0, pt.1);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.field.selected(), "alpha", "empty commit outside composition must not break double-click");

    // 2. Wheel-scroll far past the caret on line 1; an empty commit outside composition must
    // neither trigger caret_follow nor commit a frame.
    rt.wheel(150.0, 60.0, -400.0);
    rt.frame(1.0 / 60.0);
    let scrolled = scroll_y(&rt);
    assert!(scrolled > 200.0, "wheel scrolled past the caret: {scrolled}");

    let commits_before = rt.core().commit.commits;
    rt.ime(Ime::Commit(String::new()));
    rt.frame(1.0 / 60.0);
    assert_eq!(scroll_y(&rt), scrolled, "empty commit outside composition must not trigger caret_follow");
    assert_eq!(rt.core().commit.commits, commits_before, "empty commit outside composition must not commit");

    // 3. An empty commit while a composition is open must still end that composition without
    // inserting text.
    rt.ime(Ime::Preedit("か".into(), Some((0, 3)), Vec::new()));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.field.preedit(), Some(("か", Some((0, 3)))));

    rt.ime(Ime::Commit(String::new()));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.field.preedit(), None, "empty commit ends an open composition");
    assert_eq!(rt.state.field.text, text, "empty commit inserts no text");
}
