use super::*;
use crate::{Direction, Mods, col, el, row, text};

fn focused_key<S: 'static>(rt: &Runtime<S>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key)
}

fn send<S: 'static>(rt: &mut Runtime<S>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    rt.frame(0.0);
}

fn press<S: 'static>(rt: &mut Runtime<S>, key: Key) {
    send(rt, key, Mods::default());
}

// --- #1364: entering a focus_group from its focused container ---

#[derive(Default)]
struct MenuState {
    items: Vec<(&'static str, &'static str, bool)>,
    inert_items: bool,
    web_focus: bool,
    pressed: Vec<&'static str>,
}

fn menu_view(s: &MenuState, cx: &mut Cx<MenuState>) -> El {
    if s.web_focus { cx.web_focus(); }
    let mut menu = col()
        .key("menu")
        .role("menu")
        .label("Actions")
        .focus_group()
        .focusable(true)
        .tab_index(-1)
        .size(240.0, 220.0)
        .padding(20.0)
        .on_press(|s: &mut MenuState| s.pressed.push("menu"));
    for &(key, label, enabled) in &s.items {
        let mut item = text(label)
            .key(key)
            .role("menuitem")
            .label(label)
            .focusable(enabled)
            .disabled(!enabled)
            .w_full()
            .h(36.0)
            .on_press(move |s: &mut MenuState| s.pressed.push(key));
        if s.inert_items { item = item.inert(); }
        menu = menu.child(item);
    }
    menu
}

fn mount_menu(items: Vec<(&'static str, &'static str, bool)>, origin: &str) -> Runtime<MenuState> {
    let mut rt = Runtime::new(
        MenuState { items, web_focus: true, ..MenuState::default() },
        menu_view,
        (320.0, 260.0),
        Env::new(false, 1.0),
    );
    rt.frame(0.0);
    if origin == "pointer" {
        rt.pointer_down(10.0, 10.0);
        rt.pointer_up(10.0, 10.0);
        rt.frame(0.0);
    } else {
        rt.update(|_, tx| tx.focus("menu", true));
        rt.frame(0.0);
    }
    assert_eq!(focused_key(&rt).as_deref(), Some("menu"));
    rt
}

#[test]
fn focused_menu_container_enters_items_on_navigation_and_typeahead() {
    let items = vec![("alpha", "Alpha", true), ("bravo", "Bravo", true), ("charlie", "Charlie", true)];
    for origin in ["pointer", "tx"] {
        for (name, key, mods, expected) in [
            ("Home", Key::Home, Mods::default(), "alpha"),
            ("End", Key::End, Mods::default(), "charlie"),
            ("Down", Key::Down, Mods::default(), "alpha"),
            ("Up", Key::Up, Mods::default(), "charlie"),
            ("PageUp", Key::PageUp, Mods::default(), "alpha"),
            ("PageDown", Key::PageDown, Mods::default(), "charlie"),
            ("type-c", Key::Text("c".into()), Mods::default(), "charlie"),
            ("shift-B", Key::Text("B".into()), Mods { shift: true, ..Default::default() }, "bravo"),
        ] {
            let mut rt = mount_menu(items.clone(), origin);
            send(&mut rt, key, mods);
            assert_eq!(
                focused_key(&rt).as_deref(),
                Some(expected),
                "origin={origin} key={name} should enter {expected} in a single frame"
            );
            assert!(rt.kbd.focus.keyboard(), "entering from container shows keyboard modality");
        }
    }
}

#[test]
fn focused_menu_container_typeahead_starts_at_index_zero_then_cycles() {
    let items = vec![("alpha", "Alpha", true), ("alpine", "Alpine", true), ("bravo", "Bravo", true)];
    for origin in ["pointer", "tx"] {
        let mut rt = mount_menu(items.clone(), origin);
        press(&mut rt, Key::Text("a".into()));
        assert_eq!(
            focused_key(&rt).as_deref(),
            Some("alpha"),
            "origin={origin}: typing 'a' on [Alpha, Alpine, Bravo] enters Alpha directly without skipping to Alpine"
        );
        // Repeating 'a' from Alpha now cycles to Alpine.
        press(&mut rt, Key::Text("a".into()));
        assert_eq!(focused_key(&rt).as_deref(), Some("alpine"));
    }
}

#[test]
fn focused_menu_container_handles_empty_disabled_and_inert_groups_without_panic() {
    for (name, items, inert_items) in [
        ("empty", vec![], false),
        ("all-disabled", vec![("a", "Alpha", false), ("b", "Bravo", false)], false),
        ("all-inert", vec![("a", "Alpha", true), ("b", "Bravo", true)], true),
    ] {
        let mut rt = Runtime::new(
            MenuState { items, inert_items, web_focus: true, ..MenuState::default() },
            menu_view,
            (320.0, 260.0),
            Env::new(false, 1.0),
        );
        rt.frame(0.0);
        rt.update(|_, tx| tx.focus("menu", true));
        rt.frame(0.0);
        assert_eq!(focused_key(&rt).as_deref(), Some("menu"), "{name}: container starts focused");
        for key in [Key::Down, Key::Up, Key::Home, Key::End, Key::PageUp, Key::PageDown, Key::Text("a".into())] {
            press(&mut rt, key);
            assert_eq!(focused_key(&rt).as_deref(), Some("menu"), "{name}: focus stays on container");
        }
    }
}

#[test]
fn focused_horizontal_and_rtl_group_container_enters_on_flow_arrows() {
    fn view(rtl: &bool, _: &mut Cx<bool>) -> El {
        row()
            .key("tabs")
            .role("tablist")
            .dir(if *rtl { Direction::Rtl } else { Direction::Ltr })
            .focus_group()
            .focusable(true)
            .tab_index(-1)
            .size(200.0, 40.0)
            .child(el().key("first").role("tab").label("First").focusable(true).size(60.0, 30.0))
            .child(el().key("last").role("tab").label("Last").focusable(true).size(60.0, 30.0))
    }
    for (rtl, forward_key, backward_key) in [(false, Key::Right, Key::Left), (true, Key::Left, Key::Right)] {
        let mut rt = Runtime::new(rtl, view, (240.0, 60.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.update(|_, tx| tx.focus("tabs", true));
        rt.frame(0.0);
        press(&mut rt, forward_key);
        assert_eq!(focused_key(&rt).as_deref(), Some("first"), "rtl={rtl}: forward arrow enters first item");

        rt.update(|_, tx| tx.focus("tabs", true));
        rt.frame(0.0);
        press(&mut rt, backward_key);
        assert_eq!(focused_key(&rt).as_deref(), Some("last"), "rtl={rtl}: backward arrow enters last item");
    }
}

// --- #1397: El::press_on_return() ---

#[derive(Default)]
struct ReturnState {
    pressed: Vec<&'static str>,
    has_default: bool,
    unhandled: usize,
}

fn return_view(s: &ReturnState, _: &mut Cx<ReturnState>) -> El {
    let mut root = col()
        .key("root")
        .size(300.0, 200.0)
        .child(
            el().key("opt-in")
                .role("button")
                .label("Apply")
                .size(80.0, 30.0)
                .focusable(true)
                .press_on_return()
                .on_press(|s: &mut ReturnState| s.pressed.push("opt-in")),
        )
        .child(
            el().key("ordinary")
                .role("button")
                .label("Cancel")
                .size(80.0, 30.0)
                .focusable(true)
                .on_press(|s: &mut ReturnState| s.pressed.push("ordinary")),
        )
        .child(
            el().key("no-handler")
                .role("button")
                .label("Noop")
                .size(80.0, 30.0)
                .focusable(true)
                .press_on_return(),
        )
        .child(
            el().key("disabled")
                .role("button")
                .label("Disabled")
                .size(80.0, 30.0)
                .focusable(true)
                .disabled(true)
                .press_on_return()
                .on_press(|s: &mut ReturnState| s.pressed.push("disabled")),
        );
    if s.has_default {
        root = root.child(
            el().key("default")
                .role("button")
                .label("OK")
                .size(80.0, 30.0)
                .focusable(true)
                .default_button()
                .on_press(|s: &mut ReturnState| s.pressed.push("default")),
        );
    }
    root
}

#[test]
fn press_on_return_activates_focused_button_and_preempts_default_button() {
    let mut rt = Runtime::new(
        ReturnState { has_default: true, ..ReturnState::default() },
        return_view,
        (300.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    // Focused opt-in button fires on Return without Cx::web_focus and preempts default_button.
    rt.update(|_, tx| tx.focus("opt-in", true));
    rt.frame(0.0);
    press(&mut rt, Key::Enter);
    assert_eq!(rt.state.pressed, ["opt-in"]);

    // Key repeat is consumed without re-firing on_press or default_button.
    rt.repeat_key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.pressed, ["opt-in"]);

    // Modified Return does not activate the button.
    for mods in [
        Mods { shift: true, ..Default::default() },
        Mods { alt: true, ..Default::default() },
        Mods { ctrl: true, ..Default::default() },
        Mods { cmd: true, ..Default::default() },
    ] {
        send(&mut rt, Key::Enter, mods);
    }
    assert_eq!(rt.state.pressed, ["opt-in"], "modified Return must not activate press_on_return");

    // Ordinary button without press_on_return still routes Return to default_button.
    rt.update(|_, tx| tx.focus("ordinary", true));
    rt.frame(0.0);
    press(&mut rt, Key::Enter);
    assert_eq!(rt.state.pressed, ["opt-in", "default"]);

    // Space still activates the ordinary button.
    press(&mut rt, Key::Text(" ".into()));
    assert_eq!(rt.state.pressed, ["opt-in", "default", "ordinary"]);

    // Element with press_on_return but no on_press does not swallow Return.
    rt.update(|_, tx| tx.focus("no-handler", true));
    rt.frame(0.0);
    press(&mut rt, Key::Enter);
    assert_eq!(rt.state.pressed, ["opt-in", "default", "ordinary", "default"]);

    // Disabled element with press_on_return cannot take focus or fire.
    rt.update(|_, tx| tx.focus("disabled", true));
    rt.frame(0.0);
    assert_ne!(focused_key(&rt).as_deref(), Some("disabled"));
}

#[test]
fn press_on_return_preserves_button_role_and_leaves_ordinary_buttons_unchanged() {
    let mut rt = Runtime::new(ReturnState::default(), return_view, (300.0, 200.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, ev, _| { if ev.key == Key::Enter { s.unhandled += 1; } false });
    rt.frame(0.0);

    // Without default_button, Return on an ordinary button does not press it.
    rt.update(|_, tx| tx.focus("ordinary", true));
    rt.frame(0.0);
    press(&mut rt, Key::Enter);
    assert!(rt.state.pressed.is_empty());
    assert_eq!(rt.state.unhandled, 1);

    // press_on_return does not promote the accessibility role to default-button.
    let tree = rt.accessibility_json();
    fn find_by_key<'a>(n: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
        if n["key"].as_str() == Some(key) { return Some(n); }
        n["children"].as_array()?.iter().find_map(|c| find_by_key(c, key))
    }
    let opt = find_by_key(&tree, "opt-in").expect("opt-in button in a11y tree");
    assert_eq!(opt["role"].as_str(), Some("button"));
}

// --- #1409 core: WAI-ARIA Alt+Down opens a focused non-input combobox ---

#[derive(Default)]
struct ComboState {
    opens: usize,
    button_presses: usize,
    edit: crate::TextEdit,
}

fn combo_view(s: &ComboState, _: &mut Cx<ComboState>) -> El {
    col()
        .key("root")
        .size(300.0, 200.0)
        .child(
            el().key("combo")
                .role("combobox")
                .label("Theme")
                .size(120.0, 28.0)
                .focusable(true)
                .on_press(|s: &mut ComboState| s.opens += 1),
        )
        .child(
            el().key("btn")
                .role("button")
                .label("Plain")
                .size(120.0, 28.0)
                .focusable(true)
                .on_press(|s: &mut ComboState| s.button_presses += 1),
        )
        .child(
            crate::text_input(&s.edit, crate::TextStyle::new(14.0, 400.0))
                .key("editable-combo")
                .role("combobox")
                .label("Search")
                .size(120.0, 28.0)
                .on_edit(|s: &mut ComboState| &mut s.edit)
                .on_press(|s: &mut ComboState| s.opens += 100),
        )
}

#[test]
fn alt_down_opens_focused_non_input_combobox_and_consumes_repeats() {
    let mut rt = Runtime::new(ComboState::default(), combo_view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Focus combobox without keyboard modality first.
    rt.update(|_, tx| tx.focus("combo", false));
    rt.frame(0.0);
    assert!(!rt.kbd.focus.keyboard());

    let alt = Mods { alt: true, ..Default::default() };
    send(&mut rt, Key::Down, alt);
    assert_eq!(rt.state.opens, 1, "Alt+Down opens a focused combobox");
    assert!(rt.kbd.focus.keyboard(), "Alt+Down sets keyboard focus modality");

    // Repeat is consumed without firing again.
    rt.repeat_key(KeyEvent { key: Key::Down, mods: alt });
    rt.frame(0.0);
    assert_eq!(rt.state.opens, 1);

    // Extra modifiers or Alt+Up do not trigger it.
    send(&mut rt, Key::Up, alt);
    send(&mut rt, Key::Down, Mods { alt: true, shift: true, ..Default::default() });
    send(&mut rt, Key::Down, Mods { alt: true, ctrl: true, ..Default::default() });
    send(&mut rt, Key::Down, Mods { alt: true, cmd: true, ..Default::default() });
    assert_eq!(rt.state.opens, 1);

    // Non-combobox buttons ignore Alt+Down.
    rt.update(|_, tx| tx.focus("btn", true));
    rt.frame(0.0);
    send(&mut rt, Key::Down, alt);
    assert_eq!(rt.state.button_presses, 0);

    // Editable combobox (text_input with role("combobox")) ignores Alt+Down in focus_key.
    rt.update(|_, tx| tx.focus("editable-combo", true));
    rt.frame(0.0);
    send(&mut rt, Key::Down, alt);
    assert_eq!(rt.state.opens, 1);
}

// --- #1047: El::keyboard_focus_only() ---

#[derive(Default)]
struct KbdOnlyState {
    web_focus: bool,
    modal: Option<&'static str>,
    field: crate::TextEdit,
    pressed: Vec<&'static str>,
    window_keys: usize,
    secondary: Vec<&'static str>,
}

fn kbd_only_view(s: &KbdOnlyState, cx: &mut Cx<KbdOnlyState>) -> El {
    if s.web_focus { cx.web_focus(); }
    let btn = |key: &'static str, label: &'static str| {
        el().key(key)
            .role("button")
            .label(label)
            .size(100.0, 24.0)
            .focusable(true)
            .on_press(move |s: &mut KbdOnlyState| s.pressed.push(key))
            .on_secondary_press(move |s: &mut KbdOnlyState, _, _| s.secondary.push(key))
    };
    let mut root = col()
        .key("root")
        .size(300.0, 360.0)
        .child(
            crate::text_input(&s.field, crate::TextStyle::new(14.0, 400.0))
                .key("field")
                .size(100.0, 24.0)
                .on_edit(|s: &mut KbdOnlyState| &mut s.field),
        )
        .child(btn("normal", "Normal"))
        .child(btn("kbd-only", "Toolbar").keyboard_focus_only())
        .child(btn("kbd-preserve", "Preserve").keyboard_focus_only().preserve_focus_on_press(true))
        .child(
            col()
                .key("key-host")
                .size(100.0, 24.0)
                .focusable(true)
                .on_key(|_: &mut KbdOnlyState, _, _| false)
                .child(btn("inside-key-host", "Inside").keyboard_focus_only()),
        )
        .child(
            col()
                .key("kbd-only-host")
                .size(100.0, 24.0)
                .focusable(true)
                .keyboard_focus_only()
                .on_key(|_: &mut KbdOnlyState, _, _| false)
                .child(btn("inside-kbd-host", "InsideKbd").keyboard_focus_only()),
        )
        .child(
            el().key("disabled")
                .role("button")
                .label("Disabled")
                .size(100.0, 24.0)
                .focusable(false)
                .disabled(true)
                .keyboard_focus_only()
                .on_press(|s: &mut KbdOnlyState| s.pressed.push("disabled")),
        )
        .child(
            btn("inert-btn", "Inert")
                .keyboard_focus_only()
                .inert(),
        );
    if let Some(kind) = s.modal {
        let mut dialog = col()
            .key(if kind == "first" { "dialog-first" } else { "dialog-auto" })
            .absolute(0.0, 0.0)
            .size(200.0, 120.0)
            .role("dialog")
            .modal(true, |s: &mut KbdOnlyState| s.modal = None);
        dialog = if kind == "first" {
            dialog.child(btn("modal-first", "Modal First").keyboard_focus_only())
                .child(btn("modal-second", "Modal Second"))
        } else {
            dialog.child(btn("modal-first", "Modal First"))
                .child(btn("modal-auto", "Modal Auto").keyboard_focus_only().autofocus())
        };
        root = root.child(dialog);
    }
    root
}

fn click_key<S: 'static>(rt: &mut Runtime<S>, key: &str) {
    let e = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).expect("element by key");
    let (x, y) = (e.rect[0] + e.rect[2] * 0.5, e.rect[1] + e.rect[3] * 0.5);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
}

#[test]
fn keyboard_focus_only_click_runs_on_press_without_taking_focus() {
    let mut rt = Runtime::new(KbdOnlyState::default(), kbd_only_view, (300.0, 360.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, ev, _| {
        if matches!(ev.key, Key::Text(ref t) if t == " ") { s.window_keys += 1; }
        false
    });
    rt.frame(0.0);

    // 1. Nothing focused: click fires on_press, leaves nothing focused so window on_key still sees Space.
    click_key(&mut rt, "kbd-only");
    assert_eq!(rt.state.pressed, ["kbd-only"]);
    assert_eq!(focused_key(&rt), None, "keyboard_focus_only click must not take focus");
    press(&mut rt, Key::Text(" ".into()));
    assert_eq!(rt.state.window_keys, 1, "window on_key still sees keys after click");
    assert_eq!(rt.state.pressed, ["kbd-only"], "Space did not re-press unfocused toolbar button");

    // 2. Ordinary button without keyboard_focus_only still takes focus on click as before.
    click_key(&mut rt, "normal");
    assert_eq!(rt.state.pressed, ["kbd-only", "normal"]);
    assert_eq!(focused_key(&rt).as_deref(), Some("normal"));

    // 3. Clicking keyboard_focus_only while a field is focused fires on_press and blurs the field.
    click_key(&mut rt, "field");
    assert_eq!(focused_key(&rt).as_deref(), Some("field"));
    click_key(&mut rt, "kbd-only");
    assert_eq!(rt.state.pressed.last(), Some(&"kbd-only"));
    assert_eq!(focused_key(&rt), None, "click blurs previously focused field like plain content");

    // 4. With preserve_focus_on_press(true), clicking keeps the focused field focused.
    click_key(&mut rt, "field");
    assert_eq!(focused_key(&rt).as_deref(), Some("field"));
    click_key(&mut rt, "kbd-preserve");
    assert_eq!(rt.state.pressed.last(), Some(&"kbd-preserve"));
    assert_eq!(focused_key(&rt).as_deref(), Some("field"), "preserve_focus_on_press still keeps field focused");

    // 5. Inside a focusable on_key ancestor (without keyboard_focus_only), click focuses that ancestor.
    click_key(&mut rt, "inside-key-host");
    assert_eq!(rt.state.pressed.last(), Some(&"inside-key-host"));
    assert_eq!(focused_key(&rt).as_deref(), Some("key-host"));

    // 6. Inside a keyboard_focus_only on_key ancestor, click does not focus that ancestor either.
    click_key(&mut rt, "inside-kbd-host");
    assert_eq!(rt.state.pressed.last(), Some(&"inside-kbd-host"));
    assert_eq!(focused_key(&rt), None);

    // 7. Touch press also runs on_press without taking focus.
    let e = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("kbd-only")).unwrap();
    let (tx, ty) = (e.rect[0] + e.rect[2] * 0.5, e.rect[1] + e.rect[3] * 0.5);
    let before = rt.state.pressed.len();
    rt.touch(1, crate::TouchPhase::Started, tx, ty, 0.0);
    rt.touch(1, crate::TouchPhase::Ended, tx, ty, 0.05);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.pressed.len(), before + 1);
    assert_eq!(focused_key(&rt), None);

    // 8. Under Cx::web_focus, primary and secondary clicks skip keyboard_focus_only while ordinary buttons focus.
    rt.update(|s, _| s.web_focus = true);
    rt.frame(0.0);
    let e = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("kbd-only")).unwrap();
    let (x, y) = (e.rect[0] + e.rect[2] * 0.5, e.rect[1] + e.rect[3] * 0.5);
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    rt.frame(0.0);
    assert_eq!(rt.state.secondary, ["kbd-only"]);
    assert_eq!(focused_key(&rt), None, "secondary click under web_focus must not focus keyboard_focus_only");
    click_key(&mut rt, "kbd-only");
    assert_eq!(focused_key(&rt), None, "primary click under web_focus must not focus keyboard_focus_only");

    let ne = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("normal")).unwrap();
    rt.secondary_down(ne.rect[0] + ne.rect[2] * 0.5, ne.rect[1] + ne.rect[3] * 0.5);
    rt.secondary_up(ne.rect[0] + ne.rect[2] * 0.5, ne.rect[1] + ne.rect[3] * 0.5);
    rt.frame(0.0);
    assert_eq!(focused_key(&rt).as_deref(), Some("normal"), "ordinary button still focuses on secondary click under web_focus");
}

#[test]
fn keyboard_focus_only_stays_reachable_by_tab_modal_and_a11y() {
    use accesskit::{Action as Ak, ActionRequest, TreeId};

    let mut rt = Runtime::new(KbdOnlyState::default(), kbd_only_view, (300.0, 360.0), Env::new(true, 1.0));
    rt.set_tab_visits_controls(true);
    rt.frame(0.0);

    // 1. Tab reaches keyboard_focus_only button, shows keyboard modality, and Space activates on_press.
    let mut visited = Vec::new();
    for _ in 0..8 {
        press(&mut rt, Key::Tab);
        if let Some(k) = focused_key(&rt) {
            if k == "kbd-only" { break; }
            visited.push(k);
        }
    }
    assert_eq!(focused_key(&rt).as_deref(), Some("kbd-only"), "Tab must reach keyboard_focus_only (visited {visited:?})");
    assert!(rt.kbd.focus.keyboard());
    press(&mut rt, Key::Text(" ".into()));
    assert_eq!(rt.state.pressed, ["kbd-only"]);

    // Web a11y mirror includes the button and emits focus proxy while keyboard-focused.
    assert!(rt.web_a11y_items().iter().any(|it| it.kind == "button" && it.text == "Toolbar"));
    let mut mirror = crate::runtime::web_a11y::A11yMirror::default();
    let focus_json = rt.core().web_a11y_focus(&mut mirror).expect("web a11y focus proxy");
    assert!(focus_json.contains(r#""Toolbar""#), "web a11y focus proxy reports Toolbar: {focus_json}");

    // Clicking the button while it already holds focus from Tab runs on_press and blurs it.
    click_key(&mut rt, "kbd-only");
    assert_eq!(rt.state.pressed, ["kbd-only", "kbd-only"]);
    assert_eq!(focused_key(&rt), None);

    // 2. AccessKit tree exposes role button + Focus action, and Action::Focus focuses it.
    let tree = rt.accessibility_json();
    fn find_by_key<'a>(n: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
        if n["key"].as_str() == Some(key) { return Some(n); }
        n["children"].as_array()?.iter().find_map(|c| find_by_key(c, key))
    }
    let node = find_by_key(&tree, "kbd-only").expect("kbd-only in a11y tree");
    assert_eq!(node["role"].as_str(), Some("button"));
    let actions: Vec<&str> = node["actions"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
    assert!(actions.contains(&"focus"), "keyboard_focus_only must still advertise a11y focus: {actions:?}");
    let ak_tree = rt.accessibility_tree();
    let (id, _) = ak_tree.nodes.iter().find(|(_, n)| n.author_id() == Some("kbd-only")).unwrap();
    assert!(rt.a11y_action(&ActionRequest { action: Ak::Focus, target_tree: TreeId::ROOT, target_node: *id, data: None }));
    rt.frame(0.0);
    assert_eq!(focused_key(&rt).as_deref(), Some("kbd-only"), "AccessKit Action::Focus focuses keyboard_focus_only");

    // 3. Disabled and inert keyboard_focus_only controls cannot be focused or pressed.
    let inert_node = find_by_key(&tree, "inert-btn").expect("inert-btn in a11y tree");
    let inert_actions: Vec<&str> = inert_node["actions"].as_array().map_or(Vec::new(), |a| a.iter().filter_map(|v| v.as_str()).collect());
    assert!(!inert_actions.contains(&"focus"), "inert element must not advertise focus action");
    let (inert_id, _) = ak_tree.nodes.iter().find(|(_, n)| n.author_id() == Some("inert-btn")).unwrap();
    assert!(!rt.a11y_action(&ActionRequest { action: Ak::Focus, target_tree: TreeId::ROOT, target_node: *inert_id, data: None }));
    let before = rt.state.pressed.len();
    click_key(&mut rt, "inert-btn");
    assert_eq!(rt.state.pressed.len(), before, "inert click does not fire on_press");
    rt.update(|_, tx| tx.focus("disabled", true));
    rt.frame(0.0);
    assert_ne!(focused_key(&rt).as_deref(), Some("disabled"));

    // 4. Modal initial focus (first Tab stop and autofocus) still focuses keyboard_focus_only controls.
    rt.update(|s, _| s.modal = Some("first"));
    rt.frame(0.0);
    assert_eq!(focused_key(&rt).as_deref(), Some("modal-first"), "modal first-control focus picks keyboard_focus_only");

    rt.update(|s, _| s.modal = Some("auto"));
    rt.frame(0.0);
    assert_eq!(focused_key(&rt).as_deref(), Some("modal-auto"), "modal autofocus picks keyboard_focus_only");
}
