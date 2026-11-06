//! Rune's home page, drawn by Rune. The page is one Rune app in web document mode: the browser
//! scrolls a semantic mirror of it (real headings, paragraphs and links), and the canvas draws
//! the band in view. `--html` exports the same tree as the static first frame, so the page reads
//! before the wasm arrives and without it.

use std::time::Duration;

use rune::time::Instant;
use rune::{AppSpec, Cx, El, ImageCache, Tx, col, memo};

mod code;
mod hero;
mod pipeline;
mod sections;
mod springs;
mod tokens;
mod ui;
mod wordmark;

use sections::{HELLO, HOME, LIGHT, GLASS, METAL, SESSION, STREAMING, ready};
use tokens::*;
use wordmark::{Glints, Glyphs, Which};

pub const TITLE: &str = "Rune · A Rust UI framework with its own GPU renderer";
pub const REPO: &str = "https://github.com/anomalyco/rune";
pub const DOCS: &str = "https://github.com/anomalyco/rune/blob/dev/crates/rune/README.md";
/// The home page, relative to the site's host.
pub const BRAND: &str = "/rune/";
/// The commit the Cargo line pins: the one the site was built from.
pub const REV: &str = match option_env!("RUNE_REV") {
    Some(rev) => rev,
    None => "dev",
};
/// How long the copy button says "Copied".
const COPIED: f64 = 1.6;

pub struct State {
    glyphs: Glyphs,
    card: hero::Card,
    spring: springs::Demo,
    pipeline: pipeline::Pipeline,
    glints: Glints,
    images: Option<ImageCache>,
    copied_until: Option<f64>,
    /// The frame's time, in seconds since start, and when it was read.
    clock: (f64, Instant),
    not_found: bool,
}

impl State {
    fn new() -> State {
        State {
            glyphs: Glyphs::new(),
            card: hero::Card::default(),
            spring: springs::Demo::default(),
            pipeline: pipeline::Pipeline::default(),
            glints: Glints::default(),
            images: None,
            copied_until: None,
            clock: (0.0, Instant::now()),
            not_found: not_found(),
        }
    }

    /// Now, on the frame clock: for handlers, which run between frames.
    fn clock_now(&self) -> f64 {
        self.clock.0 + self.clock.1.elapsed().as_secs_f64()
    }

    fn copied(&mut self) {
        self.copied_until = Some(self.clock_now() + COPIED);
    }
}

/// The 404 page is exported natively with `RUNE_WWW_PAGE=404`; the browser never runs it.
fn not_found() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    return std::env::var("RUNE_WWW_PAGE").is_ok_and(|p| p == "404");
    #[cfg(target_arch = "wasm32")]
    false
}

fn poll(s: &mut State, t: f64, tx: &mut Tx) -> bool {
    s.clock = (t, Instant::now());
    let mut changed = false;
    let mut moving = false;
    let was = s.spring.running();
    let on = s.spring.tick(t);
    changed |= was || on;
    moving |= on;
    let was = s.pipeline.active();
    let on = s.pipeline.tick(t);
    changed |= was || on;
    moving |= on;
    let (glinted, glinting, next) = s.glints.tick(t);
    changed |= glinted;
    moving |= glinting;
    if let Some(next) = next {
        tx.wake_after(Duration::from_secs_f64((next - t).max(0.0)));
    }
    if let Some(until) = s.copied_until {
        if t >= until {
            s.copied_until = None;
            changed = true;
        } else {
            tx.wake_after(Duration::from_secs_f64(until - t));
        }
    }
    if moving {
        tx.wake_after(Duration::ZERO);
    }
    changed
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let bp = cx.layout_with("page", |r| Bp::of(r[2])).unwrap_or(Bp { at: 1000 });
    let reduced = cx.is_reduced_motion();
    let t = s.clock.0;
    let gutter = bp.gutter();
    let framed = |e: El| e.w_full().max_w(CONTENT + 2.0 * gutter).px(gutter);
    let main = if s.not_found { col().child(sections::not_found(bp)) } else { home(s, cx, bp, reduced) };
    col().key("page").w_full().fill(BG).items_center()
        .child(framed(sections::topbar(s, s.glints.glow(Which::Brand, t, reduced))))
        .child(framed(main.key("main").role("main")))
        .child(framed(sections::footer()))
}

fn home(s: &State, cx: &Cx<State>, bp: Bp, reduced: bool) -> El {
    let images = s.images.as_ref();
    let glow = s.glints.glow(Which::Masthead, s.clock.0, reduced);
    let springs = sections::springs_intro(bp)
        .child(springs::demo(s, cx, bp))
        .child(memo("retarget", bp, sections::retarget_code));
    let frame = sections::frame_intro(bp).child(pipeline::pipeline(&s.pipeline, cx.layout_size("pl"), bp, reduced));
    let (hello, _hello) = ready(images, cx, &[&HELLO]);
    let (gallery, _gallery) = ready(images, cx, &[&METAL, &GLASS, &LIGHT]);
    let (shots, _shots) = ready(images, cx, &[&SESSION, &HOME, &STREAMING]);
    col().w_full()
        .child(hero::hero(s, cx, bp, glow))
        .child(memo("views", (bp, hello), || sections::views(cx, images, bp)))
        .child(springs)
        .child(frame)
        .child(memo("renderer", (bp, gallery), || sections::gallery(cx, images, bp)))
        .child(memo("opencode", (bp, shots), || sections::opencode(cx, images, bp)))
        .child(memo("agents", bp, || sections::agents(bp)))
        .child(memo("platforms", bp, || sections::platforms(bp)))
        .child(sections::cta(bp, s.copied_until.is_some()))
}

pub fn spec() -> AppSpec<State> {
    AppSpec {
        families: ("Geist", "Geist Mono"),
        setup: Some(|s, waker| s.images = Some(ImageCache::new(waker))),
        poll: Some(poll),
        ..AppSpec::new(TITLE, (1200.0, 900.0), State::new, view)
    }
}

/// The page as an app: document mode, with the page's own font files.
pub fn app() -> rune::App<State> {
    rune::App::new(spec()).web_document().font_url("fonts/geist.ttf").font_url("fonts/geist-mono.ttf")
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen::prelude::wasm_bindgen(start))]
pub fn start() {
    #[cfg(target_arch = "wasm32")]
    if let Err(error) = app().try_run() {
        log_error(&error);
    }
    #[cfg(not(target_arch = "wasm32"))]
    app().run();
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console, js_name = error)]
    fn log_error(message: &str);
}

#[cfg(test)]
mod tests;
