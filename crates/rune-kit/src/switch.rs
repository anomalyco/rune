//! Switch (`@opencode/ui` forms/switch): compact (default) and standard appearances.

use rune::{El, TextStyle, Tx, el, row, text};

use crate::{Acts, Kit, State, over, shadows};

pub struct Switch {
    label: Option<String>,
    checked: bool,
    standard: bool,
    hide_label: bool,
    description: Option<String>,
    state: State,
    acts: Acts,
}

impl Switch {
    pub fn new(checked: bool) -> Switch {
        Switch {
            label: None,
            checked,
            standard: false,
            hide_label: false,
            description: None,
            state: State::Rest,
            acts: Acts::default(),
        }
    }
    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.label = Some(l.into());
        self
    }
    /// The visible label stays for accessibility only.
    pub fn hide_label(mut self) -> Self {
        self.hide_label = true;
        self
    }
    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = Some(d.into());
        self
    }
    /// The older, larger look (`appearance="standard"`).
    pub fn standard(mut self) -> Self {
        self.standard = true;
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
    /// Called with the switch's press (flip your own state there).
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
        let hover = self.state == State::Hover;
        let disabled = self.state == State::Disabled;
        let control = if self.standard { self.standard_control(k, hover) } else { self.compact_control(k, hover) };
        let control = if disabled && !self.standard { control.dim(0.5) } else { control };
        let control = if self.state == State::Focus && !self.standard {
            control.outline(2.0, 1.0, c.border_focus)
        } else {
            control
        };
        let mut root = row().role("switch").gap(8.0).center_y();
        if let Some(l) = &self.label {
            root = root.label(l.clone());
            if !self.hide_label {
                let (font, color) = if self.standard {
                    (k.ty.text_small.lh(16.0), c.std_text)
                } else {
                    (TextStyle { line_height: 16.0, ..k.ty.small }, c.text_faint)
                };
                root = root.child(text(l.clone()).key("label").font(font).color(color));
            }
        }
        if let Some(d) = &self.description {
            root = root.child(text(d.clone()).key("description").font(k.ty.text_xsmall.lh(15.6)).color(c.std_text));
        }
        root = root.child(control.key("control").focus_ring(2.0, 1.0, c.border_focus));
        self.acts.apply(root.focusable(!disabled), !disabled)
    }

    fn compact_control(&self, k: &Kit, hover: bool) -> El {
        let c = &k.c;
        let (base, sheen_bottom, elevation, knob) = if self.checked {
            (c.bg_accent, k.light_on(0.1, c.bg_accent), &k.elevation.switch_on, c.knob_on)
        } else {
            (c.layer_04, k.light_on(0.2, c.layer_04), &k.elevation.switch_off, c.knob)
        };
        let overlay = if self.checked { k.on(c.contrast_hover, c.bg_accent) } else { c.hover };
        // The thumb grows a point on hover and leans towards the middle when on.
        let (w, radius) = if hover { (13.0, 3.0) } else { (12.0, 2.0) };
        let x = 2.0 + if self.checked { if hover { 7.0 } else { 8.0 } } else { 0.0 };
        let thumb = shadows(
            k,
            el().key("thumb")
                .size(w, 12.0)
                .absolute(x, 2.0)
                .radius(radius)
                .fill(knob)
                .gradient(k.on(c.depth_top, knob), k.on(c.depth_bottom, knob))
                .border(0.5, k.on(c.depth_top, knob)),
            &k.elevation.elements,
        )
        .motion(k.motion.control);
        let fill = if hover { over(overlay, base) } else { base };
        let mut track = el().size(24.0, 16.0).radius(4.0).fill(fill).gradient(k.light(0.0), sheen_bottom);
        if !hover && self.state == State::Rest {
            track = track.hover(overlay);
        }
        shadows(k, track, elevation).motion(k.motion.control).child(thumb)
    }

    fn standard_control(&self, k: &Kit, hover: bool) -> El {
        let c = &k.c;
        let disabled = self.state == State::Disabled;
        let (fill, border) = if disabled {
            (c.surface_disabled, c.border_disabled)
        } else if hover {
            (c.surface_hover, c.border_hover)
        } else if self.checked {
            (c.std_icon_strong, c.std_icon_strong)
        } else {
            (c.surface_base, c.border_weak)
        };
        // A content-box thumb (14 + 1 px border each side) nudged 1 px left over the 1 px track
        // border; when on it loses its border and moves 12 px right.
        let thumb_fill = if disabled { c.std_icon } else { c.std_icon_invert };
        let mut thumb = el().key("thumb").radius(2.0).fill(thumb_fill);
        thumb = if self.checked {
            thumb.size(14.0, 14.0).absolute(13.0, 1.0)
        } else {
            thumb.size(16.0, 16.0).absolute(0.0, 0.0).border(1.0, c.std_border)
        };
        thumb = shadows(k, thumb, &k.elevation.knob).motion(k.motion.control);
        el().size(28.0, 16.0).radius(3.0).fill(fill).border(1.0, border).motion(k.motion.control).child(thumb)
    }
}
