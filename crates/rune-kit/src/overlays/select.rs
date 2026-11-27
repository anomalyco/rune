//! Select (`@opencode/ui` forms/select): an inline trigger and a listbox of menu items with a
//! check on the selected option, optionally grouped.
//!
//! Whether it's open is the app's (`open`, `on_close`); how it opens and closes is Rune's
//! modal stack (`runtime/input/modal_focus.rs`): it takes focus, Escape and a press outside
//! close it, and focus goes back to the trigger, without a ring when the pointer opened it, as
//! Desktop's menu machine has it (`ui/src/navigation/menu/machine.ts`).

use rune::{Color, El, Icon, TextStyle, Tx, col, el, icon, row, text};

use super::overlay::{Modal, Placement, content_key, dismissal, dismissal_tx, push_anchored_modal, scrim_key, trigger_key};
use super::popover::fit_scroll_bar;
use crate::{Kit, State, over};

/// The listbox's tallest: its 4-point padding, six rows (2 points apart), and most of the
/// seventh, whose label shows fading under the edge (206 points at the default 28-point row).
fn listbox_max_height(row_height: f32) -> f32 {
    4.0 + 6.0 * (row_height + 2.0) + (row_height - 6.0).max(0.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SelectAppearance {
    /// The trigger the kit renders today: borderless, 24 points tall.
    #[default]
    Inline,
    /// The 280 point field look (`data-appearance` unset).
    Field,
}

type Choose = Box<dyn Fn(&str) -> Box<dyn FnOnce(El) -> El>>;
type StepClosed = Box<dyn FnOnce(El, Vec<String>, Option<String>) -> El>;

/// Previous or next option in `options` while a select is closed, clamping at both ends.
/// When `current` is absent (or no longer in `options`), Down picks the first and Up the last.
fn step_option<'a>(options: &'a [String], current: Option<&str>, down: bool) -> Option<&'a str> {
    if options.is_empty() {
        return None;
    }
    let next = match current.and_then(|c| options.iter().position(|o| o == c)) {
        Some(i) if down => (i + 1).min(options.len() - 1),
        Some(i) => i.saturating_sub(1),
        None if down => 0,
        None => options.len() - 1,
    };
    Some(options[next].as_str())
}

/// `Select::new("fruit", &FRUITS).current(s.fruit.as_deref()).open(..).on_choose(|s, v| ..)`.
pub struct Select {
    key: String,
    options: Vec<(Option<String>, String)>,
    details: Vec<Option<String>>,
    width: f32,
    fill: bool,
    menu_width: Option<f32>,
    current: Option<String>,
    placeholder: String,
    name: Option<String>,
    open: bool,
    step_closed: bool,
    invalid: bool,
    state: State,
    appearance: SelectAppearance,
    highlighted: Option<String>,
    placement: Placement,
    backdrop_blur: Option<f32>,
    surface_fill: Option<Color>,
    choose: Option<Choose>,
    step_key: Option<StepClosed>,
    toggle: Option<Box<dyn FnOnce(El) -> El>>,
    dismiss: Option<(El, Modal)>,
}

impl Select {
    pub fn new<S: AsRef<str>>(key: impl Into<String>, options: &[S]) -> Select {
        Select {
            key: key.into(),
            options: options.iter().map(|o| (None, o.as_ref().to_owned())).collect(),
            details: Vec::new(),
            width: 280.0,
            fill: false,
            menu_width: None,
            current: None,
            placeholder: String::new(),
            name: None,
            open: false,
            step_closed: false,
            invalid: false,
            state: State::Rest,
            appearance: SelectAppearance::Inline,
            highlighted: None,
            placement: Placement::BottomEnd,
            backdrop_blur: None,
            surface_fill: None,
            choose: None,
            step_key: None,
            toggle: None,
            dismiss: None,
        }
    }
    /// Options under section headers: `(group, option)` pairs, in order.
    pub fn grouped(key: impl Into<String>, options: &[(&str, &str)]) -> Select {
        let mut s = Select::new::<&str>(key, &[]);
        s.options = options.iter().map(|(g, o)| (Some((*g).to_owned()), (*o).to_owned())).collect();
        s
    }
    /// Secondary text after each option (in option order; `None` for none).
    pub fn details<S: AsRef<str>>(mut self, d: &[Option<S>]) -> Self {
        self.details = d.iter().map(|x| x.as_ref().map(|s| s.as_ref().to_owned())).collect();
        self
    }
    /// The field trigger's width in points (default 280). Ignored when [`Self::fill`] is set
    /// or when the appearance is [`SelectAppearance::Inline`].
    pub fn width(mut self, w: f32) -> Self {
        self.width = w;
        self
    }
    /// The [`SelectAppearance::Field`] trigger fills its parent's width instead of using
    /// [`Self::width`] (taking precedence regardless of call order), shrinking inside a
    /// horizontal row beside a label. Has no effect on [`SelectAppearance::Inline`].
    ///
    /// ```
    /// use rune_kit::{Select, SelectAppearance};
    ///
    /// let _select = Select::new("theme", &["System", "Light", "Dark"])
    ///     .appearance(SelectAppearance::Field)
    ///     .fill();
    /// ```
    pub fn fill(mut self) -> Self {
        self.fill = true;
        self
    }
    /// The open listbox's width in points for any [`SelectAppearance`], such as the trigger's
    /// measured width (clamped to the window bounds when wider than the viewport). Without it
    /// the listbox sizes itself to its labels, between 160 and 368 points.
    ///
    /// ```
    /// use rune_kit::{Select, SelectAppearance};
    ///
    /// let _select = Select::new("theme", &["System", "Light", "Dark"])
    ///     .appearance(SelectAppearance::Field)
    ///     .fill()
    ///     .menu_width(320.0);
    /// ```
    pub fn menu_width(mut self, w: f32) -> Self {
        self.menu_width = w.is_finite().then_some(w.max(0.0));
        self
    }
    pub fn current(mut self, c: Option<&str>) -> Self {
        self.current = c.map(str::to_owned);
        self
    }
    pub fn placeholder(mut self, p: impl Into<String>) -> Self {
        self.placeholder = p.into();
        self
    }
    /// The accessible name: what the choice is for ("Model"). Assistive technology reads the
    /// chosen option as its value. Without it the placeholder names the select.
    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.name = Some(l.into());
        self
    }
    pub fn open(mut self, o: bool) -> Self {
        self.open = o;
        self
    }
    /// Step through options with Up and Down while the select is closed, calling
    /// [`Self::on_choose`] without opening the popup (matching Chromium and Windows native
    /// `<select>` behaviour).
    ///
    /// Defaults to `false`, where Up and Down open the popup (AppKit `NSPopUpButton`). When
    /// enabled, Up and Down clamp at the first and last option, start from the first option on
    /// Down or last option on Up when [`Self::current`] is `None`, and leave `Alt+Down`, Space,
    /// and pointer press opening the popup as usual.
    ///
    /// ```
    /// use rune_kit::Select;
    ///
    /// struct App { theme: String, open: bool }
    /// let _select = Select::new("theme", &["System", "Light", "Dark"])
    ///     .current(Some("Light"))
    ///     .step_closed(true)
    ///     .on_choose(|s: &mut App, v: &str, _| s.theme = v.to_owned());
    /// ```
    pub fn step_closed(mut self, enabled: bool) -> Self {
        self.step_closed = enabled;
        self
    }
    pub fn invalid(mut self, i: bool) -> Self {
        self.invalid = i;
        self
    }
    pub fn appearance(mut self, a: SelectAppearance) -> Self {
        self.appearance = a;
        self
    }
    /// The keyboard-highlighted option while open.
    pub fn highlighted(mut self, o: Option<&str>) -> Self {
        self.highlighted = o.map(str::to_owned);
        self
    }
    pub fn placement(mut self, p: Placement) -> Self {
        self.placement = p;
        self
    }
    /// Blurs what the window draws behind the open listbox surface ([`El::backdrop_blur`](rune::El::backdrop_blur)),
    /// overriding [`Kit::metrics`](crate::Kit::metrics)'s `popup_backdrop`. `0.0` turns blur off;
    /// negative or non-finite values clamp to `0.0`.
    pub fn backdrop_blur(mut self, sigma: f32) -> Self {
        self.backdrop_blur = Some(crate::tokens::metrics::non_neg(sigma));
        self
    }
    /// Replaces the open listbox's surface fill colour (for example a translucent tint over
    /// [`Self::backdrop_blur`]), overriding the theme fill and `Kit::metrics.popup_backdrop`.
    pub fn surface_fill(mut self, fill: Color) -> Self {
        self.surface_fill = Some(fill);
        self
    }
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
    /// The trigger's press (open or close the listbox).
    pub fn on_toggle<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.toggle = Some(Box::new(move |e: El| e.on_press(f)));
        self
    }
    /// [`Self::on_toggle`] whose callback receives the change's [`Tx`], so opening or closing
    /// can focus, scroll, or announce in the same commit.
    ///
    /// ```
    /// use rune::Tx;
    /// use rune_kit::Select;
    ///
    /// struct App { open: bool }
    /// let _select = Select::new("fruit", &["Apple", "Pear"]).on_toggle_tx(|s: &mut App, tx: &mut Tx| {
    ///     s.open = !s.open;
    ///     tx.instant();
    /// });
    /// ```
    pub fn on_toggle_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.toggle = Some(Box::new(move |e: El| e.on_press_tx(f)));
        self
    }
    /// Presses outside and Escape close the listbox. While open it holds keyboard focus.
    pub fn on_close<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.dismiss = Some(dismissal(scrim_key(&self.key), f));
        self
    }
    /// [`Self::on_close`] whose callback receives the change's [`Tx`], so closing on an outside
    /// press or Escape can focus, scroll, or announce in the same commit.
    ///
    /// ```
    /// use rune::Tx;
    /// use rune_kit::Select;
    ///
    /// struct App { open: bool }
    /// let _select = Select::new("fruit", &["Apple", "Pear"]).on_close_tx(|s: &mut App, tx: &mut Tx| {
    ///     s.open = false;
    ///     tx.focus("after", true);
    /// });
    /// ```
    pub fn on_close_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.dismiss = Some(dismissal_tx(scrim_key(&self.key), f));
        self
    }
    /// Choosing an option: `f(state, option, tx)`.
    pub fn on_choose<S: 'static>(mut self, f: impl Fn(&mut S, &str, &mut Tx) + Clone + 'static) -> Self {
        let step = f.clone();
        self.choose = Some(Box::new(move |v: &str| {
            let (f, v) = (f.clone(), v.to_owned());
            Box::new(move |e: El| e.on_press_tx(move |s: &mut S, tx: &mut Tx| f(s, &v, tx)))
        }));
        self.step_key = Some(Box::new(move |e: El, options: Vec<String>, current: Option<String>| {
            e.on_key(move |s: &mut S, ev: &rune::KeyEvent, tx: &mut Tx| {
                if ev.mods != rune::Mods::default() || !matches!(ev.key, rune::Key::Up | rune::Key::Down) {
                    return false;
                }
                let Some(next) = step_option(&options, current.as_deref(), ev.key == rune::Key::Down) else {
                    return true;
                };
                if current.as_deref() != Some(next) {
                    step(s, next, tx);
                }
                true
            })
        }));
        self
    }

    /// The field alone, for an app that opens its own menu or picker on press (`on_toggle`):
    /// `open` still turns the chevron and highlights the field, but no listbox is built.
    pub fn trigger(mut self, k: &Kit) -> El {
        self.field(k)
    }

    pub fn build(mut self, k: &Kit, layer: &mut Vec<El>) -> El {
        let trigger = self.field(k)
            .has_popup(rune::HasPopup::Listbox)
            .controls(content_key(&self.key));
        if self.state == State::Disabled || !self.open {
            return trigger;
        }
        let c = &k.c;
        let row_m = k.popup_row_metrics();
        let anchor_key = trigger_key(&self.key);
        // About six and three-quarter rows tall at most, so a longer list shows its next row's label
        // fading under the edge and reads as one that scrolls (hex#840); consecutive options
        // sit 2 points apart, headers flush.
        let mut list = col().key("rows").padding(4.0).items_stretch();
        let mut group: Option<&str> = None;
        let mut after_item = false;
        let count = self.options.len();
        for (i, (g, o)) in self.options.iter().enumerate() {
            if g.as_deref() != group {
                group = g.as_deref();
                if let Some(g) = group {
                    list = list.child(
                        row()
                            .key(format!("group{i}"))
                            .h(row_m.height)
                            .px(12.0)
                            .center_y()
                            .child(text(g).font(k.ty.small_medium).color(c.text_faint)),
                    );
                    after_item = false;
                }
            }
            let selected = self.current.as_deref() == Some(o.as_str());
            let mut item = row()
                .key(format!("option{i}"))
                .role("option")
                .focusable(true).focus_selected(selected)
                .label(o.clone())
                .position_in_set(i + 1, count)
                .h(row_m.height)
                .pl(12.0)
                .pr(6.0)
                .gap(8.0)
                .center_y()
                .radius(row_m.radius)
                .mt(if after_item { 2.0 } else { 0.0 })
                .child(
                    text(o.clone()).key("label").font(k.ty.ui_compact).color(c.text_base).grow(1.0).shrink().truncate().a11y_hidden(),
                )
                .child(el().key("indicator").size(16.0, 16.0).center().a11y_hidden().child(
                    icon(k.glyph.check_small, 14.0).color(c.icon_base).opacity(if selected { 1.0 } else { 0.0 }),
                ));
            if let Some(d) = self.details.get(i).cloned().flatten() {
                item = item.description(d);
            }
            if self.highlighted.as_deref() == Some(o.as_str()) {
                item = item.fill(c.hover);
            } else {
                item = item.focus_highlight(c.hover);
            }
            if let Some(f) = &self.choose {
                item = f(o)(item);
            }
            list = list.child(item);
            after_item = true;
        }
        let name = self.name.clone().unwrap_or_else(|| self.placeholder.clone());
        let listbox = col()
            .key(content_key(&self.key))
            .role("listbox")
            .label(name)
            .focus_group()
            .focus_return(&anchor_key)
            .max_h(listbox_max_height(row_m.height))
            .min_h(0.0)
            .shrink();
        let listbox = match self.menu_width {
            Some(w) => listbox.w(w),
            None => listbox.min_w(160.0).max_w(368.0),
        };
        let surface = k.apply_popup_surface(
            listbox.radius(k.radius_md()),
            c.layer_01,
            self.backdrop_blur,
            self.surface_fill,
        );
        let content = k.elevate(
            surface.child(fit_scroll_bar(k, &self.key, list)),
            &k.elevation.floating,
        );
        push_anchored_modal(k, layer, self.dismiss, content, &anchor_key, self.placement, 2.0);
        trigger
    }

    /// The closed or open field: label, chevron, and the press that toggles the listbox.
    fn field(&mut self, k: &Kit) -> El {
        let c = &k.c;
        let disabled = self.state == State::Disabled;
        let anchor_key = trigger_key(&self.key);
        let red = self.invalid && !disabled && k.invalid_text;
        let (label, ink) = match &self.current {
            Some(v) => (v.clone(), if red { c.danger.fg } else { c.text_base }),
            None => (self.placeholder.clone(), if red { c.danger.fg } else { c.text_faint }),
        };
        let sm = k.select_metrics();
        let chevron: &'static Icon = if self.open { k.glyph.chevron_up } else { k.glyph.chevron_down };
        // Named for what it chooses, its value the chosen option; the text and chevron inside
        // show the same.
        let name = self.name.clone().unwrap_or_else(|| self.placeholder.clone());
        let mut trigger = row().key(anchor_key.clone()).role("combobox").label(name)
            .expanded(self.open).disabled(disabled).center_y()
            .focusable(!disabled);
        if let Some(v) = &self.current {
            trigger = trigger.a11y_value(v.clone());
        }
        trigger = match self.appearance {
            SelectAppearance::Inline => {
                let font = TextStyle { line_height: 16.0, ..k.ty.ui_medium };
                let hover = self.state == State::Hover || self.open;
                // select.css: the inline trigger takes the default ring, 2 out, while closed
                // (opencode#64909).
                let t = if self.open { trigger.no_focus_ring() } else { k.focus_ring_state(trigger, 2.0, 2.0, self.state == State::Focus) };
                let t = t
                    .h(24.0)
                    .pl(8.0)
                    .pr(4.0)
                    .py(4.0)
                    .gap(4.0)
                    .radius(k.radius.sm)
                    .child(text(label).key("value").font(font).color(ink).truncate().a11y_hidden())
                    .child(icon(chevron, sm.chevron).key("chevron").color(c.icon_muted).a11y_hidden());
                k.overlay_state(t, self.state, hover, !disabled)
            }
            SelectAppearance::Field => {
                let t = if self.fill { trigger.w_full().shrink() } else { trigger.w(self.width) };
                let mut t = t
                    .h(28.0)
                    .pr(6.0)
                    .gap(8.0)
                    .radius(k.radius.md)
                    .fill(if self.open || self.state == State::Hover { over(c.hover, c.bg_base) } else { c.bg_base })
                    .child(
                        row()
                            .key("value")
                            .grow(1.0)
                            .shrink()
                            .pl(8.0)
                            .child(text(label).font(k.ty.ui_compact).color(ink).truncate())
                            .a11y_hidden(),
                    )
                    .child(
                        el().size(sm.chevron_box, sm.chevron_box).center().child(icon(chevron, sm.chevron).key("chevron").color(c.icon_muted)).a11y_hidden(),
                    );
                // select.css:34-35, 100-108: a 1 point ring just inside the box, in place of the
                // shadow, while focus is in the closed field (`:focus-within`) or it's invalid.
                if self.invalid && !disabled {
                    t = t.outline(1.0, -1.0, c.danger.fg).no_focus_ring();
                } else if self.state == State::Focus && !self.open {
                    t = t.outline(1.0, -1.0, c.border_focus).no_focus_ring();
                } else {
                    t = k.elevate(t, k.elevation.input());
                    // select.css:73,100-103: focus inside drops the shadow and the hover tint.
                    t = if self.open { t.no_focus_ring() } else { t.focus_within_ring(1.0, -1.0, c.border_focus).focus_within_flat() };
                    if self.state == State::Rest && !self.open {
                        t = k.apply_hover_motion(t.hover(c.hover));
                    }
                }
                t
            }
        };
        if disabled {
            return k.disabled_dim(trigger, true);
        }
        if let Some(f) = self.toggle.take() {
            trigger = f(trigger);
        }
        if self.step_closed && !self.open && let Some(step) = self.step_key.take() {
            let options: Vec<String> = self.options.iter().map(|(_, o)| o.clone()).collect();
            trigger = step(trigger, options, self.current.clone());
        }
        trigger
    }
}
