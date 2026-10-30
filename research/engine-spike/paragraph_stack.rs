//! Throwaway spike for research/32-engine-plan.md ("Spike results"), included from
//! `crates/rune/src/text.rs` under `cfg(test)` on the `spike/engine` branch only.
//!
//! - `paragraphs_stack_like_one_layout`: one Parley layout per `\n` paragraph, stacked, against
//!   one layout of the whole text, over the rewrap matrix joined into multi-paragraph texts.
//! - `bench_edit_keystroke` (ignored): typing in the 50 KB `edit` note, whole text vs stacked.
//! - `bench_word_cache` (ignored): a per-word `parley_engine` shaper cache vs Parley paragraph
//!   shaping on real files.
//!
//! cargo test --profile perf -p rune --lib engine_spike -- --ignored --nocapture --test-threads=1
use super::*;
use crate::edit::Block;
use std::cell::Cell;

thread_local! {
    /// Overrides the base direction Parley otherwise takes from a layout's first strong character.
    pub(crate) static FORCE_DIR: Cell<Option<parley::BaseDirection>> = const { Cell::new(None) };
}

#[derive(Clone)]
struct Case { name: String, text: String, spans: Vec<Span>, blocks: Vec<Block>, pills: Vec<PillChip>, style: TextStyle, align: Alignment, wrap_anywhere: bool }

fn case(name: &str, text: &str, style: TextStyle) -> Case {
    Case { name: name.into(), text: text.into(), spans: vec![], blocks: vec![], pills: vec![], style, align: Alignment::Start, wrap_anywhere: false }
}

/// `rewrap_tests::cases`, plus an Arabic paragraph ending in digits (bidi weak types across `\n`).
fn cases() -> Vec<Case> {
    let body = TextStyle::new(15.0, 400.0).lh(22.0);
    let rich = "The quick brown fox jumps over the_lazy_dog and keeps running past \
        https://example.com/a/very/long/unbreakable/path/that/overflows/any/line while larger words sit beside it.";
    let at = |s: &str| rich.find(s).unwrap() as u32;
    let spans = vec![
        Span { start: at("jumps"), end: at("jumps") + 5, ..Span::default() }.bold(),
        Span { start: at("the_lazy_dog"), end: at("the_lazy_dog") + 12, ..Span::default() }.mono(),
        Span { start: at("keeps"), end: at("keeps") + 5, ..Span::default() }.italic(),
        Span { start: at("larger"), end: at("larger") + 12, ..Span::default() }.size(22.0),
    ];
    let paragraphs = "First a plain paragraph that wraps over a few lines of text.\nA bullet item long enough to wrap onto a second line at most widths.\nA quoted line that also wraps when the width narrows enough.\nlet code = run(\"a long line of code\");";
    let pill_text = "ask @code-reviewer about @src/main.rs and @notes today please, then wrap";
    let pills = ["@code-reviewer", "@src/main.rs", "@notes"].iter().map(|label| {
        let start = pill_text.find(label).unwrap() as u32;
        PillChip { start, end: start + label.len() as u32, icon: None, bg: None, fg: None }
    }).collect();
    vec![
        Case { spans: spans.clone(), ..case("spans", rich, body) },
        Case { spans: spans.clone(), wrap_anywhere: true, ..case("wrap anywhere", rich, body) },
        Case { spans: spans.clone(), align: Alignment::Center, ..case("centred", rich, body) },
        Case { spans: spans.clone(), align: Alignment::End, ..case("end", rich, body) },
        Case { spans, align: Alignment::Justify, ..case("justified", rich, body) },
        case("right to left", "مرحبا بالعالم هذا نص طويل يلتف عبر عدة أسطر في التخطيط، ثم שלום עולם וטקסט נוסף", body),
        Case { align: Alignment::Justify, ..case("right to left justified", "مرحبا بالعالم هذا نص طويل يلتف عبر عدة أسطر في التخطيط والمزيد من الكلمات هنا", body) },
        case("arabic then digits", "مرحبا بالعالم 2024", body),
        case("mixed fonts", "Mixed 中文字符和日本語のテキスト with emoji 🎉🚀 and math 𝑥² ∑ beside Latin words that wrap", body),
        case("mono tabs", "fn main() {\n\tlet x = 1;\t// a comment that runs long enough to wrap\n}", TextStyle::new(13.0, 400.0).family(Family::Mono)),
        Case { blocks: vec![Block::Paragraph, Block::Bullet(0), Block::Quote, Block::Code], ..case("blocks", paragraphs, body) },
        Case { pills, ..case("pills", pill_text, TextStyle::new(14.0, 400.0)) },
    ]
}

const PLAIN: &str = "123 then a plain paragraph between the others, long enough to wrap at the narrower widths.";

/// `c` joined with plain and empty paragraphs: `parts` of "case", "plain" and "empty".
fn join(c: &Case, parts: &[&str], trailing: bool) -> Case {
    let name = format!("{} [{}{}]", c.name, parts.join(","), if trailing { ",\\n" } else { "" });
    let mut out = Case { name, text: String::new(), spans: vec![], blocks: vec![], pills: vec![], ..c.clone() };
    for (i, part) in parts.iter().enumerate() {
        if i > 0 { out.text.push('\n'); }
        let at = out.text.len() as u32;
        if *part == "case" {
            out.spans.extend(c.spans.iter().map(|s| Span { start: s.start + at, end: s.end + at, ..s.clone() }));
            out.pills.extend(c.pills.iter().map(|p| PillChip { start: p.start + at, end: p.end + at, ..p.clone() }));
            out.blocks.extend((0..c.text.split('\n').count()).map(|k| c.blocks.get(k).copied().unwrap_or(Block::Paragraph)));
            out.text += &c.text;
        } else {
            out.text += if *part == "plain" { PLAIN } else { "" };
            out.blocks.push(Block::Paragraph);
        }
    }
    if trailing { out.text.push('\n'); }
    out
}

type Row = (&'static str, Vec<f64>);

/// The first `keep` lines of `l` as rows, moved down `y0` and along `byte0`, pill ids by `pill0`.
/// With `frac`, each line top rounds from `y0` plus the unrounded height above it, as breaking
/// the paragraph with Parley's `line_y` starting at `y0` would. Returns the kept lines' height.
fn rows(l: &Layout<Ink>, byte0: usize, (dx, y0): (f64, f64), pill0: u64, keep: usize, frac: bool, out: &mut Vec<Row>) -> f64 {
    let mut h = 0.0;
    for line in l.lines().take(keep) {
        let (m, r) = (line.metrics(), line.text_range());
        let shift = if frac { (y0 + h).round() - m.block_min_coord as f64 } else { y0 };
        let y = |v: f32| v as f64 + shift;
        h += m.line_height as f64;
        out.push(("line", vec![(r.start + byte0) as f64, (r.end + byte0) as f64, m.offset as f64 + dx, y(m.baseline), m.advance as f64,
            m.inline_min_coord as f64, y(m.block_min_coord), y(m.block_max_coord), m.line_height as f64]));
        for item in line.items() {
            match item {
                PositionedLayoutItem::GlyphRun(run) => {
                    let font = run.run().font();
                    let mut g = vec![font.data.id() as f64, font.index as f64, run.run().font_size() as f64, y(run.baseline()), run.offset() as f64 + dx];
                    g.extend(run.positioned_glyphs().flat_map(|p| [p.id as f64, p.x as f64 + dx, y(p.y)]));
                    out.push(("glyphs", g));
                }
                PositionedLayoutItem::InlineBox(b) => {
                    let id = if b.id & PILL_BOX != 0 { b.id + (pill0 << 1) } else { b.id };
                    out.push(("box", vec![id as f64, b.x as f64 + dx, y(b.y)]));
                }
            }
        }
    }
    h
}

fn whole(ts: &mut TextSystem, c: &Case, scale: f32, width: Option<f32>) -> (Vec<Row>, bool) {
    let l = ts.layout_blocks_inner(&c.text, &c.spans, &c.blocks, &c.pills, c.style, scale, width, c.align, c.wrap_anywhere, true);
    let mut out = vec![("size", vec![l.full_width() as f64, l.width() as f64, l.height() as f64, l.len() as f64])];
    rows(&l, 0, (0.0, 0.0), 0, l.len(), false, &mut out);
    (out, l.is_rtl())
}

/// `text` split after each `\n`; a paragraph keeps its `\n`, so its layout ends in an empty line.
fn split(text: &str) -> Vec<(usize, &str)> {
    let mut at = 0;
    let mut out: Vec<(usize, &str)> = text.split_inclusive('\n').map(|p| { at += p.len(); (at - p.len(), p) }).collect();
    if out.is_empty() { out.push((0, "")); }
    out
}

/// One layout per paragraph, stacked: each drops the empty line after its `\n` unless it is last.
/// With `fix`, line tops round as one layout's do (see `rows`), and at an unbounded width each
/// paragraph moves over to align in the widest paragraph rather than in its own widest line.
fn stacked(ts: &mut TextSystem, c: &Case, scale: f32, width: Option<f32>, fix: bool) -> Vec<Row> {
    let (mut out, mut y, mut size, mut pill0) = (Vec::new(), 0.0f64, [0.0f64; 4], 0u64);
    let paras = split(&c.text);
    let mut laid = Vec::new();
    for (k, &(start, text)) in paras.iter().enumerate() {
        let (s, e) = (start as u32, (start + text.len()) as u32);
        let spans: Vec<Span> = c.spans.iter().filter(|x| x.start < e && x.end > s)
            .map(|x| Span { start: x.start.max(s) - s, end: x.end.min(e) - s, ..x.clone() }).collect();
        let pills: Vec<PillChip> = c.pills.iter().filter(|p| p.start >= s && p.end <= e)
            .map(|p| PillChip { start: p.start - s, end: p.end - s, ..p.clone() }).collect();
        let blocks = [c.blocks.get(k).copied().unwrap_or(Block::Paragraph)];
        let l = ts.layout_blocks_inner(text, &spans, &blocks, &pills, c.style, scale, width, c.align, c.wrap_anywhere, true);
        laid.push((start, l, pills.len() as u64));
    }
    let widest = laid.iter().map(|p| p.1.width()).fold(0.0f32, f32::max) as f64;
    for (k, (start, l, pills)) in laid.into_iter().enumerate() {
        let keep = if k + 1 < paras.len() { l.len() - 1 } else { l.len() };
        let share = match (c.align, l.is_rtl()) {
            (Alignment::Center, _) => 0.5,
            (Alignment::End, false) | (Alignment::Start | Alignment::Justify, true) => 1.0,
            _ => 0.0,
        };
        let dx = if fix && width.is_none() { (widest - l.width() as f64) * share } else { 0.0 };
        size[0] = size[0].max(l.full_width() as f64);
        size[1] = size[1].max(l.width() as f64);
        size[3] += keep as f64;
        y += rows(&l, start, (dx, y), pill0, keep, fix, &mut out);
        pill0 += pills;
    }
    size[2] = y;
    out.insert(0, ("size", size.to_vec()));
    out
}

/// Rows that differ, the largest difference in px, and the first differing pair.
fn diff(a: &[Row], b: &[Row]) -> (usize, f64, String) {
    let (mut n, mut max, mut first) = (a.len().abs_diff(b.len()), 0.0f64, String::new());
    if n > 0 { max = f64::INFINITY; first = format!("{} rows vs {}", a.len(), b.len()); }
    for (x, y) in a.iter().zip(b) {
        let d = if x.0 != y.0 || x.1.len() != y.1.len() { f64::INFINITY } else {
            x.1.iter().zip(&y.1).map(|(p, q)| (p - q).abs()).fold(0.0, f64::max)
        };
        if d > 0.0 {
            n += 1;
            if first.is_empty() {
                let short = |r: &Row| format!("{} {:?}", r.0, &r.1[..r.1.len().min(9)]);
                first = format!("{} | {}", short(x), short(y));
            }
            max = max.max(d);
        }
    }
    (n, max, first)
}

#[test]
fn paragraphs_stack_like_one_layout() {
    let mut ts = TextSystem::default();
    let shapes: [(&[&str], bool); 4] = [(&["case", "plain", "empty", "case"], false), (&["plain", "case"], false),
        (&["case", "plain", "empty", "case"], true), (&["case"], true)];
    let (mut runs, mut exact, mut ulp) = (0, [0; 3], [0; 3]);
    for c in cases() {
        for (parts, trailing) in shapes {
            let j = join(&c, parts, trailing);
            for scale in [1.0, 2.0] {
                for width in [Some(1200.0), Some(433.5), Some(210.0), None] {
                    for (m, mode) in ["auto", "forced", "forced+fix"].iter().enumerate() {
                        (ts.layouts.clear(), ts.shapes.clear());
                        let (one, rtl) = whole(&mut ts, &j, scale, width);
                        let dir = if rtl { parley::BaseDirection::Rtl } else { parley::BaseDirection::Ltr };
                        FORCE_DIR.set((m >= 1).then_some(dir));
                        (ts.layouts.clear(), ts.shapes.clear());
                        let many = stacked(&mut ts, &j, scale, width, m == 2);
                        FORCE_DIR.set(None);
                        let (n, max, first) = diff(&one, &many);
                        runs += usize::from(m == 0);
                        if n == 0 { exact[m] += 1 } else if max < 1e-3 { ulp[m] += 1 }
                        if max >= 1e-3 {
                            println!("MISMATCH {mode} {} @{scale}x w={width:?}: {n}/{} rows, max {max:.3} px\n    {first}", j.name, one.len());
                        }
                    }
                }
            }
        }
    }
    println!("SUMMARY runs={runs} auto: exact={} ulp={} | forced: exact={} ulp={} | forced+fix: exact={} ulp={}",
        exact[0], ulp[0], exact[1], ulp[1], exact[2], ulp[2]);
}

// The `edit` workload: the 50 KB note of `layout_text_cost` (copied from its generator).

const WORDS: &[&str] = &["amber", "lantern", "orchard", "quiet", "harbor", "copper", "meadow", "signal", "drift",
    "pebble", "willow", "ember", "tide", "cinder", "fable", "glacier", "hollow", "juniper", "kestrel", "lumen",
    "marrow", "nimbus", "opal", "prism", "quill", "ripple", "sable", "thistle", "umber", "vellum", "wander",
    "yarrow", "zephyr", "bramble", "cobalt", "dune", "fern", "grove", "heron", "isle", "the", "a", "of", "and",
    "to", "in", "with", "under", "over", "before"];

fn note(bytes: usize) -> String {
    let mut s = 0x9e37_79b9_7f4a_7c15u64;
    let mut r = |n: usize| { s ^= s << 13; s ^= s >> 7; s ^= s << 17; (s % n as u64) as usize };
    let w = |r: &mut dyn FnMut(usize) -> usize, n: usize| (0..n).map(|_| WORDS[r(WORDS.len())]).collect::<Vec<_>>().join(" ");
    let rich = |r: &mut dyn FnMut(usize) -> usize, n: usize| (0..n).map(|_| {
        let x = WORDS[r(WORDS.len())];
        match r(24) {
            0 => format!("**{x} {}**", WORDS[r(WORDS.len())]),
            1 => format!("*{x}*"),
            2 => format!("`{x}_{}`", r(100)),
            3 => format!("[{x}](https://example.com/{x})"),
            _ => x.to_string(),
        }
    }).collect::<Vec<_>>().join(" ");
    let (mut out, mut section) = (String::new(), 0);
    while out.len() < bytes {
        section += 1;
        out += &format!("# Section {section}: {}\n\n", w(&mut r, 3));
        for sub in 0..3 {
            out += &format!("## {} {sub}\n\n", w(&mut r, 2));
            for _ in 0..2 { let n = 25 + r(60); out += &rich(&mut r, n); out += ".\n\n"; }
            for i in 0..4 + r(5) {
                let n = 4 + r(10);
                let item = rich(&mut r, n);
                out += &if sub % 2 == 0 { format!("- {item}\n") } else { format!("{}. {item}\n", i + 1) };
            }
            out += "\n";
            match r(3) {
                0 => {
                    out += "```rust\n";
                    for i in 0..5 + r(6) {
                        let (name, call) = (WORDS[r(WORDS.len())], WORDS[r(WORDS.len())]);
                        out += &format!("    let {name}_{i} = {call}(\"{}\");\n", w(&mut r, 2));
                    }
                    out += "```\n\n";
                }
                1 => { let n = 12 + r(20); out += &format!("> {}\n\n", rich(&mut r, n)); }
                _ => { for _ in 0..3 { let n = 3 + r(6); out += &format!("- [ ] {}\n", rich(&mut r, n)); } out += "\n"; }
            }
        }
    }
    out
}

/// This thread's CPU clock, ms.
fn cpu_ms() -> f64 {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `t` is a valid timespec for the call to fill.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
    t.tv_sec as f64 * 1e3 + t.tv_nsec as f64 / 1e6
}

fn median(v: &mut [f64]) -> (f64, f64) {
    v.sort_by(f64::total_cmp);
    (v[v.len() / 2], v[((v.len() - 1) as f64 * 0.95).round() as usize])
}

/// Each keystroke types `w` mid-note, as `layout_text_cost --case edit` does, then lays out:
/// whole (one layout), naive (every paragraph through the layout cache) or incremental (only
/// the edited paragraph; the others keep their `Rc`). Modes alternate order every keystroke.
#[test]
#[ignore]
fn bench_edit_keystroke() {
    let src = note(50 * 1024);
    let style = TextStyle::new(15.0, 400.0).lh(22.0);
    let lay = |ts: &mut TextSystem, t: &str| ts.layout_blocks_inner(t, &[], &[], &[], style, 2.0, Some(696.0 * 2.0), Alignment::Start, false, true);
    struct Mode { ts: TextSystem, text: String, caret: usize, paras: Vec<(usize, Rc<Layout<Ink>>)>, cpu: Vec<f64>, peak: Vec<f64>, heap: isize, open: isize }
    let mut modes: Vec<Mode> = (0..3).map(|_| {
        let mut ts = TextSystem::default();
        lay(&mut ts, "Warm fonts and a few words");
        for _ in 0..3 { ts.end_commit(); }
        Mode { ts, text: src.clone(), caret: src[..src.len() / 2].rfind(' ').unwrap() + 1, paras: vec![], cpu: vec![], peak: vec![], heap: 0, open: 0 }
    }).collect();
    let step = |k: usize, m: &mut Mode, typed: bool| {
        if typed { m.text.insert(m.caret, 'w'); m.caret += 1; }
        match k {
            0 => { lay(&mut m.ts, &m.text); }
            1 => { let ls: Vec<_> = split(&m.text).into_iter().map(|(s, p)| (s, lay(&mut m.ts, p))).collect(); m.paras = ls; }
            _ if m.paras.is_empty() => { m.paras = split(&m.text).into_iter().map(|(s, p)| (s, lay(&mut m.ts, p))).collect(); }
            _ => {
                let i = m.paras.partition_point(|p| p.0 < m.caret) - 1;
                for p in &mut m.paras[i + 1..] { p.0 += 1; }
                let end = m.paras.get(i + 1).map_or(m.text.len(), |p| p.0);
                m.paras[i].1 = lay(&mut m.ts, &m.text[m.paras[i].0..end]);
            }
        }
        // The prefix index a composite keeps: each paragraph's top, from the heights above it.
        let mut y = 0.0f64;
        let tops: Vec<f64> = m.paras.iter().map(|p| { let t = y; y += p.1.height() as f64; t }).collect();
        std::hint::black_box(tops);
        m.ts.end_commit();
    };
    for (k, m) in modes.iter_mut().enumerate() {
        let live = crate::test_alloc::live();
        step(k, m, false);
        m.open = crate::test_alloc::live() - live;
        for _ in 0..3 { step(k, m, true); }
    }
    let mut mid = [0isize; 3];
    for i in 0..480 {
        if i == 120 { for k in 0..3 { mid[k] = modes[k].open + modes[k].heap; } }
        for j in 0..3 {
            let k = if i % 2 == 0 { j } else { 2 - j };
            let m = &mut modes[k];
            let (live, cpu) = (crate::test_alloc::live(), cpu_ms());
            let ((), peak) = crate::test_alloc::peak(|| step(k, m, true));
            m.cpu.push(cpu_ms() - cpu);
            m.heap += crate::test_alloc::live() - live;
            m.peak.push(peak as f64 / 1e6);
        }
    }
    let n = split(&modes[0].text).len();
    let edited = split(&modes[2].text).into_iter().filter(|p| p.0 < modes[2].caret).last().unwrap().1.len();
    println!("EDIT edited_paragraph_bytes={edited}");
    for (k, m) in modes.iter_mut().enumerate() {
        // Stale entries only: live paragraphs stay alive through `m.paras`.
        let live = crate::test_alloc::live();
        (m.ts.layouts.clear(), m.ts.shapes.clear());
        println!("EDIT mode={} stale_cache_mb={:.2}", ["whole", "naive", "incremental"][k], (live - crate::test_alloc::live()) as f64 / 1e6);
        let (cpu, cpu95) = median(&mut m.cpu);
        let (peak, _) = median(&mut m.peak);
        println!("EDIT mode={} paragraphs={n} cpu_ms median={cpu:.3} p95={cpu95:.3} open_heap_mb={:.2} heap_at_120_mb={:.2} heap_at_480_mb={:.2} keystroke_peak_mb={peak:.2} shaped={} cached_layouts={}",
            ["whole", "naive", "incremental"][k], m.open as f64 / 1e6, mid[k] as f64 / 1e6, (m.open + m.heap) as f64 / 1e6, m.ts.shaped, m.ts.layouts.len());
    }
}

// Word cache: Parley shaping and breaking each `\n` paragraph, against a cache of shaped words
// over `parley_engine` (shaped alone on a miss) with a greedy breaker at spaces.

struct OneFont<'a>(&'a parley_engine::FontInstance);
impl parley_engine::FontSelector for OneFont<'_> {
    fn select_font(&mut self, _: &parley_engine::itemize::Segment, _: &parley_engine::ShapeOptions<'_>,
        _: &mut parley_engine::shape::CharCluster) -> Option<parley_engine::FontInstanceRef<'_>> {
        Some(self.0.as_ref())
    }
}

#[derive(Default)]
struct Fx(u64);
impl Hasher for Fx {
    fn finish(&self) -> u64 { self.0 }
    fn write(&mut self, bytes: &[u8]) { for &b in bytes { self.0 = (self.0.rotate_left(5) ^ b as u64).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95); } }
}

#[derive(Default)]
struct Engine { analyzer: parley_engine::Analyzer, analysis: parley_engine::Analysis, shaper: parley_engine::Shaper, shaped: parley_engine::ShapedText, styles: Vec<u16> }
impl Engine {
    fn shape(&mut self, word: &str, font: &parley_engine::FontInstance, px: f32) -> (f32, Vec<parley_engine::Glyph>) {
        self.analyzer.analyze(word, &parley_engine::AnalysisOptions::default(), &mut self.analysis);
        let n = self.analysis.char_info().len();
        (self.styles.clear(), self.styles.resize(n, 0));
        let options = parley_engine::ShapeOptions { font_size: px, language: None, features: &[], variations: &[] };
        let item = parley_engine::itemize::Item { char_end: n as u32, options };
        self.shaper.shape_text(word, &self.analysis, &self.styles, [item], OneFont(font), &mut self.shaped);
        let glyphs = self.shaped.glyphs().to_vec();
        (glyphs.iter().map(|g| g.advance).sum(), glyphs)
    }
}

#[test]
#[ignore]
fn bench_word_cache() {
    let mut collection = Collection::new(CollectionOptions { shared: false, system_fonts: false });
    for data in [&include_bytes!("../../crates/rune/assets/Inter-Regular.ttf")[..], include_bytes!("../../crates/rune/assets/IBMPlexMono-Regular.ttf")] {
        collection.register_fonts(Blob::new(Arc::new(data)), None);
    }
    let mut fcx = FontContext { collection, source_cache: SourceCache::default() };
    let mut lcx = LayoutContext::<Ink>::new();
    let (scale, width) = (2.0f32, 696.0f32 * 2.0);
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../");
    for (file, family, size) in [("PLAN.md", "Inter", 15.0f32), ("CHANGELOG.md", "Inter", 15.0), ("crates/rune/src/runtime.rs", "IBM Plex Mono", 13.0)] {
        let all = std::fs::read_to_string(format!("{root}{file}")).unwrap();
        let text = &all[..all[..all.len().min(50 * 1024)].rfind('\n').unwrap()];
        let paras: Vec<&str> = text.split('\n').collect();
        let families = [FontFamilyName::named(family)];
        let mut parley = |lcx: &mut LayoutContext<Ink>, p: &str| {
            let mut b = lcx.ranged_builder(&mut fcx, p, scale, true);
            b.push_default(FontFamily::from(families.as_slice()));
            b.push_default(StyleProperty::FontSize(size));
            b.push_default(LineHeight::Absolute(size * 1.5));
            let mut l: Layout<Ink> = b.build(p);
            l.break_all_lines(Some(width));
            l
        };
        let probe = parley(&mut lcx, "x");
        let font = probe.lines().next().unwrap().runs().next().unwrap().font().clone();
        let font = parley_engine::FontInstance { font, synthesis: Default::default() };
        let px = size * scale;
        // Tokens and how many a per-word cache shapes: one per distinct word.
        let words: Vec<&str> = paras.iter().flat_map(|p| p.split(' ')).filter(|w| !w.is_empty()).collect();
        let distinct = words.iter().collect::<std::collections::HashSet<_>>().len();
        let mut engine = Engine::default();
        type Words = HashMap<Box<str>, (f32, Vec<parley_engine::Glyph>), std::hash::BuildHasherDefault<Fx>>;
        let mut warm: Words = HashMap::default();
        let cached = |engine: &mut Engine, analyze: bool, cache: &mut Words| {
            let space = engine.shape(" ", &font, px).0;
            let mut lines: Vec<(u32, f32)> = Vec::new();
            for p in &paras {
                if analyze { engine.analyzer.analyze(p, &parley_engine::AnalysisOptions::default(), &mut engine.analysis); }
                let mut x = 0.0f32;
                for (i, w) in p.split(' ').enumerate() {
                    let adv = if w.is_empty() { 0.0 } else if let Some(e) = cache.get(w) { e.0 } else {
                        let e = engine.shape(w, &font, px);
                        let adv = e.0;
                        cache.insert(w.into(), e);
                        adv
                    };
                    let next = if i == 0 { adv } else { x + space + adv };
                    if next > width && x > 0.0 { lines.push((i as u32, x)); x = adv } else { x = next }
                }
                lines.push((u32::MAX, x));
            }
            std::hint::black_box(&lines);
            cache.len()
        };
        let (mut t, mut parley_lines) = ([vec![], vec![], vec![], vec![]], 0);
        for round in 0..21 {
            for j in 0..4 {
                let k = if round % 2 == 0 { j } else { 3 - j };
                let cpu = cpu_ms();
                match k {
                    0 => parley_lines = paras.iter().map(|p| parley(&mut lcx, p).len()).sum(),
                    1 => { cached(&mut engine, false, &mut HashMap::default()); }
                    2 => { cached(&mut engine, true, &mut HashMap::default()); }
                    _ => { cached(&mut engine, true, &mut warm); }
                }
                if round > 0 { t[k].push(cpu_ms() - cpu); }
            }
        }
        let [a, b, c, d] = t.map(|mut v: Vec<f64>| median(&mut v).0);
        println!("WORDCACHE file={file} bytes={} paragraphs={} lines={parley_lines} tokens={} distinct={distinct} hit_rate={:.1}% \
            parley_ms={a:.2} cache_ms={b:.2} cache_plus_analysis_ms={c:.2} speedup={:.2}x speedup_with_analysis={:.2}x warm_ms={d:.2} warm_speedup={:.2}x",
            text.len(), paras.len(), words.len(), 100.0 * (1.0 - distinct as f64 / words.len() as f64), a / b, a / c, a / d);
    }
}
