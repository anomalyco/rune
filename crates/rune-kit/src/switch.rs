//! Switch (`@opencode/ui` forms/switch): compact (default, drawn as an NSSwitch), the compact
//! look as the web draws it ([`Switch::web`]), and the standard appearance.

use rune::{El, TextStyle, Tx, el, row, text};

use crate::{Acts, ControlSize, Kit, State, over, shadows};

pub struct Switch {
    label: Option<String>,
    checked: bool,
    standard: bool,
    web: bool,
    size: ControlSize,
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
            web: false,
            size: ControlSize::Mini,
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
    /// The compact appearance as `@opencode/ui` draws it on the web (switch.css:20-152): a 24 x 16
    /// track with 4 point corners and a 12 point thumb with 2 point corners, 2 points in, that
    /// moves 8 points when on. Under the pointer the thumb grows a point towards the middle.
    pub fn web(mut self) -> Self {
        self.web = true;
        self
    }
    /// The track and knob of an `NSSwitch` of this size on macOS 27: Mini (36 × 16, the size
    /// System Settings uses) unless set. The standard appearance keeps its own size.
    pub fn control_size(mut self, s: ControlSize) -> Self {
        self.size = s;
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
        let control = match (self.standard, self.web) {
            (true, _) => self.standard_control(k, hover),
            (false, true) => self.web_control(k),
            (false, false) => self.compact_control(k),
        };
        let control = if disabled && !self.standard { control.layer_opacity(0.5) } else { control };
        let control = if self.state == State::Focus && !self.standard {
            control.outline(2.0, 1.0, c.border_focus)
        } else {
            control
        };
        // The switch's label, description and on state say what its text and track show.
        let mut root = row().role("switch").checked(self.checked).gap(8.0).center_y();
        if let Some(l) = &self.label {
            root = root.label(l.clone());
            if !self.hide_label {
                let (font, color) = if self.standard {
                    (k.ty.text_small.lh(16.0), c.std_text)
                } else {
                    (TextStyle { line_height: 16.0, ..k.ty.small }, c.text_faint)
                };
                root = root.child(text(l.clone()).key("label").font(font).color(color).a11y_hidden());
            }
        }
        if let Some(d) = &self.description {
            root = root
                .description(d.clone())
                .child(text(d.clone()).key("description").font(k.ty.text_xsmall.lh(15.6)).color(c.std_text).a11y_hidden());
        }
        root = root.child(control.key("control").focus_ring(2.0, 1.0, c.border_focus).a11y_hidden());
        self.acts.apply(root.focusable(!disabled).disabled(disabled), !disabled)
    }

    fn compact_control(&self, k: &Kit) -> El {
        let c = &k.c;
        let hover = self.state == State::Hover;
        let (base, sheen_bottom, elevation, knob) = if self.checked {
            (c.bg_accent, k.light_on(0.1, c.bg_accent), &k.elevation.switch_on, c.knob_on)
        } else {
            (c.layer_04, k.light_on(0.2, c.layer_04), &k.elevation.switch_off, c.knob)
        };
        let overlay = if self.checked { k.on(c.contrast_hover, c.bg_accent) } else { c.hover };
        // NSSwitch's capsule track and pill knob, measured on macOS 27: (track w, h, knob w, h, inset).
        let (tw, th, kw, kh, inset) = match self.size {
            ControlSize::Mini => (36.0, 16.0, 21.0, 14.0, 1.0),
            ControlSize::Small => (44.0, 20.0, 26.0, 16.0, 2.0),
            ControlSize::Regular => (54.0, 24.0, 32.0, 20.0, 2.0),
            ControlSize::Large => (64.0, 28.0, 38.0, 24.0, 2.0),
            ControlSize::ExtraLarge => (80.0, 36.0, 48.0, 30.0, 3.0),
        };
        // The knob stretches towards the middle on hover, and further while pressed.
        let stretch = match self.state {
            State::Hover => (kh * 0.15f32).round(),
            State::Pressed => (kh * 0.35f32).round(),
            _ => 0.0,
        };
        let w = kw + stretch;
        let x = if self.checked { tw - inset - w } else { inset };
        let thumb = shadows(
            k,
            el().key("thumb")
                .size(w, kh)
                .absolute(x, inset)
                .radius(kh / 2.0)
                .fill(knob)
                .gradient(k.on(c.depth_top, knob), k.on(c.depth_bottom, knob))
                .border(0.5, k.on(c.depth_top, knob)),
            &k.elevation.elements,
        )
        .motion(k.motion.control);
        let fill = if hover { over(overlay, base) } else { base };
        let mut track = el().size(tw, th).radius(th / 2.0).fill(fill).gradient(k.light(0.0), sheen_bottom);
        if !hover && self.state == State::Rest {
            track = track.hover(overlay);
        }
        shadows(k, track, elevation).motion(k.motion.control).child(thumb)
    }

    /// switch.css:20-61 at rest, :89-99 under the pointer, :106-140 on.
    fn web_control(&self, k: &Kit) -> El {
        let c = &k.c;
        let hover = self.state == State::Hover;
        let (base, sheen_bottom, elevation, knob) = if self.checked {
            (c.bg_accent, k.light_on(0.1, c.bg_accent), &k.elevation.switch_on, c.knob_on)
        } else {
            (c.layer_04, k.light_on(0.2, c.layer_04), &k.elevation.switch_off, c.knob)
        };
        let overlay = if self.checked { k.on(c.contrast_hover, c.bg_accent) } else { c.hover };
        let (w, radius) = if hover { (13.0, 3.0) } else { (12.0, 2.0) };
        let x = 2.0 + if self.checked { 24.0 - 4.0 - w } else { 0.0 };
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
        if self.state == State::Rest {
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
        // border; when on it loses its border and moves 12 px right. Pressed, it stretches
        // towards the middle.
        let thumb_fill = if disabled { c.std_icon } else { c.std_icon_invert };
        let stretch = if self.state == State::Pressed { 4.0 } else { 0.0 };
        let mut thumb = el().key("thumb").fill(thumb_fill);
        thumb = if self.checked {
            thumb.size(14.0 + stretch, 14.0).absolute(13.0 - stretch, 1.0).radius(7.0)
        } else {
            thumb.size(16.0 + stretch, 16.0).absolute(0.0, 0.0).radius(8.0).border(1.0, c.std_border)
        };
        thumb = shadows(k, thumb, &k.elevation.knob).motion(k.motion.control);
        el().size(28.0, 16.0).radius(8.0).fill(fill).border(1.0, border).motion(k.motion.control).child(thumb)
    }
}
