//! The stories, one function per `@opencode/ui` Storybook story id (title + export name).
//! Layout wrappers copy each story's own inline styles.

use rune::{Edge, El, Stagger, TextEdit, TextStyle, Transition, Tx, col, row, text};
use rune_kit::*;
use rune_theme_opencode::icons as ic;

use crate::G;

mod combobox;
mod controls;
mod display;
mod inputs;
mod markdown;
mod overlays;

use controls::{button_all_states, button_icon, button_playground, button_sizes, button_variants, checkbox_basic, checkbox_custom_icon, checkbox_hidden_label, checkbox_states, icon_button_all_states, icon_button_playground, icon_button_sizes, icon_button_variants, radio_basic, radio_controlled, radio_states, switch_appearances, switch_basic, switch_hidden_label, switch_sizes, switch_states};
use display::{accordion_basic, accordion_multiple, accordion_single, badge_accent, badge_appearances, badge_basic, badge_high_contrast, badge_subtle, card_error, card_info, card_normal, card_success, card_warning, collapsible_basic, collapsible_ghost, keybind_all, keybind_multiple, keybind_playground, keybind_symbols, keybind_variants, list_active_icon, list_basic, list_divider, list_empty, list_grouped, list_no_search, list_with_add, loader_basic, loader_sizes, progress_basic, progress_indeterminate, progress_no_label, rolling_text_basic, segmented_basic, segmented_controlled, spinner_basic, spinner_sizes};
use inputs::{field_direct_child, field_invalid, field_text_input, field_textarea, select_disabled, select_grouped, select_invalid, select_playground, select_with_field, text_input_appearances, text_input_controlled, text_input_copy, text_input_playground, text_input_states, text_input_with_field, textarea_controlled, textarea_playground, textarea_states, textarea_with_field};
use markdown::{
    markdown_basic, markdown_code, markdown_diagrams, markdown_lenient, markdown_mermaid_charts, markdown_mermaid_data, markdown_mermaid_flows, markdown_mermaid_git, markdown_mermaid_hover, markdown_mermaid_maps, markdown_mermaid_shapes, markdown_mermaid_systems, markdown_streaming, markdown_table_styles,
    markdown_walkthrough,
};
use overlays::{hover_card_basic, menu_appearances, menu_basic, menu_checkbox, menu_radio, menu_rich, menu_submenu, popover_basic, popover_controlled, toast_all, tooltip_keybind, tooltip_path, tooltip_simple, tooltip_title_description};

type Story = fn(&G, &Kit, &mut Vec<El>) -> El;

macro_rules! stories {
    ($($id:literal => $f:path,)*) => {
        pub const STORIES: &[(&str, Story)] = &[$(($id, $f),)*];
        pub const SCENES: &[(&str, fn(&mut G))] = &[$(($id, |s: &mut G| {
            s.story = $id;
            s.combo = ComboboxState::default();
            if $id.starts_with("rune-combobox--") { s.combo.open = true; s.combo.search.focused = true; }
            if $id == "rune-popup-list--composer" {
                s.edits[7] = TextEdit::new("/");
                s.edits[7].focused = true;
            }
            s.open = OPENED.iter().find(|(id, _)| *id == $id).map(|(_, k)| (*k).to_owned());
        }),)*];
    };
}

/// Stories whose web version opens an overlay (by default or through the harness's click):
/// their scene starts with that overlay open.
const OPENED: &[(&str, &str)] = &[
    ("ui-dialog--basic", "dialog"),
    ("rune-dialog--fit", "dialog-fit"),
    ("rune-popup-list--composer", "popup-list"),
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

fn flip(key: &'static str) -> impl Fn(&mut G) {
    move |s: &mut G| s.flip(key)
}

stories! {
    "rune-combobox--searchable" => combobox::searchable,
    "rune-combobox--animated-filter" => combobox::animated_filter,
    "ui-dialog--basic" => dialog_basic,
    "motion-focus-modal" => motion_focus_modal,
    "motion-stagger-list" => motion_stagger_list,
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
    "rune-switch--sizes" => switch_sizes,
    "rune-rolling-text--basic" => rolling_text_basic,
    "rune-markdown--basic" => markdown_basic,
    "rune-markdown--streaming" => markdown_streaming,
    "rune-markdown--walkthrough" => markdown_walkthrough,
    "rune-markdown--diagrams" => markdown_diagrams,
    "rune-markdown--lenient" => markdown_lenient,
    "rune-markdown--mermaid-shapes" => markdown_mermaid_shapes,
    "rune-markdown--mermaid-charts" => markdown_mermaid_charts,
    "rune-markdown--mermaid-maps" => markdown_mermaid_maps,
    "rune-markdown--mermaid-git" => markdown_mermaid_git,
    "rune-markdown--mermaid-data" => markdown_mermaid_data,
    "rune-markdown--mermaid-systems" => markdown_mermaid_systems,
    "rune-markdown--mermaid-flows" => markdown_mermaid_flows,
    "rune-markdown--mermaid-hover" => markdown_mermaid_hover,
    "rune-markdown--table-styles" => markdown_table_styles,
    "rune-markdown--code" => markdown_code,
    "rune-text--selectable-help" => text_selectable_help,
    "rune-text--truncate-middle" => text_truncate_middle,
    "rune-dialog--fit" => dialog_fit,
    "rune-popup-list--composer" => popup_list_composer,
}

fn popup_list_composer(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let entries = [
        ("review", "/review", "Review branch changes", &glyphs::SPARKLE, "⏎", false),
        ("compact", "/compact", "Summarize the conversation", &glyphs::FILE_TEXT, "⌘K", false),
        ("search", "/search", "Search workspace files", k.glyph.search, "⌘P", false),
        ("share", "/share", "Create a read-only link", k.glyph.lock, "", true),
    ];
    let q = s.edits[7].text.trim_start_matches('/').to_lowercase();
    let filtered: Vec<_> = entries.into_iter().filter(|(_, label, desc, ..)| {
        q.is_empty() || label.to_lowercase().contains(&q) || desc.to_lowercase().contains(&q)
    }).collect();
    let count = filtered.len();
    let active = s.pick.get("popup-list").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0).min(count.saturating_sub(1));
    let items = filtered.into_iter().map(|(id, label, desc, glyph, hint, disabled)| {
        let mut item = PopupItem::new(id, label)
            .description(desc)
            .leading(rune::icon(glyph, 14.0).color(k.c.icon_muted))
            .disabled(disabled);
        if !hint.is_empty() {
            item = item.trailing(text(hint).font(k.ty.small_medium).color(k.c.text_faint));
        }
        item
    });
    PopupList::new("popup-list", "composer")
        .open(is_open(s, "popup-list"))
        .width(320.0)
        .items(items)
        .active((count > 0).then_some(active))
        .on_active(|s: &mut G, i| { s.pick.insert("popup-list".into(), i.to_string()); })
        .on_choose(|s: &mut G, id, _| {
            s.edits[7] = TextEdit::new(format!("/{id} "));
            s.edits[7].focused = true;
            s.open = None;
        })
        .build(k, layer);
    TextInput::new(&s.edits[7])
        .width(Some(320.0))
        .placeholder("Type / for commands or @ to mention…")
        .animate_edit_layout()
        .on_edit(|s: &mut G| &mut s.edits[7])
        .on_change(|s: &mut G, _| { s.open = Some("popup-list".into()); s.pick.insert("popup-list".into(), "0".into()); })
        .build(k)
        .key("composer")
}

fn dialog_basic(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    Dialog::new("dialog", "Dialog").description("Description").open(is_open(s, "dialog"))
        .on_close(close).child(body_text(k, "Dialog body content.")).build(k, layer);
    Button::new("Open dialog").on_press(toggle("dialog")).build(k).key("opener")
}

fn dialog_fit(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let actions = row().w_full().padding(16.0).gap(8.0).justify_end()
        .child(Button::new("Cancel").variant(ButtonVariant::Ghost).on_press(close).build(k))
        .child(Button::new("Delete").on_press(close).build(k));
    Dialog::new("dialog-fit", "Delete item?").description("This cannot be undone.").width(400.0).fit()
        .open(is_open(s, "dialog-fit")).on_close(close).child(actions).build(k, layer);
    Button::new("Open confirmation").on_press(toggle("dialog-fit")).build(k).key("opener")
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

/// Five rows that rise and fade in one after another (`El::enter_delay` with a `Stagger`) and
/// leave together.
fn motion_stagger_list(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    const ROWS: [&str; 5] = ["Inbox", "Drafts", "Sent", "Archive", "Trash"];
    let stagger = Stagger::new(0.05);
    let rows = ROWS.iter().enumerate().filter(|_| s.is("rows")).map(|(i, label)| {
        row().key(*label).w(240.0).h(36.0).px(12.0).center_y().fill(k.c.surface_base).radius(k.radius.md)
            .child(text(*label).font(k.ty.ui).color(k.c.text_base))
            .transition(Transition::offset(0.0, 12.0).combined(Transition::opacity()))
            .enter_delay(stagger.delay(i, ROWS.len()))
    });
    col().gap(8.0)
        .child(Button::new("Show rows").on_press(|s: &mut G| s.flip("rows")).build(k).key("opener"))
        .child(col().key("rows").gap(4.0).children(rows))
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
// Selectable text and help

const SERVE: &str = "opencode serve --port 4096 --hostname 0.0.0.0";

/// A settings row: a command people can select and copy, highlighted as the kit's fields
/// highlight a selection, and an icon-only copy button that explains itself on hover.
fn text_selectable_help(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let c = &k.c;
    col().w(480.0).gap(8.0)
        .child(caption(k, "Run the server from a terminal"))
        .child(
            row().key("command-row").h(36.0).pl(12.0).pr(4.0).gap(8.0).center_y()
                .fill(c.surface_base).radius(k.radius.md).border(1.0, c.border_weak)
                .child(text(SERVE).key("command").font(k.ty.mono).color(c.text_base).grow(1.0).shrink()
                    .selectable(true).selection_color(k.selection()))
                .child(IconButton::new(&ic::COPY).variant(IconButtonVariant::Ghost).control_size(ControlSize::Small)
                    .label("Copy command").on_press_tx(|_: &mut G, tx: &mut Tx| tx.copy(SERVE)).build(k)
                    .key("copy").help("Copy command")),
        )
        .child(row().w(280.0).child(TextInput::new(&s.edits[7]).placeholder("Selected like a field")
            .on_edit(|s: &mut G| &mut s.edits[7]).build(k).key("field")))
}

const LONG_PATH: &str = "~/Projects/rune/crates/rune-kit/src/components/settings/appearance_panel.rs";

/// A path keeps its folder and file name as its box narrows, beside the end and head cuts, each
/// with a `Type::caption` line under it.
fn text_truncate_middle(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let c = &k.c;
    let path = |w: f32, cut: fn(El) -> El, how: &str| col().w(w)
        .child(cut(text(LONG_PATH).font(k.ty.ui).color(c.text_base)).selectable(true).selection_color(k.selection()))
        .child(text(format!("{how}, {w} pt")).font(k.ty.caption()).color(c.text_weak));
    col().gap(12.0)
        .child(path(360.0, El::truncate_middle, "Middle"))
        .child(path(240.0, El::truncate_middle, "Middle"))
        .child(path(160.0, El::truncate_middle, "Middle"))
        .child(path(240.0, El::truncate, "End"))
        .child(path(240.0, El::truncate_head, "Head"))
}
