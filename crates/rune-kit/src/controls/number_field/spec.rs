//! Numeric parsing, formatting, clamping, and stepping rules for [`super::NumberField`].

use super::NumberFieldState;

/// Maximum decimal precision preserved when rounding or formatting.
const MAX_DECIMALS: u8 = 12;

/// Classification of the live text buffer while the user is typing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Draft {
    /// Empty or whitespace-only text.
    Empty,
    /// Valid in-progress prefix or trailing decimal point, such as `-`, `+`, `.`, `-.`, or `1.`.
    Intermediate,
    /// Complete finite numeric value.
    Number(f64),
    /// Non-numeric or malformed text (for example `abc`, `1.2.3`, `NaN`, `inf`).
    Invalid,
}

/// Sanitized numeric constraints shared by live editing, commit, stepping, and a11y handlers.
#[derive(Clone, Copy, Debug)]
pub(super) struct Spec {
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: f64,
    pub page_step: f64,
    pub decimals: Option<u8>,
    pub fixed_decimals: bool,
    pub allow_empty: bool,
}

/// Result of committing the text buffer on Enter or blur.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CommitOutcome {
    /// `Some(v)` when the accepted typed value changed.
    pub changed: Option<Option<f64>>,
    /// `Some(v)` when commit callbacks should fire.
    pub committed: Option<Option<f64>>,
}

impl Spec {
    pub(super) fn normalized(
        min: Option<f64>,
        max: Option<f64>,
        step: f64,
        page_step: Option<f64>,
        decimals: Option<u8>,
        fixed_decimals: bool,
        allow_empty: bool,
    ) -> Spec {
        let min = min.filter(|v| v.is_finite()).map(normalize_zero);
        let max = max.filter(|v| v.is_finite()).map(normalize_zero);
        let (min, max) = match (min, max) {
            (Some(lo), Some(hi)) if lo > hi => (Some(lo), Some(lo)),
            other => other,
        };
        let step = if step.is_finite() && step > 0.0 { step } else { 1.0 };
        let page_step = page_step
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or_else(|| {
                let scaled = step * 10.0;
                if scaled.is_finite() { scaled } else { step }
            });
        let decimals = decimals.map(|d| d.min(MAX_DECIMALS));
        Spec { min, max, step, page_step, decimals, fixed_decimals, allow_empty }
    }

    pub(super) fn finite_bounds(&self) -> Option<(f64, f64)> {
        match (self.min, self.max) {
            (Some(min), Some(max)) if min.is_finite() && max.is_finite() && min <= max => Some((min, max)),
            _ => None,
        }
    }

    pub(super) fn clamp(&self, v: f64) -> f64 {
        let mut out = v;
        if let Some(min) = self.min {
            out = out.max(min);
        }
        if let Some(max) = self.max {
            out = out.min(max);
        }
        normalize_zero(out)
    }

    pub(super) fn in_bounds(&self, v: f64) -> bool {
        self.min.is_none_or(|min| v >= min) && self.max.is_none_or(|max| v <= max)
    }

    pub(super) fn format(&self, v: f64) -> String {
        format_number(v, self.decimals, self.fixed_decimals)
    }

    pub(super) fn effective_number(&self, state: &NumberFieldState) -> Option<f64> {
        parse_commit_number(&state.edit.text).or(state.value)
    }

    pub(super) fn at_min(&self, state: &NumberFieldState) -> bool {
        self.min.is_some_and(|min| self.effective_number(state).is_some_and(|v| v <= min + 1e-12))
    }

    pub(super) fn at_max(&self, state: &NumberFieldState) -> bool {
        self.max.is_some_and(|max| self.effective_number(state).is_some_and(|v| v >= max - 1e-12))
    }

    /// Processes a live text edit without overwriting the user's in-progress text.
    /// Returns `Some(next)` when the accepted typed value changed.
    pub(super) fn apply_input_change(&self, state: &mut NumberFieldState) -> Option<Option<f64>> {
        state.dirty = true;
        match parse_draft(&state.edit.text, self.decimals) {
            Draft::Empty => {
                if self.allow_empty {
                    state.invalid = false;
                    if state.value.is_some() {
                        state.value = None;
                        return Some(None);
                    }
                } else {
                    state.invalid = true;
                }
                None
            }
            Draft::Intermediate => {
                state.invalid = false;
                None
            }
            Draft::Number(v) => {
                let v = normalize_zero(v);
                if self.in_bounds(v) && fits_decimals(&state.edit.text, self.decimals) {
                    state.invalid = false;
                    if state.value != Some(v) {
                        state.value = Some(v);
                        return Some(Some(v));
                    }
                } else {
                    state.invalid = true;
                }
                None
            }
            Draft::Invalid => {
                state.invalid = true;
                None
            }
        }
    }

    /// Commits the current draft (on Enter with `force = true`, or on blur with `force = false`):
    /// clamps to bounds, rounds to `decimals`, reformats the text, or reverts invalid input to the
    /// last accepted value.
    pub(super) fn commit(&self, state: &mut NumberFieldState, force: bool) -> CommitOutcome {
        if !force && !state.dirty && !state.invalid {
            return CommitOutcome { changed: None, committed: None };
        }
        let trimmed = state.edit.text.trim();
        let next = if trimmed.is_empty() {
            if self.allow_empty {
                None
            } else {
                state.value.map(|v| self.finalize(v))
            }
        } else if let Some(parsed) = parse_commit_number(trimmed) {
            Some(self.finalize(parsed))
        } else {
            state.value.map(|v| self.finalize(v))
        };

        let formatted = next.map(|v| self.format(v)).unwrap_or_default();
        if state.edit.text != formatted {
            state.edit.replace_text(formatted);
        }
        let changed = (state.value != next).then_some(next);
        state.value = next;
        state.invalid = false;
        state.dirty = false;
        CommitOutcome { changed, committed: Some(next) }
    }

    /// Steps the current number by `delta` (`±1` for single steps, `±10` for page steps).
    /// Returns `(changed, committed_value)`.
    pub(super) fn step_by(&self, state: &mut NumberFieldState, delta: i32) -> (Option<Option<f64>>, Option<f64>) {
        let offset = match delta {
            10 => self.page_step,
            -10 => -self.page_step,
            d => f64::from(d) * self.step,
        };
        let base = self.effective_number(state).unwrap_or(0.0);
        let raw = base + offset;
        let next = if raw.is_finite() {
            let prec = self.decimals.unwrap_or_else(|| step_precision(offset.abs(), base));
            self.clamp(round_to_decimals(raw, prec))
        } else {
            self.clamp(base)
        };
        let formatted = self.format(next);
        if state.edit.text != formatted {
            state.edit.replace_text(formatted);
        }
        let changed = (state.value != Some(next)).then_some(Some(next));
        state.value = Some(next);
        state.invalid = false;
        state.dirty = false;
        (changed, Some(next))
    }

    /// Applies an absolute numeric value from assistive technology (`ActionData::NumericValue`).
    pub(super) fn set_numeric(&self, state: &mut NumberFieldState, target: f64) -> Option<(Option<Option<f64>>, Option<f64>)> {
        if !target.is_finite() {
            return None;
        }
        let next = self.finalize(target);
        let formatted = self.format(next);
        if state.edit.text != formatted {
            state.edit.replace_text(formatted);
        }
        let changed = (state.value != Some(next)).then_some(Some(next));
        state.value = Some(next);
        state.invalid = false;
        state.dirty = false;
        Some((changed, Some(next)))
    }

    fn finalize(&self, v: f64) -> f64 {
        let rounded = self.decimals.map_or(v, |d| round_to_decimals(v, d));
        self.clamp(normalize_zero(rounded))
    }
}

pub(super) fn normalize_zero(v: f64) -> f64 {
    if v == 0.0 { 0.0 } else { v }
}

pub(super) fn round_to_decimals(v: f64, decimals: u8) -> f64 {
    let d = decimals.min(MAX_DECIMALS) as i32;
    let factor = 10_f64.powi(d);
    let scaled = v * factor;
    if !scaled.is_finite() {
        return normalize_zero(v);
    }
    normalize_zero(scaled.round() / factor)
}

pub(super) fn format_number(v: f64, decimals: Option<u8>, fixed: bool) -> String {
    let v = normalize_zero(v);
    match decimals {
        Some(d) => {
            let d = d.min(MAX_DECIMALS);
            let rounded = round_to_decimals(v, d);
            let mut s = format!("{rounded:.prec$}", prec = usize::from(d));
            if !fixed && d > 0 && s.contains('.') {
                let trimmed = s.trim_end_matches('0').trim_end_matches('.');
                s.truncate(trimmed.len());
            }
            s
        }
        None => format!("{v}"),
    }
}

pub(super) fn parse_draft(raw: &str, decimals: Option<u8>) -> Draft {
    let s = raw.trim();
    if s.is_empty() {
        return Draft::Empty;
    }
    if matches!(s, "-" | "+") {
        return Draft::Intermediate;
    }
    if matches!(s, "." | "-." | "+.") {
        return if decimals == Some(0) { Draft::Invalid } else { Draft::Intermediate };
    }
    let Some(parsed) = parse_commit_number(s) else {
        return Draft::Invalid;
    };
    if s.ends_with('.') {
        return if decimals == Some(0) { Draft::Invalid } else { Draft::Intermediate };
    }
    Draft::Number(parsed)
}

/// Parses an ASCII decimal number (optional leading `+`/`-`, digits, optional single `.`, at
/// least one digit). Rejects `NaN`, `inf`, and alphabetic/exponent notation.
pub(super) fn parse_commit_number(raw: &str) -> Option<f64> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let body = s.strip_prefix(['+', '-']).unwrap_or(s);
    let mut digits = 0usize;
    let mut dots = 0usize;
    for b in body.bytes() {
        match b {
            b'0'..=b'9' => digits += 1,
            b'.' => dots += 1,
            _ => return None,
        }
    }
    if digits == 0 || dots > 1 {
        return None;
    }
    s.parse::<f64>().ok().filter(|v| v.is_finite()).map(normalize_zero)
}

fn fits_decimals(raw: &str, decimals: Option<u8>) -> bool {
    let Some(d) = decimals else { return true };
    let s = raw.trim();
    if d == 0 {
        return !s.contains('.');
    }
    match s.split_once('.') {
        None => true,
        Some((_, frac)) => frac.len() <= usize::from(d),
    }
}

fn decimal_places(x: f64) -> u8 {
    if !x.is_finite() {
        return 0;
    }
    let s = format!("{}", x.abs());
    if let Some((mant, exp)) = s.split_once(['e', 'E']) {
        let exp: i32 = exp.parse().unwrap_or(0);
        let frac = mant.split_once('.').map_or(0, |(_, f)| f.len() as i32);
        return (frac - exp).clamp(0, i32::from(MAX_DECIMALS)) as u8;
    }
    s.split_once('.').map_or(0, |(_, f)| f.len().min(usize::from(MAX_DECIMALS)) as u8)
}

fn step_precision(step: f64, base: f64) -> u8 {
    decimal_places(step).max(decimal_places(base)).min(MAX_DECIMALS)
}
