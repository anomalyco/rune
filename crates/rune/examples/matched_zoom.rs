//! Matched flight off and back onto a zoomed, panned board (`live_scale` + `live_offset`) that
//! unmounts while the card is open and mounts again as it flies home.
//!
//! Headless trace and motion audit (`scripts/motion-audit/run.sh` records both):
//!   cargo run -p rune --example matched_zoom -- --script scripts/motion-audit/scenarios/matched-zoom.json --trace t.jsonl --trace-all
use rune::{AppSpec, Color, Cx, El, Live, col, el, oklch};

struct State {
    open: bool,
    zoom: Live<f32>,
    pan: Live<[f32; 2]>,
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let card = |w: f32, h: f32| {
        el().key("card")
            .matched("card")
            .size(w, h)
            .radius(8.0)
            .fill(oklch(0.63, 0.22, 27.0))
            .child(el().key("badge").absolute(10.0, 6.0).size(20.0, 10.0).radius(3.0).fill(Color::WHITE))
            .on_press(|s: &mut State| s.open = !s.open)
    };
    let mut root = col().w_full().h_full().fill(oklch(0.18, 0.01, 250.0));
    if s.open {
        root = root.child(card(160.0, 80.0).absolute(120.0, 20.0));
    } else {
        let board = el()
            .key("board")
            .size(400.0, 300.0)
            .fill(oklch(0.24, 0.02, 250.0))
            .live_scale(&s.zoom)
            .live_offset(&s.pan)
            .child(card(100.0, 50.0).absolute(40.0, 40.0));
        root = root.child(board);
    }
    root
}

fn main() {
    rune::run(AppSpec::new(
        "Matched zoom",
        (400.0, 300.0),
        || State {
            open: false,
            zoom: Live::new(0.5),
            pan: Live::new([60.0, 20.0]),
        },
        view,
    ));
}
