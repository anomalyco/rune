//! Window GPU surface configuration, pacing, backoff, presentation tracking and platform handles.

#[cfg(not(target_arch = "wasm32"))]
use crate::render::Gpu;
use crate::render::Renderer;
use crate::runtime::RuntimeCore;

#[cfg(not(target_arch = "wasm32"))]
pub(in crate::window) mod backoff;
pub(in crate::window) mod configure;
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::window) mod corners;
pub(crate) mod damage;
#[cfg(all(not(target_arch = "wasm32"), any(test, target_os = "macos")))]
pub(in crate::window) mod hidden_drawable;
#[cfg(target_os = "macos")]
pub(in crate::window) mod launch_drawables;
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::window) mod loop_frame;
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::window) mod native;
pub(in crate::window) mod pacing;
pub(in crate::window) mod presented;
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::window) mod redraw;
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::window) mod win;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use backoff::{Backoff, BackoffEffect, BackoffEvent};
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use configure::{ConfigureError, configure, configure_checked};
#[cfg(test)]
pub(crate) use configure::{ConfigureErrorKind, classify_scope_error};
#[cfg(target_arch = "wasm32")]
pub(crate) use configure::configure_then;
#[cfg(target_os = "macos")]
pub(crate) use hidden_drawable::{HiddenDrawable, HiddenDrawableEffect, HiddenDrawableEvent};
#[cfg(target_os = "macos")]
pub(crate) use launch_drawables::LaunchDrawables;
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::window) use loop_frame::LoopFrame;
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::window) use native::Native;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use pacing::{
    HiddenPace, TimerFrame, acquire, acquire_checked, poll_timer_deadline_core,
    timer_deadline_core, timer_wants_frame_core,
};
pub(crate) use pacing::{Missed, TIMER_FRAME};
#[cfg(target_os = "windows")]
pub(crate) use pacing::{motion_wake_wall_core, poll_wake_wall_core};
#[cfg(test)]
pub(crate) use pacing::{
    HIDDEN_REFRESH, OCCLUDED_MAX, OCCLUDED_RETRY, TIMEOUT_RETRY, TIMER_FRAME_STALL, timer_deadline,
    timer_wants_frame, wall_wake_deadline,
};
pub(crate) use presented::Presented;
#[cfg(test)]
pub(in crate::window) use presented::composing;
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::window) use redraw::{Redraw, RedrawEffect, RedrawEvent};
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::window) use win::{SurfaceRetries, SurfaceStep, SURFACE_TRIES, Win};

#[cfg(test)]
#[path = "surface/backoff_tests.rs"]
mod backoff_tests;
#[cfg(all(test, target_os = "macos"))]
#[path = "surface/occluded_surface_tests.rs"]
mod occluded_surface_tests;
#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "surface/present_mode_tests.rs"]
mod present_mode_tests;
#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "surface/wide_color_tests.rs"]
mod wide_color_tests;

/// Frames a window's swap chain may hold that the compositor hasn't shown. Windows: one, so a
/// frame shows at the first composition after it's done, and the window waits for room itself
/// (`slot`) rather than in `acquire`. Elsewhere two.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const FRAME_LATENCY: u32 = if cfg!(target_os = "windows") { 1 } else { 2 };

/// Resolves an optional `frame_latency` request for a platform whose default is
/// `default_latency`: only macOS honours `requested` (clamped to `1..=2`, where `1` = 2
/// drawables and `2` = 3 drawables); Windows, Linux, and the web ignore it and keep
/// `default_latency`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const fn effective_frame_latency(
    requested: Option<u32>,
    is_macos: bool,
    default_latency: u32,
) -> u32 {
    if !is_macos {
        return default_latency;
    }
    match requested {
        Some(n) if n <= 1 => 1,
        Some(_) => 2,
        None => default_latency,
    }
}

/// The frame latency a surface asked for `requested` gets on the current platform:
/// macOS clamps `requested` to `1..=2` (default `2`); Windows and Linux ignore it and keep
/// `FRAME_LATENCY`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const fn frame_latency(requested: Option<u32>) -> u32 {
    effective_frame_latency(requested, cfg!(target_os = "macos"), FRAME_LATENCY)
}

/// The largest 2D surface side accepted while no GPU device is attached yet (Windows CPU boot, or
/// between surfaces): matches D3D12 and Metal's 2D texture limit, large enough for multi-monitor
/// desktops while rejecting wrapped negative Windows client sizes (such as `65527` for `-9`).
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::window) const CPU_MAX_SURFACE_DIM: u32 = 16_384;

/// The largest 2D surface side the active GPU accepts, or `CPU_MAX_SURFACE_DIM` while no GPU is
/// attached yet.
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::window) fn max_surface_dim(gpu: Option<&Gpu>) -> u32 {
    gpu.map_or(CPU_MAX_SURFACE_DIM, |gpu| gpu.device.limits().max_texture_dimension_2d.max(1))
}

/// The physical drawable size for a platform-reported extent under `max_dim`.
///
/// Windows can report a transient negative client extent wrapped to `u16` (in `WM_SIZE`, e.g.
/// `65527` for `-9`) or `u32` (from `GetClientRect`), exceeding `max_texture_dimension_2d`. When
/// a window already has a drawable (`Some(prev)`), a zero or out-of-range size is ignored so the
/// surface, viewport and CPU buffer keep the previous size until a valid resize arrives; when
/// creating a surface (`None`), each edge is clamped to `1..=max_dim`.
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::window) fn drawable_size(current: Option<(u32, u32)>, reported: (u32, u32), max_dim: u32) -> (u32, u32) {
    let max_dim = max_dim.max(1);
    if reported.0 > 0 && reported.1 > 0 && reported.0 <= max_dim && reported.1 <= max_dim {
        return reported;
    }
    match current {
        Some(prev) => (prev.0.clamp(1, max_dim), prev.1.clamp(1, max_dim)),
        None => (reported.0.clamp(1, max_dim), reported.1.clamp(1, max_dim)),
    }
}

/// Picks the swap-chain presentation mode from the surface's advertised `modes`.
///
/// Surfaces that already pace every commit on Wayland's `wl_surface.frame` callback
/// (`frame_callback_paced == true`) prefer [`wgpu::PresentMode::Mailbox`] when offered: Wayland
/// compositors latch buffers atomically and never tear, and `Mailbox` stops the Vulkan WSI driver
/// from adding a `wp_fifo_v1` FIFO barrier on top of the frame callback (which can stall commits
/// mid-animation until unrelated compositor activity on compositors such as Mutter). Surfaces
/// without frame-callback pacing, or drivers that do not advertise `Mailbox`, keep
/// [`wgpu::PresentMode::AutoVsync`].
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn present_mode(
    modes: &[wgpu::PresentMode],
    frame_callback_paced: bool,
) -> wgpu::PresentMode {
    if frame_callback_paced && modes.contains(&wgpu::PresentMode::Mailbox) {
        wgpu::PresentMode::Mailbox
    } else {
        wgpu::PresentMode::AutoVsync
    }
}

/// Whether `config` is an active wide-color extended-range linear sRGB surface
/// (`Rgba16Float` + `ExtendedSrgbLinear`, blending in linear light).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const fn is_wide_color_config(config: &wgpu::SurfaceConfiguration) -> bool {
    matches!(config.format, wgpu::TextureFormat::Rgba16Float)
        && matches!(config.color_space, wgpu::SurfaceColorSpace::ExtendedSrgbLinear)
}

/// Selects the surface `(format, view_formats, color_space)` from `caps` for `srgb_blend` and
/// optional `wide_color`.
///
/// When `wide_color && allow_wide_color` and `caps` advertises `EXTENDED_SRGB_LINEAR` on
/// `Rgba16Float` (macOS Metal / wgpu-hal Metal), chooses `Rgba16Float` with
/// `SurfaceColorSpace::ExtendedSrgbLinear` so out-of-sRGB-gamut Display P3 colours render in
/// linear light without clamping. Otherwise keeps the standard 8-bit sRGB / non-sRGB choice.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn select_surface_format(
    caps: &wgpu::SurfaceCapabilities,
    srgb_blend: bool,
    wide_color: bool,
    allow_wide_color: bool,
) -> Option<(wgpu::TextureFormat, Vec<wgpu::TextureFormat>, wgpu::SurfaceColorSpace)> {
    let first = *caps.formats.first()?;
    if wide_color && allow_wide_color {
        let supports_wide = caps.format_capabilities.iter().any(|fc| {
            fc.format == wgpu::TextureFormat::Rgba16Float
                && fc.color_spaces.contains(wgpu::SurfaceColorSpaces::EXTENDED_SRGB_LINEAR)
        });
        if supports_wide {
            return Some((
                wgpu::TextureFormat::Rgba16Float,
                Vec::new(),
                wgpu::SurfaceColorSpace::ExtendedSrgbLinear,
            ));
        }
    }
    let srgb = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(first);
    if srgb_blend {
        let non_srgb = srgb.remove_srgb_suffix();
        if caps.formats.contains(&non_srgb) {
            Some((non_srgb, Vec::new(), wgpu::SurfaceColorSpace::Auto))
        } else {
            Some((srgb, vec![non_srgb], wgpu::SurfaceColorSpace::Auto))
        }
    } else {
        Some((srgb, Vec::new(), wgpu::SurfaceColorSpace::Auto))
    }
}

/// A vsynced sRGB (or non-sRGB when `srgb_blend` is set) configuration for the surface, and the
/// first alpha mode it supports that composites translucent pixels. `frame_callback_paced` is true
/// when the surface paces every frame on Wayland's `wl_surface.frame` callback so
/// [`present_mode`] can pick `Mailbox` when offered instead of stacking a `wp_fifo_v1` barrier on
/// top of it.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn surface_config(
    surface: &wgpu::Surface,
    adapter: &wgpu::Adapter,
    size: (u32, u32),
    srgb_blend: bool,
    frame_callback_paced: bool,
) -> Option<(wgpu::SurfaceConfiguration, Option<wgpu::CompositeAlphaMode>)> {
    surface_config_with(surface, adapter, size, srgb_blend, frame_callback_paced, false)
}

/// [`surface_config`] with opt-in wide-color (`App::wide_color`): on macOS, selects
/// `Rgba16Float` with `SurfaceColorSpace::ExtendedSrgbLinear` when advertised; otherwise falls
/// back to standard sRGB without changing defaults.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn surface_config_with(
    surface: &wgpu::Surface,
    adapter: &wgpu::Adapter,
    (width, height): (u32, u32),
    srgb_blend: bool,
    frame_callback_paced: bool,
    wide_color: bool,
) -> Option<(wgpu::SurfaceConfiguration, Option<wgpu::CompositeAlphaMode>)> {
    let (width, height) = drawable_size(None, (width, height), adapter.limits().max_texture_dimension_2d);
    // The surface is asked what it can do once. A Vulkan driver answers each part of that with a
    // call of its own, and Mesa's Wayland surfaces answer each call by asking the compositor for
    // its globals again and waiting for it twice (`wsi_wl_display_init`). Asked three times here
    // (`get_default_config`, this, the alpha mode), a window's swap chain was made after 26
    // such requests and 52 round trips; asked once, after 16 and 32. What follows is
    // `get_default_config`'s answer.
    let caps = surface.get_capabilities(adapter);
    let (format, view_formats, color_space) =
        select_surface_format(&caps, srgb_blend, wide_color, cfg!(target_os = "macos"))?;
    let mut config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        color_space,
        width,
        height,
        desired_maximum_frame_latency: FRAME_LATENCY,
        present_mode: wgpu::PresentMode::AutoVsync,
        alpha_mode: wgpu::CompositeAlphaMode::Auto,
        view_formats,
    };
    // Backdrop copies read the drawable mid-frame (`ActivePass::split`: backdrop blur, effect
    // backdrops, rune-fx blend modes). On Metal it costs `framebufferOnly`, but without it they
    // quietly fall back.
    config.usage |= caps.usages & wgpu::TextureUsages::COPY_SRC;
    // A frame that redrew part of itself is copied in whole from the one the renderer keeps
    // (`render::damaged`).
    config.usage |= caps.usages & wgpu::TextureUsages::COPY_DST;
    config.present_mode = present_mode(&caps.present_modes, frame_callback_paced);
    config.desired_maximum_frame_latency = FRAME_LATENCY;
    Some((config, crate::render::translucency::mode_of(&caps, adapter)))
}

/// Follows a blend switch (`Tx::set_srgb_blend`) on a configured surface: reconfigures it for
/// `rt`'s mode at its size and alpha mode, rebuilds `renderer` when its target format changes,
/// and rebuilds the `alpha` pass (when there is one) for the new format. Returns false when the
/// surface can't be configured.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn reblend_core(surface: &wgpu::Surface, gpu: &Gpu, config: &mut wgpu::SurfaceConfiguration,
    renderer: &mut Renderer, alpha: &mut Option<crate::render::surface_alpha::AlphaSurface>, rt: &mut RuntimeCore) -> bool {
    // Wide-color `Rgba16Float` + `ExtendedSrgbLinear` surfaces always blend in linear light so
    // out-of-sRGB-gamut channels (negative or > 1.0) are preserved without sRGB clamping.
    if is_wide_color_config(config) {
        rt.set_srgb_blend(false);
        renderer.set_srgb_blend(false);
        return true;
    }
    let srgb = rt.srgb_blend();
    let Some((next, _)) = surface_config(
        surface,
        &gpu.adapter,
        (config.width, config.height),
        srgb,
        config.present_mode == wgpu::PresentMode::Mailbox,
    ) else { return false };
    if (config.format, &config.view_formats) != (next.format, &next.view_formats) {
        (config.format, config.view_formats) = (next.format, next.view_formats);
        if let Err(error) = configure(surface, gpu, config) {
            eprintln!("rune: could not reconfigure a surface for a blend switch: {error}");
            return false;
        }
    }
    let target = if srgb { config.format.remove_srgb_suffix() } else { config.format };
    if let Err(error) = renderer.try_set_format(&gpu.device, target) {
        eprintln!("rune: could not rebuild the renderer pipeline for a blend switch: {error}");
        return false;
    }
    renderer.set_srgb_blend(rt.srgb_blend());
    renderer.attach_text(&mut rt.text.atlas);
    if alpha.is_some() {
        *alpha = Some(crate::render::surface_alpha::AlphaSurface::new(&gpu.device, target, (config.width, config.height)));
    }
    true
}

/// Switches `renderer` to `target` when its format changed, keeping its atlases, layer targets
/// and extra pipelines, and sets its blend mode to `rt`'s.
pub(crate) fn rebuild_renderer_core(device: &wgpu::Device, renderer: &mut Renderer, target: wgpu::TextureFormat, rt: &mut RuntimeCore) {
    renderer.set_format(device, target);
    renderer.set_srgb_blend(rt.srgb_blend());
    // Before the first frame, so its glyph masks go to the GPU fill when the renderer has one.
    renderer.attach_text(&mut rt.text.atlas);
}
