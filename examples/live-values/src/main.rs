//! Live-bound opacity, cursor offset, needle rotation, and wrapped/truncated live text updated
//! at 60–120 Hz from background threads without rebuilding the view (`--bench` compares live
//! bindings against `--rebuild`).

#![forbid(unsafe_code)]

use rune::{AppSpec, Color, Cx, El, Env, Live, LiveText, Runtime, Spring, TextStyle, Tx, col, el, row, text};
use std::sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}};

static WINDOWED: AtomicBool = AtomicBool::new(false);

struct State {
    levels: [f32; 16],
    cursors: [[f32; 2]; 4],
    angle: f32,
    caption: String,
    ticker: String,
    tick: u64,
    live_levels: [Live<f32>; 16],
    live_cursors: [Live<[f32; 2]>; 4],
    live_angle: Live<f32>,
    live_caption: LiveText,
    live_ticker: LiveText,
    rebuild: bool,
    smooth: bool,
}

fn format_caption(angle: f32, tick: u64) -> String {
    format!("Angle {angle:.2} rad at tick {tick:03} (wrapped live caption)")
}

fn format_ticker(angle: f32, tick: u64) -> String {
    format!("Needle {:.0} deg — frame {tick:03} / 480", angle.to_degrees())
}

fn init() -> State {
    let mut s = State {
        levels: [0.5; 16],
        cursors: [[0.0; 2]; 4],
        angle: 0.0,
        caption: format_caption(0.0, 0),
        ticker: format_ticker(0.0, 0),
        tick: 0,
        live_levels: std::array::from_fn(|_| Live::new(0.5)),
        live_cursors: std::array::from_fn(|_| Live::new([0.0; 2])),
        live_angle: Live::new(0.0),
        live_caption: LiveText::new(format_caption(0.0, 0)),
        live_ticker: LiveText::new(format_ticker(0.0, 0)),
        rebuild: rune::extra_args().iter().any(|a| a == "--rebuild"),
        smooth: false,
    };
    sample(&mut s, 0);
    s
}

fn sample(s: &mut State, tick: u64) {
    if tick % 2 == 0 {
        for (i, level) in s.levels.iter_mut().enumerate() {
            *level = 0.5 + 0.45 * (tick as f32 / 120.0 * 3.0 + i as f32).sin();
        }
    }
    for (i, cursor) in s.cursors.iter_mut().enumerate() {
        let t = tick as f32 / 120.0 + i as f32;
        *cursor = [250.0 + 180.0 * t.sin(), 190.0 + 120.0 * (t * 0.7).cos()];
    }
    s.angle = tick as f32 / 120.0 * std::f32::consts::TAU * 0.5;
    if tick % 4 == 0 {
        s.caption = format_caption(s.angle, tick);
        s.ticker = format_ticker(s.angle, tick);
    }
    if !s.rebuild {
        for (live, value) in s.live_levels.iter().zip(s.levels) { live.set(value); }
        for (live, value) in s.live_cursors.iter().zip(s.cursors) { live.set(value); }
        s.live_angle.set(s.angle);
        if tick % 4 == 0 {
            s.live_caption.set(s.caption.as_str());
            s.live_ticker.set(s.ticker.as_str());
        }
    }
}

fn poll(s: &mut State, t: f64, tx: &mut Tx) -> bool {
    if WINDOWED.load(Ordering::Relaxed) && !s.rebuild { return false; }
    let tick = (t.min(4.0) * 120.0).round() as u64;
    if tick == s.tick { return false; }
    s.tick = tick;
    sample(s, tick);
    tx.instant();
    s.rebuild
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let green = Color { r: 0.1, g: 0.8, b: 0.4, a: 1.0 };
    let dial_ring = Color { r: 0.2, g: 0.26, b: 0.32, a: 1.0 };
    let fg = Color { r: 0.88, g: 0.92, b: 0.96, a: 1.0 };
    let muted = Color { r: 0.56, g: 0.64, b: 0.72, a: 1.0 };
    let mut grid = col().gap(20.0);
    for y in 0..4 {
        let mut line = row().gap(24.0);
        for x in 0..4 {
            let i = y * 4 + x;
            let ring = el().key(format!("speaker-{i}")).size(64.0, 64.0)
                .radius(32.0).border(3.0, green);
            line = line.child(if s.rebuild { ring.opacity(s.levels[i]) } else { ring.live_opacity(&s.live_levels[i]) });
        }
        grid = grid.child(line);
    }
    let needle = el().key("needle").absolute(50.0, 10.0).size(4.0, 42.0)
        .radius(2.0).fill(green).origin(0.5, 1.0);
    let needle = if s.rebuild {
        needle.rotate(s.angle)
    } else if s.smooth {
        needle.live_rotation(s.live_angle.smooth(Spring::new(0.16, 0.0)))
    } else {
        needle.live_rotation(&s.live_angle)
    };
    let dial = el().key("dial").size(104.0, 104.0).radius(52.0).border(2.0, dial_ring)
        .child(needle)
        .child(el().absolute(48.0, 48.0).size(8.0, 8.0).radius(4.0).fill(fg));
    let caption = text(if s.rebuild { s.caption.as_str() } else { "Angle 0.00 rad at tick 000" })
        .key("live-caption").size(120.0, 40.0).max_lines(2)
        .font(TextStyle::new(12.0, 400.0).features("'tnum' 1")).color(fg);
    let caption = if s.rebuild { caption } else { caption.live_text(&s.live_caption) };
    let ticker = text(if s.rebuild { s.ticker.as_str() } else { "Needle 0 deg" })
        .key("live-ticker").size(120.0, 18.0).truncate()
        .font(TextStyle::new(11.0, 500.0).features("'tnum' 1")).color(muted);
    let ticker = if s.rebuild { ticker } else { ticker.live_text(&s.live_ticker) };
    let sidebar = col().w(120.0).gap(16.0).child(dial).child(caption).child(ticker);
    let layout = row().padding(40.0).gap(32.0).child(grid).child(sidebar);
    let mut root = el().fill(Color { r: 0.018, g: 0.024, b: 0.03, a: 1.0 })
        .size(560.0, 440.0).child(layout);
    for (i, p) in s.cursors.iter().enumerate() {
        let cursor = el().key(format!("cursor-{i}")).size(12.0, 18.0).radius(3.0).fill(green);
        root = root.child(if s.rebuild { cursor.pin(Some(p[0]), Some(p[1]), None, None) }
            else {
                let cursor = cursor.pin(Some(0.0), Some(0.0), None, None);
                if s.smooth { cursor.live_offset(s.live_cursors[i].smooth(Spring::new(0.16, 0.0))) }
                else { cursor.live_offset(&s.live_cursors[i]) }
            });
    }
    root
}

fn bench_mode(rebuild: bool) {
    let mut medians = Vec::new();
    for run in 0..8 {
        let mut state = init(); state.rebuild = rebuild;
        let mut rt = Runtime::new(state, view, (560.0, 440.0), Env::new(true, 2.0));
        let wakes = Arc::new(AtomicUsize::new(0)); let count = wakes.clone();
        rt.set_live_waker(Arc::new(move || { count.fetch_add(1, Ordering::Relaxed); }));
        rt.poll = Some(poll);
        rt.frame(0.0);
        let mut times = Vec::new();
        for _ in 0..480 {
            let start = rune::time::now();
            rt.frame(1.0 / 120.0);
            times.push(start.elapsed().as_secs_f64() * 1e6);
        }
        times.sort_by(f64::total_cmp);
        if run > 0 { medians.push(times[times.len() / 2]); }
        if run == 7 && !rebuild {
            let wake_count = wakes.load(Ordering::Relaxed);
            let mut frames = 0;
            for _ in 0..1200 {
                sample(&mut rt.state, 480);
                if rt.live_pending() || rt.animating { rt.frame(1.0 / 120.0); frames += 1; }
            }
            println!("METRIC simulated_wakes_per_second={} idle_frames={} idle_wakes={}", wake_count as f64 / 4.0,
                frames, wakes.load(Ordering::Relaxed) - wake_count);
        }
    }
    medians.sort_by(f64::total_cmp);
    println!("METRIC {}_cpu_us_median={:.3} runs={medians:?}", if rebuild { "rebuild" } else { "live" }, medians[3]);
}

fn setup(s: &mut State, wake: rune::Waker) {
    if !WINDOWED.load(Ordering::Relaxed) { return; }
    if s.rebuild {
        std::thread::spawn(move || {
            for _ in 0..480 { std::thread::sleep(std::time::Duration::from_secs_f64(1.0 / 120.0)); wake(); }
        });
        return;
    }
    let levels = s.live_levels.clone();
    let cursors = s.live_cursors.clone();
    let angle = s.live_angle.clone();
    let caption = s.live_caption.clone();
    let ticker = s.live_ticker.clone();
    std::thread::spawn(move || {
        let start = rune::time::now();
        for tick in 1..=480 {
            std::thread::sleep((start + std::time::Duration::from_secs_f64(tick as f64 / 120.0)).saturating_duration_since(rune::time::now()));
            if tick % 2 == 0 {
                for (i, level) in levels.iter().enumerate() { level.set(0.5 + 0.45 * (tick as f32 / 120.0 * 3.0 + i as f32).sin()); }
            }
            for (i, cursor) in cursors.iter().enumerate() {
                let t = tick as f32 / 120.0 + i as f32;
                cursor.set([250.0 + 180.0 * t.sin(), 190.0 + 120.0 * (t * 0.7).cos()]);
            }
            let rad = tick as f32 / 120.0 * std::f32::consts::TAU * 0.5;
            angle.set(rad);
            if tick % 4 == 0 {
                caption.set(format_caption(rad, tick));
                ticker.set(format_ticker(rad, tick));
            }
        }
    });
}

fn main() {
    if std::env::args().any(|a| a == "--bench") { bench_mode(true); return bench_mode(false); }
    WINDOWED.store(!std::env::args().any(|a| matches!(a.as_str(), "--snap" | "--trace" | "--tree" | "--frames" | "--strip" | "--first-pixels" | "--list-scenes")), Ordering::Relaxed);
    rune::run(AppSpec::<State> {
        scenes: &[("smooth", |s| s.smooth = true)], setup: Some(setup), poll: Some(poll),
        ..AppSpec::new("Live values", (560.0, 440.0), init, view)
    });
}
