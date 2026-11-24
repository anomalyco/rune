//! `Transition::hold_layout()` analytic slot collapse in vertical and horizontal flex containers,
//! including mid-exit revival without per-frame layout.
//!
//! Headless trace and motion audit (`scripts/motion-audit/run.sh` records both):
//!   cargo run -p rune --example hold_layout -- --script scripts/motion-audit/scenarios/hold-layout.json --trace t.jsonl --trace-all
use rune::{AppSpec, Color, Cx, El, Spring, TextStyle, Transition, col, el, oklch, row, text};

struct State {
    vert: Vec<&'static str>,
    horiz: Vec<&'static str>,
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let vert = col()
        .key("vert")
        .w(240.0)
        .gap(6.0)
        .children(s.vert.iter().map(|&k| {
            row()
                .key(k)
                .w_full()
                .h(40.0)
                .px(12.0)
                .items_center()
                .radius(8.0)
                .fill(oklch(0.30, 0.03, 250.0))
                .motion(Spring::new(0.35, 0.0))
                .transition(Transition::opacity().hold_layout())
                .on_press(move |s: &mut State| s.vert.retain(|x| *x != k))
                .child(text(k).font(TextStyle::new(14.0, 500.0)).color(Color::WHITE))
        }));

    let horiz = row()
        .key("horiz")
        .w(320.0)
        .h(36.0)
        .gap(8.0)
        .children(s.horiz.iter().map(|&k| {
            row()
                .key(k)
                .w(88.0)
                .h_full()
                .px(10.0)
                .items_center()
                .radius(8.0)
                .fill(oklch(0.36, 0.05, 220.0))
                .motion(Spring::new(0.35, 0.0))
                .transition(Transition::opacity().hold_layout())
                .on_press(move |s: &mut State| s.horiz.retain(|x| *x != k))
                .child(text(k).font(TextStyle::new(13.0, 500.0)).color(Color::WHITE))
        }));

    let restore = el()
        .key("restore")
        .px(12.0)
        .py(6.0)
        .radius(6.0)
        .fill(oklch(0.45, 0.12, 250.0))
        .on_press(|s: &mut State| {
            s.vert = vec!["v-a", "v-b", "v-c"];
            s.horiz = vec!["h-a", "h-b", "h-c"];
        })
        .child(text("Restore").font(TextStyle::new(13.0, 500.0)).color(Color::WHITE));

    col()
        .w_full()
        .h_full()
        .padding(20.0)
        .gap(16.0)
        .fill(oklch(0.18, 0.01, 250.0))
        .children([vert, horiz, restore])
}

fn main() {
    rune::run(AppSpec::new(
        "Hold layout",
        (380.0, 280.0),
        || State {
            vert: vec!["v-a", "v-b", "v-c"],
            horiz: vec!["h-a", "h-b", "h-c"],
        },
        view,
    ));
}
