//! PROOF ONLY, never merged (rune-proof: device-removal): opens a Rune window on WARP, removes
//! the WARP device mid-frame right after `renderer.submit` with extra compute work queued on the
//! same queue, verifies `reattach` brings up a new WARP device without crashing or hanging, and
//! draws frames 3..10 cleanly.
#![allow(unsafe_code)]
#![windows_subsystem = "windows"]
use rune::{AppSpec, Color, Cx, El, TextStyle, col, el, text};

#[derive(Default)]
struct State {
    ticks: u32,
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let label = text(format!("Frame tick {}", s.ticks & 1)).font(TextStyle::new(20.0, 400.0)).color(Color::WHITE);
    let badge = el()
        .key("badge")
        .px(16.0)
        .h(32.0)
        .center()
        .radius(4.0)
        .fill(rune::oklch(0.63, 0.22, 250.0))
        .child(text("WARP recovery").font(TextStyle::new(14.0, 500.0)).color(Color::WHITE));
    col().w_full().h_full().center().gap(16.0).fill(rune::oklch(0.2, 0.0, 0.0)).child(label).child(badge)
}

fn poll(s: &mut State, _: f64, tx: &mut rune::Tx) -> bool {
    s.ticks = s.ticks.wrapping_add(1);
    tx.wake_after(std::time::Duration::from_millis(1));
    true
}

fn main() {
    // SAFETY: proof-only single-threaded setup before Rune starts.
    unsafe { std::env::set_var("RUNE_PROOF_WINDOW_MIDFRAME", "1"); }
    rune::run(AppSpec {
        poll: Some(poll),
        ..AppSpec::new("gpu-safe-window-recover", (480.0, 320.0), State::default, view)
    });
}
