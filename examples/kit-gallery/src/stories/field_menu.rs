//! Small-window submenu clamping (`rune-menu--submenu-small-window`) and in-window text-field
//! menu fallback (`rune-menu--field-menu-fallback`) using `App::on_field_menu`,
//! `PopupMenu::entries`, and `Tx::resolve_field_menu`.

use rune::{El, PointerPress, PopupEntry, PopupItem, PopupMenu, Tx, col, el, row, text};
use rune_kit::{Button, ButtonVariant, Kit, Menu, MenuEntry, MenuItem, TextInput, context_menu};

use crate::G;

/// Maps a `rune::PopupMenu`'s entries into `rune_kit::MenuEntry` rows, wiring item picks to
/// `Tx::resolve_field_menu` (or recording preview picks when showing the sample preview).
fn map_entries(entries: &[PopupEntry], live: bool, open_sub: Option<&str>) -> Vec<MenuEntry> {
    entries
        .iter()
        .map(|entry| match entry {
            PopupEntry::Separator => MenuEntry::Separator,
            PopupEntry::Item(item) => {
                let id = item.id().to_owned();
                let title = item.title().to_owned();
                let mut row_item = MenuItem::new(title.clone())
                    .key(format!("fm-{id}"))
                    .disabled(!item.is_enabled());
                if item.is_checked() {
                    row_item = row_item.checked(true);
                }
                if let Some(shortcut) = item.key_equivalent_shortcut() {
                    row_item = row_item.shortcut(shortcut.to_string());
                }
                row_item = row_item.on_select_tx(move |s: &mut G, tx: &mut Tx| {
                    s.field_menu = None;
                    s.on.remove("fm-preview");
                    s.pick.remove("fm-sub");
                    s.field_menu_last = Some(id.clone());
                    if live {
                        tx.resolve_field_menu(Some(&id));
                    }
                });
                MenuEntry::item(row_item)
            }
            PopupEntry::Submenu { title, menu, enabled, .. } => {
                let sub_title = title.clone();
                let is_open = open_sub == Some(sub_title.as_str());
                let sub_entries = map_entries(menu.entries(), live, None);
                MenuEntry::item(
                    MenuItem::new(title.clone())
                        .disabled(!*enabled)
                        .submenu(sub_entries, is_open)
                        .on_select(move |s: &mut G| {
                            if s.pick.get("fm-sub").map(String::as_str) == Some(sub_title.as_str()) {
                                s.pick.remove("fm-sub");
                            } else {
                                s.pick.insert("fm-sub".into(), sub_title.clone());
                            }
                        }),
                )
            }
            _ => MenuEntry::Separator,
        })
        .collect()
}

/// Mounts the active `App::on_field_menu` request (if any) as an in-window `rune_kit::Menu`.
pub(crate) fn mount_live_field_menu(s: &G, k: &Kit, layer: &mut Vec<El>) {
    let Some(req) = &s.field_menu else { return };
    let open_sub = s.pick.get("fm-sub").map(String::as_str);
    let mut menu = Menu::new("field-menu-fallback")
        .label("Text field menu")
        .at(req.point().x, req.point().y)
        .open(true)
        .on_close(|s: &mut G| {
            s.field_menu = None;
            s.pick.remove("fm-sub");
        });
    for entry in map_entries(req.menu().entries(), true, open_sub) {
        menu = match entry {
            MenuEntry::Item(i) => menu.item(*i),
            MenuEntry::Separator => menu.separator(),
            MenuEntry::Label(l) => menu.group_label(l),
        };
    }
    let _ = menu.build(k, layer);
}

/// 420x400 small-window submenu story: a 20-row (or 8-row) "Open With" submenu clamps on both
/// axes inside the window and scrolls instead of clipping off the right or bottom edge.
pub(super) fn menu_submenu_small_window(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let short = s.is("submenu-short");
    let count = if short { 8 } else { 20 };
    let sub = (0..count)
        .map(|i| {
            let name = format!("App {i} — Editor");
            let pick = name.clone();
            MenuEntry::item(MenuItem::new(name).on_select(move |s: &mut G| {
                s.pick.insert("small-sub-pick".into(), pick.clone());
            }))
        })
        .collect();
    let point = s.menu_at.or_else(|| {
        (s.story == "rune-menu--submenu-small-window").then_some(PointerPress { x: 130.0, y: 60.0 })
    });
    let trigger = context_menu("small-ctx", el().size(360.0, 220.0), point)
        .label("File actions")
        .item(MenuItem::new("Open"))
        .item(MenuItem::new("Open With").submenu(sub, true))
        .item(MenuItem::new("Copy"))
        .build(k, layer);
    let picked = s.picked("small-sub-pick", "None");
    col()
        .gap(10.0)
        .child(
            row()
                .gap(8.0)
                .center_y()
                .child(
                    Button::new(if short { "Show 20 rows" } else { "Show 8 rows" })
                        .variant(ButtonVariant::Outline)
                        .on_press(|s: &mut G| s.flip("submenu-short"))
                        .build(k),
                )
                .child(text(format!("Picked: {picked}")).font(k.ty.small).color(k.c.text_muted)),
        )
        .child(trigger)
}

/// In-window text-field menu fallback demo using `App::on_field_menu`, `PopupMenu::entries`,
/// and `Tx::resolve_field_menu`. Right-clicking the field uses the native menu on macOS/Windows
/// and invokes `App::on_field_menu` on platforms where `popup_menu_supported()` is false; the
/// preview button lets any platform inspect the rendered fallback menu.
pub(super) fn menu_field_menu_fallback(s: &G, k: &Kit, layer: &mut Vec<El>) -> El {
    let native = rune::popup_menu_supported();
    let last = s.field_menu_last.as_deref().unwrap_or("None");
    let status = format!("popup_menu_supported() = {native} · last action: {last}");
    let field = TextInput::new(&s.field_menu_edit)
        .on_edit(|s: &mut G| &mut s.field_menu_edit)
        .label("Editable text field")
        .placeholder("Right-click or press Shift+F10")
        .build(k)
        .key("fallback-field");
    let preview_open = s.is("fm-preview") && s.field_menu.is_none();
    let preview_btn = Button::new(if preview_open { "Hide fallback preview" } else { "Preview fallback menu" })
        .variant(ButtonVariant::Outline)
        .on_press(|s: &mut G| s.flip("fm-preview"))
        .build(k);
    let preview = if preview_open {
        let sample = PopupMenu::new()
            .item(PopupItem::new("cut", "Cut"))
            .item(PopupItem::new("copy", "Copy"))
            .item(PopupItem::new("paste", "Paste"))
            .item(PopupItem::new("select-all", "Select All"))
            .separator()
            .item(PopupItem::new("spellcheck", "Spellcheck").checked(true));
        let mut menu = Menu::new("fm-preview-menu")
            .label("Field menu preview")
            .trigger(preview_btn)
            .open(true)
            .on_close(|s: &mut G| { s.on.remove("fm-preview"); });
        for entry in map_entries(sample.entries(), false, None) {
            menu = match entry {
                MenuEntry::Item(i) => menu.item(*i),
                MenuEntry::Separator => menu.separator(),
                MenuEntry::Label(l) => menu.group_label(l),
            };
        }
        menu.build(k, layer)
    } else {
        preview_btn
    };
    col()
        .w(360.0)
        .gap(12.0)
        .child(field)
        .child(row().gap(8.0).center_y().child(preview))
        .child(text(status).font(k.ty.small).color(k.c.text_muted))
}
