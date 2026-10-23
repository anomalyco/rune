//! Menu, context menu and their items (`@opencode/ui` navigation/menu, components/context-menu).
//! Compact (default) and standard appearances.

use rune::{El, Icon, PointerPress, TextStyle, Tx, col, el, icon, row, text};

use crate::overlay::{Placement, anchor, scrim};
use crate::{Acts, Kit, State, shadows};

/// A menu at a secondary-press position. Store the point from `El::on_secondary_press`
/// in app state; clear it in `on_close` and item callbacks. Uses Menu's flipping anchor.
pub fn context_menu(key: impl Into<String>, trigger: El, point: Option<PointerPress>) -> Menu {
    let menu = Menu::new(key).trigger(trigger).open(point.is_some());
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
    label: String,
    icon: Option<&'static Icon>,
    lead: Option<El>,
    shortcut: Option<String>,
    badge: Option<String>,
    /// Checkbox and radio items: whether the indicator shows.
    checked: Option<bool>,
    sub: Option<(Vec<MenuEntry>, bool)>,
    state: State,
    acts: Acts,
}

impl MenuItem {
    pub fn new(label: impl Into<String>) -> MenuItem {
        MenuItem {
            label: label.into(),
            icon: None,
            lead: None,
            shortcut: None,
            badge: None,
            checked: None,
            sub: None,
            state: State::Rest,
            acts: Acts::default(),
        }
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
    /// A checkbox or radio item, showing the check when `on`.
    pub fn checked(mut self, on: bool) -> Self {
        self.checked = Some(on);
        self
    }
    /// A submenu trigger; `open` shows its entries to the right.
    pub fn submenu(mut self, entries: Vec<MenuEntry>, open: bool) -> Self {
        self.sub = Some((entries, open));
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

    fn build(self, k: &Kit, standard: bool, key: String, layer: &mut Vec<El>) -> El {
        let c = &k.c;
        let disabled = self.state == State::Disabled;
        let highlighted = self.state == State::Hover || self.sub.as_ref().is_some_and(|s| s.1);
        let hover = if standard { c.surface_raised_hover } else { c.hover };
        let (font, ink) = if standard {
            (k.ty.text_small_medium, if disabled { c.text_weak } else { c.text_strong })
        } else {
            (k.ty.ui_compact, c.text_base)
        };
        let mut content = row().key("content").grow(1.0).shrink().gap(8.0).center_y();
        if let Some(l) = self.lead {
            content = content.child(l);
        }
        if let Some(i) = self.icon {
            content = content.child(icon(i, 14.0).key("icon").color(if standard { c.std_icon } else { c.icon_base }));
        }
        content = content.child(text(self.label.clone()).key("label").font(font).color(ink).truncate());
        let mut e = row()
            .key(key.clone())
            .role("menuitem")
            .label(self.label)
            .radius(if standard { k.radius.sm } else { 4.0 })
            .gap(8.0)
            .center_y();
        e = if standard { e.px(8.0).py(4.0) } else { e.h(28.0).pl(12.0).pr(6.0) };
        if highlighted {
            e = e.fill(hover);
        } else if !disabled {
            e = e.hover(hover);
        }
        e = e.child(content);
        if let Some(s) = self.shortcut {
            let st = TextStyle { line_height: 11.0, ..k.ty.small_medium };
            e = e.child(text(s).key("shortcut").font(st).color(c.text_faint));
        }
        if let Some(b) = self.badge {
            e = e.child(
                row()
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
            e = e.child(el().key("indicator").size(16.0, 16.0).center().child(mark));
        }
        if let Some((entries, open)) = self.sub {
            e = e.child(icon(k.glyph.caret_right, 16.0).key("chevron").color(if standard {
                c.std_icon
            } else {
                c.text_faint
            }));
            if open {
                let sub = Menu { key: format!("{key}-sub"), entries, standard, min_width: 160.0, ..Menu::new("") };
                let mut deeper = Vec::new();
                let surface = sub.surface(k, &mut deeper);
                layer.push(anchor(k, surface, &key, Placement::RightStart, 0.0));
                layer.extend(deeper);
            }
        }
        if disabled && !standard {
            e = e.dim(0.5);
        }
        self.acts.apply(e.focusable(!disabled).focus_ring(1.0, -1.0, c.border_focus), !disabled)
    }
}

/// A dropdown menu: `Menu::new("file").trigger(button).item(..).open(s.open).build(&kit, &mut layer)`.
pub struct Menu {
    key: String,
    trigger: Option<El>,
    entries: Vec<MenuEntry>,
    open: bool,
    standard: bool,
    placement: Placement,
    gutter: f32,
    min_width: f32,
    at: Option<(f32, f32)>,
    dismiss: Option<El>,
}

impl Menu {
    pub fn new(key: impl Into<String>) -> Menu {
        Menu {
            key: key.into(),
            trigger: None,
            entries: Vec::new(),
            open: false,
            standard: false,
            placement: Placement::BottomStart,
            gutter: 6.0,
            min_width: 160.0,
            at: None,
            dismiss: None,
        }
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
    /// Presses outside close the menu.
    pub fn on_close<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.dismiss = Some(scrim(format!("{}-scrim", self.key), f));
        self
    }

    fn surface(self, k: &Kit, layer: &mut Vec<El>) -> El {
        let c = &k.c;
        let standard = self.standard;
        let mut list = col().key(format!("{}-menu", self.key)).role("menu").focus_group().min_w(self.min_width).items_stretch();
        list = if standard {
            shadows(
                k,
                list.padding(4.0).radius(k.radius.md).fill(c.surface_raised).border(1.0, k.alpha(c.std_border, 0.5)),
                &k.elevation.md,
            )
        } else {
            shadows(k, list.padding(2.0).radius(6.0).fill(c.layer_01), &k.elevation.floating)
        };
        for (i, entry) in self.entries.into_iter().enumerate() {
            list = list.child(match entry {
                MenuEntry::Item(item) => item.build(k, standard, format!("{}-item{i}", self.key), layer),
                MenuEntry::Separator => {
                    let (m, color) = if standard { (4.0, c.border_weak) } else { (2.0, c.border_muted) };
                    el().key(format!("sep{i}")).h(1.0).mt(m).mb(m).ml(-m).mr(-m).fill(color)
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
        list
    }

    pub fn build(mut self, k: &Kit, layer: &mut Vec<El>) -> El {
        let anchor_key = format!("{}-trigger", self.key);
        let trigger = row().key(anchor_key.clone()).children(self.trigger.take());
        if !self.open {
            return trigger;
        }
        if let Some(s) = self.dismiss.take() {
            layer.push(s);
        }
        let (placement, gutter, key) = match self.at {
            Some((x, y)) => {
                let point = format!("{}-point", self.key);
                layer.push(el().key(point.clone()).absolute(x, y).size(0.0, 0.0));
                (Placement::BottomStart, 0.0, point)
            }
            None => (self.placement, self.gutter, anchor_key.clone()),
        };
        // Submenus anchor to items inside this surface, so they mount after it.
        let mut subs = Vec::new();
        let surface = self.surface(k, &mut subs).focus_return(anchor_key);
        layer.push(anchor(k, surface, &key, placement, gutter));
        layer.extend(subs);
        trigger
    }
}
