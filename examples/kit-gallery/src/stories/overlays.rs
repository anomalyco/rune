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
