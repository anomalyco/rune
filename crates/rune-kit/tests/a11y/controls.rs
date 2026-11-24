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

#[test]
fn button_with_show_shortcut_keeps_accessibility_tree_clean() {
    fn view(_: &App, _: &mut Cx<App>) -> El {
        let k = kit();
        col()
            .gap(8.0)
            .child(
                Button::new("Save")
                    .help("Write the file to disk")
                    .shortcut("F5")
                    .show_shortcut()
                    .on_press(|s: &mut App| s.presses += 1)
                    .build(&k)
                    .key("save-inline"),
            )
            .child(
                Button::new("Commit")
                    .shortcut("cmd+s")
                    .show_shortcut()
                    .build(&k)
                    .key("commit-inline"),
            )
            .child(
                Button::new("Unavailable")
                    .shortcut("F5")
                    .show_shortcut()
                    .disabled(true)
                    .on_press(|s: &mut App| s.presses += 100)
                    .build(&k)
                    .key("off-inline"),
            )
    }

    let mut rt = mount(App::default(), view);
    let tree = rt.accessibility_json();

    let save = node(&tree, "save-inline");
    assert_eq!(said(save), ("button", "Save"));
    assert_eq!(save["description"], "Write the file to disk");
    assert_eq!(save["shortcut"], "F5");
    assert!(can(save, "click") && can(save, "focus"));
    assert!(save["children"].is_null(), "inline shortcut is hidden from assistive technology: {save}");

    let expected_cmd_s = if cfg!(target_os = "macos") { "⌘S" } else { "Super+S" };
    let commit = node(&tree, "commit-inline");
    assert_eq!(said(commit), ("button", "Commit"));
    assert_eq!(commit["shortcut"], expected_cmd_s);
    assert!(commit["children"].is_null(), "inline accelerator is hidden from assistive technology: {commit}");

    let off = node(&tree, "off-inline");
    assert_eq!(said(off), ("button", "Unavailable"));
    assert_eq!(off["shortcut"], "F5");
    assert!(has(off, "disabled") && actions(off).is_empty());
    assert!(off["children"].is_null());

    assert!(act(&mut rt, "save-inline", Action::Click));
    tap(&mut rt, "save-inline");
    assert!(act(&mut rt, "save-inline", Action::Focus));
    press_key(&mut rt, Key::Text(" ".into()));
    assert!(!act(&mut rt, "off-inline", Action::Click));
    assert_eq!(rt.state.presses, 3);
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

fn sliders(s: &App, _: &mut Cx<App>) -> El {
    let k = kit();
    col()
        .gap(12.0)
        .child(
            Slider::new("volume", s.level * 100.0)
                .range(0.0..=100.0)
                .step(5.0)
                .label("Volume")
                .value_text(format!("{:.0}%", s.level * 100.0))
                .on_change(|s: &mut App, v, _| s.level = v / 100.0)
                .build(&k),
        )
        .child(
            Slider::new("speed", s.level)
                .label("Speed")
                .hide_label()
                .disabled(true)
                .build(&k),
        )
}

#[test]
fn sliders_read_range_value_and_step_by_any_input() {
    use rune::accesskit::ActionData;
    let mut rt = mount(App::default(), sliders);
    let tree = rt.accessibility_json();
    let vol = node(&tree, "volume");
    assert_eq!((said(vol), vol["value"].as_str()), (("slider", "Volume"), Some("40%")));
    assert_eq!(vol["range"], serde_json::json!([40.0, 0.0, 100.0, 5.0]));
    assert!(can(vol, "focus") && can(vol, "increment") && can(vol, "decrement") && can(vol, "set-value"));
    assert!(vol["children"].is_null(), "decorative slider parts stay hidden: {vol}");
    let speed = node(&tree, "speed");
    assert_eq!(said(speed), ("slider", "Speed"), "a hidden label still names it");
    assert!(has(speed, "disabled") && actions(speed).is_empty());
    no_echo(&tree);

    assert!(act(&mut rt, "volume", Action::Increment));
    assert!((rt.state.level - 0.45).abs() < 1e-4);
    assert!(act(&mut rt, "volume", Action::Decrement));
    assert!((rt.state.level - 0.40).abs() < 1e-4);
    assert!(rt.accessibility_action("volume", Action::SetValue, Some(ActionData::NumericValue(73.0))));
    settle(&mut rt);
    assert!((rt.state.level - 0.75).abs() < 1e-4, "SetValue snaps to step 5: {}", rt.state.level);
    assert_eq!(node(&rt.accessibility_json(), "volume")["value"], "75%");
    assert!(act(&mut rt, "volume", Action::Focus));
    press_key(&mut rt, Key::Right);
    assert!((rt.state.level - 0.80).abs() < 1e-4);
    press_key(&mut rt, Key::Home);
    assert_eq!(rt.state.level, 0.0);
    press_key(&mut rt, Key::End);
    assert_eq!(rt.state.level, 1.0);
    assert!(!act(&mut rt, "speed", Action::Increment), "disabled slider takes no assistive action");
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

#[test]
fn kit_fields_attach_description_to_their_inner_editor_node() {
    fn view(s: &App, _: &mut Cx<App>) -> El {
        let k = kit();
        col()
            .gap(8.0)
            .child(
                TextInput::new(&s.edit)
                    .label("Filename")
                    .description("Name is taken")
                    .on_edit(|s: &mut App| &mut s.edit)
                    .build(&k)
                    .key("ti"),
            )
            .child(
                TextArea::new(&s.edit)
                    .label("Notes")
                    .description("Markdown supported")
                    .on_edit(|s: &mut App| &mut s.edit)
                    .build(&k)
                    .key("ta"),
            )
            .child(
                TextField::new("Alias", &s.edit)
                    .description("Lowercase letters only")
                    .on_edit(|s: &mut App| &mut s.edit)
                    .build(&k)
                    .key("tf-desc"),
            )
            .child(
                TextField::new("Slug", &s.edit)
                    .description("Lowercase letters only")
                    .error("Already in use")
                    .on_edit(|s: &mut App| &mut s.edit)
                    .build(&k)
                    .key("tf-both"),
            )
            .child(
                TextField::new("Path", &s.edit)
                    .description("Must be relative.")
                    .error("Starts with /")
                    .on_edit(|s: &mut App| &mut s.edit)
                    .build(&k)
                    .key("tf-period"),
            )
            .child(
                TextField::new("Empty", &s.edit)
                    .description("")
                    .error("Required")
                    .on_edit(|s: &mut App| &mut s.edit)
                    .build(&k)
                    .key("tf-empty-desc"),
            )
            .child(
                TextArea::new(&s.edit)
                    .label("الملاحظات")
                    .description("أدخل التفاصيل هنا")
                    .on_edit(|s: &mut App| &mut s.edit)
                    .build(&k)
                    .key("ta-rtl"),
            )
    }

    let rt = mount(App::default(), view);
    let tree = rt.accessibility_json();

    for (outer_key, expected_role, expected_desc) in [
        ("ti", "text-input", "Name is taken"),
        ("ta", "multiline-text-input", "Markdown supported"),
        ("tf-desc", "text-input", "Lowercase letters only"),
        ("tf-both", "text-input", "Lowercase letters only. Already in use"),
        ("tf-period", "text-input", "Must be relative. Starts with /"),
        ("tf-empty-desc", "text-input", "Required"),
        ("ta-rtl", "multiline-text-input", "أدخل التفاصيل هنا"),
    ] {
        let outer = node(&tree, outer_key);
        assert!(
            outer["description"].is_null(),
            "{outer_key}: description must not land on outer wrapper: {outer}"
        );
        let inner = node(outer, "input");
        assert_eq!(inner["role"], expected_role, "{outer_key}: inner role");
        assert_eq!(
            inner["description"].as_str(),
            Some(expected_desc),
            "{outer_key}: description on inner editor"
        );
    }
}

#[test]
fn textarea_plain_trailing_and_scroll_bar_accessibility() {
    fn view(s: &App, _: &mut Cx<App>) -> El {
        let k = kit();
        col()
            .gap(8.0)
            .child(
                TextArea::new(&s.edit)
                    .label("Composer")
                    .description("Shift+Enter for a newline")
                    .plain()
                    .auto_grow(1, 3)
                    .scroll_bar(true)
                    .trailing(
                        Button::new("Send")
                            .control_size(ControlSize::Small)
                            .on_press(|s: &mut App| s.presses += 1)
                            .build(&k)
                            .key("send"),
                    )
                    .on_edit(|s: &mut App| &mut s.edit)
                    .build(&k)
                    .key("composer"),
            )
            .child(
                TextArea::new(&s.edit)
                    .label("Invalid plain")
                    .plain()
                    .invalid(true)
                    .build(&k)
                    .key("invalid-plain"),
            )
            .child(
                TextArea::new(&s.edit)
                    .label("Disabled plain")
                    .plain()
                    .disabled(true)
                    .build(&k)
                    .key("disabled-plain"),
            )
    }

    let mut rt = mount(App::default(), view);
    let tree = rt.accessibility_json();

    let composer = node(&tree, "composer");
    let input = node(composer, "input");
    assert_eq!(said(input), ("multiline-text-input", "Composer"));
    assert_eq!(input["description"], "Shift+Enter for a newline");
    assert!(can(input, "focus") && can(input, "set-value"));
    let send = node(composer, "send");
    assert_eq!(said(send), ("button", "Send"));
    assert!(can(send, "click") && can(send, "focus"));
    assert!(with_role(&tree, "scroll-bar").is_empty(), "decorative scroll bar stays hidden from a11y");

    let invalid = node(node(&tree, "invalid-plain"), "input");
    assert!(has(invalid, "invalid"), "invalid plain TextArea reports invalid state: {invalid}");
    let disabled = node(node(&tree, "disabled-plain"), "input");
    assert!(has(disabled, "disabled") && !can(disabled, "set-value"));

    // Web accessibility mirror exposes the trailing Send button and omits the decorative scroll bar.
    let mirror: Vec<serde_json::Value> = serde_json::from_str(&rt.a11y_json()).unwrap();
    assert!(
        mirror.iter().any(|v| v["t"] == "Send" && v["k"] == "button"),
        "web mirror exposes trailing Send button: {mirror:?}"
    );
    assert!(
        !mirror.iter().any(|v| v["p"].as_str().is_some_and(|p| p.contains("scroll-bar"))),
        "web mirror omits decorative scroll bar: {mirror:?}"
    );

    // Assistive click activates the trailing Send button independently of the editor.
    assert!(act(&mut rt, "send", Action::Click));
    assert_eq!(rt.state.presses, 1);
    assert!(act(&mut rt, "send", Action::Focus));
    assert_eq!(focused(&rt.accessibility_json()).as_deref(), Some("send"));
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

#[test]
fn segmented_control_tabs_icons_descriptions_and_controls() {
    use rune::{Direction, el, text};

    fn tabs(s: &App, _: &mut Cx<App>) -> El {
        let k = kit();
        let sel = s.picked.unwrap_or(0);
        col()
            .gap(8.0)
            .child(
                SegmentedControl::new(&["General", "Network", "Security"], Some(sel))
                    .label("Settings sections")
                    .icons(&[Some(k.glyph.search), Some(k.glyph.copy), Some(k.glyph.lock)])
                    .trailing(1, el().size(6.0, 6.0).radius(3.0).fill(k.c.danger.fg).a11y_hidden())
                    .help(0, "General")
                    .shortcut(0, "cmd+1")
                    .help(1, "Network status")
                    .shortcut(1, "F2")
                    .description(1, "2 connection errors")
                    .help(2, "Locked by policy")
                    .segment_state(2, State::Disabled)
                    .controls(0, "settings-panel")
                    .controls(1, "settings-panel")
                    .controls(2, "settings-panel")
                    .as_tabs()
                    .on_pick(|s: &mut App, i, _| s.picked = Some(i))
                    .build(&k)
                    .key("sections"),
            )
            .child(
                col()
                    .key("settings-panel")
                    .role("tabpanel")
                    .label("Settings detail")
                    .child(text(format!("Panel {sel}"))),
            )
            .child(
                SegmentedControl::new(&["", "Notes"], Some(0))
                    .label("Quick switcher")
                    .icons(&[Some(k.glyph.search), Some(k.glyph.copy)])
                    .help(0, "Search")
                    .shortcut(0, "F3")
                    .help(1, "Show inline notes")
                    .build(&k)
                    .key("quick"),
            )
            .child(
                rune::row()
                    .dir(Direction::Rtl)
                    .child(
                        SegmentedControl::new(&["الأول", "الثاني"], Some(sel.min(1)))
                            .label("تبويبات")
                            .as_tabs()
                            .on_pick(|s: &mut App, i, _| s.picked = Some(i))
                            .build(&k)
                            .key("rtl-tabs"),
                    ),
            )
    }

    let mut rt = mount(App { picked: Some(0), ..App::default() }, tabs);
    let tree = rt.accessibility_json();

    // Root switches to tab-list with horizontal orientation and setSize.
    let sections = node(&tree, "sections");
    assert_eq!(said(sections), ("tab-list", "Settings sections"));
    assert_eq!(sections["orientation"], "horizontal");
    assert_eq!(sections["setSize"], 3);

    let cmd1 = if cfg!(target_os = "macos") { "⌘1" } else { "Super+1" };
    let tab0 = node(sections, "segment0");
    assert_eq!(said(tab0), ("tab", "General"));
    assert!(has(tab0, "selected") && !has(tab0, "checked") && !has(tab0, "unchecked"), "tab uses selected: {tab0}");
    assert_eq!(tab0["position"], 1);
    assert_eq!(tab0["shortcut"], cmd1);
    assert!(tab0["description"].is_null(), "help matching label does not echo as description: {tab0}");
    assert_eq!(tab0["controls"], serde_json::json!(["settings-panel"]));
    assert!(can(tab0, "click") && can(tab0, "focus"));

    let tab1 = node(sections, "segment1");
    assert_eq!(said(tab1), ("tab", "Network"));
    assert!(!has(tab1, "selected") && !has(tab1, "checked"));
    assert_eq!(tab1["position"], 2);
    assert_eq!(tab1["shortcut"], "F2");
    assert_eq!(tab1["description"], "2 connection errors", "explicit description overrides tooltip text");
    assert_eq!(tab1["controls"], serde_json::json!(["settings-panel"]));

    let tab2 = node(sections, "segment2");
    assert_eq!(said(tab2), ("tab", "Security"));
    assert!(has(tab2, "disabled") && actions(tab2).is_empty());
    assert_eq!(tab2["description"], "Locked by policy");
    assert_eq!(tab2["controls"], serde_json::json!(["settings-panel"]));

    // Icon-only segment uses help as its accessible name without echoing it as description;
    // labelled segment whose help differs from its label uses help as description.
    let quick = node(&tree, "quick");
    assert_eq!(said(quick), ("radio-group", "Quick switcher"));
    assert!(quick["orientation"].is_null(), "radiogroup leaves orientation unset");
    let q0 = node(quick, "segment0");
    assert_eq!(said(q0), ("radio-button", "Search"));
    assert!(q0["description"].is_null(), "help naming an icon-only segment is not echoed: {q0}");
    assert_eq!(q0["shortcut"], "F3");
    let q1 = node(quick, "segment1");
    assert_eq!(said(q1), ("radio-button", "Notes"));
    assert_eq!(q1["description"], "Show inline notes");

    assert!(with_role(&tree, "image").is_empty(), "decorative segment icons stay out of the tree");
    no_echo(&tree);

    // Web accessibility mirror exposes tablist orientation, tab roles, aria-selected, description, and controls.
    let mirror: Vec<serde_json::Value> = serde_json::from_str(&rt.a11y_json()).unwrap();
    let m_list = mirror.iter().find(|v| v["o"] == "tablist" && v["t"] == "Settings sections").expect("mirrored tablist");
    assert_eq!(m_list["or"], "horizontal");
    let m_tab0 = mirror.iter().find(|v| v["o"] == "tab" && v["t"] == "General").expect("mirrored tab0");
    assert_eq!(m_tab0["se"], true);
    assert_eq!(m_tab0["ct"], "settings-panel");
    let m_tab1 = mirror.iter().find(|v| v["o"] == "tab" && v["t"] == "Network").expect("mirrored tab1");
    assert_eq!(m_tab1["se"], false);
    assert_eq!(m_tab1["d"], "2 connection errors");
    assert_eq!(m_tab1["ct"], "settings-panel");
    let m_tab2 = mirror.iter().find(|v| v["o"] == "tab" && v["t"] == "Security").expect("mirrored tab2");
    assert_eq!(m_tab2["dis"], true);
    let m_icon_only = mirror.iter().find(|v| v["o"] == "radio" && v["t"] == "Search").expect("mirrored icon-only radio");
    assert!(m_icon_only["d"].is_null(), "icon-only radio does not echo help in web mirror: {m_icon_only}");

    // Keyboard and assistive navigation in tab mode: arrows move selection and skip disabled tabs.
    assert!(act(&mut rt, "sections/segment0", Action::Focus) || act(&mut rt, "segment0", Action::Focus));
    press_key(&mut rt, Key::Right);
    assert_eq!(rt.state.picked, Some(1));
    assert!(has(node(node(&rt.accessibility_json(), "sections"), "segment1"), "selected"));
    press_key(&mut rt, Key::Right);
    assert_eq!(rt.state.picked, Some(0), "Right skips disabled tab and wraps to first");
    assert!(!act(&mut rt, "segment2", Action::Click), "disabled tab rejects assistive click");
    tap(&mut rt, "segment1");
    assert_eq!(rt.state.picked, Some(1));

    // RTL tablist: Left moves forward from first tab to second tab.
    rt.update(|s, tx| { s.picked = Some(0); tx.focus("rtl-tabs", true); });
    settle(&mut rt);
    press_key(&mut rt, Key::Left);
    assert_eq!(rt.state.picked, Some(1), "RTL tablist moves forward on Left arrow");
}
