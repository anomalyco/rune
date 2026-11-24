//! Multi-display overlay lab: a pointer-following pill with regional hit testing and
//! full-screen area pickers, with optional capture exclusion.

use std::sync::{Arc, Mutex};
use rune::{Align, App, AppSpec, Color, Cx, Display, DisplayTarget, El, Key, KeyEvent, KeyPolicy, OverlayHandle, OverlayInput, OverlayLevel, OverlayOptions, OverlayPlacement, OverlaySpec, Phase, RunOptions, ScreenEdge, Tx, WindowHandle, WindowSpec, WindowsWindowOptions, col, el, row, text};

const BLUE: Color = Color { r: 0.25, g: 0.55, b: 1.0, a: 1.0 };
const DARK: Color = Color { r: 0.055, g: 0.065, b: 0.085, a: 0.97 };

const EDGE_SLOTS: [(&str, ScreenEdge, Align, f32, f32); 6] = [
    ("Top · Center", ScreenEdge::Top, Align::Center, 40.0, 0.0),
    ("Top · Start", ScreenEdge::Top, Align::Start, 16.0, 16.0),
    ("Top · End", ScreenEdge::Top, Align::End, 16.0, 16.0),
    ("Bottom · Start", ScreenEdge::Bottom, Align::Start, 16.0, 16.0),
    ("Bottom · Center", ScreenEdge::Bottom, Align::Center, 16.0, 0.0),
    ("Bottom · End", ScreenEdge::Bottom, Align::End, 16.0, 16.0),
];

#[derive(Default)]
struct Handles {
    pill: Option<OverlayHandle<State>>,
    pickers: Vec<OverlayHandle<State>>,
    window: Option<WindowHandle>,
}
struct State {
    underlay: bool,
    native_window: bool,
    exclude_from_capture: bool,
    undecorated_shadow: bool,
    no_activate: bool,
    tool_window: bool,
    hit_only: bool,
    clicks: u32,
    picker: bool,
    expanded: bool,
    edge_slot: usize,
    hovered: String,
    work_area: String,
    selection: Option<[f32; 4]>,
    selection_at_press: Option<[f32; 4]>,
    pointer: rune::Live<[f32; 2]>,
    handles: Arc<Mutex<Handles>>,
}
fn init() -> State {
    State {
        underlay: false, native_window: false, exclude_from_capture: true, undecorated_shadow: true,
        no_activate: false, tool_window: false, hit_only: false, clicks: 0, picker: false, expanded: false,
        edge_slot: 0, hovered: String::new(), work_area: String::new(), selection: None, selection_at_press: None,
        pointer: rune::Live::new([160.0, 100.0]), handles: Arc::default(),
    }
}
fn toggle_capture_exclusion(s: &mut State) {
    s.exclude_from_capture = !s.exclude_from_capture;
    if let Some(window) = &s.handles.lock().unwrap().window {
        window.set_exclude_from_capture(s.exclude_from_capture);
    }
    eprintln!("overlay-lab window exclude_from_capture={}", s.exclude_from_capture);
}
fn format_work_area(display: &Display) -> String {
    let ((wx, wy), (ww, wh)) = display.work_area();
    let ((dx, dy), (dw, dh)) = (display.position, display.size);
    format!("work_area ({wx},{wy}) {ww}×{wh} / display ({dx},{dy}) {dw}×{dh}")
}
fn cycle_edge(s: &mut State) {
    s.edge_slot = (s.edge_slot + 1) % EDGE_SLOTS.len();
    let (label, edge, align, margin, inset) = EDGE_SLOTS[s.edge_slot];
    if let Some(pill) = &s.handles.lock().unwrap().pill {
        pill.set_placement(Some(OverlayPlacement::Edge { edge, margin, align, inset }));
    }
    s.hovered = if s.work_area.is_empty() { label.into() } else { format!("{label} · {}", s.work_area) };
}
fn show_picker(s: &mut State) {
    let handles = s.handles.lock().unwrap();
    for picker in &handles.pickers { picker.show(); }
    // One key owner, even with several full-display surfaces.
    if let Some(picker) = handles.pickers.first() { picker.request_keys(); }
    eprintln!("overlay-lab picker shown");
}
fn hide_picker(s: &mut State) {
    for picker in &s.handles.lock().unwrap().pickers { picker.hide(); }
    eprintln!("overlay-lab picker hidden");
}
fn key(s: &mut State, event: &KeyEvent, _: &mut Tx) -> bool {
    eprintln!("overlay-lab key {event:?}");
    if event.key == Key::Escape { hide_picker(s); return true; }
    false
}
fn drag_handle() -> El {
    let dot = || el().size(3.0, 3.0).radius(1.5).fill(Color::WHITE.alpha(0.55));
    row().key("drag-handle").label("Drag overlay").hit().window_drag().cursor(rune::Cursor::Grab)
        .padding(8.0).gap(3.0).radius(14.0).center()
        .child(col().gap(3.0).child(dot()).child(dot()).child(dot()))
        .child(col().gap(3.0).child(dot()).child(dot()).child(dot()))
}
fn chip(label: &'static str, action: fn(&mut State)) -> El {
    el().key(label).hit().padding(10.0).radius(16.0).fill(BLUE.alpha(0.16))
        .child(text(label).color(Color::WHITE))
        .on_hover(move |s: &mut State| { s.hovered = label.into(); })
        .on_press(move |s: &mut State| { eprintln!("overlay-lab chip {label}"); action(s); })
}
fn view(s: &State, _: &mut Cx<State>) -> El {
    if s.native_window {
        let capture_label = if s.exclude_from_capture { "Capture: Excluded" } else { "Capture: Visible" };
        let flags = format!(
            "borderless · undecorated_shadow={} · no_activate={} · tool_window={}",
            s.undecorated_shadow, s.no_activate, s.tool_window
        );
        return col().w_full().h_full().fill(DARK).padding(20.0).gap(12.0)
            .child(row().w_full().gap(8.0).items_center()
                .child(el().key("window-drag").grow(1.0).window_drag()
                    .child(text("Native window · WindowsWindowOptions + exclude_from_capture").color(Color::WHITE)))
                .child(el().key("close-window").hit().padding(6.0).radius(8.0).fill(BLUE.alpha(0.16))
                    .label("Close window")
                    .child(text("×").color(Color::WHITE))
                    .on_press_tx(|_: &mut State, tx| tx.quit())))
            .child(text(flags).color(BLUE))
            .child(row().gap(8.0)
                .child(el().key("toggle-capture").hit().padding(10.0).radius(12.0).fill(BLUE.alpha(0.22))
                    .label(capture_label)
                    .child(text(capture_label).color(Color::WHITE))
                    .on_press(toggle_capture_exclusion)));
    }
    if s.underlay {
        return el().w_full().h_full().fill(DARK).padding(40.0)
            .child(text(format!("Underlay clicks: {}", s.clicks)).color(Color::WHITE))
            .on_press(|s: &mut State| { s.clicks += 1; eprintln!("overlay-lab underlay click {}", s.clicks); });
    }
    if s.picker {
        let mut root = el().w_full().h_full().hit().fill(BLUE.alpha(0.06))
            .on_pointer_move_local(|s: &mut State, p, tx| {
                s.pointer.set([p.window_x, p.window_y]);
                tx.live_only();
            })
            .on_drag(|s: &mut State, drag, tx| {
                tx.instant();
                if drag.phase == Phase::Down { s.selection_at_press = s.selection; }
                if drag.cancelled() { s.selection = s.selection_at_press.take(); return; }
                let (x, y) = (drag.x.min(drag.start_x), drag.y.min(drag.start_y));
                s.selection = Some([x, y, (drag.x - drag.start_x).abs(), (drag.y - drag.start_y).abs()]);
                if drag.phase == Phase::Up { eprintln!("overlay-lab selection {:?}", s.selection); }
            })
            .child(text("Drag an area · Escape to close").color(Color::WHITE).pin(Some(24.0), Some(24.0), None, None))
            .child(el().size(8.0, 8.0).radius(4.0).fill(BLUE).pin(Some(0.0), Some(0.0), None, None).live_offset(&s.pointer));
        if let Some([x, y, w, h]) = s.selection {
            root = root.child(el().key("selection").size(w, h).border(2.0, BLUE).fill(BLUE.alpha(0.12)).pin(Some(x), Some(y), None, None));
        }
        return root;
    }
    if s.hit_only {
        return col().w_full().h_full().center().child(el().key("hit-only").size(300.0, 48.0).hit()
            .fill(DARK).child(text("Native hit region").color(Color::WHITE)));
    }
    let mut chips = row().key("pill").padding(8.0).gap(4.0).radius(28.0).fill(DARK)
        .child(drag_handle())
        .child(chip("Area", show_picker)).child(chip("Window", show_picker))
        .child(chip("Screen", show_picker)).child(chip("iPhone", |s| s.expanded = !s.expanded));
    if s.expanded { chips = chips.child(chip("Route", |s| s.expanded = false)); }
    chips = chips.child(chip("Paste", |s| s.expanded = !s.expanded))
        .child(chip("Edge", cycle_edge))
        .child(chip("×", |s| { if let Some(pill) = &s.handles.lock().unwrap().pill { pill.hide(); } }));
    let status = if !s.hovered.is_empty() { s.hovered.as_str() } else { s.work_area.as_str() };
    col().w_full().h_full().center().gap(8.0).child(chips)
        .child(text(status).color(BLUE))
}
fn spec() -> AppSpec<State> {
    AppSpec { title: "Overlay lab", size: (680.0, 160.0), min_size: None, init, view,
        scenes: &[
            ("pill", |_| {}),
            ("expanded", |s| s.expanded = true),
            ("work-area", |s| {
                s.edge_slot = 5;
                s.work_area = "Bottom · End · work_area (0,0) 1920×1032 / display (0,0) 1920×1080".into();
            }),
            ("picker", |s| { s.picker = true; s.selection = Some([140.0, 100.0, 560.0, 320.0]); }),
            ("native-window", |s| { s.native_window = true; s.exclude_from_capture = true; s.undecorated_shadow = true; }),
        ],
        fonts: &[], families: ("sans-serif", "monospace"), setup: None, poll: None, on_key: Some(key) }
}

/// Optional local control socket for bounded native lifecycle/presentation probes.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn native_probe(pill: OverlayHandle<State>, app: rune::AppHandle<State>) {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    #[link(name = "QuartzCore", kind = "framework")]
    unsafe extern "C" { fn CACurrentMediaTime() -> f64; }
    let Some(path) = std::env::var_os("OVERLAY_LAB_CONTROL") else { return };
    let listener = UnixListener::bind(&path).expect("unique overlay probe socket");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut command = String::new();
            if BufReader::new(&stream).read_line(&mut command).is_err() { continue; }
            // SAFETY: `CACurrentMediaTime` takes no arguments and is safe to call from any thread.
            let at = unsafe { CACurrentMediaTime() };
            match command.trim() {
                "show" => pill.show(),
                "hide" => pill.hide(),
                "none" => pill.set_input(OverlayInput::None),
                "all" => pill.set_input(OverlayInput::All),
                "regions" => pill.set_input(OverlayInput::Regions),
                "hit-only" => pill.update(|s, _| s.hit_only = true),
                "chips" => pill.update(|s, _| s.hit_only = false),
                "status" => {},
                "quit" => app.quit(),
                _ => { writeln!(stream, "unknown command").ok(); continue; }
            }
            let line = format!("probe command={} time={at:.9} window={:?}", command.trim(), pill.window_number());
            eprintln!("{line}");
            writeln!(stream, "{line}").ok();
        }
    });
}
fn main() {
    if std::env::var_os("OVERLAY_LAB_UNDERLAY").is_some() {
        rune::run_windows(|displays| {
            let mut state = init(); state.underlay = true;
            let app = App::new(spec()).init(move || state);
            if let Ok(seconds) = rune::env::var("OVERLAY_LAB_SECONDS").unwrap_or_default().parse::<u64>() {
                let handle = app.handle();
                std::thread::spawn(move || { std::thread::sleep(std::time::Duration::from_secs(seconds)); handle.quit(); });
            }
            let options = displays.iter().find(|d| d.primary).or_else(|| displays.first()).map(|display| rune::WindowOptions {
                borderless: true, position: Some(display.position), physical_size: Some(display.size), ..Default::default()
            }).unwrap_or_default();
            let native = std::env::var_os("OVERLAY_LAB_NATIVE_WINDOW").is_some();
            let options = if native {
                options
                    .macos(rune::MacWindowOptions::default().shadow(false).level(24).collection_behavior(1 | 16 | 64 | 256))
                    .windows(|w| w.undecorated_shadow(true))
            } else { options.into() };
            let mut window = WindowSpec::from_app("underlay", app).configured_options(options);
            if native { window = window.exclude_from_capture(true); }
            vec![window]
        });
        return;
    }
    if std::env::var_os("OVERLAY_LAB_NATIVE_WINDOW").is_some() {
        rune::run_windows(|_| {
            let exclude = std::env::var_os("OVERLAY_LAB_INCLUDE_IN_CAPTURE").is_none();
            let no_activate = std::env::var_os("OVERLAY_LAB_NO_ACTIVATE").is_some();
            let tool_window = std::env::var_os("OVERLAY_LAB_TOOL_WINDOW").is_some();
            let handles = Arc::new(Mutex::new(Handles::default()));
            let mut state = init();
            state.native_window = true;
            state.exclude_from_capture = exclude;
            state.undecorated_shadow = true;
            state.no_activate = no_activate;
            state.tool_window = tool_window;
            state.handles = handles.clone();
            let app = App::new(spec()).init(move || state);
            if let Ok(seconds) = rune::env::var("OVERLAY_LAB_SECONDS").unwrap_or_default().parse::<u64>() {
                let handle = app.handle();
                std::thread::spawn(move || { std::thread::sleep(std::time::Duration::from_secs(seconds)); handle.quit(); });
            }
            let options = rune::WindowOptions { borderless: true, always_on_top: true, ..Default::default() }
                .windows(WindowsWindowOptions::default().undecorated_shadow(true).no_activate(no_activate).tool_window(tool_window));
            vec![WindowSpec::from_app("native-window", app)
                .configured_options(options)
                .exclude_from_capture(exclude)
                .on_created(move |handle| { handles.lock().unwrap().window = Some(handle); })]
        });
        return;
    }
    rune::run_windows_with(RunOptions { accessory: true }, |displays| {
        let exclude = std::env::var_os("OVERLAY_LAB_INCLUDE_IN_CAPTURE").is_none();
        let handles = Arc::new(Mutex::new(Handles::default()));
        let mut app = App::new(spec()).windowless();
        for display in displays {
            eprintln!("overlay-lab display {:?} {} scale={}", display.name, format_work_area(display), display.scale);
            let mut state = init();
            state.picker = true;
            state.handles = handles.clone();
            let target = display.id.clone().map_or(DisplayTarget::Primary, DisplayTarget::Id);
            let picker = app.overlay(OverlaySpec::new("Rune picker", (680.0, 160.0), view).options(OverlayOptions {
                display: target, placement: Some(OverlayPlacement::FullScreen), level: OverlayLevel::AboveStatus,
                input: OverlayInput::All, exclude_from_capture: exclude, keys: KeyPolicy::OnDemand, prewarm: true, ..Default::default()
            }).on_key(key).on_created(|created| eprintln!("overlay-lab picker created {created:?}")), state);
            handles.lock().unwrap().pickers.push(picker);
        }
        let mut state = init();
        state.handles = handles.clone();
        state.work_area = displays.iter().find(|d| d.primary).or_else(|| displays.first()).map(format_work_area).unwrap_or_default();
        let pill = app.overlay(OverlaySpec { margin: 40.0, ..OverlaySpec::new("Rune pill", (680.0, 160.0), view) }.options(OverlayOptions {
            display: DisplayTarget::UnderPointer, follow_pointer: true,
            input: OverlayInput::Regions, exclude_from_capture: exclude, level: OverlayLevel::AboveOverlays, prewarm: true, ..Default::default()
        }).on_created(|created| eprintln!("overlay-lab pill created {created:?}")), state);
        handles.lock().unwrap().pill = Some(pill.clone());
        #[cfg(target_os = "macos")]
        native_probe(pill.clone(), app.handle());
        if !cfg!(target_os = "macos") || std::env::var_os("OVERLAY_LAB_CONTROL").is_none() { pill.show(); }
        if std::env::var_os("OVERLAY_LAB_PICKER").is_some() {
            let mut state = init(); state.handles = handles; show_picker(&mut state);
        }
        if let Ok(seconds) = rune::env::var("OVERLAY_LAB_SECONDS").unwrap_or_default().parse::<u64>() {
            let app_handle = app.handle();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(750));
                eprintln!("overlay-lab window={:?} capture={:?}", pill.window_number(), pill.capture_exclusion());
                std::thread::sleep(std::time::Duration::from_secs(seconds));
                app_handle.quit();
            });
        }
        vec![WindowSpec::from_app("lab", app).options(rune::WindowOptions { transparent: true, ..Default::default() })]
    });
}
