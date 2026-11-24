//! Accessible range slider control (`role="slider"`) with pointer drag, track jump,
//! Escape/cancel restore, keyboard stepping, and assistive technology actions.

use std::cell::Cell;
use std::ops::RangeInclusive;
use std::rc::Rc;

use rune::{Direction, Drag, El, Phase, TextStyle, Tx, col, el, text};

use crate::{ControlSize, Kit, State};

/// Normalized range, step and geometry values shared by the slider's input handlers.
#[derive(Clone, Copy, Debug)]
struct Binding {
    value: f32,
    min: f32,
    max: f32,
    explicit_step: Option<f64>,
    kbd_step: f64,
    knob: f32,
}

impl Binding {
    fn new(value: f32, range: RangeInclusive<f32>, step: f32, knob: f32) -> Binding {
        let (raw_min, raw_max) = (*range.start(), *range.end());
        let (min, max) = if !raw_min.is_finite() || !raw_max.is_finite() {
            (0.0, 1.0)
        } else if raw_min <= raw_max {
            (raw_min, raw_max)
        } else {
            (raw_min, raw_min)
        };
        let span = f64::from(max) - f64::from(min);
        let current = if value.is_nan() { min } else { value.clamp(min, max) };
        let explicit_step = (step > 0.0 && step.is_finite()).then(|| f64::from(step));
        let kbd_step = explicit_step.unwrap_or_else(|| if span > 0.0 { span / 20.0 } else { 0.0 });
        Binding { value: current, min, max, explicit_step, kbd_step, knob }
    }

    fn min64(self) -> f64 {
        f64::from(self.min)
    }

    fn max64(self) -> f64 {
        f64::from(self.max)
    }

    fn span64(self) -> f64 {
        self.max64() - self.min64()
    }

    fn frac(self, v: f32) -> f32 {
        let span = self.span64();
        if span > 0.0 && span.is_finite() && !v.is_nan() {
            ((f64::from(v) - self.min64()) / span).clamp(0.0, 1.0) as f32
        } else {
            0.0
        }
    }

    fn snap(self, target: f64) -> f32 {
        let (min64, max64) = (self.min64(), self.max64());
        let clamped = target.clamp(min64, max64);
        let snapped = match self.explicit_step {
            Some(step) if step > 0.0 => {
                let n = ((clamped - min64) / step).round();
                (min64 + n * step).clamp(min64, max64)
            }
            _ => clamped,
        };
        (snapped as f32).clamp(self.min, self.max)
    }

    fn from_frac(self, frac: f32) -> f32 {
        let f = f64::from(frac.clamp(0.0, 1.0));
        self.snap(self.min64() + f * self.span64())
    }

    fn step_by(self, current: f32, delta: i32) -> f32 {
        if self.span64() <= 0.0 || self.kbd_step <= 0.0 {
            return self.min;
        }
        self.snap(f64::from(current) + f64::from(delta) * self.kbd_step)
    }

    fn from_a11y(self, current: f32, target: f64) -> f32 {
        if !target.is_finite() {
            return current;
        }
        self.snap(target)
    }
}

fn handle_drag(
    b: Binding,
    live: &Cell<f32>,
    d: &mut Drag,
    tx: &mut Tx,
    mut emit: impl FnMut(f32, bool, &mut Tx),
) {
    let rtl = d.direction() == Direction::Rtl;
    let logical = |x: f32| if rtl { d.w - x } else { x };
    let w = d.w.max(b.knob);
    let travel = (w - b.knob).max(0.0);
    let to_value = |lx: f32| {
        let frac = if travel > 0.0 { ((lx - b.knob * 0.5) / travel).clamp(0.0, 1.0) } else { 0.0 };
        b.from_frac(frac)
    };
    let grab_offset = |pre_value: f32| {
        let knob_center = b.knob * 0.5 + b.frac(pre_value) * travel;
        let start_lx = logical(d.start_x);
        if (start_lx - knob_center).abs() <= b.knob * 0.5 + 2.0 {
            start_lx - knob_center
        } else {
            0.0
        }
    };
    match d.phase {
        Phase::Down => {
            let pre = live.get();
            d.data = pre;
            let knob_center = b.knob * 0.5 + b.frac(pre) * travel;
            let lx = logical(d.x);
            if (lx - knob_center).abs() > b.knob * 0.5 + 2.0 {
                // Clicked the track: jump to the press point and let the knob spring there.
                let next = to_value(lx);
                live.set(next);
                emit(next, false, tx);
            }
        }
        Phase::Move => {
            let grab = grab_offset(d.data);
            let next = to_value(logical(d.x) - grab);
            live.set(next);
            tx.instant();
            emit(next, false, tx);
        }
        Phase::Up => {
            if d.cancelled() {
                live.set(d.data);
                emit(d.data, true, tx);
            } else {
                let grab = grab_offset(d.data);
                let next = to_value(logical(d.x) - grab);
                live.set(next);
                emit(next, true, tx);
            }
        }
    }
}

type BindFn = Box<dyn FnOnce(El, Binding) -> El>;

/// An accessible range slider control following the WAI-ARIA Slider pattern.
///
/// Supports track click jump, smooth knob drag with grab offset, Escape or cancelled-drag
/// value restore, keyboard stepping (Arrows by one step, PageUp/PageDown by ten steps,
/// Home/End to bounds, mirrored in `Direction::Rtl`), AccessKit `Increment`/`Decrement`/`SetValue`,
/// and disabled state.
///
/// ```
/// # use rune_kit::{Kit, Slider};
/// # struct App { size: f32 }
/// # let s = App { size: 240.0 };
/// # let k = &Kit::neutral(true);
/// let el = Slider::new("thumb-size", s.size)
///     .range(160.0..=420.0)
///     .step(20.0)
///     .label("Thumbnail size")
///     .value_text(format!("{:.0} px", s.size))
///     .on_change(|app: &mut App, value, done| {
///         app.size = value;
///         let _ = done;
///     })
///     .build(k);
/// # let _ = el;
/// ```
pub struct Slider {
    key: String,
    value: f32,
    range: RangeInclusive<f32>,
    step: f32,
    label: Option<String>,
    hide_label: bool,
    value_text: Option<String>,
    size: ControlSize,
    width: f32,
    state: State,
    bind: Option<BindFn>,
}

impl Slider {
    /// Creates a slider with element key `key` and current numeric `value`.
    ///
    /// Defaults to range `0.0..=1.0`, continuous drag (`step = 0.0`, 1/20th keyboard step),
    /// `ControlSize::Regular`, and width `180.0` points.
    pub fn new(key: impl Into<String>, value: f32) -> Slider {
        Slider {
            key: key.into(),
            value,
            range: 0.0..=1.0,
            step: 0.0,
            label: None,
            hide_label: false,
            value_text: None,
            size: ControlSize::Regular,
            width: 180.0,
            state: State::Rest,
            bind: None,
        }
    }

    /// Sets the inclusive minimum and maximum values (default `0.0..=1.0`).
    pub fn range(mut self, range: RangeInclusive<f32>) -> Self {
        self.range = range;
        self
    }

    /// Sets the step increment. Values `<= 0.0` (the default) drag continuously and step by
    /// 1/20th of the range on keyboard and assistive increment/decrement.
    pub fn step(mut self, step: f32) -> Self {
        self.step = step;
        self
    }

    /// Sets the slider's visible and accessible label.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Keeps [`Self::label`] for assistive technology without drawing visible label text.
    pub fn hide_label(mut self) -> Self {
        self.hide_label = true;
        self
    }

    /// Sets the human-readable value text exposed to assistive technology (`aria-valuetext`),
    /// such as `"50%"` or `"1.00×"`.
    pub fn value_text(mut self, text: impl Into<String>) -> Self {
        self.value_text = Some(text.into());
        self
    }

    /// Sets the slider's track and knob scale (default [`ControlSize::Regular`]).
    pub fn control_size(mut self, size: ControlSize) -> Self {
        self.size = size;
        self
    }

    /// Sets the slider track width in points (default `180.0`).
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// Forces a visual state for galleries and stories (`State::Rest` uses live interaction).
    pub fn state(mut self, state: State) -> Self {
        self.state = state;
        self
    }

    /// Disables the slider, dimming it and stripping all pointer, keyboard, and assistive
    /// handlers.
    pub fn disabled(mut self, disabled: bool) -> Self {
        if disabled {
            self.state = State::Disabled;
        }
        self
    }

    /// Called when the slider's value changes: `done` is `false` during live pointer drags
    /// and `true` on pointer release, cancelled-drag restore, keyboard steps, and assistive
    /// `SetValue`/`Increment`/`Decrement` actions.
    pub fn on_change<S: 'static>(self, f: impl Fn(&mut S, f32, bool) + 'static) -> Self {
        self.on_change_tx(move |s: &mut S, value, done, _| f(s, value, done))
    }

    /// [`Self::on_change`] with access to the commit transaction [`Tx`].
    pub fn on_change_tx<S: 'static>(mut self, f: impl Fn(&mut S, f32, bool, &mut Tx) + 'static) -> Self {
        let f = Rc::new(f);
        self.bind = Some(Box::new(move |el: El, b: Binding| {
            let live = Rc::new(Cell::new(b.value));
            let (f_drag, live_drag) = (Rc::clone(&f), Rc::clone(&live));
            let el = el.on_drag(move |s: &mut S, d: &mut Drag, tx: &mut Tx| {
                handle_drag(b, &live_drag, d, tx, |v, done, tx| f_drag(s, v, done, tx));
            });
            let (f_step, live_step) = (Rc::clone(&f), Rc::clone(&live));
            let el = el.on_step(move |s: &mut S, delta: i32, tx: &mut Tx| {
                let next = b.step_by(live_step.get(), delta);
                live_step.set(next);
                f_step(s, next, true, tx);
            });
            let (f_limit, live_limit) = (Rc::clone(&f), Rc::clone(&live));
            let el = el.on_step_limit(move |s: &mut S, to_max: bool, tx: &mut Tx| {
                let next = if to_max { b.max } else { b.min };
                live_limit.set(next);
                f_limit(s, next, true, tx);
            });
            let (f_set, live_set) = (Rc::clone(&f), Rc::clone(&live));
            el.on_set_value(move |s: &mut S, target: f64, tx: &mut Tx| {
                let next = b.from_a11y(live_set.get(), target);
                live_set.set(next);
                f_set(s, next, true, tx);
            })
        }));
        self
    }

    /// Builds the slider element using semantic tokens from `k`.
    pub fn build(self, k: &Kit) -> El {
        let c = &k.c;
        let disabled = self.state == State::Disabled;
        let (hit_h, knob_d, bar_h) = match self.size {
            ControlSize::Mini => (16.0, 12.0, 3.0),
            ControlSize::Small => (20.0, 14.0, 4.0),
            ControlSize::Regular => (24.0, 18.0, 4.0),
            ControlSize::Large => (28.0, 20.0, 5.0),
            ControlSize::ExtraLarge => (36.0, 24.0, 6.0),
        };
        let w = if self.width.is_finite() { self.width.max(knob_d) } else { 180.0 };
        let travel = (w - knob_d).max(0.0);
        let b = Binding::new(self.value, self.range, self.step, knob_d);
        let frac = b.frac(b.value);
        let fill_w = (knob_d * 0.5 + frac * travel).min(w);

        let track = el()
            .key("track")
            .size(w, bar_h)
            .absolute(0.0, (hit_h - bar_h) * 0.5)
            .radius(bar_h * 0.5)
            .fill(c.layer_03)
            .a11y_hidden();
        let fill = el()
            .key("fill")
            .size(fill_w, bar_h)
            .absolute(0.0, (hit_h - bar_h) * 0.5)
            .radius(bar_h * 0.5)
            .fill(c.bg_accent)
            .a11y_hidden();
        let knob = el()
            .key("knob")
            .size(knob_d, knob_d)
            .absolute(frac * travel, (hit_h - knob_d) * 0.5)
            .radius(knob_d * 0.5)
            .border(1.0, c.border_base);
        let knob = k.surface_state(knob, self.state, c.knob, c.hover, c.pressed, !disabled);
        let knob = k.elevate(knob, &k.elevation.knob);
        let knob = k.focus_ring_state(knob, 2.0, 2.0, self.state == State::Focus).a11y_hidden();

        let mut control = el()
            .key(self.key.clone())
            .role("slider")
            .range_value(f64::from(b.value), b.min64(), b.max64())
            .size(w, hit_h)
            .motion(k.motion.control)
            .child(track)
            .child(fill)
            .child(knob);
        if b.kbd_step > 0.0 {
            control = control.range_step(b.kbd_step);
        }
        if let Some(l) = &self.label {
            control = control.label(l.clone());
        }
        if let Some(vt) = self.value_text {
            control = control.a11y_value(vt);
        }
        control = k.disabled_dim(control, disabled);
        if disabled {
            control = control.focusable(false).disabled(true);
        } else {
            control = control.focusable(true).disabled(false);
            if let Some(bind) = self.bind {
                control = bind(control, b);
            }
        }

        match (self.label, self.hide_label) {
            (Some(l), false) => col()
                .key(format!("{}-field", self.key))
                .gap(4.0)
                .items_start()
                .child(
                    text(l)
                        .key("label")
                        .font(TextStyle { line_height: 16.0, ..k.ty.small })
                        .color(if disabled { c.text_muted } else { c.text_base })
                        .a11y_hidden(),
                )
                .child(control),
            _ => control,
        }
    }
}

#[cfg(test)]
#[path = "slider_tests.rs"]
mod tests;
