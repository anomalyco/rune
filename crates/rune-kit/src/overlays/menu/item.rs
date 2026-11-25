//! Menu item builder and row rendering for [`super::Menu`].

use std::rc::Rc;
use std::time::Duration;

use rune::{El, Icon, Key, KeyEvent, TextStyle, Tx, col, el, icon, row, text};

use super::super::overlay::Placement;
use super::super::popover::anchored;
use super::{
    AnyState, Closers, Menu, MenuEntry, MenuStyle, RowHover, SUBMENU_HOVER_DELAY, Select, SubToggle,
    close_submenus, plain, submenu_arrows,
};
use crate::{Kit, State};

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
    pub(super) sub: Option<(Vec<MenuEntry>, bool)>,
    sub_placement: Placement,
    sub_min_width: f32,
    pub(super) sub_toggle: Option<SubToggle>,
    state: State,
    selects: Vec<Select>,
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
            sub_toggle: None,
            state: State::Rest,
            selects: Vec::new(),
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
    /// Lets the menu open and close this item's submenu: `f(state, open)` should store `open`
    /// for the next [`Self::submenu`]. With it, Right, Return and Space on the row open the
    /// submenu and move focus into it, and Left or Escape inside it closes only the submenu and
    /// returns focus to the row (the WAI-ARIA menu pattern). Right to left ([`El::dir`]), Left
    /// opens and Right closes, as the row's chevron points. The keys don't change when the
    /// submenu flips to the other side for lack of room. Resting the pointer on the row opens
    /// it, and resting on another row of the same menu (disabled rows included) closes it, after
    /// a short delay.
    ///
    /// The menu also closes its open submenus through `f` when it is dismissed
    /// ([`Menu::on_close`]: Escape, a press outside) and when an item other than a checkbox or
    /// radio is picked ([`Self::on_select`]). An app that closes the menu any other way (setting
    /// its `open` flag itself) clears the submenu flags too, or the menu reopens with them open.
    ///
    /// All `on_submenu` callbacks in one menu must take the same state type: the menu wires its
    /// rows' hover with the first one's, and a callback for another type does nothing on hover.
    ///
    /// ```
    /// use rune::El;
    /// use rune_kit::{Button, Kit, Menu, MenuEntry, MenuItem};
    ///
    /// #[derive(Default)]
    /// struct App { menu_open: bool, share_open: bool }
    ///
    /// fn file_menu(s: &App, k: &Kit, layer: &mut Vec<El>) -> El {
    ///     Menu::new("file")
    ///         .trigger(Button::new("File").build(k))
    ///         .item(
    ///             MenuItem::new("Share")
    ///                 .submenu(vec![MenuEntry::item(MenuItem::new("Mail"))], s.share_open)
    ///                 .on_submenu(|s: &mut App, open| s.share_open = open),
    ///         )
    ///         .open(s.menu_open)
    ///         // The menu clears `share_open` itself before this runs.
    ///         .on_close(|s: &mut App| s.menu_open = false)
    ///         .build(k, layer)
    /// }
    /// # let _ = file_menu(&App::default(), &Kit::neutral(true), &mut Vec::new());
    /// ```
    pub fn on_submenu<S: 'static>(mut self, f: impl Fn(&mut S, bool) + 'static) -> Self {
        self.sub_toggle = Some(SubToggle::new(f));
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
    /// Runs `f` when the item is picked. Picking an item that isn't a checkbox or radio
    /// ([`Self::checked`]) also closes the menu's open [`Self::on_submenu`] submenus first.
    pub fn on_select<S: 'static>(self, f: impl Fn(&mut S) + 'static) -> Self {
        self.on_select_tx(move |s: &mut S, _: &mut Tx| f(s))
    }
    pub fn on_select_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.selects.push(Box::new(move |e: El, closers: Closers| {
            e.on_press_tx(move |s: &mut S, tx: &mut Tx| {
                close_submenus(&closers, s);
                f(s, tx)
            })
        }));
        self
    }

    pub(super) fn build(self, k: &Kit, style: MenuStyle, fallback_key: String, row_hover: Option<RowHover>, tree: &Closers, layer: &mut Vec<El>) -> El {
        let MenuStyle { standard, look } = style;
        let key = self.key.unwrap_or(fallback_key);
        let a11y_name = self.a11y_label.unwrap_or_else(|| self.label.clone());
        let c = &k.c;
        let row_m = k.popup_row_metrics();
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
            .radius(if standard { k.radius.sm } else { row_m.radius })
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
            (false, false) => e.h(row_m.height).pl(12.0).pr(6.0),
            (false, true) => e.min_h(row_m.height).py(6.0).pl(12.0).pr(6.0),
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
        let sub_open = self.sub.as_ref().is_some_and(|s| s.1);
        let toggle = self.sub.as_ref().and(self.sub_toggle.clone()).filter(|_| !disabled);
        if let Some(t) = &toggle {
            let set = t.set.clone();
            let sub_menu = format!("{key}-sub-menu");
            e = (t.key)(
                e,
                Rc::new(move |s: &mut AnyState, ev: &KeyEvent, tx: &mut Tx| {
                    if !plain(ev) || !(ev.key == submenu_arrows(tx).0 || ev.key == Key::Enter || ev.key == Key::Text(" ".into())) {
                        return false;
                    }
                    if !sub_open {
                        set(s, true);
                    }
                    // Its first focusable item (APG menu: the submenu opens with focus inside).
                    tx.focus_when_shown(sub_menu.clone(), true);
                    true
                }),
            );
        }
        if let Some(h) = row_hover {
            // A disabled row still closes an open sibling submenu; it just opens nothing.
            let own = toggle.as_ref().filter(|_| !sub_open).map(|t| t.set.clone());
            let RowHover { bind, index, close: closers } = h;
            e = bind(
                e.hover_delays(SUBMENU_HOVER_DELAY, Duration::ZERO),
                Rc::new(move |s: &mut AnyState, inside: bool, _: &mut Tx| {
                    if !inside {
                        return;
                    }
                    for (i, close) in closers.iter() {
                        if *i != index {
                            close(&mut *s, false);
                        }
                    }
                    if let Some(open) = &own {
                        open(&mut *s, true);
                    }
                }),
            );
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
                let sub = Menu { key: format!("{key}-sub"), label, entries, standard, min_width, look, ..Menu::new("") };
                let mut deeper = Vec::new();
                let mut surface = sub.surface(k, tree, &mut deeper);
                if let Some(t) = &toggle {
                    let (set, row_key) = (t.set.clone(), key.clone());
                    surface = (t.key)(
                        surface,
                        Rc::new(move |s: &mut AnyState, ev: &KeyEvent, tx: &mut Tx| {
                            if !plain(ev) || !(ev.key == submenu_arrows(tx).1 || ev.key == Key::Escape) {
                                return false;
                            }
                            set(s, false);
                            tx.focus(row_key.clone(), true);
                            true
                        }),
                    );
                }
                layer.push(anchored(k, surface, &key, self.sub_placement, 0.0));
                layer.extend(deeper);
            }
        }
        e = k.disabled_dim(e, disabled && !standard);
        // A popup opens on its checked item (NSPopUpButton).
        let mut e = e.focusable(!disabled).focus_selected(self.checked == Some(true)).disabled(disabled);
        if !disabled {
            // A checkbox or radio pick may keep the menu open, so it leaves submenus alone.
            let closers: Closers = if self.checked.is_some() { Rc::new([]) } else { tree.clone() };
            for select in self.selects {
                e = select(e, closers.clone());
            }
        }
        e
    }
}
