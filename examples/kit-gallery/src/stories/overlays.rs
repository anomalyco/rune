//! Popover, Menu, Tooltip, HoverCard and Toast.

use super::*;

fn trigger_text(k: &Kit, s: &str) -> El {
    body_text(k, s)
}

pub(super) fn popover_basic(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
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

pub(super) fn popover_controlled(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
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

/// A popover card clamped inside a list pane via `El::anchored_in` beside an adjacent drawer,
/// displaying its resolved `anchor_placement` as the trigger moves across the pane.
pub(super) fn popover_bounded(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let open = is_open(s, "popover-bounded");
    let at_left = s.is("bounded-left");
    let resolved = rune::memo::anchor_placement("bounded-card");
    let placement_label = match resolved {
        Some((side, align)) => format!("Resolved: {side:?} / {align:?}"),
        None => "Resolved: closed".to_owned(),
    };
    if open {
        let card = popover::raised(
            k,
            col()
                .w(180.0)
                .padding(12.0)
                .gap(4.0)
                .child(text("Row details").font(k.ty.ui_medium).color(k.c.text_strong))
                .child(text("Authored: Right / Start").font(k.ty.small).color(k.c.text_weak))
                .child(text(placement_label.clone()).key("bounded-placement").font(k.ty.small_medium).color(k.c.text_base)),
            k.radius.md,
        )
        .key("bounded-card")
        .anchored("bounded-trigger", rune::Side::Right, rune::Align::Start, 6.0)
        .anchored_in("bounded-list-pane");
        layer.push(card);
    }
    let trigger = Button::new("Inspect row")
        .variant(ButtonVariant::Outline)
        .control_size(ControlSize::Small)
        .expanded(open)
        .on_press(toggle("popover-bounded"))
        .build(k)
        .key("bounded-trigger");
    let trigger_row = if at_left {
        row().w_full().child(trigger)
    } else {
        row().w_full().justify_end().child(trigger)
    };
    let list_pane = col()
        .key("bounded-list-pane")
        .w(380.0)
        .h(220.0)
        .padding(12.0)
        .gap(12.0)
        .radius(k.radius.md)
        .fill(k.c.layer_01)
        .border(1.0, k.c.border_weak)
        .child(
            row()
                .w_full()
                .center_y()
                .justify_between()
                .child(text("List pane (boundary)").font(k.ty.ui_medium).color(k.c.text_strong))
                .child(
                    Button::new(if at_left { "Move trigger right" } else { "Move trigger left" })
                        .variant(ButtonVariant::Ghost)
                        .control_size(ControlSize::Small)
                        .on_press(flip("bounded-left"))
                        .build(k)
                        .key("bounded-move"),
                ),
        )
        .child(trigger_row)
        .child(text(placement_label).font(k.ty.small).color(k.c.text_weak));
    let drawer_pane = col()
        .key("bounded-drawer-pane")
        .w(240.0)
        .h(220.0)
        .padding(12.0)
        .gap(6.0)
        .radius(k.radius.md)
        .fill(k.c.surface_base)
        .border(1.0, k.c.border_weak)
        .child(text("Inspector drawer").font(k.ty.ui_medium).color(k.c.text_strong))
        .child(text("The anchored card stays inside the list pane and flips left instead of covering this drawer.").font(k.ty.small).color(k.c.text_weak).wrap());
    row().gap(8.0).child(list_pane).child(drawer_pane)
}

/// An anchored modal popover over a scrollable list with `El::modal_wheel_outside(true)`:
/// scrolling over the list dismisses the popover and scrolls the list in one gesture, while
/// scrolling inside the popover scrolls its own body without closing it.
pub(super) fn popover_wheel_outside(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let open = is_open(s, "popover-wheel-outside");
    let wheel_outside = !s.is("wheel-outside-off");
    if open {
        let mut details = col().gap(4.0);
        for i in 1..=10 {
            details = details.child(
                text(format!("Metadata field {i}: value-{i:02}"))
                    .font(k.ty.small)
                    .color(k.c.text_weak),
            );
        }
        let body = col()
            .w(220.0)
            .padding(12.0)
            .gap(8.0)
            .child(text("Item details").font(k.ty.ui_medium).color(k.c.text_strong))
            .child(
                text(if wheel_outside {
                    "Scroll the file list outside to close this popover and scroll it at once."
                } else {
                    "modal_wheel_outside(false): outside wheels are blocked while open."
                })
                .font(k.ty.small)
                .color(k.c.text_muted)
                .wrap(),
            )
            .child(
                col()
                    .key("wheel-outside-popover-scroll")
                    .w_full()
                    .h(88.0)
                    .padding(6.0)
                    .radius(k.radius.sm)
                    .fill(k.c.layer_01)
                    .scroll(rune::ScrollSpec { stick_bottom: false })
                    .child(details),
            );
        let surface = popover::raised(k, body, k.radius.md)
            .key("wheel-outside-card")
            .role("dialog")
            .label("Item details")
            .modal(true, close)
            .modal_wheel_outside(wheel_outside);
        layer.push(popover::anchored(k, surface, "wheel-outside-trigger", Placement::RightStart, 8.0));
    }
    let mut rows = col().gap(2.0).padding(6.0);
    for i in 1..=24 {
        rows = rows.child(
            row()
                .key(format!("file-row-{i}"))
                .w_full()
                .h(28.0)
                .px(8.0)
                .center_y()
                .justify_between()
                .radius(4.0)
                .fill(if i % 2 == 0 { k.c.layer_01 } else { k.c.surface_base })
                .child(text(format!("document-{i:02}.rs")).font(k.ty.ui_compact).color(k.c.text_base))
                .child(text(format!("{} KB", i * 4)).font(k.ty.small).color(k.c.text_faint)),
        );
    }
    let list = scroll_bar(
        k,
        "wheel-outside-list-area",
        "wheel-outside-list",
        col()
            .key("wheel-outside-list")
            .w_full()
            .h(220.0)
            .scroll(rune::ScrollSpec { stick_bottom: false })
            .child(rows),
    )
    .w(320.0)
    .radius(k.radius.md)
    .fill(k.c.surface_base)
    .border(1.0, k.c.border_weak);
    let controls = row()
        .gap(12.0)
        .center_y()
        .child(
            Button::new(if open { "Close popover" } else { "Open popover" })
                .variant(ButtonVariant::Outline)
                .control_size(ControlSize::Small)
                .expanded(open)
                .on_press(toggle("popover-wheel-outside"))
                .build(k)
                .key("wheel-outside-trigger"),
        )
        .child(
            Switch::new(wheel_outside)
                .label("modal_wheel_outside")
                .on_toggle(flip("wheel-outside-off"))
                .build(k)
                .key("wheel-outside-toggle"),
        );
    col().gap(10.0).items_start().child(controls).child(list)
}

fn menu_button(k: &Kit, label: &str, key: &'static str, open: bool) -> El {
    Button::new(label).expanded(open).on_press(toggle(key)).build(k)
}

pub(super) fn menu_basic(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
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

pub(super) fn menu_appearances(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
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

pub(super) fn menu_rich(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
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

pub(super) fn menu_checkbox(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
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

pub(super) fn menu_radio(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
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

pub(super) fn menu_submenu(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
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

/// Menu with `Menu::label`, explicit `MenuItem::key`, distinct `MenuItem::a11y_label`, and a
/// compact `Placement::LeftStart` submenu (`submenu_min_width(0.0)`) near the trailing edge.
pub(super) fn menu_submenu_options(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let open = is_open(s, "menu");
    let http = s.picked("http-version", "HTTP/2").to_owned();
    let pick_http = |v: &'static str| {
        move |s: &mut G| {
            s.pick.insert("http-version".into(), v.into());
        }
    };
    let versions = vec![
        MenuEntry::item(MenuItem::new("HTTP/1.1").key("http-1").checked(http == "HTTP/1.1").on_select(pick_http("HTTP/1.1"))),
        MenuEntry::item(MenuItem::new("HTTP/2").key("http-2").checked(http == "HTTP/2").on_select(pick_http("HTTP/2"))),
        MenuEntry::item(MenuItem::new("HTTP/3").key("http-3").checked(http == "HTTP/3").on_select(pick_http("HTTP/3"))),
    ];
    let mut menu = Menu::new("menu")
        .label("HTTP settings")
        .trigger(menu_button(k, "HTTP settings", "menu", open))
        .placement(Placement::BottomEnd)
        .open(open)
        .on_close(close);
    if s.is("prepend-row") {
        menu = menu.item(MenuItem::new("Inserted above").key("inserted").on_select(flip("prepend-row")));
    }
    menu = menu
        .item(
            MenuItem::new("…/overlays/menu.rs")
                .key("menu-rs")
                .a11y_label("crates/rune-kit/src/overlays/menu.rs")
                .on_select(close),
        )
        .item(
            MenuItem::new("HTTP version")
                .key("http-version")
                .shortcut(http)
                .submenu(versions, !s.is("sub-closed"))
                .submenu_placement(Placement::LeftStart)
                .submenu_min_width(0.0)
                .on_select(flip("sub-closed")),
        )
        .separator()
        .item(MenuItem::new("Reload").key("reload").shortcut("⌘ R").on_select(close));
    row()
        .w(520.0)
        .gap(12.0)
        .center_y()
        .child(
            Button::new(if s.is("prepend-row") { "Remove top item" } else { "Insert item above" })
                .variant(ButtonVariant::Outline)
                .on_press(flip("prepend-row"))
                .build(k)
                .key("toggle-prepend"),
        )
        .child(el().grow(1.0))
        .child(menu.build(k, layer))
}

/// Standalone `PopupList::build_list` embedded inside a custom panel, with rich
/// `PopupItem::label_with` match-highlighted labels that keep plain accessible names.
pub(super) fn popup_list_rich_inline(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let entries = [
        ("review", "/review", "/rev", "iew", "Review branch changes", "⏎", false),
        ("rename", "/rename", "/re", "name", "Rename current symbol", "F2", false),
        ("revert", "/revert", "/rev", "ert", "Revert staged hunks", "", true),
    ];
    let active = s.pick.get("rich-inline-active").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
    let chosen = s.picked("rich-inline-chosen", "None");
    let items = entries.into_iter().map(|(id, full, hit, rest, desc, hint, disabled)| {
        let rich = row()
            .center_y()
            .child(text(hit).font(k.ty.ui_medium).color(k.c.text_accent))
            .child(text(rest).font(k.ty.ui_compact).color(k.c.text_base));
        let mut item = PopupItem::new(id, full)
            .label_with(rich)
            .description(desc)
            .disabled(disabled);
        if !hint.is_empty() {
            item = item.trailing(text(hint).font(k.ty.small_medium).color(k.c.text_faint));
        }
        item
    });
    let list = PopupList::new("rich-inline", "")
        .label("Matched commands")
        .max_height(160.0)
        .items(items)
        .active(Some(active))
        .on_active(|s: &mut G, i| { s.pick.insert("rich-inline-active".into(), i.to_string()); })
        .on_choose(|s: &mut G, id, _| { s.pick.insert("rich-inline-chosen".into(), format!("/{id}")); })
        .build_list(k);
    col()
        .gap(8.0)
        .items_start()
        .child(
            col()
                .key("custom-card")
                .w(340.0)
                .padding(6.0)
                .radius(k.radius.md)
                .fill(k.c.surface_raised)
                .border(1.0, k.c.border_weak)
                .child(list),
        )
        .child(text(format!("Chosen: {chosen}")).font(k.ty.small).color(k.c.text_muted))
}

/// The tooltip stories' frame: 80 points of padding, centred.
fn tip_frame(e: El) -> El {
    row().w_full().padding(80.0).center().child(e)
}

pub(super) fn tooltip_simple(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let t = Tooltip::new("tip").trigger(body_text(k, "Hover me")).open(is_open(s, "tip"));
    tip_frame(t.text(k, "Tooltip Text").build(k, layer))
}

pub(super) fn tooltip_keybind(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let t = Tooltip::new("tip").trigger(body_text(k, "Hover me")).open(is_open(s, "tip"));
    tip_frame(t.text(k, "Tooltip Text").child(Keybind::new(&["⌘", "⌘"]).build(k)).build(k, layer))
}

pub(super) fn tooltip_path(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let t = Tooltip::new("tip").trigger(body_text(k, "Hover me")).open(is_open(s, "tip"));
    let font = t.font(k);
    let parts = row()
        .gap(3.0)
        .child(text("Components").font(font).color(k.c.text_base))
        .child(text("/").font(font).color(k.c.text_faint))
        .child(text("Tooltip").font(font).color(k.c.text_base));
    tip_frame(t.child(parts).build(k, layer))
}

pub(super) fn tooltip_title_description(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let t = Tooltip::new("tip").trigger(body_text(k, "Hover me")).open(is_open(s, "tip"));
    let font = t.font(k);
    tip_frame(
        t.child(text("Title").font(font).color(k.c.text_base))
            .child(text("·").font(font).color(k.c.text_faint))
            .child(text("Description").font(font).color(k.c.text_faint))
            .build(k, layer),
    )
}

/// Drawn `El::help` tooltips configured with `HelpLook::side(Side::Top).arrow(5.0)` and
/// per-element `El::help_side` overrides.
pub(super) fn tooltip_help_arrow(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    tip_frame(
        row()
            .gap(16.0)
            .center_y()
            .child(
                Button::new("Default (Top)")
                    .on_press(toggle("help-arrow"))
                    .build(k)
                    .key("help-default")
                    .help("Opens above by default")
                    .help_keys(["⌘", "K"])
                    .help_open(is_open(s, "help-arrow")),
            )
            .child(
                Button::new("Bottom override")
                    .variant(ButtonVariant::Outline)
                    .build(k)
                    .key("help-bottom")
                    .help("Overridden below")
                    .help_side(rune::Side::Bottom),
            )
            .child(
                Button::new("Right override")
                    .variant(ButtonVariant::Outline)
                    .build(k)
                    .key("help-right")
                    .help("Overridden right")
                    .help_side(rune::Side::Right),
            ),
    )
}

pub(super) fn hover_card_basic(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
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

/// HoverCard with `LeftStart`, `LeftEnd`, and `RightEnd` placements, custom gutter/width,
/// accessible role/label, and `on_hover_tx` so moving onto the surface keeps the card open.
pub(super) fn hover_card_placements(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let card = |id: &'static str, title: &'static str, placement: Placement, gutter: f32, width: f32, layer: &mut Vec<El>| {
        let trig_flag = format!("{id}-t");
        let surf_flag = format!("{id}-s");
        let open = is_open(s, id) || s.is(&trig_flag) || s.is(&surf_flag);
        let t_flag = trig_flag.clone();
        let s_flag = surf_flag.clone();
        let trigger = Button::new(title)
            .variant(ButtonVariant::Outline)
            .build(k)
            .on_hover_change(move |s: &mut G, over, _| {
                if over {
                    s.on.insert(t_flag.clone());
                    s.open = Some(id.to_owned());
                } else {
                    s.on.remove(&t_flag);
                    if !s.is(&s_flag) && s.open.as_deref() == Some(id) {
                        s.open = None;
                    }
                }
            });
        let t_flag_s = trig_flag;
        let s_flag_s = surf_flag;
        HoverCard::new(id)
            .trigger(trigger)
            .placement(placement)
            .gutter(gutter)
            .width(width)
            .role("dialog")
            .label(format!("{title} preview"))
            .open(open)
            .on_hover_tx(move |s: &mut G, over, tx: &mut Tx| {
                if over {
                    s.on.insert(s_flag_s.clone());
                    tx.instant();
                } else {
                    s.on.remove(&s_flag_s);
                    if !s.is(&t_flag_s) && s.open.as_deref() == Some(id) {
                        s.open = None;
                    }
                }
            })
            .child(
                col()
                    .gap(6.0)
                    .padding(4.0)
                    .child(text(format!("{title} ({placement:?})")).font(k.ty.body_medium).color(k.c.text_strong))
                    .child(
                        text("Move the pointer onto this surface: on_hover_tx keeps it open.")
                            .font(k.ty.text_small)
                            .color(k.c.text_weak)
                            .wrap(),
                    ),
            )
            .build(k, layer)
    };
    col()
        .w_full()
        .py(48.0)
        .px(240.0)
        .gap(16.0)
        .items_start()
        .child(card("hc-left-start", "LeftStart", Placement::LeftStart, 8.0, 210.0, layer))
        .child(card("hc-left-end", "LeftEnd", Placement::LeftEnd, 8.0, 210.0, layer))
        .child(card("hc-right-end", "RightEnd", Placement::RightEnd, 10.0, 220.0, layer))
}

/// Popover and Select using `on_close_tx` and `on_toggle_tx` to move focus to the target button
/// in the same commit.
pub(super) fn overlays_tx_callbacks(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    const OPTIONS: [&str; 3] = ["Apple", "Pear", "Plum"];
    let popover = Popover::new("tx-popover")
        .trigger(Button::new("Open popover").on_press(toggle("tx-popover")).build(k))
        .title("Popover (on_close_tx)")
        .description("Closing moves focus to the button on the right.")
        .child(overlay_text(k, "Click Close or outside to test on_close_tx."))
        .open(is_open(s, "tx-popover"))
        .on_close_tx(|s: &mut G, tx: &mut Tx| {
            s.open = None;
            tx.focus("tx-after", true);
        })
        .build(k, layer);
    let select = Select::new("tx-select", &OPTIONS)
        .label("Fruit")
        .placeholder("Choose fruit")
        .appearance(SelectAppearance::Field)
        .width(160.0)
        .current(s.pick.get("tx-select").map(String::as_str))
        .open(is_open(s, "tx-select"))
        .on_toggle_tx(|s: &mut G, tx: &mut Tx| {
            s.toggle_open("tx-select");
            if s.open.is_none() {
                tx.focus("tx-after", true);
            }
        })
        .on_close_tx(|s: &mut G, tx: &mut Tx| {
            s.open = None;
            tx.focus("tx-after", true);
        })
        .on_choose(|s: &mut G, v, tx| {
            s.pick.insert("tx-select".into(), v.to_owned());
            s.open = None;
            tx.focus("tx-after", true);
        })
        .build(k, layer);
    let target = Button::new("Focus target after close").variant(ButtonVariant::Outline).build(k).key("tx-after");
    row().gap(16.0).center_y().child(popover).child(select).child(target)
}

/// Keyboard focus features: `El::press_on_return()` on an individual button (ahead of a window
/// `default_button()`), and entering a `focus_group` menu's items directly from its focused
/// container via arrows, Home/End, PageUp/PageDown or typeahead.
pub(super) fn focus_keyboard(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let last_btn = s.picked("focus-btn", "None");
    let last_item = s.picked("focus-item", "None");
    let buttons = col()
        .gap(8.0)
        .items_start()
        .child(text("El::press_on_return()").font(k.ty.ui_medium).color(k.c.text_base))
        .child(
            row()
                .gap(8.0)
                .child(
                    Button::new("Apply (Return)")
                        .variant(ButtonVariant::Contrast)
                        .on_press(|s: &mut G| { s.pick.insert("focus-btn".into(), "Apply (Return)".into()); })
                        .build(k)
                        .key("return-btn")
                        .press_on_return(),
                )
                .child(
                    Button::new("Ordinary (Space)")
                        .variant(ButtonVariant::Outline)
                        .on_press(|s: &mut G| { s.pick.insert("focus-btn".into(), "Ordinary (Space)".into()); })
                        .build(k)
                        .key("ordinary-btn"),
                )
                .child(
                    Button::new("Default")
                        .variant(ButtonVariant::Ghost)
                        .on_press(|s: &mut G| { s.pick.insert("focus-btn".into(), "Default".into()); })
                        .build(k)
                        .key("default-btn")
                        .default_button(),
                ),
        )
        .child(text(format!("Last button: {last_btn}")).font(k.ty.small).color(k.c.text_muted));

    const ITEMS: [(&str, &str); 4] = [("alpha", "Alpha"), ("alpine", "Alpine"), ("bravo", "Bravo"), ("charlie", "Charlie")];
    let mut menu = col()
        .key("focus-menu")
        .role("menu")
        .label("Actions")
        .focus_group()
        .focusable(true)
        .tab_index(-1)
        .w(220.0)
        .padding(8.0)
        .gap(2.0)
        .radius(k.radius.md)
        .fill(k.c.layer_01)
        .outline(1.0, 0.0, k.c.border_base);
    for (key, label) in ITEMS {
        menu = menu.child(
            row()
                .key(key)
                .role("menuitem")
                .label(label)
                .focusable(true)
                .focus_highlight(k.c.hover)
                .w_full()
                .h(28.0)
                .px(8.0)
                .center_y()
                .radius(4.0)
                .on_press(move |s: &mut G| { s.pick.insert("focus-item".into(), label.into()); })
                .child(text(label).font(k.ty.ui_compact).color(k.c.text_base)),
        );
    }
    let group = col()
        .gap(8.0)
        .items_start()
        .child(text("focus_group container entry").font(k.ty.ui_medium).color(k.c.text_base))
        .child(
            Button::new("Focus menu container")
                .variant(ButtonVariant::Outline)
                .on_press_tx(|_: &mut G, tx: &mut Tx| tx.focus("focus-menu", true))
                .build(k)
                .key("focus-menu-btn"),
        )
        .child(menu)
        .child(
            text(format!("Chosen item: {last_item} — focus menu, then press Down, End, or type a/c."))
                .font(k.ty.small)
                .color(k.c.text_muted),
        );
    col().gap(20.0).items_start().child(buttons).child(group)
}

static DOWNLOAD: rune::Icon = rune::Icon {
    name: "story-download",
    view: 16.0,
    body: r#"<path d="M13.5554 10.4453V13.5564H11.7777H4.22211C3.23989 13.5564 2.44434 13.5564 2.44434 13.5564V10.4453" stroke="currentColor"/><path d="M4.88867 6L7.99978 9.11111L11.1109 6" stroke="currentColor"/><path d="M8 9.11198V2.44531" stroke="currentColor"/>"#,
};

pub(super) fn toast_all(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
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
