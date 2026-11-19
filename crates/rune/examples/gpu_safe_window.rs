//! PROOF ONLY, never merged (rune-proof: device-removal): a window with a label and a button,
//! for `RUNE_PROOF_REMOVE=window` (the window's device removed after its second GPU frame) and
//! `RUNE_PROOF_REMOVE=snap` (with `--snap`: removed with the snapshot's copy in flight). WARP only.
#![windows_subsystem = "windows"]
use rune::{AppSpec, Color, Cx, El, TextStyle, col, el, text};

#[derive(Default)]
struct State {
    clicks: u32,
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let label = text(format!("Clicked {} times", s.clicks)).font(TextStyle::new(20.0, 400.0)).color(Color::WHITE);
    let button = el()
        .key("button")
        .px(16.0)
        .h(32.0)
        .center()
        .radius(4.0)
        .fill(rune::oklch(0.63, 0.22, 250.0))
        .on_press(|s: &mut State| s.clicks += 1)
        .child(text("Click me").font(TextStyle::new(14.0, 500.0)).color(Color::WHITE));
    col().w_full().h_full().center().gap(16.0).fill(rune::oklch(0.2, 0.0, 0.0)).child(label).child(button)
}

fn main() {
    rune::run(AppSpec::new("gpu-safe-hello", (480.0, 320.0), State::default, view));
}
