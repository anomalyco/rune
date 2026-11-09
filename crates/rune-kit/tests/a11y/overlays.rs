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
    assert!(act(&mut rt, "file-button", Action::Expand));
    let tree = rt.accessibility_json();
    assert_eq!(said(node(&tree, "file-menu")).0, "menu");
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
    assert!(find(&tree, "fruit-trigger").is_none(), "the open listbox is modal: the trigger waits behind it");
    assert_eq!(said(node(&tree, "fruit-content")), ("dialog", "Fruit"));
    let listbox = node(&tree, "listbox");
    assert_eq!(said(listbox), ("list-box", "Fruit"));
    let pear = node(&tree, "option1");
    assert_eq!(said(pear), ("list-box-option", "Pear"));
    assert!(has(pear, "selected") && pear["position"] == 2);
    assert!(has(node(&tree, "option0"), "unselected"));
    golden("select-open", &tree);

    assert!(act(&mut rt, "option2", Action::Click));
    assert_eq!((rt.state.chosen.as_deref(), rt.state.open), (Some("Plum"), false));
    assert_eq!(node(&rt.accessibility_json(), "fruit-trigger")["value"], "Plum");
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
