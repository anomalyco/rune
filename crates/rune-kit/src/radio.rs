//! RadioGroup and RadioItem (`@opencode/ui` forms/radio).

use rune::{El, Shadow, TextStyle, Tx, col, el, row, text};

use crate::{Acts, Kit, State, over};

pub struct RadioItem {
    label: String,
    description: Option<String>,
    checked: bool,
    state: State,
    invalid: bool,
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
            acts: Acts::default(),
        }
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
        let (base, sheen) = if self.checked {
            (c.bg_accent, k.light_on(0.2, c.bg_accent))
        } else if self.invalid {
            (c.danger.bg, k.light(0.0))
        } else {
            (c.bg_base, k.light(0.06))
        };
        let overlay = if self.checked { k.on(c.contrast_hover, c.bg_accent) } else { c.hover };
        let ring = if self.invalid && !self.checked { c.danger.border } else { c.border_strong };
        let mut control = el()
            .key("control")
            .size(16.0, 16.0)
            .absolute(0.0, 2.0)
            .radius(8.0)
            .fill(if hover { over(overlay, base) } else { base })
            .gradient(sheen, k.light(0.0))
            .box_shadow(Shadow { spread: 0.5, color: ring, inset: true, ..Shadow::default() })
            .motion(k.motion.control);
        control = control.focus_ring(2.0, 1.0, c.border_focus);
        if self.checked {
            let dot = c.knob_on;
            control = control.child(
                el().key("dot")
                    .size(6.0, 6.0)
                    .absolute(5.0, 5.0)
                    .radius(3.0)
                    .fill(dot)
                    .box_shadow(Shadow { spread: 0.5, color: k.on(c.depth_top, dot), inset: true, ..Shadow::default() })
                    .box_shadow(Shadow { y: 0.5, blur: 0.5, color: k.dark_a(0.4), ..Shadow::default() }),
            );
        }
        if self.state == State::Rest && !self.checked {
            control = control.hover(c.hover);
        }
        if disabled {
            control = control.dim(0.5);
        }
        if self.state == State::Focus {
            control = control.outline(2.0, 1.0, c.border_focus);
        }
        let label_color = if disabled { c.text_muted } else { c.text_base };
        let mut words = col().key("text").gap(6.0).child(
            text(self.label.clone()).key("label").font(TextStyle { line_height: 20.0, ..k.ty.ui }).color(label_color),
        );
        if let Some(d) = &self.description {
            let color = if disabled { c.text_faint } else { c.text_muted };
            words = words.child(text(d.clone()).key("description").font(k.ty.small.lh(11.0)).color(color));
        }
        let root = row()
            .role("radio")
            .label(self.label)
            .gap(8.0)
            .items_start()
            .child(el().key("stack").size(16.0, 20.0).child(control))
            .child(words);
        self.acts.apply(root.focusable(!disabled).focus_selected(self.checked), !disabled)
    }
}

pub struct RadioGroup {
    label: Option<String>,
    description: Option<String>,
    items: Vec<RadioItem>,
    disabled: bool,
    invalid: bool,
}

impl RadioGroup {
    pub fn new() -> RadioGroup {
        RadioGroup { label: None, description: None, items: Vec::new(), disabled: false, invalid: false }
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

    pub fn build(self, k: &Kit) -> El {
        let c = &k.c;
        let mut root = col().role("radiogroup").focus_group().gap(8.0);
        if let Some(l) = &self.label {
            root =
                root.label(l.clone()).child(text(l.clone()).key("label").font(k.ty.small.lh(11.0)).color(c.text_faint));
        }
        if let Some(d) = &self.description {
            root = root.child(text(d.clone()).key("description").font(k.ty.small.lh(13.2)).color(c.text_faint));
        }
        let mut items = col().key("items").gap(12.0);
        for (i, mut item) in self.items.into_iter().enumerate() {
            if self.disabled {
                item = item.disabled(true);
            }
            item.invalid = self.invalid;
            items = items.child(item.build(k).key(format!("item{i}")));
        }
        root.child(items)
    }
}

impl Default for RadioGroup {
    fn default() -> Self {
        Self::new()
    }
}
