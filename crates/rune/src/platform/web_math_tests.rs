use super::*;

#[test]
fn a_phone_canvas_reported_in_css_pixels_gets_its_device_height() {
    // Chromium's emulated device scale reports the CSS box: 390 × 844 at 2× used to take the
    // width's fix and leave the height at 844, stretching the canvas 2× vertically.
    assert_eq!(fix_canvas_size((390.0, 844.0), (390, 844), (1200.0, 900.0), 2.0, false), (780, 1688));
}

#[test]
fn a_canvas_still_at_its_intrinsic_size_is_sized_to_its_box() {
    // The canvas exists at the HTML default 300 × 150 until `resumed` styles it; a frame that
    // races the browser's first ResizeObserver sees that size.
    assert_eq!(fix_canvas_size((393.0, 852.0), (300, 150), (1200.0, 900.0), 3.0, false), (1179, 2556));
}

#[test]
fn a_window_that_fits_its_content_keeps_the_height_it_measured() {
    // `fit_content_height`: the canvas is exactly its content, and the app set that height.
    assert_eq!(fix_canvas_size((600.0, 640.0), (600, 640), (1200.0, 900.0), 2.0, true).1, 640);
    assert_eq!(fix_canvas_size((600.0, 640.0), (300, 150), (1200.0, 900.0), 2.0, true).1, 150);
}

#[test]
fn a_canvas_with_no_box_yet_falls_back_to_the_spec() {
    assert_eq!(fix_canvas_size((0.0, 0.0), (0, 0), (1200.0, 900.0), 2.0, false), (2400, 1800));
    assert_eq!(fix_canvas_size((0.0, 0.0), (0, 0), (1200.0, 900.0), 1.0, true), (1200, 900));
}

#[test]
fn a_size_the_box_agrees_with_is_left_alone() {
    // The box's fractional width at 1.5× rounds to a pixel the browser may report ±1.
    assert_eq!(fix_canvas_size((1001.0, 700.0), (1502, 1050), (1200.0, 900.0), 1.5, false), (1502, 1050));
}

#[test]
fn a_page_surface_never_shrinks_for_a_toolbar() {
    let phone = (1179, 2556);
    // Safari's toolbar expanding shrinks the viewport while the page is scrolling.
    assert_eq!(should_reconfigure_surface(phone, (1179, 2400), true), (phone, false));
    // Collapsing again lands on the size it already has.
    assert_eq!(should_reconfigure_surface(phone, phone, true), (phone, false));
    // A rotation has a new width, and does reconfigure.
    assert_eq!(should_reconfigure_surface(phone, (2556, 1179), true), ((2556, 1179), true));
}

#[test]
fn other_windows_reconfigure_on_every_size_change() {
    assert_eq!(should_reconfigure_surface((800, 600), (800, 500), false), ((800, 500), true));
    assert_eq!(should_reconfigure_surface((800, 600), (800, 600), false), ((800, 600), false));
    // A window hidden to nothing keeps its surface until it has a size again.
    assert_eq!(should_reconfigure_surface((800, 600), (0, 0), false), ((800, 600), false));
}

#[test]
fn webgl_picks_non_srgb_surface_format_directly_for_srgb_blend() {
    use wgpu::{Backend, TextureFormat};
    // wgpu-core sorts sRGB formats first, so WebGL2's default config prefers Rgba8UnormSrgb
    // while advertising both Rgba8UnormSrgb and Rgba8Unorm.
    let caps = [TextureFormat::Rgba8UnormSrgb, TextureFormat::Rgba8Unorm];
    let srgb_on = select_surface_formats(TextureFormat::Rgba8UnormSrgb, &caps, Backend::Gl, true);
    assert_eq!(srgb_on, SurfaceFormats {
        surface: TextureFormat::Rgba8Unorm,
        view_format: None,
        render: TextureFormat::Rgba8Unorm,
    });
    assert!(srgb_on.srgb_blend());

    // Switching back to linear blending (srgb_blend == false) selects Rgba8UnormSrgb directly.
    let srgb_off = select_surface_formats(TextureFormat::Rgba8Unorm, &caps, Backend::Gl, false);
    assert_eq!(srgb_off, SurfaceFormats {
        surface: TextureFormat::Rgba8UnormSrgb,
        view_format: None,
        render: TextureFormat::Rgba8UnormSrgb,
    });
    assert!(!srgb_off.srgb_blend());
}

#[test]
fn webgpu_uses_view_format_for_linear_blend_and_direct_for_srgb_blend() {
    use wgpu::{Backend, TextureFormat};
    let caps = [TextureFormat::Bgra8Unorm, TextureFormat::Rgba8Unorm];
    let srgb_on = select_surface_formats(TextureFormat::Bgra8Unorm, &caps, Backend::BrowserWebGpu, true);
    assert_eq!(srgb_on, SurfaceFormats {
        surface: TextureFormat::Bgra8Unorm,
        view_format: None,
        render: TextureFormat::Bgra8Unorm,
    });
    assert!(srgb_on.srgb_blend());

    let srgb_off = select_surface_formats(TextureFormat::Bgra8Unorm, &caps, Backend::BrowserWebGpu, false);
    assert_eq!(srgb_off, SurfaceFormats {
        surface: TextureFormat::Bgra8Unorm,
        view_format: Some(TextureFormat::Bgra8UnormSrgb),
        render: TextureFormat::Bgra8UnormSrgb,
    });
    assert!(!srgb_off.srgb_blend());
}

#[test]
fn webgl_without_srgb_twin_keeps_supported_format_without_view_formats() {
    use wgpu::{Backend, TextureFormat};
    let caps = [TextureFormat::Rgba8Unorm];
    let linear_asked = select_surface_formats(TextureFormat::Rgba8Unorm, &caps, Backend::Gl, false);
    assert_eq!(linear_asked, SurfaceFormats {
        surface: TextureFormat::Rgba8Unorm,
        view_format: None,
        render: TextureFormat::Rgba8Unorm,
    });
    assert!(linear_asked.srgb_blend());
}

#[test]
fn webgl_surface_usage_never_adds_unsupported_copy_flags() {
    use wgpu::{Backend, TextureUsages};
    let render_only = TextureUsages::RENDER_ATTACHMENT;
    assert_eq!(surface_usage(render_only, Backend::Gl, false), TextureUsages::RENDER_ATTACHMENT);
    assert_eq!(surface_usage(render_only, Backend::Gl, true), TextureUsages::RENDER_ATTACHMENT);
    assert_eq!(
        surface_usage(render_only, Backend::BrowserWebGpu, false),
        TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_DST,
    );
    assert_eq!(
        surface_usage(render_only, Backend::BrowserWebGpu, true),
        TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_DST | TextureUsages::COPY_SRC,
    );
    assert_eq!(
        surface_usage(render_only | TextureUsages::COPY_SRC, Backend::Gl, false),
        TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
    );
}

#[test]
fn only_non_webgpu_backends_need_device_polling() {
    use wgpu::Backend;
    assert!(!needs_device_poll(Backend::BrowserWebGpu));
    assert!(needs_device_poll(Backend::Gl));
}

#[test]
fn backend_order_skips_webgpu_when_navigator_gpu_is_absent() {
    assert_eq!(backend_order(true, true), &[WebBackend::WebGpu, WebBackend::WebGl2]);
    assert_eq!(backend_order(true, false), &[WebBackend::WebGpu]);
    assert_eq!(backend_order(false, true), &[WebBackend::WebGl2]);
    assert_eq!(backend_order(false, false), &[]);
}

#[test]
fn webgl_context_loss_recovers_instead_of_terminating_the_app() {
    // WebGL2 full-window (CPU-first or ?cpu-first=off): immediate CPU fallback, recoverable on restore.
    assert_eq!(loss_action(true, false, false, false), LossAction::FallbackToCpu { recoverable: true });
    assert_eq!(loss_action(true, true, false, false), LossAction::FallbackToCpu { recoverable: true });
    // WebGL2 inline or document: pause until webglcontextrestored instead of showing a fatal overlay.
    assert_eq!(loss_action(true, true, true, false), LossAction::PauseForRestore);
    assert_eq!(loss_action(true, true, false, true), LossAction::PauseForRestore);
    // WebGPU keeps its existing policy: permanent CPU fallback on CPU-first, fatal on GPU-first.
    assert_eq!(loss_action(false, false, false, false), LossAction::FallbackToCpu { recoverable: false });
    assert_eq!(loss_action(false, true, false, false), LossAction::Fatal);
    assert_eq!(loss_action(false, true, true, false), LossAction::Fatal);
}
