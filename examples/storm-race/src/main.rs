//! The storm window: `cargo run -p storm-race --release -- 5000` for 5,000 cells. `STORM_AT=x,y`
//! places its top-left corner at `x, y` points on the primary display (centred otherwise).
//! Headless: `storm-race --size 1000x720 --scale 2 --frames 240 --snap last.png 5000`.

use rune::{App, AppSpec, WindowOptions, WindowSpec};
use storm_race::{SIZE, init, poll, view};

fn spec() -> AppSpec<storm_race::State> {
    AppSpec { poll: Some(poll), ..AppSpec::new("Storm", SIZE, init, view) }
}

fn main() {
    let at = std::env::var("STORM_AT").ok().and_then(|v| v.split_once(',').and_then(|(x, y)| Some((x.parse::<f64>().ok()?, y.parse::<f64>().ok()?))));
    let Some((x, y)) = at.filter(|_| !rune::is_headless()) else { return rune::run(spec()) };
    rune::run_windows(move |displays| {
        let display = displays.iter().find(|d| d.primary).or_else(|| displays.first());
        let origin = display.map_or((0, 0), |d| (d.position.0 + (x * d.scale) as i32, d.position.1 + (y * d.scale) as i32));
        vec![WindowSpec::from_app("storm", App::new(spec())).options(WindowOptions { position: Some(origin), ..Default::default() })]
    });
}
