//! The stories, one function per `@opencode/ui` Storybook story id (title + export name).
//! Layout wrappers copy each story's own inline styles.

use rune::{Edge, El, Stagger, TextEdit, TextStyle, Transition, Tx, col, el, row, text};
use rune_kit::*;
use rune_theme_opencode::icons as ic;

use crate::G;

mod caret_browsing;
mod combobox;
mod controls;
mod diff;
mod display;
pub(crate) mod field_menu;

pub(crate) use diff::{custom_diff, sample_diff, scroll_bars_diff};
mod icons;
pub(crate) mod images;
mod index;
mod inputs;
mod link_hover;
mod link_popover;
mod markdown;
mod overlays;
pub(crate) mod sidebar;
mod split_view;
mod table;

use controls::{button_all_states, button_focus, button_icon, button_playground, button_shortcuts, button_sizes, button_variants, checkbox_basic, checkbox_custom_icon, checkbox_hidden_label, checkbox_states, icon_button_all_states, icon_button_playground, icon_button_sizes, icon_button_variants, radio_basic, radio_controlled, radio_states, radio_surfaces, slider_basic, slider_sizes, slider_states, switch_appearances, switch_basic, switch_hidden_label, switch_sizes, switch_states, switch_web};
use display::{a11y_aria_properties, accordion_basic, accordion_multiple, accordion_single, badge_accent, badge_appearances, badge_basic, badge_high_contrast, badge_subtle, card_error, card_info, card_normal, card_success, card_warning, collapsible_basic, collapsible_ghost, keybind_all, keybind_keep_case, keybind_multiple, keybind_playground, keybind_symbols, keybind_variants, list_active_icon, list_basic, list_delegated_a11y, list_divider, list_empty, list_grouped, list_no_search, list_with_add, loader_basic, loader_sizes, progress_basic, progress_indeterminate, progress_no_label, rolling_text_basic, scroll_view_thumb, segmented_basic, segmented_controlled, segmented_rich, spinner_basic, spinner_sizes};
use inputs::{field_direct_child, field_invalid, field_text_input, field_textarea, select_disabled, select_grouped, select_invalid, select_playground, select_states, select_step_closed, select_with_field, text_input_appearances, text_input_controlled, text_input_copy, text_input_hooks, text_input_playground, text_input_read_only, text_input_states, text_input_with_field, textarea_code_keys, textarea_composer, textarea_controlled, textarea_focus_caret, textarea_playground, textarea_states, textarea_with_field};
use markdown::{
    markdown_basic, markdown_code, markdown_diagrams, markdown_lenient, markdown_mermaid_charts, markdown_mermaid_data, markdown_mermaid_flows, markdown_mermaid_git, markdown_mermaid_hover, markdown_mermaid_large, markdown_mermaid_maps, markdown_mermaid_plan, markdown_mermaid_plan_zoomed, markdown_mermaid_shapes, markdown_mermaid_streaming, markdown_mermaid_systems, markdown_stream_tail, markdown_streaming, markdown_styled, markdown_table_styles,
    markdown_walkthrough, markdown_wrap_and_frames,
};
use field_menu::{menu_field_menu_fallback, menu_submenu_small_window};
use overlays::{focus_keyboard, hover_card_basic, hover_card_placements, menu_appearances, menu_basic, menu_checkbox, menu_radio, menu_rich, menu_submenu, menu_submenu_options, overlays_tx_callbacks, popover_basic, popover_bounded, popover_controlled, popover_wheel_outside, popup_list_rich_inline, toast_all, tooltip_help_arrow, tooltip_keybind, tooltip_path, tooltip_simple, tooltip_title_description};

pub(super) type Story = fn(&G, &Kit, &mut Vec<El>) -> El;

macro_rules! stories {
    ($($id:literal => $f:path,)*) => {
        pub const STORIES: &[(&str, Story)] = &[$(($id, $f),)*];
        pub const SCENES: &[(&str, fn(&mut G))] = &[$(($id, |s: &mut G| {
            s.story = $id;
            s.combo = ComboboxState::default();
            s.combo_animated = ComboboxState::default();
            s.combo_localized = ComboboxState::default();
            s.reorder = sidebar::ReorderDemo::default();
            if $id == "rune-combobox--searchable" { let _ = s.combo.step(ComboboxEvent::Toggle { current: None }); }
            if $id == "rune-combobox--animated-filter" { let _ = s.combo_animated.step(ComboboxEvent::Toggle { current: None }); }
            if $id == "rune-combobox--localized" { let _ = s.combo_localized.step(ComboboxEvent::Toggle { current: None }); }
            if $id == "rune-popup-list--composer" {
                s.edits[7] = TextEdit::new("/");
                s.edits[7].focused = true;
            }
            if $id == "rune-text--nowrap-input" {
                s.nowrap_code = TextEdit::new(NOWRAP_CODE);
                s.nowrap_code.focused = true;
            }
            if $id == "rune-text--align-y-input" {
                s.align_y_edits = [
                    TextEdit::new("Vertically centred single-line text"),
                    TextEdit::new(""),
                    TextEdit::new("Bottom-aligned (Align::End)"),
                    TextEdit::new("Default top-aligned (Align::Start)"),
                ];
                s.align_y_edits[0].focused = true;
                s.align_y_edits[0].set_caret(10);
            }
            if $id == "rune-textarea--composer" {
                s.edits[8] = TextEdit::new("Draft a release note in a borderless composer container.");
                s.edits[9] = TextEdit::new("feat(rune-kit): add plain, trailing, and scroll_bar builders to TextArea so long messages wrap clear of the trailing Commit button and scroll cleanly.");
                s.edits[10] = TextEdit::new("Line 1: Borderless plain() mode\nLine 2: Bottom-end trailing(El) slot\nLine 3: Built-in scroll_bar(true)\nLine 4: Custom scroll_bar_styled(style)\nLine 5: Works with auto_grow and max_height\nLine 6: Keeps focus while dragging the thumb\nLine 7: RTL inline-end placement\nLine 8: Accessible editor and trailing action");
            }
            if $id == "rune-text-input--read-only" {
                s.read_only_edits = [
                    TextEdit::new("https://api.example.com/v1/workspaces/rune"),
                    TextEdit::new("fn preview(input: &str) -> usize {\n    // Move with arrows, Shift+arrows, Home/End, or word steps.\n    input.lines().count()\n}"),
                ];
                s.read_only_edits[1].set_caret(3);
                s.read_only_edits[1].focused = true;
                s.pick.remove("ro-status");
            }
            if $id == "rune-textarea--code-keys" {
                s.edits[11] = TextEdit::new("fn greet(name: &str) {\n    println!(\"hello, {name}\");\n}");
                s.edits[11].set_caret(3);
                s.edits[11].focused = true;
            }
            if $id == "rune-textarea--focus-caret" {
                s.focus_caret_edit = TextEdit::new(focus_caret_sample());
            }
            if $id == "rune-split-view--flat-main" {
                s.split_sidebar.set_open(true);
                s.split_inspector.set_open(true);
            }
            if $id == "rune-table--options" {
                s.table = TableState::new().key("log-table").multi_select(true);
                s.table.select("log-15");
                s.table_rows = 16;
                s.table_menu = None;
            }
            if $id == "rune-table--wide" {
                s.table = TableState::new().key("wide-table");
                s.table.hide_column("region");
                s.table_menu = None;
            }
            if $id == "rune-table--overscan" {
                s.table = TableState::new().key("overscan-table");
                s.pick.insert("table-overscan".into(), "tight".into());
                s.table_menu = None;
            }
            if $id == "rune-table--virtual-columns" {
                s.table = TableState::new().key("vc-table");
                s.pick.insert("table-frozen".into(), "2".into());
                s.pick.remove("vc-last");
                s.table_menu = None;
            }
            if $id == "rune-image--decode" {
                s.images.enter(true);
            }
            if $id == "rune-tree--lazy-branches" {
                s.lazy_files = crate::lazy_files();
                s.lazy_loaded.clear();
            }
            if $id == "rune-focus--key-preview" {
                s.key_preview_edit = TextEdit::new("");
                s.key_preview_edit.focused = true;
                s.on.insert("kp-tip".into());
                s.on.insert("kp-list".into());
                s.pick.remove("kp-log");
            }
            if $id == "rune-menu--field-menu-fallback" {
                s.field_menu_edit = TextEdit::new("Select text, right-click or press Shift+F10");
                s.field_menu_edit.set_selection(0, 11);
                s.on.insert("fm-preview".into());
            }
            s.field_menu = None;
            s.tree_menu = None;
            s.tree_action = None;
            s.menu_at = None;
            s.link_popover = None;
            s.link_hover = None;
            s.open = OPENED.iter().find(|(id, _)| *id == $id).map(|(_, k)| (*k).to_owned());
        }),)*];
    };
}

/// Stories whose web version opens an overlay (by default or through the harness's click):
/// their scene starts with that overlay open.
pub(super) const OPENED: &[(&str, &str)] = &[
    ("ui-dialog--basic", "dialog"),
    ("rune-dialog--fit", "dialog-fit"),
    ("rune-dialog--close-disabled", "dialog-close-disabled"),
    ("rune-popup-list--composer", "popup-list"),
    ("ui-menu--basic", "menu-basic"),
    ("ui-menu--rich", "menu-rich"),
    ("ui-menu--with-checkbox", "menu-checkbox"),
    ("ui-menu--with-radio", "menu-radio"),
    ("ui-menu--with-submenu", "menu-submenu"),
    ("rune-menu--submenu-options", "menu-submenu-options"),
    ("ui-menu--appearances", "menu-appearances"),
    ("ui-select--playground", "select-playground"),
    ("ui-select--grouped", "select-grouped"),
    ("rune-select--states", "select-states"),
    ("components-popover--basic", "popover-basic"),
    ("components-popover--controlled", "popover-controlled"),
    ("rune-popover--bounded", "popover-bounded"),
    ("rune-popover--wheel-outside", "popover-wheel-outside"),
    ("components-hover-card--basic", "card"),
    ("rune-hover-card--placements", "hc-right-end"),
    ("rune-overlays--tx-callbacks", "tx-popover"),
    ("ui-tooltip--simple", "tip-simple"),
    ("ui-tooltip--with-keybind", "tip-keybind"),
    ("ui-tooltip--path", "tip-path"),
    ("ui-tooltip--title-description", "tip-title-description"),
    ("rune-tooltip--help-arrow", "help-arrow"),
    ("ui-toast--all-examples", "toast"),
];

fn is_open(s: &G, key: &str) -> bool {
    s.open.as_deref() == Some(key)
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
    "rune-combobox--localized" => combobox::localized,
    "ui-dialog--basic" => dialog_basic,
    "motion-focus-modal" => motion_focus_modal,
    "motion-stagger-list" => motion_stagger_list,
    "ui-button--playground" => button_playground,
    "ui-button--variants" => button_variants,
    "ui-button--sizes" => button_sizes,
    "ui-button--icon" => button_icon,
    "ui-button--all-states" => button_all_states,
    "rune-button--shortcuts" => button_shortcuts,
    "rune-button--focus" => button_focus,
    "ui-icon-button--playground" => icon_button_playground,
    "ui-icon-button--variants" => icon_button_variants,
    "ui-icon-button--sizes" => icon_button_sizes,
    "ui-icon-button--all-states" => icon_button_all_states,
    "ui-switch--basic" => switch_basic,
    "ui-switch--appearances" => switch_appearances,
    "ui-switch--states" => switch_states,
    "ui-switch--hidden-label" => switch_hidden_label,
    "rune-switch--web" => switch_web,
    "ui-checkbox--basic" => checkbox_basic,
    "ui-checkbox--states" => checkbox_states,
    "ui-checkbox--custom-icon" => checkbox_custom_icon,
    "ui-checkbox--hidden-label" => checkbox_hidden_label,
    "ui-radio--basic" => radio_basic,
    "ui-radio--controlled" => radio_controlled,
    "ui-radio--states" => radio_states,
    "ui-radio--surfaces" => radio_surfaces,
    "ui-text-input--playground" => text_input_playground,
    "ui-text-input--with-copy-button" => text_input_copy,
    "ui-text-input--controlled" => text_input_controlled,
    "ui-text-input--appearances" => text_input_appearances,
    "ui-text-input--with-field" => text_input_with_field,
    "ui-text-input--states" => text_input_states,
    "rune-text-input--hooks" => text_input_hooks,
    "rune-text-input--read-only" => text_input_read_only,
    "rune-text--caret-browsing" => caret_browsing::text_caret_browsing,
    "ui-textarea--playground" => textarea_playground,
    "ui-textarea--controlled" => textarea_controlled,
    "ui-textarea--with-field" => textarea_with_field,
    "ui-textarea--states" => textarea_states,
    "rune-textarea--composer" => textarea_composer,
    "rune-textarea--code-keys" => textarea_code_keys,
    "rune-textarea--focus-caret" => textarea_focus_caret,
    "ui-field--text-input-example" => field_text_input,
    "ui-field--text-input-direct-child" => field_direct_child,
    "ui-field--textarea-example" => field_textarea,
    "ui-field--invalid" => field_invalid,
    "ui-select--playground" => select_playground,
    "ui-select--grouped" => select_grouped,
    "ui-select--invalid" => select_invalid,
    "ui-select--disabled" => select_disabled,
    "ui-select--with-field" => select_with_field,
    "rune-select--states" => select_states,
    "rune-select--step-closed" => select_step_closed,
    "components-popover--basic" => popover_basic,
    "components-popover--controlled" => popover_controlled,
    "rune-popover--bounded" => popover_bounded,
    "rune-popover--wheel-outside" => popover_wheel_outside,
    "ui-menu--basic" => menu_basic,
    "ui-menu--appearances" => menu_appearances,
    "ui-menu--rich" => menu_rich,
    "ui-menu--with-checkbox" => menu_checkbox,
    "ui-menu--with-radio" => menu_radio,
    "ui-menu--with-submenu" => menu_submenu,
    "rune-menu--submenu-options" => menu_submenu_options,
    "rune-menu--submenu-small-window" => menu_submenu_small_window,
    "rune-menu--field-menu-fallback" => menu_field_menu_fallback,
    "ui-tooltip--simple" => tooltip_simple,
    "ui-tooltip--with-keybind" => tooltip_keybind,
    "ui-tooltip--path" => tooltip_path,
    "ui-tooltip--title-description" => tooltip_title_description,
    "rune-tooltip--help-arrow" => tooltip_help_arrow,
    "components-hover-card--basic" => hover_card_basic,
    "rune-hover-card--placements" => hover_card_placements,
    "rune-overlays--tx-callbacks" => overlays_tx_callbacks,
    "rune-focus--keyboard" => focus_keyboard,
    "rune-focus--key-preview" => combobox::key_preview,
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
    "rune-segmented-control--rich" => segmented_rich,
    "rune-keybind--symbols" => keybind_symbols,
    "rune-keybind--keep-case" => keybind_keep_case,
    "rune-switch--sizes" => switch_sizes,
    "rune-slider--basic" => slider_basic,
    "rune-slider--sizes" => slider_sizes,
    "rune-slider--states" => slider_states,
    "rune-a11y--aria-properties" => a11y_aria_properties,
    "rune-list--delegated-a11y" => list_delegated_a11y,
    "rune-scroll-view--thumb" => scroll_view_thumb,
    "rune-rolling-text--basic" => rolling_text_basic,
    "rune-markdown--basic" => markdown_basic,
    "rune-markdown--link-hover" => link_hover::markdown_link_hover,
    "rune-markdown--link-popover" => link_popover::markdown_link_popover,
    "rune-markdown--streaming" => markdown_streaming,
    "rune-markdown--stream-tail" => markdown_stream_tail,
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
    "rune-markdown--mermaid-streaming" => markdown_mermaid_streaming,
    "rune-markdown--mermaid-plan" => markdown_mermaid_plan,
    "rune-markdown--mermaid-plan-zoomed" => markdown_mermaid_plan_zoomed,
    "rune-markdown--mermaid-large" => markdown_mermaid_large,
    "rune-markdown--table-styles" => markdown_table_styles,
    "rune-markdown--code" => markdown_code,
    "rune-markdown--styled" => markdown_styled,
    "rune-markdown--wrap-and-frames" => markdown_wrap_and_frames,
    "rune-text--selectable-help" => text_selectable_help,
    "rune-text--truncate-middle" => text_truncate_middle,
    "rune-text--max-lines-elide" => text_max_lines_elide,
    "rune-text--nowrap-input" => text_nowrap_input,
    "rune-text--align-y-input" => text_align_y_input,
    "rune-dialog--fit" => dialog_fit,
    "rune-dialog--close-disabled" => dialog_close_disabled,
    "rune-popup-list--composer" => popup_list_composer,
    "rune-popup-list--rich-inline" => popup_list_rich_inline,
    "rune-scroll-bar--basic" => scroll_bar_basic,
    "rune-scroll-bar--grow" => scroll_bar_grow,
    "rune-scroll-bar--styled" => scroll_bar_styled,
    "rune-scroll-bar--xy" => scroll_bar_xy,
    "rune-sidebar--reorder" => sidebar::reorder,
    "components-file-icon--samples" => icons::file_icon_samples,
    "components-file-icon--folder" => icons::file_icon_folder,
    "ui-icon--gallery" => icons::icon_gallery,
    "rune-icon--catalog" => icons::icon_catalog,
    "rune-file-icon--catalog" => icons::file_icon_catalog,
    "rune-tree-icon--catalog" => icons::tree_icon_catalog,
    "rune-tree--files" => icons::file_tree,
    "rune-tree--scroll-menu" => icons::file_tree_scroll_menu,
    "rune-tree--lazy-branches" => icons::file_tree_lazy_branches,
    "rune-split-view--flat-main" => split_view::flat_main,
    "rune-table--options" => table::options,
    "rune-table--wide" => table::wide,
    "rune-table--overscan" => table::overscan,
    "rune-table--virtual-columns" => table::virtual_columns,
    "rune-diff--localized" => diff::localized,
    "rune-diff--custom" => diff::custom,
    "rune-diff--scroll-bars" => diff::scroll_bars,
    "rune-image--decode" => images::decode,
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

fn dialog_close_disabled(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let saving = !s.is("dialog-close-enabled");
    let label = if saving { "Autoriser la fermeture" } else { "Bloquer la fermeture" };
    let actions = row().w_full().padding(16.0).gap(8.0).justify_end()
        .child(Button::new(label).variant(ButtonVariant::Ghost).on_press(flip("dialog-close-enabled")).build(k).key("toggle-close"))
        .child(Button::new("Fermer").disabled(saving).on_press(close).build(k).key("confirm"));
    Dialog::new("dialog-close-disabled", "Enregistrer les modifications")
        .description(if saving { "Patientez pendant l'enregistrement." } else { "Enregistrement terminé." })
        .width(400.0).fit()
        .open(is_open(s, "dialog-close-disabled"))
        .close_label("Fermer")
        .close_disabled(saving)
        .on_close(close)
        .child(actions)
        .build(k, layer);
    Button::new("Ouvrir le dialogue").on_press(toggle("dialog-close-disabled")).build(k).key("opener")
}

fn motion_focus_modal(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    Dialog::new("motion-dialog", "Loading").open(is_open(s, "motion-dialog")).on_close(close)
        .child(row().gap(16.0).child(Spinner::new().build(k).key("spinner"))
            .child(Loader::new().build(k).key("loader")))
        .child(Button::new("Done").on_press(close).build(k).key("done")).build(k, layer);
    col().gap(16.0)
        .child(Button::new("Open dialog").on_press(toggle("motion-dialog")).build(k).key("opener"))
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

pub fn render(s: &G, k: &'static Kit, overlays: &mut Vec<El>) -> El {
    match STORIES.iter().find(|(id, _)| *id == s.story) {
        Some((_, f)) => f(s, k, overlays),
        None => index::index(s, k, overlays),
    }
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

const LONG_FILENAME: &str = "Quarterly report final revised version two of the budget.xlsx";

/// Two-line filename cards comparing `max_lines(2).elide_middle()`, `max_lines(2).elide_head()`,
/// and default end elision: middle elision keeps both the opening words and the `.xlsx` suffix.
fn text_max_lines_elide(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let c = &k.c;
    let card = |title: &str, label: &str, w: f32, build: fn(El) -> El| {
        col()
            .w(w)
            .padding(10.0)
            .gap(6.0)
            .radius(k.radius.md)
            .fill(c.surface_base)
            .border(1.0, c.border_weak)
            .child(text(title).font(k.ty.caption()).color(c.text_weak))
            .child(
                build(text(label).font(k.ty.ui).color(c.text_base))
                    .selectable(true)
                    .selection_color(k.selection()),
            )
    };
    row()
        .gap(12.0)
        .flex_wrap()
        .child(card("2 lines · elide_middle", LONG_FILENAME, 160.0, |e| e.max_lines(2).elide_middle()))
        .child(card("2 lines · elide_head", LONG_PATH, 180.0, |e| e.wrap_anywhere().max_lines(2).elide_head()))
        .child(card("2 lines · default (end)", LONG_FILENAME, 160.0, |e| e.max_lines(2)))
}

pub(crate) fn focus_caret_sample() -> String {
    (1..=30)
        .map(|i| format!("Line {i:02}: multi-line entry with caret parked at the end."))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) const NOWRAP_CODE: &str = concat!(
    "pub fn render_file_entry(name: &str, max_width: f32, elide_middle: bool) -> El {\n",
    "    let label = text(name).w(max_width).max_lines(2);\n",
    "    if elide_middle { label.elide_middle() } else { label }\n",
    "}\n",
    "// Long unwrapped line that extends past the viewport width so scroll_xy scrolls sideways under the caret.\n",
    "fn main() { println!(\"hello from an unwrapped multiline text_input\"); }",
);

/// An unwrapped multi-line code `text_input` (`El::nowrap`) inside a `scroll_xy` viewport:
/// long lines keep their natural width and scroll sideways as the caret moves.
fn text_nowrap_input(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let c = &k.c;
    let border = if s.nowrap_code.focused { c.border_strong } else { c.border_weak };
    col()
        .gap(6.0)
        .child(caption(k, "Unwrapped multi-line text_input (.nowrap() inside .scroll_xy())"))
        .child(
            col()
                .key("nowrap-viewport")
                .w(340.0)
                .h(140.0)
                .radius(k.radius.md)
                .fill(c.surface_base)
                .border(1.0, border)
                .scroll_xy()
                .child(
                    rune::text_input(&s.nowrap_code, k.ty.mono)
                        .key("nowrap-code")
                        .label("Code editor")
                        .nowrap()
                        .padding(10.0)
                        .color(c.text_base)
                        .caret(c.text_base, k.selection())
                        .on_edit(|s: &mut G| &mut s.nowrap_code)
                        .on_submit(|s: &mut G, _| {
                            let _ = s.nowrap_code.insert("\n");
                        }),
                )
                .scroll_bar_styled(ScrollBarStyle::from_kit(k)),
        )
}

/// Tall 48 pt single-line `text_input` fields showing opt-in `El::align_y`: vertically centred
/// text + caret (`Align::Center`), centred placeholder, bottom-aligned (`Align::End`), and the
/// default top-aligned layout (`Align::Start`).
fn text_align_y_input(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let c = &k.c;
    let st = TextStyle::new(14.0, 400.0).lh(20.0);
    let field = |key: &'static str, label: &'static str, edit: &TextEdit, placeholder: &'static str, align: rune::Align, idx: usize| {
        let border = if edit.focused { c.border_strong } else { c.border_weak };
        col()
            .gap(4.0)
            .child(caption(k, label))
            .child(
                rune::text_input(edit, st)
                    .single_line()
                    .key(key)
                    .label(label)
                    .w(340.0)
                    .h(48.0)
                    .px(12.0)
                    .align_y(align)
                    .radius(k.radius.md)
                    .fill(c.surface_base)
                    .border(1.0, border)
                    .color(c.text_base)
                    .placeholder(placeholder, c.text_faint)
                    .caret(c.text_base, k.selection())
                    .on_edit(rune::edit_access(move |s: &mut G| &mut s.align_y_edits[idx])),
            )
    };
    col()
        .gap(12.0)
        .child(field("align-y-center", "48 pt single_line · align_y(Align::Center) (text + caret)", &s.align_y_edits[0], "Type here…", rune::Align::Center, 0))
        .child(field("align-y-placeholder", "48 pt single_line · align_y(Align::Center) (placeholder)", &s.align_y_edits[1], "Search notes…", rune::Align::Center, 1))
        .child(field("align-y-end", "48 pt single_line · align_y(Align::End)", &s.align_y_edits[2], "Bottom-aligned…", rune::Align::End, 2))
        .child(field("align-y-start", "48 pt single_line · default (Align::Start)", &s.align_y_edits[3], "Top-aligned…", rune::Align::Start, 3))
}

fn scroll_bar_basic(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    scroll_feed(k, 24, None)
}

/// A feed whose button lengthens or shortens what follows its rows, so its thumb glides to each
/// new length.
fn scroll_bar_grow(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    col().gap(8.0)
        .child(Button::new("Change rows").on_press(flip("more")).build(k).key("more"))
        .child(scroll_feed(k, 12, Some(if s.is("more") { 600.0 } else { 0.0 })))
}

/// An always-visible thin scroll bar with custom `track_inset`, `min_thumb`, and `track` /
/// `track_hover` trough fills.
fn scroll_bar_styled(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let style = ScrollBarStyle::native(k.c.border_base, k.c.border_strong)
        .inside()
        .always_visible()
        .track_inset(8.0)
        .min_thumb(40.0)
        .track(k.c.layer_02)
        .track_hover(k.c.surface_hover);
    scroll_feed_with(k, 24, None, style)
}

/// A two-axis `scroll_xy` viewport with both vertical and horizontal scroll bars leaving the
/// bottom-end corner clear.
fn scroll_bar_xy(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let always = s.is("xy-always");
    let grid = col().w(760.0).padding(8.0).gap(4.0).children((0..24).map(|r| {
        row()
            .key(format!("xy-row-{r}"))
            .w_full()
            .h(30.0)
            .px(10.0)
            .gap(16.0)
            .center_y()
            .radius(k.radius.sm)
            .fill(if r % 2 == 0 { k.c.layer_02 } else { k.c.layer_01 })
            .children((0..6).map(|c| {
                text(format!("r{:02} · col {} · payload_{}_{}", r + 1, c + 1, r, c))
                    .key(format!("cell-{r}-{c}"))
                    .font(k.ty.mono)
                    .color(if c == 0 { k.c.text_base } else { k.c.text_muted })
                    .single_line()
            }))
    }));
    let viewport = col()
        .key("xy-feed")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll_xy()
        .child(grid);
    let viewport = if always {
        let style = ScrollBarStyle::from_kit(k).always_visible();
        viewport.scroll_bars_xy_styled(style, style.horizontal())
    } else {
        viewport.scroll_bars_xy(k)
    };
    col()
        .key("xy-story")
        .gap(10.0)
        .child(
            Switch::new(always)
                .label("Always visible")
                .on_toggle(|s: &mut G| s.flip("xy-always"))
                .build(k)
                .key("xy-always-toggle"),
        )
        .child(
            col()
                .key("xy-panel")
                .w(420.0)
                .h(260.0)
                .radius(k.radius.md)
                .fill(k.c.layer_01)
                .border(1.0, k.c.border_weak)
                .clip_soft(k.radius.md)
                .child(viewport),
        )
}

fn scroll_feed(k: &Kit, rows: usize, tail: Option<f32>) -> El {
    scroll_feed_with(k, rows, tail, ScrollBarStyle::from_kit(k))
}

fn scroll_feed_with(k: &Kit, rows: usize, tail: Option<f32>, style: ScrollBarStyle) -> El {
    col()
        .key("panel")
        .w(360.0)
        .h(260.0)
        .radius(k.radius.md)
        .fill(k.c.layer_01)
        .border(1.0, k.c.border_weak)
        .clip_soft(k.radius.md)
        .child(
            col()
                .key("feed")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .padding(8.0)
                .gap(4.0)
                .scroll(rune::ScrollSpec { stick_bottom: false })
                .children((0..rows).map(|i| {
                    row()
                        .key(format!("item-{i}"))
                        .w_full()
                        .h(32.0)
                        .px(10.0)
                        .center_y()
                        .justify_between()
                        .radius(k.radius.sm)
                        .child(text(format!("Build step {:02}", i + 1)).font(k.ty.ui_compact).color(k.c.text_base))
                        .child(text(format!("{} ms", 12 + i * 7)).font(k.ty.small).color(k.c.text_muted))
                }))
                .children(tail.map(|h| el().key("tail").w_full().h(h)))
                .scroll_bar_styled(style),
        )
}
