//! Markdown resize: a long Markdown note rendered with `Markdown` in a scroll view, the window
//! narrowing and widening a point every frame, as a live resize does. Headless, no window.
//!
//! The note is generated (fictional words): headings, long wrapping paragraphs with bold,
//! italic, inline code and links, bullet, numbered and task lists, quotes and fenced code.
//! `--sizes 5,50` picks the note sizes in KB. For each size it times, on a fresh runtime:
//!
//! - `open`: the first frame of a fresh runtime;
//! - `resize`: the width stepping 900 → 600 → 900 pt in 1 pt steps, one frame per step;
//! - `scroll`: wheel frames at a fixed width, for comparison.
//!
//! `--memo` wraps the note in `rune::memo`, so a resize reuses its subtree instead of building it.
//! `--only marks` times adding 2,000 source-range highlights and reading their element boxes.
//! `--only fonts` times `Runtime::set_font_families` switching between Inter and Helvetica Neue
//! with Menlo (system fonts on macOS), the note scrolled to the top: the frame that follows
//! re-shapes and lays out every text.
//!
//! cargo build --profile perf -p rune-kit --example markdown_resize --features markdown,frame-profile
//! target/perf/examples/markdown_resize [--sizes 5,50] [--only resize] [--step 1] [--memo]
//!
//! Prints `METRIC size=.. case=.. metric=.. median=.. p95=.. max=..` per measurement. `cpu_ms` is
//! thread CPU of the frame; phases are thread CPU and overlap: commit includes layout, layout
//! includes text (shaping and line breaking), apply is mounting the laid-out tree, paint is
//! building the frame's draw list.
//! `shaped` counts texts shaped that frame, `rebroken` texts re-broken from an earlier shape,
//! `nodes` layout nodes and `glyphs` glyph quads painted.
use rune::text::TextSystem;
use rune::{Color, Cx, El, Env, Runtime, ScrollSpec, col};
use rune_kit::{Highlight, Markdown, MarkdownStyle};

const HEIGHT: f32 = 900.0;
const BG: Color = Color { r: 0.008, g: 0.008, b: 0.009, a: 1.0 };

struct State { src: String, style: MarkdownStyle, marks: Vec<(std::ops::Range<usize>, Highlight)> }

fn view(s: &State, _: &mut Cx<State>) -> El {
    let note = || Markdown::new(s.src.as_str()).highlights(&s.marks).render(&s.style);
    // `--memo`: an app would use a document/search revision here.
    let note = if MEMO.get() { rune::memo("note", (s.src.len(), s.marks.len()), note) } else { note() };
    col().w_full().h_full().fill(BG).child(
        col().key("viewport").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false })
            .child(col().w_full().px(32.0).py(24.0).child(note)),
    )
}

thread_local! {
    static MEMO: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A deterministic fictional Markdown note of at least `bytes` bytes.
fn note(bytes: usize) -> String {
    const WORDS: &[&str] = &["amber", "lantern", "orchard", "quiet", "harbor", "copper", "meadow", "signal", "drift",
        "pebble", "willow", "ember", "tide", "cinder", "fable", "glacier", "hollow", "juniper", "kestrel", "lumen",
        "marrow", "nimbus", "opal", "prism", "quill", "ripple", "sable", "thistle", "umber", "vellum", "wander",
        "yarrow", "zephyr", "bramble", "cobalt", "dune", "fern", "grove", "heron", "isle", "the", "a", "of", "and",
        "to", "in", "with", "under", "over", "before"];
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as usize
    };
    // Words with now and then a bold, italic, inline code or link run.
    let words = |next: &mut dyn FnMut(usize) -> usize, n: usize| {
        let mut out = Vec::new();
        let mut i = 0;
        while i < n {
            let w = WORDS[next(WORDS.len())];
            out.push(match next(24) {
                0 => format!("**{w} {}**", WORDS[next(WORDS.len())]),
                1 => format!("*{w}*"),
                2 => format!("`{w}_{}`", next(100)),
                3 => format!("[{w}](https://example.com/{w})"),
                _ => w.to_string(),
            });
            i += 1;
        }
        out.join(" ")
    };
    let plain = |next: &mut dyn FnMut(usize) -> usize, n: usize| {
        (0..n).map(|_| WORDS[next(WORDS.len())]).collect::<Vec<_>>().join(" ")
    };
    let mut out = String::new();
    let mut section = 0;
    while out.len() < bytes {
        section += 1;
        out += &format!("# Section {section}: {}\n\n", plain(&mut next, 3));
        for sub in 0..3 {
            out += &format!("## {} {sub}\n\n", plain(&mut next, 2));
            for _ in 0..2 {
                let n = 25 + next(60);
                out += &words(&mut next, n);
                out += ".\n\n";
            }
            for i in 0..4 + next(5) {
                let n = 4 + next(10);
                if sub % 2 == 0 { out += &format!("- {}\n", words(&mut next, n)); } else { out += &format!("{}. {}\n", i + 1, words(&mut next, n)); }
                if next(4) == 0 { let n = 3 + next(5); out += &format!("  - {}\n", words(&mut next, n)); }
            }
            out += "\n";
            match next(3) {
                0 => {
                    out += "```rust\n";
                    for i in 0..5 + next(6) {
                        let (indent, name, call) = ("    ".repeat(next(3)), WORDS[next(WORDS.len())], WORDS[next(WORDS.len())]);
                        out += &format!("{indent}let {name}_{i} = {call}(\"{}\");\n", plain(&mut next, 2));
                    }
                    out += "```\n\n";
                }
                1 => {
                    let n = 12 + next(20);
                    out += &format!("> {}\n\n", words(&mut next, n));
                }
                _ => {
                    for _ in 0..3 { let n = 3 + next(6); out += &format!("- [ ] {}\n", words(&mut next, n)); }
                    out += "\n";
                }
            }
        }
    }
    out
}

const METRICS: &[&str] = &["cpu_ms", "view_ms", "commit_ms", "layout_ms", "text_ms", "paint_ms", "apply_ms", "shaped", "rebroken", "nodes", "glyphs"];

/// Times `input` and the one frame that follows it.
fn timed(rt: &mut Runtime<State>, input: impl FnOnce(&mut Runtime<State>)) -> Vec<f64> {
    rune::take_frame_phases();
    let (cpu0, rebroken0) = (rune::thread_cpu_ms(), rt.text.rebroken);
    input(rt);
    rt.frame(1.0 / 120.0);
    let cpu = rune::thread_cpu_ms() - cpu0;
    let p = rune::take_frame_phases();
    let q = rune::PHASES;
    let s = &rt.stats;
    vec![cpu, p[q], p[q + 1], p[q + 2], p[q + 3], p[q + 4], p[q + 5], s.shaped as f64, (rt.text.rebroken - rebroken0) as f64, s.layout_nodes as f64, s.glyphs as f64]
}

fn report(size: usize, case: &str, rows: &[Vec<f64>]) {
    if rows.is_empty() { return; }
    for (m, name) in METRICS.iter().enumerate() {
        let mut v: Vec<f64> = rows.iter().map(|r| r[m]).collect();
        v.sort_by(f64::total_cmp);
        let p95 = v[((v.len() - 1) as f64 * 0.95).round() as usize];
        println!("METRIC size={size}k case={case} metric={name} median={:.4} p95={p95:.4} max={:.4} n={}",
            v[v.len() / 2], v[v.len() - 1], v.len());
    }
}

fn runtime(src: String, width: f32) -> Runtime<State> {
    let mut fonts = TextSystem::default();
    fonts.layout_rich("Warm fonts", &[], rune::TextStyle::new(15.0, 400.0), 2.0, None);
    for _ in 0..3 { fonts.end_commit(); }
    let state = State { src, style: MarkdownStyle::neutral(true), marks: Vec::new() };
    let mut rt = Runtime::with_text(state, view, (width, HEIGHT), Env::new(true, 2.0), fonts);
    rt.frame(0.0);
    let mut n = 0;
    while n < 240 && rt.frame(1.0 / 120.0) { n += 1; }
    rt
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let sizes: Vec<usize> = value("--sizes").map_or(vec![5, 50], |s| s.split(',').map(|n| n.parse().expect("--sizes 5,50")).collect());
    let step: f32 = value("--step").map_or(1.0, |s| s.parse().expect("--step 1"));
    let only = value("--only");
    let run = |case: &str| only.as_deref().is_none_or(|o| o.split(',').any(|o| o == case));
    MEMO.set(args.iter().any(|a| a == "--memo"));
    if cfg!(debug_assertions) { eprintln!("markdown_resize: debug build; time a --profile perf build"); }
    for &kb in &sizes {
        let src = note(kb * 1024);
        let rt = runtime(src.clone(), 900.0);
        let texts = rt.elements().into_iter().filter(|e| e.text.is_some()).map(|e| e.visible[2] > 0.0 && e.visible[3] > 0.0).collect::<Vec<_>>();
        println!("NOTE size={kb}k bytes={} lines={} texts={} visible_texts={}", src.len(), src.lines().count(), texts.len(), texts.iter().filter(|v| **v).count());
        drop(rt);

        if run("marks") {
            let mut rt = runtime(src.clone(), 900.0);
            let ranges: Vec<_> = src.char_indices().filter(|&(i, c)| c.is_ascii_alphabetic() && (i == 0 || src.as_bytes()[i - 1].is_ascii_whitespace()))
                .take(2000).map(|(i, _)| (i..i + src[i..].bytes().take_while(u8::is_ascii_alphabetic).count(), Highlight::new())).collect();
            let row = timed(&mut rt, |rt| { rt.state.marks = ranges; rt.invalidate(); });
            report(kb, "marks", &[row]);
            let t = std::time::Instant::now();
            let elements = rt.elements();
            println!("MARKS requested={} reported={} nodes={} elements={} elements_ms={:.3}", rt.state.marks.len(), elements.iter().filter(|e| e.role == "mark").count(), rt.stats.layout_nodes, elements.len(), t.elapsed().as_secs_f64() * 1000.0);
        }

        if run("open") {
            // The first frame of a fresh runtime: every text shaped once, its other widths re-broken.
            let rows: Vec<_> = (0..10).map(|_| {
                let mut fonts = TextSystem::default();
                fonts.layout_rich("Warm fonts", &[], rune::TextStyle::new(15.0, 400.0), 2.0, None);
                let state = State { src: src.clone(), style: MarkdownStyle::neutral(true), marks: Vec::new() };
                let mut rt = Runtime::with_text(state, view, (900.0, HEIGHT), Env::new(true, 2.0), fonts);
                timed(&mut rt, |_| {})
            }).collect();
            report(kb, "open", &rows);
        }

        if run("resize") {
            let mut rt = runtime(src.clone(), 900.0);
            let steps = (300.0 / step).round() as usize;
            let widths = (1..=steps).map(|i| 900.0 - i as f32 * step).chain((1..=steps).map(|i| 600.0 + i as f32 * step));
            let rows: Vec<_> = widths.map(|w| timed(&mut rt, |rt| rt.set_viewport((w, HEIGHT), 2.0))).collect();
            report(kb, "resize", &rows);
        }

        if run("fonts") {
            let mut rt = runtime(src.clone(), 900.0);
            let rows: Vec<_> = (0..40).map(|i| {
                let (sans, mono) = if i % 2 == 0 { ("Helvetica Neue", "Menlo") } else { ("Inter", "Inter") };
                timed(&mut rt, |rt| rt.set_font_families(sans, mono))
            }).collect();
            report(kb, "fonts", &rows);
        }

        if run("scroll") {
            let mut rt = runtime(src.clone(), 760.0);
            let rows: Vec<_> = (0..120).map(|_| timed(&mut rt, |rt| rt.wheel(380.0, HEIGHT / 2.0, -120.0))).collect();
            report(kb, "scroll", &rows);
        }
    }
}
