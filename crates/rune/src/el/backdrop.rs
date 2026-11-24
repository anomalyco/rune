//! Backdrops: system materials behind the window (`El::system_material`) and blur of the
//! window's own content (`El::backdrop_blur`).
use crate::El;

impl El {
    /// Back this node with a system backdrop material: the platform blurs what is behind the
    /// window under the node's rect and corner radius (the largest of its corners), which follow
    /// layout and animation like any other paint. Native on macOS in transparent windows, where
    /// it replaces the fill and the node's shadows skip its interior; elsewhere, in opaque
    /// windows and in headless renders, the fill (or a neutral tint without one) draws instead.
    /// Native materials sit under everything the window draws and ignore ancestor clips.
    /// `Glass` also turns on `continuous_corners`, the only curve `NSGlassEffectView` draws, so
    /// the fill, clip and shadow hole match the glass; `Popover` and `Sidebar` follow
    /// `continuous_corners`. A window hosts at most [`App::max_native_materials`](crate::App::max_native_materials)
    /// native views (default [`MAX_NATIVE_MATERIALS`](crate::MAX_NATIVE_MATERIALS)); later visible
    /// elements draw their fallback and report [`ElementInfo::material_fallback`](crate::ElementInfo::material_fallback).
    pub fn system_material(mut self, material: crate::SystemMaterial) -> Self {
        if material == crate::SystemMaterial::Glass { self.vis_flags.set(crate::el::VisualFlags::CONTINUOUS_CORNERS, true); }
        self.cold_mut().material = Some(material);
        self
    }

    /// Blurs what is already drawn behind this element in the same window, inside its rounded
    /// (or continuous) corners and clip, then draws the element's fill over the blur as a tint,
    /// with its border and children on top: `el().backdrop_blur(8.0).fill(tint)` is a frosted
    /// bar. `sigma` is the Gaussian standard deviation in points, as in CSS
    /// `backdrop-filter: blur()`; 0 turns it off. Blurs past about 78 points at 2x stop growing.
    ///
    /// Unlike `El::system_material`, which shows what is behind the window, this blurs Rune's
    /// own content and looks the same on every platform. The element's own opacity (including its
    /// default enter and exit fade) fades the blur with it while still blurring what sits behind
    /// the element, and its outer shadows leave the blurred interior clear. Blurs drawn into a
    /// target that can't be copied (no `COPY_SRC`, or a caller's own `wgpu::RenderPass` via
    /// `Renderer::draw_range`) draw only their fill. Inside an ancestor drawn as one layer (an
    /// ancestor with opacity below 1 — including one playing a fade transition — `El::group_opacity`,
    /// `El::blur`, `El::cache` or a rotation) the backdrop is what that layer has drawn so far,
    /// matching CSS's Backdrop Root.
    pub fn backdrop_blur(mut self, sigma: f32) -> Self {
        let s = if sigma.is_finite() { sigma.max(0.0) } else { 0.0 };
        if s != 0.0 || !self.vis.cold.is_empty() {
            self.vis.cold_mut().backdrop_blur = s;
        }
        self
    }
}
