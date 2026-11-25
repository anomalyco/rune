//! Switch (`@opencode/ui` forms/switch): compact (default, drawn as an NSSwitch), the compact
//! look as the web draws it ([`Switch::web`]), and the standard appearance.

use rune::{El, TextStyle, Tx, el, row, text};

use crate::{Acts, ControlSize, Kit, State};

pub struct Switch {
    label: Option<String>,
    checked: bool,
    standard: bool,
    web: bool,
    flat: bool,
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
            flat: false,
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
    /// Omit the track sheen gradient, knob depth gradient, 0.5 point knob rim, and track and knob
    /// shadows on this compact or web switch without mutating `Kit::elevation`. For a theme-wide
    /// setting, use [`Kit::metrics`](Kit::metrics)'s `switch_look` ([`SwitchLook`](crate::SwitchLook)).
    ///
    /// ```
    /// # use rune_kit::{Kit, Switch};
    /// let k = Kit::neutral(true);
    /// let _ = Switch::new(true).flat().build(&k);
    /// ```
    pub fn flat(mut self) -> Self {
        self.flat = true;
        self
    }
    /// The track and knob of an `NSSwitch` of this size on macOS 27: Mini (36 × 16, the size
    /// System Settings uses) unless set, or the kit's own size from `Kit::metrics.switch`. The
    /// standard and web appearances keep their own size.
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
        let control = k.disabled_dim(control, disabled && !self.standard);
        let control = k.focus_ring_state(control, 2.0, 2.0, self.state == State::Focus);
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
                .child(text(d.clone()).key("description").font(k.ty.text_xsmall.lh(15.6)).color(c.std_text).wrap().shrink().a11y_hidden());
        }
        root = root.child(control.key("control").a11y_hidden());
        self.acts.apply(root.focusable(!disabled).disabled(disabled), !disabled)
    }

    fn compact_control(&self, k: &Kit) -> El {
        // A capsule track and pill knob: NSSwitch's, measured on macOS 27, unless the kit sets its own.
        let m = k.switch_metrics(self.size);
        let (tw, th, kw, kh, inset) = (m.track_width, m.track_height, m.knob_width, m.knob_height, m.inset);
        // The knob stretches towards the middle on hover, and further while pressed.
        let stretch = match self.state {
            State::Hover => (kh * 0.15f32).round(),
            State::Pressed => (kh * 0.35f32).round(),
            _ => 0.0,
        };
        let w = kw + stretch;
        let x = if self.checked { tw - inset - w } else { inset };
        self.raised_switch(k, (tw, th, th / 2.0), (w, kh, x, inset, kh / 2.0))
    }

    /// switch.css:20-61 at rest, :89-99 under the pointer, :106-140 on.
    fn web_control(&self, k: &Kit) -> El {
        let (w, radius) = if self.state == State::Hover { (13.0, 3.0) } else { (12.0, 2.0) };
        let x = 2.0 + if self.checked { 24.0 - 4.0 - w } else { 0.0 };
        self.raised_switch(k, (24.0, 16.0, 4.0), (w, 12.0, x, 2.0, radius))
    }

    fn raised_switch(&self, k: &Kit, track_box: (f32, f32, f32), thumb_box: (f32, f32, f32, f32, f32)) -> El {
        let c = &k.c;
        let look = k.switch_look();
        let (base, sheen_bottom, elevation, knob, border) = if self.checked {
            (c.bg_accent, k.light_on(0.1, c.bg_accent), &k.elevation.switch_on, c.knob_on, look.track_border_on)
        } else {
            (c.layer_04, k.light_on(0.2, c.layer_04), &k.elevation.switch_off, c.knob, look.track_border_off)
        };
        let overlay = if self.checked { k.on(c.contrast_hover, c.bg_accent) } else { c.hover };
        let (tw, th, tr) = track_box;
        let (w, h, x, y, r) = thumb_box;
        let mut thumb = el().key("thumb").size(w, h).absolute(x, y).radius(r).fill(knob);
        if !self.flat && look.knob_depth {
            thumb = thumb.gradient(k.on(c.depth_top, knob), k.on(c.depth_bottom, knob)).border(0.5, k.on(c.depth_top, knob));
        }
        let thumb = if self.flat { thumb } else { k.elevate(thumb, &k.elevation.elements) }.motion(k.motion.control);
        let mut track = el().size(tw, th).radius(tr);
        if !self.flat && look.sheen {
            track = track.gradient(k.light(0.0), sheen_bottom);
        }
        if look.track_border > 0.0 && border.a > 0.0 {
            track = track.border(look.track_border, border);
        }
        let track = k.hover_surface(track, base, overlay, self.state == State::Hover, self.state == State::Rest);
        let track = if self.flat { track } else { k.elevate(track, elevation) };
        track.motion(k.motion.control).child(thumb)
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
        let m = k.standard_switch_metrics();
        let (tw, th, thumb_size, inset) = (m.track_width, m.track_height, m.thumb, m.inset);
        let thumb_fill = if disabled { c.std_icon } else { c.std_icon_invert };
        let mut thumb = el().key("thumb").fill(thumb_fill);
        thumb = if self.checked {
            let max_stretch = (tw - 2.0 * inset - thumb_size).max(0.0);
            let stretch = if self.state == State::Pressed { 4.0f32.min(max_stretch) } else { 0.0 };
            let x = (tw - inset - thumb_size - stretch).max(0.0);
            let y = ((th - thumb_size) * 0.5).max(0.0);
            thumb.size(thumb_size + stretch, thumb_size).absolute(x, y).radius(thumb_size * 0.5)
        } else {
            let off_size = thumb_size + 2.0;
            let x = (inset - 1.0).max(0.0);
            let y = ((th - off_size) * 0.5).max(0.0);
            let max_stretch = (tw - x - off_size).max(0.0);
            let stretch = if self.state == State::Pressed { 4.0f32.min(max_stretch) } else { 0.0 };
            thumb
                .size(off_size + stretch, off_size)
                .absolute(x, y)
                .radius(off_size * 0.5)
                .border(1.0, c.std_border)
        };
        thumb = k.elevate(thumb, &k.elevation.knob).motion(k.motion.control);
        el().size(tw, th).radius(th * 0.5).fill(fill).border(1.0, border).motion(k.motion.control).child(thumb)
    }
}
