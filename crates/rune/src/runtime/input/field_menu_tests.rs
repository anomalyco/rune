//! Tests for `App::on_field_menu`, `FieldMenuRequest`, and `Tx::resolve_field_menu`.
use crate::{
    App, AppSpec, Cx, El, Env, FieldMenuRequest, Key, KeyEvent, Mods, OverlayOptions, OverlaySpec,
    PopupEntry, Runtime, TextEdit, TextStyle, Tx, col, el, text_input,
};

struct State {
    edit: TextEdit,
    disabled_edit: TextEdit,
    custom_edit: TextEdit,
    spellcheck: bool,
    requests: Vec<FieldMenuRequest>,
    custom_secondary: usize,
}

impl Default for State {
    fn default() -> Self {
        let mut edit = TextEdit::new("hello world");
        edit.set_selection(0, 5);
        Self {
            edit,
            disabled_edit: TextEdit::new("locked"),
            custom_edit: TextEdit::new("custom"),
            spellcheck: true,
            requests: Vec::new(),
            custom_secondary: 0,
        }
    }
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    col()
        .size(400.0, 300.0)
        .on_spellcheck(s.spellcheck, |s: &mut State, on, _| s.spellcheck = on)
        .child(
            text_input(&s.edit, TextStyle::new(14.0, 400.0))
                .key("main-field")
                .on_edit(|s: &mut State| &mut s.edit)
                .size(200.0, 32.0),
        )
        .child(
            text_input(&s.disabled_edit, TextStyle::new(14.0, 400.0))
                .key("disabled-field")
                .disabled(true)
                .size(200.0, 32.0),
        )
        .child(
            col().inert().child(
                text_input(&s.disabled_edit, TextStyle::new(14.0, 400.0))
                    .key("inert-field")
                    .on_edit(|s: &mut State| &mut s.disabled_edit)
                    .size(200.0, 32.0),
            ),
        )
        .child(
            text_input(&s.custom_edit, TextStyle::new(14.0, 400.0))
                .key("custom-field")
                .on_edit(|s: &mut State| &mut s.custom_edit)
                .size(200.0, 32.0)
                .on_secondary_press(|s: &mut State, _, _| s.custom_secondary += 1),
        )
        .child(el().key("other").size(80.0, 32.0).focusable(true))
}

fn make_runtime(with_hook: bool, force_fallback: bool) -> Runtime<State> {
    let spec = AppSpec::new("field-menu", (400.0, 300.0), State::default, view);
    let mut app = App::new(spec);
    if with_hook {
        app = app.on_field_menu(|s: &mut State, req: FieldMenuRequest, _| {
            s.requests.push(req);
        });
    }
    let mut rt = app.headless_runtime((400.0, 300.0), Env::new(true, 1.0));
    rt.core_mut().host.force_field_menu_fallback = force_fallback;
    rt.frame(0.0);
    rt
}

fn right_click(rt: &mut Runtime<State>, key: &str) -> (f32, f32) {
    let (x, y) = rt.find(key).expect("target element");
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    rt.frame(0.0);
    (x, y)
}

#[test]
fn native_platform_keeps_native_field_menu_even_when_on_field_menu_is_registered() {
    if !crate::popup_menu_supported() {
        return;
    }
    let mut rt = make_runtime(true, false);
    right_click(&mut rt, "main-field");
    assert!(rt.state.requests.is_empty(), "on_field_menu must not run when popup_menu_supported() is true");
    assert!(rt.popup_menu().is_some(), "native field menu still queues on popup_menu_supported() platforms");
}

#[test]
fn without_on_field_menu_behavior_is_unchanged() {
    let mut rt = make_runtime(false, true);
    right_click(&mut rt, "main-field");
    assert!(rt.state.requests.is_empty());
    assert!(rt.popup_menu().is_some(), "without hook, existing popup queue path stays unchanged");
}

#[test]
fn fallback_delivers_field_menu_request_and_resolves_copy_cut_paste_select_all_and_spellcheck() {
    let board = crate::clipboard::memory();
    let mut rt = make_runtime(true, true);
    rt.use_clipboard(&board);

    // Focus and select "hello" so Cut and Copy are enabled.
    rt.update(|s, _| {
        s.edit.focused = true;
        s.edit.set_selection(0, 5);
    });
    rt.frame(0.0);

    // Trigger via ContextMenu key so the existing selection is preserved regardless of hit x.
    rt.key(KeyEvent::from(Key::ContextMenu));
    rt.frame(0.0);

    assert!(rt.popup_menu().is_none(), "fallback path does not queue native popup");
    assert_eq!(rt.state.requests.len(), 1);
    let req = rt.state.requests[0].clone();
    assert_eq!(req.field_key(), Some("main-field"));
    assert!(req.field_path().ends_with("main-field"));
    assert!(req.point().y >= 30.0, "keyboard menu anchors at field bottom-left: {:?}", req.point());

    // Inspect items via PopupMenu::entries().
    let ids: Vec<&str> = req.menu().entries().iter().filter_map(|e| match e {
        PopupEntry::Item(i) => Some(i.id()),
        _ => None,
    }).collect();
    assert!(ids.contains(&"cut") && ids.contains(&"copy") && ids.contains(&"paste") && ids.contains(&"spellcheck"), "entries: {ids:?}");
    assert!(req.menu().find("copy").unwrap().is_enabled());
    assert!(req.menu().find("cut").unwrap().is_enabled());
    assert_eq!(req.clone().into_menu(), *req.menu());

    // Move focus away (as an in-window modal menu does) before resolving Copy.
    rt.update(|_, tx| tx.focus("other", true));
    rt.frame(0.0);
    rt.update(|_, tx| tx.resolve_field_menu(Some("copy")));
    rt.frame(0.0);
    assert_eq!(board.content().unwrap().text(), "hello", "resolve_field_menu(copy) copies selected text");
    assert_eq!(rt.state.edit.text, "hello world");

    // Resolve Cut.
    rt.update(|s, _| s.edit.set_selection(0, 5));
    rt.frame(0.0);
    rt.key(KeyEvent::from(Key::ContextMenu));
    rt.frame(0.0);
    rt.update(|_, tx| tx.resolve_field_menu(Some("cut")));
    rt.frame(0.0);
    assert_eq!(board.content().unwrap().text(), "hello");
    assert_eq!(rt.state.edit.text, " world");

    // Resolve Paste.
    rt.key(KeyEvent::from(Key::ContextMenu));
    rt.frame(0.0);
    rt.update(|_, tx| tx.resolve_field_menu(Some("paste")));
    rt.frame(0.0);
    assert_eq!(rt.state.edit.text, "hello world");

    // Resolve Spellcheck toggle.
    assert!(rt.state.spellcheck);
    rt.key(KeyEvent::from(Key::ContextMenu));
    rt.frame(0.0);
    rt.update(|_, tx| tx.resolve_field_menu(Some("spellcheck")));
    rt.frame(0.0);
    assert!(!rt.state.spellcheck);

    // Resolve Select All off macOS.
    if !cfg!(target_os = "macos") {
        rt.update(|s, _| s.edit.set_caret(0));
        rt.frame(0.0);
        rt.key(KeyEvent::from(Key::ContextMenu));
        rt.frame(0.0);
        rt.update(|_, tx| tx.resolve_field_menu(Some("select-all")));
        rt.frame(0.0);
        assert_eq!(rt.state.edit.range(), (0, "hello world".len()));
    }
}

#[test]
fn fallback_right_click_disabled_unknown_dismiss_custom_and_disabled_field() {
    let board = crate::clipboard::memory();
    let mut rt = make_runtime(true, true);
    rt.use_clipboard(&board);

    // Right-clicking main-field with collapsed caret leaves Cut/Copy disabled.
    rt.update(|s, _| s.edit.set_caret(0));
    rt.frame(0.0);
    let (rx, ry) = right_click(&mut rt, "main-field");
    assert_eq!(rt.state.requests.len(), 1);
    let req = &rt.state.requests[0];
    assert_eq!(req.field_key(), Some("main-field"));
    assert!((req.point().x - rx).abs() < 0.5 && (req.point().y - ry).abs() < 0.5);
    assert!(!req.menu().find("copy").unwrap().is_enabled());

    // Resolving a disabled or unknown id is ignored and leaves the pending menu intact.
    rt.update(|_, tx| tx.resolve_field_menu(Some("copy")));
    rt.frame(0.0);
    assert!(board.content().is_none(), "disabled item does not run");
    rt.update(|_, tx| tx.resolve_field_menu(Some("nonexistent")));
    rt.frame(0.0);
    assert!(rt.core().host.field_menu_fallback.is_some(), "invalid pick leaves menu pending");

    // Dismissing with None clears the pending menu; a later resolve is a no-op.
    rt.update(|_, tx| tx.resolve_field_menu(None));
    rt.frame(0.0);
    assert!(rt.core().host.field_menu_fallback.is_none());
    rt.update(|_, tx| tx.resolve_field_menu(Some("paste")));
    rt.frame(0.0);
    assert_eq!(rt.state.edit.text, "hello world");

    // Disabled and inert text fields do not invoke on_field_menu.
    let before = rt.state.requests.len();
    right_click(&mut rt, "disabled-field");
    assert_eq!(rt.state.requests.len(), before, "disabled field does not open field menu");
    right_click(&mut rt, "inert-field");
    assert_eq!(rt.state.requests.len(), before, "inert field does not open field menu");

    // A field with its own on_secondary_press keeps running that instead of on_field_menu.
    right_click(&mut rt, "custom-field");
    assert_eq!(rt.state.custom_secondary, 1);
    assert_eq!(rt.state.requests.len(), before);

    // Shift+F10 opens the fallback menu too.
    rt.update(|s, _| s.edit.focused = true);
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Function(10), mods: Mods { shift: true, ..Mods::default() } });
    rt.frame(0.0);
    assert_eq!(rt.state.requests.len(), before + 1);
}

#[test]
fn configured_overlay_wires_on_field_menu() {
    let overlay = OverlaySpec::new("quick", (320.0, 200.0), |_: &(), _| el())
        .options(OverlayOptions::default())
        .on_field_menu(|_: &mut (), _: FieldMenuRequest, _: &mut Tx| {});
    assert!(overlay.runtime.on_field_menu.is_some());
}

#[test]
fn disabled_and_aria_disabled_text_inputs_with_on_edit_are_not_secondary_targets() {
    struct Case {
        disabled_edit: TextEdit,
        disabled_custom: TextEdit,
        disabled_in_ancestor: TextEdit,
        ancestor_disabled: TextEdit,
        aria_edit: TextEdit,
        aria_custom: TextEdit,
        read_only_edit: TextEdit,
        disabled_pill: TextEdit,
        requests: Vec<FieldMenuRequest>,
        field_secondary: Vec<&'static str>,
        ancestor_secondary: Vec<&'static str>,
        pill_secondary: usize,
    }

    impl Default for Case {
        fn default() -> Self {
            let mut disabled_pill = TextEdit::new("").rich();
            disabled_pill.insert_pill("p1", "@Alex");
            disabled_pill.insert(" note");
            Self {
                disabled_edit: TextEdit::new("locked"),
                disabled_custom: TextEdit::new("locked custom"),
                disabled_in_ancestor: TextEdit::new("in ancestor"),
                ancestor_disabled: TextEdit::new("under disabled"),
                aria_edit: TextEdit::new("pending"),
                aria_custom: TextEdit::new("pending custom"),
                read_only_edit: TextEdit::new("read only"),
                disabled_pill,
                requests: Vec::new(),
                field_secondary: Vec::new(),
                ancestor_secondary: Vec::new(),
                pill_secondary: 0,
            }
        }
    }

    fn case_view(s: &Case, _: &mut Cx<Case>) -> El {
        let st = TextStyle::new(14.0, 400.0);
        col()
            .size(320.0, 320.0)
            .gap(4.0)
            .child(
                text_input(&s.disabled_edit, st)
                    .key("disabled-editable")
                    .label("Disabled editable")
                    .disabled(true)
                    .on_edit(|s: &mut Case| &mut s.disabled_edit)
                    .size(200.0, 28.0),
            )
            .child(
                text_input(&s.disabled_custom, st)
                    .key("disabled-custom")
                    .label("Disabled custom")
                    .disabled(true)
                    .on_edit(|s: &mut Case| &mut s.disabled_custom)
                    .on_secondary_press(|s: &mut Case, _, _| s.field_secondary.push("disabled-custom"))
                    .size(200.0, 28.0),
            )
            .child(
                col()
                    .key("enabled-ancestor")
                    .size(200.0, 28.0)
                    .on_secondary_press(|s: &mut Case, _, _| s.ancestor_secondary.push("enabled-ancestor"))
                    .child(
                        text_input(&s.disabled_in_ancestor, st)
                            .key("disabled-in-ancestor")
                            .label("Disabled in ancestor")
                            .disabled(true)
                            .on_edit(|s: &mut Case| &mut s.disabled_in_ancestor)
                            .on_secondary_press(|s: &mut Case, _, _| s.field_secondary.push("disabled-in-ancestor"))
                            .size(200.0, 28.0),
                    ),
            )
            .child(
                col()
                    .key("disabled-ancestor")
                    .disabled(true)
                    .size(200.0, 28.0)
                    .child(
                        text_input(&s.ancestor_disabled, st)
                            .key("under-disabled-ancestor")
                            .label("Under disabled ancestor")
                            .on_edit(|s: &mut Case| &mut s.ancestor_disabled)
                            .size(200.0, 28.0),
                    ),
            )
            .child(
                text_input(&s.aria_edit, st)
                    .key("aria-disabled")
                    .label("Aria disabled")
                    .aria_disabled(true)
                    .on_edit(|s: &mut Case| &mut s.aria_edit)
                    .size(200.0, 28.0),
            )
            .child(
                col()
                    .key("aria-ancestor")
                    .size(200.0, 28.0)
                    .on_secondary_press(|s: &mut Case, _, _| s.ancestor_secondary.push("aria-ancestor"))
                    .child(
                        text_input(&s.aria_custom, st)
                            .key("aria-disabled-custom")
                            .label("Aria disabled custom")
                            .aria_disabled(true)
                            .on_edit(|s: &mut Case| &mut s.aria_custom)
                            .on_secondary_press(|s: &mut Case, _, _| s.field_secondary.push("aria-disabled-custom"))
                            .size(200.0, 28.0),
                    ),
            )
            .child(
                text_input(&s.read_only_edit, st)
                    .key("read-only-editable")
                    .label("Read only")
                    .read_only(true)
                    .on_edit(|s: &mut Case| &mut s.read_only_edit)
                    .size(200.0, 28.0),
            )
            .child(
                text_input(&s.disabled_pill, st)
                    .key("disabled-pill")
                    .label("Disabled pill")
                    .disabled(true)
                    .on_edit(|s: &mut Case| &mut s.disabled_pill)
                    .on_pill_secondary(|s: &mut Case, _, _, _| {
                        s.pill_secondary += 1;
                        true
                    })
                    .size(200.0, 28.0),
            )
    }

    let spec = AppSpec::new("disabled-secondary", (320.0, 320.0), Case::default, case_view);
    let mut rt = App::new(spec)
        .on_field_menu(|s: &mut Case, req: FieldMenuRequest, _| s.requests.push(req))
        .headless_runtime((320.0, 320.0), Env::new(true, 1.0));
    rt.core_mut().host.force_field_menu_fallback = true;
    rt.frame(0.0);

    let rclick = |rt: &mut Runtime<Case>, x: f32, y: f32| {
        rt.secondary_down(x, y);
        let claim = rt.core().secondary_claim();
        rt.secondary_up(x, y);
        rt.frame(0.0);
        claim
    };

    // 1. Standalone disabled, ancestor-disabled, and aria_disabled inputs with on_edit are not targeted.
    for key in ["disabled-editable", "disabled-custom", "under-disabled-ancestor", "aria-disabled"] {
        let (x, y) = rt.find(key).expect(key);
        assert_eq!(rt.core().secondary_target(x, y), None, "{key} must not be a secondary target");
        let claim = rclick(&mut rt, x, y);
        assert_eq!(claim, "", "{key} must not claim browser context menu");
        assert!(rt.popup_menu().is_none(), "{key} must not queue native field menu");
    }
    assert!(rt.state.requests.is_empty(), "disabled/aria_disabled inputs must not fire on_field_menu");
    assert!(rt.state.field_secondary.is_empty(), "disabled/aria_disabled inputs must not fire on_secondary_press");
    assert!(!rt.state.disabled_edit.focused, "secondary press on disabled input must not focus it");

    // 2. Touch long-press on a disabled input with on_edit does not open a field menu.
    let (dx, dy) = rt.find("disabled-editable").unwrap();
    rt.touch(1, crate::TouchPhase::Started, dx, dy, 0.0);
    rt.frame(0.6);
    rt.touch(1, crate::TouchPhase::Ended, dx, dy, 0.6);
    rt.frame(0.0);
    assert!(rt.state.requests.is_empty() && rt.popup_menu().is_none(), "touch long-press on disabled input opens no menu");

    // 3. Disabled and aria_disabled inputs inside an enabled ancestor fall through to the ancestor.
    for (field_key, expected_ancestor) in [
        ("disabled-in-ancestor", "enabled-ancestor"),
        ("aria-disabled-custom", "aria-ancestor"),
    ] {
        let (x, y) = rt.find(field_key).expect(field_key);
        let target = rt.core().secondary_target(x, y).expect("ancestor target");
        assert!(target.ends_with(expected_ancestor), "{field_key} should fall through to {expected_ancestor}, got {target}");
        let claim = rclick(&mut rt, x, y);
        assert_eq!(claim, "app");
        assert_eq!(rt.state.ancestor_secondary.last(), Some(&expected_ancestor));
    }
    assert!(rt.state.field_secondary.is_empty(), "disabled child on_secondary_press must not fire");
    assert!(rt.state.requests.is_empty(), "disabled child must not fire on_field_menu");

    // 4. Disabled input with a pill never fires on_pill_secondary.
    let pill_rect = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("disabled-pill")).unwrap().rect;
    let _ = rclick(&mut rt, pill_rect[0] + 12.0, pill_rect[1] + 10.0);
    assert_eq!(rt.state.pill_secondary, 0);
    assert!(rt.state.requests.is_empty());

    // 5. Focused aria_disabled inputs suppress ContextMenu and Shift+F10 (falling through to an enabled ancestor).
    rt.update(|s, _| s.aria_edit.focused = true);
    rt.frame(0.0);
    rt.key(KeyEvent::from(Key::ContextMenu));
    rt.key(KeyEvent { key: Key::Function(10), mods: Mods { shift: true, ..Mods::default() } });
    rt.frame(0.0);
    assert!(rt.state.requests.is_empty() && rt.popup_menu().is_none(), "ContextMenu/Shift+F10 on aria_disabled opens no field menu");

    let anc_before = rt.state.ancestor_secondary.len();
    rt.update(|s, _| {
        s.aria_edit.focused = false;
        s.aria_custom.focused = true;
    });
    rt.frame(0.0);
    rt.key(KeyEvent::from(Key::ContextMenu));
    rt.frame(0.0);
    assert!(rt.state.field_secondary.is_empty(), "ContextMenu on aria_disabled must not run its own on_secondary_press");
    assert_eq!(rt.state.ancestor_secondary.len(), anc_before + 1, "ContextMenu on aria_disabled falls through to enabled ancestor");
    assert_eq!(rt.state.ancestor_secondary.last(), Some(&"aria-ancestor"));

    // 6. A read-only input is not disabled and remains a secondary target.
    let (rx, ry) = rt.find("read-only-editable").unwrap();
    assert!(
        rt.core().secondary_target(rx, ry).is_some_and(|p| p.ends_with("read-only-editable")),
        "read_only input must stay distinct from disabled"
    );
}
