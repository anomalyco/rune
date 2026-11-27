//! Editable numeric input (`role="spinbutton"`) with integer and decimal entry, optional
//! bounds, step and page-step increments, empty optional values distinct from zero, editable
//! intermediate text (such as `-` or `1.`), optional prefix/suffix labels, and optional
//! increment/decrement buttons.

mod bind;
mod spec;

use std::ops::RangeInclusive;
use std::rc::Rc;

use rune::{El, Icon, KeyEvent, TextEdit, TextStyle, Tx, el, icon, row, text, text_input};

use self::bind::{Bind, Erased, FocusFn, KeyFn, LensFn, ValueFn, erase_lens, state, typed};
use self::spec::{Spec, format_number, normalize_zero};
use crate::controls::text_input::{kit_rich_style, name_field, shell};
use crate::{Kit, State, glyphs};

/// State for an editable [`NumberField`]: owns the [`TextEdit`] buffer alongside the last
/// accepted typed numeric value (`Option<f64>`, where `None` is an empty optional field,
/// distinct from `Some(0.0)`).
#[derive(Clone, Debug, Default)]
pub struct NumberFieldState {
    edit: TextEdit,
    value: Option<f64>,
    invalid: bool,
    dirty: bool,
}

impl NumberFieldState {
    /// Creates state initialized to `value`. Non-finite values are treated as empty (`None`),
    /// and `-0.0` is normalized to `0.0`.
    pub fn new(value: Option<f64>) -> NumberFieldState {
        let value = value.filter(|v| v.is_finite()).map(normalize_zero);
        let text = value.map(|v| format_number(v, None, false)).unwrap_or_default();
        NumberFieldState { edit: TextEdit::new(&text), value, invalid: false, dirty: false }
    }

    /// Creates an empty state (`value() == None`, distinct from `Some(0.0)`).
    pub fn empty() -> NumberFieldState {
        NumberFieldState::default()
    }

    /// The last accepted typed numeric value (`None` when empty, distinct from `Some(0.0)`).
    pub fn value(&self) -> Option<f64> {
        self.value
    }

    /// Convenience accessor for integer fields: rounds [`Self::value`] to `i64`.
    pub fn as_i64(&self) -> Option<i64> {
        self.value.map(|v| v.round() as i64)
    }

    /// The live text inside the editor, including editable intermediate text such as `-` or `1.`.
    pub fn text(&self) -> &str {
        &self.edit.text
    }

    /// The underlying [`TextEdit`] buffer.
    pub fn edit(&self) -> &TextEdit {
        &self.edit
    }

    /// Mutable access to the underlying [`TextEdit`] buffer (for focus, selection, or caret control).
    pub fn edit_mut(&mut self) -> &mut TextEdit {
        &mut self.edit
    }

    /// Whether the accepted value is empty (`None`), distinct from `Some(0.0)`.
    pub fn is_empty(&self) -> bool {
        self.value.is_none()
    }

    /// Whether the inner editor currently has keyboard focus.
    pub fn is_focused(&self) -> bool {
        self.edit.focused
    }

    /// Whether the current draft text is unparseable, exceeds configured decimal places, or lies
    /// outside configured bounds before commit.
    pub fn is_invalid(&self) -> bool {
        self.invalid
    }

    /// Programmatically sets the value and updates the editor text to match, clearing any draft
    /// validation error.
    pub fn set_value(&mut self, value: Option<f64>) {
        let value = value.filter(|v| v.is_finite()).map(normalize_zero);
        let text = value.map(|v| format_number(v, None, false)).unwrap_or_default();
        self.set_value_formatted(value, text);
    }

    /// Programmatically sets the value alongside explicit formatted display text.
    pub fn set_value_formatted(&mut self, value: Option<f64>, text: impl Into<String>) {
        self.value = value.filter(|v| v.is_finite()).map(normalize_zero);
        let text = text.into();
        if self.edit.text != text {
            self.edit.replace_text(text);
        }
        self.invalid = false;
        self.dirty = false;
    }
}

/// An editable numeric input (`role="spinbutton"`) styled with [`Kit::field_metrics`].
///
/// Supports integer and decimal entry, optional minimum/maximum bounds, configurable step and
/// page-step increments, optional empty values (`None`) distinct from `0`, editable intermediate
/// text (`-`, `.`, `1.`), clamping and reformatting on Enter or blur, `Up`/`Down` and
/// `PageUp`/`PageDown` stepping while leaving `Left`/`Right` to move the caret, optional
/// leading prefix / trailing unit suffix, optional `−` / `+` stepper buttons, and AccessKit
/// `SpinButton` semantics (`Increment`, `Decrement`, and numeric or text `SetValue`).
///
/// Wrap in [`crate::Field`] and pass `.width(None)` for a visible form label and help text;
/// [`Self::label`] and [`Self::description`] are accessibility-only, matching [`crate::TextInput`].
///
/// ```
/// # use rune_kit::{Kit, NumberField, NumberFieldState};
/// struct App { interval: NumberFieldState }
/// # let s = App { interval: NumberFieldState::new(Some(15.0)) };
/// # let k = &Kit::neutral(true);
/// let el = NumberField::new(&s.interval)
///     .label("Interval")
///     .range(1.0..=60.0)
///     .step(1.0)
///     .integer()
///     .suffix("s")
///     .stepper(true)
///     .on_edit(|s: &mut App| &mut s.interval)
///     .build(k);
/// # let _ = el;
/// ```
pub struct NumberField {
    state: NumberFieldState,
    min: Option<f64>,
    max: Option<f64>,
    step: f64,
    page_step: Option<f64>,
    decimals: Option<u8>,
    fixed_decimals: bool,
    allow_empty: bool,
    label: Option<String>,
    description: Option<String>,
    placeholder: String,
    prefix: Option<String>,
    suffix: Option<String>,
    stepper: bool,
    large: bool,
    style: Option<TextStyle>,
    height: Option<f32>,
    width: Option<f32>,
    plain: bool,
    invalid: bool,
    read_only: bool,
    visual_state: State,
    lens: Option<LensFn>,
    on_change: Option<ValueFn>,
    on_commit: Option<ValueFn>,
    on_focus_change: Option<FocusFn>,
    on_key: Option<KeyFn>,
    bind: Option<Rc<dyn Bind>>,
}

impl NumberField {
    /// Creates a number field displaying `state`. Defaults to unbounded decimals, `step = 1.0`,
    /// `allow_empty(true)`, width `Some(160.0)`, and no stepper buttons.
    pub fn new(state: &NumberFieldState) -> NumberField {
        NumberField {
            state: state.clone(),
            min: None,
            max: None,
            step: 1.0,
            page_step: None,
            decimals: None,
            fixed_decimals: false,
            allow_empty: true,
            label: None,
            description: None,
            placeholder: String::new(),
            prefix: None,
            suffix: None,
            stepper: false,
            large: false,
            style: None,
            height: None,
            width: Some(160.0),
            plain: false,
            invalid: false,
            read_only: false,
            visual_state: State::Rest,
            lens: None,
            on_change: None,
            on_commit: None,
            on_focus_change: None,
            on_key: None,
            bind: None,
        }
    }

    /// Sets the inclusive minimum value. Values below `min` stay editable while typing and clamp on
    /// commit (Enter or blur) and stepping.
    pub fn min(mut self, min: f64) -> Self {
        self.min = Some(min);
        self
    }

    /// Sets the inclusive maximum value. Values above `max` stay editable while typing and clamp on
    /// commit (Enter or blur) and stepping.
    pub fn max(mut self, max: f64) -> Self {
        self.max = Some(max);
        self
    }

    /// Sets both inclusive minimum and maximum bounds.
    pub fn range(mut self, range: RangeInclusive<f64>) -> Self {
        self.min = Some(*range.start());
        self.max = Some(*range.end());
        self
    }

    /// Sets the increment used by `Up`/`Down`, stepper buttons, and assistive `Increment`/`Decrement`
    /// (default `1.0`). Non-finite or non-positive values fall back to `1.0`.
    pub fn step(mut self, step: f64) -> Self {
        self.step = step;
        self
    }

    /// Sets the increment used by `PageUp`/`PageDown` (defaults to `step * 10.0`).
    pub fn page_step(mut self, step: f64) -> Self {
        self.page_step = Some(step);
        self
    }

    /// Sets the maximum fractional decimal places (0 for whole numbers). Excess fractional digits
    /// stay editable while typing and round on commit or stepping.
    pub fn decimals(mut self, decimals: u8) -> Self {
        self.decimals = Some(decimals);
        self
    }

    /// Restricts the field to whole numbers (shorthand for `.decimals(0)`).
    pub fn integer(self) -> Self {
        self.decimals(0)
    }

    /// When `true` and [`Self::decimals`] is set, pads trailing zeros on commit and stepping
    /// (for example `1.5` becomes `"1.50"` with `.decimals(2)`). Default `false`.
    pub fn fixed_decimals(mut self, fixed: bool) -> Self {
        self.fixed_decimals = fixed;
        self
    }

    /// Whether clearing the field produces `None` (default `true`, distinct from `Some(0.0)`).
    /// When `false`, committing an empty field restores the last accepted value.
    pub fn allow_empty(mut self, allow: bool) -> Self {
        self.allow_empty = allow;
        self
    }

    /// Placeholder text shown when the field is empty.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Muted prefix displayed before the editable digits (for example `"$"`).
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    /// Muted unit or suffix displayed after the editable digits (for example `"ms"`, `"px"`, `"%"`).
    pub fn suffix(mut self, suffix: impl Into<String>) -> Self {
        self.suffix = Some(suffix.into());
        self
    }

    /// Shows compact `−` and `+` stepper buttons inside the trailing edge of the box.
    /// Buttons stay out of the Tab order and preserve focus in the field when pressed.
    pub fn stepper(mut self, show: bool) -> Self {
        self.stepper = show;
        self
    }

    /// Accessible name for assistive technology (not drawn; wrap in [`crate::Field`] for a visible label).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Accessible description for assistive technology (not drawn).
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Uses the large field height (`FieldMetrics::large_height`, 32 pt by default).
    pub fn large(mut self) -> Self {
        self.large = true;
        self
    }

    /// Overrides the editor text style (defaults to `k.ty.ui_compact`).
    pub fn text_style(mut self, style: TextStyle) -> Self {
        self.style = Some(style);
        self
    }

    /// Overrides the box height in points (clamped to at least the text style's line height).
    pub fn height(mut self, points: f32) -> Self {
        self.height = Some(points.max(1.0));
        self
    }

    /// Width in points (default `Some(160.0)`); pass `None` to fill the parent (such as a [`crate::Field`]).
    pub fn width(mut self, width: Option<f32>) -> Self {
        self.width = width;
        self
    }

    /// Borderless appearance with no fill, shadow, hover tint, or focus outline (matching [`crate::TextInput::plain`]).
    pub fn plain(mut self) -> Self {
        self.plain = true;
        self
    }

    /// Marks the field invalid in addition to internal draft validation errors.
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }

    /// Keeps focus, caret, selection, and Copy while refusing typing, stepping, stepper buttons,
    /// and assistive `SetValue`.
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Forces a visual state for galleries and stories (`State::Rest` uses live interaction).
    pub fn state(mut self, state: State) -> Self {
        self.visual_state = state;
        self
    }

    /// Disables the field, dimming it and stripping all input and assistive handlers.
    pub fn disabled(mut self, disabled: bool) -> Self {
        if disabled {
            self.visual_state = State::Disabled;
        }
        self
    }

    /// Binds this field to its [`NumberFieldState`] in the application state.
    pub fn on_edit<S: 'static>(
        mut self,
        f: impl for<'a> Fn(&'a mut S) -> &'a mut NumberFieldState + 'static,
    ) -> Self {
        self.lens = Some(erase_lens::<S>(f));
        self.bind = Some(typed::<S>());
        self
    }

    /// Called whenever the accepted typed value changes (valid edits, clearing an optional field,
    /// stepping, commit clamping, or assistive `SetValue`).
    pub fn on_change<S: 'static>(self, f: impl Fn(&mut S, Option<f64>) + 'static) -> Self {
        self.on_change_tx(move |s: &mut S, v, _| f(s, v))
    }

    /// [`Self::on_change`] with access to [`Tx`].
    pub fn on_change_tx<S: 'static>(mut self, f: impl Fn(&mut S, Option<f64>, &mut Tx) + 'static) -> Self {
        self.on_change = Some(Rc::new(move |s: &mut Erased, v, tx: &mut Tx| f(state::<S>(s), v, tx)));
        self.bind = Some(typed::<S>());
        self
    }

    /// Called when the user commits a value via Enter, blur after editing, stepping, or assistive
    /// numeric `SetValue`.
    pub fn on_commit<S: 'static>(self, f: impl Fn(&mut S, Option<f64>) + 'static) -> Self {
        self.on_commit_tx(move |s: &mut S, v, _| f(s, v))
    }

    /// [`Self::on_commit`] with access to [`Tx`].
    pub fn on_commit_tx<S: 'static>(mut self, f: impl Fn(&mut S, Option<f64>, &mut Tx) + 'static) -> Self {
        self.on_commit = Some(Rc::new(move |s: &mut Erased, v, tx: &mut Tx| f(state::<S>(s), v, tx)));
        self.bind = Some(typed::<S>());
        self
    }

    /// Called with `true` when the editor gains in-window focus and `false` when it loses it.
    pub fn on_focus_change<S: 'static>(mut self, f: impl Fn(&mut S, bool, &mut Tx) + 'static) -> Self {
        self.on_focus_change = Some(Rc::new(move |s: &mut Erased, focused, tx: &mut Tx| f(state::<S>(s), focused, tx)));
        self.bind = Some(typed::<S>());
        self
    }

    /// Intercepts keys on the focused inner editor (`El::on_input_key`).
    pub fn on_key<S: 'static>(mut self, f: impl Fn(&mut S, &KeyEvent, &mut Tx) -> bool + 'static) -> Self {
        self.on_key = Some(Rc::new(move |s: &mut Erased, ev, tx: &mut Tx| f(state::<S>(s), ev, tx)));
        self.bind = Some(typed::<S>());
        self
    }

    /// Builds the number field element using semantic tokens from `k`.
    pub fn build(self, k: &Kit) -> El {
        let c = &k.c;
        let disabled = self.visual_state == State::Disabled;
        let spec = Spec::normalized(
            self.min,
            self.max,
            self.step,
            self.page_step,
            self.decimals,
            self.fixed_decimals,
            self.allow_empty,
        );
        let show_invalid = (self.invalid || self.state.invalid) && !disabled;
        let red = show_invalid && (k.invalid_text || self.plain);
        let ink = if red { c.danger.fg } else { c.text_base };
        let hint = if red { c.danger.fg } else { c.text_faint };
        let style = self.style.unwrap_or(k.ty.ui_compact);
        let m = k.field_metrics();
        let base = if self.large { m.large_height } else { m.height };
        let height = self.height.map_or_else(|| base.max(style.line_px() + 12.0), |h| h.max(style.line_px()));

        let mut field = text_input(&self.state.edit, style)
            .single_line()
            .key("input")
            .role("spinbutton")
            .grow(1.0)
            .shrink()
            .min_w(0.0)
            .color(ink)
            .rich_style(kit_rich_style(k))
            .placeholder(self.placeholder.clone(), hint)
            .caret(ink, k.selection());
        field = name_field(field, self.label.as_deref(), &self.placeholder, self.description.as_deref(), disabled)
            .invalid(show_invalid)
            .read_only(self.read_only);

        if let (Some(v), Some((min, max))) = (self.state.value, spec.finite_bounds()) {
            field = field.range_value(v, min, max).range_step(spec.step);
        }

        let wire = (!disabled)
            .then(|| {
                let bind = self.bind.as_ref()?;
                let lens = self.lens.as_ref()?;
                Some(Wiring {
                    bind: Rc::clone(bind),
                    lens: Rc::clone(lens),
                    on_change: self.on_change.clone(),
                    on_commit: self.on_commit.clone(),
                    spec,
                })
            })
            .flatten();

        if let Some(w) = &wire {
            field = w.bind.edit(field, Rc::clone(&w.lens));
            if let Some(f) = self.on_focus_change {
                field = w.bind.focus_change(field, f);
            }
            if let Some(f) = self.on_key {
                field = w.bind.input_key(field, f);
            }
            if !self.read_only {
                field = w.bind_interactive(field);
            }
        }

        let mut value_row = row().key("value").grow(1.0).shrink().min_w(0.0).gap(6.0).center_y().clip_rounded(0.0);
        if let Some(prefix) = self.prefix.filter(|p| !p.is_empty()) {
            value_row = value_row.child(
                el().key("prefix")
                    .pl(m.padding_x)
                    .center()
                    .child(text(prefix).font(style).color(c.text_muted))
                    .a11y_hidden(),
            );
        } else {
            value_row = value_row.child(el().w((m.padding_x - 6.0).max(0.0)));
        }
        value_row = value_row.child(field);

        let pr = if self.stepper { 4.0 } else { m.padding_x };
        let mut box_el = row().role("textbox").h(height).pr(pr).gap(6.0).child(value_row);
        box_el = match self.width {
            Some(w) => box_el.w(w),
            None => box_el.w_full(),
        };

        if let Some(suffix) = self.suffix.filter(|s| !s.is_empty()) {
            box_el = box_el.child(
                text(suffix)
                    .key("suffix")
                    .font(style)
                    .color(c.text_muted)
                    .self_center()
                    .a11y_hidden(),
            );
        }

        if self.stepper {
            let btn_side = (height - 8.0).clamp(18.0, 24.0);
            let icon_side = (btn_side - 6.0).clamp(12.0, 16.0);
            let dec_disabled = disabled || self.read_only || spec.at_min(&self.state);
            let inc_disabled = disabled || self.read_only || spec.at_max(&self.state);
            let dec = step_button(
                k,
                "decrement",
                "Decrement",
                &glyphs::MINUS,
                btn_side,
                icon_side,
                dec_disabled,
                wire.as_ref(),
                -1,
            );
            let inc = step_button(
                k,
                "increment",
                "Increment",
                &glyphs::PLUS,
                btn_side,
                icon_side,
                inc_disabled,
                wire.as_ref(),
                1,
            );
            box_el = box_el.child(row().key("stepper").gap(2.0).self_center().center_y().child(dec).child(inc));
        }

        if self.plain {
            let e = match k.forced {
                Some(highlight) if self.state.edit.focused && !disabled => box_el.outline(2.0, 2.0, highlight),
                _ => box_el,
            };
            return k.disabled_dim(e, disabled);
        }
        shell(k, box_el, self.visual_state, show_invalid, self.state.edit.focused && !disabled)
    }
}

struct Wiring {
    bind: Rc<dyn Bind>,
    lens: LensFn,
    on_change: Option<ValueFn>,
    on_commit: Option<ValueFn>,
    spec: Spec,
}

impl Wiring {
    fn bind_interactive(&self, mut e: El) -> El {
        let (lens, on_change, spec) = (Rc::clone(&self.lens), self.on_change.clone(), self.spec);
        e = self.bind.input_change(
            e,
            Rc::new(move |s: &mut Erased, tx: &mut Tx| {
                if let Some(next) = spec.apply_input_change(lens(s))
                    && let Some(cb) = &on_change
                {
                    cb(s, next, tx);
                }
            }),
        );

        let (lens, on_change, on_commit, spec) =
            (Rc::clone(&self.lens), self.on_change.clone(), self.on_commit.clone(), self.spec);
        e = self.bind.submit(
            e,
            Rc::new(move |s: &mut Erased, tx: &mut Tx| {
                let out = spec.commit(lens(s), true);
                if let Some(v) = out.changed
                    && let Some(cb) = &on_change
                {
                    cb(s, v, tx);
                }
                if let Some(v) = out.committed
                    && let Some(cb) = &on_commit
                {
                    cb(s, v, tx);
                }
            }),
        );

        let (lens, on_change, on_commit, spec) =
            (Rc::clone(&self.lens), self.on_change.clone(), self.on_commit.clone(), self.spec);
        e = self.bind.focus_lost(
            e,
            Rc::new(move |s: &mut Erased, tx: &mut Tx| {
                let out = spec.commit(lens(s), false);
                if let Some(v) = out.changed
                    && let Some(cb) = &on_change
                {
                    cb(s, v, tx);
                }
                if let Some(v) = out.committed
                    && let Some(cb) = &on_commit
                {
                    cb(s, v, tx);
                }
            }),
        );

        let (lens, on_change, on_commit, spec) =
            (Rc::clone(&self.lens), self.on_change.clone(), self.on_commit.clone(), self.spec);
        e = self.bind.step(
            e,
            Rc::new(move |s: &mut Erased, delta: i32, tx: &mut Tx| {
                let (changed, committed) = spec.step_by(lens(s), delta);
                if let Some(v) = changed
                    && let Some(cb) = &on_change
                {
                    cb(s, v, tx);
                }
                if let Some(cb) = &on_commit {
                    cb(s, committed, tx);
                }
            }),
        );

        let (lens, on_change, on_commit, spec) =
            (Rc::clone(&self.lens), self.on_change.clone(), self.on_commit.clone(), self.spec);
        self.bind.set_value(
            e,
            Rc::new(move |s: &mut Erased, target: f64, tx: &mut Tx| {
                if let Some((changed, committed)) = spec.set_numeric(lens(s), target) {
                    if let Some(v) = changed
                        && let Some(cb) = &on_change
                    {
                        cb(s, v, tx);
                    }
                    if let Some(cb) = &on_commit {
                        cb(s, committed, tx);
                    }
                }
            }),
        )
    }

    fn bind_step_press(&self, e: El, delta: i32) -> El {
        let (lens, on_change, on_commit, spec) =
            (Rc::clone(&self.lens), self.on_change.clone(), self.on_commit.clone(), self.spec);
        self.bind.press(
            e,
            Rc::new(move |s: &mut Erased, tx: &mut Tx| {
                let (changed, committed) = spec.step_by(lens(s), delta);
                if let Some(v) = changed
                    && let Some(cb) = &on_change
                {
                    cb(s, v, tx);
                }
                if let Some(cb) = &on_commit {
                    cb(s, committed, tx);
                }
            }),
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn step_button(
    k: &Kit,
    key: &'static str,
    label: &'static str,
    glyph: &'static Icon,
    side: f32,
    icon_side: f32,
    disabled: bool,
    wire: Option<&Wiring>,
    delta: i32,
) -> El {
    let c = &k.c;
    let color = if disabled { c.icon_faint } else { c.icon_muted };
    let mut b = row()
        .key(key)
        .role("button")
        .label(label)
        .size(side, side)
        .center()
        .radius(k.radius.sm)
        .focusable(false)
        .preserve_focus_on_press(true)
        .child(icon(glyph, icon_side).color(color))
        .disabled(disabled);
    if !disabled {
        b = k.interactive(b.hit());
        if let Some(w) = wire {
            b = w.bind_step_press(b, delta);
        }
    }
    b
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
