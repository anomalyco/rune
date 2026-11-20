//! An overlay with a repeating 100 ms timer, for measuring idle cost while the panel is
//! covered or the displays sleep. Prints its timer count once a second.
//!
//! `OCCLUDED_TIMER_SECONDS=n` quits after n seconds; `OCCLUDED_TIMER_PREWARM=1` prewarms the
//! panel; `OCCLUDED_TIMER_REGIONS=1` takes pointer input over its pill only;
//! `OCCLUDED_TIMER_BOTTOM=1` puts it at the bottom edge; `OCCLUDED_TIMER_WINDOW=1` runs the
//! timer in a regular window instead. `OCCLUDED_TIMER_UPDATE=1` replaces the repeating timer
//! with one 100 ms `wake_after` that an update from another thread sets each second, and also
//! prints the latest that timer fired.

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use rune::{App, AppSpec, Color, Cx, El, OverlayInput, OverlayOptions, OverlaySpec, RunOptions, ScreenEdge, Tx, WindowOptions, WindowSpec, el, text};

const PERIOD: f64 = 0.1;
const ARMED: Duration = Duration::from_millis(100);
static FIRED: AtomicUsize = AtomicUsize::new(0);
static WORST_MS: AtomicUsize = AtomicUsize::new(0);

#[derive(Default)]
struct State { next: f64, armed: Option<Instant> }

fn view(_: &State, _: &mut Cx<State>) -> El {
    el().key("pill").hit().padding(12.0).radius(16.0).fill(Color { r: 0.1, g: 0.12, b: 0.16, a: 0.95 })
        .child(text(format!("fired {}", FIRED.load(Ordering::Relaxed))).color(Color::WHITE))
}

fn poll(s: &mut State, t: f64, tx: &mut Tx) -> bool {
    if updates() { return poll_armed(s); }
    let fired = t >= s.next;
    if fired {
        FIRED.fetch_add(1, Ordering::Relaxed);
        s.next = t + PERIOD;
    }
    tx.wake_after(Duration::from_secs_f64((s.next - t).max(0.0)));
    fired
}

/// Counts the update's timer once it is due, and how late it ran.
fn poll_armed(s: &mut State) -> bool {
    let Some(at) = s.armed.filter(|at| at.elapsed() >= ARMED) else { return false };
    s.armed = None;
    FIRED.fetch_add(1, Ordering::Relaxed);
    WORST_MS.fetch_max(at.elapsed().as_millis() as usize, Ordering::Relaxed);
    true
}

fn arm(s: &mut State, tx: &mut Tx) {
    s.armed = Some(rune::time::now());
    tx.wake_after(ARMED);
}

fn updates() -> bool { std::env::var_os("OCCLUDED_TIMER_UPDATE").is_some() }

fn main() {
    rune::run_windows_with(RunOptions { accessory: true }, |_| {
        let window = std::env::var_os("OCCLUDED_TIMER_WINDOW").is_some();
        let spec = AppSpec { poll: window.then_some(poll as fn(&mut State, f64, &mut Tx) -> bool), ..AppSpec::new("Occluded timer", (420.0, 400.0), State::default, view) };
        let mut app = App::new(spec);
        if window {
            let handle = app.handle();
            report(app.handle(), move || handle.update(arm));
            // Side by side, so neither covers the other.
            let x = if std::env::var_os("OCCLUDED_TIMER_BOTTOM").is_some() { 900 } else { 100 };
            return vec![WindowSpec::from_app("occluded-timer", app).options(WindowOptions { position: Some((x, 200)), ..Default::default() })];
        }
        app = app.windowless();
        let input = if std::env::var_os("OCCLUDED_TIMER_REGIONS").is_some() { OverlayInput::Regions } else { OverlayInput::All };
        let edge = if std::env::var_os("OCCLUDED_TIMER_BOTTOM").is_some() { ScreenEdge::Bottom } else { ScreenEdge::Top };
        let overlay = app.overlay(OverlaySpec { edge, ..OverlaySpec::new("Occluded timer", (220.0, 60.0), view) }.options(OverlayOptions {
            input, prewarm: std::env::var_os("OCCLUDED_TIMER_PREWARM").is_some(), ..Default::default()
        }).poll(poll), State::default());
        overlay.show();
        report(app.handle(), move || overlay.update(arm));
        vec![WindowSpec::from_app("occluded-timer", app)]
    });
}

fn report(handle: rune::AppHandle<State>, arm: impl Fn() + Send + 'static) {
    let quit = rune::env::var("OCCLUDED_TIMER_SECONDS").and_then(|s| s.parse::<u64>().ok());
    std::thread::spawn(move || {
        let start = rune::time::now();
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let elapsed = start.elapsed().as_secs();
            eprintln!("occluded-timer t={elapsed} fired={} worst_ms={}", FIRED.load(Ordering::Relaxed), WORST_MS.load(Ordering::Relaxed));
            if quit.is_some_and(|q| elapsed >= q) { handle.quit(); return; }
            if updates() { arm(); }
        }
    });
}
