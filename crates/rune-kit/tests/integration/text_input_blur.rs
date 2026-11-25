//! `on_blur` on `TextInput`, `TextArea` and `TextField`: window focus loss and focus moving
//! off the field commit, Return does not, and a disabled field never hears it.

use rune::{Cx, El, Env, Key, KeyEvent, Modifier, Mods, PhysicalInput, PhysicalKeyCode, Runtime, TextEdit, col, el, text};
use rune_kit::{Kit, TextArea, TextField, TextInput};

#[derive(Default)]
struct State {
    input: TextEdit,
    area: TextEdit,
    field: TextEdit,
    disabled: bool,
    shown: bool,
    keys: Vec<KeyEvent>,
    blurred: Vec<(&'static str, String)>,
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let k = Kit::neutral(true);
    let saved = s.blurred.last().map(|(_, v)| v.clone()).unwrap_or_default();
    let mut root = col().w_full().h_full().padding(12.0).gap(10.0);
    if s.shown {
        root = root.child(
            TextInput::new(&s.input)
                .label("Input")
                .disabled(s.disabled)
                .on_edit(|s: &mut State| &mut s.input)
                .on_submit(|s: &mut State, _| s.blurred.push(("submit", s.input.text.clone())))
                .on_blur(|s: &mut State, _| s.blurred.push(("input", s.input.text.clone())))
                .build(&k)
                .key("input"),
        );
    }
    root.child(
        TextArea::new(&s.area)
            .label("Notes")
            .on_edit(|s: &mut State| &mut s.area)
            .on_blur(|s: &mut State, _| s.blurred.push(("area", s.area.text.clone())))
            .build(&k)
            .key("area"),
    )
    .child(
        TextField::new("Name", &s.field)
            .on_edit(|s: &mut State| &mut s.field)
            .on_blur(|s: &mut State, _| s.blurred.push(("field", s.field.text.clone())))
            .build(&k)
            .key("field"),
    )
    .child(el().key("other").size(80.0, 24.0).focusable(true))
    .child(text(format!("saved: {saved}")).key("saved"))
}

fn runtime() -> Runtime<State> {
    let mut rt = Runtime::new(State { shown: true, ..State::default() }, view, (400.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn focus(rt: &mut Runtime<State>, pick: impl FnOnce(&mut State) -> &mut TextEdit) {
    rt.update(|s, _| pick(s).focused = true);
    rt.frame(0.0);
}

fn type_text(rt: &mut Runtime<State>, t: &str) {
    rt.key(KeyEvent { key: Key::Text(t.into()), mods: Mods::default() });
    rt.frame(0.0);
}

fn shows(rt: &Runtime<State>, s: &str) -> bool {
    rt.elements().iter().any(|e| e.text.as_deref() == Some(s))
}

#[test]
fn window_focus_loss_commits_the_focused_field_and_rebuilds_the_view() {
    let mut rt = runtime();
    focus(&mut rt, |s| &mut s.input);
    type_text(&mut rt, "half");
    rt.window_focus(false);
    rt.frame(0.0);
    assert_eq!(rt.state.blurred, [("input", "half".to_owned())]);
    assert!(shows(&rt, "saved: half"), "the view rebuilds with what on_blur changed");

    // Coming back and losing focus again commits again, once.
    rt.window_focus(true);
    rt.frame(0.0);
    type_text(&mut rt, "!");
    rt.window_focus(false);
    rt.frame(0.0);
    assert_eq!(rt.state.blurred.len(), 2);
    assert_eq!(rt.state.blurred[1], ("input", "half!".to_owned()));
}

#[test]
fn return_submits_without_blur_and_moving_focus_blurs() {
    let mut rt = runtime();
    focus(&mut rt, |s| &mut s.input);
    type_text(&mut rt, "a");
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.blurred, [("submit", "a".to_owned())]);

    rt.update(|_, tx| tx.focus("other", false));
    rt.frame(0.0);
    assert_eq!(rt.state.blurred[1..], [("input", "a".to_owned())]);

    focus(&mut rt, |s| &mut s.area);
    type_text(&mut rt, "b");
    focus(&mut rt, |s| &mut s.field);
    assert_eq!(rt.state.blurred[2..], [("area", "b".to_owned())]);
    type_text(&mut rt, "c");
    rt.window_focus(false);
    rt.frame(0.0);
    assert_eq!(rt.state.blurred[3..], [("field", "c".to_owned())]);
}

#[test]
fn disabled_or_removed_fields_do_not_blur() {
    let mut rt = runtime();
    focus(&mut rt, |s| &mut s.input);
    rt.update(|s, _| s.shown = false);
    rt.frame(0.0);
    rt.window_focus(false);
    rt.frame(0.0);
    assert!(!rt.state.blurred.iter().any(|(who, _)| *who == "input"), "{:?}", rt.state.blurred);

    let mut rt = runtime();
    rt.update(|s, _| s.disabled = true);
    rt.frame(0.0);
    focus(&mut rt, |s| &mut s.input);
    rt.window_focus(false);
    rt.frame(0.0);
    assert!(rt.state.blurred.is_empty(), "{:?}", rt.state.blurred);
}

#[test]
fn tab_and_a_click_elsewhere_blur_the_field() {
    let mut rt = runtime();
    focus(&mut rt, |s| &mut s.input);
    type_text(&mut rt, "tab");
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.blurred, [("input", "tab".to_owned())]);
    assert!(!rt.state.input.focused, "Tab moved focus off the input");

    focus(&mut rt, |s| &mut s.field);
    type_text(&mut rt, "click");
    let (x, y) = rt.find("other").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    assert_eq!(rt.state.blurred.last(), Some(&("field", "click".to_owned())));
    assert!(shows(&rt, "saved: click"));
}

#[test]
fn repeated_window_deactivation_and_blur_inside_on_blur_do_not_double_fire() {
    #[derive(Default)]
    struct S {
        edit: TextEdit,
        saved: Vec<String>,
        clear_on_blur: bool,
    }
    fn v(s: &S, _: &mut Cx<S>) -> El {
        let k = Kit::neutral(true);
        let clear = s.clear_on_blur;
        col().w_full().h_full().padding(12.0).child(
            TextInput::new(&s.edit)
                .label("Name")
                .on_edit(|s: &mut S| &mut s.edit)
                .on_blur(move |s: &mut S, tx| {
                    s.saved.push(s.edit.text.clone());
                    if clear {
                        tx.blur();
                    }
                })
                .build(&k)
                .key("name"),
        )
    }
    let mut rt = Runtime::new(S { clear_on_blur: true, ..S::default() }, v, (400.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.edit.focused = true);
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Text("abc".into()), mods: Mods::default() });
    rt.frame(0.0);

    // Window loses focus and on_blur calls tx.blur(): must fire once, not twice.
    rt.window_focus(false);
    rt.frame(0.0);
    // Repeated window_focus(false) (e.g. close/hide after blur) must not fire again.
    rt.window_focus(false);
    rt.frame(0.0);
    assert_eq!(rt.state.saved, ["abc"], "blur must fire once across window deactivation + tx.blur + close");

    // Window returns: no blur, and re-focusing then deactivating again blurs once.
    rt.window_focus(true);
    rt.frame(0.0);
    assert_eq!(rt.state.saved.len(), 1);
    rt.update(|s, _| s.edit.focused = true);
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Text("d".into()), mods: Mods::default() });
    rt.frame(0.0);
    rt.window_focus(false);
    rt.frame(0.0);
    assert_eq!(rt.state.saved, ["abc", "abcd"]);
}

#[test]
fn read_only_field_blurs_and_disabling_while_focused_does_not_blur() {
    #[derive(Default)]
    struct S {
        ro: TextEdit,
        dis: TextEdit,
        disable_now: bool,
        changes: usize,
        submits: usize,
        blurs: Vec<&'static str>,
    }
    fn v(s: &S, _: &mut Cx<S>) -> El {
        let k = Kit::neutral(true);
        col().w_full().h_full().padding(12.0).gap(8.0)
            .child(
                TextInput::new(&s.ro)
                    .label("Endpoint")
                    .read_only(true)
                    .on_edit(|s: &mut S| &mut s.ro)
                    .on_blur(|s: &mut S, _| s.blurs.push("ro"))
                    .build(&k)
                    .key("ro"),
            )
            .child(
                TextInput::new(&s.dis)
                    .label("Editable")
                    .disabled(s.disable_now)
                    .on_edit(|s: &mut S| &mut s.dis)
                    .on_change(|s: &mut S, _| s.changes += 1)
                    .on_submit(|s: &mut S, _| s.submits += 1)
                    .on_blur(|s: &mut S, _| s.blurs.push("dis"))
                    .build(&k)
                    .key("dis"),
            )
            .child(el().key("other").size(80.0, 24.0).focusable(true))
    }
    let mut rt = Runtime::new(S { ro: TextEdit::new("https://example.com"), ..S::default() }, v, (400.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Read-only field focuses and fires on_blur on Tab, click elsewhere, and window deactivation.
    rt.update(|s, _| s.ro.focused = true);
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.blurs, ["ro"], "Tab blurs a read-only field");

    // Tab landed on "dis"; focusing "ro" again blurs "dis", then clicking "other" blurs "ro".
    rt.state.blurs.clear();
    rt.update(|s, _| s.ro.focused = true);
    rt.frame(0.0);
    assert_eq!(rt.state.blurs, ["dis"]);
    let (x, y) = rt.find("other").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    assert_eq!(rt.state.blurs, ["dis", "ro"], "click elsewhere blurs a read-only field");

    rt.update(|s, _| s.ro.focused = true);
    rt.frame(0.0);
    rt.window_focus(false);
    rt.frame(0.0);
    assert_eq!(rt.state.blurs, ["dis", "ro", "ro"], "window focus loss blurs a read-only field");
    rt.window_focus(true);
    rt.frame(0.0);

    // on_change and on_submit work unchanged alongside on_blur; disabling while focused does not blur.
    rt.update(|s, _| s.dis.focused = true);
    rt.frame(0.0);
    rt.state.blurs.clear();
    rt.key(KeyEvent { key: Key::Text("ok".into()), mods: Mods::default() });
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!((rt.state.changes, rt.state.submits), (1, 1));
    assert!(rt.state.blurs.is_empty(), "typing and Return must not trigger on_blur: {:?}", rt.state.blurs);

    rt.update(|s, _| s.disable_now = true);
    rt.frame(0.0);
    assert!(rt.state.blurs.is_empty(), "disabling a focused field must not fire on_blur: {:?}", rt.state.blurs);
}

#[test]
fn opening_a_modal_blurs_the_focused_field_and_rebuilds_the_view() {
    #[derive(Default)]
    struct S {
        edit: TextEdit,
        modal_open: bool,
        saved: String,
    }
    fn v(s: &S, _: &mut Cx<S>) -> El {
        let k = Kit::neutral(true);
        let mut root = col().w_full().h_full().padding(12.0).gap(8.0)
            .child(
                TextInput::new(&s.edit)
                    .label("Field")
                    .on_edit(|s: &mut S| &mut s.edit)
                    .on_blur(|s: &mut S, _| s.saved = s.edit.text.clone())
                    .build(&k)
                    .key("field"),
            )
            .child(text(format!("saved: {}", s.saved)).key("saved"));
        if s.modal_open {
            root = root.child(
                col().key("dlg").role("dialog").modal(true, |s: &mut S| s.modal_open = false)
                    .child(el().key("dlg-ok").size(60.0, 24.0).focusable(true)),
            );
        }
        root
    }
    let mut rt = Runtime::new(S::default(), v, (400.0, 240.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.edit.focused = true);
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Text("draft".into()), mods: Mods::default() });
    rt.frame(0.0);

    rt.update(|s, _| s.modal_open = true);
    rt.frame(0.0);
    assert_eq!(rt.state.saved, "draft");
    assert!(
        rt.elements().iter().any(|e| e.text.as_deref() == Some("saved: draft")),
        "modal focus-steal must rebuild the view with what on_blur changed in the same frame",
    );
}

#[test]
fn a_held_modifier_still_releases_when_the_window_loses_focus() {
    let mut rt = runtime();
    rt.set_modifier_keys(true);
    rt.on_key = Some(|s, ev, _| {
        if matches!(ev.key, Key::Modifier { .. }) { s.keys.push(ev.clone()); }
        false
    });
    focus(&mut rt, |s| &mut s.input);
    type_text(&mut rt, "x");
    let press = PhysicalInput::Key {
        key: PhysicalKeyCode::SuperLeft,
        mods: Mods::default(),
        pressed: true,
        repeat: false,
        text: None,
        consumed_mods: Mods::default(),
    };
    rt.physical_input(press);
    rt.frame(0.0);
    rt.window_focus(false);
    rt.frame(0.0);
    assert_eq!(rt.state.blurred, [("input", "x".to_owned())], "on_blur ran once");
    let modifiers: Vec<_> = rt.state.keys.iter().map(|ev| ev.key.clone()).collect();
    assert_eq!(
        modifiers,
        [
            Key::Modifier { modifier: Modifier::SuperLeft, pressed: true },
            Key::Modifier { modifier: Modifier::SuperLeft, pressed: false },
        ],
    );
}
