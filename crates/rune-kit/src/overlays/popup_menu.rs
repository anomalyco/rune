//! One-call `rune_kit::Menu::from_popup_menu` adapter for `rune::PopupMenuRequest`
//! (`App::on_popup_menu` fallback on Linux).

use std::borrow::Borrow;
use std::cell::RefCell;
use std::rc::Rc;

use rune::{PopupAnchor, PopupEntry, PopupMenuRequest, Tx};

use super::menu::{Menu, MenuEntry, MenuItem};
use super::overlay::Placement;

type ResolveFn<S> = Rc<dyn Fn(&mut S, Option<&str>, &mut Tx)>;

fn path_slug(path: &[usize]) -> String {
    let mut out = String::new();
    for (i, idx) in path.iter().enumerate() {
        if i > 0 { out.push('-'); }
        out.push_str(&idx.to_string());
    }
    out
}

fn map_entries<S: 'static>(
    menu_key: &str,
    entries: &[PopupEntry],
    prefix: &[usize],
    current_open: &[usize],
    open_path: &Rc<RefCell<Vec<usize>>>,
    on_resolve: &ResolveFn<S>,
) -> Vec<MenuEntry> {
    entries
        .iter()
        .enumerate()
        .map(|(idx, entry)| match entry {
            PopupEntry::Separator => MenuEntry::Separator,
            PopupEntry::Item(item) => {
                let id = item.id().to_owned();
                let mut row = MenuItem::new(item.title())
                    .key(format!("{menu_key}-item-{id}"))
                    .disabled(!item.is_enabled());
                if item.is_checked() {
                    row = row.checked(true);
                }
                if let Some(shortcut) = item.key_equivalent_shortcut() {
                    row = row.shortcut(shortcut.to_string());
                }
                let resolve = Rc::clone(on_resolve);
                let clear_open = Rc::clone(open_path);
                row = row.on_select_tx(move |s: &mut S, tx: &mut Tx| {
                    clear_open.borrow_mut().clear();
                    resolve(s, Some(&id), tx);
                });
                MenuEntry::item(row)
            }
            PopupEntry::Submenu { title, menu, enabled, .. } => {
                let mut path = prefix.to_vec();
                path.push(idx);
                let is_open = *enabled && current_open.starts_with(&path);
                let sub_entries = map_entries(menu_key, menu.entries(), &path, current_open, open_path, on_resolve);
                let sub_key = format!("{menu_key}-sub-{}", path_slug(&path));
                let toggle_open = Rc::clone(open_path);
                let toggle_path = path.clone();
                let click_open = Rc::clone(open_path);
                let click_path = path;
                let row = MenuItem::new(title)
                    .key(sub_key)
                    .disabled(!*enabled)
                    .submenu(sub_entries, is_open)
                    .on_submenu(move |_: &mut S, open: bool| {
                        let mut cur = toggle_open.borrow_mut();
                        if open {
                            *cur = toggle_path.clone();
                        } else if cur.starts_with(&toggle_path) {
                            cur.truncate(toggle_path.len() - 1);
                        }
                    })
                    .on_select(move |_: &mut S| {
                        let mut cur = click_open.borrow_mut();
                        if cur.starts_with(&click_path) {
                            cur.truncate(click_path.len() - 1);
                        } else {
                            *cur = click_path.clone();
                        }
                    });
                MenuEntry::item(row)
            }
            _ => MenuEntry::Separator,
        })
        .collect()
}

impl Menu {
    /// Builds an in-window [`Menu`] from a [`rune::PopupMenuRequest`] handed to
    /// [`rune::App::on_popup_menu`], ready to mount into `layer` with [`.build(&kit, &mut layer)`](Self::build).
    ///
    /// Maps every [`rune::PopupEntry`] (items, separators, disabled and checked states,
    /// shortcut hints, and arbitrarily nested submenus), anchors at `request.anchor_rect()`
    /// (dropping below the element, or covering it when `request.positioned()` is set) or at
    /// `request.point()`, flips/clamps and scrolls inside small windows via `El::anchored_fit`,
    /// and invokes `on_resolve(state, Some(id), tx)` on pick or `on_resolve(state, None, tx)` on
    /// Escape or outside click.
    ///
    /// ```
    /// use rune::{El, PopupMenuRequest, Tx};
    /// use rune_kit::{Kit, Menu};
    ///
    /// struct MyApp { popup: Option<PopupMenuRequest> }
    /// fn mount_popup(s: &MyApp, kit: &Kit, layer: &mut Vec<El>) {
    ///     if let Some(req) = &s.popup {
    ///         let _ = Menu::from_popup_menu("popup-fallback", req, |s: &mut MyApp, picked, tx: &mut Tx| {
    ///             s.popup = None;
    ///             tx.resolve_popup_menu(picked);
    ///         }).build(kit, layer);
    ///     }
    /// }
    /// ```
    pub fn from_popup_menu<S: 'static>(
        key: impl Into<String>,
        request: impl Borrow<PopupMenuRequest>,
        on_resolve: impl Fn(&mut S, Option<&str>, &mut Tx) + 'static,
    ) -> Menu {
        let key = key.into();
        let req = request.borrow();
        let on_resolve: ResolveFn<S> = Rc::new(on_resolve);
        let open_path = req.submenu_open_path();
        let current_open = RefCell::borrow(&open_path).clone();
        let entries = map_entries(&key, req.menu().entries(), &[], &current_open, &open_path, &on_resolve);
        let close_resolve = Rc::clone(&on_resolve);
        let close_open = Rc::clone(&open_path);
        let mut menu = Menu::new(key)
            .label("Context menu")
            .min_width(req.min_width().max(160.0))
            .open(true)
            .on_close_tx(move |s: &mut S, tx: &mut Tx| {
                close_open.borrow_mut().clear();
                close_resolve(s, None, tx);
            });
        if let Some([x, y, w, h]) = req.anchor_rect() {
            if req.positioned().is_some() {
                menu = menu.at(x, y);
            } else {
                menu = menu.with_anchor_rect([x, y, w, h]).placement(Placement::BottomStart).gutter(4.0);
            }
            if let PopupAnchor::Key(anchor_key) = req.anchor() {
                menu = menu.with_focus_return(anchor_key.clone());
            }
        } else {
            let p = req.point();
            menu = menu.at(p.x, p.y);
        }
        for entry in entries {
            menu = match entry {
                MenuEntry::Item(i) => menu.item(*i),
                MenuEntry::Separator => menu.separator(),
                MenuEntry::Label(l) => menu.group_label(l),
            };
        }
        menu
    }
}

#[cfg(test)]
#[path = "popup_menu_tests.rs"]
mod tests;
