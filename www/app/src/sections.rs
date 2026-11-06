//! The page below the hero: each section a heading that says its point, a paragraph, and the
//! code, demo or pictures that back it. The tree is the same at every width; only styles change
//! at the breakpoints, so the static page can switch them with `@media` rules.

use rune::{Color, Cx, El, Image, ImageCache, Span, SurfaceFit, TextStyle, Tx, col, el, rich, row, text};

use crate::code::{self, HELLO_TEST, HELLO_VIEW, RETARGET};
use crate::tokens::*;
use crate::ui::{Button, button, hairline, intro, linked, nav_link, prose};
use crate::wordmark::{Glow, wordmark};
use crate::{BRAND, DOCS, REPO, REV, State};

/// A section: its heading (an anchor at `id`) and what follows.
pub fn section(id: &str, title: &str, bp: Bp) -> El {
    let h2 = bp.h2();
    col().key(id).role("region").label(title).w_full().pt(if bp.phone() { 32.0 } else { 64.0 }).pb(32.0)
        .child(text(title.to_owned()).key(format!("{id}-h")).role("heading").level(2)
            .font(h2).color(fg()).wrap().selectable(true).max_w(h2.size * 22.0))
}

/// `e` across the whole width when the parent stacks, else a share `grow` of the row.
pub fn fill(e: El, stacked: bool, grow: f32) -> El {
    if stacked { e.w_full() } else { e.w(0.0).grow(grow) }
}

pub struct Picture<'a> {
    pub name: &'a str,
    /// The widths, in pixels, it was exported at: `img/{name}-{width}.avif`.
    pub widths: &'a [u32],
    /// How wide it shows, at most, in points: the guess before layout.
    pub pt: f32,
    pub alt: &'a str,
    /// Width over height, so the page keeps its place before the image arrives.
    pub aspect: f32,
}

impl Picture<'_> {
    /// The file for the width it shows at, from every size there is. It plans for at most 2.2
    /// times its box: past that the extra pixels of a larger file are bytes the browser decodes
    /// and Rune mip-maps and uploads on the main thread, and a 345-point phone tile at `@3x`
    /// can't tell 760 pixels from 1520.
    fn image(&self, images: Option<&ImageCache>, cx: &Cx<State>) -> Option<Image> {
        let shown = cx.layout_size(self.name).map_or(self.pt, |s| s[0]);
        let scale = cx.env().scale.max(1.0);
        let plan = shown.min(shown * 2.2 / scale);
        let urls: Vec<(String, u32)> = self.widths.iter().map(|&w| (format!("img/{}-{w}.avif", self.name), w)).collect();
        let sources: Vec<(&str, u32)> = urls.iter().map(|(u, w)| (u.as_str(), *w)).collect();
        images.map(|c| c.url_set(&sources, plan, scale))
    }

    /// The image as an element, `fit` into its box.
    pub fn el(&self, images: Option<&ImageCache>, cx: &Cx<State>, fit: SurfaceFit) -> El {
        let e = el().key(self.name.to_owned()).role("image").label(self.alt).w_full();
        match self.image(images, cx) {
            Some(img) => e.pixels_fit(img.pixels(), fit),
            None => e,
        }
    }
}

/// Which file each picture in `pics` shows and whether it has arrived, for a memo's version.
/// Hold the handles while the memo builds, so its elements find the same images.
pub fn ready(images: Option<&ImageCache>, cx: &Cx<State>, pics: &[&Picture]) -> (Vec<(Option<String>, bool)>, Vec<Image>) {
    let held: Vec<Image> = pics.iter().filter_map(|p| p.image(images, cx)).collect();
    (held.iter().map(|i| (i.pixels().source_url().map(str::to_owned), i.is_ready())).collect(), held)
}

pub const HELLO: Picture = Picture {
    name: "hello",
    widths: &[320, 640],
    pt: 320.0,
    alt: "A dark window reading “Lights on” above a blue switch that is on",
    aspect: 1.6,
};
pub const METAL: Picture =
    Picture { name: "metal", widths: &[760, 1520], pt: 740.0, alt: "A ball of liquid chrome on black, reflecting a cool studio light", aspect: 16.0 / 9.0 };
pub const GLASS: Picture = Picture {
    name: "glass",
    widths: &[380, 760],
    pt: 360.0,
    alt: "A frosted glass tile over a blue disc, sharp beside the glass and soft through it",
    aspect: 16.0 / 9.0,
};
pub const LIGHT: Picture =
    Picture { name: "light", widths: &[380, 760], pt: 360.0, alt: "Nine raised keys under one soft light, the middle one lit blue", aspect: 16.0 / 9.0 };
pub const SESSION: Picture = Picture {
    name: "opencode-session",
    widths: &[720, 1120, 1680, 2240],
    pt: 1120.0,
    alt: "OpenCode drawn by Rune: a session with a sidebar of projects and a conversation with code",
    aspect: 2560.0 / 1640.0,
};
pub const HOME: Picture =
    Picture { name: "opencode-home", widths: &[560, 1100], pt: 548.0, alt: "OpenCode drawn by Rune: the home screen with a prompt field", aspect: 2560.0 / 1640.0 };
pub const STREAMING: Picture =
    Picture { name: "opencode-streaming", widths: &[560, 1100], pt: 548.0, alt: "OpenCode drawn by Rune: a reply streaming in", aspect: 2560.0 / 1640.0 };

/// The header: the wordmark home and two links.
pub fn topbar(s: &State, glow: Glow) -> El {
    row().key("top").role("banner").w_full().h(TOPBAR).justify_between().center_y()
        .child(row().key("brand").href(BRAND).label("Rune home").h(28.0).center_y()
            .on_hover_change(|s: &mut State, on: bool, _tx: &mut Tx| if on { let now = s.clock_now(); s.glints.brand(now) })
            .child(wordmark(&s.glyphs, 2.0, glow)))
        .child(row().key("nav").role("navigation").label("Rune").gap(24.0)
            .child(nav_link("nav-guide", "Guide", DOCS, TEXT_14))
            .child(nav_link("nav-github", "GitHub", REPO, TEXT_14)))
}

pub fn views(cx: &Cx<State>, images: Option<&ImageCache>, bp: Bp) -> El {
    let stacked = bp.stack_split();
    let shot = col().key("shot").role("figure").gap(8.0)
        .child(HELLO.el(images, cx, SurfaceFit::Fill).aspect_ratio(HELLO.aspect).radius(R2))
        .child(prose("caption", "What it draws, rendered headless with `--snap`", TEXT_13, fg3()));
    let split = if stacked { col() } else { row() }.key("split").w_full().gap(24.0).mt(32.0)
        .child(fill(code::block("hello", &HELLO_VIEW), stacked, 1.0))
        .child(if stacked { shot.w_full().max_w(320.0) } else { shot.w(320.0) });
    section("views", "A view is a plain Rust function of your state.", bp)
        .child(intro("views-p", "There is no markup and no macro language. `view` reads the state and returns elements; a handler changes the state and Rune draws what follows. Give an element `.motion()` and it moves to its new layout on a spring."))
        .child(split)
}

pub fn springs_intro(bp: Bp) -> El {
    section("springs", "Every animation is a spring that keeps its speed when you change your mind.", bp)
        .child(intro("springs-p", "Springs take SwiftUI's numbers, a duration and a bounce, and are solved exactly, so a retarget starts from where the value is and how fast it's going. Nothing animates on the first frame, a finger moves things directly, and reduced motion swaps travel for a short fade."))
}

pub fn retarget_code() -> El {
    code::block("retarget", &RETARGET).mt(32.0)
}

pub fn frame_intro(bp: Bp) -> El {
    section("frame", "Every frame takes the same short path, from your input to one draw call.", bp)
        .child(intro("frame-p", "Input changes your state. Rune calls your view, diffs it by key against the last one, lays it out with Taffy, moves what changed on springs and draws the whole window as one batch of instanced quads. At 120\u{a0}Hz all of it has 8.33\u{a0}ms, and an idle window draws nothing."))
}

/// A gallery tile: the render fading into the page's black at its edges, its caption under it.
fn tile(cx: &Cx<State>, images: Option<&ImageCache>, p: &Picture, caption: &str, frame: impl FnOnce(El) -> El) -> El {
    let key = format!("{}-frame", p.name);
    let [w, h] = cx.layout_size(&key).unwrap_or([360.0, 202.5]);
    let vignette = el().key("vignette").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .radial_gradient([0.5, 0.5], [w / 2.0, h / 2.0], [(0.58, Color::TRANSPARENT), (1.0, Color::BLACK)])
        .a11y_hidden();
    let img = p.el(images, cx, SurfaceFit::Fill).h_full();
    col().key(format!("{}-tile", p.name)).role("figure").w_full()
        .child(frame(el().key(key).w_full().radius(R2).clip_rounded(R2).child(img).child(vignette)))
        .child(text(caption.to_owned()).key("caption").font(TEXT_13).color(fg3()).w_full().align(rune::Align::Center))
}

pub fn gallery(cx: &Cx<State>, images: Option<&ImageCache>, bp: Bp) -> El {
    let stacked = bp.stack_gallery();
    let metal = tile(cx, images, &METAL, "Liquid metal", |f| if stacked { f.aspect_ratio(METAL.aspect) } else { f.grow(1.0) });
    let small = |p: &Picture, caption: &str| tile(cx, images, p, caption, |f| f.aspect_ratio(p.aspect));
    let side = col().key("side").gap(24.0).child(small(&GLASS, "Frosted glass")).child(small(&LIGHT, "Raised keys under one light"));
    let body = if stacked { col() } else { row() }.key("gallery").w_full().gap(24.0).mt(32.0)
        // Two of three columns and the gap between them: 24 + 2/3 of what's left after both gaps.
        .child(if stacked { metal.w_full() } else { metal.w(24.0).grow(2.0) })
        .child(fill(side, stacked, 1.0));
    section("renderer", "Metal, glass and light are drawn by the same renderer.", bp)
        .child(intro("renderer-p", "They're GPU shaders, drawn live. These stills come straight out of Rune."))
        .child(body)
        // The pictures are megabytes to read back and upload: they're asked for when the section
        // comes on screen, not while the reader is still on the hero.
        .on_visible(|s: &mut State, on: bool, _: &mut Tx| if on { s.seen.insert("renderer"); })
}

fn screen(cx: &Cx<State>, images: Option<&ImageCache>, p: &Picture) -> El {
    let img = p.el(images, cx, SurfaceFit::Fill).aspect_ratio(p.aspect);
    col().key(format!("{}-fig", p.name)).role("figure").w_full().radius(R2).clip_rounded(R2)
        .child(img)
        .child(el().key("outline").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).radius(R2).border(1.0, line()).a11y_hidden())
}

pub fn opencode(cx: &Cx<State>, images: Option<&ImageCache>, bp: Bp) -> El {
    let stacked = bp.stack_gallery();
    let pair = if stacked { col() } else { row() }.key("pair").w_full().gap(24.0)
        .child(fill(screen(cx, images, &HOME), stacked, 1.0))
        .child(fill(screen(cx, images, &STREAMING), stacked, 1.0));
    section("opencode", "OpenCode's desktop app is the test Rune is built against.", bp)
        .child(intro("opencode-p", "Rune's first big job is rebuilding OpenCode's desktop app and raising its bar: long streaming sessions, markdown and code, window chrome and scroll momentum. These screens are drawn by Rune."))
        .child(col().key("screens").w_full().gap(24.0).mt(32.0).child(screen(cx, images, &SESSION)).child(pair))
        .on_visible(|s: &mut State, on: bool, _: &mut Tx| if on { s.seen.insert("opencode"); })
}

const FACTS: [(&str, &str); 3] = [
    ("A reference renderer.", "`rune-ref` draws the same scenes on the CPU, and golden tests hold the GPU within two levels per channel of it."),
    ("A motion audit.", "Example apps run scripted scenarios headless at 120\u{a0}fps, and a checker flags snaps, pops, jumps and retargets that lose their speed."),
    ("Over 2,600 tests", "across the framework, its components and its motion."),
];

pub fn agents(bp: Bp) -> El {
    let stacked = bp.stack_split();
    let mut facts = if stacked { col() } else { row() }.key("facts").role("list").w_full().gap(24.0).mt(32.0);
    for (i, (lead, rest)) in FACTS.into_iter().enumerate() {
        let item = col().key(format!("fact-{i}")).role("listitem").gap(4.0)
            .child(text(lead).key("lead").font(TEXT_14_MEDIUM).color(fg()).wrap().selectable(true))
            .child(prose("rest", rest, TEXT_14, fg2()));
        facts = facts.child(fill(item, stacked, 1.0));
    }
    section("agents", "An agent can build, run and check a Rune app without a screen.", bp)
        .child(intro("agents-p", "Tests drive the real runtime: tap an element by key, step the clock, read where things are. Any app renders headless to a PNG with `--snap`, to a strip of frames with `--frames`, and prints its element tree with `--tree`."))
        .child(code::block("hello-test", &HELLO_TEST).mt(32.0))
        .child(facts)
}

const PLATFORMS: [(&str, &str); 5] = [
    ("macOS", "Rune's own Metal backend"),
    ("Windows", "wgpu; windows stay hidden until their first frame"),
    ("Linux", "wgpu on X11 and Wayland"),
    ("iOS", "Embedded in UIKit or SwiftUI"),
    ("Web", "WebGPU on one canvas, experimental"),
];

pub fn platforms(bp: Bp) -> El {
    let stacked = bp.stack_split();
    let mut list = if stacked { col() } else { row() }.key("platforms-list").role("list").w_full().mt(32.0).radius(R2).border(1.0, line());
    for (i, (name, note)) in PLATFORMS.into_iter().enumerate() {
        let item = if stacked { row().justify_between().center_y().gap(16.0) } else { col().gap(4.0) };
        let item = fill(item, stacked, 1.0);
        let mut item = item.key(name.to_lowercase()).role("listitem").padding(16.0)
            .child(text(name).key("name").font(TEXT_16_MEDIUM).color(fg()))
            .child(text(note).key("note").font(TEXT_13).color(fg3()).wrap().shrink().align(if stacked { rune::Align::End } else { rune::Align::Start }));
        if i > 0 {
            // The hairline between this platform and the last, inside the item so the list holds only items.
            let rule = el().key("rule").fill(line()).a11y_hidden();
            let rule = if stacked { rule.pin(Some(0.0), Some(0.0), Some(0.0), None).h(1.0) } else { rule.pin(Some(0.0), Some(0.0), None, Some(0.0)).w(1.0) };
            item = item.child(rule);
        }
        list = list.child(item);
    }
    section("platforms", "The same code runs on macOS, Windows, Linux, iOS and the web.", bp).child(list)
}

/// The Cargo line, as the CTA shows and copies it.
pub fn cargo_line() -> String {
    format!("rune = {{ git = \"{REPO}\", rev = \"{REV}\" }}")
}

fn cargo_spans(line: &str) -> Vec<Span> {
    let mut spans = vec![Span::new(0, 4).color(code_call())];
    let mut in_string = None;
    for (i, c) in line.char_indices() {
        match (c, in_string) {
            ('"', None) => in_string = Some(i),
            ('"', Some(start)) => {
                spans.push(Span::new(start, i + 1).color(code_string()));
                in_string = None;
            }
            ('=' | '{' | '}' | ',', None) => spans.push(Span::new(i, i + 1).color(fg2())),
            _ => {}
        }
    }
    spans
}

pub fn cta(bp: Bp, copied: bool) -> El {
    let cargo = cargo_line();
    let copy = row().key("copy").role("button").label(if copied { "Copied" } else { "Copy" }).focusable(true)
        .h(28.0).px(12.0).mr(8.0).radius(14.0).center_y().fill(surface2()).hover(Color::WHITE.alpha(0.04))
        .on_press_tx(|s: &mut State, tx: &mut Tx| {
            tx.copy(cargo_line());
            s.copied();
        })
        .child(text(if copied { "Copied" } else { "Copy" }).font(TEXT_12_MEDIUM).color(fg2()).a11y_hidden());
    let copyline = row().key("copyline").w_full().max_w(832.0).mt(32.0).center_y().radius(R2).fill(surface1()).border(1.0, line())
        .child(col().key("scroll").w(0.0).grow(1.0).min_w(0.0).scroll_x().scroll_edge_fade(24.0)
            .child(rich(cargo.clone(), cargo_spans(&cargo)).key("cargo").font(MONO_13).color(fg()).selectable(true).px(16.0).py(12.0)))
        .child(copy);
    section("cta", "Pin a commit and start with one view.", bp).pb(96.0)
        .child(intro("cta-p", "Rune isn't on crates.io yet. Add it from Git at a commit, then start from `examples/hello`."))
        .child(copyline)
        .child(row().key("actions").flex_wrap().gap(12.0).mt(32.0)
            .child(button("cta-guide", "Read the guide", DOCS, Button::Primary))
            .child(button("cta-examples", "Browse the examples", &format!("{REPO}/tree/dev/examples"), Button::Secondary)))
}

pub fn footer() -> El {
    row().key("footer").role("contentinfo").w_full().justify_between().flex_wrap().gaps(8.0, 24.0).py(32.0)
        .child(hairline("rule").pin(Some(0.0), Some(0.0), Some(0.0), None))
        .child(text("Rune is open source under the MIT license.").key("license").font(TEXT_13).color(fg3()).selectable(true))
        .child(linked("made", "Made by [Anomaly](https://anoma.ly).", TEXT_13, fg3()))
}

/// The page for a path that isn't there.
pub fn not_found(bp: Bp) -> El {
    col().key("notfound").w_full().py(96.0).min_h(480.0)
        .child(text("There's no page here.").key("notfound-h").role("heading").level(1).font(TextStyle::new(40.0, 500.0).lh(48.0).tracking(-1.0)).color(fg()).wrap())
        .child(linked("notfound-p", &format!("Try [Rune's home page]({BRAND}) or the [code on GitHub]({REPO})."), bp.lede(), fg2()).mt(24.0))
}
