//! Checkbox (`@opencode/ui` forms/checkbox).

use rune::{Color, El, Icon, Tx, col, el, icon, row, text};

use crate::{Acts, Kit, State};

pub struct Checkbox {
    label: Option<String>,
    checked: bool,
    indeterminate: bool,
    hide_label: bool,
    description: Option<String>,
    glyph: Option<&'static Icon>,
    state: State,
    acts: Acts,
}

impl Checkbox {
    pub fn new(checked: bool) -> Checkbox {
        Checkbox {
            label: None,
            checked,
            indeterminate: false,
            hide_label: false,
            description: None,
            glyph: None,
            state: State::Rest,
            acts: Acts::default(),
        }
    }
    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.label = Some(l.into());
        self
    }
    pub fn hide_label(mut self) -> Self {
        self.hide_label = true;
        self
    }
    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = Some(d.into());
        self
    }
    pub fn indeterminate(mut self, on: bool) -> Self {
        self.indeterminate = on;
        self
    }
    /// Replace the check mark (the web's `icon` prop), drawn at 14 points.
    pub fn icon(mut self, i: &'static Icon) -> Self {
        self.glyph = Some(i);
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
    pub fn on_toggle<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.acts.press(f);
        self
    }
    pub fn on_toggle_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.acts.press_tx(f);
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let c = &k.c;
        let on = self.checked || self.indeterminate;
        let (fill, border) = match self.state {
            State::Disabled => (c.surface_disabled, c.border_disabled),
            State::Hover => (c.surface_hover, c.border_hover),
            State::Focus => (if on { c.surface_weak } else { Color::TRANSPARENT }, c.border_active),
            _ if on => (c.surface_weak, c.std_border),
            _ => (Color::TRANSPARENT, c.border_weak),
        };
        let mark = match self.glyph {
            Some(g) => icon(g, 14.0),
            None => icon(k.glyph.check, 10.0),
        };
        let mut control = el()
            .key("control")
            .size(16.0, 16.0)
            .center()
            .radius(k.radius.sm)
            .fill(fill)
            .border(1.0, border)
            .token_motion(k.motion.control)
            .child(mark.key("mark").color(c.std_icon).opacity(if on { 1.0 } else { 0.0 }));
        if self.state == State::Rest {
            control = control.hover(c.surface_hover);
        }
        let mut root = row().role("checkbox").gap(12.0).center_y().child(control.focus_ring(1.0, 0.0, c.border_active));
        if let Some(l) = &self.label {
            root = root.label(l.clone());
            let mut content = col().key("content");
            if !self.hide_label {
                content = content.child(text(l.clone()).key("label").font(k.ty.text_small.lh(24.0)).color(c.std_text));
            }
            if let Some(d) = &self.description {
                content =
                    content.child(text(d.clone()).key("description").font(k.ty.text_xsmall.lh(15.6)).color(c.std_text));
            }
            root = root.child(content);
        }
        self.acts.apply(root.focusable(self.state != State::Disabled), self.state != State::Disabled)
    }
}
