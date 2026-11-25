//! Paired alternating headless benchmark comparing `El::virtual_items` window shifts
//! (`CommitKind::Remount`) against full `view` rebuilds on 200-node and 10,000-node canvases.
//! Headless `Runtime`, no window, no GPU upload or presentation.
//!
//! Run with:
//! `cargo run --profile perf -p rune --example virtual_items_bench`

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rune::{Color, Cx, El, Env, Live, LiveText, Runtime, TextStyle, VirtualItems, col, el, text};

thread_local! {
    static VIEWS: Cell<usize> = const { Cell::new(0) };
    static BUILDS: Cell<usize> = const { Cell::new(0) };
}

struct BenchNode {
    key: String,
    x: f32,
    y: f32,
    title: String,
    progress: LiveText,
}

struct BenchCanvas {
    nodes: Vec<BenchNode>,
    ver: Cell<u64>,
    window: RefCell<Vec<usize>>,
}

impl BenchCanvas {
    fn new(n: usize) -> Rc<Self> {
        let cols = (n as f64).sqrt().ceil() as usize;
        let nodes = (0..n)
            .map(|i| BenchNode {
                key: format!("node-{i}"),
                x: (i % cols) as f32 * 150.0,
                y: (i / cols) as f32 * 90.0,
                title: format!("Node #{i:05}"),
                progress: LiveText::new(format!("{}%", i % 100)),
            })
            .collect();
        Rc::new(Self { nodes, ver: Cell::new(1), window: RefCell::new(Vec::new()) })
    }

    fn set_window(&self, start: usize, count: usize) {
        let n = self.nodes.len();
        let win: Vec<usize> = (0..count).map(|k| (start + k) % n).collect();
        *self.window.borrow_mut() = win;
        self.ver.set(self.ver.get() + 1);
    }

    fn build_node(&self, idx: usize) -> El {
        BUILDS.set(BUILDS.get() + 1);
        let node = &self.nodes[idx];
        col()
            .key(node.key.clone())
            .size(136.0, 74.0)
            .absolute(node.x, node.y)
            .padding(8.0)
            .gap(4.0)
            .radius(6.0)
            .fill(Color { r: 0.14, g: 0.16, b: 0.20, a: 1.0 })
            .border(1.0, Color { r: 0.28, g: 0.32, b: 0.38, a: 1.0 })
            .child(text(&node.title).font(TextStyle::new(12.0, 600.0)).color(Color::WHITE))
            .child(
                text("")
                    .live_text(&node.progress)
                    .font(TextStyle::new(11.0, 400.0))
                    .color(Color { r: 0.7, g: 0.8, b: 0.9, a: 1.0 }),
            )
    }
}

impl VirtualItems for BenchCanvas {
    fn version(&self) -> u64 {
        self.ver.get()
    }
    fn keys(&self) -> Vec<String> {
        self.window.borrow().iter().map(|&i| self.nodes[i].key.clone()).collect()
    }
    fn build(&self, key: &str) -> El {
        let idx: usize = key.trim_start_matches("node-").parse().unwrap_or(0);
        self.build_node(idx)
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn p95(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * 0.95).round() as usize]
}

fn run_one(total: usize, visible: usize, use_virtual: bool) -> (f64, f64, usize, usize) {
    struct App {
        cam: Live<[f32; 2]>,
        canvas: Rc<BenchCanvas>,
        use_virtual: bool,
    }
    fn view(s: &App, _: &mut Cx<App>) -> El {
        VIEWS.set(VIEWS.get() + 1);
        let base = el().key("canvas").w_full().h_full().clip_soft(0.0).live_offset(&s.cam);
        if s.use_virtual {
            base.virtual_items(s.canvas.clone())
        } else {
            let kids: Vec<El> = s.canvas.window.borrow().iter().map(|&i| s.canvas.build_node(i)).collect();
            base.children(kids)
        }
    }
    let canvas = BenchCanvas::new(total);
    canvas.set_window(0, visible);
    let mut rt = Runtime::new(
        App { cam: Live::new([0.0, 0.0]), canvas: canvas.clone(), use_virtual },
        view,
        (1200.0, 800.0),
        Env::new(true, 2.0),
    );
    for f in 0..20 {
        rt.state.cam.set([-(f as f32) * 20.0, 0.0]);
        canvas.set_window((f * 4) % total, visible);
        if !use_virtual {
            rt.invalidate();
        }
        rt.frame(1.0 / 120.0);
    }
    VIEWS.set(0);
    BUILDS.set(0);
    let mut times_ms = Vec::with_capacity(120);
    for f in 0..120 {
        let start = ((20 + f) * 4) % total;
        rt.state.cam.set([-((20 + f) as f32) * 20.0, 0.0]);
        canvas.nodes[(start + visible / 2) % total].progress.set(format!("{}%", (f * 3) % 100));
        canvas.set_window(start, visible);
        if !use_virtual {
            rt.invalidate();
        }
        let t0 = rune::time::now();
        rt.frame(1.0 / 120.0);
        times_ms.push(t0.elapsed().as_secs_f64() * 1e3);
    }
    (median(times_ms.clone()), p95(times_ms), VIEWS.get(), BUILDS.get())
}

fn main() {
    if cfg!(debug_assertions) {
        eprintln!("virtual_items_bench: debug build; run with --profile perf");
    }
    for (total, visible) in [(200usize, 40usize), (10_000usize, 60usize)] {
        let (mut r_p50, mut r_p95, mut f_p50, mut f_p95) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let (mut r_views, mut r_builds, mut f_views, mut f_builds) = (0, 0, 0, 0);
        for run in 0..5 {
            let order = if run % 2 == 0 { [true, false] } else { [false, true] };
            for use_virtual in order {
                let (m, p, v, b) = run_one(total, visible, use_virtual);
                if use_virtual {
                    r_p50.push(m);
                    r_p95.push(p);
                    r_views = v;
                    r_builds = b;
                } else {
                    f_p50.push(m);
                    f_p95.push(p);
                    f_views = v;
                    f_builds = b;
                }
            }
        }
        let rp50 = median(r_p50.clone());
        let rp95 = median(r_p95.clone());
        let fp50 = median(f_p50.clone());
        let fp95 = median(f_p95.clone());
        println!(
            "canvas={total} visible={visible} (5 paired alternating runs x 120 frames, 4 enter/4 leave/frame, headless no GPU upload/present):"
        );
        println!(
            "  virtual_items remount: p50={rp50:.3} ms (runs {r_p50:.3?})  p95={rp95:.3} ms  view_runs={r_views}  item_builds={r_builds}"
        );
        println!(
            "  full view rebuild:     p50={fp50:.3} ms (runs {f_p50:.3?})  p95={fp95:.3} ms  view_runs={f_views}  item_builds={f_builds}"
        );
        println!(
            "  speedup: {:.2}x (p50), {:.2}x (p95)",
            fp50 / rp50.max(1e-9),
            fp95 / rp95.max(1e-9),
        );
    }
}
