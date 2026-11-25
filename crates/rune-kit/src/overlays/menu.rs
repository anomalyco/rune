//! Menu, context menu and their items (`@opencode/ui` navigation/menu, components/context-menu).
//! Compact (default) and standard appearances.
//!
//! Whether a menu is open is the app's (`open`, `on_close`); how it opens and closes is Rune's
//! modal stack (`runtime/input/modal_focus.rs`), which follows Desktop's menu machine
//! (`ui/src/navigation/menu/machine.ts`): focus returns to the trigger as the menu closes,
//! without a ring when the pointer opened it.

use rune::{El, Icon, PointerPress, TextStyle, Tx, col, el, icon, row, text};

use super::overlay::{Modal, Placement, anchor_trigger, dismissal, push_anchored_modal, scrim_key, trigger_key};
use super::popover::{anchored, fit_scroll_bar};
use crate::{Acts, Kit, State};

/// A menu at a secondary-press position. Store the point from `El::on_secondary_press`
/// in app state; clear it in `on_close` and item callbacks. Uses Menu's flipping anchor.
pub fn context_menu(key: impl Into<String>, trigger: El, point: Option<PointerPress>) -> Menu {
    let mut menu = Menu::new(key).trigger(trigger).open(point.is_some());
    menu.context = true;
    match point {
        Some(p) => menu.at(p.x, p.y),
        None => menu,
    }
}

pub enum MenuEntry {
    Item(Box<MenuItem>),
    Separator,
    Label(String),
}

impl MenuEntry {
    pub fn item(i: MenuItem) -> MenuEntry {
        MenuEntry::Item(Box::new(i))
    }
}

pub struct MenuItem {
    key: Option<String>,
    label: String,
    a11y_label: Option<String>,
    icon: Option<&'static Icon>,
    lead: Option<El>,
    shortcut: Option<String>,
    badge: Option<String>,
    description: Option<String>,
    danger: bool,
    /// Checkbox and radio items: whether the indicator shows.
    checked: Option<bool>,
    sub: Option<(Vec<MenuEntry>, bool)>,
    sub_placement: Placement,
    sub_min_width: f32,
    state: State,
    acts: Acts,
}

impl MenuItem {
    pub fn new(label: impl Into<String>) -> MenuItem {
        MenuItem {
            key: None,
            label: label.into(),
            a11y_label: None,
            icon: None,
            lead: None,
            shortcut: None,
            badge: None,
            description: None,
            danger: false,
            checked: None,
            sub: None,
            sub_placement: Placement::RightStart,
            sub_min_width: 160.0,
            state: State::Rest,
            acts: Acts::default(),
        }
    }
    /// Explicit element key for this item row instead of the default positional
    /// `{menu_key}-item{i}`, so inserting or removing rows above it preserves focus and its
    /// submenu anchors to `k`. Blank values fall back to the positional key.
    ///
    /// ```
    /// use rune_kit::MenuItem;
    ///
    /// let _item = MenuItem::new("Save").key("save");
    /// ```
    pub fn key(mut self, k: impl Into<String>) -> Self {
        let k = k.into();
        self.key = (!k.trim().is_empty()).then_some(k);
        self
    }
    /// Accessible name for this item (ARIA `aria-label`) distinct from the drawn text passed to
    /// [`Self::new`], for example when the visible label is truncated or abbreviated. Also used
    /// for menu typeahead and as the default accessible name of an open submenu. Blank values
    /// fall back to the drawn label.
    ///
    /// ```
    /// use rune_kit::MenuItem;
    ///
    /// let _item = MenuItem::new("…/menu.rs").a11y_label("crates/rune-kit/src/overlays/menu.rs");
    /// ```
    pub fn a11y_label(mut self, l: impl Into<String>) -> Self {
        let l = l.into();
        self.a11y_label = (!l.trim().is_empty()).then_some(l);
        self
    }
    /// A 14 point icon before the label.
    pub fn icon(mut self, i: &'static Icon) -> Self {
        self.icon = Some(i);
        self
    }
    /// Anything before the label (an avatar, …).
    pub fn lead(mut self, e: El) -> Self {
        self.lead = Some(e);
        self
    }
    pub fn shortcut(mut self, s: impl Into<String>) -> Self {
        self.shortcut = Some(s.into());
        self
    }
    pub fn badge(mut self, b: impl Into<String>) -> Self {
        self.badge = Some(b.into());
        self
    }
    /// A faint second line under the label, such as what a choice does. The item grows to fit it.
    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = Some(d.into());
        self
    }
    /// A destructive choice: the label and icon take the danger ink.
    pub fn danger(mut self, on: bool) -> Self {
        self.danger = on;
        self
    }
    /// A checkbox or radio item, showing the check when `on`.
    pub fn checked(mut self, on: bool) -> Self {
        self.checked = Some(on);
        self
    }
    /// A submenu trigger; `open` shows its entries at [`Self::submenu_placement`] (right-start
    /// by default).
    pub fn submenu(mut self, entries: Vec<MenuEntry>, open: bool) -> Self {
        self.sub = Some((entries, open));
        self
    }
    /// Where an open submenu anchors relative to this item row (default
    /// [`Placement::RightStart`]). For example, [`Placement::LeftStart`] places the submenu's
    /// right edge against the item row's left edge.
    ///
    /// ```
    /// use rune_kit::{MenuEntry, MenuItem, Placement};
    ///
    /// let _item = MenuItem::new("HTTP version")
    ///     .submenu(vec![MenuEntry::item(MenuItem::new("HTTP/2"))], true)
    ///     .submenu_placement(Placement::LeftStart);
    /// ```
    pub fn submenu_placement(mut self, placement: Placement) -> Self {
        self.sub_placement = placement;
        self
    }
    /// Minimum width of an open submenu in points (default `160.0`). Pass `0.0` to let the
    /// submenu size to its content. Negative or non-finite values clamp to `0.0`.
    ///
    /// ```
    /// use rune_kit::{MenuEntry, MenuItem};
    ///
    /// let _item = MenuItem::new("HTTP version")
    ///     .submenu(vec![MenuEntry::item(MenuItem::new("HTTP/2"))], true)
    ///     .submenu_min_width(0.0);
    /// ```
    pub fn submenu_min_width(mut self, width: f32) -> Self {
        self.sub_min_width = width;
        self
    }
    /// `Hover` paints the highlighted (keyboard-focused) row.
    pub fn state(mut self, s: State) -> Self {
        self.state = s;
        self
    }
    pub fn disabled(mut self, d: bool) -> Self {
        if d {
            self.state = State::Disabled;
        }
        self
    }
    pub fn on_select<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.acts.press(f);
        self
    }
    pub fn on_select_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.acts.press_tx(f);
        self
    }

    fn build(self, k: &Kit, standard: bool, fallback_key: String, layer: &mut Vec<El>) -> El {
        let key = self.key.unwrap_or(fallback_key);
        let a11y_name = self.a11y_label.unwrap_or_else(|| self.label.clone());
        let c = &k.c;
        let disabled = self.state == State::Disabled;
        let highlighted = self.state == State::Hover || self.sub.as_ref().is_some_and(|s| s.1);
        let hover = if standard { c.surface_raised_hover } else { c.hover };
        let (font, ink) = if standard {
            (k.ty.text_small_medium, if disabled { c.text_weak } else { c.text_strong })
        } else {
            (k.ty.ui_compact, c.text_base)
        };
        let danger = self.danger && !disabled;
        let ink = if danger { c.danger.fg } else { ink };
        // The item's label, shortcut, badge and check say what this row shows.
        let mut content = row().key("content").grow(1.0).shrink().gap(8.0).center_y().a11y_hidden();
        if let Some(l) = self.lead {
            content = content.child(l);
        }
        if let Some(i) = self.icon {
            content = content.child(icon(i, 14.0).key("icon").color(if danger {
                c.danger.fg
            } else if standard {
                c.std_icon
            } else {
                c.icon_base
            }));
        }
        let label = text(self.label).key("label").font(font).color(ink).truncate();
        content = match &self.description {
            Some(d) => {
                let (font, ink) = if standard { (k.ty.text_xsmall, c.text_weak) } else { (k.ty.small, c.text_faint) };
                content.child(
                    col().key("text").grow(1.0).shrink().gap(2.0).child(label).child(
                        text(d.clone()).key("description").font(font).color(ink).truncate(),
                    ),
                )
            }
            None => content.child(label),
        };
        let mut e = row()
            .key(key.clone())
            .role(if self.checked.is_some() { "menuitemcheckbox" } else { "menuitem" })
            .label(a11y_name.clone())
            .radius(if standard { k.radius.sm } else { 4.0 })
            .gap(8.0)
            .center_y();
        if let Some(on) = self.checked {
            e = e.checked(on);
        }
        if let Some(s) = &self.shortcut {
            e = e.shortcut_hint(s.clone());
        }
        match (&self.description, &self.badge) {
            (Some(d), Some(b)) => e = e.description(format!("{d}, {b}")),
            (Some(d), None) | (None, Some(d)) => e = e.description(d.clone()),
            (None, None) => {}
        }
        if let Some((_, open)) = &self.sub {
            e = e.expanded(*open);
        }
        e = match (standard, self.description.is_some()) {
            (true, _) => e.px(8.0).py(4.0),
            (false, false) => e.h(28.0).pl(12.0).pr(6.0),
            (false, true) => e.min_h(28.0).py(6.0).pl(12.0).pr(6.0),
        };
        if highlighted {
            e = e.fill(hover);
        } else if !disabled {
            // One highlight for pointer and keys, landing at once like NSMenu's.
            e = e.focus_highlight(hover);
        }
        e = e.child(content);
        if let Some(s) = self.shortcut {
            let st = TextStyle { line_height: 11.0, ..k.ty.small_medium };
            e = e.child(text(s).key("shortcut").font(st).color(c.text_faint).a11y_hidden());
        }
        if let Some(b) = self.badge {
            e = e.child(
                row()
                    .a11y_hidden()
                    .key("badge")
                    .h(16.0)
                    .px(4.0)
                    .center()
                    .radius(2.0)
                    .fill(c.layer_02)
                    .border(0.5, c.border_base)
                    .child(text(b).font(TextStyle { line_height: 11.0, ..k.ty.small_medium }).color(c.text_muted)),
            );
        }
        if let Some(on) = self.checked {
            let mark = icon(k.glyph.check_small, 14.0)
                .color(if standard { c.std_icon } else { c.icon_base })
                .opacity(if on { 1.0 } else { 0.0 });
            e = e.child(el().key("indicator").size(16.0, 16.0).center().child(mark).a11y_hidden());
        }
        if let Some((entries, open)) = self.sub {
            // It points where the submenu opens: left, right to left (`menu.css`).
            e = e.child(icon(k.glyph.caret_right, 16.0).key("chevron").a11y_hidden().mirror_in_rtl().color(if standard {
                c.std_icon
            } else {
                c.text_faint
            }));
            if open {
                let min_width = if self.sub_min_width.is_finite() { self.sub_min_width.max(0.0) } else { 0.0 };
                let label = (!a11y_name.trim().is_empty()).then_some(a11y_name);
                let sub = Menu { key: format!("{key}-sub"), label, entries, standard, min_width, ..Menu::new("") };
                let mut deeper = Vec::new();
                let surface = sub.surface(k, &mut deeper);
                layer.push(anchored(k, surface, &key, self.sub_placement, 0.0));
                layer.extend(deeper);
            }
        }
        e = k.disabled_dim(e, disabled && !standard);
        // A popup opens on its checked item (NSPopUpButton).
        self.acts.apply(e.focusable(!disabled).focus_selected(self.checked == Some(true)).disabled(disabled), !disabled)
    }
}

/// A dropdown menu: `Menu::new("file").trigger(button).item(..).open(s.open).build(&kit, &mut layer)`.
pub struct Menu {
    key: String,
    label: Option<String>,
    trigger: Option<El>,
    entries: Vec<MenuEntry>,
    open: bool,
    standard: bool,
    placement: Placement,
    gutter: f32,
    min_width: f32,
    at: Option<(f32, f32)>,
    /// A context menu: its trigger is the area it opens over, not a button that opens it.
    context: bool,
    dismiss: Option<(El, Modal)>,
}

impl Menu {
    pub fn new(key: impl Into<String>) -> Menu {
        Menu {
            key: key.into(),
            label: None,
            trigger: None,
            entries: Vec::new(),
            open: false,
            standard: false,
            placement: Placement::BottomStart,
            gutter: 6.0,
            min_width: 160.0,
            at: None,
            context: false,
            dismiss: None,
        }
    }
    /// Accessible name for the open menu (`role="menu"`, ARIA `aria-label`), matching
    /// [`PopupList::label`](crate::PopupList::label). Blank values leave the menu unnamed.
    ///
    /// ```
    /// use rune_kit::Menu;
    ///
    /// let _menu = Menu::new("file").label("File actions");
    /// ```
    pub fn label(mut self, l: impl Into<String>) -> Self {
        let l = l.into();
        self.label = (!l.trim().is_empty()).then_some(l);
        self
    }
    pub fn trigger(mut self, t: El) -> Self {
        self.trigger = Some(t);
        self
    }
    pub fn item(mut self, i: MenuItem) -> Self {
        self.entries.push(MenuEntry::Item(Box::new(i)));
        self
    }
    pub fn separator(mut self) -> Self {
        self.entries.push(MenuEntry::Separator);
        self
    }
    pub fn group_label(mut self, l: impl Into<String>) -> Self {
        self.entries.push(MenuEntry::Label(l.into()));
        self
    }
    pub fn open(mut self, o: bool) -> Self {
        self.open = o;
        self
    }
    /// The older bordered look (`appearance="standard"`).
    pub fn standard(mut self) -> Self {
        self.standard = true;
        self.min_width = 128.0;
        self
    }
    pub fn placement(mut self, p: Placement) -> Self {
        self.placement = p;
        self
    }
    pub fn gutter(mut self, g: f32) -> Self {
        self.gutter = g;
        self
    }
    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = w;
        self
    }
    /// A context menu: open at a window point (where the secondary press landed).
    pub fn at(mut self, x: f32, y: f32) -> Self {
        self.at = Some((x, y));
        self
    }
    /// Presses outside and Escape close the menu. While open it holds keyboard focus.
    pub fn on_close<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.dismiss = Some(dismissal(scrim_key(&self.key), f));
        self
    }

    fn surface(self, k: &Kit, layer: &mut Vec<El>) -> El {
        let c = &k.c;
        let standard = self.standard;
        let mut frame = col().key(format!("{}-menu", self.key)).role("menu").focus_group().min_w(self.min_width).items_stretch();
        if let Some(label) = self.label {
            frame = frame.label(label);
        }
        let frame = if standard {
            k.elevate(frame.radius(k.radius.md).fill(c.surface_raised).border(1.0, k.alpha(c.std_border, 0.5)), &k.elevation.md)
        } else {
            k.elevate(frame.radius(6.0).fill(c.layer_01), &k.elevation.floating)
        };
        let mut list = col().key("rows").items_stretch().padding(if standard { 4.0 } else { 2.0 });
        for (i, entry) in self.entries.into_iter().enumerate() {
            list = list.child(match entry {
                MenuEntry::Item(item) => item.build(k, standard, format!("{}-item{i}", self.key), layer),
                MenuEntry::Separator => {
                    let (m, color) = if standard { (4.0, c.border_weak) } else { (2.0, c.border_muted) };
                    el().key(format!("sep{i}")).h(1.0).mt(m).mb(m).ml(-m).mr(-m).fill(color).a11y_hidden()
                }
                MenuEntry::Label(l) => {
                    let e = row().key(format!("label{i}")).center_y();
                    if standard {
                        e.px(8.0).py(4.0).child(text(l).font(k.ty.text_xsmall_medium).color(c.text_weak))
                    } else {
                        e.h(28.0).px(12.0).child(text(l).font(k.ty.small_medium).color(c.text_faint))
                    }
                }
            });
        }
        frame.child(fit_scroll_bar(k, &self.key, list))
    }

    pub fn build(mut self, k: &Kit, layer: &mut Vec<El>) -> El {
        let anchor_key = trigger_key(&self.key);
        // A dropdown's trigger reads as open or closed; a context menu's is just where it opened.
        let open = self.open;
        let pointer = self.context || self.at.is_some();
        let menu_key = format!("{}-menu", self.key);
        let trigger = anchor_trigger(
            anchor_key.clone(),
            self.trigger.take().map(|t| {
                if pointer {
                    t
                } else {
                    t.expanded(open).has_popup(rune::HasPopup::Menu).controls(menu_key)
                }
            }),
        );
        if !self.open {
            return trigger;
        }
        let (placement, gutter, key) = match self.at {
            Some((x, y)) => {
                let point = format!("{}-point", self.key);
                layer.push(el().key(point.clone()).absolute(x, y).size(0.0, 0.0).a11y_hidden());
                (Placement::BottomStart, 0.0, point)
            }
            None => (self.placement, self.gutter, anchor_key.clone()),
        };
        // Submenus anchor to items inside this surface, so they mount after it.
        let mut subs = Vec::new();
        let dismiss = self.dismiss.take();
        let surface = self.surface(k, &mut subs).focus_return(anchor_key);
        push_anchored_modal(k, layer, dismiss, surface, &key, placement, gutter);
        layer.extend(subs);
        trigger
    }
}
