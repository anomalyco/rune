//! A Rune app hosting a Luau script (`scripts/panel`): a header with buttons, a pulse driven
//! by `task.wait`, a signal, and a 1,000-row `ui.virtualList`. Native builds watch the script
//! folder, so saving a `.luau` file reloads the panel in place. The same script runs on the web:
//! `scripts/web-build.sh luau-panel` (with `WASI_SDK_PATH` set).
//!
//! `cargo run -p luau-panel`; `cargo run -p luau-panel --bin luau-bench --profile perf` measures
//! the performance gates (idle, memo hit, 20-row rebuild, list scroll, hot reload).

use std::cell::Cell;
use std::time::Duration;

use rune::time::Instant;
use rune::{AppSpec, Cx, El, Tx, Waker, col};
use rune_luau::{Script, Sources};

pub mod bench;
pub mod checks;

pub struct State {
    pub script: Script,
    /// Host state the script doesn't read: bumping it re-runs the view, not the script.
    pub host_ticks: u64,
    /// How long the last `Script::view` call took, and the VM entries it made.
    pub view_time: Cell<Duration>,
    pub view_calls: Cell<u64>,
}

impl State {
    pub fn new(script: Script) -> Self {
        State { script, host_ticks: 0, view_time: Cell::new(Duration::ZERO), view_calls: Cell::new(0) }
    }
}

/// The panel's files, built into the binary.
pub fn sources() -> Sources {
    Sources::new()
        .with("init.luau", include_str!("../scripts/panel/init.luau"))
        .with("rows.luau", include_str!("../scripts/panel/rows.luau"))
}

fn init() -> State {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/panel");
        if let Ok(sources) = Sources::from_dir(dir) {
            let options = rune_luau::ScriptOptions::new().on_log(|level, line| eprintln!("luau-panel {level:?}: {line}"));
            let script = Script::with_options("panel", sources, options);
            if let Err(e) = script.watch(dir) {
                eprintln!("luau-panel: not watching {dir}: {e}");
            }
            return State::new(script);
        }
    }
    State::new(Script::new("panel", sources()))
}

pub fn view(s: &State, cx: &mut Cx<State>) -> El {
    let calls = s.script.metrics().vm_calls;
    let start = Instant::now();
    let panel = s.script.view(cx);
    s.view_time.set(start.elapsed());
    s.view_calls.set(s.script.metrics().vm_calls - calls);
    col().w_full().h_full().child(panel)
}

fn setup(s: &mut State, waker: Waker) {
    s.script.set_waker(waker);
}

fn poll(s: &mut State, now: f64, tx: &mut Tx) -> bool {
    s.script.poll(now, tx)
}

pub fn spec() -> AppSpec<State> {
    AppSpec { setup: Some(setup), poll: Some(poll), ..AppSpec::new("Luau panel", (720.0, 640.0), init, view) }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen::prelude::wasm_bindgen(start))]
pub fn start() {
    rune::run(spec());
}

/// Runs the performance measurements in the page and returns them as JSON.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn luau_bench() -> String {
    bench::run(bench::Scale::Web).json()
}
