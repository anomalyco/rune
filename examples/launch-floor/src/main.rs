//! The floor under an app's launch: one window and the least a first frame can hold, so
//! `perf/launch.bench.ts` can time what Rune itself costs from the process's start to that frame.
//!
//! `LAUNCH_FLOOR=bare` (the default) draws one line of text and one rectangle. `LAUNCH_FLOOR=shell`
//! draws what a desktop app's first screen is made of: a custom title bar, a sidebar of icon and
//! text rows, wrapped paragraphs, bordered cards and a composer with a shadow. `LAUNCH_FLOOR=reads` is
//! that screen with a view that reads three rects from layout, as a desktop app's does: with
//! `LAUNCH_FLOOR_SEED=<file>` it keeps what it read in the file and gives it to the next launch
//! (`App::layout_seed`), whose first frame then builds the view once. `LAUNCH_FLOOR_QUIT_MS=<n>`
//! quits the app that long after its report, as a person would, so the file is written.
//! `LAUNCH_FLOOR=warm` opens nothing: it makes the GPU device and Rune's pipelines
//! (`rune::warm_gpu_cache`) and exits, as a setup would run an installed app once, so a first
//! launch finds the driver's compiled pipelines. `LAUNCH_FLOOR_MEMORY_POLICY` sets Rune's
//! startup GPU suballocation policy (`memory-usage`, `performance`, `manual:<mib>`, or
//! `manual:<min>-<max>` in MiB) via `App::gpu_memory_policy` / `rune::set_gpu_memory_policy`.
//! `LAUNCH_FLOOR_CPU_FIRST` sets Rune's startup CPU-first policy (`auto`, `only`, or `off`) via
//! `App::cpu_first` without touching `RUNE_CPU_FIRST`.
//! `LAUNCH_FLOOR_LIVE=1` makes the shell's first
//! sidebar row answer the pointer (it changes colour under it, and a press counts in the title), to
//! see that a window takes input from its first frame on, whichever renderer draws it.
//!
//! A moment after the first frame it prints every `rune::marks` step as
//! `launch-mark <name> <µs since the process started> at=<epoch ms>`, then one JSON line
//! (`launch-floor {..}`) with the marks and the totals before the first frame. `LAUNCH_FLOOR_TRACE`
//! names a file that gets the JSON line appended too.

#![forbid(unsafe_code)]
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::io::Write;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
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
    reads: bool,
    /// Presses of the first sidebar row, with `LAUNCH_FLOOR_LIVE`.
    presses: Option<u32>,
}

/// Runs of the view so far, and how many the first frame took.
static VIEWS: AtomicU32 = AtomicU32::new(0);
static FIRST_VIEWS: AtomicU32 = AtomicU32::new(0);

/// Whether `LAUNCH_FLOOR` asks for the desktop app's first screen.
fn shell_scene() -> bool {
    rune::env::var("LAUNCH_FLOOR").is_some_and(|v| v == "shell" || v == "reads")
}

/// Whether `LAUNCH_FLOOR` asks for that screen with a view that reads rects from layout.
fn reads_scene() -> bool {
    rune::env::var("LAUNCH_FLOOR").is_some_and(|v| v == "reads")
}

fn init() -> State {
    State { shell: shell_scene(), reads: reads_scene(), presses: rune::env::var("LAUNCH_FLOOR_LIVE").map(|_| 0) }
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    VIEWS.fetch_add(1, Ordering::Relaxed);
    let t = tok::theme(true);
    if !s.shell { return bare(t); }
    // What a desktop app's view asks layout: a breakpoint, a pane's size, where the composer is.
    let read = s.reads.then(|| Read {
        wide: cx.layout_with("main", |r| r[2] >= 600.0).unwrap_or(false),
        side: cx.layout_size("sidebar").map_or(0.0, |[w, _]| w),
        dock: cx.layout("composer").map_or(0.0, |r| r[1]),
    });
    shell(t, read, s.presses)
}

/// What the `reads` scene's view read from layout.
struct Read {
    wide: bool,
    side: f32,
    dock: f32,
}

fn bare(t: Theme) -> El {
    col().w_full().h_full().center().gap(16.0).fill(t.bg)
        .child(text("The floor").key("floor").font(type_::TITLE).color(t.text))
        .child(el().key("rect").size(120.0, 40.0).fill(tok::accent(0, true)))
}

fn shell(t: Theme, read: Option<Read>, presses: Option<u32>) -> El {
    let title = match (&read, presses) {
        (Some(r), _) if r.wide => format!("Launch floor, sidebar {:.0}, composer at {:.0}", r.side, r.dock),
        (_, Some(presses)) => format!("Launch floor, pressed {presses} times"),
        _ => "Launch floor".to_owned(),
    };
    let titlebar = row().key("titlebar").w_full().h(TITLEBAR_H).px(16.0).gap(12.0).center_y()
        .child(icon(&CHAT, 16.0).color(t.text_secondary))
        .child(text(title).font(type_::LABEL).color(t.text).grow(1.0))
        .child(icon(&PLUS, 16.0).color(t.text_secondary));
    let mut sidebar = col().key("sidebar").w(280.0).h_full().padding(8.0).gap(2.0).fill(t.card);
    for (i, name) in SESSIONS.iter().enumerate() {
        let session = row().key(format!("session-{i}")).h(32.0).px(8.0).gap(8.0).center_y().radius(radius::SM)
            .fill(if i == 0 { t.hover } else { tok::theme(true).card })
            .child(icon(&CHAT, 14.0).color(t.text_secondary))
            .child(text(*name).font(type_::BODY).color(t.text).grow(1.0).truncate())
            .child(text(format!("{}m", 3 + i * 7)).font(type_::LABEL).color(t.text_secondary));
        let live = i == 0 && presses.is_some();
        sidebar = sidebar.child(if live { session.hover(tok::accent(0, true)).on_press(|s: &mut State| s.presses = s.presses.map(|n| n + 1)) } else { session });
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
        "{{\"scene\":\"{}\",\"first_views\":{},\"marks\":{{{}}},\"metrics\":{{{}}},\"notes\":{{{}}}}}",
        if reads_scene() { "reads" } else if shell_scene() { "shell" } else { "bare" },
        FIRST_VIEWS.load(Ordering::Relaxed),
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

/// A seed as text: the viewport, scale and zoom on the first line, then a line per key with
/// its rect (`-` for a key with no element). Rust prints a float so that it parses back exactly.
fn encode(seed: &rune::LayoutSeed) -> String {
    let mut out = format!("{} {} {} {}\n", seed.viewport[0], seed.viewport[1], seed.scale, seed.zoom);
    for (key, rect) in &seed.rects {
        match rect {
            Some([x, y, w, h]) => out.push_str(&format!("{x} {y} {w} {h}\t{key}\n")),
            None => out.push_str(&format!("-\t{key}\n")),
        }
    }
    out
}

fn decode(text: &str) -> Option<rune::LayoutSeed> {
    let floats = |s: &str| s.split(' ').map(|v| v.parse::<f32>().ok()).collect::<Option<Vec<f32>>>();
    let mut lines = text.lines();
    let &[w, h, scale, zoom] = floats(lines.next()?)?.as_slice() else { return None };
    let rects = lines
        .map(|line| {
            let (rect, key) = line.split_once('\t')?;
            if rect == "-" { return Some((key.to_owned(), None)); }
            let &[x, y, w, h] = floats(rect)?.as_slice() else { return None };
            Some((key.to_owned(), Some([x, y, w, h])))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(rune::LayoutSeed { viewport: [w, h], scale, zoom, rects })
}

fn main() {
    let age = rune::process_age().unwrap_or_default();
    BOOT.get_or_init(|| (rune::time::now(), age, rune::time::system_now()));
    rune::marks::mark("main.entered");
    let memory_policy = rune::env::var("LAUNCH_FLOOR_MEMORY_POLICY").as_deref().and_then(parse_memory_policy);
    // LAUNCH_FLOOR=warm: what a setup would run once after an install. No window: the GPU device
    // and Rune's pipelines are made, so Direct3D keeps them for this exe's later launches.
    if rune::env::var("LAUNCH_FLOOR").is_some_and(|v| v == "warm") {
        if let Some(policy) = memory_policy {
            rune::set_gpu_memory_policy(policy);
        }
        let began = rune::time::now();
        let made = rune::warm_gpu_cache();
        let line = format!("{{\"scene\":\"warm\",\"ok\":{},\"us\":{},\"why\":\"{}\"}}\n", made.is_ok(), began.elapsed().as_micros(), made.err().unwrap_or_default().replace('"', "'"));
        print!("launch-floor {line}");
        if let Some(path) = rune::env::var_os("LAUNCH_FLOOR_TRACE")
            && let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path)
        {
            file.write_all(line.as_bytes()).ok();
        }
        return;
    }    let spec = AppSpec::new("Launch floor", (1280.0, 800.0), init, view);
    let shell = shell_scene();
    let cpu_first = rune::env::var("LAUNCH_FLOOR_CPU_FIRST").as_deref().and_then(parse_cpu_first);
    let app = rune::App::new(spec);
    let app = match memory_policy {
        Some(policy) => app.gpu_memory_policy(policy),
        None => app,
    };
    let app = match cpu_first {
        Some(mode) => app.cpu_first(mode),
        None => app,
    };
    let handle = app.handle();
    let app = app.on_ready(move |_| {
        FIRST_VIEWS.store(VIEWS.load(Ordering::Relaxed), Ordering::Relaxed);
        let handle = handle.clone();
        // Off the UI thread, once the loop has gone to wait for input.
        std::thread::spawn(move || {
            // LAUNCH_FLOOR_SETTLE_MS: how long after the first frame the report waits (300); a GPU
            // hand-over on a machine that has never compiled the pipelines needs about 2,500.
            let settle = rune::env::var("LAUNCH_FLOOR_SETTLE_MS").and_then(|v| v.parse().ok()).unwrap_or(300);
            std::thread::sleep(Duration::from_millis(settle));
            report();
            if let Some(quit) = rune::env::var("LAUNCH_FLOOR_QUIT_MS").and_then(|v| v.parse().ok()) {
                std::thread::sleep(Duration::from_millis(quit));
                handle.quit();
            }
        });
    });
    // The view's layout reads, kept in a file between launches.
    let app = match rune::env::var_os("LAUNCH_FLOOR_SEED") {
        Some(path) => {
            let seed = std::fs::read_to_string(&path).ok().and_then(|s| decode(&s));
            app.layout_seed(seed, move |seed| { std::fs::write(&path, encode(&seed)).ok(); })
        }
        None => app,
    };
    let app = if shell {
        app.titlebar(rune::Titlebar::new(TITLEBAR_H)).chrome(rune::Chrome { full_size_content: true, traffic_lights: Some((20.0, 20.0)), drag_regions: true })
    } else {
        app
    };
    app.run();
}

/// Parses `LAUNCH_FLOOR_MEMORY_POLICY`: `memory-usage`, `performance`, `manual:<mib>`, or
/// `manual:<min>-<max>` (MiB).
fn parse_memory_policy(raw: &str) -> Option<rune::GpuMemoryPolicy> {
    const MIB: u64 = 1024 * 1024;
    match raw.trim() {
        "memory-usage" => Some(rune::GpuMemoryPolicy::MemoryUsage),
        "performance" => Some(rune::GpuMemoryPolicy::Performance),
        other => {
            let spec = other.strip_prefix("manual:")?;
            let (min, max) = match spec.split_once('-') {
                Some((a, b)) => (a.parse::<u64>().ok()?, b.parse::<u64>().ok()?),
                None => {
                    let size = spec.parse::<u64>().ok()?;
                    (size, size)
                }
            };
            Some(rune::GpuMemoryPolicy::Manual {
                block_size: min.saturating_mul(MIB)..max.saturating_mul(MIB),
            })
        }
    }
}

/// Parses `LAUNCH_FLOOR_CPU_FIRST`: `auto`, `only`, or `off` (`0`).
fn parse_cpu_first(raw: &str) -> Option<rune::CpuFirst> {
    match raw.trim() {
        "auto" => Some(rune::CpuFirst::Auto),
        "only" => Some(rune::CpuFirst::Only),
        "off" | "0" => Some(rune::CpuFirst::Off),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_cpu_first, parse_memory_policy};
    use rune::{CpuFirst, GpuMemoryPolicy};

    #[test]
    fn parses_launch_floor_memory_policy_values() {
        const MIB: u64 = 1024 * 1024;
        assert_eq!(parse_memory_policy("memory-usage"), Some(GpuMemoryPolicy::MemoryUsage));
        assert_eq!(parse_memory_policy("performance"), Some(GpuMemoryPolicy::Performance));
        assert_eq!(
            parse_memory_policy("manual:4-8"),
            Some(GpuMemoryPolicy::Manual { block_size: (4 * MIB)..(8 * MIB) }),
        );
        assert_eq!(
            parse_memory_policy("manual:4"),
            Some(GpuMemoryPolicy::Manual { block_size: (4 * MIB)..(4 * MIB) }),
        );
        assert_eq!(parse_memory_policy("bogus"), None);
    }

    #[test]
    fn parses_launch_floor_cpu_first_values() {
        assert_eq!(parse_cpu_first("auto"), Some(CpuFirst::Auto));
        assert_eq!(parse_cpu_first("only"), Some(CpuFirst::Only));
        assert_eq!(parse_cpu_first("off"), Some(CpuFirst::Off));
        assert_eq!(parse_cpu_first("0"), Some(CpuFirst::Off));
        assert_eq!(parse_cpu_first("bogus"), None);
    }
}
