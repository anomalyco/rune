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

#[test]
fn disabled_text_inputs_cannot_take_focus_edit_or_show_text_cursor() {
    use accesskit::{Action as Ak, ActionRequest, TreeId};
    use crate::{Cursor, TextEdit, TextStyle, text_input};

    struct State {
        first: TextEdit,
        disabled: TextEdit,
        disabled_explicit: TextEdit,
        in_disabled_anc: TextEdit,
        aria: TextEdit,
        read_only: TextEdit,
        group_dis: TextEdit,
        dynamic: TextEdit,
        dynamic_off: bool,
        modal_open: bool,
        modal_dis: TextEdit,
        opener_dis: bool,
        changes: Vec<&'static str>,
        pastes: Vec<&'static str>,
        submits: Vec<&'static str>,
        keys_heard: Vec<&'static str>,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let st = TextStyle::new(14.0, 400.0);
        let mut root = col()
            .size(320.0, 460.0)
            .gap(4.0)
            .child(
                text_input(&s.first, st)
                    .key("first")
                    .label("First")
                    .size(200.0, 26.0)
                    .disabled(s.opener_dis)
                    .on_edit(|s: &mut State| &mut s.first),
            )
            .child(
                text_input(&s.disabled, st)
                    .key("disabled-field")
                    .label("Disabled")
                    .size(200.0, 26.0)
                    .disabled(true)
                    .on_edit(|s: &mut State| &mut s.disabled)
                    .on_input_change(|s: &mut State, _| s.changes.push("disabled"))
                    .on_input_paste(|s: &mut State, _, _| { s.pastes.push("disabled"); true })
                    .on_submit(|s: &mut State, _| s.submits.push("disabled")),
            )
            .child(
                text_input(&s.disabled_explicit, st)
                    .key("disabled-explicit")
                    .label("Disabled explicit")
                    .size(200.0, 26.0)
                    .disabled(true)
                    .cursor(Cursor::NotAllowed)
                    .on_edit(|s: &mut State| &mut s.disabled_explicit),
            )
            .child(
                col()
                    .key("disabled-anc")
                    .disabled(true)
                    .size(200.0, 26.0)
                    .child(
                        text_input(&s.in_disabled_anc, st)
                            .key("under-disabled-anc")
                            .label("Under disabled ancestor")
                            .size(200.0, 26.0)
                            .on_edit(|s: &mut State| &mut s.in_disabled_anc)
                            .on_input_change(|s: &mut State, _| s.changes.push("anc")),
                    ),
            )
            .child(
                text_input(&s.aria, st)
                    .key("aria-field")
                    .label("Aria disabled")
                    .size(200.0, 26.0)
                    .aria_disabled(true)
                    .on_edit(|s: &mut State| &mut s.aria)
                    .on_input_key(|s: &mut State, _, _| { s.keys_heard.push("aria"); false })
                    .on_input_change(|s: &mut State, _| s.changes.push("aria"))
                    .on_input_paste(|s: &mut State, _, _| { s.pastes.push("aria"); true })
                    .on_submit(|s: &mut State, _| s.submits.push("aria")),
            )
            .child(
                text_input(&s.read_only, st)
                    .key("read-only-field")
                    .label("Read only")
                    .size(200.0, 26.0)
                    .read_only(true)
                    .on_edit(|s: &mut State| &mut s.read_only),
            )
            .child(
                row()
                    .key("grp")
                    .role("toolbar")
                    .focus_group()
                    .size(220.0, 26.0)
                    .child(el().key("grp-a").role("button").label("A").size(60.0, 26.0).focusable(true))
                    .child(
                        text_input(&s.group_dis, st)
                            .key("grp-dis-input")
                            .label("Group disabled")
                            .size(80.0, 26.0)
                            .disabled(true)
                            .on_edit(|s: &mut State| &mut s.group_dis),
                    )
                    .child(el().key("grp-b").role("button").label("B").size(60.0, 26.0).focusable(true)),
            )
            .child(
                text_input(&s.dynamic, st)
                    .key("dynamic")
                    .label("Dynamic")
                    .size(200.0, 26.0)
                    .disabled(s.dynamic_off)
                    .on_edit(|s: &mut State| &mut s.dynamic),
            )
            .child(el().key("after").role("button").label("After").size(100.0, 26.0).focusable(true));
        if s.modal_open {
            root = root.child(
                col()
                    .key("dlg")
                    .role("dialog")
                    .absolute(0.0, 0.0)
                    .size(240.0, 120.0)
                    .modal(true, |s: &mut State| s.modal_open = false)
                    .child(
                        text_input(&s.modal_dis, st)
                            .key("modal-dis-auto")
                            .label("Modal disabled")
                            .size(200.0, 26.0)
                            .disabled(true)
                            .autofocus()
                            .on_edit(|s: &mut State| &mut s.modal_dis),
                    )
                    .child(el().key("modal-ok").role("button").label("OK").size(80.0, 26.0).focusable(true)),
            );
        }
        root
    }

    let mut rt = Runtime::new(
        State {
            first: TextEdit::new("one"),
            disabled: TextEdit::new("locked"),
            disabled_explicit: TextEdit::new("explicit"),
            in_disabled_anc: TextEdit::new("anc"),
            aria: TextEdit::new("pending"),
            read_only: TextEdit::new("ro"),
            group_dis: TextEdit::new("gd"),
            dynamic: TextEdit::new("dyn"),
            dynamic_off: false,
            modal_open: false,
            modal_dis: TextEdit::new("md"),
            opener_dis: false,
            changes: Vec::new(),
            pastes: Vec::new(),
            submits: Vec::new(),
            keys_heard: Vec::new(),
        },
        view,
        (320.0, 460.0),
        Env::new(true, 1.0),
    );
    let board = crate::clipboard::memory();
    board.set_text("PASTE");
    rt.use_clipboard(&board);
    rt.frame(0.0);

    let cursor_of = |rt: &mut Runtime<State>, key: &str| {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing {key}"));
        rt.pointer_move(x, y);
        rt.cursor()
    };

    // 1. Primary click, Tx::focus, edit.focused = true, and AccessKit Focus cannot focus disabled or ancestor-disabled inputs,
    //    and typed characters, delete, paste, and submit must never reach their handlers.
    for key in ["disabled-field", "under-disabled-anc"] {
        click_key(&mut rt, key);
        let focused_after_click = focused_key(&rt);
        rt.key(KeyEvent::from(Key::Text("Z".into())));
        rt.frame(0.0);
        assert_eq!(rt.state.disabled.text, "locked", "typing mutated disabled field (focused={focused_after_click:?})");
        assert_eq!(rt.state.in_disabled_anc.text, "anc", "typing mutated field under disabled ancestor (focused={focused_after_click:?})");
        rt.key(KeyEvent::from(Key::Backspace));
        rt.key(KeyEvent::from(Key::Delete));
        rt.paste("PASTE");
        rt.key(KeyEvent::from(Key::Enter));
        rt.frame(0.0);
        assert_eq!(rt.state.disabled.text, "locked");
        assert_eq!(rt.state.in_disabled_anc.text, "anc");
        assert!(rt.state.changes.is_empty() && rt.state.pastes.is_empty() && rt.state.submits.is_empty());
        assert_ne!(focused_after_click.as_deref(), Some(key), "click must not focus {key}");

        rt.update(move |_, tx| tx.focus(key, true));
        rt.frame(0.0);
        assert_ne!(focused_key(&rt).as_deref(), Some(key), "Tx::focus must not focus {key}");

        let ak_tree = rt.accessibility_tree();
        let (id, node) = ak_tree.nodes.iter().find(|(_, n)| n.author_id() == Some(key)).unwrap();
        assert!(!node.supports_action(Ak::Focus), "{key} must not advertise AccessKit Focus");
        assert!(!rt.a11y_action(&ActionRequest { action: Ak::Focus, target_tree: TreeId::ROOT, target_node: *id, data: None }));
        rt.frame(0.0);
        assert_ne!(focused_key(&rt).as_deref(), Some(key), "AccessKit Focus must not focus {key}");
    }
    rt.update(|s, _| s.disabled.focused = true);
    rt.frame(0.0);
    assert_ne!(focused_key(&rt).as_deref(), Some("disabled-field"), "edit.focused = true must not focus disabled input");
    assert!(!rt.state.disabled.focused, "sync clears edit.focused on disabled input");
    assert_eq!(rt.state.disabled.text, "locked");
    assert_eq!(rt.state.in_disabled_anc.text, "anc");
    assert!(rt.state.changes.is_empty() && rt.state.pastes.is_empty() && rt.state.submits.is_empty());

    // 3. Tab and Shift+Tab skip all disabled inputs, keep aria_disabled and read_only in Tab order.
    let mut visited = Vec::new();
    for _ in 0..8 {
        rt.key(KeyEvent::from(Key::Tab));
        rt.frame(0.0);
        let k = focused_key(&rt).unwrap_or_default();
        if visited.first() == Some(&k) { break; }
        visited.push(k);
    }
    assert_eq!(visited, ["first", "aria-field", "read-only-field", "grp-a", "dynamic", "after"]);
    // Shift+Tab walks the exact reverse order, skipping disabled inputs too.
    rt.key(KeyEvent { key: Key::Tab, mods: Mods { shift: true, ..Mods::default() } });
    rt.frame(0.0);
    assert_eq!(focused_key(&rt).as_deref(), Some("after"), "Shift+Tab wraps from first to after, skipping disabled inputs");

    // 4. Focus-group arrows skip disabled text_input inside the group.
    rt.update(|_, tx| tx.focus("grp-a", true));
    rt.frame(0.0);
    rt.key(KeyEvent::from(Key::Right));
    rt.frame(0.0);
    assert_eq!(focused_key(&rt).as_deref(), Some("grp-b"), "focus group arrow skips disabled input");

    // 5. aria_disabled input stays focusable (Tab, Tx::focus, AccessKit Focus, web focus proxy) but rejects all edits/paste/delete/submit.
    rt.update(|_, tx| tx.focus("aria-field", true));
    rt.frame(0.0);
    assert_eq!(focused_key(&rt).as_deref(), Some("aria-field"), "aria_disabled field stays focusable");
    assert!(!rt.core().web_has_editable_focus(), "aria_disabled field must not activate web DOM editor");
    let mut mirror = crate::runtime::web_a11y::A11yMirror::default();
    let focus_json = rt.core().web_a11y_focus(&mut mirror).expect("web focus proxy for aria_disabled");
    assert!(focus_json.contains(r#""dis":true"#), "focus proxy reports aria-disabled: {focus_json}");

    rt.key(KeyEvent::from(Key::Text("X".into())));
    rt.key(KeyEvent::from(Key::Backspace));
    rt.key(KeyEvent::from(Key::Delete));
    rt.key(KeyEvent::from(Key::Enter));
    rt.paste("NOPE");
    rt.key(KeyEvent { key: Key::Command('v'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(0.0);
    rt.frame(0.0);
    assert_eq!(rt.state.aria.text, "pending", "aria_disabled input must not accept edits, delete, or paste");
    assert!(rt.state.changes.is_empty() && rt.state.pastes.is_empty() && rt.state.submits.is_empty() && rt.state.keys_heard.is_empty());

    // 6. Disabling an already focused text_input blurs it, clears edit.focused, and hands focus to the next stop.
    rt.update(|_, tx| tx.focus("dynamic", true));
    rt.frame(0.0);
    assert_eq!(focused_key(&rt).as_deref(), Some("dynamic"));
    assert!(rt.state.dynamic.focused);
    rt.update(|s, _| s.dynamic_off = true);
    rt.frame(0.0);
    assert_eq!(focused_key(&rt).as_deref(), Some("after"), "disabling focused input rescues focus");
    assert!(!rt.state.dynamic.focused, "disabled input clears edit.focused");

    // 7. Modal autofocus skips disabled input, and closing a modal does not restore focus to an opener disabled while open.
    rt.update(|_, tx| tx.focus("first", true));
    rt.frame(0.0);
    assert_eq!(focused_key(&rt).as_deref(), Some("first"));
    rt.update(|s, _| s.modal_open = true);
    rt.frame(0.0);
    assert_eq!(focused_key(&rt).as_deref(), Some("modal-ok"), "modal autofocus skips disabled text_input");
    rt.update(|s, _| {
        s.opener_dis = true;
        s.modal_open = false;
    });
    rt.frame(0.0);
    assert_ne!(focused_key(&rt).as_deref(), Some("first"), "modal close must not restore focus to disabled input");
    assert!(!rt.state.first.focused);

    // 8. read_only input stays focusable on click.
    click_key(&mut rt, "read-only-field");
    assert_eq!(focused_key(&rt).as_deref(), Some("read-only-field"), "read_only input remains focusable");

    // 9. Cursor: disabled, ancestor-disabled, and aria_disabled show Default; explicit cursor and enabled/read_only keep theirs.
    rt.update(|s, _| s.opener_dis = false);
    rt.frame(0.0);
    assert_eq!(cursor_of(&mut rt, "first"), Cursor::Text);
    assert_eq!(cursor_of(&mut rt, "read-only-field"), Cursor::Text, "read_only keeps text cursor");
    assert_eq!(cursor_of(&mut rt, "disabled-field"), Cursor::Default, "disabled text_input must not show text cursor");
    assert_eq!(cursor_of(&mut rt, "under-disabled-anc"), Cursor::Default, "text_input under disabled ancestor must not show text cursor");
    assert_eq!(cursor_of(&mut rt, "aria-field"), Cursor::Default, "aria_disabled text_input shows default cursor");
    assert_eq!(cursor_of(&mut rt, "disabled-explicit"), Cursor::NotAllowed, "explicit cursor on disabled text_input still wins");
}
