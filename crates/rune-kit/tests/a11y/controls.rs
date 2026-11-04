//! Buttons, toggles and choices: role, name, value, state and actions, by assistive action,
//! pointer and keyboard.

use rune::accesskit::Action;
use rune::{Cx, El, Key, col};
use rune_kit::*;

use crate::support::*;

fn buttons(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    col()
        .gap(8.0)
        .child(Button::new("Save").help("Write the file to disk").shortcut("F5").on_press(|s: &mut App| s.presses += 1).build(&k).key("save"))
        .child(Button::new("Add").icon(k.glyph.copy).variant(ButtonVariant::Ghost).expanded(s.open)
            .on_press(|s: &mut App| s.open = !s.open).build(&k).key("menu"))
        .child(Button::new("Unavailable").disabled(true).on_press(|s: &mut App| s.presses += 100).build(&k).key("off"))
        .child(Button::new("Saving").variant(ButtonVariant::Loading).build(&k).key("busy"))
        .child(IconButton::new(k.glyph.close).label("Close").help("Close the panel").on_press(|s: &mut App| s.presses += 10).build(&k).key("close"))
        .child(IconButton::new(k.glyph.copy).help("Copy").build(&k).key("copy"))
        .child(IconButton::new(k.glyph.copy).label("Copy link").disabled(true).build(&k).key("copy-off"))
}

#[test]
fn buttons_read_their_label_tooltip_shortcut_and_state() {
    let mut rt = mount(App::default(), buttons);
    let tree = rt.accessibility_json();
    let save = node(&tree, "save");
    assert_eq!(said(save), ("button", "Save"));
    assert_eq!(save["description"], "Write the file to disk");
    assert_eq!(save["shortcut"], "F5");
    assert!(can(save, "click") && can(save, "focus"));
    assert!(save["children"].is_null(), "the label's text isn't read again: {save}");
    let menu = node(&tree, "menu");
    assert!(has(menu, "collapsed") && can(menu, "expand"), "a menu trigger reads closed: {menu}");
    let off = node(&tree, "off");
    assert!(has(off, "disabled") && actions(off).is_empty());
    assert!(has(node(&tree, "busy"), "disabled"), "a loading button can't be pressed");
    let close = node(&tree, "close");
    assert_eq!((said(close), close["description"].as_str()), (("button", "Close"), Some("Close the panel")));
    assert_eq!(said(node(&tree, "copy")), ("button", "Copy"), "help names an unlabelled icon button");
    assert!(has(node(&tree, "copy-off"), "disabled"));
    assert!(with_role(&tree, "image").is_empty(), "icons are decoration");
    golden("buttons", &tree);

    assert!(act(&mut rt, "save", Action::Click));
    assert!(act(&mut rt, "menu", Action::Expand));
    assert!(rt.state.open);
    assert!(has(node(&rt.accessibility_json(), "menu"), "expanded"));
    assert!(!act(&mut rt, "off", Action::Click), "a disabled button takes no action");
    tap(&mut rt, "close");
    assert!(act(&mut rt, "save", Action::Focus));
    assert_eq!(focused(&rt.accessibility_json()).as_deref(), Some("save"));
    press_key(&mut rt, Key::Text(" ".into()));
    assert_eq!(rt.state.presses, 12, "assistive click, pointer and Space reach the same handlers");
}

fn toggles(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    col()
        .gap(8.0)
        .child(Checkbox::new(s.on).label("Sync").description("Keep files in step").on_toggle(|s: &mut App| s.on = !s.on).build(&k).key("sync"))
        .child(Checkbox::new(false).label("Some").indeterminate(true).build(&k).key("some"))
        .child(Checkbox::new(true).label("Locked").disabled(true).build(&k).key("locked"))
        .child(Switch::new(s.on).label("Wi-Fi").description("Join known networks").on_toggle(|s: &mut App| s.on = !s.on).build(&k).key("wifi"))
        .child(Switch::new(true).label("Airplane").hide_label().disabled(true).build(&k).key("airplane"))
}

#[test]
fn checkboxes_and_switches_read_checked_and_toggle_by_any_input() {
    let mut rt = mount(App::default(), toggles);
    let tree = rt.accessibility_json();
    let sync = node(&tree, "sync");
    assert_eq!(said(sync), ("check-box", "Sync"));
    assert_eq!(sync["description"], "Keep files in step");
    assert!(has(sync, "unchecked") && can(sync, "click") && can(sync, "focus"));
    assert!(has(node(&tree, "some"), "mixed"));
    let locked = node(&tree, "locked");
    assert!(has(locked, "checked") && has(locked, "disabled") && actions(locked).is_empty());
    let wifi = node(&tree, "wifi");
    assert_eq!((said(wifi), wifi["description"].as_str()), (("switch", "Wi-Fi"), Some("Join known networks")));
    assert!(has(wifi, "unchecked"));
    let airplane = node(&tree, "airplane");
    assert_eq!(said(airplane), ("switch", "Airplane"), "a hidden label still names it");
    assert!(has(airplane, "checked") && has(airplane, "disabled"));
    no_echo(&tree);
    golden("toggles", &tree);

    assert!(act(&mut rt, "sync", Action::Click));
    let tree = rt.accessibility_json();
    assert!(has(node(&tree, "sync"), "checked") && has(node(&tree, "wifi"), "checked"));
    tap(&mut rt, "wifi");
    assert!(!rt.state.on);
    act(&mut rt, "wifi", Action::Focus);
    press_key(&mut rt, Key::Text(" ".into()));
    assert!(rt.state.on && has(node(&rt.accessibility_json(), "wifi"), "checked"));
}

fn radios(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    let pick = |i: usize| move |s: &mut App| s.picked = Some(i);
    col()
        .gap(8.0)
        .child(
            RadioGroup::new()
                .label("Theme")
                .description("How windows look")
                .item(RadioItem::new("Light", s.picked == Some(0)).on_select(pick(0)))
                .item(RadioItem::new("Dark", s.picked == Some(1)).description("Easier at night").on_select(pick(1)))
                .item(RadioItem::new("Auto", s.picked == Some(2)).disabled(true))
                .build(&k)
                .key("theme"),
        )
        .child(
            SegmentedControl::new(&["Day", "Week", "Month"], s.picked)
                .label("Range")
                .segment_state(2, State::Disabled)
                .on_pick(|s: &mut App, i, _| s.picked = Some(i))
                .build(&k)
                .key("range"),
        )
}

#[test]
fn radios_and_segments_read_their_group_place_and_choice() {
    let mut rt = mount(App { picked: Some(0), ..App::default() }, radios);
    let tree = rt.accessibility_json();
    let theme = node(&tree, "theme");
    assert_eq!((said(theme), theme["description"].as_str()), (("radio-group", "Theme"), Some("How windows look")));
    let light = node(theme, "item0");
    assert_eq!(said(light), ("radio-button", "Light"));
    assert!(has(light, "checked") && light["position"] == 1);
    let dark = node(theme, "item1");
    assert!(has(dark, "unchecked") && dark["description"] == "Easier at night");
    assert!(has(node(theme, "item2"), "disabled"));
    let range = node(&tree, "range");
    assert_eq!(said(range), ("radio-group", "Range"));
    let day = node(range, "segment0");
    assert_eq!(said(day), ("radio-button", "Day"));
    assert!(has(day, "checked") && day["position"] == 1 && range["setSize"] == 3);
    assert!(has(node(range, "segment2"), "disabled"));
    no_echo(&tree);
    golden("radios", &tree);

    assert!(act(&mut rt, "item1", Action::Click));
    assert_eq!(rt.state.picked, Some(1));
    let tree = rt.accessibility_json();
    assert!(has(node(&tree, "item1"), "checked") && has(node(&tree, "segment1"), "checked"));
    tap(&mut rt, "segment0");
    assert_eq!(rt.state.picked, Some(0));
    act(&mut rt, "segment0", Action::Focus);
    press_key(&mut rt, Key::Right);
    assert_eq!(rt.state.picked, Some(1), "arrows move the choice");
}

fn status(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    col()
        .gap(8.0)
        .child(Badge::new("Beta").build(&k).key("beta"))
        .child(Divider::horizontal().build(&k))
        .child(Keybind::new(&["Ctrl", "K"]).build(&k).key("keys"))
        .child(Spinner::new().build(&k).key("spinner"))
        .child(Loader::new().build(&k).key("loader"))
        .child(Progress::new(Some(s.level)).label("Upload").show_value().build(&k).key("upload"))
        .child(Progress::new(None).label("Indexing").build(&k).key("indexing"))
        .child(ProgressCircle::new(0.25).label("Sync").build(&k).key("ring"))
        .child(RollingText::new("count", format!("{} files", s.presses)).build(&k))
}

#[test]
fn status_and_progress_read_their_text_and_fraction() {
    let mut rt = mount(App::default(), status);
    let tree = rt.accessibility_json();
    let beta = node(&tree, "beta");
    assert_eq!(said(beta), ("status", "Beta"));
    assert!(has(beta, "live-polite"), "a status badge announces a change");
    assert!(with_role(&tree, "splitter").is_empty(), "a divider is decoration");
    assert_eq!(said(node(&tree, "keys")), ("group", "Ctrl K"));
    assert_eq!(said(node(&tree, "spinner")), ("progress-indicator", "Loading"));
    assert!(node(&tree, "spinner")["children"].is_null(), "the spinner's squares are decoration");
    assert_eq!(said(node(&tree, "loader")), ("progress-indicator", "Loading"));
    let upload = node(&tree, "upload");
    assert_eq!((said(upload), upload["value"].as_str()), (("progress-indicator", "Upload"), Some("40%")));
    assert_eq!(upload["range"][0].as_f64().map(|v| (v * 100.0).round()), Some(40.0));
    assert!(upload["children"].is_null(), "the label and track say what the node does: {upload}");
    let indexing = node(&tree, "indexing");
    assert!(indexing["range"].is_null() && indexing["value"].is_null(), "indeterminate progress has no value");
    assert_eq!(node(&tree, "ring")["value"], "25%");
    let count = node(&tree, "count");
    assert_eq!(said(count), ("label", "0 files"));
    assert!(count["children"].is_null(), "a rolling label reads whole, not a glyph at a time");
    golden("status", &tree);

    rt.update(|s, _| { s.presses = 12; s.level = 1.0; });
    settle(&mut rt);
    let tree = rt.accessibility_json();
    assert_eq!(node(&tree, "count")["name"], "12 files");
    assert_eq!(node(&tree, "upload")["value"], "100%");
}

fn fields(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    col()
        .gap(8.0)
        .child(TextInput::new(&s.edit).placeholder("Search files").clear_button().on_edit(|s: &mut App| &mut s.edit).build(&k).key("search"))
        .child(TextInput::new(&s.edit).label("Email").placeholder("you@example.com").disabled(true).build(&k).key("email"))
        .child(TextArea::new(&s.edit).label("Notes").on_edit(|s: &mut App| &mut s.edit).build(&k).key("notes"))
        .child(TextField::new("Name", &s.edit).error("Name is taken").on_edit(|s: &mut App| &mut s.edit).build(&k).key("name"))
        .child(
            Field::new()
                .label("Token")
                .info()
                .prefix("From your account page")
                .suffix("Kept in the keychain")
                .control(TextInput::new(&s.edit).label("Token").build(&k))
                .build(&k)
                .key("token"),
        )
}

#[test]
fn fields_name_their_editor_and_describe_their_help_and_errors() {
    let mut rt = mount(App::default(), fields);
    let tree = rt.accessibility_json();
    let search = node(node(&tree, "search"), "input");
    assert_eq!(said(search), ("text-input", "Search files"), "the placeholder names an unlabelled field");
    assert!(can(search, "set-value") && can(search, "focus"));
    assert_eq!(said(node(&tree, "action")), ("button", "Clear"));
    let email = node(node(&tree, "email"), "input");
    assert_eq!((said(email), email["placeholder"].as_str()), (("text-input", "Email"), Some("you@example.com")));
    assert!(has(email, "disabled") && !can(email, "set-value"));
    assert_eq!(said(node(node(&tree, "notes"), "input")), ("multiline-text-input", "Notes"));
    let name = node(node(&tree, "name"), "input");
    assert_eq!((said(name), name["description"].as_str()), (("text-input", "Name"), Some("Name is taken")));
    assert!(has(node(&tree, "error"), "live-assertive"), "an error is announced as it appears");
    let token = node(&tree, "token");
    assert_eq!((said(token), token["description"].as_str()), (("group", "Token"), Some("From your account page. Kept in the keychain")));
    golden("fields", &tree);

    assert!(rt.accessibility_action("input", Action::SetValue, Some(rune::accesskit::ActionData::Value("Ada".into()))));
    settle(&mut rt);
    assert_eq!(rt.state.edit.text, "Ada");
    assert_eq!(node(node(&rt.accessibility_json(), "search"), "input")["value"], "Ada");
}

fn recorder(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    col()
        .child(ShortcutRecorder::new("show", &s.recorder, |s: &mut App| &mut s.recorder).label("Show window").build(&k))
        .child(ShortcutRecorder::new("plain", &s.recorder, |s: &mut App| &mut s.recorder).disabled(true).build(&k))
}

#[test]
fn a_shortcut_recorder_reads_its_name_keys_and_recording() {
    let mut rt = mount(App::default(), recorder);
    let tree = rt.accessibility_json();
    let show = node(&tree, "show");
    assert_eq!((said(show), show["value"].as_str()), (("button", "Show window"), Some("Record shortcut")));
    assert!(can(show, "click") && show["children"].is_null());
    let plain = node(&tree, "plain");
    assert_eq!(said(plain), ("button", "Record shortcut"));
    assert!(has(plain, "disabled"));
    golden("shortcut-recorder", &tree);

    assert!(act(&mut rt, "show", Action::Click));
    assert!(rt.state.recorder.is_recording());
    let show = rt.accessibility_json();
    let show = node(&show, "show");
    assert_eq!(show["value"], "Press shortcut");
    assert!(show["description"].as_str().is_some_and(|d| d.contains("Escape")));
}
