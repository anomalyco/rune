//! Regression tests for `BeginDrag` GPU-present synchronization before `detach_dcomp`,
//! pre-paint `WM_ERASEBKGND` suppression, stray damage disarming, and capacited drag-target
//! pixel parity across overlay/layer/cache/backdrop-blur mount and unmount transitions (#2113).

use std::sync::Arc;

use super::DragResizeSync;
use super::frame::{DragFrameConsumers, run_drag_frame};
use super::gpu::{DragDevice, DragSeam, SharedDragTarget};
use crate::frame::clock::FrameClock;
use crate::frame::encoded::FrameReports;
use crate::frame::surface_stats::SurfaceMeter;
use crate::render::{Gpu, Renderer};
use crate::window::host::app::TypedWindowApp;
use crate::window::state::Rest;
use crate::window::surface::damage::Damage;
use crate::{Color, El, Env, Runtime, col, el, text};

#[derive(Debug, PartialEq, Eq)]
enum SeamCall {
    SetValidRects(bool),
    PresentShared { wait_for_present: bool },
    DetachDcomp,
    AttachDcomp,
    ReleaseSwapChain,
}

#[derive(Default)]
struct RecordingSeam {
    calls: Vec<SeamCall>,
    last_pixels: Vec<u8>,
    fail_present: bool,
}

impl DragSeam for RecordingSeam {
    fn present_shared(
        &mut self,
        drag: &mut DragDevice,
        target: &SharedDragTarget,
        copy_size: (u32, u32),
        ready_value: u64,
        wait_for_present: bool,
    ) -> Result<u64, String> {
        self.calls.push(SeamCall::PresentShared { wait_for_present });
        if self.fail_present {
            return Err("injected present failure".into());
        }
        let (pixels, release_value) = drag.readback_via_d3d11(target, copy_size, ready_value)?;
        self.last_pixels = pixels;
        if wait_for_present {
            // Exercise the exact post-present fence signal + CPU event wait path used by
            // `Win32DragSeam` on `BeginDrag` before detaching DirectComposition.
            let done = drag.signal_post_present()?;
            drag.wait_for_release(done)?;
            return Ok(done);
        }
        Ok(release_value)
    }

    fn detach_dcomp(&mut self) -> Result<(), String> {
        self.calls.push(SeamCall::DetachDcomp);
        Ok(())
    }

    fn attach_dcomp(&mut self) -> Result<(), String> {
        self.calls.push(SeamCall::AttachDcomp);
        Ok(())
    }

    fn set_valid_rects_active(&mut self, active: bool) {
        self.calls.push(SeamCall::SetValidRects(active));
    }

    fn release_swap_chain(&mut self) {
        self.calls.push(SeamCall::ReleaseSwapChain);
    }
}

#[derive(Clone, Copy, Default)]
struct OverlayScene {
    dialog_open: bool,
    blur_popover_open: bool,
}

const fn rgb(r: f32, g: f32, b: f32) -> Color {
    Color { r, g, b, a: 1.0 }
}

fn overlay_view(s: &OverlayScene, _: &mut crate::Cx<OverlayScene>) -> El {
    let mut root = col()
        .w_full()
        .h_full()
        .fill(rgb(0.11, 0.12, 0.15))
        .padding(12.0)
        .gap(8.0)
        .child(
            el().key("cached-card")
                .w_full()
                .h(44.0)
                .radius(8.0)
                .fill(rgb(0.20, 0.45, 0.85))
                .padding(8.0)
                .cache()
                .child(text("Cached header").color(Color::WHITE)),
        )
        .child(
            el().key("body-card")
                .w_full()
                .grow(1.0)
                .radius(6.0)
                .fill(rgb(0.18, 0.19, 0.23))
                .border(1.0, Color::WHITE.alpha(0.2))
                .padding(8.0)
                .child(text("Gallery body").color(rgb(0.92, 0.93, 0.96))),
        );

    if s.dialog_open {
        root = root
            .child(
                el().key("dialog-scrim")
                    .absolute(0.0, 0.0)
                    .w_full()
                    .h_full()
                    .fill(Color::BLACK.alpha(0.55)),
            )
            .child(
                col()
                    .key("dialog-layer")
                    .absolute(0.0, 0.0)
                    .w_full()
                    .h_full()
                    .center()
                    .child(
                        col()
                            .key("dialog-panel")
                            .size(160.0, 88.0)
                            .radius(8.0)
                            .fill(rgb(0.22, 0.24, 0.29))
                            .border(1.0, Color::WHITE.alpha(0.35))
                            .padding(10.0)
                            .scale(0.96)
                            .layer_opacity(0.88)
                            .child(text("Dialog").color(Color::WHITE)),
                    ),
            );
    }

    if s.blur_popover_open {
        root = root
            .child(
                col()
                    .key("blur-popover")
                    .absolute(24.0, 24.0)
                    .size(140.0, 72.0)
                    .radius(8.0)
                    .backdrop_blur(10.0)
                    .fill(rgb(0.16, 0.18, 0.22).alpha(0.6))
                    .border(1.0, Color::WHITE.alpha(0.3))
                    .padding(8.0)
                    .child(text("Frosted").color(Color::WHITE)),
            )
            .child(
                // Post-blur element covering the bottom-right corner inside the viewport, proving
                // the pass re-opened by `ActivePass::split` keeps viewport + scissor aligned.
                el().key("post-blur-badge")
                    .absolute(172.0, 108.0)
                    .size(48.0, 24.0)
                    .radius(4.0)
                    .fill(rgb(0.95, 0.35, 0.25)),
            );
    }

    root
}

fn render_exact_reference(
    gpu: &Gpu,
    renderer: &mut Renderer,
    rt: &mut crate::runtime::RuntimeCore,
    size: (u32, u32),
    format: wgpu::TextureFormat,
) -> Vec<u8> {
    let (width, height) = size;
    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("exact resize-sync reference"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    crate::frame::host_step::prepare_gpu_resources_with(renderer, gpu, rt, size, true, |r, _| {
        r.set_punched_surfaces(Vec::new());
    });
    renderer.next_instances(rt.paint_count(), None);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    let clear = rt.clear_color();
    crate::frame::host_step::draw_ui(renderer, gpu, &mut encoder, &view, rt, size, clear);
    let row = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let staging = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("exact resize-sync readback"),
        size: u64::from(row) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    renderer.submit(&gpu.queue, encoder);
    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely()).expect("gpu poll");
    let mapped = slice.get_mapped_range().expect("mapped staging");
    let row_bytes = (width as usize) * 4;
    let mut out = vec![0u8; row_bytes * (height as usize)];
    for y in 0..height as usize {
        let start = y * (row as usize);
        out[y * row_bytes..(y + 1) * row_bytes].copy_from_slice(&mapped[start..start + row_bytes]);
    }
    out
}

fn run_one_drag_step(
    sync: &mut DragResizeSync,
    gpu: &Gpu,
    renderer: &mut Renderer,
    app: &mut TypedWindowApp<OverlayScene>,
    seam: &mut RecordingSeam,
    size: (u32, u32),
    format: wgpu::TextureFormat,
    detach_dcomp: bool,
) -> Result<(), String> {
    let now = crate::time::now();
    let mut clock = FrameClock::new(now);
    let mut meter = SurfaceMeter::new("main", Arc::default());
    let mut frame_reports = FrameReports::new(None, Some("main"));
    let mut rest = Rest::default();
    let mut damage = Damage::default();
    let mut trace = None;
    let mut trace_frames = 0;
    let mut idle = true;
    app.rt.core_mut().set_viewport((size.0 as f32, size.1 as f32), 1.0);
    let mut consumers = DragFrameConsumers {
        app,
        clock: &mut clock,
        meter: &mut meter,
        frame_reports: &mut frame_reports,
        observability: None,
        rest: &mut rest,
        damage: &mut damage,
        trace: &mut trace,
        trace_start: Some(now),
        trace_frames: &mut trace_frames,
        trace_limit: None,
        frame_moving: false,
        idle: &mut idle,
        refresh_hz: Some(239.0),
        paced_for_stats: true,
        next_tick: None,
        base_format: format,
    };
    run_drag_frame(sync, gpu, renderer, &mut consumers, seam, size, detach_dcomp, || {}).map(|_| ())
}

#[test]
fn begin_drag_suppresses_erase_waits_for_present_then_detaches_dcomp() {
    let Some(gpu) = Gpu::test_headless_wgpu() else { return };
    let format = wgpu::TextureFormat::Bgra8Unorm;
    let mut renderer = Renderer::new(&gpu.device, format);
    renderer.attach_gpu(&gpu);
    renderer.set_srgb_blend(true);

    let mut rt = Runtime::new(OverlayScene::default(), overlay_view, (240.0, 160.0), Env::new(true, 1.0));
    rt.set_srgb_blend(true);
    rt.frame(0.0);
    let mut app = TypedWindowApp::for_test(rt, None);

    let mut sync = DragResizeSync::default();
    let mut seam = RecordingSeam::default();

    // Frame 0 (`BeginDrag`): suppress erase before `BeginPaint`, wait on the D3D11 post-present
    // fence, and detach DirectComposition strictly after the present completes.
    run_one_drag_step(&mut sync, &gpu, &mut renderer, &mut app, &mut seam, (240, 160), format, true)
        .expect("begin drag succeeds");
    assert_eq!(
        seam.calls,
        vec![
            SeamCall::SetValidRects(true),
            SeamCall::PresentShared { wait_for_present: true },
            SeamCall::DetachDcomp,
        ],
    );

    // Steady-state drag frames: never block on `wait_for_present` and never re-detach DComp.
    seam.calls.clear();
    run_one_drag_step(&mut sync, &gpu, &mut renderer, &mut app, &mut seam, (272, 168), format, false)
        .expect("steady drag step succeeds");
    run_one_drag_step(&mut sync, &gpu, &mut renderer, &mut app, &mut seam, (300, 180), format, false)
        .expect("steady drag step 2 succeeds");
    assert_eq!(
        seam.calls,
        vec![
            SeamCall::PresentShared { wait_for_present: false },
            SeamCall::PresentShared { wait_for_present: false },
        ],
    );

    // On a `BeginDrag` present failure, `detach_dcomp` must NOT run (caller drives `DragFailed`).
    seam.calls.clear();
    seam.fail_present = true;
    let err = run_one_drag_step(&mut sync, &gpu, &mut renderer, &mut app, &mut seam, (312, 184), format, true)
        .expect_err("injected failure surfaces");
    assert!(err.contains("injected present failure"), "unexpected error: {err}");
    assert_eq!(
        seam.calls,
        vec![
            SeamCall::SetValidRects(true),
            SeamCall::PresentShared { wait_for_present: true },
        ],
    );

    sync.drop_d3d11_device();
}

#[test]
fn capacited_drag_target_matches_normal_across_overlay_layer_blur_cache_and_armed_damage() {
    let Some(gpu) = Gpu::test_headless_wgpu() else { return };
    for (srgb_blend, format) in [
        (true, wgpu::TextureFormat::Bgra8Unorm),
        (false, wgpu::TextureFormat::Bgra8UnormSrgb),
    ] {
        let mut renderer = Renderer::new(&gpu.device, format);
        renderer.attach_gpu(&gpu);
        renderer.set_srgb_blend(srgb_blend);

        let mut rt = Runtime::new(OverlayScene::default(), overlay_view, (240.0, 160.0), Env::new(true, 1.0));
        rt.set_srgb_blend(srgb_blend);
        rt.frame(0.0);
        let mut app = TypedWindowApp::for_test(rt, None);
        let mut sync = DragResizeSync::default();
        let mut seam = RecordingSeam::default();

        let transitions = [
            ("closed + stray armed damage", OverlayScene { dialog_open: false, blur_popover_open: false }, (240, 160), true),
            ("dialog + scrim + isolated layer", OverlayScene { dialog_open: true, blur_popover_open: false }, (264, 176), true),
            ("dialog + backdrop_blur popover (ActivePass::split)", OverlayScene { dialog_open: true, blur_popover_open: true }, (288, 192), false),
            ("dialog closed (parked spare/retained layers)", OverlayScene { dialog_open: false, blur_popover_open: false }, (252, 168), true),
        ];

        for (label, state, size, begin_drag) in transitions {
            app.rt.state = state;
            app.rt.core_mut().set_viewport((size.0 as f32, size.1 as f32), 1.0);
            app.rt.frame(0.0);
            // Simulate a stray armed partial-damage rect whose retained buffer matches the
            // capacited drag target's full dimensions (as when shrinking from a 256-aligned size):
            // `run_drag_frame` must disarm it before `draw_ui` rather than running `draw_armed`.
            if begin_drag {
                let cap = super::gpu::grow_capacity(None, size, gpu.device.limits().max_texture_dimension_2d, false);
                renderer.arm_damage(
                    &gpu.device,
                    &gpu.queue,
                    &[[0, 0, 32, 32]],
                    app.rt.core().clear_color(),
                    cap,
                    format,
                    true,
                );
            }
            run_one_drag_step(&mut sync, &gpu, &mut renderer, &mut app, &mut seam, size, format, begin_drag)
                .unwrap_or_else(|e| panic!("drag step failed for {label} ({format:?}): {e}"));
            let cap = sync.ring.capacity().expect("capacited ring");
            assert!(cap.0 > size.0 && cap.1 > size.1, "expected capacited target for {label}: cap={cap:?} size={size:?}");

            let expected = render_exact_reference(&gpu, &mut renderer, app.rt.core_mut(), size, format);
            let diff_pixels = seam
                .last_pixels
                .chunks_exact(4)
                .zip(expected.chunks_exact(4))
                .filter(|(a, b)| a != b)
                .count();
            assert_eq!(
                diff_pixels, 0,
                "capacited drag target diverged from exact-sized reference on {label} ({format:?}): {diff_pixels} differing pixels"
            );
            if begin_drag {
                // End the drag cleanly between overlay transitions so the next transition exercises
                // a fresh `BeginDrag` ring allocation just like live drags.
                sync.release_drag_resources();
            }
        }

        sync.drop_d3d11_device();
    }
}
