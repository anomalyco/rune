//! The least a Rune window holds: one window, one line of text. A measuring instrument for
//! memory: read its private working set from outside (`measure.ps1` beside this file does, as
//! Task Manager's "Memory" column does), change one thing, read it again.
//!
//! `HELLO_FLOOR_STEP` picks what the window draws:
//! - `empty`: a filled window, no text (a device, a swap chain and a frame);
//! - `text` (the default): one line of text (the font stack and a glyph atlas page);
//! - `effect`: the line of text under a frosted bar (a backdrop blur: its pipelines, its copy of
//!   what is under it and its blur pyramid).
//!
//! `HELLO_FLOOR_MEMORY_POLICY` sets the GPU suballocation policy before the device is made
//! (`memory-usage`, Rune's default; `performance`; or `manual:<min>-<max>` in MiB, as
//! `manual:4-64`). `HELLO_FLOOR_WARM` names the effect pipelines compiled ahead of their first
//! use (`none`, `all`, `backdrop`). `WGPU_ADAPTER_NAME` picks the GPU, as in every Rune app, and
//! `RUNE_CPU_FIRST=only` keeps the window on Rune's CPU renderer: no GPU device at all until a
//! frame needs one (the `effect` step does).

#![forbid(unsafe_code)]
#![cfg_attr(windows, windows_subsystem = "windows")]

use rune::tok::{self, type_};
use rune::{AppSpec, Cx, El, col, el, text};

#[derive(Clone, Copy, PartialEq)]
enum Step {
    Empty,
    Text,
    Effect,
}

struct State {
    step: Step,
}

fn init() -> State {
    let step = match rune::env::var("HELLO_FLOOR_STEP").as_deref() {
        Some("empty") => Step::Empty,
        Some("effect") => Step::Effect,
        _ => Step::Text,
    };
    State { step }
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let t = tok::theme(true);
    let page = col().w_full().h_full().center().fill(t.bg);
    match s.step {
        Step::Empty => page,
        Step::Text => page.child(text("Hello, world").font(type_::TITLE).color(t.text)),
        Step::Effect => page
            .child(text("Hello, world").font(type_::TITLE).color(t.text))
            .child(el().key("frost").absolute(120.0, 150.0).size(240.0, 60.0).radius(12.0).backdrop_blur(8.0)),
    }
}

fn main() {
    let app = rune::App::new(AppSpec::new("Hello floor", (480.0, 320.0), init, view));
    let app = match rune::env::var("HELLO_FLOOR_MEMORY_POLICY").as_deref().and_then(policy) {
        Some(policy) => app.gpu_memory_policy(policy),
        None => app,
    };
    let app = match rune::env::var("HELLO_FLOOR_WARM").as_deref() {
        Some("none") => app.prewarm_effects(rune::PrewarmEffects::NONE),
        Some("all") => app.prewarm_effects(rune::PrewarmEffects::ALL),
        Some("backdrop") => app.prewarm_effects(rune::PrewarmEffects::BACKDROP_BLUR),
        _ => app,
    };
    app.run();
}

fn policy(raw: &str) -> Option<rune::GpuMemoryPolicy> {
    match raw {
        "memory-usage" => Some(rune::GpuMemoryPolicy::MemoryUsage),
        "performance" => Some(rune::GpuMemoryPolicy::Performance),
        _ => {
            let (min, max) = raw.strip_prefix("manual:")?.split_once('-')?;
            let mib = |s: &str| s.trim().parse::<u64>().ok().map(|n| n * 1024 * 1024);
            Some(rune::GpuMemoryPolicy::Manual { block_size: mib(min)?..mib(max)? })
        }
    }
}