//! Editable number fields: a bounded whole-number interval next to an optional decimal field.
//!
//! `cargo run -p rune-kit --example number_field`

use rune::{App, AppSpec, Cx, El, col, row, text};
use rune_kit::{Field, Kit, NumberField, NumberFieldState};

struct State {
    interval: NumberFieldState,
    threshold: NumberFieldState,
}

impl Default for State {
    fn default() -> Self {
        Self {
            interval: NumberFieldState::new(Some(15.0)),
            threshold: NumberFieldState::empty(),
        }
    }
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let k = Kit::neutral(true);
    let interval_summary = match s.interval.as_i64() {
        Some(n) => format!("{n} s"),
        None => "unset".to_owned(),
    };
    let threshold_summary = match s.threshold.value() {
        Some(v) => format!("{v}"),
        None => "auto (empty)".to_owned(),
    };
    col()
        .w_full()
        .h_full()
        .padding(24.0)
        .gap(16.0)
        .fill(k.c.page)
        .child(
            row()
                .w_full()
                .gap(16.0)
                .child(
                    Field::new()
                        .label("Poll interval")
                        .suffix("Whole seconds between 1 and 60.")
                        .control(
                            NumberField::new(&s.interval)
                                .label("Poll interval")
                                .range(1.0..=60.0)
                                .step(1.0)
                                .page_step(5.0)
                                .integer()
                                .allow_empty(false)
                                .suffix("s")
                                .stepper(true)
                                .width(None)
                                .on_edit(|s: &mut State| &mut s.interval)
                                .build(&k)
                                .key("interval"),
                        )
                        .build(&k),
                )
                .child(
                    Field::new()
                        .label("Alert threshold")
                        .suffix("Leave empty for automatic; 0 is distinct from empty.")
                        .control(
                            NumberField::new(&s.threshold)
                                .label("Alert threshold")
                                .placeholder("Auto")
                                .step(0.25)
                                .decimals(2)
                                .stepper(true)
                                .width(None)
                                .on_edit(|s: &mut State| &mut s.threshold)
                                .build(&k)
                                .key("threshold"),
                        )
                        .build(&k),
                ),
        )
        .child(
            text(format!("Interval: {interval_summary}  |  Threshold: {threshold_summary}"))
                .font(k.ty.ui_compact)
                .color(k.c.text_muted),
        )
}

fn main() {
    App::new(AppSpec::new("Number field", (520.0, 180.0), State::default, view)).run();
}
