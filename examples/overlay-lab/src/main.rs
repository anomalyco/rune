use std::sync::{Arc, Mutex};
use rune::{App, AppSpec, Color, Cx, DisplayTarget, El, Key, KeyEvent, KeyPolicy, OverlayHandle, OverlayInput, OverlayLevel, OverlayOptions, OverlayPlacement, OverlaySpec, Phase, RunOptions, Tx, WindowSpec, col, el, row, text};

const BLUE: Color = Color { r: 0.25, g: 0.55, b: 1.0, a: 1.0 };
const DARK: Color = Color { r: 0.055, g: 0.065, b: 0.085, a: 0.97 };

#[derive(Default)]
struct Handles { pill: Option<OverlayHandle<State>>, pickers: Vec<OverlayHandle<State>> }
struct State {
    underlay: bool,
    clicks: u32,
    picker: bool,
    expanded: bool,
    hovered: String,
    selection: Option<[f32; 4]>,
    pointer: rune::Live<[f32; 2]>,
    handles: Arc<Mutex<Handles>>,
}
fn init() -> State {
    State { underlay: false, clicks: 0, picker: false, expanded: false, hovered: String::new(), selection: None, pointer: rune::Live::new([160.0, 100.0]), handles: Arc::default() }
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
fn chip(label: &'static str, action: fn(&mut State)) -> El {
    el().key(label).hit().padding(10.0).radius(16.0).fill(BLUE.alpha(0.16))
        .child(text(label).color(Color::WHITE))
        .on_hover(move |s: &mut State| { s.hovered = label.into(); })
        .on_press(move |s: &mut State| { eprintln!("overlay-lab chip {label}"); action(s); })
}
fn view(s: &State, _: &mut Cx<State>) -> El {
    if s.underlay {
        return el().w_full().h_full().fill(DARK).padding(40.0)
            .child(text(format!("Underlay clicks: {}", s.clicks)).color(Color::WHITE))
            .on_press(|s: &mut State| { s.clicks += 1; eprintln!("overlay-lab underlay click {}", s.clicks); });
    }
    if s.picker {
        let mut root = el().w_full().h_full().hit().fill(BLUE.alpha(0.06))
            .on_pointer_move(|s: &mut State, p| { s.pointer.set([p.x, p.y]); })
            .on_drag(|s: &mut State, drag, tx| {
                tx.instant();
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
    let mut chips = row().key("pill").padding(8.0).gap(4.0).radius(28.0).fill(DARK)
        .child(chip("Area", show_picker)).child(chip("Window", show_picker))
        .child(chip("Screen", show_picker)).child(chip("iPhone", |s| s.expanded = !s.expanded));
    if s.expanded { chips = chips.child(chip("Route", |s| s.expanded = false)); }
    chips = chips.child(chip("Paste", |s| s.expanded = !s.expanded))
        .child(chip("×", |s| { if let Some(pill) = &s.handles.lock().unwrap().pill { pill.hide(); } }));
    col().w_full().h_full().center().gap(8.0).child(chips)
        .child(text(&s.hovered).color(BLUE))
}
fn spec() -> AppSpec<State> {
    AppSpec { title: "Overlay lab", size: (680.0, 160.0), min_size: None, init, view,
        scenes: &[("pill", |_| {}), ("expanded", |s| s.expanded = true), ("picker", |s| { s.picker = true; s.selection = Some([140.0, 100.0, 560.0, 320.0]); })],
        fonts: &[], families: ("sans-serif", "monospace"), setup: None, poll: None, on_key: Some(key) }
}
fn main() {
    if std::env::var_os("OVERLAY_LAB_UNDERLAY").is_some() {
        rune::run_windows(|displays| {
            let mut state = init(); state.underlay = true;
            let app = App::new(spec()).init(move || state);
            if let Ok(seconds) = std::env::var("OVERLAY_LAB_SECONDS").unwrap_or_default().parse::<u64>() {
                let handle = app.handle();
                std::thread::spawn(move || { std::thread::sleep(std::time::Duration::from_secs(seconds)); handle.quit(); });
            }
            let options = displays.iter().find(|d| d.primary).or_else(|| displays.first()).map(|display| rune::WindowOptions {
                borderless: true, position: Some(display.position), physical_size: Some(display.size), ..Default::default()
            }).unwrap_or_default();
            vec![WindowSpec::from_app("underlay", app).options(options)]
        });
        return;
    }
    rune::run_windows_with(RunOptions { accessory: true }, |displays| {
        let exclude = std::env::var_os("OVERLAY_LAB_INCLUDE_IN_CAPTURE").is_none();
        let handles = Arc::new(Mutex::new(Handles::default()));
        let mut app = App::new(spec()).windowless();
        for display in displays {
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
        let pill = app.overlay(OverlaySpec { margin: 40.0, ..OverlaySpec::new("Rune pill", (680.0, 160.0), view) }.options(OverlayOptions {
            display: DisplayTarget::UnderPointer, follow_pointer: true,
            input: OverlayInput::Regions, exclude_from_capture: exclude, level: OverlayLevel::AboveOverlays, prewarm: true, ..Default::default()
        }).on_created(|created| eprintln!("overlay-lab pill created {created:?}")), state);
        handles.lock().unwrap().pill = Some(pill.clone());
        pill.show();
        if std::env::var_os("OVERLAY_LAB_PICKER").is_some() {
            let mut state = init(); state.handles = handles; show_picker(&mut state);
        }
        if let Ok(seconds) = std::env::var("OVERLAY_LAB_SECONDS").unwrap_or_default().parse::<u64>() {
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
