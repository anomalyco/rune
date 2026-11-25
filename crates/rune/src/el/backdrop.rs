//! Backdrops: system materials behind the window (`El::system_material`) and blur of the
//! window's own content (`El::backdrop_blur`).
use crate::El;

impl El {
    /// Back this node with a system backdrop material: the platform blurs what is behind the
    /// window under the node's rect and corner radius (the largest of its corners), which follow
    /// layout and animation like any other paint.
    ///
    /// - **macOS**: native in transparent windows (`NSGlassEffectView` / `NSVisualEffectView`).
    ///   The native view sits behind the element, replaces its fill, ignores ancestor clips, and
    ///   punches a hole inside the node's outer shadows.
    /// - **Windows 11 Build 22000+**: opt-in behind the off-by-default `windows-system-material`
    ///   Cargo feature on transparent DirectComposition hardware-GPU windows and overlays when the
    ///   system permits effects (Transparency effects enabled, Battery/Energy Saver off,
    ///   composition effects supported/fast, High Contrast off, and not in a Remote Desktop
    ///   session). Uses Microsoft's Windows 11 Acrylic Luminosity + Color `CompositionEffectBrush`
    ///   over `HostBackdropBrush` (`Popover` = Default Acrylic, `Sidebar` = Base Acrylic,
    ///   `Glass` = clearer chromatic Acrylic), following light/dark theme switches and degrading to
    ///   a WCAG >=4.5:1 flat tint if the effect factory fails. Follows Wayland's fill rules:
    ///   opaque fills (`alpha >= 1.0`) stay opaque with no backdrop visual or shadow hole;
    ///   translucent fills draw above Acrylic as an authored tint; unfilled elements paint
    ///   transparent so Acrylic supplies the theme-aware tint. Whenever the system will not draw
    ///   the material, Rune tears down visuals and draws the element's fallback fill instead.
    /// - **Linux Wayland**: active on transparent GPU windows and overlays (`zwlr_layer_shell_v1`
    ///   and toplevel fallbacks) when [`WindowBackdrop::None`](crate::WindowBackdrop::None) is
    ///   active and the compositor advertises `ext_background_effect_v1` (`Capability::Blur`) or
    ///   `org_kde_kwin_blur_manager` (opt out with `RUNE_SYSTEM_MATERIAL=off`). Because Wayland
    ///   compositor blur has no tint of its own, Wayland mode keeps a translucent fill as the tint
    ///   (drawing transparent when no fill is set), punches the outer-shadow interior hole, clips
    ///   the blur region to ancestor clips and rounded corners, and leaves opaque fills
    ///   (`alpha >= 1.0`) unchanged without adding them to the blur region. Because Wayland blur
    ///   protocols have no opacity parameter, compositor blur is all-or-nothing under a fading
    ///   element: it stays at full strength until effective opacity drops below `0.10`, then turns
    ///   off. Respects [`App::max_native_materials`](crate::App::max_native_materials) (default 4;
    ///   elements beyond the limit draw their fallback fill).
    ///   Query [`Cx::window_backdrop_supported`](crate::Cx::window_backdrop_supported) with
    ///   [`WindowBackdrop::Blur`](crate::WindowBackdrop::Blur) to choose translucent vs opaque
    ///   fills with one check.
    /// - **Elsewhere**: in opaque windows, X11/XWayland, unsupported compositors, web, and
    ///   headless renders, the element's fill (or a neutral tint without one) draws instead. Under
    ///   [`El::cache`](Self::cache), a fallback-only material keeps reusing its cached raster and
    ///   replays its recorded rect (translated and scaled with the layer) into
    ///   [`Runtime::system_materials`](crate::Runtime::system_materials) and
    ///   [`ConfiguredOverlay::on_materials`](crate::ConfiguredOverlay::on_materials), while an
    ///   active native or Wayland blur mode draws the subtree live.
    ///
    /// `Glass` also turns on `continuous_corners`, the only curve `NSGlassEffectView` draws, so
    /// the fill, clip and shadow hole match the glass; `Popover` and `Sidebar` follow
    /// `continuous_corners`. A window hosts at most [`App::max_native_materials`](crate::App::max_native_materials)
    /// native views (default [`MAX_NATIVE_MATERIALS`](crate::MAX_NATIVE_MATERIALS)); later visible
    /// elements draw their fallback and report [`ElementInfo::material_fallback`](crate::ElementInfo::material_fallback).
    ///
    /// For a whole-window OS backdrop (Windows 11 Mica, Mica Alt, or Desktop Acrylic, or Linux
    /// Wayland whole-surface blur via [`WindowBackdrop::Blur`](crate::WindowBackdrop::Blur)), use
    /// [`WindowBackdrop`](crate::WindowBackdrop) instead. For an in-window blur of Rune's own
    /// content on every platform, use [`El::backdrop_blur`](Self::backdrop_blur).
    ///
    /// ```no_run
    /// use rune::{Color, Cx, El, SystemMaterial, WindowBackdrop, el};
    ///
    /// fn launcher_card(_: &(), cx: &mut Cx<()>) -> El {
    ///     let base = rune::tok::theme(cx.system_dark()).bg;
    ///     let bg = if cx.window_backdrop_supported(WindowBackdrop::Blur) {
    ///         base.alpha(0.75)
    ///     } else {
    ///         base
    ///     };
    ///     el().w_full().h_full().center().child(
    ///         el().size(750.0, 474.0).radius(12.0)
    ///             .shadow(16.0, 40.0, Color::BLACK.alpha(0.35))
    ///             .fill(bg)
    ///             .system_material(SystemMaterial::Popover),
    ///     )
    /// }
    /// ```
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
    /// Unlike [`El::system_material`](Self::system_material) (per-element behind-window blur on
    /// macOS, Linux Wayland, and opt-in Windows 11) and [`WindowBackdrop`](crate::WindowBackdrop)
    /// (whole-window OS backdrop on Windows 11 and Linux Wayland), this blurs Rune's own content
    /// and looks the same on every platform. The element's own opacity (including its
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
