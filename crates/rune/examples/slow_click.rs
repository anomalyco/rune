//! A settings page whose Save button stalls its handler for 40 ms: the slow click interaction
//! timing (`RUNE_PERF`) reports, with its page and control names, to a local OTLP collector.
//!
//! `RUNE_PERF=1 cargo run -p rune --example slow_click -- --script crates/rune/examples/slow_click.json
//! --snap saved.png`, then find the `ui.interaction` span in the collector (route `settings`,
//! control `settings/toolbar/save`, action `on_press`).

use std::time::Duration;

use rune::{AppSpec, Color, Cx, El, TextStyle, col, el, row, text};

#[derive(Default)]
struct Settings {
    saves: u32,
}

fn view(s: &Settings, _: &mut Cx<Settings>) -> El {
    let font = TextStyle::new(14.0, 400.0);
    let rows = (0..40).fold(col().key("fields").gap(4.0), |c, i| c.child(row().key(format!("field-{i}")).h(20.0).child(text(format!("Setting {i}")).font(font))));
    col().key("settings").w_full().h_full().padding(16.0).gap(12.0)
        .child(row().key("toolbar").gap(8.0).center_y()
            .child(el().key("save").size(80.0, 30.0).radius(6.0).center().fill(Color::WHITE.alpha(0.12))
                .on_press(|s: &mut Settings| {
                    std::thread::sleep(Duration::from_millis(40));
                    s.saves += 1;
                })
                .child(text("Save").font(font)))
            .child(text(format!("saved {}", s.saves)).font(font)))
        .child(rows)
}

fn main() {
    rune::run(AppSpec::new("Slow click", (480.0, 360.0), Settings::default, view));
}
