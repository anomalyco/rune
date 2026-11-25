//! General in-window pop-up menu fallback story (`rune-menu--popup-menu-fallback`) using
//! `App::on_popup_menu`, `rune_kit::Menu::from_popup_menu`, and `Tx::resolve_popup_menu`.

use rune::{El, Link, PointerPress, PopupItem, PopupMenu, Tx, col, el, row, text};
use rune_kit::{Button, ButtonVariant, Kit, Menu};

use crate::G;

const THEMES: [(&str, &str); 3] = [("system", "System"), ("light", "Light"), ("dark", "Dark")];

fn context_menu() -> PopupMenu {
    let mut copy = PopupItem::new("copy", "Copy");
    if let Ok(sc) = "mod+c".parse() { copy = copy.key_equivalent(sc); }
    PopupMenu::new()
        .item(copy)
        .item(PopupItem::new("paste", "Paste").enabled(false))
        .item(PopupItem::new("wrap", "Word Wrap").checked(true))
        .separator()
        .submenu(
            "Share",
            PopupMenu::new()
                .item(PopupItem::new("mail", "Mail"))
                .item(PopupItem::new("messages", "Messages")),
        )
}

/// Mounts the active `App::on_popup_menu` request (if any) in one call via
/// `rune_kit::Menu::from_popup_menu`.
pub(crate) fn mount_live_popup_menu(s: &G, k: &Kit, layer: &mut Vec<El>) {
    let Some(req) = &s.popup_menu else { return };
    let _ = Menu::from_popup_menu("popup-menu-fallback", req, |s: &mut G, picked, tx: &mut Tx| {
        s.popup_menu = None;
        s.popup_menu_last = Some(picked.unwrap_or("(dismissed)").into());
        tx.resolve_popup_menu(picked);
    })
    .build(k, layer);
}

/// In-window pop-up menu fallback story (`rune-menu--popup-menu-fallback`): demonstrates
/// `Tx::popup_menu` (key-anchored picker + point-anchored context menu with submenu),
/// selectable text Copy menu, and an inline link all flowing through `App::on_popup_menu` on
/// Linux.
pub(super) fn menu_popup_menu_fallback(s: &G, k: &Kit, _layer: &mut Vec<El>) -> El {
    let native = rune::popup_menu_supported();
    let last = s.popup_menu_last.as_deref().unwrap_or("None");
    let theme = s.popup_menu_theme.as_str();
    let picker = Button::new(format!("Theme: {theme}"))
        .variant(ButtonVariant::Outline)
        .on_press_tx(|s: &mut G, tx: &mut Tx| {
            let menu = PopupMenu::picker(THEMES, &s.popup_menu_theme);
            tx.popup_menu(menu, "pm-picker-btn", |s: &mut G, picked, _| {
                if let Some(id) = picked { s.popup_menu_theme = id.into(); }
            });
        })
        .build(k)
        .key("pm-picker-btn");
    let ctx_area = el()
        .key("pm-ctx-area")
        .w(360.0)
        .h(72.0)
        .padding(12.0)
        .radius(6.0)
        .fill(k.c.layer_01)
        .border(1.0, k.c.border_base)
        .on_secondary_press(|_: &mut G, press: PointerPress, tx: &mut Tx| {
            tx.popup_menu(context_menu(), press, |s: &mut G, picked, _| {
                s.popup_menu_last = Some(picked.unwrap_or("(dismissed)").into());
            });
        })
        .child(text("Right-click here for an app context menu (with Share submenu)").font(k.ty.small).color(k.c.text_base));
    let selectable = text("Select part of this line and right-click for Rune's Copy menu.")
        .key("pm-selectable")
        .font(k.ty.small)
        .color(k.c.text_base)
        .selectable(true);
    let link_line = text("Right-click the Rune docs link to test link menus.")
        .key("pm-link")
        .font(k.ty.small)
        .color(k.c.text_base)
        .selectable(true)
        .link(Link::new(16..25, "https://example.com/rune"))
        .on_link(|s: &mut G, id: &str, _| s.popup_menu_last = Some(format!("link:{id}")));
    let status = format!("popup_menu_supported() = {native} · theme: {theme} · last: {last}");
    col()
        .w(400.0)
        .gap(12.0)
        .child(row().gap(8.0).center_y().child(picker))
        .child(ctx_area)
        .child(selectable)
        .child(link_line)
        .child(text(status).font(k.ty.small).color(k.c.text_muted))
}
