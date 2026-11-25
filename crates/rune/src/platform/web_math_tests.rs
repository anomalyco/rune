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
    assert_eq!(surface_usage(render_only, Backend::Gl), TextureUsages::RENDER_ATTACHMENT);
    assert_eq!(
        surface_usage(render_only | TextureUsages::COPY_SRC, Backend::Gl),
        TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
    );
}

#[test]
fn webgpu_surface_usage_always_includes_copy_src_for_backdrop_blur() {
    use wgpu::{Backend, TextureUsages};
    let render_only = TextureUsages::RENDER_ATTACHMENT;
    assert_eq!(
        surface_usage(render_only, Backend::BrowserWebGpu),
        TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_DST | TextureUsages::COPY_SRC,
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


#[test]
fn web_gate_skips_suspended_waiting_context_lost_and_moving() {
    use crate::frame::host_step::Gate;
    assert!(matches!(web_frame_gate(true, WebDrawKind::Gpu, true), Gate::Skip));
    assert!(matches!(web_frame_gate(true, WebDrawKind::Cpu, false), Gate::Skip));
    assert!(matches!(web_frame_gate(false, WebDrawKind::ContextLost, true), Gate::Skip));
    assert!(matches!(web_frame_gate(false, WebDrawKind::Moving, true), Gate::Skip));
    assert!(matches!(web_frame_gate(false, WebDrawKind::HandingOver { waiting_for_gpu: true }, true), Gate::Skip));
    assert!(matches!(web_frame_gate(false, WebDrawKind::HandingOver { waiting_for_gpu: false }, true), Gate::Draw));
    assert!(matches!(web_frame_gate(false, WebDrawKind::Cpu, true), Gate::Draw));
    assert!(matches!(web_frame_gate(false, WebDrawKind::Gpu, false), Gate::Draw));
    assert!(matches!(web_frame_gate(false, WebDrawKind::Gpu, true), Gate::QuietTimer));
}

#[test]
fn web_acquire_and_miss_policies_distinguish_transient_from_fatal() {
    use crate::window::Missed;
    assert_eq!(web_acquire_action(WebAcquireStatus::Ready, 1), WebAcquireAction::UseTexture);
    assert_eq!(web_acquire_action(WebAcquireStatus::Timeout, 1), WebAcquireAction::Miss(Missed::Timeout));
    assert_eq!(web_acquire_action(WebAcquireStatus::Occluded, 1), WebAcquireAction::Miss(Missed::Occluded));
    assert_eq!(web_acquire_action(WebAcquireStatus::OutdatedOrLost, 1), WebAcquireAction::ReconfigureAndRetry);
    assert_eq!(web_acquire_action(WebAcquireStatus::OutdatedOrLost, 0), WebAcquireAction::Miss(Missed::Unconfigured));
    assert_eq!(web_acquire_action(WebAcquireStatus::Validation, 1), WebAcquireAction::Miss(Missed::Unconfigured));

    assert_eq!(web_miss_action(Missed::Unconfigured), WebMissAction::Fatal);
    assert_eq!(web_miss_action(Missed::Occluded), WebMissAction::CoveredOnly);
    assert_eq!(web_miss_action(Missed::Timeout), WebMissAction::RetryNextFrame);
    assert_eq!(web_miss_action(Missed::Outdated), WebMissAction::RetryNextFrame);
}

#[test]
fn rtl_scroll_x_normalizes_all_browser_models_and_round_trips() {
    let max = 400.0;
    // LTR: identity clamped to [0, max].
    for model in [RtlScrollModel::Negative, RtlScrollModel::Reverse, RtlScrollModel::Default] {
        assert_eq!(scroll_x_from_dom(0.0, max, false, model), 0.0);
        assert_eq!(scroll_x_from_dom(120.0, max, false, model), 120.0);
        assert_eq!(scroll_x_from_dom(400.0, max, false, model), 400.0);
        assert_eq!(scroll_x_from_dom(-25.0, max, false, model), 0.0, "LTR rubber-band past start clamps to 0");
        assert_eq!(scroll_x_from_dom(430.0, max, false, model), 400.0, "LTR overscroll past end clamps to max");
        assert_eq!(scroll_x_to_dom(120.0, max, false, model), 120.0);
    }

    // RTL Negative (CSSOM View standard: modern Chrome/Firefox/Safari): 0 at start, -max at end.
    assert_eq!(scroll_x_from_dom(0.0, max, true, RtlScrollModel::Negative), 0.0);
    assert_eq!(scroll_x_from_dom(-80.0, max, true, RtlScrollModel::Negative), 80.0);
    assert_eq!(scroll_x_from_dom(-400.0, max, true, RtlScrollModel::Negative), 400.0);
    assert_eq!(scroll_x_from_dom(20.0, max, true, RtlScrollModel::Negative), 0.0, "RTL negative rubber-band past start");
    assert_eq!(scroll_x_from_dom(-425.0, max, true, RtlScrollModel::Negative), 400.0, "RTL negative rubber-band past end");
    let dom_zero = scroll_x_to_dom(0.0, max, true, RtlScrollModel::Negative);
    assert_eq!(dom_zero, 0.0);
    assert!(!dom_zero.is_sign_negative(), "start offset writes +0.0, not -0.0");
    assert_eq!(scroll_x_to_dom(80.0, max, true, RtlScrollModel::Negative), -80.0);
    assert_eq!(scroll_x_to_dom(400.0, max, true, RtlScrollModel::Negative), -400.0);

    // RTL Reverse (legacy Edge/IE): 0 at start, +max at end.
    assert_eq!(scroll_x_from_dom(0.0, max, true, RtlScrollModel::Reverse), 0.0);
    assert_eq!(scroll_x_from_dom(80.0, max, true, RtlScrollModel::Reverse), 80.0);
    assert_eq!(scroll_x_from_dom(400.0, max, true, RtlScrollModel::Reverse), 400.0);
    assert_eq!(scroll_x_from_dom(-15.0, max, true, RtlScrollModel::Reverse), 0.0);
    assert_eq!(scroll_x_from_dom(420.0, max, true, RtlScrollModel::Reverse), 400.0);
    assert_eq!(scroll_x_to_dom(0.0, max, true, RtlScrollModel::Reverse), 0.0);
    assert_eq!(scroll_x_to_dom(80.0, max, true, RtlScrollModel::Reverse), 80.0);
    assert_eq!(scroll_x_to_dom(400.0, max, true, RtlScrollModel::Reverse), 400.0);

    // RTL Default (legacy Blink/WebKit): +max at start, 0 at end.
    assert_eq!(scroll_x_from_dom(400.0, max, true, RtlScrollModel::Default), 0.0);
    assert_eq!(scroll_x_from_dom(320.0, max, true, RtlScrollModel::Default), 80.0);
    assert_eq!(scroll_x_from_dom(0.0, max, true, RtlScrollModel::Default), 400.0);
    assert_eq!(scroll_x_from_dom(430.0, max, true, RtlScrollModel::Default), 0.0, "RTL default rubber-band past start");
    assert_eq!(scroll_x_from_dom(-20.0, max, true, RtlScrollModel::Default), 400.0, "RTL default rubber-band past end");
    assert_eq!(scroll_x_to_dom(0.0, max, true, RtlScrollModel::Default), 400.0);
    assert_eq!(scroll_x_to_dom(80.0, max, true, RtlScrollModel::Default), 320.0);
    assert_eq!(scroll_x_to_dom(400.0, max, true, RtlScrollModel::Default), 0.0);

    // Round-trip invariant across every model, direction, and edge value.
    for rtl in [false, true] {
        for model in [RtlScrollModel::Negative, RtlScrollModel::Reverse, RtlScrollModel::Default] {
            for offset in [0.0, 0.5, 80.0, 199.75, 400.0] {
                let dom = scroll_x_to_dom(offset, max, rtl, model);
                let back = scroll_x_from_dom(dom, max, rtl, model);
                assert!((back - offset).abs() < 1e-4, "round-trip mismatch for rtl={rtl} model={model:?} offset={offset}: dom={dom} -> {back}");
            }
            assert_eq!(scroll_x_from_dom(f32::NAN, max, rtl, model), 0.0);
            assert_eq!(scroll_x_from_dom(f32::INFINITY, max, rtl, model), 0.0);
            assert_eq!(scroll_x_to_dom(f32::NAN, max, rtl, model), 0.0);
            assert_eq!(scroll_x_to_dom(100.0, 0.0, rtl, model), 0.0);
            assert_eq!(scroll_x_from_dom(-100.0, 0.0, rtl, model), 0.0);
        }
    }
}

#[test]
fn web_runtime_tracks_marks_only_when_compositor_is_attached() {
    use std::sync::Arc;
    use crate::compose::{Compositor, Frame};
    use crate::render::KIND_EFFECT;
    use crate::{App, AppSpec, Cx, El, Env, col, effect};

    struct NoopCompositor;
    impl Compositor<()> for NoopCompositor {
        fn render(&mut self, f: &mut Frame<'_, ()>) {
            f.draw_ui();
        }
    }

    let view = |_: &(), _: &mut Cx<()>| -> El {
        col().size(120.0, 80.0).child(effect(42).size(60.0, 20.0))
    };

    let spec_with = AppSpec::new("web-marks-on", (120.0, 80.0), || (), view);
    let (mut rt_with, _, comp_with, _, _) = start_web_runtime(
        App::new(spec_with).compositor(NoopCompositor),
        (120.0, 80.0),
        Env::default(),
        Arc::new(|| {}),
        Arc::new(|| {}),
    );
    assert!(comp_with.is_some());
    assert!(rt_with.track_marks, "web runtime with a compositor must enable track_marks");
    rt_with.frame(0.0);
    assert_eq!(rt_with.marks.len(), rt_with.instances.len());
    let effect_marks: Vec<u32> = rt_with
        .instances
        .iter()
        .enumerate()
        .filter(|(_, inst)| inst.params[1] == KIND_EFFECT)
        .map(|(i, _)| rt_with.marks[i])
        .collect();
    assert_eq!(effect_marks, [42]);

    let spec_without = AppSpec::new("web-marks-off", (120.0, 80.0), || (), view);
    let (mut rt_without, _, comp_without, _, _) = start_web_runtime(
        App::new(spec_without),
        (120.0, 80.0),
        Env::default(),
        Arc::new(|| {}),
        Arc::new(|| {}),
    );
    assert!(comp_without.is_none());
    assert!(!rt_without.track_marks, "web runtime without a compositor keeps track_marks off");
    rt_without.frame(0.0);
    assert!(rt_without.marks.is_empty());
}

#[test]
fn web_runtime_routes_live_wakes_separately_from_app_wakes() {
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
    use crate::{App, AppSpec, Cx, El, Env, Live, el};

    let app_wakes = Arc::new(AtomicUsize::new(0));
    let live_wakes = Arc::new(AtomicUsize::new(0));
    let app_c = app_wakes.clone();
    let live_c = live_wakes.clone();
    let spec = AppSpec::new(
        "web-live-lane",
        (120.0, 80.0),
        || Live::new([0.0f32, 0.0]),
        |offset: &Live<[f32; 2]>, _: &mut Cx<Live<[f32; 2]>>| -> El {
            el().size(120.0, 80.0).child(el().size(10.0, 10.0).live_offset(offset))
        },
    );
    let (mut rt, _, _, _, _) = start_web_runtime(
        App::new(spec),
        (120.0, 80.0),
        Env::default(),
        Arc::new(move || { app_c.fetch_add(1, Ordering::Relaxed); }),
        Arc::new(move || { live_c.fetch_add(1, Ordering::Relaxed); }),
    );
    rt.frame(0.0);
    let commits = rt.stats.commits;

    rt.state.set([12.0, 0.0]);
    assert_eq!(live_wakes.load(Ordering::Relaxed), 1, "Live publication uses the Wake::Live lane");
    assert_eq!(app_wakes.load(Ordering::Relaxed), 0, "Live publication must not wake Wake::App");
    assert!(rt.live_pending());
    rt.frame(1.0 / 60.0);
    assert!(!rt.live_pending());
    assert_eq!(rt.stats.commits, commits, "Live update does not rebuild the view");
}

mod web_frame_host_tests {
    use std::collections::VecDeque;
    use std::time::Duration;

    use super::*;
    use crate::frame::clock::FrameClock;
    use crate::frame::host_step::{
        self, Acquire, FrameHost, Gate, Outcome, PostStepCheck, ReacquireReason, RenderPrep, StepInfo,
    };
    use crate::runtime::RuntimeCore;
    use crate::time::Instant;
    use crate::window::Missed;
    use crate::{Cx, El, Env, Runtime, Tx, el};

    struct MockState {
        width: f32,
        polls: u32,
        switch_blend_to: Option<bool>,
        offset: crate::Live<[f32; 2]>,
        next_live_x: Option<f32>,
    }

    impl Default for MockState {
        fn default() -> Self {
            Self {
                width: 0.0,
                polls: 0,
                switch_blend_to: None,
                offset: crate::Live::new([0.0, 0.0]),
                next_live_x: None,
            }
        }
    }

    fn view(s: &MockState, _: &mut Cx<MockState>) -> El {
        el().child(el().key("bar").size(s.width + 12.0, 8.0).fill(crate::Color::WHITE).live_offset(&s.offset))
    }

    fn poll(s: &mut MockState, _: f64, tx: &mut Tx) -> bool {
        s.polls += 1;
        if let Some(next) = s.switch_blend_to.take() {
            tx.set_srgb_blend(next);
        }
        if let Some(x) = s.next_live_x.take() {
            s.offset.set([x, 0.0]);
            tx.wake_after(Duration::from_millis(40));
        }
        false
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum MockDrawable {
        Cpu,
        Gpu(u32),
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum MockPrepared {
        Cpu,
        HandingOver { gpu: Option<u32> },
        Gpu(u32),
    }

    /// Browser-free stand-in for `platform::web::frame::WebFrameHost` sharing the exact
    /// `web_math` gate/acquire/miss/reblend decisions through `frame::host_step::run`.
    struct MockWebHost {
        rt: Runtime<MockState>,
        clock: FrameClock,
        suspended: bool,
        draw: WebDrawKind,
        handover_built: bool,
        full_redraw: bool,
        force_cycle_tick: bool,
        renderer_srgb: bool,
        reblend_supports_linear: bool,
        acquires: VecDeque<WebAcquireStatus>,
        next_tex: u32,
        step_at: Instant,
        submit_at: Instant,
        reblend_tried: Option<bool>,
        tick: bool,
        simulate_native_surface: bool,
        fatal_error: Option<&'static str>,
        redraw_requested: u32,
        reconfigures: u32,
        handover_gpu_acquired: u32,
        frames_drawn: u32,
        compositor_draws_own: bool,
        last_presented: Option<crate::window::Presented>,
        config: wgpu::SurfaceConfiguration,
        log: Vec<&'static str>,
    }

    impl MockWebHost {
        fn new(start: Instant, draw: WebDrawKind) -> Self {
            let mut rt = Runtime::new(MockState::default(), view, (300.0, 40.0), Env::default());
            rt.poll = Some(poll);
            rt.set_system_reduced_motion(false);
            rt.wall_start = Some(start);
            rt.set_srgb_blend(true);
            let config = wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: wgpu::TextureFormat::Bgra8Unorm,
                width: 300,
                height: 40,
                present_mode: wgpu::PresentMode::Fifo,
                desired_maximum_frame_latency: 2,
                alpha_mode: wgpu::CompositeAlphaMode::Opaque,
                view_formats: Vec::new(),
                color_space: Default::default(),
            };
            Self {
                rt,
                clock: FrameClock::new(start),
                suspended: false,
                draw,
                handover_built: false,
                full_redraw: true,
                force_cycle_tick: false,
                renderer_srgb: true,
                reblend_supports_linear: true,
                acquires: VecDeque::new(),
                next_tex: 1,
                step_at: start,
                submit_at: start,
                reblend_tried: None,
                tick: false,
                simulate_native_surface: false,
                fatal_error: None,
                redraw_requested: 0,
                reconfigures: 0,
                handover_gpu_acquired: 0,
                frames_drawn: 0,
                compositor_draws_own: false,
                last_presented: None,
                config,
                log: Vec::new(),
            }
        }

        fn acquire_gpu_tex(&mut self) -> Result<u32, Missed> {
            let mut retries = 1u8;
            loop {
                let status = self.acquires.pop_front().unwrap_or(WebAcquireStatus::Ready);
                match web_acquire_action(status, retries) {
                    WebAcquireAction::UseTexture => {
                        let id = self.next_tex;
                        self.next_tex += 1;
                        return Ok(id);
                    }
                    WebAcquireAction::ReconfigureAndRetry => {
                        self.reconfigures += 1;
                        self.log.push("reconfigure");
                        retries -= 1;
                    }
                    WebAcquireAction::Miss(missed) => return Err(missed),
                }
            }
        }
    }

    impl FrameHost for MockWebHost {
        type Drawable = MockDrawable;
        type Prepared = MockPrepared;

        fn gate(&mut self) -> Gate {
            self.reblend_tried = None;
            self.tick = false;
            self.fatal_error = None;
            let quiet_candidate = self.draw == WebDrawKind::Gpu
                && self.last_presented.is_some()
                && !self.compositor_draws_own
                && self.renderer_srgb == self.rt.srgb_blend();
            let gate = web_frame_gate(self.suspended, self.draw, quiet_candidate);
            self.log.push(match gate {
                Gate::Skip => "gate:skip",
                Gate::QuietTimer => "gate:quiet",
                _ => "gate:draw",
            });
            gate
        }

        fn needs_reblend(&self) -> bool {
            self.draw == WebDrawKind::Gpu && self.renderer_srgb != self.rt.srgb_blend()
        }

        fn reblend(&mut self) -> bool {
            let target = self.rt.srgb_blend();
            self.reblend_tried = Some(target);
            self.log.push("reblend");
            self.renderer_srgb = if target || self.reblend_supports_linear { target } else { true };
            self.last_presented = None;
            true
        }

        fn step_quiet(&mut self) -> StepInfo {
            self.step_runtime()
        }

        fn is_unchanged(&mut self, info: &StepInfo) -> bool {
            self.draw == WebDrawKind::Gpu
                && !self.tick
                && !self.compositor_draws_own
                && self.renderer_srgb == self.rt.srgb_blend()
                && self.last_presented.as_ref().is_some_and(|p| {
                    p.unchanged_core(self.rt.core(), &self.config, info.animating, None)
                })
        }

        fn on_unchanged(&mut self, _info: &StepInfo) {
            self.log.push("unchanged");
        }

        fn acquire(&mut self) -> Acquire<MockDrawable> {
            match self.draw {
                WebDrawKind::Gpu => match self.acquire_gpu_tex() {
                    Ok(id) => {
                        self.log.push("acquire:gpu");
                        Acquire::Ready(MockDrawable::Gpu(id))
                    }
                    Err(missed) => {
                        self.log.push("acquire:missed");
                        Acquire::Missed(missed)
                    }
                },
                WebDrawKind::Cpu | WebDrawKind::HandingOver { .. } => {
                    self.log.push("acquire:cpu");
                    Acquire::Ready(MockDrawable::Cpu)
                }
                WebDrawKind::ContextLost | WebDrawKind::TerminalFailed | WebDrawKind::Moving => Acquire::Missed(Missed::Unconfigured),
            }
        }

        fn step_runtime(&mut self) -> StepInfo {
            self.log.push("step");
            let full = std::mem::take(&mut self.full_redraw);
            let tick = !full && self.draw == WebDrawKind::Gpu && self.force_cycle_tick;
            self.tick = tick;
            let (animating, dt) = if tick {
                self.log.push("step:cycle_tick");
                let dt = self.step_at.saturating_duration_since(self.clock.last()).as_secs_f64();
                self.clock.hold(self.step_at);
                (false, dt)
            } else {
                let (core, mut app) = self.rt.split_mut();
                self.clock.advance_core(core, &mut app, self.step_at, false)
            };
            StepInfo { runtime_animating: animating, animating, dt, update_ms: 0.0, presents_at: None }
        }

        fn post_step_check(&mut self, _info: &mut StepInfo) -> PostStepCheck {
            if self.draw == WebDrawKind::Gpu
                && should_post_step_reblend(self.renderer_srgb, self.rt.srgb_blend(), self.reblend_tried)
            {
                self.log.push("post:reblend");
                PostStepCheck::Reacquire(ReacquireReason::Reblend)
            } else {
                PostStepCheck::Keep
            }
        }

        fn prepare_reacquire(&mut self, reason: ReacquireReason, _info: &mut StepInfo) -> bool {
            match reason {
                ReacquireReason::Reblend => self.reblend(),
                ReacquireReason::NativeMaterials => true,
                #[cfg(not(target_arch = "wasm32"))]
                ReacquireReason::ContentResize => true,
            }
        }

        fn on_missed(&mut self, missed: Missed, stepped: bool) {
            self.last_presented = None;
            match web_miss_action(missed) {
                WebMissAction::Fatal => {
                    self.log.push("missed:fatal");
                    self.fatal_error = Some("WebGPU canvas acquisition failed");
                }
                WebMissAction::CoveredOnly => {
                    self.log.push("missed:occluded");
                    if !stepped { self.rt.covered_wake(); }
                }
                WebMissAction::RetryNextFrame => {
                    self.log.push("missed:retry");
                    if !stepped { self.rt.covered_wake(); }
                    self.redraw_requested += 1;
                }
            }
        }

        fn sync_after_step(&mut self, _info: &mut StepInfo) {
            self.log.push("sync");
            if self.simulate_native_surface {
                self.fatal_error = Some("native video surfaces are unsupported on web");
            }
        }

        fn prepare_render(&mut self, drawable: MockDrawable, _info: &mut StepInfo) -> RenderPrep<MockPrepared> {
            if self.fatal_error.is_some() {
                self.log.push("prepare:abort");
                return RenderPrep::Abort;
            }
            match (self.draw, drawable) {
                (WebDrawKind::Cpu, MockDrawable::Cpu) => {
                    self.log.push("prepare:cpu");
                    RenderPrep::Ready(MockPrepared::Cpu)
                }
                (WebDrawKind::HandingOver { .. }, MockDrawable::Cpu) => {
                    let gpu = if self.handover_built {
                        self.handover_gpu_acquired += 1;
                        self.acquire_gpu_tex().ok()
                    } else {
                        None
                    };
                    self.log.push(if gpu.is_some() { "prepare:handover_gpu" } else { "prepare:handover_cpu" });
                    RenderPrep::Ready(MockPrepared::HandingOver { gpu })
                }
                (WebDrawKind::Gpu, MockDrawable::Gpu(id)) => {
                    self.log.push("prepare:gpu");
                    RenderPrep::Ready(MockPrepared::Gpu(id))
                }
                _ => RenderPrep::Abort,
            }
        }

        fn render(&mut self, _prepared: &mut MockPrepared, _info: &StepInfo) -> bool {
            self.log.push("render");
            false
        }

        fn submit_and_present(&mut self, _prepared: MockPrepared, _info: &StepInfo) -> Instant {
            self.log.push("present");
            self.frames_drawn += 1;
            self.submit_at
        }

        fn present_targets(&mut self) -> (Option<&mut FrameClock>, &mut RuntimeCore) {
            (Some(&mut self.clock), self.rt.core_mut())
        }

        fn settle(&mut self, _submitted: Instant, info: &StepInfo) {
            self.log.push("settle");
            if self.draw == WebDrawKind::Gpu {
                let rt = self.rt.core();
                if should_record_presented(true, info.animating, rt.stale(), rt.host.window_frame.is_some()) {
                    crate::window::Presented::record_core(
                        &mut self.last_presented,
                        rt,
                        &self.config,
                        self.compositor_draws_own,
                    );
                } else {
                    self.last_presented = None;
                }
            } else {
                self.last_presented = None;
            }
            if info.animating || self.rt.has_pending_input() || self.rt.live_pending() {
                self.redraw_requested += 1;
            }
        }
    }

    #[test]
    fn gpu_frame_acquires_before_step_and_delays_overrun_on_clock() {
        let start = crate::time::now();
        let frame = Duration::from_micros(16_667);
        let overrun = Duration::from_millis(50);
        let mut host = MockWebHost::new(start, WebDrawKind::Gpu);

        assert_eq!(host_step::run(&mut host), Outcome::Presented);
        assert_eq!(host.log, ["gate:draw", "acquire:gpu", "step", "sync", "prepare:gpu", "render", "present", "settle"]);

        // Start a spring on an overrunning frame: host_step::run calls clock.presented_core.
        host.rt.update(|s, _| s.width = 200.0);
        let f1 = start + frame;
        host.step_at = f1;
        host.submit_at = f1 + overrun;
        assert_eq!(host_step::run(&mut host), Outcome::Presented);
        let w1 = host.rt.presented("bar").unwrap().0[2];

        let f2 = f1 + overrun + frame;
        host.step_at = f2;
        host.submit_at = f2;
        assert_eq!(host_step::run(&mut host), Outcome::Presented);
        let w2 = host.rt.presented("bar").unwrap().0[2];

        // Reference without clock.presented_core: would jump ahead by the 50 ms overrun.
        let mut plain_clock = FrameClock::new(start);
        let mut plain = Runtime::new(MockState::default(), view, (300.0, 40.0), Env::default());
        plain.set_system_reduced_motion(false);
        plain.wall_start = Some(start);
        plain_clock.advance(&mut plain, start, false);
        plain.update(|s, _| s.width = 200.0);
        plain_clock.advance(&mut plain, f1, false);
        plain_clock.advance(&mut plain, f2, false);
        let undelayed_w2 = plain.presented("bar").unwrap().0[2];

        assert!(w2 > w1);
        assert!(undelayed_w2 - w2 > 5.0, "web host_step must record presentation on clock: w2={w2}, undelayed={undelayed_w2}");
    }

    #[test]
    fn gpu_transient_and_outdated_acquire_follow_web_recovery_policy() {
        let start = crate::time::now();
        let mut host = MockWebHost::new(start, WebDrawKind::Gpu);
        // Transient Timeout: missed before step -> runs covered_wake once, requests redraw, not fatal.
        host.acquires = VecDeque::from([WebAcquireStatus::Timeout]);
        let polls0 = host.rt.state.polls;
        assert_eq!(host_step::run(&mut host), Outcome::Missed);
        assert_eq!(host.rt.state.polls, polls0 + 1);
        assert_eq!(host.redraw_requested, 1);
        assert!(host.fatal_error.is_none());
        assert!(host.full_redraw, "missed acquire before step preserves full_redraw");

        // Transient Occluded: covered_wake runs, no redraw requested, not fatal.
        host.acquires = VecDeque::from([WebAcquireStatus::Occluded]);
        assert_eq!(host_step::run(&mut host), Outcome::Missed);
        assert_eq!(host.redraw_requested, 1);
        assert!(host.fatal_error.is_none());

        // Single Outdated -> reconfigures and retries once, then presents.
        host.log.clear();
        host.acquires = VecDeque::from([WebAcquireStatus::OutdatedOrLost, WebAcquireStatus::Ready]);
        assert_eq!(host_step::run(&mut host), Outcome::Presented);
        assert_eq!(host.reconfigures, 1);
        assert!(host.fatal_error.is_none());

        // Persistent Lost after reconfigure retry -> fatal error.
        host.last_presented = None;
        host.acquires = VecDeque::from([WebAcquireStatus::OutdatedOrLost, WebAcquireStatus::OutdatedOrLost]);
        assert_eq!(host_step::run(&mut host), Outcome::Missed);
        assert_eq!(host.reconfigures, 2);
        assert_eq!(host.fatal_error, Some("WebGPU canvas acquisition failed"));
    }

    #[test]
    fn post_step_reblend_reacquires_once_and_does_not_loop_without_srgb_twin() {
        let start = crate::time::now();
        let mut host = MockWebHost::new(start, WebDrawKind::Gpu);
        // Handler flips srgb_blend during step: drops old drawable, reblends, reacquires once.
        host.rt.state.switch_blend_to = Some(false);
        assert_eq!(host_step::run(&mut host), Outcome::Presented);
        assert_eq!(
            host.log,
            ["gate:draw", "acquire:gpu", "step", "post:reblend", "reblend", "acquire:gpu", "sync", "prepare:gpu", "render", "present", "settle"]
        );

        // WebGL2 without sRGB twin: renderer stays srgb_blend=true while rt wants false.
        // Pre-step reblend tries once; post_step_check must NOT loop re-acquiring.
        host.log.clear();
        host.renderer_srgb = true;
        host.reblend_supports_linear = false;
        assert_eq!(host_step::run(&mut host), Outcome::Presented);
        assert_eq!(
            host.log,
            ["gate:draw", "reblend", "acquire:gpu", "step", "sync", "prepare:gpu", "render", "present", "settle"]
        );
    }

    #[test]
    fn cycle_tick_handover_and_native_surface_abort_follow_host_step() {
        let start = crate::time::now();
        // Cycle tick holds clock without advancing runtime poll.
        let mut gpu_host = MockWebHost::new(start, WebDrawKind::Gpu);
        assert_eq!(host_step::run(&mut gpu_host), Outcome::Presented);
        let polls_after_first = gpu_host.rt.state.polls;
        gpu_host.log.clear();
        gpu_host.force_cycle_tick = true;
        gpu_host.step_at = start + Duration::from_millis(16);
        gpu_host.submit_at = gpu_host.step_at;
        assert_eq!(host_step::run(&mut gpu_host), Outcome::Presented);
        assert_eq!(gpu_host.rt.state.polls, polls_after_first, "paint_cycle_tick skips full runtime step");
        assert!(gpu_host.log.contains(&"step:cycle_tick"));
        assert_eq!(gpu_host.clock.last(), gpu_host.step_at);

        // HandingOver acquires GPU texture only during prepare_render when `built` is true.
        let mut handover = MockWebHost::new(start, WebDrawKind::HandingOver { waiting_for_gpu: false });
        assert_eq!(host_step::run(&mut handover), Outcome::Presented);
        assert_eq!(handover.handover_gpu_acquired, 0);
        assert!(handover.log.contains(&"prepare:handover_cpu"));

        handover.log.clear();
        handover.handover_built = true;
        assert_eq!(host_step::run(&mut handover), Outcome::Presented);
        assert_eq!(handover.handover_gpu_acquired, 1);
        assert_eq!(handover.log, ["gate:draw", "acquire:cpu", "step", "sync", "prepare:handover_gpu", "render", "present", "settle"]);

        // Waiting on GPU handover or ContextLost skips at gate without stepping.
        handover.log.clear();
        handover.draw = WebDrawKind::HandingOver { waiting_for_gpu: true };
        assert_eq!(host_step::run(&mut handover), Outcome::Skipped);
        assert_eq!(handover.log, ["gate:skip"]);

        handover.log.clear();
        handover.draw = WebDrawKind::ContextLost;
        assert_eq!(host_step::run(&mut handover), Outcome::Skipped);
        assert_eq!(handover.log, ["gate:skip"]);

        // Native video surface aborts in prepare_render without presenting or settling.
        let mut cpu = MockWebHost::new(start, WebDrawKind::Cpu);
        cpu.simulate_native_surface = true;
        assert_eq!(host_step::run(&mut cpu), Outcome::Aborted);
        assert_eq!(cpu.fatal_error, Some("native video surfaces are unsupported on web"));
        assert_eq!(cpu.log, ["gate:draw", "acquire:cpu", "step", "sync", "prepare:abort"]);
    }

    #[test]
    fn live_poll_updates_draw_one_frame_each_and_unchanged_frames_skip_present() {
        let start = crate::time::now();
        let mut host = MockWebHost::new(start, WebDrawKind::Gpu);
        assert_eq!(host_step::run(&mut host), Outcome::Presented);
        assert_eq!(host.frames_drawn, 1);
        assert_eq!(host.redraw_requested, 0);
        let commits = host.rt.stats.commits;

        // Five timed sprite steps in poll must each draw exactly one GPU frame (not two)
        // and leave live_pending false so Wake::Live does not request a second frame.
        for step in 1..=5u32 {
            let at = start + Duration::from_millis(u64::from(step) * 40);
            host.step_at = at;
            host.submit_at = at;
            host.rt.state.next_live_x = Some(step as f32 * 8.0);
            assert_eq!(host_step::run(&mut host), Outcome::Presented);
            assert!(!host.rt.live_pending(), "step {step} must not leave live_pending set");
            assert_eq!(host.redraw_requested, 0, "step {step} must not ask for an extra frame");
        }
        assert_eq!(host.frames_drawn, 6, "1 initial + 5 live updates = 6 drawn frames, not 11");
        assert_eq!(host.rt.stats.commits, commits, "live updates must not trigger commits");

        // Redraws that change nothing skip acquiring and presenting a canvas texture.
        host.log.clear();
        for extra in 1..=3u32 {
            let at = start + Duration::from_millis(240 + u64::from(extra) * 40);
            host.step_at = at;
            host.submit_at = at;
            assert_eq!(host_step::run(&mut host), Outcome::Unchanged);
        }
        assert_eq!(host.frames_drawn, 6, "unchanged frames must not present");
        assert_eq!(host.log, ["gate:quiet", "step", "unchanged", "gate:quiet", "step", "unchanged", "gate:quiet", "step", "unchanged"]);

        // A compositor that draws its own content still draws even when UI instances match.
        host.compositor_draws_own = true;
        host.step_at = start + Duration::from_millis(400);
        host.submit_at = host.step_at;
        assert_eq!(host_step::run(&mut host), Outcome::Presented);
        assert_eq!(host.frames_drawn, 7);

        // Clearing last_presented (as a surface reconfigure on resize does) forces a draw.
        host.compositor_draws_own = false;
        host.last_presented = None;
        host.step_at = start + Duration::from_millis(440);
        host.submit_at = host.step_at;
        assert_eq!(host_step::run(&mut host), Outcome::Presented);
        assert_eq!(host.frames_drawn, 8);
    }
}

#[test]
fn a_resize_draws_in_one_microtask_before_the_browser_paints() {
    let steps = |state: &mut ResizeRedraw, events: &[ResizeRedrawEvent]| -> Vec<ResizeRedrawEffect> {
        events.iter().flat_map(|&e| step_resize_redraw(state, e)).collect()
    };
    let mut state = ResizeRedraw::Idle;
    // A scale change and the resize it brings queue one microtask, which draws once.
    use ResizeRedrawEvent::*;
    assert_eq!(steps(&mut state, &[Resized, Resized]), [ResizeRedrawEffect::QueueMicrotask]);
    assert_eq!(steps(&mut state, &[Fired, Fired]), [ResizeRedrawEffect::Draw]);
    assert_eq!(state, ResizeRedraw::Idle);
    // A frame drawn before the microtask runs leaves it nothing to draw.
    assert_eq!(steps(&mut state, &[Resized, Drawing, Fired]), [ResizeRedrawEffect::QueueMicrotask]);
    // The next resize queues again.
    assert_eq!(steps(&mut state, &[Resized]), [ResizeRedrawEffect::QueueMicrotask]);
}

#[test]
fn surface_health_backs_off_ignores_stale_attempts_and_gives_up_until_resize() {
    use std::time::Duration;
    let mut seq = crate::machine::Seq::default();
    let mut health = SurfaceHealth::default();
    assert!(health.can_draw());

    // A stale attempt answer after a newer configure began is ignored.
    let (stale, current) = (seq.next(), seq.next());
    health.step(&mut seq, SurfaceHealthEvent::Begin(stale));
    health.step(&mut seq, SurfaceHealthEvent::Begin(current));
    let ignored: Vec<_> = health.step(&mut seq, SurfaceHealthEvent::Outcome { attempt: stale, error: Some("stale failure".into()) }).into_iter().collect();
    assert!(ignored.is_empty() && health.can_draw());

    // First failure reports once and schedules 50ms backoff; subsequent failures double up to 5.
    let expected_delays = [50u64, 100, 200, 400];
    let mut attempt = current;
    for (i, ms) in expected_delays.into_iter().enumerate() {
        let effects: Vec<_> = health.step(&mut seq, SurfaceHealthEvent::Outcome {
            attempt,
            error: Some(format!("fail {i}")),
        }).into_iter().collect();
        assert!(!health.can_draw(), "waiting for retry should pause drawing");
        let retry_attempt = match (i, effects.as_slice()) {
            (0, [SurfaceHealthEffect::ReportFirst(_), SurfaceHealthEffect::ScheduleRetry { attempt, delay }]) => {
                assert_eq!(*delay, Duration::from_millis(ms));
                *attempt
            }
            (1.., [SurfaceHealthEffect::ScheduleRetry { attempt, delay }]) => {
                assert_eq!(*delay, Duration::from_millis(ms));
                *attempt
            }
            other => panic!("unexpected effects on failure {i}: {other:?}"),
        };
        let due: Vec<_> = health.step(&mut seq, SurfaceHealthEvent::RetryDue(retry_attempt)).into_iter().collect();
        assert_eq!(due, [SurfaceHealthEffect::Configure]);
        attempt = seq.next();
        health.step(&mut seq, SurfaceHealthEvent::Begin(attempt));
    }

    // Fifth consecutive failure gives up and waits for a resize.
    let gave_up: Vec<_> = health.step(&mut seq, SurfaceHealthEvent::Outcome {
        attempt,
        error: Some("final fail".into()),
    }).into_iter().collect();
    assert_eq!(gave_up, [SurfaceHealthEffect::ReportGaveUp("final fail".into())]);
    assert!(!health.can_draw());

    // Resize wakes the surface back up; a presented frame resets failure count.
    health.step(&mut seq, SurfaceHealthEvent::Resized);
    assert!(health.can_draw());
    let next = seq.next();
    health.step(&mut seq, SurfaceHealthEvent::Begin(next));
    health.step(&mut seq, SurfaceHealthEvent::Outcome { attempt: next, error: None });
    health.step(&mut seq, SurfaceHealthEvent::Presented);
    assert_eq!(health, SurfaceHealth::Ready { failures: 0 });
}

#[test]
fn surface_health_drain_retry_and_async_present_ordering() {
    let (mut seq, mut health) = (crate::machine::Seq::default(), SurfaceHealth::default());
    let a1 = seq.next();
    health.step(&mut seq, SurfaceHealthEvent::Begin(a1));
    let e1: Vec<_> = health.step(&mut seq, SurfaceHealthEvent::Outcome { attempt: a1, error: Some("oom".into()) }).into_iter().collect();
    let [SurfaceHealthEffect::ReportFirst(_), SurfaceHealthEffect::ScheduleRetry { attempt: r1, .. }] = e1.as_slice() else { panic!("{e1:?}") };
    let mut due = Some(*r1);
    assert!(!health.can_draw() && !health.drain_retry(&mut seq, &mut due, true) && due == Some(*r1));
    assert!(health.drain_retry(&mut seq, &mut due, false) && due.is_none() && health.can_draw());
    // WebGPU presents before async popErrorScope resolves: refusal still escalates to 100 ms.
    let a2 = seq.next();
    health.step(&mut seq, SurfaceHealthEvent::Begin(a2));
    health.step(&mut seq, SurfaceHealthEvent::Presented);
    let e2: Vec<_> = health.step(&mut seq, SurfaceHealthEvent::Outcome { attempt: a2, error: Some("oom2".into()) }).into_iter().collect();
    let [SurfaceHealthEffect::ScheduleRetry { attempt: r2, delay }] = e2.as_slice() else { panic!("{e2:?}") };
    assert_eq!(*delay, std::time::Duration::from_millis(100));
    due = Some(*r2);
    assert!(health.drain_retry(&mut seq, &mut due, false));
    // When async popErrorScope succeeds after Presented, failures reset to 0.
    let a3 = seq.next();
    for ev in [SurfaceHealthEvent::Begin(a3), SurfaceHealthEvent::Presented, SurfaceHealthEvent::Outcome { attempt: a3, error: None }] {
        health.step(&mut seq, ev);
    }
    assert_eq!(health, SurfaceHealth::Ready { failures: 0 });
}
