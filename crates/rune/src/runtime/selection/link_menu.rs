//! Default Open Link / Copy Link context menu for inline [`Link`] spans with a real address.
//!
//! Built as a pure item-list function ([`build_context_menu`]) over what sits under the secondary
//! gesture (a link with an address, selectable text, and later images), queued through Rune's
//! unified [`Popups`](crate::runtime::input::popup::Popups) path so macOS, Windows, touch
//! long-press and Linux's popup-menu fallback all render it identically.
use super::*;
use crate::el::{TextFlags, TextSpec};

/// Semantic URL for inline link `l` on `spec`: `None` when redacted or not `role("link")`;
/// otherwise `l.href` if set (trimmed, non-empty, free of whitespace and control characters),
/// else `l.id` when it passes [`crate::open_url::check`] (`http:`, `https:`, `mailto:`).
pub(crate) fn link_url<'a>(spec: &'a TextSpec, l: &'a crate::Link) -> Option<&'a str> {
    if spec.redacted || l.role != "link" {
        return None;
    }
    if let Some(href) = l.href.as_deref() {
        let trimmed = href.trim();
        return (!trimmed.is_empty() && !trimmed.chars().any(|c| c.is_whitespace() || c.is_control()))
            .then_some(trimmed);
    }
    crate::open_url::check(&l.id).is_ok().then(|| l.id.trim())
}

/// Resolved address eligible for Rune's default link context menu: [`link_url`] unless the text
/// opted out with [`El::link_menu`](crate::El::link_menu)`(false)`.
pub(crate) fn link_address<'a>(spec: &'a TextSpec, l: &'a crate::Link) -> Option<&'a str> {
    if spec.flags.has(TextFlags::NO_LINK_MENU) {
        return None;
    }
    link_url(spec, l)
}

/// What sits under the secondary gesture for Rune's built-in text/link context menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ContextMenuTarget {
    /// A link with a resolved real address is under the gesture.
    pub has_link: bool,
    /// That link's text node has an [`El::on_link`](crate::El::on_link) handler.
    pub has_on_link: bool,
    /// The text node is selectable (so Copy is offered).
    pub selectable: bool,
    /// There is a non-empty text selection to copy.
    pub copy_enabled: bool,
}

/// Pure item-list builder for Rune's default link and selectable-text context menu.
/// Returns `None` when nothing in `target` produces an item.
pub(crate) fn build_context_menu(target: ContextMenuTarget) -> Option<crate::PopupMenu> {
    let mut menu = crate::PopupMenu::new();
    let mut any = false;
    if target.has_link {
        if target.has_on_link {
            menu = menu.item(crate::PopupItem::new("open-link", crate::labels::label(crate::Label::OpenLink)));
        }
        menu = menu.item(crate::PopupItem::new("copy-link", crate::labels::label(crate::Label::CopyLink)));
        any = true;
    }
    if target.selectable {
        if any {
            menu = menu.separator();
        }
        let mut copy = crate::PopupItem::new("copy", crate::labels::label(crate::Label::Copy))
            .enabled(target.copy_enabled);
        // The primary modifier, as the copy key handler takes it: ⌘C on macOS, Ctrl+C elsewhere.
        if let Ok(shortcut) = "mod+c".parse::<crate::Shortcut>() {
            copy = copy.key_equivalent(shortcut);
        }
        menu = menu.item(copy);
        any = true;
    }
    any.then_some(menu)
}

impl RuntimeCore {
    /// Whether node `i` or any allowed ancestor has an active `on_secondary_press` handler.
    pub(crate) fn ancestor_has_secondary_press(&self, i: usize) -> bool {
        std::iter::successors(Some(i), |&idx| self.tree.nodes[idx].parent)
            .take_while(|&idx| self.allowed(idx))
            .any(|idx| {
                let n = &self.tree.nodes[idx];
                !(n.input.is_some() && self.node_disabled(idx)) && n.handlers.on_secondary_press.is_some()
            })
    }

    /// Link of node `i` under `(x, y)` eligible for Rune's default link menu (no explicit
    /// `on_link_secondary` on node `i`, and `link_address` resolves to a real address).
    pub(in crate::runtime) fn default_link_at(&self, i: usize, x: f32, y: f32) -> Option<usize> {
        let spec = self.tree.nodes[i].text.as_deref()?;
        if spec.cold.on_link_secondary.is_some() {
            return None;
        }
        let k = self.link_at(i, x, y)?;
        link_address(spec, spec.cold.links.get(k)?).is_some().then_some(k)
    }

    /// Which secondary link target (if any) lies on node `i` under `(x, y)`.
    pub(in crate::runtime) fn secondary_link_kind_at(&self, i: usize, x: f32, y: f32) -> Option<capture::SecondaryLink> {
        if let Some(k) = self.secondary_link_at(i, x, y) {
            return Some(capture::SecondaryLink::Explicit(k));
        }
        if self.ancestor_has_secondary_press(i) {
            return None;
        }
        self.default_link_at(i, x, y).map(capture::SecondaryLink::Default)
    }

    /// Opens Rune's default link context menu for link `k` on node `path` at `(x, y)`.
    /// Unlike plain `selection_menu`, does not select a word on macOS right-click (Blink's
    /// `SelectionController::SelectClosestWordOrLinkFromMouseEvent` never selects a single word
    /// on a link either, and keeping any existing selection intact preserves multi-word links
    /// and selections spanning a link).
    pub(in crate::runtime) fn open_link_menu(
        &mut self,
        app: &dyn ViewDriver,
        path: &str,
        k: usize,
        x: f32,
        y: f32,
        at: Option<(f32, f32)>,
    ) -> bool {
        let Some(i) = self.live(path) else { return false };
        if self.node_disabled(i) { return false; }
        let Some((address, on_link, id)) = self.tree.nodes[i].text.as_deref().and_then(|spec| {
            let l = spec.cold.links.get(k)?;
            let address = link_address(spec, l)?.to_owned();
            Some((address, spec.cold.on_link.clone(), l.id.clone()))
        }) else { return false };
        let link_rect = self.link_drawn_rect(i, k, at);
        let selectable = self.is_selectable(i);
        let text = selectable.then(|| self.copy_text(app)).flatten();
        let html = text.as_ref().and_then(|_| self.copy_html());
        let Some(menu) = build_context_menu(ContextMenuTarget {
            has_link: true,
            has_on_link: on_link.is_some(),
            selectable,
            copy_enabled: text.is_some(),
        }) else { return false };
        log_event!(self, "link menu {path} {id}");
        self.host.popups.push_back(crate::popup_menu::Request {
            menu,
            anchor: crate::PopupAnchor::Point(x, y),
            on_pick: Box::new(move |states, picked, tx| match picked {
                Some("open-link") => {
                    if let Some(f) = on_link {
                        let prev = std::mem::replace(&mut tx.link_rect, link_rect);
                        (f.0)(states, &id, tx);
                        tx.link_rect = prev;
                    }
                }
                Some("copy-link") => tx.copy(address),
                Some("copy") => {
                    if let Some(text) = text {
                        match html {
                            Some(html) => tx.copy_rich(crate::ClipboardContent::new(text).with_html(html)),
                            None => tx.copy(text),
                        }
                    }
                }
                _ => {}
            }),
            field: None,
        });
        true
    }

    /// Runs `on_link_secondary` for link `k` on `path`, or falls back to Rune's default link
    /// menu when no explicit handler or ancestor `on_secondary_press` overrides it (used by
    /// `Action::LinkSecondary`, including AccessKit `ShowContextMenu`).
    pub(in crate::runtime) fn secondary_press_link_or_menu(
        &mut self,
        app: &mut dyn AppDriver,
        path: &str,
        k: usize,
        point: PointerEvent,
        tx: &mut Tx,
    ) -> bool {
        if self.secondary_press_link(app, path, k, point, tx) {
            return true;
        }
        let Some(i) = self.live(path) else { return false };
        if self.node_disabled(i) || self.ancestor_has_secondary_press(i) {
            return false;
        }
        self.open_link_menu(app, path, k, point.window_x, point.window_y, None)
    }

    /// The Menu key / Shift+F10 on focused node `i`'s focused link when `on_link_secondary` is
    /// absent and no element or ancestor `on_secondary_press` overrides it.
    pub(in crate::runtime) fn focused_link_default_menu(&mut self, app: &mut dyn AppDriver, i: usize) -> bool {
        if self.node_disabled(i) || self.ancestor_has_secondary_press(i) {
            return false;
        }
        let Some(k) = self.edit.link_focus().filter(|&k| k < links::link_count(&self.tree.nodes[i])) else {
            return false;
        };
        let has_addr = self.tree.nodes[i].text.as_deref()
            .and_then(|spec| spec.cold.links.get(k).and_then(|l| link_address(spec, l)))
            .is_some();
        if !has_addr {
            return false;
        }
        let Some((_, r)) = self.link_boxes_for(i, k, self.drawn_box(i)).into_iter().next() else {
            return false;
        };
        let path = self.tree.nodes[i].path.clone();
        self.open_link_menu(app, &path, k, r[0], r[3], None)
    }
}

#[cfg(test)]
#[path = "link_menu_tests.rs"]
mod tests;
