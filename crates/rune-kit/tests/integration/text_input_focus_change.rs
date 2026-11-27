//! `on_focus_change` on `TextInput`, `TextArea`, and `TextField`: in-window focus gain and
//! loss (keyboard, pointer, programmatic, modal, disable/unmount while focused), staying silent
//! on window deactivation and return.

use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, TextEdit, col, el, text};
use rune_kit::{Kit, TextArea, TextField, TextInput};

#[derive(Default)]
struct State {
    input: TextEdit,
    area: TextEdit,
    field: TextEdit,
    input_disabled: bool,
    area_disabled: bool,
    field_disabled: bool,
    input_shown: bool,
    area_shown: bool,
    field_shown: bool,
    events: Vec<(&'static str, bool, bool)>,
    blurs: Vec<&'static str>,
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let k = Kit::neutral(true);
    let last = s
        .events
        .last()
        .map(|(who, f, kv)| format!("{who}:{f}:{kv}"))
        .unwrap_or_default();
    let mut root = col().w_full().h_full().padding(12.0).gap(10.0);
    if s.input_shown {
        root = root.child(
            TextInput::new(&s.input)
                .label("Input")
                .disabled(s.input_disabled)
                .on_edit(|s: &mut State| &mut s.input)
                .on_blur(|s: &mut State, _| s.blurs.push("input"))
                .on_focus_change(|s: &mut State, focused, tx| {
                    s.events.push(("input", focused, tx.focus_visible()));
                })
                .build(&k)
                .key("input"),
        );
    }
    if s.area_shown {
        root = root.child(
            TextArea::new(&s.area)
                .label("Notes")
                .disabled(s.area_disabled)
                .on_edit(|s: &mut State| &mut s.area)
                .on_blur(|s: &mut State, _| s.blurs.push("area"))
                .on_focus_change(|s: &mut State, focused, tx| {
                    s.events.push(("area", focused, tx.focus_visible()));
                })
                .build(&k)
                .key("area"),
        );
    }
    if s.field_shown {
        root = root.child(
            TextField::new("Name", &s.field)
                .disabled(s.field_disabled)
                .on_edit(|s: &mut State| &mut s.field)
                .on_blur(|s: &mut State, _| s.blurs.push("field"))
                .on_focus_change(|s: &mut State, focused, tx| {
                    s.events.push(("field", focused, tx.focus_visible()));
                })
                .build(&k)
                .key("field"),
        );
    }
    root.child(el().key("other").size(80.0, 24.0).focusable(true))
        .child(text(format!("last: {last}")).key("last"))
}

fn runtime() -> Runtime<State> {
    let mut rt = Runtime::new(
        State { input_shown: true, area_shown: true, field_shown: true, ..State::default() },
        view,
        (400.0, 420.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    rt
}

fn tab(rt: &mut Runtime<State>) {
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(0.0);
}

fn click_key<S>(rt: &mut Runtime<S>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing element {key}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
}

fn shows<S>(rt: &Runtime<S>, s: &str) -> bool {
    rt.elements().iter().any(|e| e.text.as_deref() == Some(s))
}

#[test]
fn in_window_focus_gain_and_loss_across_all_three_fields() {
    let mut rt = runtime();
    assert!(rt.state.events.is_empty(), "initial unfocused mount must fire nothing");

    // Tab through TextInput -> TextArea -> TextField -> other: keyboard focus_visible is true.
    tab(&mut rt);
    assert_eq!(rt.state.events, [("input", true, true)]);
    assert!(shows(&rt, "last: input:true:true"), "view rebuilds with what on_focus_change changed");

    tab(&mut rt);
    assert_eq!(rt.state.events[1..], [("input", false, true), ("area", true, true)]);

    tab(&mut rt);
    assert_eq!(rt.state.events[3..], [("area", false, true), ("field", true, true)]);

    tab(&mut rt);
    assert_eq!(rt.state.events[5..], [("field", false, true)]);
    assert!(shows(&rt, "last: field:false:true"));

    // Pointer click into TextInput -> TextArea -> other: pointer focus_visible is false.
    rt.state.events.clear();
    click_key(&mut rt, "input");
    assert_eq!(rt.state.events, [("input", true, false)]);

    click_key(&mut rt, "area");
    assert_eq!(rt.state.events[1..], [("input", false, false), ("area", true, false)]);

    click_key(&mut rt, "field");
    assert_eq!(rt.state.events[3..], [("area", false, false), ("field", true, false)]);

    click_key(&mut rt, "other");
    assert_eq!(rt.state.events[5..], [("field", false, false)]);

    // Programmatic focus and tx.blur().
    rt.state.events.clear();
    rt.update(|s, _| s.input.focused = true);
    rt.frame(0.0);
    assert_eq!(rt.state.events, [("input", true, false)]);

    rt.update(|_, tx| tx.blur());
    rt.frame(0.0);
    assert_eq!(rt.state.events[1..], [("input", false, false)]);
}

#[test]
fn window_deactivation_and_return_stay_silent_while_on_blur_still_fires() {
    for (name, pick) in [
        ("input", (|s: &mut State| &mut s.input) as fn(&mut State) -> &mut TextEdit),
        ("area", |s: &mut State| &mut s.area),
        ("field", |s: &mut State| &mut s.field),
    ] {
        let mut rt = runtime();
        rt.update(|s, _| pick(s).focused = true);
        rt.frame(0.0);
        assert_eq!(rt.state.events, [(name, true, false)], "{name} gained focus");
        rt.state.events.clear();
        rt.state.blurs.clear();

        // Window deactivation fires on_blur, NOT on_focus_change.
        rt.window_focus(false);
        rt.frame(0.0);
        assert!(rt.state.events.is_empty(), "{name}: window deactivation must not fire on_focus_change");
        assert_eq!(rt.state.blurs, [name], "{name}: on_blur still fires on window deactivation");

        // Window return fires neither.
        rt.window_focus(true);
        rt.frame(0.0);
        assert!(rt.state.events.is_empty(), "{name}: window return must not fire on_focus_change");

        // Subsequent in-window move still delivers false.
        click_key(&mut rt, "other");
        assert_eq!(rt.state.events, [(name, false, false)], "{name}: in-window move after return fires false");
    }
}

#[test]
fn disabling_or_removing_a_focused_field_delivers_false_and_initially_disabled_never_fires() {
    // Disabling while focused delivers on_focus_change(false) on all three fields, without on_blur.
    for (name, focus_field, disable_field) in [
        (
            "input",
            (|s: &mut State| s.input.focused = true) as fn(&mut State),
            (|s: &mut State| s.input_disabled = true) as fn(&mut State),
        ),
        ("area", |s| s.area.focused = true, |s| s.area_disabled = true),
        ("field", |s| s.field.focused = true, |s| s.field_disabled = true),
    ] {
        let mut rt = runtime();
        rt.update(|s, _| focus_field(s));
        rt.frame(0.0);
        rt.state.events.clear();
        rt.state.blurs.clear();

        rt.update(|s, _| disable_field(s));
        rt.frame(0.0);
        assert_eq!(
            rt.state.events.first().copied(),
            Some((name, false, false)),
            "{name}: disabling while focused delivers false first (before focus rescue): {:?}",
            rt.state.events,
        );
        assert!(rt.state.blurs.is_empty(), "{name}: disabling while focused does not fire on_blur");
        let count = rt.state.events.len();
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.events.len(), count, "{name}: next frame must not double-fire");
    }

    // Removing while focused delivers on_focus_change(false) on all three fields.
    for (name, focus_field, hide_field) in [
        (
            "input",
            (|s: &mut State| s.input.focused = true) as fn(&mut State),
            (|s: &mut State| s.input_shown = false) as fn(&mut State),
        ),
        ("area", |s| s.area.focused = true, |s| s.area_shown = false),
        ("field", |s| s.field.focused = true, |s| s.field_shown = false),
    ] {
        let mut rt = runtime();
        rt.update(|s, _| focus_field(s));
        rt.frame(0.0);
        rt.state.events.clear();

        rt.update(|s, _| hide_field(s));
        rt.frame(0.0);
        assert_eq!(
            rt.state.events.first().copied(),
            Some((name, false, false)),
            "{name}: removing while focused delivers false first (before focus rescue): {:?}",
            rt.state.events,
        );
        let count = rt.state.events.len();
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.events.len(), count, "{name}: next frame must not double-fire");
    }

    // Initially disabled fields never gain focus or fire on_focus_change.
    let mut rt = Runtime::new(
        State {
            input_shown: true,
            area_shown: true,
            field_shown: true,
            input_disabled: true,
            area_disabled: true,
            field_disabled: true,
            ..State::default()
        },
        view,
        (400.0, 420.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    click_key(&mut rt, "input");
    click_key(&mut rt, "area");
    click_key(&mut rt, "field");
    rt.update(|s, _| {
        s.input.focused = true;
        s.area.focused = true;
        s.field.focused = true;
    });
    rt.frame(0.0);
    assert!(rt.state.events.is_empty(), "initially disabled fields must never fire: {:?}", rt.state.events);
}

#[test]
fn read_only_fields_modal_steal_and_trailing_action_button_transitions() {
    #[derive(Default)]
    struct S {
        ro_input: TextEdit,
        ro_area: TextEdit,
        with_clear: TextEdit,
        modal_open: bool,
        live_only_hits: usize,
        log: Vec<(&'static str, bool)>,
    }
    fn v(s: &S, _: &mut Cx<S>) -> El {
        let k = Kit::neutral(true);
        let mut root = col()
            .w_full()
            .h_full()
            .padding(12.0)
            .gap(8.0)
            .child(
                TextInput::new(&s.ro_input)
                    .label("Endpoint")
                    .read_only(true)
                    .on_edit(|s: &mut S| &mut s.ro_input)
                    .on_focus_change(|s: &mut S, focused, _| s.log.push(("ro-input", focused)))
                    .build(&k)
                    .key("ro-input"),
            )
            .child(
                TextArea::new(&s.ro_area)
                    .label("Log")
                    .read_only(true)
                    .on_edit(|s: &mut S| &mut s.ro_area)
                    .on_focus_change(|s: &mut S, focused, tx| {
                        s.log.push(("ro-area", focused));
                        s.live_only_hits += 1;
                        tx.live_only();
                    })
                    .build(&k)
                    .key("ro-area"),
            )
            .child(
                TextInput::new(&s.with_clear)
                    .label("Search")
                    .trailing(el().key("trail-btn").size(24.0, 24.0).focusable(true))
                    .on_edit(|s: &mut S| &mut s.with_clear)
                    .on_focus_change(|s: &mut S, focused, _| s.log.push(("clear-field", focused)))
                    .build(&k)
                    .on_focus_within_change(|s: &mut S, within, _| s.log.push(("clear-within", within)))
                    .key("clear-field"),
            )
            .child(text(format!("events: {}", s.log.len())).key("count"));
        if s.modal_open {
            root = root.child(
                col()
                    .key("dlg")
                    .role("dialog")
                    .modal(true, |s: &mut S| s.modal_open = false)
                    .child(el().key("dlg-ok").size(60.0, 24.0).focusable(true)),
            );
        }
        root
    }
    let mut rt = Runtime::new(
        S { ro_input: TextEdit::new("https://example.com"), ..S::default() },
        v,
        (400.0, 360.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    // Read-only TextInput fires gain, stays silent on window blur/return, fires false when modal steals focus,
    // and fires true again when the modal closes and returns focus.
    rt.update(|s, _| s.ro_input.focused = true);
    rt.frame(0.0);
    assert_eq!(rt.state.log, [("ro-input", true)]);

    rt.window_focus(false);
    rt.frame(0.0);
    rt.window_focus(true);
    rt.frame(0.0);
    assert_eq!(rt.state.log, [("ro-input", true)], "read-only field ignores window deactivation/return");

    rt.update(|s, _| s.modal_open = true);
    rt.frame(0.0);
    assert_eq!(rt.state.log, [("ro-input", true), ("ro-input", false)]);
    assert!(shows(&rt, "events: 2"), "modal focus-steal rebuilds view with on_focus_change state");

    rt.update(|s, _| s.modal_open = false);
    rt.frame(0.0);
    assert_eq!(rt.state.log, [("ro-input", true), ("ro-input", false), ("ro-input", true)]);

    // Read-only TextArea with tx.live_only() receives gain and loss cleanly.
    rt.state.log.clear();
    rt.update(|s, _| s.ro_area.focused = true);
    rt.frame(0.0);
    assert_eq!(rt.state.log, [("ro-input", false), ("ro-area", true)]);
    assert_eq!(rt.state.live_only_hits, 1);

    // Moving from inner editor to the field's own trailing focusable control fires on_focus_change(false)
    // on the editor even though focus stays within the outer TextInput box (on_focus_within_change stays silent).
    rt.state.log.clear();
    rt.update(|s, _| s.with_clear.focused = true);
    rt.frame(0.0);
    assert_eq!(rt.state.log, [("ro-area", false), ("clear-field", true), ("clear-within", true)]);

    rt.state.log.clear();
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(
        rt.state.log,
        [("clear-field", false)],
        "Tabbing from inner editor to its own trailing control fires on_focus_change(false) while within stays true",
    );
    let mut focused_after = None;
    rt.update(|_, tx| focused_after = tx.focused().map(str::to_owned));
    assert_eq!(
        focused_after.as_deref(),
        Some("trail-btn"),
        "focus landed on clear-field's own trailing button",
    );
}
