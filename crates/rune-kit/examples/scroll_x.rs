//! Horizontal scrolling: a filmstrip (`El::scroll_x`) above a wide code block and table
//! (`Markdown::scroll_wide`), in a page that scrolls vertically around them.
//! `cargo run -p rune-kit --example scroll_x --features markdown` opens the window. The strip in
//! `demo/kit/scroll-x.png` is a sideways swipe on the filmstrip, a mostly vertical one over it
//! (the page scrolls), then sideways swipes on the code block and the table:
//! `-- --script crates/rune-kit/examples/scroll_x.json --fps 60 --start 0.25 --frames 193 --every 48 --cols 5 --scale 1 --strip strip.png`
#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]
use rune::tok::type_;
use rune::{AppSpec, Cx, El, ScrollSpec, col, el, oklch, row, text};
use rune_kit::{Kit, Markdown};

const CODE: &str = "```rust
fn main() {
    let frames: Vec<Frame> = clips.iter().flat_map(|clip| clip.frames_between(start, end)).filter(|f| f.keyframe).collect();
    println!(\"{} keyframes across {} clips, first at {:?}\", frames.len(), clips.len(), frames.first().map(|f| f.at));
}
```";

const TABLE: &str = "| Clip | Codec | Resolution | Frame rate | Duration | Keyframe interval | Size |
|---|---|---|---|---|---|---|
| Harbour at dawn | HEVC Main 10 | 3840 × 2160 | 59.94 | 00:01:12 | 2 s | 812 MB |
| Night market | ProRes 422 HQ | 4096 × 2160 | 25 | 00:00:48 | every frame | 3.9 GB |
| Ridge line | AV1 | 2560 × 1440 | 30 | 00:02:05 | 4 s | 214 MB |";

fn view(_: &(), _: &mut Cx<()>) -> El {
    let k = Kit::neutral(true);
    let tile = |i: usize| {
        col().key(format!("tile{i}")).w(132.0).h(96.0).radius(10.0).padding(10.0).justify_end()
            .fill(oklch(0.62, 0.13, 25.0 + 31.0 * i as f32))
            .child(text(format!("Clip {}", i + 1)).font(type_::TITLE).color(rune::Color::WHITE))
    };
    let strip = col().key("strip").w_full().h(96.0).scroll_x().scroll_edge_fade(16.0)
        .child((0..12).fold(row().gap(10.0), |r, i| r.child(tile(i))));
    let md = |key: &str, src: &str| col().key(key).w_full().child(Markdown::new(src).scroll_wide(true).build(&k));
    let page = col().w_full().padding(20.0).gap(16.0)
        .child(text("Filmstrip").font(type_::TITLE).color(k.c.text_base))
        .child(strip)
        .child(md("code", CODE))
        .child(md("table", TABLE))
        .child(el().h(240.0));
    col().w_full().h_full().fill(k.c.page)
        .child(col().key("page").w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).child(page))
}

fn main() {
    rune::run(AppSpec::new("Horizontal scrolling", (520.0, 520.0), || (), view));
}
