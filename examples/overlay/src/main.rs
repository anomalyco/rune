#![forbid(unsafe_code)]

use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use rune::{AppSpec, Color, Cx, El, Icon, Live, Tx, WindowHandle, WindowOptions, WindowSpec, col, el, icon, text};

static WINDOWED: AtomicBool = AtomicBool::new(false);
static STROKE: Icon = Icon {
    name: "shared-stroke", view: 240.0,
    body: r#"<path d="M 12 190 C 55 30 90 210 130 70 S 190 170 228 35" fill="none" stroke="currentColor" stroke-width="4" stroke-linecap="round"/>"#,
};
struct State {
    overlay: bool,
    cursor: Live<[f32; 2]>,
    overlays: Arc<Mutex<Vec<WindowHandle>>>,
}
impl Drop for State {
    fn drop(&mut self) {
        if !self.overlay {
            for window in self.overlays.lock().unwrap().iter() { window.close(); }
        }
    }
}
fn init() -> State {
    State { overlay: false, cursor: Live::new([120.0, 90.0]), overlays: Arc::new(Mutex::new(Vec::new())) }
}
fn view(s: &State, _: &mut Cx<State>) -> El {
    let green = Color { r: 0.1, g: 0.85, b: 0.45, a: 1.0 };
    if s.overlay {
        return el().w_full().h_full().child(icon(&STROKE, 240.0).key("stroke").color(green)
            .pin(Some(80.0), Some(80.0), None, None))
            .child(el().key("remote-cursor").size(12.0, 18.0).radius(3.0).fill(green)
                .pin(Some(0.0), Some(0.0), None, None).live_offset(&s.cursor))
            .on_press(|s: &mut State| {
                for window in s.overlays.lock().unwrap().iter() { window.set_click_through(true).ok(); }
            });
    }
    col().w_full().h_full().padding(40.0).gap(20.0)
        .fill(Color { r: 0.018, g: 0.024, b: 0.03, a: 1.0 })
        .child(text("Display overlays").font(rune::tok::type_::BODY))
        .child(el().padding(16.0).radius(8.0).fill(green.alpha(0.15))
            .child(text("Test overlay input"))
            .on_press(|s: &mut State| {
                for window in s.overlays.lock().unwrap().iter() {
                    if let Err(e) = window.set_click_through(false) { eprintln!("click-through: {e}"); }
                }
            }))
}
fn point(t: f64) -> [f32; 2] {
    let t = t.min(4.0) as f32;
    [120.0 + 120.0 * (t * 0.7).sin(), 90.0 + 50.0 * t.sin()]
}
fn poll(s: &mut State, t: f64, _: &mut Tx) -> bool {
    if !WINDOWED.load(Ordering::Relaxed) { s.cursor.set(point(t)); }
    false
}
fn setup(s: &mut State, _: rune::Waker) {
    if !WINDOWED.load(Ordering::Relaxed) || s.overlay { return; }
    let cursor = s.cursor.clone();
    std::thread::spawn(move || {
        let start = rune::time::now();
        for tick in 1..=480 {
            let t = tick as f64 / 120.0;
            std::thread::sleep((start + std::time::Duration::from_secs_f64(t)).saturating_duration_since(rune::time::now()));
            cursor.set(point(t));
        }
    });
}
fn spec() -> AppSpec<State> {
    AppSpec { setup: Some(setup), poll: Some(poll), ..AppSpec::new("Rooms overlay example", (560.0, 440.0), init, view) }
}
fn main() {
    WINDOWED.store(!std::env::args().any(|a| matches!(a.as_str(), "--snap" | "--trace" | "--tree" | "--frames" | "--strip" | "--first-pixels" | "--list-scenes" | "--list-windows")), Ordering::Relaxed);
    rune::run_windows(|displays| {
        let controls = init();
        let cursor = controls.cursor.clone();
        let overlays = controls.overlays.clone();
        let mut windows = vec![WindowSpec::new("controls", spec()).state(controls)];
        for (i, display) in displays.iter().enumerate() {
            let state = State { overlay: true, cursor: cursor.clone(), overlays: overlays.clone() };
            let handles = overlays.clone();
            windows.push(WindowSpec::new(format!("overlay-{i}"), spec()).state(state)
                .options(WindowOptions::overlay(display)).on_created(move |window| {
                    // ScreenCaptureKit: exclude this CGWindowID using SCContentFilter.
                    eprintln!("overlay native window number: {:?}", window.window_number());
                    handles.lock().unwrap().push(window);
                }));
        }
        windows
    });
}
