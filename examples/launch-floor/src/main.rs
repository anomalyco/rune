//! The floor under an app's launch: one window and the least a first frame can hold, so
//! `perf/launch.bench.ts` can time what Rune itself costs from the process's start to that frame.
//!
//! `LAUNCH_FLOOR=bare` (the default) draws one line of text and one rectangle. `LAUNCH_FLOOR=shell`
//! draws what a desktop app's first screen is made of: a custom title bar, a sidebar of icon and
//! text rows, wrapped paragraphs, bordered cards and a composer with a shadow.
//!
//! A moment after the first frame it prints every `rune::marks` step as
//! `launch-mark <name> <µs since the process started> at=<epoch ms>`, then one JSON line
//! (`launch-floor {..}`) with the marks and the totals before the first frame. `LAUNCH_FLOOR_TRACE`
//! names a file that gets the JSON line appended too.

#![forbid(unsafe_code)]
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::io::Write;
use std::sync::OnceLock;
use std::time::Duration;

use rune::time::Instant;
use rune::tok::{self, Theme, radius, type_};
use rune::{AppSpec, Cx, El, Icon, col, el, icon, row, text};

static CHAT: Icon = Icon { name: "floor-chat", view: 256.0, body: r#"<path d="M216,48H40A16,16,0,0,0,24,64V224a15.84,15.84,0,0,0,9.25,14.5A16.05,16.05,0,0,0,40,240a15.89,15.89,0,0,0,10.25-3.78l.09-.07L83,208H216a16,16,0,0,0,16-16V64A16,16,0,0,0,216,48ZM216,192H80a8,8,0,0,0-5.23,1.95L40,224V64H216Z" fill="currentColor"/>"# };
static PLUS: Icon = Icon { name: "floor-plus", view: 256.0, body: r#"<path d="M224,128a8,8,0,0,1-8,8H136v80a8,8,0,0,1-16,0V136H40a8,8,0,0,1,0-16h80V40a8,8,0,0,1,16,0v80h80A8,8,0,0,1,224,128Z" fill="currentColor"/>"# };
static SEND: Icon = Icon { name: "floor-send", view: 256.0, body: r#"<path d="M205.66,117.66a8,8,0,0,1-11.32,0L136,59.31V216a8,8,0,0,1-16,0V59.31L61.66,117.66a8,8,0,0,1-11.32-11.32l72-72a8,8,0,0,1,11.32,0l72,72A8,8,0,0,1,205.66,117.66Z" fill="currentColor"/>"# };

const TITLEBAR_H: f32 = 40.0;

/// When `main` began, and how long the process had run by then.
static BOOT: OnceLock<(Instant, Duration, std::time::SystemTime)> = OnceLock::new();

struct State {
    shell: bool,
}

/// Whether `LAUNCH_FLOOR` asks for the desktop app's first screen.
fn shell_scene() -> bool {
    rune::env::var("LAUNCH_FLOOR").is_some_and(|v| v == "shell")
}

fn init() -> State {
    State { shell: shell_scene() }
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let t = tok::theme(true);
    if s.shell { shell(t) } else { bare(t) }
}

fn bare(t: Theme) -> El {
    col().w_full().h_full().center().gap(16.0).fill(t.bg)
        .child(text("The floor").key("floor").font(type_::TITLE).color(t.text))
        .child(el().key("rect").size(120.0, 40.0).fill(tok::accent(0, true)))
}

fn shell(t: Theme) -> El {
    let titlebar = row().key("titlebar").w_full().h(TITLEBAR_H).px(16.0).gap(12.0).center_y()
        .child(icon(&CHAT, 16.0).color(t.text_secondary))
        .child(text("Launch floor").font(type_::LABEL).color(t.text).grow(1.0))
        .child(icon(&PLUS, 16.0).color(t.text_secondary));
    let mut sidebar = col().key("sidebar").w(280.0).h_full().padding(8.0).gap(2.0).fill(t.card);
    for (i, name) in SESSIONS.iter().enumerate() {
        sidebar = sidebar.child(
            row().key(format!("session-{i}")).h(32.0).px(8.0).gap(8.0).center_y().radius(radius::SM)
                .fill(if i == 0 { t.hover } else { tok::theme(true).card })
                .child(icon(&CHAT, 14.0).color(t.text_secondary))
                .child(text(*name).font(type_::BODY).color(t.text).grow(1.0).truncate())
                .child(text(format!("{}m", 3 + i * 7)).font(type_::LABEL).color(t.text_secondary)),
        );
    }
    let mut timeline = col().key("timeline").grow(1.0).gap(12.0)
        .child(text("Where a launch's time goes").key("floor").font(type_::TITLE).color(t.text));
    for (i, body) in MESSAGES.iter().enumerate() {
        timeline = timeline.child(
            col().key(format!("message-{i}")).w_full().padding(12.0).gap(6.0).radius(radius::SM)
                .fill(t.card).border(1.0, t.card_border)
                .child(text(if i % 2 == 0 { "You" } else { "Assistant" }).font(type_::LABEL).color(t.text_secondary))
                .child(text(*body).font(type_::BODY).color(t.text).wrap()),
        );
    }
    let composer = row().key("composer").w_full().h(56.0).px(16.0).gap(12.0).center_y().radius(12.0)
        .fill(t.card).border(1.0, t.card_border).shadow(4.0, 16.0, t.shadow)
        .child(text("Ask anything").font(type_::BODY).color(t.text_secondary).grow(1.0))
        .child(row().size(32.0, 32.0).center().radius(16.0).fill(tok::accent(0, true)).child(icon(&SEND, 16.0).color(t.text)));
    let main = col().key("main").grow(1.0).h_full().padding(24.0).gap(16.0).child(timeline).child(composer);
    col().w_full().h_full().fill(t.bg)
        .child(titlebar)
        .child(row().w_full().grow(1.0).child(sidebar).child(main))
}

const SESSIONS: [&str; 14] = [
    "Measure the floor", "Cut the first frame's layout", "Fonts off the UI thread", "A cold start's pages",
    "Window shown with its frame", "The title bar's frame", "Shape the sidebar once", "Paint tiles in parallel",
    "The loader before main", "Keyboard layout beside the launch", "Accessibility after the frame", "Hand over to the GPU",
    "Input ready", "Content live",
];

const MESSAGES: [&str; 4] = [
    "How long does the window take to come up with its first frame, counted from the moment the process starts?",
    "From the process's start: the loader maps the program and its libraries, main runs, the event loop is built, the window is made hidden, the first view is built, laid out and shaped, the frame is painted and the window shows with it.",
    "Which of those steps belong to the app, and which to the library under it?",
    "The view's build and its state are the app's. The loop, the window, text shaping, layout, the painter and the show are the library's: this window measures those alone, so an app's own cost is what it adds above them.",
];

/// The marks so far as lines and one JSON line, on stdout and in `LAUNCH_FLOOR_TRACE`.
fn report() {
    let Some(&(main_at, age, wall)) = BOOT.get() else { return };
    let epoch_ms = |at: Instant| wall.checked_add(at.saturating_duration_since(main_at)).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis());
    let micros = |at: Instant| (age + at.saturating_duration_since(main_at)).as_micros();
    let marks = rune::marks::marks();
    let mut out = String::new();
    for (name, at) in &marks {
        out.push_str(&format!("launch-mark {name} {} at={}\n", micros(*at), epoch_ms(*at)));
    }
    let list = |items: Vec<String>| items.join(",");
    let json = format!(
        "{{\"scene\":\"{}\",\"marks\":{{{}}},\"metrics\":{{{}}},\"notes\":{{{}}}}}",
        if shell_scene() { "shell" } else { "bare" },
        list(marks.iter().map(|(name, at)| format!("\"{name}\":{}", micros(*at))).collect()),
        list(rune::marks::metrics().iter().map(|(name, total)| format!("\"{name}\":{total}")).collect()),
        list(rune::marks::notes().iter().map(|(key, value)| format!("\"{key}\":\"{value}\"")).collect()),
    );
    out.push_str(&format!("launch-floor {json}\n"));
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(out.as_bytes()).ok();
    stdout.flush().ok();
    if let Some(path) = rune::env::var_os("LAUNCH_FLOOR_TRACE")
        && let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path)
    {
        file.write_all(format!("{json}\n").as_bytes()).ok();
    }
}

fn main() {
    let age = rune::process_age().unwrap_or_default();
    BOOT.get_or_init(|| (rune::time::now(), age, rune::time::system_now()));
    rune::marks::mark("main.entered");
    let spec = AppSpec::new("Launch floor", (1280.0, 800.0), init, view);
    let shell = shell_scene();
    let app = rune::App::new(spec).on_ready(|_| {
        // Off the UI thread, once the loop has gone to wait for input.
        std::thread::spawn(|| {
            // LAUNCH_FLOOR_SETTLE_MS: how long after the first frame the report waits (300); a GPU
            // hand-over on a machine that has never compiled the pipelines needs about 2,500.
            let settle = rune::env::var("LAUNCH_FLOOR_SETTLE_MS").and_then(|v| v.parse().ok()).unwrap_or(300);
            std::thread::sleep(Duration::from_millis(settle));
            report();
        });
    });
    let app = if shell {
        app.titlebar(rune::Titlebar::new(TITLEBAR_H)).chrome(rune::Chrome { full_size_content: true, traffic_lights: Some((20.0, 20.0)), drag_regions: true })
    } else {
        app
    };
    app.run();
}
