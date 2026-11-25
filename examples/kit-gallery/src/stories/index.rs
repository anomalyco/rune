//! Virtualized scrollable index of every story, building only rows in and near the viewport.

use std::borrow::Cow;

use rune::{El, GrowthSpec, ScrollSpec, col};
use rune_kit::Kit;

use crate::G;

use super::{
    STORIES, Story, caption, combobox, dialog_basic, dialog_close_disabled, dialog_fit, field_menu, icons,
    inputs, link_hover, link_popover, motion_focus_modal, overlay_metrics, overlays, popup_list_composer, table,
};

/// Every story stacked in a scrollable virtualized list, for browsing in a window.
/// Only rows in and near the viewport are built and mounted; open overlays are collected
/// separately so they still mount above the scroll container under `kit-root`.
pub(super) fn index(s: &G, k: &'static Kit, layer: &mut Vec<El>) -> El {
    collect_open_overlays(s, k, layer);
    let rows = col()
        .key("index")
        .w_full()
        .padding(24.0)
        .gap(24.0)
        .growth(GrowthSpec::default())
        .virtual_rows(
            STORIES.len(),
            120.0,
            |_: &G, i| Cow::Borrowed(STORIES[i].0),
            move |s: &G, i| {
                let (id, f) = STORIES[i];
                let mut discard = Vec::new();
                col().w_full().gap(8.0).child(caption(k, id)).child(f(s, k, &mut discard))
            },
        );
    col()
        .key("index-scroll")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(rows)
}

fn collect_open_overlays(s: &G, k: &Kit, layer: &mut Vec<El>) {
    if s.combo.is_open() { let _ = combobox::searchable(s, k, layer); }
    if s.combo_animated.is_open() { let _ = combobox::animated_filter(s, k, layer); }
    if s.combo_localized.is_open() { let _ = combobox::localized(s, k, layer); }
    if s.is("kp-tip") { let _ = combobox::key_preview(s, k, layer); }
    if s.is("fm-preview") && s.field_menu.is_none() { let _ = field_menu::menu_field_menu_fallback(s, k, layer); }
    if s.menu_at.is_some() { let _ = inputs::text_input_hooks(s, k, layer); }
    if s.link_hover.is_some() { let _ = link_hover::markdown_link_hover(s, k, layer); }
    if s.link_popover.is_some() { let _ = link_popover::markdown_link_popover(s, k, layer); }
    if let Some((which, _, _)) = &s.tree_menu {
        match *which {
            "files" => { let _ = icons::file_tree(s, k, layer); }
            "scroll" => { let _ = icons::file_tree_scroll_menu(s, k, layer); }
            _ => {}
        }
    }
    if let Some((which, _, _)) = &s.table_menu {
        match *which {
            "options" => { let _ = table::options(s, k, layer); }
            "wide" => { let _ = table::wide(s, k, layer); }
            _ => {}
        }
    }
    if let Some(open) = s.open.as_deref() {
        let story: Option<Story> = match open {
            "popup-list" => Some(popup_list_composer),
            "dialog" => Some(dialog_basic),
            "dialog-fit" => Some(dialog_fit),
            "dialog-close-disabled" => Some(dialog_close_disabled),
            "motion-dialog" => Some(motion_focus_modal),
            "select-playground" => Some(inputs::select_playground),
            "select-grouped" => Some(inputs::select_grouped),
            "select-invalid" => Some(inputs::select_invalid),
            "select-states" => Some(inputs::select_states),
            "select-with-field" => Some(inputs::select_with_field),
            "step-closed" => Some(inputs::select_step_closed),
            "popover-basic" => Some(overlays::popover_basic),
            "popover-controlled" => Some(overlays::popover_controlled),
            "popover-bounded" => Some(overlays::popover_bounded),
            "popover-wheel-outside" => Some(overlays::popover_wheel_outside),
            "menu-basic" => Some(overlays::menu_basic),
            "menu-appearances" => Some(overlays::menu_appearances),
            "menu-rich" => Some(overlays::menu_rich),
            "menu-checkbox" => Some(overlays::menu_checkbox),
            "menu-radio" => Some(overlays::menu_radio),
            "menu-submenu" => Some(overlays::menu_submenu),
            "menu-submenu-options" => Some(overlays::menu_submenu_options),
            "tip-simple" => Some(overlays::tooltip_simple),
            "tip-keybind" => Some(overlays::tooltip_keybind),
            "tip-path" => Some(overlays::tooltip_path),
            "tip-title-description" => Some(overlays::tooltip_title_description),
            "card" => Some(overlays::hover_card_basic),
            "hc-left-start" | "hc-left-end" | "hc-right-end" => Some(overlays::hover_card_placements),
            "om-menu" | "om-select" | "om-popover" | "om-dialog" => Some(overlay_metrics::overlays_metrics),
            "tx-popover" | "tx-select" => Some(overlays::overlays_tx_callbacks),
            "toast" => Some(overlays::toast_all),
            _ => None,
        };
        if let Some(f) = story {
            let _ = f(s, k, layer);
        }
    }
}
