//! `TextArea::reveal_caret_on_focus(bool)` and `TextInput::reveal_caret_on_focus(bool)`
//! integration tests (default off, Tab and `Tx::focus`, `auto_grow`, disabled state, wheel
//! preservation, and accessibility tree verification).

use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, ScrollSpec, TextEdit, col, el};
use rune_kit::{Button, Kit, TextArea, TextInput};

fn numbered_lines(count: usize) -> String {
    (1..=count).map(|i| format!("Line {i:02}: release note entry")).collect::<Vec<_>>().join("\n")
}

struct App {
    opt_in: TextEdit,
    default_off: TextEdit,
    grow: TextEdit,
    disabled: TextEdit,
    single: TextEdit,
}

fn view(s: &App, _: &mut Cx<App>) -> El {
    let k = Kit::neutral(true);
    col()
        .size(340.0, 520.0)
        .padding(8.0)
        .gap(8.0)
        .child(Button::new("Before").build(&k).key("before"))
        .child(
            TextArea::new(&s.opt_in)
                .label("Opt-in notes")
                .width(None)
                .max_height(100.0)
                .reveal_caret_on_focus(true)
                .on_edit(|s: &mut App| &mut s.opt_in)
                .build(&k)
                .key("opt-in"),
        )
        .child(
            TextArea::new(&s.default_off)
                .label("Default notes")
                .width(None)
                .max_height(100.0)
                .on_edit(|s: &mut App| &mut s.default_off)
                .build(&k)
                .key("default-off"),
        )
        .child(
            TextArea::new(&s.grow)
                .label("Auto-grow notes")
                .width(None)
                .auto_grow(2, 4)
                .reveal_caret_on_focus(true)
                .on_edit(|s: &mut App| &mut s.grow)
                .build(&k)
                .key("grow"),
        )
        .child(
            TextArea::new(&s.disabled)
                .label("Disabled notes")
                .width(None)
                .max_height(80.0)
                .reveal_caret_on_focus(true)
                .disabled(true)
                .on_edit(|s: &mut App| &mut s.disabled)
                .build(&k)
                .key("disabled"),
        )
        .child(
            col()
                .key("form-scroll")
                .w_full()
                .h(80.0)
                .scroll(ScrollSpec::default())
                .child(el().w_full().h(180.0))
                .child(
                    TextInput::new(&s.single)
                        .label("Footer field")
                        .width(None)
                        .reveal_caret_on_focus(true)
                        .on_edit(|s: &mut App| &mut s.single)
                        .build(&k)
                        .key("single"),
                ),
        )
}

fn find_el(rt: &Runtime<App>, path_suffix: &str) -> rune::ElementInfo {
    rt.elements()
        .into_iter()
        .find(|e| e.path.ends_with(path_suffix))
        .unwrap_or_else(|| panic!("missing element ending with {path_suffix}"))
}

#[test]
fn text_area_and_text_input_reveal_caret_on_focus() {
    let mut rt = Runtime::new(
        App {
            opt_in: TextEdit::new(numbered_lines(30)),
            default_off: TextEdit::new(numbered_lines(30)),
            grow: TextEdit::new(numbered_lines(20)),
            disabled: TextEdit::new(numbered_lines(20)),
            single: TextEdit::new("hello"),
        },
        view,
        (340.0, 520.0),
        Env::new(true, 1.0),
    );
    rt.set_tab_visits_controls(true);
    rt.frame(0.0);

    // Tab onto Before button, then Tab into opt-in TextArea: caret at line 30 scrolls into view.
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert!(rt.state.opt_in.focused);
    let vp = find_el(&rt, "opt-in/scroll").rect;
    let input = find_el(&rt, "opt-in/scroll/input");
    let caret = input.caret.expect("opt-in caret");
    assert!(
        caret[1] >= vp[1] - 1.0 && caret[1] + caret[3] <= vp[1] + vp[3] + 1.0,
        "opt-in TextArea caret {caret:?} outside scroll viewport {vp:?}"
    );

    // Accessibility tree shows focused MultilineTextInput with label.
    let tree = rt.accessibility_tree();
    let (ak_id, ak_node) = tree
        .nodes
        .iter()
        .find(|(_, n)| n.label() == Some("Opt-in notes"))
        .expect("a11y opt-in");
    assert_eq!(ak_node.role(), rune::accesskit::Role::MultilineTextInput);
    assert_eq!(tree.focus, *ak_id);

    // Wheel-scrolling away from the caret while focused stays put across idle frames.
    rt.wheel(vp[0] + 20.0, vp[1] + 20.0, 1200.0);
    rt.frame(1.0 / 60.0);
    let scrolled_top = find_el(&rt, "opt-in/scroll/input").rect[1];
    rt.frame(1.0 / 60.0);
    assert_eq!(
        find_el(&rt, "opt-in/scroll/input").rect[1],
        scrolled_top,
        "wheel scroll stays put while focused"
    );

    // Tab to default-off TextArea: caret remains below viewport (unchanged default behavior).
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert!(rt.state.default_off.focused);
    let def_vp = find_el(&rt, "default-off/scroll").rect;
    let def_caret = find_el(&rt, "default-off/scroll/input").caret.expect("default caret");
    assert!(
        def_caret[1] > def_vp[1] + def_vp[3],
        "default-off TextArea caret {def_caret:?} should stay below {def_vp:?}"
    );

    // Tab to auto_grow TextArea with reveal_caret_on_focus(true): caret scrolls into view.
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert!(rt.state.grow.focused);
    let grow_vp = find_el(&rt, "grow/scroll").rect;
    let grow_caret = find_el(&rt, "grow/scroll/input").caret.expect("grow caret");
    assert!(
        grow_caret[1] >= grow_vp[1] - 1.0 && grow_caret[1] + grow_caret[3] <= grow_vp[1] + grow_vp[3] + 1.0,
        "auto_grow TextArea caret {grow_caret:?} outside viewport {grow_vp:?}"
    );

    // Disabled TextArea is skipped by Tab and refuses Tx::focus without scrolling.
    let disabled_before = find_el(&rt, "disabled/scroll/input").rect[1];
    rt.update(|_, tx| tx.focus("disabled", true));
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.disabled.focused);
    assert_eq!(find_el(&rt, "disabled/scroll/input").rect[1], disabled_before);

    // TextInput with reveal_caret_on_focus(true) inside a scrolled form reveals on Tx::focus.
    rt.update(|_, tx| tx.focus("single", false));
    rt.frame(1.0 / 60.0);
    assert!(rt.state.single.focused);
    let form_vp = find_el(&rt, "form-scroll").rect;
    let single_caret = rt
        .elements()
        .into_iter()
        .find(|e| e.focused && e.path.contains("/single/"))
        .expect("focused single input")
        .caret
        .expect("single caret");
    assert!(
        single_caret[1] >= form_vp[1] - 1.0 && single_caret[1] + single_caret[3] <= form_vp[1] + form_vp[3] + 1.0,
        "TextInput caret {single_caret:?} outside form viewport {form_vp:?}"
    );
}
