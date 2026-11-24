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
