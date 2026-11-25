//! Overlays and pickers: dialog, menu, popover, tooltip, select, combobox, popup list and
//! toast read their role, name, value and state, and open, choose and close by assistive
//! action, pointer and keys.

use rune::accesskit::Action;
use rune::{Cx, El, Key, col, text};
use rune_kit::*;

use crate::support::*;

fn toggle(s: &mut App) {
    s.open = !s.open;
}

fn close(s: &mut App) {
    s.open = false;
}

fn dialog(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    let mut layer = Vec::new();
    let behind = Button::new("Delete").on_press(toggle).build(&k).key("delete");
    Dialog::new("confirm", "Delete file?")
        .description("This can't be undone.")
        .open(s.open)
        .on_close(close)
        .fit()
        .child(Button::new("Delete").variant(ButtonVariant::Danger).on_press(|s: &mut App| s.presses += 1).build(&k).key("confirm-delete"))
        .build(&k, &mut layer);
    root(col().child(behind), layer)
}

#[test]
fn a_dialog_is_modal_named_and_described() {
    let mut rt = mount(App::default(), dialog);
    assert!(act(&mut rt, "delete", Action::Click));
    let tree = rt.accessibility_json();
    let panel = node(&tree, "confirm-dialog");
    assert_eq!((said(panel), panel["description"].as_str()), (("dialog", "Delete file?"), Some("This can't be undone.")));
    assert!(has(panel, "modal"));
    assert!(find(&tree, "delete").is_none(), "the page behind a modal leaves the tree");
    assert_eq!(said(node(&tree, "close")), ("button", "Close"));
    assert!(find(&tree, "confirm-scrim").is_none(), "the scrim is decoration");
    golden("dialog", &tree);

    assert!(act(&mut rt, "close", Action::Click));
    assert!(!rt.state.open);
    act(&mut rt, "delete", Action::Click);
    press_key(&mut rt, Key::Escape);
    assert!(!rt.state.open, "Escape dismisses it");
}

#[test]
fn dialog_close_label_and_close_disabled_reach_accessibility_tree_and_web_mirror() {
    #[derive(Default)]
    struct Local { open: bool, saving: bool, label: Option<String>, closes: usize }

    fn view(s: &Local, _: &mut Cx<Local>) -> El {
        let k = kit();
        let mut layer = Vec::new();
        let mut dlg = Dialog::new("save", "Confirmer")
            .description("Enregistrement en cours.")
            .open(s.open)
            .close_disabled(s.saving)
            .on_close(|s: &mut Local| { s.closes += 1; s.open = false; })
            .fit()
            .child(Button::new("Basculer").on_press(|s: &mut Local| s.saving = !s.saving).build(&k).key("toggle-saving"));
        if let Some(label) = &s.label {
            dlg = dlg.close_label(label.clone());
        }
        dlg.build(&k, &mut layer);
        root(col().child(Button::new("Ouvrir").on_press(|s: &mut Local| s.open = true).build(&k).key("open")), layer)
    }

    let mount_local = |state: Local| {
        let mut rt = rune::Runtime::new(state, view, (720.0, 720.0), rune::Env::new(false, 1.0));
        rt.frame(0.0);
        rt.settle_all();
        rt.frame(1.0 / 60.0);
        rt.frame(1.0 / 60.0);
        rt
    };

    // Custom French close_label reaches AccessKit and the web DOM mirror, and still dismisses.
    let mut rt = mount_local(Local { open: true, saving: false, label: Some("Fermer".into()), closes: 0 });
    let tree = rt.accessibility_json();
    assert_eq!(said(node(&tree, "close")), ("button", "Fermer"));
    assert!(!has(node(&tree, "close"), "disabled"));
    assert!(can(node(&tree, "close"), "click"));
    let mirror: Vec<serde_json::Value> = serde_json::from_str(&rt.a11y_json()).unwrap();
    assert!(mirror.iter().any(|v| v["k"] == "button" && v["t"] == "Fermer"), "custom close_label reaches web mirror: {mirror:?}");
    assert!(rt.accessibility_action("close", Action::Click, None));
    rt.frame(0.0);
    assert!(!rt.state.open && rt.state.closes == 1);

    // RTL close_label is preserved verbatim in both trees.
    let rt_rtl = mount_local(Local { open: true, saving: false, label: Some("إغلاق".into()), closes: 0 });
    assert_eq!(said(node(&rt_rtl.accessibility_json(), "close")), ("button", "إغلاق"));
    let mirror_rtl: Vec<serde_json::Value> = serde_json::from_str(&rt_rtl.a11y_json()).unwrap();
    assert!(mirror_rtl.iter().any(|v| v["k"] == "button" && v["t"] == "إغلاق"));

    // Empty and whitespace-only close_label fall back to "Close" so the button is never unnamed.
    for blank in ["", "   "] {
        let rt_blank = mount_local(Local { open: true, saving: false, label: Some(blank.into()), closes: 0 });
        assert_eq!(said(node(&rt_blank.accessibility_json(), "close")), ("button", "Close"));
    }

    // close_disabled(true) keeps the close button present and labelled, marks it disabled with
    // no actions, rejects assistive Click and Escape, and leaves body controls usable.
    let mut rt = mount_local(Local { open: true, saving: true, label: Some("Fermer".into()), closes: 0 });
    let tree = rt.accessibility_json();
    let close_btn = node(&tree, "close");
    assert_eq!(said(close_btn), ("button", "Fermer"));
    assert!(has(close_btn, "disabled"), "close button reports disabled: {close_btn}");
    assert!(!can(close_btn, "click") && !can(close_btn, "focus"), "disabled close button has no actions: {close_btn}");
    assert!(has(node(&tree, "save-dialog"), "modal"));
    assert!(!rt.accessibility_action("close", Action::Click, None));
    rt.frame(0.0);
    assert!(rt.state.open && rt.state.closes == 0);
    rt.key(rune::KeyEvent { key: Key::Escape, mods: rune::Mods::default() });
    rt.frame(0.0);
    assert!(rt.state.open && rt.state.closes == 0, "Escape does not dismiss while close_disabled");
    assert!(rt.accessibility_action("toggle-saving", Action::Click, None));
    rt.frame(0.0);
    assert!(!rt.state.saving);
    assert!(!has(node(&rt.accessibility_json(), "close"), "disabled"));
    rt.key(rune::KeyEvent { key: Key::Escape, mods: rune::Mods::default() });
    rt.frame(0.0);
    assert!(!rt.state.open && rt.state.closes == 1, "re-enabling dismissal restores Escape");
}

#[test]
fn a_dialog_without_on_close_is_modal_and_omits_close_button() {
    fn view(s: &App, _: &mut Cx<App>) -> El {
        let k = kit();
        let mut layer = Vec::new();
        let behind = Button::new("Delete").on_press(toggle).build(&k).key("delete");
        Dialog::new("confirm", "Delete file?")
            .description("This can't be undone.")
            .open(s.open)
            .fit()
            .child(Button::new("Cancel").on_press(close).build(&k).key("cancel"))
            .build(&k, &mut layer);
        root(col().child(behind), layer)
    }

    let mut rt = mount(App::default(), view);
    assert!(act(&mut rt, "delete", Action::Click));
    let tree = rt.accessibility_json();
    let panel = node(&tree, "confirm-dialog");
    assert_eq!((said(panel), panel["description"].as_str()), (("dialog", "Delete file?"), Some("This can't be undone.")));
    assert!(has(panel, "modal"));
    assert!(find(&tree, "delete").is_none(), "the page behind a modal leaves the tree");
    assert!(find(&tree, "close").is_none(), "no close button without on_close");
    assert_eq!(said(node(&tree, "cancel")), ("button", "Cancel"));

    press_key(&mut rt, Key::Escape);
    assert!(rt.state.open, "Escape does not dismiss a dialog without on_close");
    assert!(act(&mut rt, "cancel", Action::Click));
    assert!(!rt.state.open, "dialog's own button closes it");
}

fn menu(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    let mut layer = Vec::new();
    let trigger = Menu::new("file")
        .trigger(Button::new("File").on_press(toggle).build(&k).key("file-button"))
        .item(MenuItem::new("New").shortcut("Ctrl+N").on_select(|s: &mut App| { s.presses += 1; s.open = false; }))
        .item(MenuItem::new("Word wrap").checked(s.on).on_select(|s: &mut App| s.on = !s.on))
        .item(MenuItem::new("Revert").disabled(true))
        .separator()
        .group_label("Share")
        .item(MenuItem::new("Publish").badge("New"))
        .item(MenuItem::new("Export").submenu(vec![MenuEntry::item(MenuItem::new("PDF"))], false))
        .open(s.open)
        .on_close(close)
        .build(&k, &mut layer);
    root(col().child(trigger), layer)
}

#[test]
fn menus_read_items_checks_shortcuts_and_submenus() {
    let mut rt = mount(App::default(), menu);
    let button = node(&rt.accessibility_json(), "file-button").clone();
    assert!(has(&button, "collapsed") && can(&button, "expand"), "a menu button reads closed: {button}");
    assert_eq!(button["hasPopup"], "menu");
    assert!(act(&mut rt, "file-button", Action::Expand));
    let tree = rt.accessibility_json();
    let open_button = node(&tree, "file-button");
    assert!(has(open_button, "expanded") && !has(open_button, "disabled"), "trigger reads expanded while menu is open: {open_button}");
    assert_eq!(open_button["hasPopup"], "menu");
    assert_eq!(open_button["controls"], serde_json::json!(["file-menu"]));
    assert!(actions(open_button).is_empty(), "kept menu trigger is inert to actions: {open_button}");
    assert!(!act(&mut rt, "file-button", Action::Click), "kept menu trigger blocks assistive click");
    let menu_node = node(&tree, "file-menu");
    assert_eq!(said(menu_node).0, "menu");
    assert!(!has(menu_node, "modal"), "popup menu does not announce modal state: {menu_node}");
    let new = node(&tree, "file-item0");
    assert_eq!((said(new), new["shortcut"].as_str()), (("menu-item", "New"), Some("Ctrl+N")));
    assert!(new["children"].is_null(), "the row's label and shortcut aren't read twice");
    let wrap = node(&tree, "file-item1");
    assert_eq!(said(wrap), ("menu-item-check-box", "Word wrap"));
    assert!(has(wrap, "unchecked"));
    assert!(has(node(&tree, "file-item2"), "disabled"));
    assert_eq!(node(&tree, "file-item5")["description"], "New");
    let export = node(&tree, "file-item6");
    assert!(has(export, "collapsed"), "a submenu reads closed: {export}");
    assert!(with_role(&tree, "splitter").is_empty() && with_role(&tree, "image").is_empty());
    golden("menu", &tree);

    assert!(act(&mut rt, "file-item1", Action::Click));
    assert!(has(node(&rt.accessibility_json(), "file-item1"), "checked"));
    act(&mut rt, "file-item0", Action::Focus);
    press_key(&mut rt, Key::Enter);
    assert_eq!((rt.state.presses, rt.state.open), (1, false), "Return runs the item and the menu closes");
    assert!(has(node(&rt.accessibility_json(), "file-button"), "collapsed"));
}

fn popover(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    let mut layer = Vec::new();
    let share = Popover::new("share")
        .trigger(Button::new("Share").on_press(toggle).build(&k).key("share-button"))
        .title("Share")
        .description("Anyone with the link can view")
        .child(text("example.com/f/1").key("link"))
        .open(s.open)
        .on_close(close)
        .build(&k, &mut layer);
    let tip = Tooltip::new("tip").trigger(Button::new("Save").build(&k).key("save")).label("Saves to disk").open(true).build(&k, &mut layer);
    root(col().gap(8.0).child(share).child(tip), layer)
}

#[test]
fn popovers_and_tooltips_name_describe_and_open() {
    let mut rt = mount(App::default(), popover);
    let tree = rt.accessibility_json();
    let tip = node(&tree, "tip-tooltip");
    assert_eq!(said(tip), ("tooltip", "Saves to disk"));
    assert_eq!(node(&tree, "save")["description"], "Saves to disk", "a showing tooltip describes its trigger");
    assert!(with_role(&tree, "button").iter().all(|b| b["children"].is_null()), "no button wraps another");
    tap(&mut rt, "share-button");
    assert!(rt.state.open);
    let tree = rt.accessibility_json();
    assert!(has(node(&tree, "share-button"), "expanded"));
    let panel = node(&tree, "share-content");
    assert_eq!((said(panel), panel["description"].as_str()), (("dialog", "Share"), Some("Anyone with the link can view")));
    assert_eq!(said(node(panel, "link")), ("label", "example.com/f/1"));
    golden("popover", &tree);
    assert!(act(&mut rt, "close", Action::Click));
    assert!(!rt.state.open);
}

const FRUIT: [&str; 3] = ["Apple", "Pear", "Plum"];

fn select(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    let mut layer = Vec::new();
    let fruit = Select::new("fruit", &FRUIT)
        .label("Fruit")
        .placeholder("Choose")
        .current(s.chosen.as_deref())
        .open(s.open)
        .on_toggle(toggle)
        .on_close(close)
        .on_choose(|s: &mut App, v: &str, _| { s.chosen = Some(v.to_owned()); s.open = false; })
        .build(&k, &mut layer);
    let off = Select::new("size", &FRUIT).placeholder("Size").disabled(true).build(&k, &mut layer);
    root(col().gap(8.0).child(fruit).child(off), layer)
}

#[test]
fn a_select_reads_its_name_choice_and_options() {
    let mut rt = mount(App { chosen: Some("Pear".into()), ..App::default() }, select);
    let tree = rt.accessibility_json();
    let field = node(&tree, "fruit-trigger");
    assert_eq!((said(field), field["value"].as_str()), (("combo-box", "Fruit"), Some("Pear")));
    assert!(has(field, "collapsed") && can(field, "expand") && field["children"].is_null());
    let off = node(&tree, "size-trigger");
    assert_eq!(said(off), ("combo-box", "Size"));
    assert!(has(off, "disabled"));
    golden("select", &tree);

    assert!(act(&mut rt, "fruit-trigger", Action::Expand));
    let tree = rt.accessibility_json();
    let open_trigger = node(&tree, "fruit-trigger");
    assert!(has(open_trigger, "expanded") && !has(open_trigger, "disabled"), "select trigger reads expanded while listbox is open: {open_trigger}");
    assert_eq!(open_trigger["hasPopup"], "listbox");
    assert_eq!(open_trigger["controls"], serde_json::json!(["fruit-content"]));
    assert!(actions(open_trigger).is_empty(), "kept select trigger is inert to actions: {open_trigger}");
    assert!(!act(&mut rt, "fruit-trigger", Action::Click), "kept select trigger blocks assistive click");
    assert!(find(&tree, "size-trigger").is_none(), "unrelated select behind open listbox stays pruned");
    let listbox = node(&tree, "fruit-content");
    assert_eq!(said(listbox), ("list-box", "Fruit"));
    assert!(!has(listbox, "modal"), "select listbox does not announce modal state: {listbox}");
    let pear = node(&tree, "option1");
    assert_eq!(said(pear), ("list-box-option", "Pear"));
    assert!(has(pear, "selected") && pear["position"] == 2);
    assert!(has(node(&tree, "option0"), "unselected"));
    golden("select-open", &tree);

    assert!(act(&mut rt, "option2", Action::Click));
    assert_eq!((rt.state.chosen.as_deref(), rt.state.open), (Some("Plum"), false));
    assert_eq!(node(&rt.accessibility_json(), "fruit-trigger")["value"], "Plum");
}

#[test]
fn a_select_exposes_its_selected_value_in_the_web_mirror() {
    fn view(s: &App, _: &mut Cx<App>) -> El {
        let mut layers = Vec::new();
        Select::new("preset", &["Nothing", "Balanced", "Everything"])
            .label("Message content preset")
            .current(s.chosen.as_deref())
            .build(&kit(), &mut layers)
    }
    let mut rt = mount(App { chosen: Some("Balanced".into()), ..App::default() }, view);
    let items: Vec<serde_json::Value> = serde_json::from_str(&rt.a11y_json()).unwrap();
    let combo = items.iter().find(|v| v["t"] == "Message content preset").expect("select mirrored");
    assert_eq!((combo.get("o"), combo.get("x"), combo.get("v")),
        (Some(&serde_json::json!("combobox")), Some(&serde_json::json!(false)), Some(&serde_json::json!("Balanced"))));

    rt.update(|s, _| s.chosen = Some("Everything".into()));
    rt.frame(0.0);
    let items: Vec<serde_json::Value> = serde_json::from_str(&rt.a11y_json()).unwrap();
    let combo = items.iter().find(|v| v["t"] == "Message content preset").expect("select mirrored");
    assert_eq!(combo.get("v"), Some(&serde_json::json!("Everything")));
}

fn combobox(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    let mut layer = Vec::new();
    let mut late = ComboboxOption::new("late", "Retired model");
    late.disabled = true;
    let options = [ComboboxOption::new("fast", "Fast"), ComboboxOption::new("smart", "Smart"), late];
    let picker = Combobox::new("model", &s.combo, &options, |s: &mut App| &mut s.combo)
        .label("Model")
        .placeholder("Pick a model")
        .current(s.chosen.as_deref())
        .on_choose(|s: &mut App, id, _| s.chosen = Some(id.to_owned()))
        .build(&k, &mut layer);
    root(col().child(picker), layer)
}

#[test]
fn a_combobox_reads_its_choice_search_and_results() {
    let mut rt = mount(App { chosen: Some("fast".into()), ..App::default() }, combobox);
    let tree = rt.accessibility_json();
    let field = node(&tree, "model-value-trigger");
    assert_eq!((said(field), field["value"].as_str()), (("combo-box", "Model"), Some("Fast")));
    assert!(has(field, "collapsed"));
    assert!(act(&mut rt, "model-value-trigger", Action::Expand));
    let tree = rt.accessibility_json();
    assert!(has(node(&tree, "model-value-trigger"), "expanded"));
    let search = node(node(&tree, "search"), "input");
    assert_eq!(said(search), ("text-input", "Search Model"));
    assert!(has(search, "focused"), "the search field takes focus");
    let results = node(&tree, "viewport");
    assert_eq!(said(results), ("list-box", "Model"));
    assert_eq!(results["children"][0]["setSize"], 3, "the list says how many results it has: {results}");
    let fast = node(&tree, "model-option-fast");
    assert!(has(fast, "selected") && fast["position"] == 1);
    assert!(has(node(&tree, "model-option-late"), "disabled"));
    golden("combobox-open", &tree);

    press_key(&mut rt, Key::Down);
    let tree = rt.accessibility_json();
    assert!(has(node(&tree, "model-option-smart"), "selected"), "arrows move the active result");
    assert_eq!(node(node(&tree, "search"), "input")["activeDescendant"], "model-option-smart", "focus stays in the search field");
    press_key(&mut rt, Key::Enter);
    assert_eq!((rt.state.chosen.as_deref(), rt.state.combo.is_open()), (Some("smart"), false));
}

#[test]
fn combobox_search_label_overrides_default_and_handles_edges() {
    struct Local {
        combo: ComboboxState,
        outer_label: Option<String>,
        search_placeholder: Option<String>,
        search_label: Option<String>,
    }

    fn view(s: &Local, _: &mut Cx<Local>) -> El {
        let k = kit();
        let mut layer = Vec::new();
        let options = [
            ComboboxOption::new("fast", "Rapide"),
            ComboboxOption::new("smart", "Précis"),
        ];
        let mut picker = Combobox::new("models", &s.combo, &options, |s: &mut Local| &mut s.combo);
        if let Some(label) = &s.outer_label { picker = picker.label(label.clone()); }
        if let Some(ph) = &s.search_placeholder { picker = picker.search_placeholder(ph.clone()); }
        if let Some(sl) = &s.search_label { picker = picker.search_label(sl.clone()); }
        let trigger = picker.build(&k, &mut layer);
        root(col().child(trigger), layer)
    }

    let open_tree = |outer_label: Option<&str>, search_placeholder: Option<&str>, search_label: Option<&str>| {
        let mut combo = ComboboxState::default();
        combo.step(ComboboxEvent::Toggle { current: None });
        let mut rt = rune::Runtime::new(
            Local {
                combo,
                outer_label: outer_label.map(str::to_owned),
                search_placeholder: search_placeholder.map(str::to_owned),
                search_label: search_label.map(str::to_owned),
            },
            view,
            (720.0, 720.0),
            rune::Env::new(false, 1.0),
        );
        rt.frame(0.0);
        rt.settle_all();
        rt.frame(1.0 / 60.0);
        rt.accessibility_json()
    };

    // Custom French search_label replaces the English "Search <label>" prefix independently of
    // outer label and search_placeholder.
    let tree = open_tree(Some("Modèle : Fixture"), Some("Rechercher des modèles..."), Some("Rechercher des modèles"));
    let search = node(node(&tree, "search"), "input");
    assert_eq!(said(search), ("text-input", "Rechercher des modèles"));
    assert_eq!(search["placeholder"].as_str(), Some("Rechercher des modèles..."));
    assert_eq!(said(node(&tree, "models-value-trigger")), ("combo-box", "Modèle : Fixture"));

    // RTL search_label is preserved verbatim.
    let tree_rtl = open_tree(Some("النموذج"), Some("ابحث..."), Some("البحث عن النماذج"));
    assert_eq!(said(node(node(&tree_rtl, "search"), "input")), ("text-input", "البحث عن النماذج"));

    // Blank search_label falls back to the existing default ("Search <label>" or "Search").
    for blank in ["", "   "] {
        let with_outer = open_tree(Some("Model"), None, Some(blank));
        assert_eq!(said(node(node(&with_outer, "search"), "input")), ("text-input", "Search Model"));
        let without_outer = open_tree(None, None, Some(blank));
        assert_eq!(said(node(node(&without_outer, "search"), "input")), ("text-input", "Search"));
    }

    // Unset search_label without outer label or placeholder keeps the exact "Search" fallback.
    let bare = open_tree(None, None, None);
    assert_eq!(said(node(node(&bare, "search"), "input")), ("text-input", "Search"));
}

fn toasts(_: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    let region = ToastRegion::new()
        .toast(
            Toast::new("saved", "Saved")
                .description("3 files written")
                .icon(&k, k.glyph.check)
                .action(&k, "Undo", true, |s: &mut App, _| s.presses += 1)
                .on_close(|s: &mut App| s.presses += 10)
                .build(&k),
        )
        .build(&k);
    root(col(), [region])
}

#[test]
fn toasts_announce_politely_and_their_buttons_reach_the_keyboard() {
    let mut rt = mount(App::default(), toasts);
    let tree = rt.accessibility_json();
    assert_eq!(said(node(&tree, "toast-region")), ("region", "Notifications"));
    let toast = node(&tree, "saved");
    assert_eq!((said(toast), toast["description"].as_str()), (("status", "Saved"), Some("3 files written")));
    assert!(has(toast, "live-polite"));
    let close = node(toast, "close");
    assert_eq!(said(close), ("button", "Close"));
    assert!(can(close, "focus"), "the close button is a Tab stop");
    assert_eq!(named(toast, "Undo")[0]["role"], "button");
    no_echo(&tree);
    golden("toast", &tree);
    assert!(act(&mut rt, "close", Action::Click));
    press_key(&mut rt, Key::Tab);
    press_key(&mut rt, Key::Tab);
    press_key(&mut rt, Key::Text(" ".into()));
    assert_eq!(rt.state.presses, 11, "Tab reaches Undo, Space presses it");
}

#[test]
fn hover_card_role_and_label_reach_accessibility_tree_and_web_mirror() {
    struct Local {
        role: Option<&'static str>,
        label: Option<String>,
    }
    fn view(s: &Local, _: &mut Cx<Local>) -> El {
        let k = kit();
        let mut layer = Vec::new();
        let mut card = HoverCard::new("preview")
            .trigger(Button::new("readme.md").build(&k).key("trigger-btn"))
            .child(text("File metadata").key("meta"))
            .open(true);
        if let Some(r) = s.role {
            card = card.role(r);
        }
        if let Some(l) = &s.label {
            card = card.label(l.clone());
        }
        let trigger = card.build(&k, &mut layer);
        root(col().padding(40.0).child(trigger), layer)
    }

    let mount_local = |role: Option<&'static str>, label: Option<&str>| {
        let mut rt = rune::Runtime::new(
            Local { role, label: label.map(str::to_owned) },
            view,
            (720.0, 480.0),
            rune::Env::new(false, 1.0),
        );
        rt.frame(0.0);
        rt.settle_all();
        rt.frame(1.0 / 60.0);
        rt
    };

    // Default without role or label keeps group role and unnamed surface.
    let rt_default = mount_local(None, None);
    let tree = rt_default.accessibility_json();
    assert_eq!(said(node(&tree, "preview-card")), ("group", ""));

    // Explicit role("dialog") and label reach AccessKit and web DOM mirror.
    let rt_dialog = mount_local(Some("dialog"), Some("File preview"));
    let tree = rt_dialog.accessibility_json();
    assert_eq!(said(node(&tree, "preview-card")), ("dialog", "File preview"));
    let mirror: Vec<serde_json::Value> = serde_json::from_str(&rt_dialog.a11y_json()).unwrap();
    assert!(
        mirror.iter().any(|v| v["o"] == "dialog" && v["t"] == "File preview"),
        "HoverCard role and label reach web mirror: {mirror:?}",
    );

    // RTL label and region role are preserved verbatim in both trees.
    let rt_rtl = mount_local(Some("region"), Some("معاينة الملف"));
    let tree = rt_rtl.accessibility_json();
    assert_eq!(said(node(&tree, "preview-card")), ("region", "معاينة الملف"));
    let mirror: Vec<serde_json::Value> = serde_json::from_str(&rt_rtl.a11y_json()).unwrap();
    assert!(
        mirror.iter().any(|v| v["g"] == "section" && v["t"] == "معاينة الملف"),
        "RTL HoverCard region label reaches web mirror: {mirror:?}",
    );

    // Blank role or label falls back cleanly to default group / unnamed.
    let rt_blank = mount_local(Some("   "), Some("   "));
    assert_eq!(said(node(&rt_blank.accessibility_json(), "preview-card")), ("group", ""));
}

#[test]
fn popover_and_select_tx_callbacks_fire_from_assistive_actions() {
    #[derive(Default)]
    struct Local {
        popover_open: bool,
        select_open: bool,
        closes: usize,
        toggles: usize,
    }
    fn view(s: &Local, _: &mut Cx<Local>) -> El {
        let k = kit();
        let mut layer = Vec::new();
        let pop = Popover::new("share")
            .trigger(Button::new("Share").build(&k).key("share-btn"))
            .title("Share")
            .child(text("Link"))
            .open(s.popover_open)
            .on_close_tx(|s: &mut Local, tx: &mut rune::Tx| {
                s.popover_open = false;
                s.closes += 1;
                tx.focus("after", true);
            })
            .build(&k, &mut layer);
        let sel = Select::new("fruit", &FRUIT)
            .label("Fruit")
            .open(s.select_open)
            .on_toggle_tx(|s: &mut Local, tx: &mut rune::Tx| {
                s.select_open = !s.select_open;
                s.toggles += 1;
                tx.instant();
            })
            .on_close_tx(|s: &mut Local, tx: &mut rune::Tx| {
                s.select_open = false;
                s.closes += 1;
                tx.focus("after", true);
            })
            .build(&k, &mut layer);
        let after = Button::new("After").build(&k).key("after");
        root(col().gap(8.0).child(pop).child(sel).child(after), layer)
    }

    let mut rt = rune::Runtime::new(
        Local { popover_open: true, ..Local::default() },
        view,
        (720.0, 480.0),
        rune::Env::new(false, 1.0),
    );
    rt.frame(0.0);
    rt.settle_all();
    rt.frame(1.0 / 60.0);

    // Assistive Click on Popover close button runs on_close_tx and focuses "after".
    assert!(rt.accessibility_action("close", Action::Click, None));
    rt.frame(0.0);
    assert!(!rt.state.popover_open && rt.state.closes == 1);
    assert!(has(node(&rt.accessibility_json(), "after"), "focused"));

    // Assistive Expand on Select trigger runs on_toggle_tx.
    assert!(rt.accessibility_action("fruit-trigger", Action::Expand, None));
    rt.frame(0.0);
    assert!(rt.state.select_open && rt.state.toggles == 1);
    assert!(find(&rt.accessibility_json(), "fruit-content").is_some());
}

#[test]
fn step_closed_select_updates_a11y_value_and_web_mirror_while_collapsed() {
    fn view(s: &App, _: &mut Cx<App>) -> El {
        let mut layer = Vec::new();
        let sel = Select::new("fruit", &FRUIT)
            .label("Fruit")
            .current(s.chosen.as_deref())
            .open(s.open)
            .step_closed(true)
            .on_toggle(toggle)
            .on_close(close)
            .on_choose(|s: &mut App, v: &str, _| s.chosen = Some(v.to_owned()))
            .build(&kit(), &mut layer);
        root(col().child(sel), layer)
    }
    let mut rt = mount(App { chosen: Some("Pear".into()), ..App::default() }, view);
    rt.key(rune::KeyEvent { key: rune::Key::Tab, mods: rune::Mods::default() });
    rt.frame(0.0);
    rt.key(rune::KeyEvent { key: rune::Key::Down, mods: rune::Mods::default() });
    rt.frame(0.0);

    // AccessKit tree: still collapsed, focused on trigger, value updated to Plum.
    let tree = rt.accessibility_json();
    let field = node(&tree, "fruit-trigger");
    assert_eq!((said(field), field["value"].as_str()), (("combo-box", "Fruit"), Some("Plum")));
    assert!(has(field, "collapsed") && has(field, "focused"));
    assert!(find(&tree, "fruit-content").is_none());

    // Web mirror reflects the updated value while staying collapsed.
    let items: Vec<serde_json::Value> = serde_json::from_str(&rt.a11y_json()).unwrap();
    let combo = items.iter().find(|v| v["t"] == "Fruit").expect("select mirrored");
    assert_eq!(
        (combo.get("o"), combo.get("x"), combo.get("v")),
        (Some(&serde_json::json!("combobox")), Some(&serde_json::json!(false)), Some(&serde_json::json!("Plum")))
    );
}

#[test]
fn menu_label_and_item_a11y_label_reach_accessibility_tree_and_web_mirror() {
    struct Local {
        menu_label: Option<String>,
        item_a11y: Option<String>,
        sub_open: bool,
    }
    fn view(s: &Local, _: &mut Cx<Local>) -> El {
        let k = kit();
        let mut layer = Vec::new();
        let mut path_item = MenuItem::new("…/menu.rs")
            .key("menu-rs")
            .on_select(|_: &mut Local| {});
        if let Some(l) = &s.item_a11y {
            path_item = path_item.a11y_label(l.clone());
        }
        let sub_item = MenuItem::new("Export…")
            .key("export")
            .a11y_label("Export document")
            .submenu(
                vec![MenuEntry::item(MenuItem::new("PDF").key("export-pdf").on_select(|_: &mut Local| {}))],
                s.sub_open,
            )
            .submenu_placement(Placement::LeftStart)
            .submenu_min_width(0.0)
            .on_select(|_: &mut Local| {});
        let mut m = Menu::new("file")
            .trigger(Button::new("File").build(&k).key("file-btn"))
            .item(path_item)
            .item(sub_item)
            .item(MenuItem::new("Disabled").key("off").a11y_label("Disabled full action").disabled(true))
            .open(true);
        if let Some(l) = &s.menu_label {
            m = m.label(l.clone());
        }
        let trigger = m.build(&k, &mut layer);
        root(col().padding(40.0).child(trigger), layer)
    }

    let mount_local = |menu_label: Option<&str>, item_a11y: Option<&str>, sub_open: bool| {
        let mut rt = rune::Runtime::new(
            Local {
                menu_label: menu_label.map(str::to_owned),
                item_a11y: item_a11y.map(str::to_owned),
                sub_open,
            },
            view,
            (720.0, 480.0),
            rune::Env::new(false, 1.0),
        );
        rt.frame(0.0);
        rt.settle_all();
        rt.frame(1.0 / 60.0);
        rt
    };

    // Menu::label, MenuItem::a11y_label, and submenu menu name reach AccessKit and web mirror.
    let rt = mount_local(Some("File actions"), Some("crates/rune-kit/src/overlays/menu.rs"), true);
    let tree = rt.accessibility_json();
    assert_eq!(said(node(&tree, "file-menu")), ("menu", "File actions"));
    let item = node(&tree, "menu-rs");
    assert_eq!(said(item), ("menu-item", "crates/rune-kit/src/overlays/menu.rs"));
    assert!(item["children"].is_null(), "drawn elided text is not read twice");
    let off = node(&tree, "off");
    assert_eq!(said(off), ("menu-item", "Disabled full action"));
    assert!(has(off, "disabled"));
    assert_eq!(said(node(&tree, "export-sub-menu")), ("menu", "Export document"));
    assert_eq!(said(node(&tree, "export-pdf")), ("menu-item", "PDF"));

    let mirror: Vec<serde_json::Value> = serde_json::from_str(&rt.a11y_json()).unwrap();
    assert!(
        mirror.iter().any(|v| v["o"] == "menu" && v["t"] == "File actions"),
        "Menu::label reaches web mirror: {mirror:?}",
    );
    assert!(
        mirror.iter().any(|v| v["o"] == "menu" && v["t"] == "Export document"),
        "submenu menu label reaches web mirror: {mirror:?}",
    );
    assert!(
        mirror.iter().any(|v| v["o"] == "menuitem" && v["t"] == "crates/rune-kit/src/overlays/menu.rs"),
        "MenuItem::a11y_label reaches web mirror: {mirror:?}",
    );
    assert!(
        !mirror.iter().any(|v| v["t"] == "…/menu.rs"),
        "drawn truncated text does not echo in web mirror: {mirror:?}",
    );

    // RTL labels are preserved verbatim in both trees.
    let rt_rtl = mount_local(Some("إجراءات الملف"), Some("حفظ المستند"), false);
    let tree_rtl = rt_rtl.accessibility_json();
    assert_eq!(said(node(&tree_rtl, "file-menu")), ("menu", "إجراءات الملف"));
    assert_eq!(said(node(&tree_rtl, "menu-rs")), ("menu-item", "حفظ المستند"));
    let mirror_rtl: Vec<serde_json::Value> = serde_json::from_str(&rt_rtl.a11y_json()).unwrap();
    assert!(mirror_rtl.iter().any(|v| v["o"] == "menu" && v["t"] == "إجراءات الملف"));
    assert!(mirror_rtl.iter().any(|v| v["o"] == "menuitem" && v["t"] == "حفظ المستند"));

    // Blank menu label stays unnamed and blank item a11y_label falls back to drawn text.
    let rt_blank = mount_local(Some("   "), Some("   "), false);
    let tree_blank = rt_blank.accessibility_json();
    assert_eq!(said(node(&tree_blank, "file-menu")), ("menu", ""));
    assert_eq!(said(node(&tree_blank, "menu-rs")), ("menu-item", "…/menu.rs"));
}

#[test]
fn popup_item_label_with_and_build_list_reach_accessibility_tree_and_web_mirror() {
    fn view(_: &App, _: &mut Cx<App>) -> El {
        let k = kit();
        let rich = rune::row()
            .child(text("/rev").font(k.ty.ui_medium))
            .child(text("iew").font(k.ty.ui_compact));
        let rtl = rune::row()
            .child(text("بحث").font(k.ty.ui_medium))
            .child(text(" الملفات").font(k.ty.ui_compact));
        let list = PopupList::<App>::new("completions", "")
            .label("Command completions")
            .items([
                PopupItem::new("review", "/review").label_with(rich).description("Review branch"),
                PopupItem::new("rtl", "بحث الملفات").label_with(rtl),
                PopupItem::new("off", "/locked").label_with(text("/locked")).disabled(true),
            ])
            .active(Some(0))
            .on_choose(|s: &mut App, _, _| s.presses += 1)
            .build_list(&k);
        col().w(300.0).padding(8.0).child(list)
    }

    let rt = mount(App::default(), view);
    let tree = rt.accessibility_json();
    let listbox = node(&tree, "viewport");
    assert_eq!(said(listbox), ("list-box", "Command completions"));
    let review = node(&tree, "completions-option-review");
    assert_eq!((said(review), review["description"].as_str()), (("list-box-option", "/review"), Some("Review branch")));
    assert!(review["children"].is_null(), "label_with spans stay hidden so option is not read twice");
    assert!(has(review, "selected"));
    let rtl = node(&tree, "completions-option-rtl");
    assert_eq!(said(rtl), ("list-box-option", "بحث الملفات"));
    assert!(rtl["children"].is_null());
    let off = node(&tree, "completions-option-off");
    assert_eq!(said(off), ("list-box-option", "/locked"));
    assert!(has(off, "disabled"));

    let mirror: Vec<serde_json::Value> = serde_json::from_str(&rt.a11y_json()).unwrap();
    assert!(
        mirror.iter().any(|v| v["o"] == "listbox" && v["t"] == "Command completions"),
        "standalone build_list listbox reaches web mirror: {mirror:?}",
    );
    assert!(
        mirror.iter().any(|v| v["o"] == "option" && v["t"] == "/review" && v["se"] == true),
        "rich option reaches web mirror with plain label: {mirror:?}",
    );
    assert!(
        mirror.iter().any(|v| v["o"] == "option" && v["t"] == "بحث الملفات"),
        "RTL rich option reaches web mirror: {mirror:?}",
    );
    assert!(
        !mirror.iter().any(|v| v["t"] == "/rev" || v["t"] == "iew"),
        "split label_with spans do not echo in web mirror: {mirror:?}",
    );
}

#[test]
fn select_inside_dialog_keeps_dialog_and_expanded_trigger_readable() {
    #[derive(Default)]
    struct Local {
        dialog: bool,
        select: bool,
        chosen: Option<String>,
    }

    fn view(s: &Local, _: &mut Cx<Local>) -> El {
        let k = kit();
        let mut layer = Vec::new();
        let mut select_layer = Vec::new();
        let select = Select::new("fruit", &FRUIT)
            .label("Fruit")
            .current(s.chosen.as_deref())
            .open(s.select)
            .on_toggle(|s: &mut Local| s.select = !s.select)
            .on_close(|s: &mut Local| s.select = false)
            .on_choose(|s: &mut Local, v: &str, _| {
                s.chosen = Some(v.to_owned());
                s.select = false;
            })
            .build(&k, &mut select_layer);
        Dialog::new("settings", "Settings")
            .open(s.dialog)
            .on_close(|s: &mut Local| s.dialog = false)
            .child(select)
            .build(&k, &mut layer);
        // Mount the select's popup after the dialog so the listbox is the topmost active modal.
        layer.extend(select_layer);
        root(col().child(Button::new("Page").build(&k).key("page-btn")), layer)
    }

    let mut rt = rune::Runtime::new(
        Local { dialog: true, select: true, chosen: Some("Pear".into()) },
        view,
        (720.0, 480.0),
        rune::Env::new(false, 1.0),
    );
    rt.frame(0.0);
    rt.settle_all();
    rt.frame(1.0 / 60.0);

    let tree = rt.accessibility_json();
    let dlg = node(&tree, "settings-dialog");
    assert_eq!(said(dlg), ("dialog", "Settings"));
    assert!(has(dlg, "modal"));
    let trig = node(dlg, "fruit-trigger");
    assert_eq!(said(trig), ("combo-box", "Fruit"));
    assert!(has(trig, "expanded") && !has(trig, "disabled"));
    assert_eq!(trig["hasPopup"], "listbox");
    assert_eq!(trig["controls"], serde_json::json!(["fruit-content"]));
    assert!(actions(trig).is_empty(), "select trigger inside dialog is inert while listbox is open: {trig}");
    assert!(find(&tree, "page-btn").is_none(), "page behind outer dialog stays pruned");
    let list = node(&tree, "fruit-content");
    assert_eq!(said(list), ("list-box", "Fruit"));
    assert!(!has(list, "modal"));

    let mirror: Vec<serde_json::Value> = serde_json::from_str(&rt.a11y_json()).unwrap();
    let dlg_i = mirror
        .iter()
        .position(|v| v["k"] == "group" && v["o"] == "dialog" && v["t"] == "Settings")
        .unwrap_or_else(|| panic!("outer dialog missing in web mirror: {mirror:?}"));
    let m_trig = mirror
        .iter()
        .find(|v| v["o"] == "combobox" && v["t"] == "Fruit")
        .unwrap_or_else(|| panic!("select trigger missing in web mirror: {mirror:?}"));
    assert_eq!(m_trig["a"], dlg_i, "select trigger sits inside outer dialog in web mirror: {mirror:?}");
    assert_eq!(m_trig["x"], true);
    assert_eq!(m_trig["hp"], "listbox");
    assert_eq!(m_trig["ct"], "fruit-content");
    assert!(mirror.iter().any(|v| v["o"] == "listbox" && v["i"] == "fruit-content"));
    assert!(!mirror.iter().any(|v| v["t"] == "Page"));
}
