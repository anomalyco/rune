//! The stories, one function per `@opencode/ui` Storybook story id (title + export name).
//! Layout wrappers copy each story's own inline styles.

use rune::{Edge, El, TextEdit, TextStyle, Tx, col, el, row, text};
use rune_kit::*;
use rune_theme_opencode::icons as ic;

use crate::G;

type Story = fn(&G, &Kit, &mut Vec<El>) -> El;

macro_rules! stories {
    ($($id:literal => $f:path,)*) => {
        pub const STORIES: &[(&str, Story)] = &[$(($id, $f),)*];
        pub const SCENES: &[(&str, fn(&mut G))] = &[$(($id, |s: &mut G| {
            s.story = $id;
            s.combo = ComboboxState::default();
            if $id == "rune-combobox--searchable" { s.combo.open = true; s.combo.search.focused = true; }
            s.open = OPENED.iter().find(|(id, _)| *id == $id).map(|(_, k)| (*k).to_owned());
        }),)*];
    };
}

/// Stories whose web version opens an overlay (by default or through the harness's click):
/// their scene starts with that overlay open.
const OPENED: &[(&str, &str)] = &[
    ("ui-dialog--basic", "dialog"),
    ("ui-menu--basic", "menu"),
    ("ui-menu--rich", "menu"),
    ("ui-menu--with-checkbox", "menu"),
    ("ui-menu--with-radio", "menu"),
    ("ui-menu--with-submenu", "menu"),
    ("ui-menu--appearances", "both"),
    ("ui-select--playground", "select"),
    ("ui-select--grouped", "select"),
    ("components-popover--basic", "popover"),
    ("components-popover--controlled", "popover"),
    ("components-hover-card--basic", "card"),
    ("ui-tooltip--simple", "tip"),
    ("ui-tooltip--with-keybind", "tip"),
    ("ui-tooltip--path", "tip"),
    ("ui-tooltip--title-description", "tip"),
    ("ui-toast--all-examples", "toast"),
];

fn is_open(s: &G, key: &str) -> bool {
    s.open.as_deref() == Some(key) || s.open.as_deref() == Some("both")
}

fn toggle(key: &'static str) -> impl Fn(&mut G) + Clone {
    move |s: &mut G| s.toggle_open(key)
}

fn close(s: &mut G) {
    s.open = None;
}

stories! {
    "rune-combobox--searchable" => combobox_searchable,
    "ui-dialog--basic" => dialog_basic,
    "motion-focus-modal" => motion_focus_modal,
    "ui-button--playground" => button_playground,
    "ui-button--variants" => button_variants,
    "ui-button--sizes" => button_sizes,
    "ui-button--icon" => button_icon,
    "ui-button--all-states" => button_all_states,
    "ui-icon-button--playground" => icon_button_playground,
    "ui-icon-button--variants" => icon_button_variants,
    "ui-icon-button--sizes" => icon_button_sizes,
    "ui-icon-button--all-states" => icon_button_all_states,
    "ui-switch--basic" => switch_basic,
    "ui-switch--appearances" => switch_appearances,
    "ui-switch--states" => switch_states,
    "ui-switch--hidden-label" => switch_hidden_label,
    "ui-checkbox--basic" => checkbox_basic,
    "ui-checkbox--states" => checkbox_states,
    "ui-checkbox--custom-icon" => checkbox_custom_icon,
    "ui-checkbox--hidden-label" => checkbox_hidden_label,
    "ui-radio--basic" => radio_basic,
    "ui-radio--controlled" => radio_controlled,
    "ui-radio--states" => radio_states,
    "ui-text-input--playground" => text_input_playground,
    "ui-text-input--with-copy-button" => text_input_copy,
    "ui-text-input--controlled" => text_input_controlled,
    "ui-text-input--appearances" => text_input_appearances,
    "ui-text-input--with-field" => text_input_with_field,
    "ui-text-input--states" => text_input_states,
    "ui-textarea--playground" => textarea_playground,
    "ui-textarea--controlled" => textarea_controlled,
    "ui-textarea--with-field" => textarea_with_field,
    "ui-textarea--states" => textarea_states,
    "ui-field--text-input-example" => field_text_input,
    "ui-field--text-input-direct-child" => field_direct_child,
    "ui-field--textarea-example" => field_textarea,
    "ui-field--invalid" => field_invalid,
    "ui-select--playground" => select_playground,
    "ui-select--grouped" => select_grouped,
    "ui-select--invalid" => select_invalid,
    "ui-select--disabled" => select_disabled,
    "ui-select--with-field" => select_with_field,
    "components-popover--basic" => popover_basic,
    "components-popover--controlled" => popover_controlled,
    "ui-menu--basic" => menu_basic,
    "ui-menu--appearances" => menu_appearances,
    "ui-menu--rich" => menu_rich,
    "ui-menu--with-checkbox" => menu_checkbox,
    "ui-menu--with-radio" => menu_radio,
    "ui-menu--with-submenu" => menu_submenu,
    "ui-tooltip--simple" => tooltip_simple,
    "ui-tooltip--with-keybind" => tooltip_keybind,
    "ui-tooltip--path" => tooltip_path,
    "ui-tooltip--title-description" => tooltip_title_description,
    "components-hover-card--basic" => hover_card_basic,
    "ui-toast--all-examples" => toast_all,
    "components-card--normal" => card_normal,
    "components-card--error" => card_error,
    "components-card--warning" => card_warning,
    "components-card--success" => card_success,
    "components-card--info" => card_info,
    "components-list--basic" => list_basic,
    "components-list--grouped" => list_grouped,
    "components-list--empty" => list_empty,
    "components-list--with-add" => list_with_add,
    "components-list--divider" => list_divider,
    "components-list--active-icon" => list_active_icon,
    "components-list--no-search" => list_no_search,
    "ui-accordion--basic" => accordion_basic,
    "ui-accordion--multiple" => accordion_multiple,
    "ui-accordion--non-collapsible" => accordion_single,
    "components-collapsible--basic" => collapsible_basic,
    "components-collapsible--ghost" => collapsible_ghost,
    "ui-badge--basic" => badge_basic,
    "ui-badge--appearances" => badge_appearances,
    "ui-badge--subtle" => badge_subtle,
    "ui-badge--high-contrast" => badge_high_contrast,
    "ui-badge--accent" => badge_accent,
    "ui-keybind--playground" => keybind_playground,
    "ui-keybind--variants" => keybind_variants,
    "ui-keybind--multiple-keys" => keybind_multiple,
    "ui-keybind--all-examples" => keybind_all,
    "components-spinner--basic" => spinner_basic,
    "components-spinner--sizes" => spinner_sizes,
    "ui-loader--basic" => loader_basic,
    "ui-loader--sizes" => loader_sizes,
    "components-progress--basic" => progress_basic,
    "components-progress--no-label" => progress_no_label,
    "components-progress--indeterminate" => progress_indeterminate,
    "ui-segmented-control--basic" => segmented_basic,
    "ui-segmented-control--controlled" => segmented_controlled,
    "rune-keybind--symbols" => keybind_symbols,
    "rune-rolling-text--basic" => rolling_text_basic,
}

fn combobox_searchable(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let options: Vec<_> = (0..200).map(|i| ComboboxOption::new(format!("model-{i}"), format!("Model {i:03}"))).collect();
    Combobox::new("model", &s.combo, &options, |s: &mut G| &mut s.combo)
        .placeholder("Choose model").current(s.pick.get("model").map(String::as_str))
        .on_choose(|s, id, _| { s.pick.insert("model".into(), id.into()); })
        .build(k, layer)
}

fn dialog_basic(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    Dialog::new("dialog", "Dialog").description("Description").open(is_open(s, "dialog"))
        .on_close(close).child(body_text(k, "Dialog body content.")).build(k, layer);
    Button::new("Open dialog").on_press(toggle("dialog")).build(k).key("opener")
}

fn motion_focus_modal(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    Dialog::new("dialog", "Loading").open(is_open(s, "dialog")).on_close(close)
        .child(row().gap(16.0).child(Spinner::new().build(k).key("spinner"))
            .child(Loader::new().build(k).key("loader")))
        .child(Button::new("Done").on_press(close).build(k).key("done")).build(k, layer);
    col().gap(16.0)
        .child(Button::new("Open dialog").on_press(toggle("dialog")).build(k).key("opener"))
        .child(Switch::new(s.is("switch")).label("Sync").on_toggle(|s: &mut G| s.flip("switch")).build(k).key("switch"))
        .child(Checkbox::new(s.is("check")).label("Remember").on_toggle(|s: &mut G| s.flip("check")).build(k).key("check"))
        .child(SegmentedControl::new(&["First", "Second"], Some(if s.is("second") { 1 } else { 0 }))
            .on_pick(|s: &mut G, i, _| { if s.is("second") != (i == 1) { s.flip("second"); } }).build(k))
}

pub fn render(s: &G, k: &Kit, overlays: &mut Vec<El>) -> El {
    match STORIES.iter().find(|(id, _)| *id == s.story) {
        Some((_, f)) => f(s, k, overlays),
        None => index(s, k, overlays),
    }
}

/// Every story stacked, for browsing in a window.
fn index(s: &G, k: &Kit, overlays: &mut Vec<El>) -> El {
    let mut c = col().key("index").gap(24.0);
    for (id, f) in STORIES {
        c = c.child(col().key(*id).gap(8.0).child(caption(k, id)).child(f(s, k, overlays)));
    }
    c
}

/// The body's default text: 13 px Inter at 440 on a 24 px line (Storybook's `BodyTypography`).
#[allow(dead_code)]
fn body(k: &Kit) -> TextStyle {
    TextStyle { tracking: 0.0, ..k.ty.ui }.lh(24.0)
}

/// The stories' `font-size: 12px; color: var(--text-weak)` labels (line height inherited).
fn caption(k: &Kit, label: &str) -> El {
    text(label).font(TextStyle::new(12.0, 440.0).lh(24.0)).color(k.c.text_weak)
}

/// `display: flex; gap; align-items: center; flex-wrap: wrap`.
fn flex(gap: f32) -> El {
    row().gap(gap).center_y().flex_wrap()
}

/// CSS `text-transform: capitalize`: the first letter of every word (hyphens split words).
fn title_case(s: &str) -> String {
    let mut out = String::new();
    let mut start = true;
    for ch in s.chars() {
        if start {
            out.extend(ch.to_uppercase())
        } else {
            out.push(ch)
        }
        start = !ch.is_alphanumeric();
    }
    out
}

/// The stories' own `toTitleCase`: only the first letter.
fn cap_first(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

const STATES: [(&str, State); 5] = [
    ("default", State::Rest),
    ("hover", State::Hover),
    ("pressed", State::Pressed),
    ("focus", State::Focus),
    ("disabled", State::Disabled),
];

// ---------------------------------------------------------------------------------------------
// Button

fn button_playground(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(Button::new("Button").build(k))
}

fn button_variants(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    use ButtonVariant::*;
    flex(12.0)
        .child(Button::new("Neutral").variant(Neutral).build(k))
        .child(Button::new("Danger").variant(Danger).build(k))
        .child(Button::new("Warning").variant(Warning).build(k))
        .child(Button::new("Contrast").variant(Contrast).build(k))
        .child(Button::new("Submit").variant(Submit).build(k))
        .child(Button::new("Ghost").variant(Ghost).build(k))
        .child(Button::new("Ghost muted").variant(GhostMuted).icon(&ic::EDIT).build(k))
        .child(Button::new("Ghost faint").variant(GhostFaint).icon(&ic::EDIT).build(k))
        .child(Button::new("Loading").variant(Loading).build(k))
}

fn button_sizes(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    flex(12.0)
        .child(Button::new("Small").size(ButtonSize::Small).build(k))
        .child(Button::new("Normal").size(ButtonSize::Normal).build(k))
        .child(Button::new("Large").size(ButtonSize::Large).build(k))
}

fn button_icon(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    flex(12.0)
        .child(Button::new("Normal").icon(&ic::PLUS).build(k))
        .child(Button::new("Large").variant(ButtonVariant::Contrast).size(ButtonSize::Large).icon(&ic::PLUS).build(k))
}

fn button_all_states(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    use ButtonVariant::*;
    let variants = [
        ("neutral", Neutral),
        ("danger", Danger),
        ("warning", Warning),
        ("contrast", Contrast),
        ("submit", Submit),
        ("ghost", Ghost),
        ("ghost-muted", GhostMuted),
        ("ghost-faint", GhostFaint),
        ("loading", Loading),
    ];
    let mut c = col().gap(12.0);
    for (name, v) in variants {
        let mut r = row().gap(8.0).flex_wrap();
        for (state, st) in STATES {
            r = r.child(Button::new(cap_first(state)).variant(v).state(st).build(k).key(format!("{name}-{state}")));
        }
        c = c.child(col().gap(8.0).child(caption(k, &title_case(name))).child(r));
    }
    c
}

// ---------------------------------------------------------------------------------------------
// IconButton

fn icon_button_playground(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(IconButton::new(&ic::PLUS).build(k))
}

fn icon_button_variants(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    use IconButtonVariant::*;
    flex(12.0)
        .child(IconButton::new(&ic::PLUS).variant(Neutral).build(k))
        .child(IconButton::new(&ic::PLUS).variant(Contrast).build(k))
        .child(IconButton::new(&ic::ARROW_UP).variant(Submit).build(k))
        .child(IconButton::new(&ic::PLUS).variant(Ghost).build(k))
}

fn icon_button_sizes(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    flex(12.0)
        .child(IconButton::new(&ic::PLUS).size(ButtonSize::Small).build(k))
        .child(IconButton::new(&ic::PLUS).size(ButtonSize::Normal).build(k))
        .child(IconButton::new(&ic::PLUS).size(ButtonSize::Large).build(k))
}

fn icon_button_all_states(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    use IconButtonVariant::*;
    let variants = [("neutral", Neutral), ("contrast", Contrast), ("submit", Submit), ("ghost", Ghost)];
    let mut c = col().gap(12.0);
    for (name, v) in variants {
        let mut r = row().gap(8.0).flex_wrap();
        for (state, st) in STATES {
            r = r.child(IconButton::new(&ic::PLUS).variant(v).state(st).build(k).key(format!("{name}-{state}")));
        }
        c = c.child(col().gap(8.0).child(caption(k, &title_case(name))).child(r));
    }
    c
}

// ---------------------------------------------------------------------------------------------
// Switch

fn flip(key: &'static str) -> impl Fn(&mut G) {
    move |s: &mut G| s.flip(key)
}

/// `defaultChecked` items read as on until toggled.
fn on_by_default(s: &G, key: &str) -> bool {
    !s.is(key)
}

fn switch_basic(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(Switch::new(on_by_default(s, "sw")).label("Enable notifications").on_toggle(flip("sw")).build(k))
}

fn switch_appearances(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    col()
        .gap(12.0)
        .child(Switch::new(on_by_default(s, "std")).label("Standard").standard().on_toggle(flip("std")).build(k))
        .child(Switch::new(on_by_default(s, "cmp")).label("Compact").on_toggle(flip("cmp")).build(k))
}

fn switch_states(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    col()
        .gap(12.0)
        .child(Switch::new(on_by_default(s, "a")).label("Enabled").on_toggle(flip("a")).build(k))
        .child(Switch::new(s.is("b")).label("Disabled").on_toggle(flip("b")).build(k))
        .child(Switch::new(false).label("Disabled switch").disabled(true).build(k))
}

fn switch_hidden_label(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(Switch::new(on_by_default(s, "sw")).label("Hidden label").hide_label().on_toggle(flip("sw")).build(k))
}

// ---------------------------------------------------------------------------------------------
// Checkbox

fn checkbox_basic(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(Checkbox::new(on_by_default(s, "cb")).label("Checkbox").on_toggle(flip("cb")).build(k))
}

fn checkbox_states(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    col()
        .gap(12.0)
        .child(Checkbox::new(on_by_default(s, "a")).label("Checked").on_toggle(flip("a")).build(k))
        .child(Checkbox::new(s.is("b")).label("Unchecked").on_toggle(flip("b")).build(k))
        .child(Checkbox::new(false).label("Disabled").disabled(true).build(k))
        .child(
            Checkbox::new(s.is("d")).label("With description").description("Helper text").on_toggle(flip("d")).build(k),
        )
}

fn checkbox_custom_icon(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(
        Checkbox::new(on_by_default(s, "cb")).label("Custom icon").icon(&ic::CHECK).on_toggle(flip("cb")).build(k),
    )
}

fn checkbox_hidden_label(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(Checkbox::new(s.is("cb")).label("Hidden label").hide_label().on_toggle(flip("cb")).build(k))
}

// ---------------------------------------------------------------------------------------------
// Radio

fn radio_group(
    s: &G,
    group: &'static str,
    default: &str,
    label: &str,
    items: &[(&'static str, &str, Option<&str>)],
) -> RadioGroup {
    let current = s.picked(group, default).to_owned();
    let mut g = RadioGroup::new().label(label);
    for (value, l, d) in items {
        let v = *value;
        let mut item = RadioItem::new(*l, current == v).on_select(move |s: &mut G| {
            s.pick.insert(group.to_owned(), v.to_owned());
        });
        if let Some(d) = d {
            item = item.description(*d);
        }
        g = g.item(item);
    }
    g
}

fn radio_basic(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    radio_group(
        s,
        "freq",
        "daily",
        "Notification frequency",
        &[
            ("daily", "Daily", Some("Once per day at 9am.")),
            ("weekly", "Weekly", Some("Every Monday morning.")),
            ("never", "Never", Some("No notifications.")),
        ],
    )
    .build(k)
}

fn radio_controlled(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let v = s.picked("ctl", "daily").to_owned();
    col()
        .gap(12.0)
        .child(
            radio_group(
                s,
                "ctl",
                "daily",
                "Controlled",
                &[("daily", "Daily", None), ("weekly", "Weekly", None), ("never", "Never", None)],
            )
            .build(k),
        )
        .child(text(format!("Value: {v}")).font(TextStyle::new(12.0, 440.0).lh(24.0)).color(k.c.text_faint))
}

fn radio_states(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let disabled_item = RadioGroup::new()
        .label("Disabled item")
        .item(RadioItem::new("Enabled", s.picked("di", "a") == "a").on_select(|s: &mut G| {
            s.pick.insert("di".into(), "a".into());
        }))
        .item(RadioItem::new("Disabled", false).disabled(true));
    col()
        .gap(20.0)
        .child(
            radio_group(
                s,
                "def",
                "a",
                "Default",
                &[("a", "Option A", None), ("b", "Option B", Some("Has a description."))],
            )
            .build(k),
        )
        .child(
            RadioGroup::new()
                .label("Disabled group")
                .disabled(true)
                .item(RadioItem::new("Option A", true))
                .item(RadioItem::new("Option B", false))
                .build(k),
        )
        .child(disabled_item.build(k))
        .child(
            radio_group(s, "inv", "a", "Invalid", &[("a", "Option A", None), ("b", "Option B", None)])
                .description("Pick one option.")
                .invalid(true)
                .build(k),
        )
}

// ---------------------------------------------------------------------------------------------
// TextInput, Textarea, Field
//
// Solid passes `defaultValue` to these inputs as an attribute the DOM ignores, so the web
// stories show their placeholders; the gallery does the same.

fn edit(v: &str) -> TextEdit {
    TextEdit::new(v)
}

fn grid(gap: f32, width: f32) -> El {
    col().gap(gap).w(width).items_stretch()
}

fn text_input_playground(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(TextInput::new(&s.edits[0]).placeholder("Placeholder").on_edit(|s: &mut G| &mut s.edits[0]).build(k))
}

fn text_input_copy(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(TextInput::new(&edit("")).placeholder("api.example.com/v1").copy_button().build(k))
}

fn text_input_controlled(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let value = if s.edits[1].text.is_empty() && !s.is("typed") {
        "Controlled value".to_owned()
    } else {
        s.edits[1].text.clone()
    };
    col()
        .gap(12.0)
        .child(TextInput::new(&edit(&value)).placeholder("Type here…").build(k))
        .child(text(format!("Value: {value}")).font(TextStyle::new(12.0, 440.0).lh(24.0)).color(k.c.std_text))
}

fn text_input_appearances(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    grid(20.0, 280.0)
        .child(TextInput::new(&edit("")).placeholder("Base (28px)").build(k))
        .child(TextInput::new(&edit("")).placeholder("Large (32px)").large().build(k))
        .child(TextInput::new(&edit("")).placeholder("Large with copy").large().copy_button().build(k))
}

fn text_input_with_field(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    grid(24.0, 280.0)
        .child(
            Field::new()
                .label("Label")
                .info()
                .prefix("Prefix")
                .control(TextInput::new(&edit("")).placeholder("Text").copy_button().build(k))
                .suffix("Suffix")
                .build(k),
        )
        .child(
            Field::new()
                .label("Label")
                .prefix("Prefix")
                .control(TextInput::new(&edit("")).placeholder("Text").copy_button().invalid(true).build(k))
                .suffix("Suffix")
                .build(k),
        )
}

fn text_input_states(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    grid(20.0, 280.0)
        .child(TextInput::new(&edit("")).placeholder("Default").build(k))
        .child(TextInput::new(&edit("")).placeholder("With value").build(k))
        .child(TextInput::new(&edit("")).placeholder("With copy").copy_button().build(k))
        .child(TextInput::new(&edit("")).placeholder("Invalid").invalid(true).copy_button().build(k))
        .child(TextInput::new(&edit("")).placeholder("Disabled").disabled(true).build(k))
        .child(TextInput::new(&edit("")).placeholder("Disabled with value").disabled(true).copy_button().build(k))
}

fn textarea_playground(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(TextArea::new(&s.edits[2]).placeholder("Placeholder").on_edit(|s: &mut G| &mut s.edits[2]).build(k))
}

fn textarea_controlled(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    grid(12.0, 280.0)
        .child(TextArea::new(&edit("")).placeholder("Type here…").build(k))
        .child(text("Value: Controlled value").font(TextStyle::new(12.0, 440.0).lh(24.0)).color(k.c.std_text))
}

fn textarea_with_field(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    grid(24.0, 280.0)
        .child(
            Field::new()
                .label("Label")
                .info()
                .prefix("Prefix")
                .control(TextArea::new(&edit("")).placeholder("Text").build(k))
                .suffix("Suffix")
                .build(k),
        )
        .child(
            Field::new()
                .label("Label")
                .prefix("Prefix")
                .control(TextArea::new(&edit("")).placeholder("Text").invalid(true).build(k))
                .suffix("Suffix")
                .build(k),
        )
}

fn textarea_states(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    grid(20.0, 280.0)
        .child(TextArea::new(&edit("")).placeholder("Default").build(k))
        .child(TextArea::new(&edit("")).placeholder("With value").build(k))
        .child(TextArea::new(&edit("")).placeholder("Invalid").invalid(true).build(k))
        .child(TextArea::new(&edit("")).placeholder("Disabled").disabled(true).build(k))
        .child(TextArea::new(&edit("")).placeholder("Disabled with value").disabled(true).build(k))
}

fn field_text_input(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    grid(0.0, 280.0).child(
        Field::new()
            .label("Label")
            .info()
            .prefix("Prefix")
            .control(TextInput::new(&edit("")).placeholder("Text").copy_button().build(k))
            .suffix("Suffix")
            .build(k),
    )
}

fn field_direct_child(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    grid(0.0, 280.0).child(
        Field::new()
            .label("Label")
            .prefix("Prefix")
            .control(TextInput::new(&edit("")).placeholder("Text").build(k))
            .suffix("Suffix")
            .build(k),
    )
}

fn field_textarea(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    grid(0.0, 280.0).child(
        Field::new()
            .label("Label")
            .prefix("Prefix")
            .control(TextArea::new(&edit("")).placeholder("Text").build(k))
            .suffix("Suffix")
            .build(k),
    )
}

fn field_invalid(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    grid(0.0, 280.0).child(
        Field::new()
            .label("Label")
            .prefix("Prefix")
            .control(TextInput::new(&edit("")).placeholder("Text").copy_button().invalid(true).build(k))
            .suffix("Suffix")
            .build(k),
    )
}

// ---------------------------------------------------------------------------------------------
// Select

const FRUITS: [&str; 5] = ["Apple", "Banana", "Cherry", "Date", "Elderberry"];

fn fruit_select(s: &G, key: &'static str) -> Select {
    let current = s.pick.get(key).cloned();
    Select::new(key, &FRUITS)
        .placeholder("Pick a fruit")
        .current(current.as_deref())
        .open(is_open(s, key))
        .on_toggle(toggle(key))
        .on_close(close)
        .on_choose(move |s: &mut G, v: &str, _: &mut Tx| {
            s.pick.insert(key.to_owned(), v.to_owned());
            s.open = None;
        })
}

fn select_playground(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    row().child(fruit_select(s, "select").build(k, layer))
}

fn select_grouped(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let cities =
        [("North", "Boston"), ("South", "Miami"), ("South", "Atlanta"), ("West", "Seattle"), ("West", "Denver")];
    let current = s.pick.get("select").cloned();
    row().child(
        Select::grouped("select", &cities)
            .placeholder("Pick a fruit")
            .current(current.as_deref())
            .open(is_open(s, "select"))
            .on_toggle(toggle("select"))
            .on_close(close)
            .on_choose(|s: &mut G, v: &str, _: &mut Tx| {
                s.pick.insert("select".into(), v.to_owned());
                s.open = None;
            })
            .build(k, layer),
    )
}

fn select_invalid(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    row().child(fruit_select(s, "select").invalid(true).build(k, layer))
}

fn select_disabled(_: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    row().child(Select::new("select", &FRUITS).current(Some("Cherry")).disabled(true).build(k, layer))
}

fn select_with_field(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    grid(0.0, 280.0).child(
        Field::new()
            .label("Fruit")
            .info()
            .prefix("Optional helper")
            .control(fruit_select(s, "select").build(k, layer))
            .suffix("After selection")
            .build(k),
    )
}

// ---------------------------------------------------------------------------------------------
// Popover, Menu, Tooltip, HoverCard, Toast

fn trigger_text(k: &Kit, s: &str) -> El {
    body_text(k, s)
}

fn popover_basic(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    row().child(
        Popover::new("popover")
            .trigger(trigger_text(k, "Open popover"))
            .title("Popover")
            .description("Optional description")
            .child(overlay_text(k, "Popover content"))
            .open(is_open(s, "popover"))
            .on_close(close)
            .build(k, layer)
            .on_press(toggle("popover")),
    )
}

fn popover_controlled(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    row().child(
        Popover::new("popover")
            .trigger(trigger_text(k, "Toggle popover"))
            .title("Controlled")
            .description("Open state is controlled")
            .child(overlay_text(k, "Controlled content"))
            .open(is_open(s, "popover"))
            .on_close(close)
            .build(k, layer)
            .on_press(toggle("popover")),
    )
}

fn menu_button(k: &Kit, label: &str, key: &'static str, open: bool) -> El {
    Button::new(label).expanded(open).on_press(toggle(key)).build(k)
}

fn menu_basic(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let open = is_open(s, "menu");
    row().child(
        Menu::new("menu")
            .trigger(menu_button(k, "Open menu", "menu", open))
            .item(MenuItem::new("New file").on_select(close))
            .item(MenuItem::new("Open file").on_select(close))
            .item(MenuItem::new("Save").on_select(close))
            .separator()
            .item(MenuItem::new("Print").disabled(true))
            .open(open)
            .on_close(close)
            .build(k, layer),
    )
}

fn menu_appearances(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let open = is_open(s, "both");
    row()
        .gap(16.0)
        .child(
            Menu::new("standard")
                .standard()
                .trigger(menu_button(k, "Standard", "both", open))
                .item(MenuItem::new("New file"))
                .item(MenuItem::new("Open file"))
                .open(open)
                .build(k, layer),
        )
        .child(
            Menu::new("compact")
                .trigger(menu_button(k, "Compact", "both", open))
                .item(MenuItem::new("New file"))
                .item(MenuItem::new("Open file"))
                .open(open)
                .build(k, layer),
        )
}

fn menu_rich(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let open = is_open(s, "menu");
    let avatar = row()
        .size(16.0, 16.0)
        .center()
        .radius(4.0)
        .fill(k.c.layer_03)
        .child(text("A").font(k.ty.small_medium).color(k.c.text_base));
    row().child(
        Menu::new("menu")
            .trigger(menu_button(k, "Open rich menu", "menu", open))
            .min_width(240.0)
            .item(MenuItem::new("Text").lead(avatar).icon(&ic::PLUS).shortcut("⇧ D").badge("Label"))
            .item(MenuItem::new("New window").icon(&ic::PLUS).shortcut("⌘ N"))
            .item(MenuItem::new("Save as…").icon(&ic::PLUS).shortcut("⌘ S").badge("Beta"))
            .separator()
            .item(MenuItem::new("Print").icon(&ic::PLUS).shortcut("⌘ P").disabled(true))
            .open(open)
            .on_close(close)
            .build(k, layer),
    )
}

fn menu_checkbox(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let open = is_open(s, "menu");
    row().child(
        Menu::new("menu")
            .trigger(menu_button(k, "View", "menu", open))
            .min_width(200.0)
            .item(MenuItem::new("Word wrap").shortcut("⌥ Z").checked(!s.is("wrap")).on_select(flip("wrap")))
            .item(MenuItem::new("Minimap").checked(s.is("minimap")).on_select(flip("minimap")))
            .item(MenuItem::new("Ruler").checked(false).disabled(true))
            .open(open)
            .on_close(close)
            .build(k, layer),
    )
}

fn menu_radio(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let open = is_open(s, "menu");
    let theme = s.picked("theme", "system").to_owned();
    let pick = |v: &'static str| {
        move |s: &mut G| {
            s.pick.insert("theme".into(), v.into());
        }
    };
    row().child(
        Menu::new("menu")
            .trigger(menu_button(k, "Theme", "menu", open))
            .min_width(200.0)
            .group_label("Appearance")
            .item(MenuItem::new("Light").checked(theme == "light").on_select(pick("light")))
            .item(MenuItem::new("Dark").checked(theme == "dark").on_select(pick("dark")))
            .item(MenuItem::new("System").badge("Auto").checked(theme == "system").on_select(pick("system")))
            .open(open)
            .on_close(close)
            .build(k, layer),
    )
}

fn menu_submenu(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let open = is_open(s, "menu");
    let recent = vec![
        MenuEntry::item(MenuItem::new("project-alpha.tsx")),
        MenuEntry::item(MenuItem::new("project-beta.tsx")),
        MenuEntry::item(MenuItem::new("project-gamma.tsx")),
        MenuEntry::Separator,
        MenuEntry::item(MenuItem::new("Clear recent")),
    ];
    row().child(
        Menu::new("menu")
            .trigger(menu_button(k, "File", "menu", open))
            .min_width(200.0)
            .item(MenuItem::new("New file").shortcut("⌘ N"))
            .item(MenuItem::new("Open file").shortcut("⌘ O"))
            .item(MenuItem::new("Open recent").submenu(recent, s.is("recent")).on_select(flip("recent")))
            .separator()
            .item(MenuItem::new("Save").shortcut("⌘ S"))
            .item(MenuItem::new("Save as…").shortcut("⇧⌘ S"))
            .open(open)
            .on_close(close)
            .build(k, layer),
    )
}

/// The tooltip stories' frame: 80 points of padding, centred.
fn tip_frame(e: El) -> El {
    row().w_full().padding(80.0).center().child(e)
}

fn tooltip_simple(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let t = Tooltip::new("tip").trigger(body_text(k, "Hover me")).open(is_open(s, "tip"));
    tip_frame(t.text(k, "Tooltip Text").build(k, layer))
}

fn tooltip_keybind(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let t = Tooltip::new("tip").trigger(body_text(k, "Hover me")).open(is_open(s, "tip"));
    tip_frame(t.text(k, "Tooltip Text").child(Keybind::new(&["⌘", "⌘"]).build(k)).build(k, layer))
}

fn tooltip_path(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let t = Tooltip::new("tip").trigger(body_text(k, "Hover me")).open(is_open(s, "tip"));
    let font = t.font(k);
    let parts = row()
        .gap(3.0)
        .child(text("Components").font(font).color(k.c.text_base))
        .child(text("/").font(font).color(k.c.text_faint))
        .child(text("Tooltip").font(font).color(k.c.text_base));
    tip_frame(t.child(parts).build(k, layer))
}

fn tooltip_title_description(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let t = Tooltip::new("tip").trigger(body_text(k, "Hover me")).open(is_open(s, "tip"));
    let font = t.font(k);
    tip_frame(
        t.child(text("Title").font(font).color(k.c.text_base))
            .child(text("·").font(font).color(k.c.text_faint))
            .child(text("Description").font(font).color(k.c.text_faint))
            .build(k, layer),
    )
}

fn hover_card_basic(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    row().child(
        HoverCard::new("card")
            .trigger(body_text(k, "Hover me"))
            .child(
                col()
                    .gap(6.0)
                    .child(text("Preview").font(TextStyle::new(13.0, 600.0).lh(24.0)).color(k.c.std_text))
                    .child(
                        text("Short supporting text.").font(TextStyle::new(12.0, 440.0).lh(24.0)).color(k.c.text_weak),
                    ),
            )
            .open(is_open(s, "card"))
            .build(k, layer),
    )
}

static DOWNLOAD: rune::Icon = rune::Icon {
    name: "story-download",
    view: 16.0,
    body: r#"<path d="M13.5554 10.4453V13.5564H11.7777H4.22211C3.23989 13.5564 2.44434 13.5564 2.44434 13.5564V10.4453" stroke="currentColor"/><path d="M4.88867 6L7.99978 9.11111L11.1109 6" stroke="currentColor"/><path d="M8 9.11198V2.44531" stroke="currentColor"/>"#,
};

fn toast_all(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let buttons = col()
        .gap(12.0)
        .items_start()
        .child(Button::new("Show download toast").on_press(|s: &mut G| s.open = Some("toast".into())).build(k))
        .child(Button::new("Show saved toast").build(k))
        .child(Button::new("Show saving toast").build(k))
        .child(Button::new("Show unsaved changes toast").build(k));
    if is_open(s, "toast") {
        let t = Toast::new("download", "Download started...")
            .description("23% · 2 min left")
            .icon(k, &DOWNLOAD)
            .closable()
            .action(k, "Run in background", true, |s: &mut G, _: &mut Tx| s.open = None)
            .action(k, "Cancel", false, |s: &mut G, _: &mut Tx| s.open = None)
            .build(k);
        layer.push(ToastRegion::new().toast(t).build(k));
    }
    buttons
}

// ---------------------------------------------------------------------------------------------
// Card, List, Accordion, Collapsible

fn card(k: &Kit, accent: Option<rune::Color>, icon: Option<&'static rune::Icon>) -> El {
    let mut c = Card::new()
        .title("Card title", icon)
        .description("Small supporting text.")
        .action(Button::new("Action").size(ButtonSize::Small).build(k));
    if let Some(a) = accent {
        c = c.accent(a);
    }
    c.build(k)
}

fn card_normal(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    card(k, None, None)
}
fn card_error(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    card(k, Some(k.c.danger.icon), Some(&ic::CIRCLE_BAN_SIGN_BOLD))
}
fn card_warning(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    card(k, Some(k.c.warning.icon), Some(&ic::WARNING_BOLD))
}
fn card_success(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    card(k, Some(k.c.success.icon), Some(&ic::CIRCLE_CHECK_BOLD))
}
fn card_info(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    card(k, Some(k.c.info.icon), Some(&ic::HELP_BOLD))
}

fn list(s: &G, items: &[&str], search: bool) -> List {
    let mut l = List::new();
    if search {
        l = l.search(&s.edits[3], "").on_search_edit(|s: &mut G| &mut s.edits[3]);
    }
    for i in items {
        l = l.item(ListItem::new(*i));
    }
    l
}

fn list_basic(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    list(s, &["One", "Two", "Three", "Four"], true).build(k)
}
fn list_grouped(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    List::new()
        .search(&s.edits[3], "")
        .item(ListItem::new("Alpha").group("Group A"))
        .item(ListItem::new("Bravo").group("Group A"))
        .item(ListItem::new("Delta").group("Group B"))
        .build(k)
}
fn list_empty(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    list(s, &[], true).build(k)
}
fn list_with_add(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    list(s, &["One", "Two"], true)
        .add_row(
            row()
                .px(8.0)
                .py(6.0)
                .child(text("Add item").font(TextStyle::new(14.0, 500.0).lh(21.0)).color(k.c.text_strong)),
        )
        .build(k)
}
fn list_divider(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    list(s, &["One", "Two", "Three"], false).divider().build(k)
}
fn list_active_icon(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    list(s, &["Alpha", "Beta", "Gamma"], false).active_icon(&ic::CHEVRON_RIGHT).build(k)
}
fn list_no_search(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    list(s, &["One", "Two", "Three"], false).build(k)
}

fn accordion_content(k: &Kit, s: &str, padded: bool) -> El {
    let t = text(s).font(TextStyle::new(13.0, 440.0).lh(24.0));
    if padded { row().py(8.0).child(t.color(k.c.text_weak)) } else { row().child(t.color(k.c.std_text)) }
}

/// The web story's `value="first"` arrives as a control arg and renders closed; so does this.
fn accordion_basic(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let open = s.picked("acc", "").to_owned();
    let pick = |v: &'static str| {
        move |s: &mut G| {
            let cur = s.picked("acc", "").to_owned();
            s.pick.insert("acc".into(), if cur == v { String::new() } else { v.into() });
        }
    };
    col().w(420.0).gap(8.0).child(
        Accordion::new()
            .item(
                AccordionItem::new("First")
                    .expanded(open == "first")
                    .child(accordion_content(k, "Accordion content.", true))
                    .on_toggle(pick("first")),
            )
            .item(
                AccordionItem::new("Second")
                    .expanded(open == "second")
                    .child(accordion_content(k, "More content.", true))
                    .on_toggle(pick("second")),
            )
            .build(k),
    )
}

fn accordion_multiple(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    Accordion::new()
        .item(
            AccordionItem::new("First")
                .expanded(!s.is("a1"))
                .child(accordion_content(k, "Accordion content.", false))
                .on_toggle(flip("a1")),
        )
        .item(
            AccordionItem::new("Second")
                .expanded(!s.is("a2"))
                .child(accordion_content(k, "More content.", false))
                .on_toggle(flip("a2")),
        )
        .build(k)
}

/// The web story passes `value="first"` without `multiple` or `collapsible` and renders closed.
fn accordion_single(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    Accordion::new()
        .item(AccordionItem::new("First").expanded(false).child(accordion_content(k, "Accordion content.", false)))
        .build(k)
}

fn collapsible_basic(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    Collapsible::new("Details", !s.is("c"))
        .child(
            row().pt(8.0).child(
                text("Optional details sit here.").font(TextStyle::new(13.0, 440.0).lh(24.0)).color(k.c.text_weak),
            ),
        )
        .on_toggle(flip("c"))
        .build(k)
}

fn collapsible_ghost(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    Collapsible::new("Ghost trigger", s.is("c"))
        .ghost()
        .child(
            row().pt(8.0).child(text("Ghost content.").font(TextStyle::new(13.0, 440.0).lh(24.0)).color(k.c.text_weak)),
        )
        .on_toggle(flip("c"))
        .build(k)
}

// ---------------------------------------------------------------------------------------------
// Badge, Keybind, Spinner, Loader, Progress, SegmentedControl

fn badge_basic(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(Badge::new("Label").build(k))
}
fn badge_appearances(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().gap(8.0).center_y().child(Badge::new("Standard").standard().build(k)).child(Badge::new("Compact").build(k))
}
fn badge_subtle(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row()
        .gap(8.0)
        .center_y()
        .child(Badge::new("1").variant(BadgeVariant::Subtle).build(k))
        .child(Badge::new("12").variant(BadgeVariant::Subtle).build(k))
}
fn badge_high_contrast(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().gap(8.0).center_y().child(Badge::new("Label").build(k)).child(Badge::new("Label").high_contrast().build(k))
}
fn badge_accent(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(Badge::new("New").variant(BadgeVariant::Accent).build(k))
}

fn keybind_playground(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(Keybind::new(&["⌘"]).build(k))
}
fn keybind_variants(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row()
        .gap(24.0)
        .center_y()
        .child(Keybind::new(&["⌘"]).build(k))
        .child(Keybind::new(&["⌘"]).variant(KeybindVariant::Ghost).build(k))
}
fn keybind_multiple(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row()
        .gap(24.0)
        .center_y()
        .child(Keybind::new(&["⌘", "K"]).build(k))
        .child(Keybind::new(&["⌘", "K"]).variant(KeybindVariant::Ghost).build(k))
        .child(Keybind::new(&["Ctrl", "B"]).build(k))
        .child(Keybind::new(&["Ctrl", "B"]).variant(KeybindVariant::Ghost).build(k))
}
fn keybind_all(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let line = |name: &str, v: KeybindVariant| {
        row()
            .gap(24.0)
            .center_y()
            .child(
                text(name)
                    .w(50.0)
                    .font(TextStyle::new(11.0, 440.0).lh(24.0))
                    .color(rune::theme::hex("#808080").unwrap_or_default()),
            )
            .child(Keybind::new(&["⌘"]).variant(v).build(k))
            .child(Keybind::new(&["⌘", "K"]).variant(v).build(k))
            .child(Keybind::new(&["⌘", "⇧", "P"]).variant(v).build(k))
            .child(Keybind::new(&["Ctrl", "Shift", "B"]).variant(v).build(k))
    };
    col().gap(16.0).child(line("Neutral", KeybindVariant::Neutral)).child(line("Ghost", KeybindVariant::Ghost))
}

/// Keyboard symbols next to capitals, in both families at 13 and 15 pt, then keycaps.
fn keybind_symbols(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    const SAMPLE: &str = "⌘K ⇧P ⏎ ⌫ ⎋ ⌥⌃ ←↑ HP";
    let mut c = col().gap(10.0);
    for mono in [false, true] {
        for size in [13.0, 15.0] {
            let font = TextStyle::new(size, 500.0).lh(size * 1.5);
            let font = if mono { font.mono() } else { font };
            let label = format!("{} {size}", if mono { "Plex Mono" } else { "Inter" });
            c = c.child(
                row()
                    .gap(16.0)
                    .center_y()
                    .child(text(label).w(96.0).font(TextStyle::new(11.0, 440.0).lh(16.0)).color(k.c.text_faint))
                    .child(text(SAMPLE).font(font).color(k.c.text_base)),
            );
        }
        let mut kk = k.clone();
        if mono {
            kk.ty.small_medium = kk.ty.small_medium.mono();
        }
        let mut caps = row().gap(12.0).center_y().child(
            text(if mono { "Plex Mono caps" } else { "Inter caps" })
                .w(96.0)
                .font(TextStyle::new(11.0, 440.0).lh(16.0))
                .color(k.c.text_faint),
        );
        let sets: [&[&str]; 6] = [&["⌘", "⇧", "P"], &["⇧", "⏎"], &["⌫"], &["⎋"], &["⌥", "⌃", "K"], &["←", "↑"]];
        for keys in sets {
            caps = caps.child(Keybind::new(keys).build(&kk));
        }
        c = c.child(caps);
    }
    c
}

fn spinner_basic(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(Spinner::new().build(k))
}
fn spinner_sizes(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row()
        .gap(16.0)
        .center_y()
        .child(Spinner::new().size(12.0).build(k))
        .child(Spinner::new().size(20.0).build(k))
        .child(Spinner::new().size(28.0).build(k))
}
fn loader_basic(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(Loader::new().build(k))
}
fn loader_sizes(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row()
        .gap(16.0)
        .center_y()
        .child(Loader::new().size(12.0).build(k))
        .child(Loader::new().build(k))
        .child(Loader::new().size(24.0).build(k))
}
fn progress_basic(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    Progress::new(Some(0.6)).label("Progress").show_value().build(k)
}
fn progress_no_label(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    Progress::new(Some(0.3)).build(k)
}
fn progress_indeterminate(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    Progress::new(None).label("Loading").build(k)
}

fn segmented_basic(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let sel = s.picked("seg", "0").parse().unwrap_or(0);
    row().child(
        SegmentedControl::new(&["Label", "Label", "Label", "Label"], Some(sel))
            .on_pick(|s: &mut G, i: usize, _: &mut Tx| {
                s.pick.insert("seg".into(), i.to_string());
            })
            .build(k),
    )
}
fn segmented_controlled(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let sel: usize = s.picked("seg", "1").parse().unwrap_or(1);
    col()
        .gap(12.0)
        .items_start()
        .child(
            SegmentedControl::new(&["List", "Grid", "Board"], Some(sel))
                .on_pick(|s: &mut G, i: usize, _: &mut Tx| {
                    s.pick.insert("seg".into(), i.to_string());
                })
                .build(k),
        )
        .child(
            text(format!("Value: {}", ["a", "b", "c"][sel.min(2)]))
                .font(TextStyle::new(12.0, 440.0).lh(24.0))
                .color(rune::theme::hex("#808080").unwrap_or_default()),
        )
}

#[allow(dead_code)]
fn spare() -> El {
    el()
}

/// Deadbolt's copy label and countdown, sans and mono. Each press of "Roll" advances one step:
/// "Copy password" → "Copied, clears in 0:30" → "… 0:29", and the countdown "0:30" → "0:29" → ….
fn rolling_text_basic(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let step: u32 = s.picked("roll", "0").parse().unwrap_or(0);
    let label = if step == 0 { "Copy password".to_owned() } else { format!("Copied, clears in 0:{:02}", 31 - step.min(31)) };
    let count = format!("0:{:02}", 30 - step.min(30));
    let big = TextStyle::new(28.0, 600.0);
    let line = |name: &str, e: El| row().gap(12.0).center_y().child(text(name).font(k.ty.small).color(k.c.text_faint).w(56.0)).child(e);
    col()
        .gap(14.0)
        .child(
            Button::new("Roll")
                .on_press(|s: &mut G| {
                    let n: u32 = s.picked("roll", "0").parse().unwrap_or(0);
                    s.pick.insert("roll".into(), (n + 1).to_string());
                })
                .build(k)
                .key("roll"),
        )
        .child(line("Sans", RollingText::new("roll-sans", label.clone()).font(k.ty.body).build(k)))
        .child(line("Mono", RollingText::new("roll-mono", label.clone()).font(k.ty.mono).build(k)))
        .child(line("Centred", row().w(260.0).center().child(RollingText::new("roll-centre", label).font(k.ty.body).build(k))))
        .child(line("Count", RollingText::new("count-sans", count.clone()).font(big).from(Edge::Top).build(k)))
        .child(line("", RollingText::new("count-mono", count).font(big.mono()).from(Edge::Top).build(k)))
}
