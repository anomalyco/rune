//! Standalone `Runtime`, `Renderer`, `SurfaceRenderer`, and `OFFSCREEN_FORMAT` blend in sRGB by
//! default, matching `App` windows within 1 LSB; linear light is available via explicit opt-in
//! (`OFFSCREEN_FORMAT_LINEAR` + `set_srgb_blend(false)`), and HDR (`Rgba16Float`) targets stay linear.

use rune::color::hex;
use rune::native_surface::{ByteFrame, ColorInfo, Format, Matrix, SurfaceRenderer, Transfer};
use rune::render::{Gpu, OFFSCREEN_FORMAT, OFFSCREEN_FORMAT_LINEAR, Offscreen, Renderer};
use rune::text::TextSystem;
use rune::{App, AppSpec, Color, Cx, El, Env, NativeSurface, Runtime, SurfaceFit, col, el, row};

fn pixel(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * width + x) * 4) as usize;
    [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]
}

fn near(got: [u8; 4], want: [u8; 4], label: &str) {
    assert!(
        got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 1),
        "{label}: got {got:?}, want {want:?} (±1)"
    );
}

fn snap_app<S: 'static>(app: App<S>, name: &str) -> Vec<u8> {
    let dir = std::env::temp_dir().join(format!("rune-srgb-default-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("snap.png");
    app.args(["--snap", png.to_str().unwrap(), "--scale", "1"].map(String::from))
        .try_run()
        .unwrap();
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&png).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0u8; reader.output_buffer_size().unwrap()];
    reader.next_frame(&mut buf).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    buf
}

fn low_alpha_and_video_view(video: &NativeSurface, _: &mut Cx<NativeSurface>) -> El {
    let light_bg = hex("#F7F8FA").unwrap();
    let dark_bg = hex("#2B2D30").unwrap();
    row()
        .size(96.0, 32.0)
        .child(
            el().size(32.0, 32.0)
                .fill(light_bg)
                .child(el().w_full().h_full().fill(Color::BLACK.alpha(0.19))),
        )
        .child(
            el().size(32.0, 32.0)
                .fill(dark_bg)
                .child(el().w_full().h_full().fill(Color::WHITE.alpha(0.10))),
        )
        .child(
            el().size(32.0, 32.0)
                .fill(Color::BLACK)
                .child(el().w_full().h_full().native_surface(video, SurfaceFit::Fill)),
        )
}

fn bt709_surface() -> NativeSurface {
    let surface = NativeSurface::new();
    let (w, h) = (16usize, 16usize);
    surface
        .push_bytes(
            ByteFrame {
                width: w as u32,
                height: h as u32,
                format: Format::Nv12Full,
                color: ColorInfo {
                    matrix: Matrix::Bt709,
                    transfer: Transfer::Bt709,
                },
                planes: [vec![54u8; w * h], vec![128u8; w * h / 2]],
            },
            1,
        )
        .unwrap();
    surface
}

#[test]
fn defaults_blend_in_srgb_and_hdr_stays_linear() {
    assert_eq!(OFFSCREEN_FORMAT, wgpu::TextureFormat::Rgba8Unorm);
    assert_eq!(OFFSCREEN_FORMAT_LINEAR, wgpu::TextureFormat::Rgba8UnormSrgb);

    let rt = Runtime::new((), |_: &(), _| el(), (64.0, 64.0), Env::new(true, 1.0));
    assert!(rt.srgb_blend(), "Runtime::new defaults to sRGB blending");
    let rt_text = Runtime::with_text((), |_: &(), _| el(), (64.0, 64.0), Env::new(true, 1.0), TextSystem::new(&[], ("Inter", "Inter")));
    assert!(rt_text.srgb_blend(), "Runtime::with_text defaults to sRGB blending");

    let Some(gpu) = Gpu::test_headless() else { return };
    let default_target = Offscreen::new(&gpu.device, 16, 16);
    assert_eq!(default_target.format(), OFFSCREEN_FORMAT);
    let linear_target = Offscreen::with_format(&gpu.device, 16, 16, OFFSCREEN_FORMAT_LINEAR);
    assert_eq!(linear_target.format(), OFFSCREEN_FORMAT_LINEAR);

    assert!(Renderer::new(&gpu.device, OFFSCREEN_FORMAT).srgb_blend());
    assert!(Renderer::new(&gpu.device, wgpu::TextureFormat::Bgra8Unorm).srgb_blend());
    assert!(!Renderer::new(&gpu.device, OFFSCREEN_FORMAT_LINEAR).srgb_blend());
    assert!(!Renderer::new(&gpu.device, wgpu::TextureFormat::Bgra8UnormSrgb).srgb_blend());
    assert!(!Renderer::new(&gpu.device, wgpu::TextureFormat::Rgba16Float).srgb_blend(), "HDR Rgba16Float stays linear");

    assert!(SurfaceRenderer::new(&gpu.device, OFFSCREEN_FORMAT).srgb_blend());
    assert!(SurfaceRenderer::new(&gpu.device, wgpu::TextureFormat::Bgra8Unorm).srgb_blend());
    assert!(!SurfaceRenderer::new(&gpu.device, OFFSCREEN_FORMAT_LINEAR).srgb_blend());
    assert!(!SurfaceRenderer::new(&gpu.device, wgpu::TextureFormat::Rgba16Float).srgb_blend(), "HDR SurfaceRenderer stays linear");
}

#[test]
fn default_offscreen_matches_app_window_within_1_lsb() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let (w, h) = (96u32, 32u32);
    let mut rt = Runtime::new(bt709_surface(), low_alpha_and_video_view, (w as f32, h as f32), Env::new(true, 1.0));
    rt.frame(0.0);
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, w, h);
    renderer.render_runtime_to_offscreen(&gpu.device, &gpu.queue, &target, &mut rt);
    let offscreen = target.read(&gpu.device);

    let app_px = snap_app(
        App::new(AppSpec::new("srgb-default", (w as f32, h as f32), bt709_surface, low_alpha_and_video_view)),
        "srgb-match",
    );
    assert_eq!(offscreen.len(), app_px.len());
    let max_diff = offscreen.iter().zip(&app_px).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
    assert!(max_diff <= 1, "default offscreen must match App window within 1 LSB, max_diff={max_diff}");

    // Black 0.19 over #F7F8FA in sRGB: 200, 201, 203.
    near(pixel(&offscreen, w, 16, 16), [200, 201, 203, 255], "black 0.19 on #F7F8FA");
    // White 0.10 over #2B2D30 in sRGB: 64, 66, 69.
    near(pixel(&offscreen, w, 48, 16), [64, 66, 69, 255], "white 0.10 on #2B2D30");
    // BT.709 NV12 full-range luma 54 decoded to linear then sRGB-encoded: 70.
    near(pixel(&offscreen, w, 80, 16), [70, 70, 70, 255], "BT.709 video luma 54 in default sRGB");
}

#[test]
fn explicit_linear_opt_in_matches_app_linear_within_1_lsb() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let (w, h) = (96u32, 32u32);
    let mut rt = Runtime::new(bt709_surface(), low_alpha_and_video_view, (w as f32, h as f32), Env::new(true, 1.0));
    rt.set_srgb_blend(false);
    rt.frame(0.0);
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT_LINEAR);
    renderer.set_srgb_blend(false);
    let target = Offscreen::with_format(&gpu.device, w, h, OFFSCREEN_FORMAT_LINEAR);
    renderer.render_runtime_to_offscreen(&gpu.device, &gpu.queue, &target, &mut rt);
    let offscreen = target.read(&gpu.device);

    let app_px = snap_app(
        App::new(AppSpec::new("linear-opt-in", (w as f32, h as f32), bt709_surface, low_alpha_and_video_view))
            .srgb_blend(false),
        "linear-match",
    );
    assert_eq!(offscreen.len(), app_px.len());
    let max_diff = offscreen.iter().zip(&app_px).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
    assert!(max_diff <= 1, "linear opt-in offscreen must match App::srgb_blend(false) within 1 LSB, max_diff={max_diff}");

    // Black 0.19 over #F7F8FA in linear light: 225, 226, 228.
    near(pixel(&offscreen, w, 16, 16), [225, 226, 228, 255], "black 0.19 on #F7F8FA (linear)");
    // White 0.10 over #2B2D30 in linear light: 98, 99, 100.
    near(pixel(&offscreen, w, 48, 16), [98, 99, 100, 255], "white 0.10 on #2B2D30 (linear)");
    // BT.709 NV12 full-range luma 54 in linear light (hardware sRGB encode): 70.
    near(pixel(&offscreen, w, 80, 16), [70, 70, 70, 255], "BT.709 video luma 54 in linear opt-in");

    // Verify a nested isolated layer also stays consistent between default offscreen and App.
    fn layered_view(_: &(), _: &mut Cx<()>) -> El {
        col().size(40.0, 40.0).fill(hex("#F7F8FA").unwrap()).child(
            col().size(40.0, 40.0).opacity(0.5).child(el().size(40.0, 40.0).fill(Color::BLACK)),
        )
    }
    let mut rt_layer = Runtime::new((), layered_view, (40.0, 40.0), Env::new(true, 1.0));
    rt_layer.frame(0.0);
    let mut ren_srgb = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let tgt_srgb = Offscreen::new(&gpu.device, 40, 40);
    ren_srgb.render_runtime_to_offscreen(&gpu.device, &gpu.queue, &tgt_srgb, &mut rt_layer);
    let layer_off = tgt_srgb.read(&gpu.device);
    let layer_app = snap_app(
        App::new(AppSpec::new("srgb-layer", (40.0, 40.0), || (), layered_view)),
        "srgb-layer",
    );
    let layer_diff = layer_off.iter().zip(&layer_app).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
    assert!(layer_diff <= 1, "isolated layer default offscreen must match App within 1 LSB, max_diff={layer_diff}");
}
