//! An app that outlives its window: it starts with none, a background thread feeds it, and
//! commands on stdin open, focus, close and quit it. Closing the window keeps the app running;
//! reopening shows the same state (the ticks kept counting, the note kept its text).
//!
//! ```sh
//! cargo run -p background            # stdin: open | show | hide | focus | activate TOKEN | close | status | quit
//! cargo run -p background -- --open  # start with the window
//! cargo run -p background -- --release-gpu  # closing also drops the GPU device
//! cargo run -p background -- --open --release-gpu --tray --maximized  # start maximized, close to tray, reopen
//! cargo run -p background -- --open --release-gpu --tray --full-screen
//! cargo run -p background -- --accessory    # macOS: no Dock icon until `dock on`
//! cargo run -p background -- --accessory --tray  # menu-bar tray icon (macOS template image) + menu
//! # `hud show|hide|top|bottom|recording|transcribing|status` drives a recording-HUD overlay;
//! # `capsule show|hide|left|wide|status` a clickable corner capsule;
//! # `tray tooltip <text> | title [text] | icon normal|alert | icon-name [name] | rect` drives live tray updates
//! cargo run -p background -- --snap /tmp/background.png
//! ```

#![forbid(unsafe_code)]

use std::io::BufRead;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use rune::tok::{self, radius, space, type_};
use rune::{
    App, AppHandle, Availability, Cx, El, Key, KeyEvent, OverlayHandle, OverlayOptions, TextEdit,
    Tray, TrayEvent, TrayIcon, TrayItem, TraySpec, Tx, col, el, row, text, text_input,
};

struct Background {
    ticks: u64,
    closes: u32,
    note: TextEdit,
    hud: Option<OverlayHandle<Hud>>,
    hud_availability: Availability,
    hud_failure: Option<String>,
}

fn init() -> Background {
    Background {
        ticks: 0,
        closes: 0,
        note: TextEdit::new("Type here, close, reopen"),
        hud: None,
        hud_availability: Availability::Unknown,
        hud_failure: None,
    }
}

fn button(key: &'static str, label: &'static str, t: tok::Theme, f: impl Fn(&mut Background, &mut Tx) + 'static) -> El {
    row()
        .key(key)
        .role("button")
        .label(label)
        .h(28.0)
        .px(space::S3)
        .center()
        .radius(radius::MD)
        .fill(t.card)
        .border(1.0, t.card_border)
        .on_press_tx(f)
        .child(text(label).font(type_::LABEL).color(t.text))
}

fn view(s: &Background, cx: &mut Cx<Background>) -> El {
    let t = tok::theme(cx.system_dark());
    col()
        .key("window")
        .fill(t.bg)
        .px(space::S6)
        .py(space::S6)
        .gap(space::S3)
        .child(text("Background app").key("title").font(type_::TITLE).color(t.text))
        .child(
            text(format!(
                "{} ticks from a background thread, window closed {} times",
                s.ticks, s.closes
            ))
            .key("ticks")
            .font(type_::BODY)
            .color(t.text_secondary),
        )
        .child(
            text(match &s.hud_failure {
                Some(reason) => format!("HUD overlay: {:?} ({reason})", s.hud_availability),
                None => format!("HUD overlay: {:?}", s.hud_availability),
            })
            .key("hud-availability")
            .font(type_::BODY)
            .color(t.text_secondary),
        )
        .child(
            el().key("note-field")
                .w_full()
                .h(32.0)
                .px(space::S2)
                .radius(radius::MD)
                .fill(t.card)
                .border(1.0, t.card_border)
                .center_y()
                .child(
                    text_input(&s.note, type_::BODY)
                        .key("note")
                        .label("Note")
                        .color(t.text)
                        .on_edit(|s: &mut Background| &mut s.note),
                ),
        )
        .child(
            row()
                .key("actions")
                .gap(space::S2)
                .child(button("hud-show", "Show HUD", t, |s, _| {
                    if let Some(hud) = &s.hud { hud.show(); }
                }))
                .child(button("hud-hide", "Hide HUD", t, |s, _| {
                    if let Some(hud) = &s.hud { hud.hide(); }
                }))
                .child(button("close", "Close window", t, |_, tx| tx.close_window()))
                .child(button("quit", "Quit", t, |_, tx| tx.quit())),
        )
}

/// Cmd/Ctrl-W closes the window, Cmd/Ctrl-Q quits.
fn on_key(_: &mut Background, ev: &KeyEvent, tx: &mut Tx) -> bool {
    match ev.key {
        Key::Command('w') => tx.close_window(),
        Key::Command('q') => tx.quit(),
        _ => return false,
    }
    true
}

fn on_close(s: &mut Background, _: &mut Tx) {
    s.closes += 1;
    eprintln!("background: window closed; still running");
}

/// The overlay's own state: what a recording HUD shows.
struct Hud {
    seconds: u64,
    transcribing: bool,
    app: AppHandle<Background>,
    handle: Option<OverlayHandle<Hud>>,
}

impl Hud {
    fn new(app: AppHandle<Background>) -> Self {
        Self { seconds: 0, transcribing: false, app, handle: None }
    }
}

fn on_hud_availability(h: &mut Hud, availability: Availability, _: &mut Tx) {
    let failure = h.handle.as_ref().and_then(OverlayHandle::failure);
    match &failure {
        Some(reason) => eprintln!("background: hud availability -> {availability:?} ({reason})"),
        None => eprintln!("background: hud availability -> {availability:?}"),
    }
    h.app.update(move |s, _| {
        s.hud_availability = availability;
        s.hud_failure = failure;
    });
}

fn hud(h: &Hud, _: &mut Cx<Hud>) -> El {
    let label = if h.transcribing { "Transcribing".to_owned() } else { format!("Recording {}:{:02}", h.seconds / 60, h.seconds % 60) };
    row().key("hud").w_full().h_full().center().child(
        row()
            .key("pill")
            .h(36.0)
            .px(space::S3)
            .gap(space::S2)
            .center()
            .radius(18.0)
            .fill(rune::oklch(0.2, 0.0, 0.0))
            .border(1.0, rune::oklch(0.3, 0.0, 0.0))
            .child(el().key("dot").size(8.0, 8.0).radius(4.0).fill(if h.seconds.is_multiple_of(2) { rune::oklch(0.65, 0.2, 25.0) } else { rune::oklch(0.45, 0.12, 25.0) }))
            .child(text(label).key("label").font(type_::LABEL).color(rune::oklch(0.93, 0.0, 0.0))),
    )
}

fn capsule(presses: &u32, _: &mut Cx<u32>) -> El {
    let t = tok::theme(true);
    row().key("capsule").w_full().h_full().px(space::S2).gap(space::S2).center().radius(24.0).fill(t.card).border(1.0, t.card_border).child(
        text(format!("Pressed {presses} times")).key("presses").font(type_::LABEL).color(t.text),
    )
    .child(
        row().key("press").role("button").h(28.0).px(space::S3).center().radius(radius::MD).fill(t.control_off).hover(t.hover)
            .on_press(|n: &mut u32| {
                *n += 1;
                eprintln!("background: capsule pressed {n}");
            })
            .child(text("Press").font(type_::LABEL).color(t.text)),
    )
}

fn main() {
    let mut app = App::new(rune::AppSpec::<Background> {
        scenes: &[("ticked", |s| s.ticks = 42)],
        on_key: Some(on_key),
        ..rune::AppSpec::new("Background", (480.0, 420.0), init, view)
    })
    .keep_running()
    .on_close(on_close)
    .on_window(|window| {
        let native = window.with_raw_handle(|handle| format!("{:?}", handle.as_raw())).unwrap_or_default();
        let display = window.with_raw_display_handle(|handle| format!("{:?}", handle.as_raw())).unwrap_or_default();
        eprintln!(
            "background: native window {} on {}",
            native.split('(').next().unwrap_or_default(),
            display.split('(').next().unwrap_or_default(),
        );
    })
    .on_display_close(|| {
        eprintln!("background: display closing");
    });
    if !std::env::args().any(|a| a == "--open") {
        app = app.windowless();
    }
    if std::env::args().any(|a| a == "--accessory") {
        app = app.accessory();
    }
    if std::env::args().any(|a| a == "--release-gpu") {
        app = app.release_gpu_when_closed();
    }
    if std::env::args().any(|a| a == "--maximized") {
        app = app.maximized(true);
    }
    if std::env::args().any(|a| a == "--full-screen") {
        app = app.full_screen(true);
    }
    // `--hud-large` covers most of the screen, to check that clicks pass through it.
    let size = if std::env::args().any(|a| a == "--hud-large") { (1200.0, 700.0) } else { (240.0, 44.0) };
    let overlay = app.overlay(
        rune::OverlaySpec {
            name: "background-hud",
            size,
            edge: rune::ScreenEdge::Top,
            margin: 16.0,
            align: rune::Align::Center,
            inset: 0.0,
            interactive: false,
            view: hud,
        }
        .options(OverlayOptions::default())
        .on_availability(on_hud_availability),
        Hud::new(app.handle()),
    );
    let hud_for_cb = overlay.clone();
    overlay.update(move |h, _| h.handle = Some(hud_for_cb));
    let hud_for_window = overlay.clone();
    app.handle().update(move |s, _| s.hud = Some(hud_for_window));
    // A clickable capsule in the top-right corner, like a meeting offer: clicks reach it, focus
    // stays where it was.
    let capsule = app.overlay(
        rune::OverlaySpec {
            name: "background-capsule",
            size: (220.0, 48.0),
            edge: rune::ScreenEdge::Top,
            margin: 40.0,
            align: rune::Align::End,
            inset: 12.0,
            interactive: true,
            view: capsule,
        },
        0,
    );
    let ticker = app.handle();
    let hud_ticker = overlay.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(1));
            ticker.update(|s, _| s.ticks += 1);
            hud_ticker.update(|h, _| h.seconds += 1);
        }
    });
    let tray_slot: Arc<Mutex<Option<Tray>>> = Arc::new(Mutex::new(None));
    let hud_visible = Arc::new(AtomicBool::new(false));
    let alert_on = Arc::new(AtomicBool::new(false));
    let headless = std::env::args().any(|a| matches!(a.as_str(), "--snap" | "--tree" | "--script" | "--list-scenes" | "--list-windows" | "--trace" | "--strip" | "--video" | "--html" | "--desktop"));
    if std::env::args().any(|a| a == "--tray") && !headless {
        let tray_app = app.handle();
        let tray_hud = overlay.clone();
        let hud_for_cb = hud_visible.clone();
        let alert_for_cb = alert_on.clone();
        let tray_for_cb = tray_slot.clone();
        let spec = TraySpec::new("dev.rune.background", "Rune Background", tray_icon(false))
            .tooltip("Rune Background — click to open")
            .menu(tray_menu(false, false));
        match Tray::new(spec, move |event| match event {
            TrayEvent::Activate { token } => {
                if let Ok(guard) = tray_for_cb.lock() && let Some(tray) = guard.as_ref() {
                    eprintln!("background: tray activate rect {:?}", tray.rect());
                }
                if let Some(token) = token {
                    tray_app.activate(token);
                } else {
                    tray_app.open();
                }
            }
            TrayEvent::Menu(id) => match id.as_str() {
                "open" => tray_app.open(),
                "hud" => {
                    let next = !hud_for_cb.fetch_xor(true, Ordering::Relaxed);
                    if next { tray_hud.show(); } else { tray_hud.hide(); }
                    let alert = alert_for_cb.load(Ordering::Relaxed);
                    if let Ok(guard) = tray_for_cb.lock() && let Some(tray) = guard.as_ref() {
                        tray.set_menu(tray_menu(next, alert));
                        tray.set_tooltip(if next { "Rune Background — HUD visible" } else { "Rune Background — click to open" });
                    }
                }
                "alert" => {
                    let next = !alert_for_cb.fetch_xor(true, Ordering::Relaxed);
                    let hud = hud_for_cb.load(Ordering::Relaxed);
                    if let Ok(guard) = tray_for_cb.lock() && let Some(tray) = guard.as_ref() {
                        if let Err(err) = tray.set_icon(tray_icon(next)) {
                            eprintln!("background: set_icon failed: {err}");
                        }
                        tray.set_title(if next { "ALERT" } else { "" });
                        tray.set_menu(tray_menu(hud, next));
                    }
                }
                "quit" => tray_app.quit(),
                _ => {}
            },
            _ => {}
        }) {
            Ok(tray) => { *tray_slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(tray); }
            Err(err) => eprintln!("background: tray unavailable: {err}"),
        }
    }
    let commands = app.handle();
    let tray_cmd = tray_slot.clone();
    let hud_cmd = hud_visible.clone();
    let alert_cmd = alert_on.clone();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            let mut words = line.split_whitespace();
            match (words.next(), words.next()) {
                (Some("open"), _) => commands.open(),
                (Some("show"), _) => commands.show(),
                (Some("hide"), _) => commands.hide(),
                (Some("focus"), _) => commands.focus(),
                (Some("position"), Some(x)) => {
                    if let (Ok(x), Some(Ok(y))) = (x.parse::<i32>(), words.next().map(str::parse::<i32>)) {
                        commands.set_position((x, y));
                    }
                }
                (Some("activate"), Some(token)) => commands.activate(token),
                (Some("close"), _) => commands.close(),
                (Some("quit"), _) => commands.quit(),
                (Some("status"), _) => eprintln!("background: visible {}, window error {:?}", commands.is_visible(), commands.window_error()),
                (Some("dock"), Some(on)) => commands.set_dock_icon(on == "on"),
                (Some("hud"), Some("show")) => overlay.show(),
                (Some("hud"), Some("hide")) => overlay.hide(),
                (Some("hud"), Some("top")) => overlay.set_edge(rune::ScreenEdge::Top),
                (Some("hud"), Some("bottom")) => overlay.set_edge(rune::ScreenEdge::Bottom),
                (Some("hud"), Some("transcribing")) => overlay.update(|h, _| h.transcribing = true),
                (Some("hud"), Some("recording")) => overlay.update(|h, _| h.transcribing = false),
                (Some("hud"), Some("status")) => eprintln!("background: hud {:?}", overlay.availability()),
                (Some("capsule"), Some("show")) => capsule.show(),
                (Some("capsule"), Some("hide")) => capsule.hide(),
                (Some("capsule"), Some("left")) => capsule.set_align(rune::Align::Start, 12.0),
                (Some("capsule"), Some("wide")) => capsule.resize((320.0, 48.0)),
                (Some("capsule"), Some("status")) => eprintln!("background: capsule {:?}", capsule.availability()),
                (Some("tray"), Some("tooltip")) => {
                    let tip: Vec<&str> = words.collect();
                    if let Ok(guard) = tray_cmd.lock() && let Some(tray) = guard.as_ref() {
                        tray.set_tooltip(tip.join(" "));
                    }
                }
                (Some("tray"), Some("title")) => {
                    let title: Vec<&str> = words.collect();
                    if let Ok(guard) = tray_cmd.lock() && let Some(tray) = guard.as_ref() {
                        tray.set_title(title.join(" "));
                    }
                }
                (Some("tray"), Some("icon")) => {
                    let alert = words.next() == Some("alert");
                    alert_cmd.store(alert, Ordering::Relaxed);
                    let hud = hud_cmd.load(Ordering::Relaxed);
                    if let Ok(guard) = tray_cmd.lock() && let Some(tray) = guard.as_ref() {
                        if let Err(err) = tray.set_icon(tray_icon(alert)) {
                            eprintln!("background: set_icon failed: {err}");
                        }
                        tray.set_menu(tray_menu(hud, alert));
                    }
                }
                (Some("tray"), Some("icon-name")) => {
                    let name: Vec<&str> = words.collect();
                    if let Ok(guard) = tray_cmd.lock() && let Some(tray) = guard.as_ref() {
                        tray.set_icon_name(name.join(" "));
                    }
                }
                (Some("tray"), Some("rect")) => {
                    if let Ok(guard) = tray_cmd.lock() && let Some(tray) = guard.as_ref() {
                        eprintln!("background: tray rect {:?}", tray.rect());
                    }
                }
                (Some(other), _) => {
                    eprintln!("background: unknown command {other:?} (open | show | hide | focus | position X Y | activate TOKEN | close | dock on|off | hud show|hide|top|bottom|recording|transcribing|status | capsule show|hide|left|wide|status | tray tooltip|title|icon|icon-name|rect | quit)")
                }
                (None, _) => {}
            }
        }
    });
    app.run();
    drop(tray_slot.lock().unwrap_or_else(|e| e.into_inner()).take());
}

/// A 44×44 (2× for 22 pt) ring-and-dot glyph. Normal mode uses a macOS template image so macOS
/// tints it for light and dark menu bars; alert mode uses a coloured non-template glyph.
fn tray_icon(alert: bool) -> TrayIcon {
    const SIZE: u32 = 44;
    let center = (SIZE as f32 - 1.0) * 0.5;
    let rgb = if alert { [235, 87, 68] } else { [240, 240, 240] };
    let mut pixels = vec![0u8; (SIZE * SIZE * 4) as usize];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let dist = (dx * dx + dy * dy).sqrt();
            let ring = (1.5 - (dist - 12.5).abs()).clamp(0.0, 1.0);
            let dot = (4.5 - dist).clamp(0.0, 1.0);
            let alpha = (ring.max(dot) * 255.0).round() as u8;
            let idx = ((y * SIZE + x) * 4) as usize;
            pixels[idx..idx + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], alpha]);
        }
    }
    TrayIcon::rgba(SIZE, SIZE, pixels).template(!alert)
}

fn tray_menu(hud_visible: bool, alert: bool) -> Vec<TrayItem> {
    vec![
        TrayItem::action("open", "Open Window"),
        TrayItem::check("hud", "Show HUD", hud_visible),
        TrayItem::check("alert", "Alert icon", alert),
        TrayItem::Separator,
        TrayItem::action("quit", "Quit"),
    ]
}
