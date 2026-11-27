//! Menu, context menu and their items (`@opencode/ui` navigation/menu, components/context-menu).
//! Compact (default) and standard appearances.
//!
//! Whether a menu is open is the app's (`open`, `on_close`); how it opens and closes is Rune's
//! modal stack (`runtime/input/modal_focus.rs`), which follows Desktop's menu machine
//! (`ui/src/navigation/menu/machine.ts`): focus returns to the trigger as the menu closes,
//! without a ring when the pointer opened it.

use std::any::Any;
use std::rc::Rc;
use std::time::Duration;

mod item;

pub use item::MenuItem;

use rune::{Color, El, Key, KeyEvent, PointerPress, Tx, col, el, row, text};

use super::overlay::{Modal, Placement, anchor_trigger, dismissal_tx, push_anchored_modal, scrim_key, trigger_key};
use super::popover::fit_scroll_bar;
use crate::Kit;

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

/// How long the pointer rests on a row before its submenu opens, or before a sibling row closes
/// an open submenu (so a diagonal move into the submenu crosses other rows without closing it).
const SUBMENU_HOVER_DELAY: Duration = Duration::from_millis(200);

type AnyState = dyn Any + 'static;
type SetOpen = Rc<dyn Fn(&mut AnyState, bool)>;
type HoverFn = Rc<dyn Fn(&mut AnyState, bool, &mut Tx)>;
type KeyFn = Rc<dyn Fn(&mut AnyState, &KeyEvent, &mut Tx) -> bool>;
type Bind<F> = Rc<dyn Fn(El, F) -> El>;
/// Closers of every controlled submenu open in a menu, its nested submenus' included.
type Closers = Rc<[SetOpen]>;
/// An item's select handler, attached once the menu knows which submenus to close with it.
type Select = Box<dyn FnOnce(El, Closers) -> El>;
/// `Menu::on_close`'s dismissal, built once the menu knows which submenus to close with it.
type Dismiss = Box<dyn FnOnce(Closers) -> (El, Modal)>;

/// Closes the menu's open controlled submenus, so the next time it opens they start closed.
fn close_submenus(closers: &[SetOpen], s: &mut AnyState) {
    for close in closers {
        close(&mut *s, false);
    }
}

/// Collects the closers of `entries`' open controlled submenus and of submenus open inside them.
fn open_submenus(entries: &[MenuEntry], out: &mut Vec<SetOpen>) {
    for entry in entries {
        if let MenuEntry::Item(item) = entry
            && let Some((sub, true)) = &item.sub
        {
            if let Some(t) = &item.sub_toggle {
                out.push(t.set.clone());
            }
            open_submenus(sub, out);
        }
    }
}

/// A submenu's open callback ([`MenuItem::on_submenu`]) with the app's state type erased, and
/// binders that attach state-typed hover and key handlers, so a menu can wire its rows without
/// being generic over the app.
#[derive(Clone)]
struct SubToggle {
    set: SetOpen,
    hover: Bind<HoverFn>,
    key: Bind<KeyFn>,
}

impl SubToggle {
    fn new<S: 'static>(f: impl Fn(&mut S, bool) + 'static) -> SubToggle {
        SubToggle {
            set: Rc::new(move |s: &mut AnyState, open: bool| {
                if let Some(s) = s.downcast_mut::<S>() {
                    f(s, open);
                }
            }),
            hover: Rc::new(|e: El, h: HoverFn| e.on_hover_change(move |s: &mut S, inside: bool, tx: &mut Tx| h(s as &mut AnyState, inside, tx))),
            key: Rc::new(|e: El, h: KeyFn| e.on_key(move |s: &mut S, ev: &KeyEvent, tx: &mut Tx| h(s as &mut AnyState, ev, tx))),
        }
    }
}

fn plain(ev: &KeyEvent) -> bool {
    ev.mods == rune::Mods::default()
}

/// The arrows that enter and leave a submenu where the key arrived: Right and Left, swapped
/// right to left, as the submenu's chevron and Rune's focus navigation swap.
fn submenu_arrows(tx: &Tx) -> (Key, Key) {
    match tx.key_direction() {
        rune::Direction::Rtl => (Key::Left, Key::Right),
        _ => (Key::Right, Key::Left),
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

/// What a row of a menu with controlled submenus does when the pointer rests on it.
struct RowHover {
    bind: Bind<HoverFn>,
    /// This row's entry index, so it doesn't close its own submenu.
    index: usize,
    /// The menu's open submenus: entry index and closer, shared by every row.
    close: Rc<[(usize, SetOpen)]>,
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
    /// Explicit anchor rectangle `[x, y, w, h]` in view points (e.g. from `PopupMenuRequest`).
    anchor_rect: Option<[f32; 4]>,
    /// Explicit focus-return target key when closing (defaults to `{key}-trigger`).
    focus_return: Option<String>,
    /// A context menu: its trigger is the area it opens over, not a button that opens it.
    context: bool,
    dismiss: Option<Dismiss>,
    /// Overrides of the appearance's padding, separator spacing and inner border.
    look: Look,
    /// Keys given to separators, by entry index; the rest are keyed `sep{index}`.
    sep_keys: Vec<(usize, String)>,
}

/// A menu's own padding, separator spacing, inner border and backdrop; `None` keeps the theme's.
#[derive(Clone, Copy, Default)]
struct Look {
    padding: Option<f32>,
    separator_spacing: Option<(f32, f32)>,
    border: Option<f32>,
    backdrop_blur: Option<f32>,
    surface_fill: Option<Color>,
}

#[derive(Clone, Copy)]
struct MenuStyle {
    standard: bool,
    look: Look,
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
            anchor_rect: None,
            focus_return: None,
            context: false,
            dismiss: None,
            look: Look::default(),
            sep_keys: Vec::new(),
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
    /// A separator with its own key, which stays put when entries are added above it
    /// (a plain [`Self::separator`], or a blank key, is keyed by its position, `sep{index}`).
    ///
    /// ```
    /// use rune_kit::{Menu, MenuItem};
    ///
    /// let _menu = Menu::new("m").item(MenuItem::new("Add").key("add")).separator_keyed("edit-sep");
    /// ```
    pub fn separator_keyed(mut self, key: impl Into<String>) -> Self {
        let key = key.into();
        if !key.trim().is_empty() {
            self.sep_keys.push((self.entries.len(), key));
        }
        self.entries.push(MenuEntry::Separator);
        self
    }
    /// The padding around the rows, in points: defaults to [`Kit::metrics`](crate::Kit::metrics)'s
    /// `menu` (4 for [`Self::standard`] menus and 2 otherwise). Propagates to open submenus;
    /// negative or non-finite values clamp to `0.0`.
    ///
    /// ```
    /// # use rune_kit::Menu;
    /// let _menu = Menu::new("file").padding(6.0);
    /// ```
    pub fn padding(mut self, p: f32) -> Self {
        self.look.padding = Some(crate::tokens::metrics::non_neg(p));
        self
    }
    /// The space above and below each separator, in points: defaults to this menu's padding when
    /// [`Self::padding`] is set, otherwise [`Kit::metrics`](crate::Kit::metrics)'s `menu`. The
    /// separator line still runs out to the padding's edges. Propagates to open submenus;
    /// negative or non-finite values clamp to `0.0`.
    ///
    /// ```
    /// # use rune_kit::Menu;
    /// let _menu = Menu::new("file").separator_spacing(3.0, 6.0);
    /// ```
    pub fn separator_spacing(mut self, above: f32, below: f32) -> Self {
        let nonneg = crate::tokens::metrics::non_neg;
        self.look.separator_spacing = Some((nonneg(above), nonneg(below)));
        self
    }
    /// The width of a [`Self::standard`] menu's inner border, in points (defaults to
    /// [`Kit::metrics`](crate::Kit::metrics)'s `menu.standard_inner_border`, 1 by default; 0
    /// draws none). Other menus have no border unless this sets one. Propagates to open
    /// submenus; negative or non-finite values clamp to `0.0`.
    ///
    /// ```
    /// # use rune_kit::Menu;
    /// let _menu = Menu::new("file").standard().inner_border(0.0);
    /// ```
    pub fn inner_border(mut self, width: f32) -> Self {
        self.look.border = Some(crate::tokens::metrics::non_neg(width));
        self
    }
    /// Blurs what the window draws behind the menu surface ([`El::backdrop_blur`](rune::El::backdrop_blur)),
    /// overriding [`Kit::metrics`](crate::Kit::metrics)'s `popup_backdrop` for this menu and its
    /// open submenus. Pair with a translucent [`Self::surface_fill`] (or a translucent theme fill)
    /// so the blur shows through. `0.0` turns blur off; negative or non-finite values clamp to `0.0`.
    ///
    /// ```
    /// # use rune_kit::{Kit, Menu};
    /// # let k = Kit::neutral(true);
    /// let _menu = Menu::new("ctx")
    ///     .backdrop_blur(20.0)
    ///     .surface_fill(k.c.layer_01.alpha(0.78));
    /// ```
    pub fn backdrop_blur(mut self, sigma: f32) -> Self {
        self.look.backdrop_blur = Some(crate::tokens::metrics::non_neg(sigma));
        self
    }
    /// Replaces the menu surface's background fill colour (for example a translucent tint over
    /// [`Self::backdrop_blur`]), overriding the theme fill and `Kit::metrics.popup_backdrop` for
    /// this menu and its open submenus.
    ///
    /// ```
    /// # use rune_kit::{Kit, Menu};
    /// # let k = Kit::neutral(true);
    /// let _menu = Menu::new("ctx").surface_fill(k.c.layer_01.alpha(0.78));
    /// ```
    pub fn surface_fill(mut self, fill: Color) -> Self {
        self.look.surface_fill = Some(fill);
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
    pub fn on_close<S: 'static>(self, f: impl Fn(&mut S) + 'static) -> Self {
        self.on_close_tx(move |s: &mut S, _: &mut Tx| f(s))
    }
    /// Like [`Self::on_close`], also passing the transaction [`Tx`] so the close handler can
    /// resolve a pop-up menu ([`Tx::resolve_popup_menu`]) or move focus in the same commit.
    ///
    /// ```
    /// use rune::Tx;
    /// use rune_kit::Menu;
    ///
    /// struct App { open: bool }
    /// let _menu = Menu::new("file").on_close_tx(|s: &mut App, tx: &mut Tx| {
    ///     s.open = false;
    ///     tx.resolve_popup_menu(None);
    /// });
    /// ```
    pub fn on_close_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        let key = scrim_key(&self.key);
        // Escape and an outside press also close the open controlled submenus, so the menu
        // reopens with them closed.
        self.dismiss = Some(Box::new(move |closers: Closers| {
            dismissal_tx(key, move |s: &mut S, tx: &mut Tx| {
                close_submenus(&closers, s);
                f(s, tx)
            })
        }));
        self
    }
    pub(super) fn with_anchor_rect(mut self, rect: [f32; 4]) -> Self {
        self.anchor_rect = Some(rect);
        self
    }
    pub(super) fn with_focus_return(mut self, key: String) -> Self {
        self.focus_return = Some(key);
        self
    }

    fn surface(self, k: &Kit, tree: &Closers, layer: &mut Vec<El>) -> El {
        let c = &k.c;
        let standard = self.standard;
        let mut frame = col().key(format!("{}-menu", self.key)).role("menu").focus_group().min_w(self.min_width).items_stretch();
        if let Some(label) = self.label {
            frame = frame.label(label);
        }
        let look = self.look;
        let mm = k.menu_metrics();
        let row_m = k.popup_row_metrics();
        let border = look.border.unwrap_or(if standard { mm.standard_inner_border } else { 0.0 });
        let frame_radius = k.radius_md();
        let base_fill = if standard { c.surface_raised } else { c.layer_01 };
        let frame = k.apply_popup_surface(frame.radius(frame_radius), base_fill, look.backdrop_blur, look.surface_fill);
        let frame = if border > 0.0 { frame.border(border, k.alpha(c.std_border, 0.5)) } else { frame };
        let frame = k.elevate(frame, if standard { &k.elevation.md } else { &k.elevation.floating });
        let pad = look.padding.unwrap_or(if standard { mm.standard_padding } else { mm.padding });
        let (sep_above, sep_below) = look.separator_spacing.unwrap_or_else(|| {
            if look.padding.is_some() {
                (pad, pad)
            } else if standard {
                (mm.standard_separator_above, mm.standard_separator_below)
            } else {
                (mm.separator_above, mm.separator_below)
            }
        });
        let mut list = col().key("rows").items_stretch().padding(pad);
        // Rows hear a resting pointer only when some row here has a submenu the menu controls:
        // that row opens on it, and every other row closes the open one.
        let toggles: Vec<(usize, SubToggle, bool)> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| match e {
                MenuEntry::Item(item) => {
                    let open = item.sub.as_ref()?.1;
                    item.sub_toggle.clone().map(|t| (i, t, open))
                }
                _ => None,
            })
            .collect();
        let bind = toggles.first().map(|(_, t, _)| t.hover.clone());
        let open_closers: Rc<[(usize, SetOpen)]> =
            toggles.iter().filter(|(_, _, open)| *open).map(|(i, t, _)| (*i, t.set.clone())).collect();
        for (i, entry) in self.entries.into_iter().enumerate() {
            list = list.child(match entry {
                MenuEntry::Item(item) => {
                    let hover = bind.clone().map(|bind| RowHover { bind, index: i, close: open_closers.clone() });
                    item.build(k, MenuStyle { standard, look }, format!("{}-item{i}", self.key), hover, tree, layer)
                }
                MenuEntry::Separator => {
                    let color = if standard { c.border_weak } else { c.border_muted };
                    let key = match self.sep_keys.iter().find(|(at, _)| *at == i) {
                        Some((_, key)) => key.clone(),
                        None => format!("sep{i}"),
                    };
                    el().key(key).h(1.0).mt(sep_above).mb(sep_below).ml(-pad).mr(-pad).fill(color).a11y_hidden()
                }
                MenuEntry::Label(l) => {
                    let e = row().key(format!("label{i}")).center_y();
                    if standard {
                        e.px(8.0).py(4.0).child(text(l).font(k.ty.text_xsmall_medium).color(c.text_weak))
                    } else {
                        e.h(row_m.height).px(12.0).child(text(l).font(k.ty.small_medium).color(c.text_faint))
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
        let (placement, gutter, key) = match (self.anchor_rect, self.at) {
            (Some([x, y, w, h]), _) => {
                let rect_key = format!("{}-anchor", self.key);
                layer.push(el().key(rect_key.clone()).absolute(x, y).size(w, h).a11y_hidden());
                (self.placement, self.gutter, rect_key)
            }
            (None, Some((x, y))) => {
                let point = format!("{}-point", self.key);
                layer.push(el().key(point.clone()).absolute(x, y).size(0.0, 0.0).a11y_hidden());
                (Placement::BottomStart, 0.0, point)
            }
            (None, None) => (self.placement, self.gutter, anchor_key.clone()),
        };
        // Submenus anchor to items inside this surface, so they mount after it.
        let mut subs = Vec::new();
        let mut open_subs = Vec::new();
        open_submenus(&self.entries, &mut open_subs);
        let tree: Closers = open_subs.into();
        let dismiss = self.dismiss.take().map(|d| d(tree.clone()));
        let return_to = self.focus_return.take().unwrap_or(anchor_key);
        let surface = self.surface(k, &tree, &mut subs).focus_return(return_to);
        push_anchored_modal(k, layer, dismiss, surface, &key, placement, gutter);
        layer.extend(subs);
        trigger
    }
}
