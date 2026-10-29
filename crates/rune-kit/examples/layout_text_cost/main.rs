//! Layout and text cost: where a frame's CPU time and heap go, split between Taffy (layout),
//! Parley and its font stack (text), and Rune's own code. Headless, no window. See
//! research/31-layout-text-cost.md.
//!
//! Cases, each on a fresh runtime after it settles:
//!
//! - `stack`: a settings-like column of 60 cards of switches, badges and buttons (about 5,300
//!   layout nodes, no virtualization); each frame flips one switch.
//! - `stack-same`: the same column; each frame re-runs the view with nothing changed.
//! - `list`: 10,000 rows of three texts under `virtual_rows`, scrolled two rows a frame.
//! - `edit`: one multi-line `text_input` holding a `--kb` note; each frame types a character.
//! - `md-resize`: a `--kb` Markdown note in a scroll view; each frame steps the width 1 pt,
//!   900 → 600 → 900, as a live resize does.
//! - `md-same`: the Markdown note; each frame re-runs the view with nothing changed.
//! - `md-open`: the first frame of a fresh runtime showing the Markdown note (every text shaped).
//!
//! `--mode time` (default) prints `METRIC case=.. metric=.. median=.. p95=.. mean=..` per measurement;
//! `cpu_ms` is thread CPU of the input and its frame, the phases are thread CPU and overlap
//! (commit includes view, layout and apply; layout includes text), `allocs` counts allocator
//! calls in the frame. `--mode sample --out FILE` samples the main thread's stacks every
//! 200 µs during the measured frames; `--mode heap --out FILE` records the stack of every
//! allocation, what each case allocates per frame and what is live after it settles. Both are
//! macOS on Apple silicon only; `scripts/cost-attribution.py` buckets them. `--repeat N` runs each
//! case N times on fresh runtimes, for more samples.
//!
//! CARGO_PROFILE_PERF_DEBUG=limited cargo build --profile perf -p rune-kit \
//!   --example layout_text_cost --features markdown,frame-profile
//! target/perf/examples/layout_text_cost [--case stack,list] [--kb 50] [--mode time|sample|heap] [--out FILE] [--repeat N]
#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]
mod probe;

use std::borrow::Cow;

use rune::text::TextSystem;
use rune::{Color, Cx, El, Env, GrowthSpec, Ime, Runtime, ScrollSpec, TextEdit, TextStyle, col, row, text, text_input};
use rune_kit::{Badge, Button, ButtonVariant, Card, Kit, Markdown, MarkdownStyle, Switch};

#[global_allocator]
static ALLOC: probe::Probe = probe::Probe;

const CASES: &[&str] = &["stack", "stack-same", "list", "edit", "md-resize", "md-same", "md-open"];
const METRICS: &[&str] = &["cpu_ms", "view_ms", "commit_ms", "layout_ms", "text_ms", "paint_ms", "apply_ms", "allocs", "shaped", "rebroken", "nodes"];
const BG: Color = Color { r: 0.008, g: 0.008, b: 0.009, a: 1.0 };
const WORDS: &[&str] = &["amber", "lantern", "orchard", "quiet", "harbor", "copper", "meadow", "signal", "drift",
    "pebble", "willow", "ember", "tide", "cinder", "fable", "glacier", "hollow", "juniper", "kestrel", "lumen",
    "marrow", "nimbus", "opal", "prism", "quill", "ripple", "sable", "thistle", "umber", "vellum", "wander",
    "yarrow", "zephyr", "bramble", "cobalt", "dune", "fern", "grove", "heron", "isle", "the", "a", "of", "and",
    "to", "in", "with", "under", "over", "before"];

struct Rng(u64);
impl Rng {
    fn next(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
    fn words(&mut self, n: usize) -> String { (0..n).map(|_| WORDS[self.next(WORDS.len())]).collect::<Vec<_>>().join(" ") }
}

/// A deterministic fictional Markdown note of at least `bytes` bytes: headings, wrapping
/// paragraphs with bold, italic, code and link runs, lists, quotes, task lists and code.
fn note(bytes: usize) -> String {
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    let rich = |r: &mut Rng, n: usize| {
        (0..n).map(|_| {
            let w = WORDS[r.next(WORDS.len())];
            match r.next(24) {
                0 => format!("**{w} {}**", WORDS[r.next(WORDS.len())]),
                1 => format!("*{w}*"),
                2 => format!("`{w}_{}`", r.next(100)),
                3 => format!("[{w}](https://example.com/{w})"),
                _ => w.to_string(),
            }
        }).collect::<Vec<_>>().join(" ")
    };
    let mut out = String::new();
    let mut section = 0;
    while out.len() < bytes {
        section += 1;
        out += &format!("# Section {section}: {}\n\n", r.words(3));
        for sub in 0..3 {
            out += &format!("## {} {sub}\n\n", r.words(2));
            for _ in 0..2 {
                let n = 25 + r.next(60);
                out += &rich(&mut r, n);
                out += ".\n\n";
            }
            for i in 0..4 + r.next(5) {
                let n = 4 + r.next(10);
                let item = rich(&mut r, n);
                if sub % 2 == 0 { out += &format!("- {item}\n"); } else { out += &format!("{}. {item}\n", i + 1); }
            }
            out += "\n";
            match r.next(3) {
                0 => {
                    out += "```rust\n";
                    for i in 0..5 + r.next(6) {
                        let (name, call) = (WORDS[r.next(WORDS.len())], WORDS[r.next(WORDS.len())]);
                        out += &format!("    let {name}_{i} = {call}(\"{}\");\n", r.words(2));
                    }
                    out += "```\n\n";
                }
                1 => { let n = 12 + r.next(20); out += &format!("> {}\n\n", rich(&mut r, n)); }
                _ => {
                    for _ in 0..3 { let n = 3 + r.next(6); out += &format!("- [ ] {}\n", rich(&mut r, n)); }
                    out += "\n";
                }
            }
        }
    }
    out
}

fn scroller(content: El) -> El {
    col().w_full().h_full().fill(BG).child(
        col().key("viewport").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(content),
    )
}

// stack, stack-same

const CARDS: usize = 60;
const ROWS: usize = 8;

struct Stack { kit: Kit, on: Vec<bool>, labels: Vec<(String, String)> }

fn stack_view(s: &Stack, _: &mut Cx<Stack>) -> El {
    let k = &s.kit;
    let cards = (0..CARDS).map(|c| {
        let mut card = Card::new().title(format!("Section {c}"), None).description(s.labels[c * ROWS].1.clone());
        for r in 0..ROWS {
            let i = c * ROWS + r;
            let (label, about) = &s.labels[i];
            card = card.child(row().key(format!("r{r}")).w_full().gap(12.0).center_y()
                .child(Switch::new(s.on[i]).label(label.clone()).description(about.clone())
                    .on_toggle(move |s: &mut Stack| s.on[i] = !s.on[i]).build(k).grow(1.0).shrink())
                .child(Badge::new(format!("{i}")).build(k))
                .child(Button::new("Edit").variant(ButtonVariant::Outline).build(k)));
        }
        card.build(k).key(format!("c{c}"))
    });
    scroller(col().w_full().px(24.0).py(24.0).gap(16.0).children(cards))
}

fn stack_state() -> Stack {
    let mut r = Rng(7);
    let labels = (0..CARDS * ROWS).map(|_| { let (a, b) = (1 + r.next(3), 6 + r.next(10)); (r.words(a), r.words(b)) }).collect();
    Stack { kit: Kit::neutral(true), on: vec![false; CARDS * ROWS], labels }
}

// list

const LIST_ROWS: usize = 10_000;
const ROW_H: f32 = 44.0;

struct List { titles: Vec<String>, subtitles: Vec<String>, keys: Vec<String> }

fn list_row(s: &List, i: usize) -> El {
    row().w_full().h(ROW_H).px(12.0).gap(10.0).center_y()
        .child(col().key("copy").grow(1.0).shrink().gap(2.0)
            .child(text(s.titles[i].clone()).font(TextStyle::new(14.0, 500.0)).color(Color::WHITE).truncate())
            .child(text(s.subtitles[i].clone()).font(TextStyle::new(12.0, 400.0)).color(Color::WHITE.alpha(0.6)).truncate()))
        .child(text(format!("{}:{:02}", 1 + i % 12, i % 60)).font(TextStyle::new(12.0, 400.0)).color(Color::WHITE.alpha(0.6)))
}

fn list_view(s: &List, _: &mut Cx<List>) -> El {
    scroller(col().key("rows").w_full().px(8.0).growth(GrowthSpec::default())
        .virtual_rows(s.titles.len(), ROW_H, |s: &List, i| Cow::Borrowed(s.keys[i].as_str()), list_row))
}

fn list_state() -> List {
    let mut r = Rng(11);
    let titles = (0..LIST_ROWS).map(|i| { let n = 2 + r.next(4); format!("{} {i}", r.words(n)) }).collect();
    let subtitles = (0..LIST_ROWS).map(|_| { let n = 6 + r.next(10); r.words(n) }).collect();
    List { titles, subtitles, keys: (0..LIST_ROWS).map(|i| format!("m{i}")).collect() }
}

// edit

struct Edit { note: TextEdit }

fn edit_style() -> TextStyle { TextStyle::new(15.0, 400.0).lh(22.0) }

fn edit_view(s: &Edit, _: &mut Cx<Edit>) -> El {
    scroller(col().w_full().px(32.0).py(24.0)
        .child(text_input(&s.note, edit_style()).key("editor").w_full().on_edit(|s: &mut Edit| &mut s.note)
            .on_submit(|s: &mut Edit, _| s.note.insert("\n"))))
}

// md-*

struct Md { src: String, style: MarkdownStyle }

fn md_view(s: &Md, _: &mut Cx<Md>) -> El {
    scroller(col().w_full().px(32.0).py(24.0).child(Markdown::new(s.src.as_str()).render(&s.style)))
}

fn fonts() -> TextSystem {
    let mut fonts = TextSystem::default();
    fonts.layout_rich("Warm fonts", &[], TextStyle::new(15.0, 400.0), 2.0, None);
    for _ in 0..3 { fonts.end_commit(); }
    fonts
}

fn settle<S: 'static>(rt: &mut Runtime<S>) {
    rt.frame(0.0);
    let mut n = 0;
    while n < 240 && rt.frame(1.0 / 120.0) { n += 1; }
}

fn runtime<S: 'static>(state: S, view: fn(&S, &mut Cx<S>) -> El, size: (f32, f32)) -> Runtime<S> {
    let mut rt = Runtime::with_text(state, view, size, Env::new(true, 2.0), fonts());
    settle(&mut rt);
    rt
}

/// Times `input` and the one frame that follows it, under case `tag`.
fn timed<S: 'static>(rt: &mut Runtime<S>, tag: u32, input: impl FnOnce(&mut Runtime<S>)) -> Vec<f64> {
    rune::take_frame_phases();
    let (shaped0, rebroken0, allocs0) = (rt.text.shaped, rt.text.rebroken, probe::allocations());
    probe::TAG.store(tag, std::sync::atomic::Ordering::Relaxed);
    let cpu0 = rune::thread_cpu_ms();
    input(rt);
    rt.frame(1.0 / 120.0);
    let cpu = rune::thread_cpu_ms() - cpu0;
    probe::TAG.store(0, std::sync::atomic::Ordering::Relaxed);
    let allocs = probe::allocations() - allocs0;
    let p = rune::take_frame_phases();
    let q = rune::PHASES;
    vec![cpu, p[q], p[q + 1], p[q + 2], p[q + 3], p[q + 4], p[q + 5], allocs as f64,
        (rt.text.shaped - shaped0) as f64, (rt.text.rebroken - rebroken0) as f64, rt.stats.layout_nodes as f64]
}

fn report(case: &str, rows: &[Vec<f64>]) {
    for (m, name) in METRICS.iter().enumerate() {
        let mut v: Vec<f64> = rows.iter().map(|r| r[m]).collect();
        v.sort_by(f64::total_cmp);
        let p95 = v[((v.len() - 1) as f64 * 0.95).round() as usize];
        let mean = v.iter().sum::<f64>() / v.len() as f64;
        println!("METRIC case={case} metric={name} median={:.4} p95={p95:.4} mean={mean:.4} n={}", v[v.len() / 2], v.len());
    }
}

/// Texts and layout nodes now shown, and live heap, for the heap-per-node table.
fn census<S: 'static>(rt: &Runtime<S>, case: &str, heap0: i64) -> String {
    let texts = rt.elements().iter().filter(|e| e.text.is_some()).count();
    format!("case={case} nodes={} texts={texts} heap={}", rt.stats.layout_nodes, probe::live_bytes() - heap0)
}

fn run(case: &str, tag: u32, kb: usize, meta: &mut Vec<String>) -> Vec<Vec<f64>> {
    let heap0 = probe::live_bytes();
    let src = note(kb * 1024);
    match case {
        "stack" | "stack-same" => {
            let mut rt = runtime(stack_state(), stack_view, (900.0, 900.0));
            probe::snapshot(tag);
            meta.push(census(&rt, case, heap0));
            let same = case == "stack-same";
            (0..240).map(|i| timed(&mut rt, tag, |rt| {
                if !same { let n = rt.state.on.len(); rt.state.on[(i * 37) % n] ^= true; }
                rt.invalidate();
            })).collect()
        }
        "list" => {
            let mut rt = runtime(list_state(), list_view, (760.0, 800.0));
            rt.pointer_move(380.0, 400.0);
            probe::snapshot(tag);
            meta.push(census(&rt, case, heap0));
            // Two rows a frame, a quick flick: each frame mounts new rows and shapes their texts.
            (0..480).map(|_| timed(&mut rt, tag, |rt| rt.wheel(380.0, 400.0, -2.0 * ROW_H))).collect()
        }
        "edit" => {
            let mid = src[..src.len() / 2].rfind(' ').unwrap_or(0) + 1;
            let mut note = TextEdit::new(&src);
            note.focused = true;
            note.set_caret(mid);
            let mut rt = runtime(Edit { note }, edit_view, (760.0, 900.0));
            for _ in 0..3 { rt.ime(Ime::Commit("w".into())); rt.frame(1.0 / 120.0); }
            settle(&mut rt);
            probe::snapshot(tag);
            meta.push(census(&rt, case, heap0));
            (0..60).map(|_| timed(&mut rt, tag, |rt| rt.ime(Ime::Commit("w".into())))).collect()
        }
        "md-resize" | "md-same" => {
            let mut rt = runtime(Md { src, style: MarkdownStyle::neutral(true) }, md_view, (900.0, 900.0));
            probe::snapshot(tag);
            meta.push(census(&rt, case, heap0));
            if case == "md-same" {
                (0..120).map(|_| timed(&mut rt, tag, |rt| rt.invalidate())).collect()
            } else {
                let widths = (1..=300).map(|i| 900.0 - i as f32).chain((1..=300).map(|i| 600.0 + i as f32));
                widths.map(|w| timed(&mut rt, tag, |rt| rt.set_viewport((w, 900.0), 2.0))).collect()
            }
        }
        "md-open" => {
            let rows = (0..10).map(|_| {
                let fonts = fonts();
                let state = Md { src: src.clone(), style: MarkdownStyle::neutral(true) };
                let mut rt = Runtime::with_text(state, md_view, (900.0, 900.0), Env::new(true, 2.0), fonts);
                timed(&mut rt, tag, |_| {})
            }).collect();
            rows
        }
        other => panic!("unknown case {other}; one of {CASES:?}"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let mode = value("--mode").unwrap_or_else(|| "time".into());
    probe::init(mode == "heap");
    let repeat: usize = value("--repeat").map_or(1, |s| s.parse().expect("--repeat N"));
    let kb: usize = value("--kb").map_or(50, |s| s.parse().expect("--kb 50"));
    let cases: Vec<String> = value("--case").map_or(CASES.iter().map(|c| c.to_string()).collect(), |c| c.split(',').map(String::from).collect());
    if cfg!(debug_assertions) { eprintln!("layout_text_cost: debug build; time a --profile perf build"); }
    let sampler = (mode == "sample").then(|| probe::Sampler::start(200));
    let mut meta = Vec::new();
    let mut tags = Vec::new();
    for (i, case) in cases.iter().enumerate() {
        let tag = i as u32 + 1;
        tags.push((tag, case.clone()));
        let rows: Vec<_> = (0..repeat).flat_map(|_| run(case, tag, kb, &mut meta)).collect();
        report(case, &rows);
    }
    for m in &meta { println!("CENSUS {m}"); }
    // How often a word repeats: the ceiling on what a per-word shaping cache could reuse.
    let tokens = |texts: &mut dyn Iterator<Item = String>| {
        let all: Vec<String> = texts.flat_map(|t| t.split_whitespace().map(String::from).collect::<Vec<_>>()).collect();
        (all.len(), all.iter().collect::<std::collections::HashSet<_>>().len())
    };
    let (note_all, note_distinct) = tokens(&mut std::iter::once(note(kb * 1024)));
    let list = list_state();
    let (list_all, list_distinct) = tokens(&mut list.titles.into_iter().chain(list.subtitles));
    println!("WORDS note_tokens={note_all} note_distinct={note_distinct} list_tokens={list_all} list_distinct={list_distinct}");
    if let Some(s) = sampler { s.stop(); }
    if mode != "time" {
        probe::dump(&value("--out").expect("--out FILE"), &tags, &meta);
    }
}
