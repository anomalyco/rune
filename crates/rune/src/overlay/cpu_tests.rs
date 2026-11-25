use std::cell::Cell;

use super::*;
use crate::color::Color;
use crate::memo::memo;
use crate::overlay::options::{ConfiguredOverlay, OverlayOptions};
use crate::overlay::OverlaySpec;
use crate::render::cpu_boot::{NeedsGpu, attach_text};
use crate::render::{Instance, KIND_BACKDROP_BLUR, KIND_EFFECT, KIND_RECT, LayerSpec, NO_CLIP};
use crate::runtime::{Cx, Env, Runtime};
use crate::text::FontSmoothing;
use crate::{El, WindowBackdrop, el};

#[cfg(target_os = "windows")]
use crate::platform::win32::backdrop::{BackdropSeam, BackdropTracker, MIN_BACKDROP_BUILD, windows_supported_mask};
#[cfg(all(test, not(target_os = "windows")))]
use crate::platform::win32_backdrop::{BackdropSeam, BackdropTracker, MIN_BACKDROP_BUILD, windows_supported_mask};

#[cfg(target_os = "windows")]
use crate::platform::win32::layered::{BlendSpec, DibHeaderSpec, LayeredSeam, LayeredUpdate, ULW_ALPHA};
#[cfg(all(test, not(target_os = "windows")))]
use crate::platform::win32_layered::{BlendSpec, DibHeaderSpec, LayeredSeam, LayeredUpdate, ULW_ALPHA};

#[derive(Default)]
struct TestLayeredSeam {
    created: Vec<DibHeaderSpec>,
    updates: Vec<(isize, LayeredUpdate)>,
    last_pixels: Vec<u8>,
    fail_create: Option<String>,
    fail_update: Option<String>,
}

struct TestSurface {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl LayeredSeam for TestLayeredSeam {
    type Surface = TestSurface;

    fn create_surface(&mut self, header: DibHeaderSpec) -> Result<Self::Surface, String> {
        self.created.push(header);
        if let Some(err) = &self.fail_create {
            return Err(err.clone());
        }
        let (width, height) = header.dimensions();
        Ok(TestSurface {
            width,
            height,
            pixels: vec![0u8; width as usize * height as usize * 4],
        })
    }

    fn surface_pixels<'a>(&self, surface: &'a mut Self::Surface) -> (&'a mut [u8], usize) {
        let stride = surface.width as usize * 4;
        (&mut surface.pixels, stride)
    }

    fn surface_size(&self, surface: &Self::Surface) -> (u32, u32) {
        (surface.width, surface.height)
    }

    fn update_layered(
        &mut self,
        hwnd: isize,
        surface: &Self::Surface,
        update: LayeredUpdate,
    ) -> Result<(), String> {
        self.updates.push((hwnd, update));
        self.last_pixels = surface.pixels.clone();
        if let Some(err) = &self.fail_update {
            return Err(err.clone());
        }
        Ok(())
    }
}

#[derive(Default)]
struct RecordingBackdropSeam {
    backdrop_calls: Vec<(isize, i32)>,
    dark_calls: Vec<(isize, bool)>,
}

impl BackdropSeam for RecordingBackdropSeam {
    fn set_system_backdrop(&mut self, hwnd: isize, value: i32) -> i32 {
        self.backdrop_calls.push((hwnd, value));
        0
    }
    fn set_immersive_dark_mode(&mut self, hwnd: isize, dark: bool) -> i32 {
        self.dark_calls.push((hwnd, dark));
        0
    }
}

#[test]
fn selection_policy_covers_platforms_compositor_and_one_shot_failure() {
    // Default RendererKind::Gpu always stays on GPU everywhere.
    for target in [OverlayTarget::Windows, OverlayTarget::MacOs, OverlayTarget::Linux, OverlayTarget::Other] {
        assert_eq!(
            select_overlay_backend(target, RendererKind::Gpu, false, false),
            OverlayBackendChoice::Gpu
        );
        assert_eq!(
            select_overlay_backend(target, RendererKind::Gpu, false, false).renderer_kind(),
            RendererKind::Gpu
        );
    }

    // Windows opt-in enters CPU layered only when no compositor and no prior failure.
    let win_cpu = select_overlay_backend(OverlayTarget::Windows, RendererKind::Cpu, false, false);
    assert_eq!(win_cpu, OverlayBackendChoice::WindowsCpuLayered);
    assert!(win_cpu.is_cpu_layered());
    assert_eq!(win_cpu.renderer_kind(), RendererKind::Cpu);

    // Compositor overrides RendererKind::Cpu back to GPU.
    let comp = select_overlay_backend(OverlayTarget::Windows, RendererKind::Cpu, true, false);
    assert_eq!(comp, OverlayBackendChoice::GpuCompositorOverride);
    assert!(!comp.is_cpu_layered());
    assert_eq!(comp.renderer_kind(), RendererKind::Gpu);

    // Prior Win32 layered setup failure sticks on GPU for the rest of the overlay lifetime.
    let failed = select_overlay_backend(OverlayTarget::Windows, RendererKind::Cpu, false, true);
    assert_eq!(failed, OverlayBackendChoice::GpuAfterLayeredFailure);
    assert!(!failed.is_cpu_layered());
    assert_eq!(failed.renderer_kind(), RendererKind::Gpu);

    // Non-Windows platforms ignore RendererKind::Cpu cleanly in Step 1.
    for target in [OverlayTarget::MacOs, OverlayTarget::Linux, OverlayTarget::Other] {
        let choice = select_overlay_backend(target, RendererKind::Cpu, false, false);
        assert_eq!(choice, OverlayBackendChoice::GpuPlatformUnsupported);
        assert!(!choice.is_cpu_layered());
        assert_eq!(choice.renderer_kind(), RendererKind::Gpu);
    }
}

#[test]
fn configured_overlay_renderer_defaults_to_gpu_and_builder_sets_cpu() {
    let default_cfg: ConfiguredOverlay<()> = OverlaySpec::new("hud", (200.0, 80.0), |_: &(), _| el()).into();
    assert_eq!(default_cfg.renderer, RendererKind::Gpu);
    assert!(default_cfg.renderer.is_gpu());
    assert!(!default_cfg.renderer.is_cpu());

    let cpu_cfg = OverlaySpec::new("hud", (200.0, 80.0), |_: &(), _| el())
        .options(OverlayOptions::default())
        .renderer(RendererKind::Cpu);
    assert_eq!(cpu_cfg.renderer, RendererKind::Cpu);
    assert!(cpu_cfg.renderer.is_cpu());
    assert!(!cpu_cfg.renderer.is_gpu());
}

#[test]
fn default_gpu_overlay_makes_zero_layered_setup_and_leaves_core_gpu_path_unchanged() {
    let spec = crate::tooling::AppSpec::new("host", (300.0, 200.0), || (), |_, _| el());
    let (overlay, handle) = crate::overlay::app::Overlay::new(
        OverlaySpec::new("gpu-default", (240.0, 80.0), |_: &(), _| el()),
        (),
        &spec,
    );
    assert_eq!(overlay.core.requested_renderer, RendererKind::Gpu);
    assert_eq!(overlay.rt.core().renderer_kind(), RendererKind::Gpu);
    assert!(overlay.core.gpu.is_none());
    assert!(overlay.core.renderer.is_none());
    assert!(handle.surface_description().is_none());
    #[cfg(target_os = "windows")]
    assert!(!overlay.core.has_cpu());
}

#[test]
fn cpu_overlay_window_backdrop_always_reports_none_and_makes_zero_dwm_calls() {
    let mut tracker = BackdropTracker::default();
    let mut seam = RecordingBackdropSeam::default();
    for requested in [
        WindowBackdrop::None,
        WindowBackdrop::Mica,
        WindowBackdrop::MicaAlt,
        WindowBackdrop::Acrylic,
        WindowBackdrop::Blur,
    ] {
        let inputs = cpu_backdrop_inputs(requested, Some(MIN_BACKDROP_BUILD));
        assert_eq!(windows_supported_mask(inputs), 0, "CPU overlay must report 0 supported backdrops");
        let outcome = tracker.sync(0x9999, inputs, true, true, Some(true), &mut seam);
        assert_eq!(outcome.active, WindowBackdrop::None);
    }
    assert!(seam.backdrop_calls.is_empty(), "CPU overlay must never call DwmSetWindowAttribute");
    assert!(seam.dark_calls.is_empty());
}

#[test]
fn cx_renderer_kind_defaults_to_gpu_invalidates_memos_and_clear_chrome_resets() {
    struct State {
        seen: Cell<RendererKind>,
        cleared: Cell<RendererKind>,
        memo_runs: Cell<u32>,
        view_runs: Cell<u32>,
    }
    fn view(s: &State, cx: &mut Cx<State>) -> El {
        s.view_runs.set(s.view_runs.get() + 1);
        let kind = cx.renderer_kind();
        s.seen.set(kind);
        let mut sub = cx.core.clone();
        sub.clear_chrome();
        s.cleared.set(sub.renderer_kind());
        memo("card", (), || {
            s.memo_runs.set(s.memo_runs.get() + 1);
            if kind.is_cpu() {
                el().size(40.0, 20.0).fill(Color::WHITE)
            } else {
                el().size(40.0, 20.0).fill(Color::WHITE).blur(6.0)
            }
        })
    }
    let mut rt = Runtime::new(
        State {
            seen: Cell::new(RendererKind::Cpu),
            cleared: Cell::new(RendererKind::Cpu),
            memo_runs: Cell::new(0),
            view_runs: Cell::new(0),
        },
        view,
        (120.0, 60.0),
        Env::default(),
    );
    rt.frame(0.0);
    assert_eq!(rt.state.seen.get(), RendererKind::Gpu);
    assert_eq!(rt.state.cleared.get(), RendererKind::Gpu);
    assert_eq!(rt.state.view_runs.get(), 1);
    assert_eq!(rt.state.memo_runs.get(), 1);

    // Re-setting same value is a no-op.
    rt.set_renderer_kind(RendererKind::Gpu);
    rt.frame(0.0);
    assert_eq!(rt.state.view_runs.get(), 1);
    assert_eq!(rt.state.memo_runs.get(), 1);

    // Switching to Cpu rebuilds both view and memo, while clear_chrome still resets to Gpu.
    rt.set_renderer_kind(RendererKind::Cpu);
    rt.frame(0.0);
    assert_eq!(rt.state.seen.get(), RendererKind::Cpu);
    assert_eq!(rt.state.cleared.get(), RendererKind::Gpu);
    assert_eq!(rt.state.view_runs.get(), 2);
    assert_eq!(rt.state.memo_runs.get(), 2);

    // Switching back to Gpu invalidates again.
    rt.set_renderer_kind(RendererKind::Gpu);
    rt.frame(0.0);
    assert_eq!(rt.state.seen.get(), RendererKind::Gpu);
    assert_eq!(rt.state.view_runs.get(), 3);
    assert_eq!(rt.state.memo_runs.get(), 3);
}

#[test]
fn needs_gpu_classification_covers_every_degradable_and_non_degradable_variant() {
    let degradable = [
        NeedsGpu::LinearBlend,
        NeedsGpu::NativeSurface,
        NeedsGpu::LayerBlur,
        NeedsGpu::TurnedLayer,
        NeedsGpu::BackdropBlur,
        NeedsGpu::Effect,
    ];
    for why in degradable {
        assert_eq!(classify_needs_gpu(why, false), NeedsGpuAction::DegradedPaint(why));
        assert_eq!(classify_needs_gpu(why, true), NeedsGpuAction::DegradedPaint(why));
        assert!(format_surface_description(Some(why)).contains("degraded:"));
    }

    let non_degradable = [
        NeedsGpu::SlugText,
        NeedsGpu::OutlineGlyph,
        NeedsGpu::UnknownKind,
        NeedsGpu::Target,
    ];
    for why in non_degradable {
        assert_eq!(classify_needs_gpu(why, false), NeedsGpuAction::UnavailableFrameOne(why));
        assert_eq!(classify_needs_gpu(why, true), NeedsGpuAction::HoldPreviousFrame(why));
    }
}

#[test]
fn engine_paints_degradable_gpu_features_live_and_clears_degraded_status_when_gone() {
    let mut rt = Runtime::new(
        true,
        |blurred: &bool, _| {
            let card = el().size(48.0, 24.0).fill(Color { r: 0.2, g: 0.5, b: 0.9, a: 0.8 });
            el().w_full().h_full().child(if *blurred { card.blur(6.0) } else { card })
        },
        (64.0, 32.0),
        Env::new(true, 1.0),
    );
    rt.core_mut().transparent = true;
    attach_text(&mut rt.core_mut().text.atlas);
    rt.frame(0.0);

    let mut engine = CpuPaintEngine::new(TestLayeredSeam::default(), 1);
    // Frame 1 has El::blur -> LayerBlur (degradable): paints live, logs once, reports degraded.
    let out = engine.paint_and_present("test", 1, (0, 0), (64, 32), 255, rt.core_mut());
    assert_eq!(
        out,
        CpuPaintOutcome::Presented {
            degraded: Some(NeedsGpu::LayerBlur),
            newly_logged: Some(NeedsGpu::LayerBlur),
        }
    );
    assert_eq!(
        format_surface_description(engine.active_degraded()),
        "CpuPainter (Win32 layered, degraded: LayerBlur)"
    );
    // Transparent background corner (outside 48x24 card in 64x32 target) stayed zero alpha;
    // premultiplied BGRA invariant holds everywhere.
    let px = &engine.presenter().seam().last_pixels;
    let corner = ((28 * 64 + 60) * 4) as usize;
    assert_eq!(&px[corner..corner + 4], &[0, 0, 0, 0]);
    for p in px.chunks_exact(4) {
        assert!(p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3], "premul BGRA violated: {p:?}");
    }

    // Removing the blur returns to non-degraded CpuPainter.
    rt.state = false;
    rt.invalidate();
    rt.settle_all();
    rt.frame(0.0);
    let out = engine.paint_and_present("test", 1, (0, 0), (64, 32), 255, rt.core_mut());
    assert!(matches!(out, CpuPaintOutcome::Presented { degraded: None, newly_logged: None } | CpuPaintOutcome::Unchanged { degraded: None }));
    assert_eq!(format_surface_description(engine.active_degraded()), "CpuPainter (Win32 layered, ULW_ALPHA)");

    // Test every other degradable GPU-only construct directly on instances:
    // 1. TurnedLayer (non-90 degree rotation)
    let card_inst = Instance {
        rect: [8.0, 4.0, 24.0, 16.0],
        radii: [0.0; 4],
        fill: Color::WHITE.premul_srgb(1.0),
        border: [0.0; 4],
        params: [0.0, KIND_RECT, 0.0, 1.0],
        uv: [0.0; 4],
        clip: NO_CLIP,
        fade: [0; 4],
    };
    let mut turned = Instance::begin_layer([0.0, 0.0, 64.0, 32.0], NO_CLIP, LayerSpec::default());
    turned.border[1] = 0.5; // ~28.6 deg
    rt.core_mut().instances = vec![turned, card_inst, Instance::end_layer()];
    let out = engine.paint_and_present("test", 1, (0, 0), (64, 32), 255, rt.core_mut());
    assert!(matches!(out, CpuPaintOutcome::Presented { degraded: Some(NeedsGpu::TurnedLayer), .. } | CpuPaintOutcome::Unchanged { degraded: Some(NeedsGpu::TurnedLayer) }));
    assert_eq!(engine.active_degraded(), Some(NeedsGpu::TurnedLayer));

    // 2. Nested BackdropBlur inside a layer
    let blur_inst = Instance {
        rect: [4.0, 4.0, 32.0, 20.0],
        radii: [0.0; 4],
        fill: [0.0, 0.0, 0.0, 1.0],
        border: [0.0; 4],
        params: [6.0, KIND_BACKDROP_BLUR, 0.0, 1.0],
        uv: [0.0; 4],
        clip: NO_CLIP,
        fade: [0; 4],
    };
    rt.core_mut().instances = vec![
        Instance::begin_layer([0.0, 0.0, 64.0, 32.0], NO_CLIP, LayerSpec::default()),
        card_inst,
        blur_inst,
        Instance::end_layer(),
    ];
    let out = engine.paint_and_present("test", 1, (0, 0), (64, 32), 255, rt.core_mut());
    assert!(matches!(out, CpuPaintOutcome::Presented { degraded: Some(NeedsGpu::BackdropBlur), .. } | CpuPaintOutcome::Unchanged { degraded: Some(NeedsGpu::BackdropBlur) }));
    assert_eq!(engine.active_degraded(), Some(NeedsGpu::BackdropBlur));

    // 3. LinearBlend (srgb_blend == false)
    rt.core_mut().instances = vec![card_inst];
    rt.core_mut().set_srgb_blend(false);
    let out = engine.paint_and_present("test", 1, (0, 0), (64, 32), 255, rt.core_mut());
    assert!(matches!(out, CpuPaintOutcome::Presented { degraded: Some(NeedsGpu::LinearBlend), .. } | CpuPaintOutcome::Unchanged { degraded: Some(NeedsGpu::LinearBlend) }));
    assert_eq!(engine.active_degraded(), Some(NeedsGpu::LinearBlend));
    rt.core_mut().set_srgb_blend(true);

    // 4. Effect instance (with compositor == false it is ignored cleanly; check with plain card)
    let effect_inst = Instance { params: [0.0, KIND_EFFECT, 0.0, 1.0], ..card_inst };
    rt.core_mut().instances = vec![effect_inst, card_inst];
    let out = engine.paint_and_present("test", 1, (0, 0), (64, 32), 255, rt.core_mut());
    assert!(matches!(out, CpuPaintOutcome::Presented { degraded: None, .. } | CpuPaintOutcome::Unchanged { degraded: None }));
}

#[test]
fn engine_handles_non_degradable_needs_gpu_on_frame_one_and_mid_session() {
    // Frame 1 non-degradable (SlugText): fails Availability::Unavailable, never presents blank.
    let mut rt = Runtime::new(
        (),
        |_, _| el().size(40.0, 20.0).fill(Color::WHITE),
        (64.0, 32.0),
        Env::new(true, 1.0),
    );
    rt.core_mut().set_font_smoothing(FontSmoothing::Slug);
    rt.frame(0.0);
    let mut engine = CpuPaintEngine::new(TestLayeredSeam::default(), 1);
    let out = engine.paint_and_present("slug-overlay", 1, (0, 0), (64, 32), 255, rt.core_mut());
    assert!(matches!(out, CpuPaintOutcome::FrameOneUnavailable { reason: NeedsGpu::SlugText, .. }));
    assert!(engine.presenter().seam().updates.is_empty(), "must never present a blank window on frame 1 failure");

    // Mid-session non-degradable: paints frame 1 cleanly, then holds previous frame on SlugText.
    rt.core_mut().set_font_smoothing(FontSmoothing::default());
    rt.frame(0.0);
    let out1 = engine.paint_and_present("slug-overlay", 1, (0, 0), (64, 32), 255, rt.core_mut());
    assert!(matches!(out1, CpuPaintOutcome::Presented { degraded: None, .. }));
    let updates_after_1 = engine.presenter().seam().updates.len();

    rt.core_mut().set_font_smoothing(FontSmoothing::Slug);
    let out2 = engine.paint_and_present("slug-overlay", 1, (0, 0), (64, 32), 255, rt.core_mut());
    assert_eq!(out2, CpuPaintOutcome::HeldPreviousFrame { reason: NeedsGpu::SlugText, newly_logged: false });
    assert_eq!(engine.presenter().seam().updates.len(), updates_after_1, "must hold previous frame without overwriting");
}

#[test]
fn engine_steady_fade_dpi_hide_and_initial_seam_failure() {
    let mut rt = Runtime::new(
        Color { r: 0.1, g: 0.6, b: 0.9, a: 0.9 },
        |c: &Color, _| el().pin(Some(4.0), Some(4.0), None, None).size(24.0, 16.0).fill(*c),
        (64.0, 32.0),
        Env::new(true, 1.0),
    );
    rt.core_mut().transparent = true;
    rt.frame(0.0);

    let mut engine = CpuPaintEngine::new(TestLayeredSeam::default(), 1);
    // Frame 1: full present (prcDirty = None), ULW_ALPHA, alpha = 255.
    let out1 = engine.paint_and_present("pill", 42, (10, 20), (64, 32), 255, rt.core_mut());
    assert!(matches!(out1, CpuPaintOutcome::Presented { degraded: None, .. }));
    assert_eq!(engine.presenter().seam().updates[0].1.dirty, None);
    assert_eq!(engine.presenter().seam().updates[0].1.flags, ULW_ALPHA);
    assert_eq!(engine.presenter().seam().updates[0].1.blend, BlendSpec::per_pixel(255));

    // Steady unchanged frame skips UpdateLayeredWindowIndirect.
    let out2 = engine.paint_and_present("pill", 42, (10, 20), (64, 32), 255, rt.core_mut());
    assert!(matches!(out2, CpuPaintOutcome::Unchanged { degraded: None }));
    assert_eq!(engine.presenter().seam().updates.len(), 1);

    // Recolouring card produces a partial prcDirty update without reallocating the DIB.
    rt.state = Color { r: 0.9, g: 0.2, b: 0.3, a: 0.9 };
    rt.invalidate();
    rt.frame(0.0);
    let out3 = engine.paint_and_present("pill", 42, (10, 20), (64, 32), 255, rt.core_mut());
    assert!(matches!(out3, CpuPaintOutcome::Presented { degraded: None, .. }));
    assert_eq!(engine.presenter().seam().created.len(), 1);
    assert!(engine.presenter().seam().updates[1].1.dirty.is_some());

    // Fade-out steps SourceConstantAlpha via present_fade without repainting.
    assert!(engine.present_fade(42, (10, 20), 128).unwrap());
    assert_eq!(engine.presenter().seam().updates[2].1.blend, BlendSpec::per_pixel(128));
    assert_eq!(engine.presenter().seam().updates[2].1.dirty, None);

    // Hide frees the DIB; next show reallocates cleanly.
    engine.on_hide();
    assert!(!engine.presenter().has_surface());

    // Initial seam failure signals fallback cleanly.
    let mut fail_seam = TestLayeredSeam::default();
    fail_seam.fail_update = Some("ERROR_INVALID_PARAMETER".into());
    let mut fail_engine = CpuPaintEngine::new(fail_seam, 1);
    let out_fail = fail_engine.paint_and_present("pill", 42, (0, 0), (64, 32), 255, rt.core_mut());
    assert!(matches!(out_fail, CpuPaintOutcome::SeamFailed { initial: true, .. }));
}
