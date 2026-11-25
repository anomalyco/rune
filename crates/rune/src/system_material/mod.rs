//! System backdrop materials (`El::system_material`): the platform blurs what is behind the
//! window under an element's rounded rect. On macOS a native view per material sits under the
//! transparent Metal layer and moves in the same Core Animation transaction as the drawable,
//! so the material, Rune's shadow and the content on top present together. Everywhere else,
//! in opaque windows and in headless renders, the element's fill (or a neutral tint) stands in.

#[cfg(target_os = "macos")]
pub(crate) mod macos;
#[cfg(any(test, all(target_os = "windows", feature = "windows-system-material")))]
pub(crate) mod windows;
#[cfg(test)]
mod tests;

use crate::Color;
use rune_motion::NodeId;

/// A backdrop material the system composites behind a window.
///
/// ```no_run
/// use rune::{El, SystemMaterial, el};
///
/// fn sidebar(content: El) -> El {
///     el().w(240.0).h_full().system_material(SystemMaterial::Sidebar).child(content)
/// }
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SystemMaterial {
    /// Liquid Glass (`NSGlassEffectView`, regular style) on macOS 26 and later; the popover
    /// material on earlier macOS.
    Glass,
    /// The popover vibrancy material (`NSVisualEffectView`, `.popover`, behind window, always
    /// active).
    Popover,
    /// The sidebar vibrancy material (`NSVisualEffectView`, `.sidebar`, behind window,
    /// following the window's active state: vibrant while active and flat when inactive,
    /// matching Finder and Mail sidebars). Kept as `.sidebar` in legacy mode and before
    /// macOS 26.
    Sidebar,
}

/// Default native material views per window. Elements past the window's limit draw their
/// fallback instead; raise or lower the per-window cap (up to 64) with
/// [`App::max_native_materials`](crate::App::max_native_materials).
pub const MAX_NATIVE_MATERIALS: usize = 4;

/// Hard upper bound on per-window native material views so a large configured limit cannot
/// exhaust AppKit backdrop surfaces.
pub(crate) const MAX_NATIVE_MATERIALS_CAP: usize = 64;

/// Minimum effective element opacity required to remain in a Wayland compositor blur region.
/// Wayland blur protocols (`ext_background_effect_v1`, `org_kde_kwin_blur`) have no opacity
/// parameter, so dropping an element below 0.10 prevents a full-strength blur ghost during
/// exit fades.
pub(crate) const WAYLAND_BLUR_MIN_OPACITY: f32 = 0.10;

/// Per-frame physical-pixel geometry and ancestor clip for one `El::system_material` element
/// participating in Wayland compositor blur (`platform::wayland::backdrop`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WaylandMaterialBlur {
    pub rect: [f32; 4],
    pub radius: f32,
    pub clip: [f32; 4],
    pub opacity: f32,
}

/// Clamp a configured per-window native material cap to the internal ceiling.
pub(crate) fn clamp_max_native_materials(max: usize) -> usize {
    max.min(MAX_NATIVE_MATERIALS_CAP)
}

/// AppKit `NSVisualEffectView` `(material, blendingMode, state)` for non-Glass materials:
/// - `Sidebar`: `NSVisualEffectMaterialSidebar` (7), `BehindWindow` (0),
///   `FollowsWindowActiveState` (0).
/// - `Popover` (and any non-Glass fallback): `NSVisualEffectMaterialPopover` (6),
///   `BehindWindow` (0), `Active` (1).
#[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
pub(crate) fn visual_effect_config(material: SystemMaterial) -> (isize, isize, isize) {
    match material {
        SystemMaterial::Sidebar => (7, 0, 0),
        _ => (6, 0, 1),
    }
}

/// One-time stderr notice when visible `El::system_material` elements exceed the per-window cap.
pub(crate) fn fallback_notice(fallen: usize, total: usize, limit: usize) -> String {
    let first = limit + 1;
    if fallen == 1 {
        format!(
            "rune: native system material {first} of {total} exceeded limit {limit} and fell back to its fill (raise with App::max_native_materials)"
        )
    } else {
        format!(
            "rune: {fallen} of {total} native system materials ({first}..={total}) exceeded limit {limit} and fell back to their fill (raise with App::max_native_materials)"
        )
    }
}

/// One visible [`El::system_material`](crate::El::system_material) element as built this frame,
/// in paint order and in the window's physical pixels (divide by the window scale for points).
///
/// On macOS Rune hosts a native material view for each of these. Elsewhere Rune draws the
/// element's fallback fill and nothing more, but the rects are still collected, so an app can
/// drive its own backdrop (a companion window or composition visual) under them with
/// [`ConfiguredOverlay::on_materials`](crate::ConfiguredOverlay::on_materials) or
/// [`Runtime::system_materials`](crate::Runtime::system_materials). Only elements within the
/// window's native material limit ([`App::max_native_materials`](crate::App::max_native_materials))
/// are listed. Read-only: Rune builds these, apps don't.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct MaterialDraw {
    /// The element's identity, stable across frames while it stays mounted.
    pub id: NodeId,
    /// The element's [`SystemMaterial`].
    pub material: SystemMaterial,
    /// x, y, width, height from the window's top left, in physical pixels. The element's whole
    /// rect: clipping by a scroll view or a clipping ancestor is not applied, so a backdrop
    /// that must stop at a clip edge has to clip itself.
    pub rect: [f32; 4],
    /// Corner radius in physical pixels (the largest corner), already clamped to half the
    /// shorter side.
    pub radius: f32,
    /// Continuous corners (`El::continuous_corners`).
    pub continuous: bool,
    /// The element's effective opacity, ancestors included.
    pub opacity: f32,
}

impl MaterialDraw {
    /// The element's frame (x, y, width, height from the window's top left) and corner radius
    /// in window points, given the window scale ([`MaterialFrame::scale`]).
    pub fn points(&self, scale: f32) -> ([f64; 4], f64) {
        let s = f64::from(scale.max(1e-3));
        (self.rect.map(|v| f64::from(v) / s), f64::from(self.radius) / s)
    }
}

/// One presented frame's [`MaterialDraw`]s and the window scale they are measured at, as
/// [`ConfiguredOverlay::on_materials`](crate::ConfiguredOverlay::on_materials) hears it.
#[derive(Clone, Copy, Debug)]
pub struct MaterialFrame<'a> {
    draws: &'a [MaterialDraw],
    scale: f32,
}

impl<'a> MaterialFrame<'a> {
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn new(draws: &'a [MaterialDraw], scale: f32) -> Self { Self { draws, scale } }

    /// The frame's material elements in paint order, in physical pixels; empty when none are
    /// visible.
    pub fn draws(&self) -> &'a [MaterialDraw] { self.draws }

    /// Physical pixels per window point (the window's scale, before the view's zoom).
    pub fn scale(&self) -> f32 { self.scale }
}

impl<S: 'static> crate::Runtime<S> {
    /// The visible [`El::system_material`](crate::El::system_material) elements of the last
    /// built frame, in paint order and physical pixels ([`MaterialDraw`]), for an app that
    /// drives its own backdrop under them where Rune hosts no native material. A window can
    /// read them from `App::on_frame_drawn`; an overlay hears each frame's just before it
    /// presents through [`ConfiguredOverlay::on_materials`](crate::ConfiguredOverlay::on_materials).
    ///
    /// ```
    /// # use rune::{Env, Runtime, SystemMaterial, el};
    /// let view = |_: &(), _: &mut rune::Cx<()>| el().size(100.0, 40.0).radius(20.0).system_material(SystemMaterial::Glass);
    /// let mut rt = Runtime::new((), view, (100.0, 40.0), Env::default());
    /// rt.frame(0.0);
    /// let draw = rt.system_materials()[0];
    /// assert_eq!(draw.points(rt.window_scale()), ([0.0, 0.0, 100.0, 40.0], 20.0));
    /// ```
    pub fn system_materials(&self) -> &[MaterialDraw] { self.core().paint.materials() }
}

/// The native material draws and window backing scale to hand to the platform material host.
/// `MaterialDraw` rects and radii are in physical device pixels, while native material views
/// sit in window points, so this uses `window_scale()` rather than `env.scale` (`window_scale * zoom`).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn present_draws(rt: &crate::runtime::RuntimeCore) -> (&[MaterialDraw], f32) {
    (rt.paint.materials(), rt.window_scale())
}

/// Non-opaque material draws for Windows 11 Acrylic visuals (excludes elements whose resolved
/// fill is opaque, matching Wayland's fill rules while `Runtime::system_materials` and
/// `on_materials` keep listing every visible material element).
#[cfg(any(test, all(target_os = "windows", feature = "windows-system-material")))]
pub(crate) fn windows_present_draws(rt: &crate::runtime::RuntimeCore) -> &[MaterialDraw] {
    rt.paint.windows_material_draws()
}

/// Callback registered by [`ConfiguredOverlay::on_materials`](crate::ConfiguredOverlay::on_materials).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) type OnMaterials = Box<dyn FnMut(MaterialFrame<'_>)>;

/// Hands a frame about to present to `ConfiguredOverlay::on_materials`, when one is set.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn announce(hook: &mut Option<OnMaterials>, rt: &crate::runtime::RuntimeCore) {
    if let Some(hook) = hook {
        let (draws, scale) = present_draws(rt);
        hook(MaterialFrame::new(draws, scale));
    }
}

/// Which existing view each draw takes: the view that showed the same element, else a free view
/// of the same material, else none (the caller makes one). `views` holds each view's material
/// and the element it showed last; `draws` each draw's material and element.
#[cfg_attr(not(any(test, target_os = "macos", all(target_os = "windows", feature = "windows-system-material"))), allow(dead_code))]
pub(crate) fn assign(views: &[(SystemMaterial, Option<NodeId>)], draws: &[(SystemMaterial, NodeId)]) -> Vec<Option<usize>> {
    let mut taken = vec![false; views.len()];
    let mut out = vec![None; draws.len()];
    for (slot, &(material, id)) in out.iter_mut().zip(draws) {
        if let Some(v) = (0..views.len()).find(|&v| !taken[v] && views[v] == (material, Some(id))) {
            taken[v] = true;
            *slot = Some(v);
        }
    }
    for (slot, &(material, _)) in out.iter_mut().zip(draws) {
        if slot.is_some() { continue; }
        if let Some(v) = (0..views.len()).find(|&v| !taken[v] && views[v].0 == material) {
            taken[v] = true;
            *slot = Some(v);
        }
    }
    out
}

/// The Metal layer's `presentsWithTransaction`, which native materials, a moving window frame
/// and a live resize all need. One owner, so none turns it off under another.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PresentFlag {
    moving: bool,
    materials: bool,
    live: bool,
    written: bool,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
impl PresentFlag {
    /// Whether the layer presents with transactions.
    pub(crate) fn written(&self) -> bool {
        self.written
    }

    /// A window frame animation started or stopped. The value to write, when it changed.
    pub(crate) fn moving(&mut self, on: bool) -> Option<bool> {
        self.moving = on;
        self.write()
    }

    /// Native materials synced with the drawable switched on or off. The value to write, when it changed.
    pub(crate) fn materials(&mut self, on: bool) -> Option<bool> {
        self.materials = on;
        self.write()
    }

    /// The user started or stopped dragging the window's edge. The value to write, when it changed.
    pub(crate) fn live(&mut self, on: bool) -> Option<bool> {
        self.live = on;
        self.write()
    }

    fn write(&mut self) -> Option<bool> {
        let on = self.moving || self.materials || self.live;
        (on != self.written).then(|| { self.written = on; on })
    }
}

/// One corner radius for a rect: native materials take a single radius, so this uses the
/// largest corner, clamped to half the shorter side as Rune's own rounded rects are.
pub(crate) fn clamp_radius(radii: [f32; 4], width: f32, height: f32) -> f32 {
    radii.into_iter().fold(0.0f32, f32::max).min(width.min(height) * 0.5).max(0.0)
}

/// What a material element paints in Rune's own surface when no native view backs it: its
/// fill, or a neutral tint close to the material's average look when it has none.
pub(crate) fn fallback_fill(fill: Color, dark: bool) -> Color {
    if fill.a > 0.0 { return fill; }
    if dark { Color { r: 0.16, g: 0.16, b: 0.17, a: 0.94 } } else { Color { r: 0.96, g: 0.96, b: 0.97, a: 0.94 } }
}

/// How the shell hosts materials, from `RUNE_SYSTEM_MATERIAL`: unset for native views synced
/// with the drawable; `legacy` forces the pre-26 `NSVisualEffectView` path; `unsynced` moves
/// native views outside the drawable's transaction (to compare); `off` draws the fallback.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Mode {
    #[default]
    Synced,
    Legacy,
    Unsynced,
    Off,
}

impl Mode {
    pub(crate) fn parse(value: Option<&str>) -> Mode {
        match value.map(str::trim) {
            Some("legacy") => Mode::Legacy,
            Some("unsynced") => Mode::Unsynced,
            Some("off" | "0") => Mode::Off,
            _ => Mode::Synced,
        }
    }

    #[cfg_attr(not(any(target_os = "macos", all(target_os = "windows", feature = "windows-system-material"), all(unix, not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
    pub(crate) fn from_env() -> Mode {
        Mode::parse(rune_core::env::var("RUNE_SYSTEM_MATERIAL").as_deref())
    }

    /// Resolves which native material backs `material` given whether `NSGlassEffectView` is
    /// available: `Glass` falls back to `Popover` before macOS 26 and in legacy mode, while
    /// `Popover` and `Sidebar` stay as requested.
    #[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
    pub(crate) fn resolve_with(self, material: SystemMaterial, has_glass: bool) -> SystemMaterial {
        match material {
            SystemMaterial::Glass if self == Mode::Legacy || !has_glass => SystemMaterial::Popover,
            other => other,
        }
    }
}
