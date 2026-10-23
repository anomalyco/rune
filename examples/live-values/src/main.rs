use rune::{AppSpec, Color, Cx, El, Env, Live, Runtime, Spring, Tx, col, el, row};
use std::sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}};

static WINDOWED: AtomicBool = AtomicBool::new(false);

struct State {
    levels: [f32; 16],
    cursors: [[f32; 2]; 4],
    tick: u64,
    live_levels: [Live<f32>; 16],
    live_cursors: [Live<[f32; 2]>; 4],
    rebuild: bool,
    smooth: bool,
}

fn init() -> State {
    let mut s = State { levels: [0.5; 16], cursors: [[0.0; 2]; 4], tick: 0,
        live_levels: std::array::from_fn(|_| Live::new(0.5)),
        live_cursors: std::array::from_fn(|_| Live::new([0.0; 2])),
        rebuild: rune::extra_args().iter().any(|a| a == "--rebuild"), smooth: false };
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
    if !s.rebuild {
        for (live, value) in s.live_levels.iter().zip(s.levels) { live.set(value); }
        for (live, value) in s.live_cursors.iter().zip(s.cursors) { live.set(value); }
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
    let mut grid = col().gap(20.0).padding(40.0);
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
    let mut root = el().fill(Color { r: 0.018, g: 0.024, b: 0.03, a: 1.0 })
        .size(560.0, 440.0).child(grid);
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
            let start = std::time::Instant::now();
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
    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        for tick in 1..=480 {
            std::thread::sleep((start + std::time::Duration::from_secs_f64(tick as f64 / 120.0)).saturating_duration_since(std::time::Instant::now()));
            if tick % 2 == 0 {
                for (i, level) in levels.iter().enumerate() { level.set(0.5 + 0.45 * (tick as f32 / 120.0 * 3.0 + i as f32).sin()); }
            }
            for (i, cursor) in cursors.iter().enumerate() {
                let t = tick as f32 / 120.0 + i as f32;
                cursor.set([250.0 + 180.0 * t.sin(), 190.0 + 120.0 * (t * 0.7).cos()]);
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
