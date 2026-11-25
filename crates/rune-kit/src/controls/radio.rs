//! RadioGroup and RadioItem (`@opencode/ui` forms/radio).

use rune::{El, Shadow, TextStyle, Tx, col, el, row, text};

use crate::{Acts, Kit, State};

/// What a radio group's controls sit on (`@opencode/ui` radio `surface`): the fill of a control
/// that is neither selected nor invalid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RadioSurface {
    /// `c.bg_base`, for a group on the window's or a dialog's own background.
    #[default]
    Base,
    /// `c.layer_01`, for a group on a raised layer (`surface="layer-1"`).
    Layer1,
}

pub struct RadioItem {
    label: String,
    description: Option<String>,
    checked: bool,
    state: State,
    invalid: bool,
    surface: RadioSurface,
    acts: Acts,
}

impl RadioItem {
    pub fn new(label: impl Into<String>, checked: bool) -> RadioItem {
        RadioItem {
            label: label.into(),
            description: None,
            checked,
            state: State::Rest,
            invalid: false,
            surface: RadioSurface::Base,
            acts: Acts::default(),
        }
    }
    /// What the control sits on ([`RadioGroup::surface`] sets it for a group's items).
    pub fn surface(mut self, s: RadioSurface) -> Self {
        self.surface = s;
        self
    }
    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = Some(d.into());
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
    pub fn on_select<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.acts.press(f);
        self
    }
    pub fn on_select_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.acts.press_tx(f);
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let c = &k.c;
        let disabled = self.state == State::Disabled;
        let hover = self.state == State::Hover;
        // radio.css: only a selected control has the top light; one that is not is flat, on its
        // surface (or the danger background when invalid, with a ring in the danger foreground).
        let (base, sheen) = if self.checked {
            (c.bg_accent, k.light_on(0.2, c.bg_accent))
        } else if self.invalid {
            (c.danger.bg, k.light(0.0))
        } else {
            let surface = match self.surface {
                RadioSurface::Base => c.bg_base,
                RadioSurface::Layer1 => c.layer_01,
            };
            (surface, k.light(0.0))
        };
        let overlay = if self.checked { k.on(c.contrast_hover, c.bg_accent) } else { c.hover };
        let ring = if self.invalid && !self.checked { c.danger.fg } else { c.border_strong };
        let mut control = k.hover_surface(
            el().key("control")
                .size(16.0, 16.0)
                .absolute(0.0, 2.0)
                .radius(8.0)
                .gradient(sheen, k.light(0.0))
                .box_shadow(Shadow { spread: 0.5, color: ring, inset: true, ..Shadow::default() })
                .token_motion(k.motion.control),
            base,
            overlay,
            hover,
            self.state == State::Rest && !self.checked,
        );
        control = k.focus_ring_state(control, 2.0, 2.0, self.state == State::Focus);
        if self.checked {
            // The dot: the depth gradient over its grey, a ring in the contrast icon colour and
            // the small elements' drop (radio.css, `radio-v2-item-indicator`).
            let dot = c.knob_on;
            let indicator = el()
                .key("dot")
                .size(6.0, 6.0)
                .absolute(5.0, 5.0)
                .radius(3.0)
                .fill(dot)
                .gradient(k.on(c.depth_top, dot), k.on(c.depth_bottom, dot))
                .box_shadow(Shadow { spread: 0.5, color: c.icon_contrast, inset: true, ..Shadow::default() });
            control = control.child(k.elevate(indicator, &k.elevation.elements));
        }
        control = k.disabled_dim(control, disabled);
        let label_color = if disabled { c.text_muted } else { c.text_base };
        let mut words = col().key("text").shrink().gap(6.0).child(
            text(self.label.clone()).key("label").font(TextStyle { line_height: 20.0, ..k.ty.ui }).color(label_color),
        );
        if let Some(d) = &self.description {
            let color = if disabled { c.text_faint } else { c.text_muted };
            words = words.child(text(d.clone()).key("description").font(k.ty.small.lh(11.0)).color(color).wrap());
        }
        // Checked follows `focus_selected`; the label and description say what the words do.
        let root = row()
            .role("radio")
            .label(self.label)
            .description(self.description.unwrap_or_default())
            .gap(8.0)
            .items_start()
            .child(el().key("stack").size(16.0, 20.0).child(control).a11y_hidden())
            .child(words.a11y_hidden());
        self.acts.apply(root.focusable(!disabled).focus_selected(self.checked).disabled(disabled), !disabled)
    }
}

pub struct RadioGroup {
    label: Option<String>,
    description: Option<String>,
    items: Vec<RadioItem>,
    disabled: bool,
    invalid: bool,
    surface: RadioSurface,
}

impl RadioGroup {
    pub fn new() -> RadioGroup {
        RadioGroup { label: None, description: None, items: Vec::new(), disabled: false, invalid: false, surface: RadioSurface::Base }
    }
    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.label = Some(l.into());
        self
    }
    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = Some(d.into());
        self
    }
    pub fn item(mut self, i: RadioItem) -> Self {
        self.items.push(i);
        self
    }
    pub fn disabled(mut self, d: bool) -> Self {
        self.disabled = d;
        self
    }
    pub fn invalid(mut self, i: bool) -> Self {
        self.invalid = i;
        self
    }
    /// What the group's controls sit on: [`RadioSurface::Base`] unless set.
    pub fn surface(mut self, s: RadioSurface) -> Self {
        self.surface = s;
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let c = &k.c;
        let mut root = col().role("radiogroup").focus_group().gap(8.0);
        // The group's name and description say its label and the line under it.
        if let Some(l) = &self.label {
            root = root
                .label(l.clone())
                .child(text(l.clone()).key("label").font(k.ty.small.lh(11.0)).color(c.text_faint).a11y_hidden());
        }
        if let Some(d) = &self.description {
            root = root
                .description(d.clone())
                .child(text(d.clone()).key("description").font(k.ty.small.lh(13.2)).color(c.text_faint).wrap().a11y_hidden());
        }
        let mut items = col().key("items").gap(12.0);
        let n = self.items.len();
        for (i, mut item) in self.items.into_iter().enumerate() {
            if self.disabled {
                item = item.disabled(true);
            }
            item.invalid = self.invalid;
            item.surface = self.surface;
            items = items.child(item.build(k).key(format!("item{i}")).position_in_set(i + 1, n));
        }
        root.child(items).disabled(self.disabled)
    }
}

impl Default for RadioGroup {
    fn default() -> Self {
        Self::new()
    }
}
