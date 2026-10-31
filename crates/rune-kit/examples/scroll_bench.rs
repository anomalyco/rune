//! Deterministic, windowless scroll workloads for scripts/bench-ui.py --scroll.
//! Times input dispatch through draw-list construction, excluding GPU submission.
use std::cell::Cell;
use rune::{Color, Cx, El, Env, GrowthSpec, Runtime, ScrollSpec, Tx, col, el, row, text};
use rune_kit::{Markdown, MarkdownStyle};

#[global_allocator]
static ALLOC: procwatch::alloc::Counting = procwatch::alloc::Counting::system();

struct State {
    workload: String,
    rows: usize,
    source: String,
    /// The card workload's entry notes, and the entry its marker sits in (moved every frame).
    notes: Vec<String>,
    marker: usize,
    style: MarkdownStyle,
    views: Cell<u64>,
}

fn item(i: usize) -> El {
    col().key(format!("row-{i}")).w_full().h(32.0)
        .hover(Color::WHITE.alpha(0.08)).on_press(|_: &mut State| {})
        .child(text(format!("Row {i}: amber lantern orchard quiet harbor")).font(rune::TextStyle::new(14.0, 400.0)).color(Color::WHITE))
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    s.views.set(s.views.get() + 1);
    let content = match s.workload.as_str() {
        "markdown" => Markdown::new(s.source.as_str()).render(&s.style),
        "sticky" => col().w_full().children((0..40).map(|section| {
            col().key(format!("section-{section}")).w_full()
                .child(text(format!("Section {section}")).key("header").h(32.0).w_full()
                    .fill(Color::BLACK).color(Color::WHITE).sticky_top(0.0))
                .children((0..20).map(|i| item(section * 20 + i)))
        })),
        "growth" => col().key("stack").w_full().growth(GrowthSpec::default())
            .children((0..s.rows).map(item)),
        "list" => col().w_full().children((0..s.rows).map(item)),
        // Dense wrapped paragraphs: most of the frame's cost is visible glyphs.
        "prose" => col().w_full().gap(12.0).padding(16.0).children((0..200).map(|i| {
            text(format!("{i}. {}", PROSE.repeat(1 + i % 3))).key(format!("para-{i}"))
                .font(rune::TextStyle::new(13.0, 400.0)).color(Color::WHITE).w_full()
        })),
        // A long card: a thread of entries, each a header with chips, a rich note and a row of
        // links, rebuilt whole every frame because the marked entry moves (a really needed
        // commit), under the root hover curves the docs recommend.
        "card" => col().w_full().gap(14.0).padding(16.0).hover_motion_within(rune::Anim::INSTANT, rune::Anim::INSTANT)
            .children(s.notes.iter().enumerate().map(|(i, note)| entry(s, i, note))),
        other => panic!("unknown workload {other}"),
    };
    col().w_full().h_full().fill(Color::BLACK).child(
        col().key("viewport").w_full().h_full()
            .scroll(ScrollSpec { stick_bottom: s.workload == "growth" })
            .clip_fade(0.0, 12.0, 0.0, 12.0).child(content),
    )
}

fn chip(label: String, i: usize, c: usize) -> El {
    row().key(format!("chip-{i}-{c}")).h(20.0).px(6.0).gap(4.0).items_center().radius(5.0)
        .fill(Color::WHITE.alpha(0.08)).hover(Color::WHITE.alpha(0.14)).on_press(|_: &mut State| {})
        .child(el().size(8.0, 8.0).radius(4.0).fill(rune::oklch(0.72, 0.12, 245.0)))
        .child(text(label).font(rune::TextStyle::new(12.0, 500.0)).color(Color::WHITE))
}

fn entry(s: &State, i: usize, note: &str) -> El {
    let small = rune::TextStyle::new(12.0, 400.0);
    let head = row().key("head").w_full().gap(8.0).items_center()
        .child(el().size(22.0, 22.0).radius(11.0).fill(rune::oklch(0.6, 0.14, 295.0)))
        .child(text(format!("Agent {}", i % 7)).font(rune::TextStyle::new(13.0, 600.0)).color(Color::WHITE))
        .child(text(format!("{}m ago", 3 + i)).font(small).color(Color::WHITE.alpha(0.5)))
        .child(row().key("chips").gap(4.0).children((0..3).map(|c| chip(format!("t{}", 400 + i * 3 + c), i, c))));
    let links = row().key("links").w_full().gap(6.0).flex_wrap()
        .children((0..4).map(|c| text(format!("#{} lantern-{c}", 1000 + i)).key(format!("link-{c}")).font(small)
            .color(rune::oklch(0.75, 0.1, 250.0)).on_press(|_: &mut State| {})));
    let fill = if i == s.marker { 0.07 } else { 0.04 };
    col().key(format!("entry-{i}")).w_full().gap(6.0).padding(10.0).radius(8.0)
        .fill(Color::WHITE.alpha(fill)).border(1.0, Color::WHITE.alpha(0.08))
        .on_drawn_layout_tx(|_: &mut State, _, tx: &mut Tx| tx.live_only())
        .child(head)
        .child(Markdown::new(note).render(&s.style).key("note"))
        .child(links)
}

const PROSE: &str = "Amber lantern orchard quiet harbor, copper meadow signal drift. Pebble willow \
    ember tide under a juniper kestrel; nimbus opal prism and a quiet harbor beneath the lantern. ";

const NAMES: &[&str] = &[
    "cpu_ms", "view_cpu_ms", "commit_cpu_ms", "layout_cpu_ms", "text_cpu_ms",
    "paint_cpu_ms", "apply_cpu_ms", "commits", "view_rebuilds",
    "instances", "allocations", "shaped",
];

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let value = |key: &str| args.iter().position(|a| a == key).and_then(|i| args.get(i + 1));
    let workload = value("--workload").expect("--workload list|markdown|sticky|growth|prose|card").clone();
    let frames: usize = value("--frames").map_or(480, |n| n.parse().unwrap());
    let fps: f64 = value("--fps").map_or(120.0, |n| n.parse().unwrap());
    assert!(frames > 0 && fps > 0.0);
    assert!(!cfg!(debug_assertions), "build with --profile perf");
    let source = (0..80).map(|i| format!(
        "## Section {i}\n\nAmber **lantern** orchard quiet harbor. Copper meadow signal drift. \
         Pebble willow ember tide, with a [reference](https://example.com) and `inline code`.\n\n\
         - Juniper kestrel lumen\n- Nimbus opal prism\n\n> A quiet harbor under the lantern.\n\n\
         ```rust\nlet lantern = meadow(42);\n```\n\n"
    )).collect();
    let notes = (0..60).map(|i| format!(
        "Checked the **lantern** build on `meadow-{i}`: the orchard step passes, the harbor step \
         still drifts by {i} px. See [the log](https://example.com/{i}) and the diff below.\n\n\
         - Juniper kestrel lumen\n- Nimbus opal prism with `copper::signal`\n- Pebble willow ember\n\n\
         ```rust\nfn lantern(meadow: u32) -> u32 {{\n    meadow * {i} + 1\n}}\n```\n\n\
         Next: quiet harbor under the lantern, then *copper* meadow."
    )).collect();
    let state = State { workload, rows: 1000, source, notes, marker: 0, style: MarkdownStyle::neutral(true), views: Cell::new(0) };
    let size = value("--size").map_or((760.0, 800.0), |v| {
        let (w, h) = v.split_once('x').expect("--size WxH");
        (w.parse().unwrap(), h.parse().unwrap())
    });
    let mut rt = Runtime::new(state, view, size, Env::new(true, 2.0));
    rt.frame(0.0);
    rt.pointer_move(380.0, 400.0);
    for _ in 0..120 { rt.frame(1.0 / fps); }
    eprintln!("scroll-bench elements {}", rt.elements().len());
    let mut samples = Vec::with_capacity(frames);
    for i in 0..frames {
        rune::take_frame_phases();
        let (commits, views, allocs) = (rt.stats.commits, rt.state.views.get(), procwatch::alloc::thread_allocations());
        let shaped = rt.text.shaped;
        let start = rune::thread_cpu_ms();
        // Four seconds at 120 Hz. Reverse halfway, remaining away from the edges.
        // Growth starts bottom-pinned and scrolls up first; add one row every 12 frames.
        if rt.state.workload == "growth" && i % 12 == 0 {
            rt.state.rows += 1;
            rt.invalidate();
        }
        if rt.state.workload == "card" {
            rt.state.marker = (rt.state.marker + 1) % rt.state.notes.len();
            rt.invalidate();
        }
        let direction = if (i < frames / 2) != (rt.state.workload == "growth") { -1.0 } else { 1.0 };
        rt.wheel(380.0, 400.0, direction * 12.0);
        rt.frame(1.0 / fps);
        let cpu = rune::thread_cpu_ms() - start;
        let allocations = procwatch::alloc::thread_allocations() - allocs;
        let p = rune::take_frame_phases();
        let q = rune::PHASES;
        let builds = (rt.state.views.get() - views) as f64;
        samples.push([cpu, p[q], p[q+1], p[q+2], p[q+3], p[q+4], p[q+5],
            (rt.stats.commits - commits) as f64, builds,
            rt.instances.len() as f64, allocations as f64, (rt.text.shaped - shaped) as f64]);
    }
    let mut fields = Vec::new();
    for (column, name) in NAMES.iter().enumerate() {
        let mut values: Vec<_> = samples.iter().map(|r| r[column]).collect();
        values.sort_by(f64::total_cmp);
        let median = if frames % 2 == 0 { (values[frames/2 - 1] + values[frames/2]) / 2.0 } else { values[frames/2] };
        let mean = values.iter().sum::<f64>() / frames as f64;
        let p95 = values[((frames - 1) as f64 * 0.95).round() as usize];
        for (suffix, value) in [("p50", median), ("p95", p95), ("mean", mean)] {
            fields.push(format!("\"{name}_{suffix}\":{value:.9}"));
            println!("METRIC {name}_{suffix}={value:.9}");
        }
    }
    println!("rune-scroll-profile {{{}}}", fields.join(","));
}
