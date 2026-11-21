//! An app that outlives its window: it starts with none, a background thread feeds it, and
//! commands on stdin open, focus, close and quit it. Closing the window keeps the app running;
//! reopening shows the same state (the ticks kept counting, the note kept its text).
//!
//! ```sh
//! cargo run -p background            # stdin: open | show | hide | focus | activate TOKEN | close | status | quit
//! cargo run -p background -- --open  # start with the window
//! cargo run -p background -- --release-gpu  # closing also drops the GPU device
//! cargo run -p background -- --accessory    # macOS: no Dock icon until `dock on`
//! # `hud show|hide|top|bottom|recording|transcribing|status` drives a recording-HUD overlay;
//! # `capsule show|hide|left|wide|status` a clickable corner capsule
//! cargo run -p background -- --snap /tmp/background.png
//! ```

#![forbid(unsafe_code)]

use std::io::BufRead;
use std::time::Duration;

use rune::tok::{self, radius, space, type_};
use rune::{App, Cx, El, Key, KeyEvent, TextEdit, Tx, col, el, row, text, text_input};

struct Background {
    ticks: u64,
    closes: u32,
    note: TextEdit,
}

fn init() -> Background {
    Background {
        ticks: 0,
        closes: 0,
        note: TextEdit::new("Type here, close, reopen"),
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
                        .color(t.text)
                        .on_edit(|s: &mut Background| &mut s.note),
                ),
        )
        .child(
            row()
                .key("actions")
                .gap(space::S2)
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
#[derive(Default)]
struct Hud {
    seconds: u64,
    transcribing: bool,
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
        eprintln!("background: native window {}", native.split('(').next().unwrap_or_default());
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
        },
        Hud::default(),
    );
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
    let commands = app.handle();
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
                (Some(other), _) => {
                    eprintln!("background: unknown command {other:?} (open | show | hide | focus | position X Y | activate TOKEN | close | dock on|off | hud show|hide|top|bottom|recording|transcribing|status | capsule show|hide|left|wide|status | quit)")
                }
                (None, _) => {}
            }
        }
    });
    app.run();
}
