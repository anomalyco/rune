//! Documented Win32 layered-window presentation (`UpdateLayeredWindowIndirect` + `ULW_ALPHA`)
//! for opt-in CPU-rendered transparent overlays on Windows.
//!
//! Uses only Microsoft-documented Win32 APIs:
//! - `CreateDIBSection` (32-bpp top-down `BI_RGB` via `platform::win32::dib::Dib`)
//! - `WS_EX_LAYERED` (never combined with `WS_EX_NOREDIRECTIONBITMAP` or `SetLayeredWindowAttributes`)
//! - `UpdateLayeredWindowIndirect` with `UPDATELAYEREDWINDOWINFO`, `BLENDFUNCTION`
//!   (`AC_SRC_OVER`, `AC_SRC_ALPHA`), and `ULW_ALPHA`
//!
//! Microsoft Learn documents that calling `SetLayeredWindowAttributes` on a layered window makes
//! all subsequent `UpdateLayeredWindow` / `UpdateLayeredWindowIndirect` calls fail until
//! `WS_EX_LAYERED` is cleared and re-applied; this path therefore never calls
//! `SetLayeredWindowAttributes`, driving `Presence::FadingOut` via `BLENDFUNCTION::SourceConstantAlpha`.

use crate::{KeyPolicy, OverlayInput};

/// Documented `BI_RGB` (`wingdi.h`).
pub(crate) const BI_RGB: u32 = 0;
/// Documented `AC_SRC_OVER` (`wingdi.h`).
pub(crate) const AC_SRC_OVER: u8 = 0x00;
/// Documented `AC_SRC_ALPHA` (`wingdi.h`).
pub(crate) const AC_SRC_ALPHA: u8 = 0x01;
/// Documented `ULW_ALPHA` (`winuser.h`).
pub(crate) const ULW_ALPHA: u32 = 0x0000_0002;

/// Documented extended window styles (`winuser.h`).
pub(crate) const WS_EX_TOPMOST: u32 = 0x0000_0008;
pub(crate) const WS_EX_TRANSPARENT: u32 = 0x0000_0020;
pub(crate) const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
pub(crate) const WS_EX_LAYERED: u32 = 0x0008_0000;
pub(crate) const WS_EX_NOREDIRECTIONBITMAP: u32 = 0x0020_0000;
pub(crate) const WS_EX_NOACTIVATE: u32 = 0x0800_0000;

/// Maximum width or height (physical pixels) of a CPU overlay DIB section.
pub(crate) const MAX_LAYERED_DIM: u32 = 8192;
/// Maximum decoded BGRA byte size of a single CPU overlay DIB section (128 MiB; covers dual-4K).
pub(crate) const MAX_LAYERED_BYTES: usize = 128 * 1024 * 1024;
/// Minimum byte size above which a 4x+ oversized DIB is shrunk on resize.
const SHRINK_THRESHOLD_BYTES: usize = 1024 * 1024;

#[cfg(target_os = "windows")]
const _: () = {
    use windows_sys::Win32::Graphics::Gdi as gdi;
    use windows_sys::Win32::UI::WindowsAndMessaging as wm;
    assert!(BI_RGB == gdi::BI_RGB);
    assert!(AC_SRC_OVER == gdi::AC_SRC_OVER as u8);
    assert!(AC_SRC_ALPHA == gdi::AC_SRC_ALPHA as u8);
    assert!(ULW_ALPHA == wm::ULW_ALPHA);
    assert!(WS_EX_TOPMOST == wm::WS_EX_TOPMOST);
    assert!(WS_EX_TRANSPARENT == wm::WS_EX_TRANSPARENT);
    assert!(WS_EX_TOOLWINDOW == wm::WS_EX_TOOLWINDOW);
    assert!(WS_EX_LAYERED == wm::WS_EX_LAYERED);
    assert!(WS_EX_NOREDIRECTIONBITMAP == wm::WS_EX_NOREDIRECTIONBITMAP);
    assert!(WS_EX_NOACTIVATE == wm::WS_EX_NOACTIVATE);
};

/// Pure description of the `BITMAPINFOHEADER` passed to `CreateDIBSection`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DibHeaderSpec {
    pub width: i32,
    /// Negative height selects a top-down DIB (row 0 at top), matching `CpuPainter`.
    pub height: i32,
    pub planes: u16,
    pub bit_count: u16,
    pub compression: u32,
}

impl DibHeaderSpec {
    /// Top-down 32-bpp `BI_RGB` BGRA DIB specification for `width x height` physical pixels.
    #[inline]
    pub(crate) fn top_down_bgra(width: u32, height: u32) -> Self {
        let w = width.clamp(1, MAX_LAYERED_DIM);
        let h = height.clamp(1, MAX_LAYERED_DIM);
        Self {
            width: w as i32,
            height: -(h as i32),
            planes: 1,
            bit_count: 32,
            compression: BI_RGB,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    #[inline]
    pub(crate) const fn dimensions(self) -> (u32, u32) {
        (self.width.unsigned_abs(), self.height.unsigned_abs())
    }
}

/// Pure description of `BLENDFUNCTION` passed to `UpdateLayeredWindowIndirect`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BlendSpec {
    pub blend_op: u8,
    pub blend_flags: u8,
    pub source_constant_alpha: u8,
    pub alpha_format: u8,
}

impl BlendSpec {
    /// Per-pixel premultiplied alpha (`AC_SRC_OVER` + `AC_SRC_ALPHA`) scaled by `alpha`.
    #[inline]
    pub(crate) const fn per_pixel(alpha: u8) -> Self {
        Self {
            blend_op: AC_SRC_OVER,
            blend_flags: 0,
            source_constant_alpha: alpha,
            alpha_format: AC_SRC_ALPHA,
        }
    }
}

/// Pure description of one `UpdateLayeredWindowIndirect` call (`UPDATELAYEREDWINDOWINFO`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LayeredUpdate {
    /// `pptDst`: window top-left in physical desktop pixels.
    pub dst: (i32, i32),
    /// `psize`: layered surface dimensions in physical pixels.
    pub size: (i32, i32),
    /// `pptSrc`: origin in the source DIB DC, always `(0, 0)`.
    pub src: (i32, i32),
    /// `pblend`: `BLENDFUNCTION` with `AC_SRC_OVER` and `AC_SRC_ALPHA`.
    pub blend: BlendSpec,
    /// `dwFlags`: `ULW_ALPHA`.
    pub flags: u32,
    /// `prcDirty`: `None` (`NULL`, full window) on first present, resize, DPI change, show after
    /// hide, or fade; `Some([left, top, right, bottom])` on steady-size partial updates.
    pub dirty: Option<[i32; 4]>,
}

/// Window creation flags required by a Windows CPU layered overlay: neither winit's
/// `DwmEnableBlurBehindWindow` transparency hook nor `WS_EX_NOREDIRECTIONBITMAP`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CpuLayeredWindowFlags {
    pub transparent: bool,
    pub no_redirection_bitmap: bool,
}

/// Returns the winit window attributes for a Windows CPU layered overlay.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
#[inline]
pub(crate) const fn window_attrs_for_cpu_layered() -> CpuLayeredWindowFlags {
    CpuLayeredWindowFlags {
        transparent: false,
        no_redirection_bitmap: false,
    }
}

/// Computes `GWL_EXSTYLE` for a Windows CPU layered overlay without ever setting
/// `WS_EX_NOREDIRECTIONBITMAP` (and callers must never call `SetLayeredWindowAttributes`).
#[inline]
pub(crate) fn overlay_layered_ex_style(current: u32, mode: OverlayInput, keys: KeyPolicy) -> u32 {
    let base = (current & !(WS_EX_NOREDIRECTIONBITMAP | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT))
        | WS_EX_LAYERED
        | WS_EX_TOOLWINDOW
        | WS_EX_TOPMOST;
    let no_activate = if keys.takes_keys() { 0 } else { WS_EX_NOACTIVATE };
    let through = if mode == OverlayInput::All { 0 } else { WS_EX_TRANSPARENT };
    base | no_activate | through
}

/// Pure hit-test classification for a layered window under `ULW_ALPHA`.
///
/// Under Win32's Raw Input Thread, pixels whose DIB alpha byte is `0` are always click-through
/// before `WM_NCHITTEST` runs; non-zero alpha pixels reach `OverlayWindow`'s `WM_NCHITTEST`
/// subclass, which follows `fading`, `captured`, `OverlayInput`, and `regions`.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LayeredHitResult {
    /// DIB pixel has `alpha == 0`: Win32 RIT passes the pointer through before `WM_NCHITTEST`.
    TransparentPixel,
    /// `WM_NCHITTEST` returns `HTTRANSPARENT`.
    PassThrough,
    /// `WM_NCHITTEST` returns `HTCLIENT`.
    Client,
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn layered_hit_test(
    pixel_alpha: u8,
    fading: bool,
    captured: bool,
    mode: OverlayInput,
    regions: &[[f32; 4]],
    point: (f32, f32),
) -> LayeredHitResult {
    if pixel_alpha == 0 {
        return LayeredHitResult::TransparentPixel;
    }
    let hit = !fading
        && (captured
            || match mode {
                OverlayInput::All => true,
                OverlayInput::Regions => regions
                    .iter()
                    .any(|r| crate::overlay::options::contains(*r, point.0, point.1)),
                OverlayInput::None | OverlayInput::Legacy => false,
            });
    if hit {
        LayeredHitResult::Client
    } else {
        LayeredHitResult::PassThrough
    }
}

/// Clamps logical overlay `size * scale` into a bounded physical pixel size within
/// `1..=MAX_LAYERED_DIM` and `MAX_LAYERED_BYTES` (4 bytes per BGRA pixel).
pub(crate) fn bounded_layered_size(logical: (f32, f32), scale: f32) -> (u32, u32) {
    let s = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let mut w = (logical.0 * s).round().clamp(1.0, MAX_LAYERED_DIM as f32) as u32;
    let mut h = (logical.1 * s).round().clamp(1.0, MAX_LAYERED_DIM as f32) as u32;
    let bytes = (w as usize).saturating_mul(h as usize).saturating_mul(4);
    if bytes > MAX_LAYERED_BYTES {
        let ratio = (MAX_LAYERED_BYTES as f64 / bytes as f64).sqrt();
        w = ((w as f64 * ratio).floor() as u32).max(1);
        h = ((h as f64 * ratio).floor() as u32).max(1);
        while (w as usize).saturating_mul(h as usize).saturating_mul(4) > MAX_LAYERED_BYTES {
            if w >= h && w > 1 {
                w -= 1;
            } else if h > 1 {
                h -= 1;
            } else {
                break;
            }
        }
    }
    (w, h)
}

/// Whether an existing DIB of `allocated` pixels is sufficiently oversized for `wanted` that
/// shrinking it reclaims meaningful memory without thrashing small resizes.
pub(crate) const fn should_shrink_surface(allocated: (u32, u32), wanted: (u32, u32)) -> bool {
    let alloc_bytes = (allocated.0 as usize)
        .saturating_mul(allocated.1 as usize)
        .saturating_mul(4);
    let wanted_bytes = (wanted.0 as usize)
        .saturating_mul(wanted.1 as usize)
        .saturating_mul(4);
    alloc_bytes > SHRINK_THRESHOLD_BYTES && alloc_bytes > wanted_bytes.saturating_mul(4)
}

/// Computes the single bounding Win32 `RECT` `[left, top, right, bottom]` for `rects` clipped to
/// `0..size.0` x `0..size.1`, or `None` if no pixel is dirty.
pub(crate) fn dirty_bounds(rects: &[[u32; 4]], size: (u32, u32)) -> Option<[i32; 4]> {
    if rects.is_empty() || size.0 == 0 || size.1 == 0 {
        return None;
    }
    let mut left = u32::MAX;
    let mut top = u32::MAX;
    let mut right = 0u32;
    let mut bottom = 0u32;
    for r in rects {
        let x0 = r[0].min(size.0);
        let y0 = r[1].min(size.1);
        let x1 = r[2].min(size.0);
        let y1 = r[3].min(size.1);
        if x1 > x0 && y1 > y0 {
            left = left.min(x0);
            top = top.min(y0);
            right = right.max(x1);
            bottom = bottom.max(y1);
        }
    }
    (right > left && bottom > top).then_some([left as i32, top as i32, right as i32, bottom as i32])
}

/// Builds the `LayeredUpdate` for a frame, or `None` when nothing needs presenting (zero size, or
/// a steady frame with zero dirty pixels).
pub(crate) fn layered_update(
    dst: (i32, i32),
    size: (u32, u32),
    alpha: u8,
    full: bool,
    rects: &[[u32; 4]],
) -> Option<LayeredUpdate> {
    if size.0 == 0 || size.1 == 0 {
        return None;
    }
    let dirty = if full {
        None
    } else {
        Some(dirty_bounds(rects, size)?)
    };
    Some(LayeredUpdate {
        dst,
        size: (size.0 as i32, size.1 as i32),
        src: (0, 0),
        blend: BlendSpec::per_pixel(alpha),
        flags: ULW_ALPHA,
        dirty,
    })
}

/// Narrow testable seam around Win32's DIB section + `UpdateLayeredWindowIndirect`.
pub(crate) trait LayeredSeam {
    type Surface;

    /// Allocates a 32-bpp top-down `BI_RGB` backing surface matching `header`.
    fn create_surface(&mut self, header: DibHeaderSpec) -> Result<Self::Surface, String>;

    /// Mutable premultiplied BGRA8 pixels and row stride in bytes.
    fn surface_pixels<'a>(&self, surface: &'a mut Self::Surface) -> (&'a mut [u8], usize);

    /// Allocated pixel dimensions `(width, height)` of `surface`.
    fn surface_size(&self, surface: &Self::Surface) -> (u32, u32);

    /// Presents `surface` onto `hwnd` using `UpdateLayeredWindowIndirect` with `ULW_ALPHA`.
    /// Implementations must never call `SetLayeredWindowAttributes`.
    fn update_layered(
        &mut self,
        hwnd: isize,
        surface: &Self::Surface,
        update: LayeredUpdate,
    ) -> Result<(), String>;
}

/// Retained layered-window surface and presentation tracker generic over [`LayeredSeam`].
///
/// Reuses the allocated surface across frames with zero heap allocations while shown, enforces
/// full-window `prcDirty = NULL` on first present, resize, DPI change, show after hide, or fade
/// alpha changes, and releases the backing surface when hidden.
pub(crate) struct LayeredPresenter<S: LayeredSeam> {
    seam: S,
    surface: Option<S::Surface>,
    last_presented_size: Option<(u32, u32)>,
    last_alpha: u8,
    needs_full_present: bool,
    has_presented: bool,
}

impl<S: LayeredSeam> LayeredPresenter<S> {
    pub(crate) fn new(seam: S) -> Self {
        Self {
            seam,
            surface: None,
            last_presented_size: None,
            last_alpha: 255,
            needs_full_present: true,
            has_presented: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn seam(&self) -> &S {
        &self.seam
    }


    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn has_surface(&self) -> bool {
        self.surface.is_some()
    }

    pub(crate) fn has_presented(&self) -> bool {
        self.has_presented
    }

    #[cfg(test)]
    pub(crate) fn last_alpha(&self) -> u8 {
        self.last_alpha
    }

    /// Marks the next present as needing a full-window update (`prcDirty = NULL`), for example
    /// after resize, DPI change, or re-show.
    #[cfg(target_os = "windows")]
    pub(crate) fn invalidate_full(&mut self) {
        self.needs_full_present = true;
    }

    /// Frees the retained backing surface when the overlay hides or releases, reclaiming all DIB
    /// memory until the next show.
    pub(crate) fn release_surface(&mut self) {
        self.surface = None;
        self.last_presented_size = None;
        self.last_alpha = 255;
        self.needs_full_present = true;
    }

    /// Ensures a backing surface of at least `size` pixels exists (shrinking only when greatly
    /// oversized) and returns `(pixels, stride, reallocated)`. When `reallocated` is `true`, the
    /// caller must call `CpuPainter::forget()` so every tile paints fresh.
    pub(crate) fn ensure_surface(
        &mut self,
        size: (u32, u32),
    ) -> Result<(&mut [u8], usize, bool), String> {
        let size = (size.0.clamp(1, MAX_LAYERED_DIM), size.1.clamp(1, MAX_LAYERED_DIM));
        let mut reallocated = false;
        let need_new = match &self.surface {
            Some(surface) => {
                let alloc = self.seam.surface_size(surface);
                alloc.0 < size.0 || alloc.1 < size.1 || should_shrink_surface(alloc, size)
            }
            None => true,
        };
        if need_new {
            let header = DibHeaderSpec::top_down_bgra(size.0, size.1);
            self.surface = Some(self.seam.create_surface(header)?);
            self.needs_full_present = true;
            reallocated = true;
        }
        let surface = self.surface.as_mut().expect("surface ensured above");
        let (pixels, stride) = self.seam.surface_pixels(surface);
        Ok((pixels, stride, reallocated))
    }

    /// Presents the current surface to `hwnd`, returning `Ok(true)` if `UpdateLayeredWindowIndirect`
    /// was invoked or `Ok(false)` if the steady frame had no dirty pixels to present.
    pub(crate) fn present_frame(
        &mut self,
        hwnd: isize,
        origin: (i32, i32),
        size: (u32, u32),
        alpha: u8,
        dirty_rects: &[[u32; 4]],
    ) -> Result<bool, String> {
        let Some(surface) = self.surface.as_ref() else {
            return Ok(false);
        };
        let full = self.needs_full_present
            || self.last_presented_size != Some(size)
            || self.last_alpha != alpha;
        let Some(update) = layered_update(origin, size, alpha, full, dirty_rects) else {
            return Ok(false);
        };
        self.seam.update_layered(hwnd, surface, update)?;
        self.needs_full_present = false;
        self.last_presented_size = Some(size);
        self.last_alpha = alpha;
        self.has_presented = true;
        Ok(true)
    }

    /// Re-presents the already-painted surface at a new `SourceConstantAlpha` during
    /// `Presence::FadingOut` (or restores `255` when a fade is cancelled) without repainting
    /// `CpuPainter`. Always passes `prcDirty = None` (full window).
    pub(crate) fn present_fade(
        &mut self,
        hwnd: isize,
        origin: (i32, i32),
        alpha: u8,
    ) -> Result<bool, String> {
        let (Some(surface), Some(size)) = (self.surface.as_ref(), self.last_presented_size) else {
            return Ok(false);
        };
        if self.last_alpha == alpha && !self.needs_full_present {
            return Ok(false);
        }
        let Some(update) = layered_update(origin, size, alpha, true, &[]) else {
            return Ok(false);
        };
        self.seam.update_layered(hwnd, surface, update)?;
        self.last_alpha = alpha;
        self.needs_full_present = false;
        self.has_presented = true;
        Ok(true)
    }
}

/// Real Win32 layered-window seam backed by `Dib` and `UpdateLayeredWindowIndirect`.
#[cfg(target_os = "windows")]
pub(crate) struct RealLayeredSeam;

#[cfg(target_os = "windows")]
impl LayeredSeam for RealLayeredSeam {
    type Surface = super::dib::Dib;

    fn create_surface(&mut self, header: DibHeaderSpec) -> Result<Self::Surface, String> {
        let (w, h) = (header.width.unsigned_abs(), header.height.unsigned_abs());
        super::dib::Dib::new(w, h)
            .ok_or_else(|| format!("CreateDIBSection({w}x{h}) failed: {}", super::last_error()))
    }

    fn surface_pixels<'a>(&self, surface: &'a mut Self::Surface) -> (&'a mut [u8], usize) {
        let stride = surface.stride();
        (surface.bytes(), stride)
    }

    fn surface_size(&self, surface: &Self::Surface) -> (u32, u32) {
        surface.size()
    }

    #[allow(unsafe_code)]
    fn update_layered(
        &mut self,
        hwnd: isize,
        surface: &Self::Surface,
        update: LayeredUpdate,
    ) -> Result<(), String> {
        use windows_sys::Win32::Foundation::{HWND, POINT, RECT, SIZE};
        use windows_sys::Win32::Graphics::Gdi::BLENDFUNCTION;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            UPDATELAYEREDWINDOWINFO, UpdateLayeredWindowIndirect,
        };

        let pt_dst = POINT { x: update.dst.0, y: update.dst.1 };
        let size = SIZE { cx: update.size.0, cy: update.size.1 };
        let pt_src = POINT { x: update.src.0, y: update.src.1 };
        let blend = BLENDFUNCTION {
            BlendOp: update.blend.blend_op,
            BlendFlags: update.blend.blend_flags,
            SourceConstantAlpha: update.blend.source_constant_alpha,
            AlphaFormat: update.blend.alpha_format,
        };
        let dirty_rect = update.dirty.map(|[left, top, right, bottom]| RECT {
            left,
            top,
            right,
            bottom,
        });
        let info = UPDATELAYEREDWINDOWINFO {
            cbSize: size_of::<UPDATELAYEREDWINDOWINFO>() as u32,
            hdcDst: std::ptr::null_mut(),
            pptDst: &pt_dst,
            psize: &size,
            hdcSrc: surface.dc(),
            pptSrc: &pt_src,
            crKey: 0,
            pblend: &blend,
            dwFlags: update.flags,
            prcDirty: dirty_rect.as_ref().map_or(std::ptr::null(), |r| r as *const RECT),
        };
        // SAFETY: `hwnd` is a live `WS_EX_LAYERED` window owned by this thread, `surface.dc()`
        // has a 32-bpp top-down premultiplied BGRA DIB selected into it, and every pointer in
        // `info` stays alive for the duration of the synchronous Win32 call.
        let ok = unsafe { UpdateLayeredWindowIndirect(hwnd as HWND, &info) };
        if ok == 0 {
            return Err(format!(
                "UpdateLayeredWindowIndirect failed: {}",
                super::last_error()
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "layered_tests.rs"]
mod tests;
