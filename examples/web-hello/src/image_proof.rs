//! Browser image-decode fixture, selected only by ?image-proof: a JPEG the browser decodes and a
//! PNG Rune decodes, both from bytes, and the PNG again as the cursor over it.
use rune::{AppSpec, Cursor, CursorImage, Cx, El, Image, ImageCache, col, el};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = "
export function imageProof() { return new URLSearchParams(location.search).has('image-proof'); }
export function imageScaledProof() { return new URLSearchParams(location.search).has('image-scaled-proof'); }
export function imageReport(name, width, height, error) {
    (window.__imageProof ||= {})[name] = error ? { error } : { width, height };
}
export function imageScaledReport(name, width, height, iw, ih, error) {
    (window.__imageScaledProof ||= {})[name] = error ? { error } : { width, height, intrinsicWidth: iw, intrinsicHeight: ih };
}
")]
extern "C" {
    #[wasm_bindgen(js_name = imageProof)]
    pub fn enabled() -> bool;
    #[wasm_bindgen(js_name = imageScaledProof)]
    pub fn scaled_enabled() -> bool;
    #[wasm_bindgen(js_name = imageReport)]
    fn report(name: &str, width: u32, height: u32, error: &str);
    #[wasm_bindgen(js_name = imageScaledReport)]
    fn report_scaled(name: &str, width: u32, height: u32, iw: f32, ih: f32, error: &str);
}

const PNG: &[u8] = include_bytes!("../assets/proof.png");
const CURSOR: CursorImage = CursorImage::png(PNG, [1.0, 2.0]);

#[derive(Default)]
struct State {
    cache: Option<ImageCache>,
    images: Vec<(&'static str, Image)>,
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let mut out = col().w_full().h_full().padding(16.0).gap(8.0);
    for (name, image) in &s.images {
        out = out.child(el().key(*name).w(120.0).h(60.0).pixels(image.pixels())
            .cursor(Cursor::Image(CURSOR)).on_press(|_: &mut State| {}));
    }
    for (name, cursor) in [
        ("resize-ew", Cursor::ResizeEw),
        ("resize-ns", Cursor::ResizeNs),
        ("resize-nwse", Cursor::ResizeNwse),
        ("resize-nesw", Cursor::ResizeNesw),
    ] {
        out = out.child(el().key(name).w(120.0).h(40.0).hit().cursor(cursor));
    }
    out
}

pub fn run() {
    let mut spec = AppSpec::new("Image proof", (390.0, 760.0), State::default, view);
    spec.setup = Some(|s, waker| {
        let cache = s.cache.insert(ImageCache::new(waker));
        s.images = vec![
            ("jpeg", cache.load(&include_bytes!("../assets/proof.jpg")[..])),
            ("png", cache.load(PNG)),
        ];
    });
    spec.poll = Some(|s, _, _| {
        for (name, image) in &s.images {
            match (image.size(), image.error()) {
                (Some([w, h]), _) => report(name, w, h, ""),
                (_, Some(e)) => report(name, 0, 0, &e.0),
                _ => {}
            }
        }
        false
    });
    rune::run(spec);
}

const SMALL_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="8"><rect width="16" height="8" fill="#2060a0"/></svg>"##;
const WIDE_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="5000" height="1"><rect width="5000" height="1" fill="#2060a0"/></svg>"##;

pub fn run_scaled() {
    let mut spec = AppSpec::new("Image scaled proof", (390.0, 760.0), State::default, view);
    spec.setup = Some(|s, waker| {
        let cache = s.cache.insert(ImageCache::new(waker));
        s.images = vec![
            ("jpeg_scaled", cache.load_scaled(&include_bytes!("../assets/proof.jpg")[..], 3.0, 2.0)),
            ("jpeg_no_upscale", cache.load_scaled(&include_bytes!("../assets/proof.jpg")[..], 32.0, 2.0)),
            ("png_scaled", cache.load_scaled(PNG, 3.0, 2.0)),
            ("svg_load", cache.load(SMALL_SVG)),
            ("svg_wide", cache.load(WIDE_SVG)),
            ("svg_scaled", cache.load_scaled(SMALL_SVG, 32.0, 2.0)),
        ];
    });
    spec.poll = Some(|s, _, _| {
        for (name, image) in &s.images {
            match (image.size(), image.intrinsic_size(), image.error()) {
                (Some([w, h]), Some([iw, ih]), _) => report_scaled(name, w, h, iw, ih, ""),
                (_, _, Some(e)) => report_scaled(name, 0, 0, 0.0, 0.0, &e.0),
                _ => {}
            }
        }
        false
    });
    rune::run(spec);
}
