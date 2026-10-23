//! Select (`@opencode/ui` forms/select): an inline trigger and a listbox of menu items with a
//! check on the selected option, optionally grouped.

use rune::{El, Icon, TextStyle, Tx, col, el, icon, row, text};

use crate::menu::{Modal, dismissal};
use crate::overlay::{Placement, menu_anchor};
use crate::{Kit, State, over, press_over, shadows};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SelectAppearance {
    /// The trigger the kit renders today: borderless, 24 points tall.
    #[default]
    Inline,
    /// The 280 point field look (`data-appearance` unset).
    Field,
}

type Choose = Box<dyn Fn(&str) -> Box<dyn FnOnce(El) -> El>>;

/// `Select::new("fruit", &FRUITS).current(s.fruit.as_deref()).open(..).on_choose(|s, v| ..)`.
pub struct Select {
    key: String,
    options: Vec<(Option<String>, String)>,
    details: Vec<Option<String>>,
    width: f32,
    current: Option<String>,
    placeholder: String,
    open: bool,
    invalid: bool,
    state: State,
    appearance: SelectAppearance,
    highlighted: Option<String>,
    placement: Placement,
    choose: Option<Choose>,
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
            current: None,
            placeholder: String::new(),
            open: false,
            invalid: false,
            state: State::Rest,
            appearance: SelectAppearance::Inline,
            highlighted: None,
            placement: Placement::BottomEnd,
            choose: None,
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
    /// The field trigger's width in points (default 280).
    pub fn width(mut self, w: f32) -> Self {
        self.width = w;
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
    pub fn open(mut self, o: bool) -> Self {
        self.open = o;
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
    /// Presses outside and Escape close the listbox. While open it holds keyboard focus.
    pub fn on_close<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.dismiss = Some(dismissal(format!("{}-scrim", self.key), f));
        self
    }
    /// Choosing an option: `f(state, option)`.
    pub fn on_choose<S: 'static>(mut self, f: impl Fn(&mut S, &str, &mut Tx) + Clone + 'static) -> Self {
        self.choose = Some(Box::new(move |v: &str| {
            let (f, v) = (f.clone(), v.to_owned());
            Box::new(move |e: El| e.on_press_tx(move |s: &mut S, tx: &mut Tx| f(s, &v, tx)))
        }));
        self
    }

    /// The field alone, for an app that opens its own menu or picker on press (`on_toggle`):
    /// `open` still turns the chevron and highlights the field, but no listbox is built.
    pub fn trigger(mut self, k: &Kit) -> El {
        self.field(k)
    }

    pub fn build(mut self, k: &Kit, layer: &mut Vec<El>) -> El {
        let trigger = self.field(k);
        if self.state == State::Disabled || !self.open {
            return trigger;
        }
        let c = &k.c;
        let anchor_key = format!("{}-trigger", self.key);
        let modal = self.dismiss.map(|(scrim, modal)| {
            layer.push(scrim);
            modal
        });
        // 12rem tall at most; consecutive options sit 2 points apart, headers flush.
        let mut list = col().key("listbox").role("listbox").focus_group().padding(4.0).max_h(192.0).clip_soft(0.0).items_stretch();
        let mut group: Option<&str> = None;
        let mut after_item = false;
        for (i, (g, o)) in self.options.iter().enumerate() {
            if g.as_deref() != group {
                group = g.as_deref();
                if let Some(g) = group {
                    list = list.child(
                        row()
                            .key(format!("group{i}"))
                            .h(28.0)
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
                .h(28.0)
                .pl(12.0)
                .pr(6.0)
                .gap(8.0)
                .center_y()
                .radius(4.0)
                .mt(if after_item { 2.0 } else { 0.0 })
                .child(
                    text(o.clone()).key("label").font(k.ty.ui_compact).color(c.text_base).grow(1.0).shrink().truncate(),
                )
                .child(el().key("indicator").size(16.0, 16.0).center().child(
                    icon(k.glyph.check_small, 14.0).color(c.icon_base).opacity(if selected { 1.0 } else { 0.0 }),
                ));
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
        let mut content = shadows(
            k,
            col()
                .key(format!("{}-content", self.key))
                .focus_return(&anchor_key)
                .min_w(160.0)
                .max_w(368.0)
                .radius(6.0)
                .fill(c.layer_01)
                .child(list),
            &k.elevation.floating,
        );
        if let Some(modal) = modal {
            content = modal(content);
        }
        layer.push(menu_anchor(k, content, &anchor_key, self.placement, 2.0));
        trigger
    }

    /// The closed or open field: label, chevron, and the press that toggles the listbox.
    fn field(&mut self, k: &Kit) -> El {
        let c = &k.c;
        let disabled = self.state == State::Disabled;
        let anchor_key = format!("{}-trigger", self.key);
        let red = self.invalid && !disabled && k.invalid_text;
        let (label, ink) = match &self.current {
            Some(v) => (v.clone(), if red { c.danger.fg } else { c.text_base }),
            None => (self.placeholder.clone(), if red { c.danger.fg } else { c.text_faint }),
        };
        let chevron: &'static Icon = if self.open { k.glyph.chevron_up } else { k.glyph.chevron_down };
        let mut trigger = row().key(anchor_key.clone()).role("combobox").label(label.clone()).center_y()
            .focusable(!disabled).focus_ring(1.0, 0.0, c.border_focus);
        trigger = match self.appearance {
            SelectAppearance::Inline => {
                let font = TextStyle { line_height: 16.0, ..k.ty.ui_medium };
                let hover = self.state == State::Hover || self.open;
                let mut t = trigger
                    .h(24.0)
                    .pl(8.0)
                    .pr(4.0)
                    .py(4.0)
                    .gap(4.0)
                    .radius(k.radius.sm)
                    .child(text(label).key("value").font(font).color(ink).truncate())
                    .child(icon(chevron, 16.0).key("chevron").color(c.icon_muted));
                if hover {
                    t = t.fill(c.hover);
                } else if self.state == State::Pressed {
                    t = t.fill(c.pressed);
                } else if !disabled {
                    t = t.hover(c.hover).press(press_over(c.hover, c.pressed));
                }
                t
            }
            SelectAppearance::Field => {
                let mut t = trigger
                    .w(self.width)
                    .h(28.0)
                    .pr(6.0)
                    .gap(8.0)
                    .radius(k.radius.md)
                    .fill(if self.open || self.state == State::Hover { over(c.hover, c.bg_base) } else { c.bg_base })
                    .gradient(k.light(0.02), k.light(0.0))
                    .child(
                        row()
                            .key("value")
                            .grow(1.0)
                            .shrink()
                            .pl(8.0)
                            .child(text(label).font(k.ty.ui_compact).color(ink).truncate()),
                    )
                    .child(
                        el().size(20.0, 20.0).center().child(icon(chevron, 16.0).key("chevron").color(c.icon_muted)),
                    );
                if self.invalid && !disabled {
                    t = t.outline(1.0, 0.0, c.danger.fg);
                } else if self.state == State::Focus && !self.open {
                    t = t.outline(1.0, 0.0, c.border_focus);
                } else {
                    t = shadows(k, t, &k.elevation.button_neutral);
                    if self.state == State::Rest && !self.open {
                        t = t.hover(c.hover);
                    }
                }
                t
            }
        };
        if disabled {
            return trigger.dim(0.5);
        }
        if let Some(f) = self.toggle.take() {
            trigger = f(trigger);
        }
        trigger
    }
}
