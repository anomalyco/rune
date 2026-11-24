//! Standalone headless app testing (no playground): build from `AppSpec`, wire `on_key`,
//! click by key, type into a field, read `elements()` and `accessibility_json()`, and
//! inspect `PopupMenu::entries()` / `PopupEntry` / `selected_id()` from outside the crate.
use rune::{
    App, AppSpec, Cx, El, Env, Key, KeyEvent, Mods, PointerPress, PopupEntry, PopupItem,
    PopupMenu, Shortcut, TextEdit, TextStyle, Tx, col, el, text, text_input,
};

struct AppState {
    query: TextEdit,
    submitted: Vec<String>,
    theme: String,
    last_action: String,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            query: TextEdit::default(),
            submitted: Vec::new(),
            theme: "system".into(),
            last_action: String::new(),
        }
    }
}

fn on_key(s: &mut AppState, ev: &KeyEvent, _: &mut Tx) -> bool {
    if ev.key == Key::Escape {
        s.last_action = "escaped".into();
        return true;
    }
    false
}

fn view(s: &AppState, _: &mut Cx<AppState>) -> El {
    col().w_full().h_full().gap(8.0).padding(16.0)
        .child(
            text_input(&s.query, TextStyle::new(14.0, 400.0))
                .key("query")
                .label("Search")
                .size(200.0, 28.0)
                .on_edit(|s: &mut AppState| &mut s.query)
                .on_submit(|s: &mut AppState, _| s.submitted.push(s.query.text.clone())),
        )
        .child(
            el().key("theme").role("button").label("Theme").size(100.0, 28.0)
                .child(text(format!("Theme: {}", s.theme)).key("theme-label"))
                .on_press_tx(|s: &mut AppState, tx: &mut Tx| {
                    let menu = PopupMenu::picker([("system", "System"), ("dark", "Dark")], &s.theme);
                    tx.popup_menu(menu, "theme", |s: &mut AppState, picked, _| {
                        if let Some(id) = picked { s.theme = id.into(); }
                    });
                }),
        )
        .child(
            el().key("canvas").size(200.0, 60.0)
                .on_secondary_press(|_: &mut AppState, press: PointerPress, tx: &mut Tx| {
                    let copy_shortcut: Shortcut = "mod+c".parse().unwrap();
                    let menu = PopupMenu::new()
                        .item(PopupItem::new("copy", "Copy").key_equivalent(copy_shortcut).symbol("doc.on.doc"))
                        .separator()
                        .item(PopupItem::new("paste", "Paste").enabled(false))
                        .submenu("More", PopupMenu::new().item(PopupItem::new("export", "Export")));
                    tx.popup_menu(menu, press, |s: &mut AppState, picked, _| {
                        s.last_action = picked.unwrap_or("dismissed").into();
                    });
                }),
        )
}

#[test]
fn standalone_headless_app_test_drives_input_a11y_and_popup_entries() {
    let mut spec = AppSpec::new("test app", (400.0, 300.0), AppState::default, view);
    spec.on_key = Some(on_key);
    let mut rt = App::new(spec).headless_runtime((400.0, 300.0), Env::new(false, 1.0));
    rt.frame(0.0);

    // Click by key, type, and submit.
    let (x, y) = rt.find("query").expect("query field laid out");
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Text("rune".into()), mods: Mods::default() });
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.submitted, ["rune"]);

    // Inspect elements() and accessibility_json().
    let label = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("theme-label")).and_then(|e| e.text);
    assert_eq!(label.as_deref(), Some("Theme: system"));
    let a11y = rt.accessibility_json();
    assert!(a11y.to_string().contains("Theme"), "a11y tree exposes button label: {a11y}");

    // First Escape blurs the focused text_input; the next Escape routes to spec.on_key.
    assert!(rt.has_focus());
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(0.0);
    assert!(!rt.has_focus());
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.last_action, "escaped");

    // Open picker popup menu, inspect PopupEntry slice and selected_id, then pick.
    let (tx, ty) = rt.find("theme").expect("theme button");
    rt.pointer_down(tx, ty);
    rt.pointer_up(tx, ty);
    rt.frame(0.0);
    let picker = rt.popup_menu().expect("picker menu open");
    assert_eq!(picker.selected_id(), Some("system"));
    assert_eq!(picker.entries().len(), 2);
    assert!(matches!(&picker.entries()[0], PopupEntry::Item(i) if i.id() == "system" && i.is_checked()));
    assert!(matches!(&picker.entries()[1], PopupEntry::Item(i) if i.id() == "dark" && !i.is_checked()));
    rt.resolve_popup(Some("dark")).unwrap();
    rt.frame(0.0);
    assert_eq!(rt.state.theme, "dark");

    // Open context menu and inspect shortcut, symbol, separator, disabled item, and submenu.
    let (cx, cy) = rt.find("canvas").expect("canvas");
    rt.secondary_down(cx, cy);
    rt.secondary_up(cx, cy);
    rt.frame(0.0);
    let menu = rt.popup_menu().expect("context menu open");
    assert_eq!(menu.selected_id(), None);
    let mut kinds = Vec::new();
    for entry in menu.entries() {
        match entry {
            PopupEntry::Item(item) => {
                if item.id() == "copy" {
                    assert_eq!(item.symbol_name(), Some("doc.on.doc"));
                    assert_eq!(item.key_equivalent_shortcut(), Some(&"mod+c".parse().unwrap()));
                }
                kinds.push(format!("item:{}:{}", item.id(), item.is_enabled()));
            }
            PopupEntry::Separator => kinds.push("sep".into()),
            PopupEntry::Submenu { title, menu, enabled } => {
                kinds.push(format!("sub:{title}:{enabled}:{}", menu.entries().len()));
            }
            _ => kinds.push("other".into()),
        }
    }
    assert_eq!(kinds, ["item:copy:true", "sep", "item:paste:false", "sub:More:true:1"]);
    rt.resolve_popup(Some("export")).unwrap();
    rt.frame(0.0);
    assert_eq!(rt.state.last_action, "export");
}
