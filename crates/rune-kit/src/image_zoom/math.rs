//! Zoom and pan of a fitted image inside a clipping viewport (`@opencode/ui` `image-zoom.ts`).
//!
//! `scale` is relative to the fitted size, so 1 is "fit". `x` and `y` offset the image's centre
//! from the viewport's centre in points, and anchors use the same viewport-centre coordinates.

/// Upper zoom limit, relative to the image's natural pixel size.
const MAX_NATURAL_ZOOM: f32 = 8.0;

/// One zoom level step of the preview's buttons and keys (`image-preview.tsx` `STEP`).
pub const STEP: f32 = std::f32::consts::SQRT_2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoomView {
    pub scale: f32,
    pub x: f32,
    pub y: f32,
}

pub const FIT_VIEW: ZoomView = ZoomView { scale: 1.0, x: 0.0, y: 0.0 };

/// The fitted image and the viewport it zooms in, in points.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ZoomBounds {
    /// The image's fitted size, before any zoom.
    pub width: f32,
    pub height: f32,
    pub viewport_width: f32,
    pub viewport_height: f32,
    pub max_scale: f32,
}

/// The largest fitted-relative scale, given how far the fitted image is shrunk from its natural
/// size.
pub fn max_scale(fit_ratio: f32) -> f32 {
    (MAX_NATURAL_ZOOM / fit_ratio).max(1.0)
}

/// Scales the view while the image point under `anchor` stays where it is on screen.
pub fn zoom_at(view: ZoomView, scale: f32, anchor: [f32; 2], bounds: &ZoomBounds) -> ZoomView {
    let next = scale.clamp(1.0, bounds.max_scale.max(1.0));
    let ratio = next / view.scale;
    clamp_view(
        ZoomView { scale: next, x: anchor[0] - (anchor[0] - view.x) * ratio, y: anchor[1] - (anchor[1] - view.y) * ratio },
        bounds,
    )
}

pub fn pan_by(view: ZoomView, delta: [f32; 2], bounds: &ZoomBounds) -> ZoomView {
    clamp_view(ZoomView { scale: view.scale, x: view.x + delta[0], y: view.y + delta[1] }, bounds)
}

/// Keeps the zoomed image covering the viewport on each axis it overflows, and centred on each
/// axis it fits.
pub fn clamp_view(view: ZoomView, bounds: &ZoomBounds) -> ZoomView {
    ZoomView {
        scale: view.scale,
        x: clamp_axis(view.x, bounds.width * view.scale, bounds.viewport_width),
        y: clamp_axis(view.y, bounds.height * view.scale, bounds.viewport_height),
    }
}

/// The double-click target: 100% for a downscaled image, or twice the fitted size when 100% is
/// barely larger.
pub fn detail_scale(fit_ratio: f32) -> f32 {
    let actual = 1.0 / fit_ratio;
    if actual >= 1.5 { actual } else { 2.0 }
}

/// The zoom factor for one wheel event of `delta_y` (a browser's sign: positive scrolls down),
/// bounded so a coarse mouse-wheel notch is a step rather than a jump.
pub fn wheel_zoom_factor(delta_y: f32) -> f32 {
    (-delta_y.clamp(-30.0, 30.0) / 100.0).exp()
}

/// Keeps one axis of a zoomed image of `size` covering a `viewport` it overflows, and centred in
/// one it fits.
pub fn clamp_axis(offset: f32, size: f32, viewport: f32) -> f32 {
    let limit = ((size - viewport) / 2.0).max(0.0);
    // Adding zero turns -0 into 0.
    offset.clamp(-limit, limit) + 0.0
}
