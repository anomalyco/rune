//! Display-only web images (`El::web_image`, `El::web_image_fit`): an opt-in browser `<img>`
//! layer synchronized with Rune's layout, scroll, animation and clipping on every frame without
//! reading pixels or requiring CORS headers.
//!
//! `ImageCache::url` fetches bytes and decodes pixels so Rune can draw images through the GPU
//! atlas (with full z-layering, shaders, tinting and tiling), which the browser blocks for
//! cross-origin URLs that omit `Access-Control-Allow-Origin`. `El::web_image` lets an app
//! display such URLs through an ordinary `<img>` element without `crossOrigin`: the browser keeps
//! its normal cross-origin restrictions and Rune never accesses the bitmap bytes.
//!
//! Because a DOM element cannot interleave between GPU draw calls inside one `<canvas>`, the
//! web image layer sits above the canvas (and below the accessibility mirror, with
//! `pointer-events: none` so all input reaches Rune normally). It suits avatars, thumbnails,
//! previews and media cards that do not need canvas-drawn popovers or shaders composited over
//! them. On native and headless runs nothing renders the DOM layer, so the element's own fill and
//! border stand in while layout and `Runtime::web_images` stay identical.

use std::rc::Rc;
use std::sync::Arc;

use rune_motion::NodeId;
use serde_json::{Map, Value, json};

use crate::el::El;
use crate::runtime::Runtime;

/// Stored on `VisualCold` for `El::web_image` and `El::web_image_fit`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WebImage {
    pub(crate) url: Arc<str>,
    pub(crate) fit: Option<crate::SurfaceFit>,
}

/// One enclosing rounded clip (`El::clip_rounded` or `El::clip_corners`) applied to a
/// [`WebImageDraw`], in CSS pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WebImageClip {
    /// Clip box `[x, y, width, height]` in CSS pixels.
    pub rect: [f32; 4],
    /// Corner radii `[top_left, top_right, bottom_right, bottom_left]` in CSS pixels.
    pub radii: [f32; 4],
}

/// One visible display-only web image (`El::web_image` or `El::web_image_fit`) built for the
/// current frame, in paint order and CSS pixels.
///
/// Coordinates are in viewport CSS pixels for full-viewport and inline canvases, and in document
/// CSS pixels for `App::web_document`.
#[derive(Clone, Debug, PartialEq)]
pub struct WebImageDraw {
    /// Stable node identity across frames, used by the web bridge to reuse the `<img>` element
    /// without re-requesting or flickering on scroll, resize or animation.
    pub id: NodeId,
    /// Validated image URL (`http:`, `https:`, or a relative path).
    pub url: Arc<str>,
    /// Drawn image box `[x, y, width, height]` in CSS pixels, inset inside any visible border so
    /// a border drawn on the canvas remains visible around the image.
    pub rect: [f32; 4],
    /// Corner radii `[top_left, top_right, bottom_right, bottom_left]` in CSS pixels.
    pub radii: [f32; 4],
    /// Effective rectangular clip `[x, y, width, height]` in CSS pixels when enclosing clips cut
    /// into the image's bounds, or `None` when the image is wholly inside its clip.
    pub clip: Option<[f32; 4]>,
    /// Enclosing rounded clips (`El::clip_rounded`, `El::clip_corners`) whose corner zones
    /// overlap the image, outermost to innermost.
    pub rounded_clips: Vec<WebImageClip>,
    /// Aspect-ratio fit (`None` stretches to `rect`, `Some(SurfaceFit::Fit)` maps to
    /// `object-fit: contain`, `Some(SurfaceFit::Fill)` maps to `object-fit: cover`). Ignored
    /// when `unscaled` is true.
    pub fit: Option<crate::SurfaceFit>,
    /// Horizontal and vertical anchor (`El::surface_anchor`, CSS `object-position`).
    pub anchor: [crate::Align; 2],
    /// Draw at intrinsic size without scaling (`El::surface_unscaled`, CSS `object-fit: none`).
    pub unscaled: bool,
    /// Nearest-neighbour sampling (`El::image_filter(ImageFilter::Nearest)`, CSS `pixelated`).
    pub nearest: bool,
    /// Composed opacity in `0.0..=1.0`.
    pub opacity: f32,
    /// Composed CSS 2D affine matrix `[a, c, b, d, e, f]` when rotated via `El::rotate`, or
    /// `None` when upright.
    pub transform: Option<[f32; 6]>,
}

impl El {
    /// Displays the raster image at `url` on web using a display-only `<img>` element without
    /// CORS or pixel readback, stretched to fill the element's box (matching [`El::pixels`]).
    ///
    /// Use this opt-in mode when loading cross-origin images whose server omits
    /// `Access-Control-Allow-Origin`: the browser keeps its normal cross-origin restrictions and
    /// Rune never reads the decoded pixels. For same-origin or CORS-enabled images that need GPU
    /// layering under overlays, shaders, tinting or tiling, prefer [`ImageCache::url`](crate::ImageCache::url)
    /// with [`El::pixels`].
    ///
    /// Combines with [`El::radius`], [`El::corners`], [`El::border`], [`El::fill`] (shown behind
    /// the image while loading, on error, or through transparency), [`El::opacity`],
    /// [`El::layer_opacity`], [`El::rotate`], [`El::image_filter`], [`El::surface_anchor`],
    /// [`El::surface_unscaled`], scroll viewports and enclosing rounded clips.
    ///
    /// Accepts `http:`, `https:`, and relative URLs (excluding scheme-relative `//...`). Invalid
    /// URLs are ignored cleanly. On native and headless targets the element lays out normally and
    /// draws its own fill and border without fetching `url`.
    ///
    /// **Stacking note:** on web the `<img>` layer sits above the canvas (with `pointer-events:
    /// none`), so canvas-drawn overlays, popovers or sheets cannot paint over it, and
    /// canvas-only effects (`El::blur`, `El::backdrop_blur`, `El::effect`, `El::pixels_tint`)
    /// do not apply to it.
    pub fn web_image(self, url: impl Into<String>) -> Self {
        self.set_web_image(url.into(), None)
    }

    /// Like [`El::web_image`], keeping the image's aspect ratio: [`SurfaceFit::Fit`](crate::SurfaceFit::Fit)
    /// maps to CSS `object-fit: contain`, and [`SurfaceFit::Fill`](crate::SurfaceFit::Fill) maps
    /// to CSS `object-fit: cover`. Combine with [`El::surface_anchor`] to set `object-position`.
    pub fn web_image_fit(self, url: impl Into<String>, fit: crate::SurfaceFit) -> Self {
        self.set_web_image(url.into(), Some(fit))
    }

    fn set_web_image(mut self, url: String, fit: Option<crate::SurfaceFit>) -> Self {
        let valid = crate::open_url::check_image_url(&url).ok();
        if valid.is_some() || !self.vis.cold.is_empty() {
            let cold = self.vis.cold_mut();
            cold.web_image = valid.map(|u| Rc::new(WebImage { url: Arc::from(u), fit }));
            if cold.web_image.is_some() {
                cold.pixels = None;
                cold.pixels_fit = None;
                cold.pixels_wrap = None;
            }
        }
        self
    }
}

impl<S> Runtime<S> {
    /// Visible [`El::web_image`] / [`El::web_image_fit`] elements from the last built frame, in
    /// paint order and CSS pixels.
    pub fn web_images(&self) -> &[WebImageDraw] {
        self.core().paint.web_images()
    }
}

/// Rounds `v` to 0.01 CSS pixels so bridge JSON stays compact and avoids float noise.
#[inline]
fn round2(v: f32) -> f64 {
    (f64::from(v) * 100.0).round() / 100.0
}

#[inline]
fn round4(v: f32) -> f64 {
    (f64::from(v) * 10_000.0).round() / 10_000.0
}

#[inline]
pub(crate) fn css_fit(fit: Option<crate::SurfaceFit>, unscaled: bool) -> &'static str {
    if unscaled {
        "none"
    } else {
        match fit {
            None => "fill",
            Some(crate::SurfaceFit::Fit) => "contain",
            Some(crate::SurfaceFit::Fill) => "cover",
        }
    }
}

#[inline]
pub(crate) fn css_position(anchor: [crate::Align; 2]) -> &'static str {
    match anchor {
        [crate::Align::Start, crate::Align::Start] => "0% 0%",
        [crate::Align::Start, crate::Align::Center] => "0% 50%",
        [crate::Align::Start, crate::Align::End] => "0% 100%",
        [crate::Align::Center, crate::Align::Start] => "50% 0%",
        [crate::Align::Center, crate::Align::Center] => "50% 50%",
        [crate::Align::Center, crate::Align::End] => "50% 100%",
        [crate::Align::End, crate::Align::Start] => "100% 0%",
        [crate::Align::End, crate::Align::Center] => "100% 50%",
        [crate::Align::End, crate::Align::End] => "100% 100%",
    }
}

/// Serializes `draws` for `bridge.js::syncWebImages`.
#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
pub(crate) fn to_json(draws: &[WebImageDraw]) -> String {
    if draws.is_empty() {
        return "[]".into();
    }
    let items: Vec<Value> = draws.iter().map(|d| {
        let mut obj = Map::with_capacity(8);
        obj.insert("id".into(), Value::String(d.id.0.to_string()));
        obj.insert("u".into(), Value::String(d.url.to_string()));
        obj.insert("r".into(), json!(d.rect.map(round2)));
        if d.radii.iter().any(|&v| v > 0.05) {
            obj.insert("rad".into(), json!(d.radii.map(round2)));
        }
        if let Some(c) = d.clip {
            obj.insert("c".into(), json!(c.map(round2)));
        }
        if !d.rounded_clips.is_empty() {
            let clips: Vec<Value> = d.rounded_clips.iter().map(|rc| {
                json!([
                    round2(rc.rect[0]), round2(rc.rect[1]), round2(rc.rect[2]), round2(rc.rect[3]),
                    round2(rc.radii[0]), round2(rc.radii[1]), round2(rc.radii[2]), round2(rc.radii[3]),
                ])
            }).collect();
            obj.insert("rc".into(), Value::Array(clips));
        }
        let fit = css_fit(d.fit, d.unscaled);
        if fit != "fill" {
            obj.insert("f".into(), Value::String(fit.into()));
        }
        let pos = css_position(d.anchor);
        if pos != "50% 50%" {
            obj.insert("p".into(), Value::String(pos.into()));
        }
        if d.nearest {
            obj.insert("n".into(), Value::Bool(true));
        }
        if d.opacity < 0.999 {
            obj.insert("o".into(), json!((f64::from(d.opacity) * 1000.0).round() / 1000.0));
        }
        if let Some(m) = d.transform {
            obj.insert("m".into(), json!([
                round4(m[0]), round4(m[1]), round4(m[2]), round4(m[3]), round2(m[4]), round2(m[5]),
            ]));
        }
        Value::Object(obj)
    }).collect();
    serde_json::to_string(&items).unwrap_or_else(|_| "[]".into())
}

#[cfg(test)]
#[path = "web_image_tests.rs"]
mod tests;

