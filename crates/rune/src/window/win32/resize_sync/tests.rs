use std::time::Duration;

use super::gpu::{DragDevice, DragSeam, SharedDragTarget};
use super::machine::{
    Effect, Eligibility, Event, LEAVING_TIMEOUT, State, step, valid_rects,
};
use super::{ResizeSync, render_runtime_to_shared_target};
use crate::machine::{Attempt, Effects, Seq};
use crate::render::{Gpu, Renderer};
use crate::{AppSpec, Color, Env, col, el, text};

fn eligible() -> Eligibility {
    Eligibility {
        opt_in: true,
        opaque: true,
        overlay: false,
        dxgi_from_visual: true,
        handed_over: true,
        d3d12_live: true,
    }
}

fn collect(effects: Effects<Effect>) -> Vec<Effect> {
    effects.into_iter().collect()
}

#[test]
fn opt_in_off_and_exclusions_never_leave_idle() {
    let now = crate::time::now();
    let excluded = [
        Eligibility { opt_in: false, ..eligible() },
        Eligibility { opaque: false, ..eligible() },
        Eligibility { overlay: true, ..eligible() },
        Eligibility { dxgi_from_visual: false, ..eligible() },
        Eligibility { handed_over: false, ..eligible() },
        Eligibility { d3d12_live: false, ..eligible() },
    ];
    for eligibility in excluded {
        assert!(!eligibility.allowed());
        let mut seq = Seq::default();
        let mut state = State::Idle;
        assert!(collect(step(&mut seq, &mut state, Event::EnterSizeMove { eligibility })).is_empty());
        assert_eq!(state, State::Idle);
        assert!(collect(step(&mut seq, &mut state, Event::Resize { size: (800, 600), eligibility })).is_empty());
        assert_eq!(state, State::Idle);
        assert!(collect(step(&mut seq, &mut state, Event::Redraw)).is_empty());
        assert!(collect(step(&mut seq, &mut state, Event::ExitSizeMove { now })).is_empty());
        assert_eq!(state, State::Idle);
    }
}

#[test]
fn resize_outside_size_move_loop_stays_idle() {
    let mut seq = Seq::default();
    let mut state = State::Idle;
    let effects = collect(step(&mut seq, &mut state, Event::Resize {
        size: (1024, 768),
        eligibility: eligible(),
    }));
    assert!(effects.is_empty());
    assert_eq!(state, State::Idle);
}

#[test]
fn finished_drag_lifecycle_and_pure_move_exit() {
    let t0 = crate::time::now();
    let mut seq = Seq::default();
    let mut state = State::Idle;

    // Pure move (titlebar drag): enters `Entering`, exits to `Idle` with zero GPU work.
    assert!(collect(step(&mut seq, &mut state, Event::EnterSizeMove { eligibility: eligible() })).is_empty());
    assert_eq!(state, State::Entering);
    assert!(collect(step(&mut seq, &mut state, Event::ExitSizeMove { now: t0 })).is_empty());
    assert_eq!(state, State::Idle);

    // Full resize drag: Idle -> Entering -> Dragging -> Leaving -> Idle.
    assert!(collect(step(&mut seq, &mut state, Event::EnterSizeMove { eligibility: eligible() })).is_empty());
    assert_eq!(state, State::Entering);

    let effects = collect(step(&mut seq, &mut state, Event::Resize {
        size: (800, 600),
        eligibility: eligible(),
    }));
    assert_eq!(effects, vec![Effect::BeginDrag { size: (800, 600) }]);
    assert_eq!(state, State::Entering);

    assert!(collect(step(&mut seq, &mut state, Event::DragStarted { size: (800, 600) })).is_empty());
    assert_eq!(state, State::Dragging { size: (800, 600) });

    let effects = collect(step(&mut seq, &mut state, Event::Resize {
        size: (840, 610),
        eligibility: eligible(),
    }));
    assert_eq!(effects, vec![Effect::RenderDragFrame { size: (840, 610) }]);
    assert_eq!(state, State::Dragging { size: (840, 610) });

    let effects = collect(step(&mut seq, &mut state, Event::Redraw));
    assert_eq!(effects, vec![Effect::RenderDragFrame { size: (840, 610) }]);

    let effects = collect(step(&mut seq, &mut state, Event::ExitSizeMove { now: t0 }));
    let deadline = t0 + LEAVING_TIMEOUT;
    assert_eq!(effects, vec![Effect::RequestExitFrame, Effect::WakeAt(deadline)]);
    assert!(state.is_leaving());
    assert_eq!(state.deadline(), Some(deadline));

    let effects = collect(step(&mut seq, &mut state, Event::ExitFrameCompleted));
    assert_eq!(effects, vec![Effect::AttachDcomp, Effect::ReleaseDragResources]);
    assert_eq!(state, State::Idle);
    assert_eq!(state.deadline(), None);
}

#[test]
fn leaving_timer_ignores_early_and_stale_attempts_and_recovers_on_due() {
    let t0 = crate::time::now();
    let mut seq = Seq::default();
    let mut state = State::Idle;
    let _ = step(&mut seq, &mut state, Event::EnterSizeMove { eligibility: eligible() });
    let _ = step(&mut seq, &mut state, Event::Resize { size: (640, 480), eligibility: eligible() });
    let _ = step(&mut seq, &mut state, Event::DragStarted { size: (640, 480) });
    let _ = step(&mut seq, &mut state, Event::ExitSizeMove { now: t0 });
    let attempt = state.leaving_attempt().expect("leaving attempt");
    let deadline = state.deadline().expect("leaving deadline");

    // Early wake changes nothing.
    assert!(collect(step(&mut seq, &mut state, Event::TimerDue {
        attempt,
        now: t0 + Duration::from_millis(100),
    })).is_empty());
    assert!(state.is_leaving());

    // Stale attempt changes nothing even past the deadline.
    assert!(collect(step(&mut seq, &mut state, Event::TimerDue {
        attempt: Attempt(attempt.0.wrapping_add(1)),
        now: deadline + Duration::from_millis(10),
    })).is_empty());
    assert!(state.is_leaving());

    // Matching attempt at deadline re-attaches DComp and frees drag resources.
    let effects = collect(step(&mut seq, &mut state, Event::TimerDue { attempt, now: deadline }));
    assert_eq!(effects, vec![Effect::AttachDcomp, Effect::ReleaseDragResources]);
    assert_eq!(state, State::Idle);
}

#[test]
fn all_abort_exits_leave_clean_idle_state() {
    let t0 = crate::time::now();

    let enter_dragging = |seq: &mut Seq, state: &mut State| {
        *state = State::Idle;
        let _ = step(seq, state, Event::EnterSizeMove { eligibility: eligible() });
        let _ = step(seq, state, Event::Resize { size: (700, 500), eligibility: eligible() });
        let _ = step(seq, state, Event::DragStarted { size: (700, 500) });
        assert!(state.is_dragging());
    };

    // 1. BeginDrag failure falls back cleanly.
    let mut seq = Seq::default();
    let mut state = State::Idle;
    let _ = step(&mut seq, &mut state, Event::EnterSizeMove { eligibility: eligible() });
    let _ = step(&mut seq, &mut state, Event::Resize { size: (700, 500), eligibility: eligible() });
    assert_eq!(
        collect(step(&mut seq, &mut state, Event::DragFailed)),
        vec![Effect::AttachDcomp, Effect::ReleaseDragResources]
    );
    assert_eq!(state, State::Idle);

    // 2. Device lost mid-drag drops D3D11 device as well as drag resources and restores DComp.
    enter_dragging(&mut seq, &mut state);
    assert_eq!(
        collect(step(&mut seq, &mut state, Event::DeviceLost)),
        vec![Effect::AttachDcomp, Effect::ReleaseDragResources, Effect::DropD3d11Device]
    );
    assert_eq!(state, State::Idle);

    // Device lost while Entering drops any cached D3D11 device.
    let _ = step(&mut seq, &mut state, Event::EnterSizeMove { eligibility: eligible() });
    assert_eq!(collect(step(&mut seq, &mut state, Event::DeviceLost)), vec![Effect::DropD3d11Device]);
    assert_eq!(state, State::Idle);

    // Device lost while Leaving restores DComp, frees drag resources, and drops D3D11 device.
    enter_dragging(&mut seq, &mut state);
    let _ = step(&mut seq, &mut state, Event::ExitSizeMove { now: t0 });
    assert_eq!(
        collect(step(&mut seq, &mut state, Event::DeviceLost)),
        vec![Effect::AttachDcomp, Effect::ReleaseDragResources, Effect::DropD3d11Device]
    );
    assert_eq!(state, State::Idle);

    // 3. CPU-to-GPU handover mid-drag exits cleanly from Entering, Dragging, and Leaving.
    let _ = step(&mut seq, &mut state, Event::EnterSizeMove { eligibility: eligible() });
    assert!(collect(step(&mut seq, &mut state, Event::Handover)).is_empty());
    assert_eq!(state, State::Idle);

    enter_dragging(&mut seq, &mut state);
    assert_eq!(
        collect(step(&mut seq, &mut state, Event::Handover)),
        vec![Effect::AttachDcomp, Effect::ReleaseDragResources]
    );
    assert_eq!(state, State::Idle);

    enter_dragging(&mut seq, &mut state);
    let _ = step(&mut seq, &mut state, Event::ExitSizeMove { now: t0 });
    assert_eq!(
        collect(step(&mut seq, &mut state, Event::Handover)),
        vec![Effect::AttachDcomp, Effect::ReleaseDragResources]
    );
    assert_eq!(state, State::Idle);

    // 4. Window closes / destroys mid-drag releases resources without touching DComp.
    enter_dragging(&mut seq, &mut state);
    assert_eq!(collect(step(&mut seq, &mut state, Event::Destroy)), vec![Effect::ReleaseDragResources]);
    assert_eq!(state, State::Idle);

    enter_dragging(&mut seq, &mut state);
    let _ = step(&mut seq, &mut state, Event::ExitSizeMove { now: t0 });
    assert_eq!(collect(step(&mut seq, &mut state, Event::Destroy)), vec![Effect::ReleaseDragResources]);
    assert_eq!(state, State::Idle);

    // 5. Window hides or minimizes (zero size) mid-drag re-attaches DComp and releases resources.
    enter_dragging(&mut seq, &mut state);
    assert_eq!(
        collect(step(&mut seq, &mut state, Event::Hide)),
        vec![Effect::AttachDcomp, Effect::ReleaseDragResources]
    );
    assert_eq!(state, State::Idle);

    enter_dragging(&mut seq, &mut state);
    assert_eq!(
        collect(step(&mut seq, &mut state, Event::Resize { size: (0, 0), eligibility: eligible() })),
        vec![Effect::AttachDcomp, Effect::ReleaseDragResources]
    );
    assert_eq!(state, State::Idle);

    // 6. Immediate re-entry while Leaving cleans up the prior drag and enters Entering.
    enter_dragging(&mut seq, &mut state);
    let _ = step(&mut seq, &mut state, Event::ExitSizeMove { now: t0 });
    let old_attempt = state.leaving_attempt().expect("old attempt");
    let old_deadline = state.deadline().expect("old deadline");
    assert_eq!(
        collect(step(&mut seq, &mut state, Event::EnterSizeMove { eligibility: eligible() })),
        vec![Effect::AttachDcomp, Effect::ReleaseDragResources]
    );
    assert_eq!(state, State::Entering);
    assert!(collect(step(&mut seq, &mut state, Event::TimerDue {
        attempt: old_attempt,
        now: old_deadline + Duration::from_secs(1),
    })).is_empty());
    assert_eq!(state, State::Entering);
}

#[test]
fn wvr_validrects_pins_stationary_pixels_across_all_edges() {
    let old = [100, 100, 900, 700];

    // Left edge dragged left (grow): old content stays pinned at x=100..900.
    let (dest, src) = valid_rects([80, 100, 900, 700], old);
    assert_eq!(dest, [100, 100, 900, 700]);
    assert_eq!(src, dest);

    // Left edge dragged right (shrink): overlapping pixels stay pinned at x=120..900.
    let (dest, src) = valid_rects([120, 100, 900, 700], old);
    assert_eq!(dest, [120, 100, 900, 700]);
    assert_eq!(src, dest);

    // Top edge dragged up (grow): old content stays pinned at y=100..700.
    let (dest, src) = valid_rects([100, 60, 900, 700], old);
    assert_eq!(dest, [100, 100, 900, 700]);
    assert_eq!(src, dest);

    // Top-left corner dragged up-left (grow both).
    let (dest, src) = valid_rects([70, 50, 900, 700], old);
    assert_eq!(dest, [100, 100, 900, 700]);
    assert_eq!(src, dest);

    // Right/bottom edge dragged out.
    let (dest, src) = valid_rects([100, 100, 960, 740], old);
    assert_eq!(dest, [100, 100, 900, 700]);
    assert_eq!(src, dest);

    // Disjoint move: empty valid rect at new client top-left.
    let (dest, src) = valid_rects([1000, 1000, 1400, 1300], old);
    assert_eq!(dest, [1000, 1000, 1000, 1000]);
    assert_eq!(src, dest);

    // Empty/inverted rect.
    let (dest, src) = valid_rects([100, 100, 100, 700], old);
    assert_eq!(dest, [100, 100, 100, 100]);
    assert_eq!(src, dest);
}

#[derive(Default)]
struct FakeSeam {
    presents: Vec<((u32, u32), u64)>,
    dcomp_detached: bool,
    valid_rects_active: bool,
    swap_chain_live: bool,
    last_pixels: Vec<u8>,
}

impl DragSeam for FakeSeam {
    fn present_shared(
        &mut self,
        drag: &mut DragDevice,
        target: &SharedDragTarget,
        ready_value: u64,
    ) -> Result<(), String> {
        self.swap_chain_live = true;
        self.last_pixels = drag.readback_via_d3d11(target, ready_value)?;
        self.presents.push((target.size, ready_value));
        Ok(())
    }

    fn detach_dcomp(&mut self) -> Result<(), String> {
        self.dcomp_detached = true;
        Ok(())
    }

    fn attach_dcomp(&mut self) -> Result<(), String> {
        self.dcomp_detached = false;
        Ok(())
    }

    fn set_valid_rects_active(&mut self, active: bool) {
        self.valid_rects_active = active;
    }

    fn release_swap_chain(&mut self) {
        self.swap_chain_live = false;
    }
}

fn sample_app(srgb_blend: bool) -> crate::Runtime<()> {
    let mut rt = crate::App::new(AppSpec::new("drag-sync", (240.0, 160.0), || (), |_, _| {
        let t = crate::tok::theme(true);
        col()
            .w_full()
            .h_full()
            .fill(t.bg)
            .padding(14.0)
            .gap(10.0)
            .child(
                el()
                    .w_full()
                    .h(48.0)
                    .radius(10.0)
                    .fill(crate::tok::accent(0, true))
                    .border(2.0, Color::WHITE.alpha(0.55))
                    .padding(10.0)
                    .child(text("Drag sync check").font(crate::tok::type_::TITLE).color(Color::WHITE)),
            )
            .child(
                el()
                    .w_full()
                    .grow(1.0)
                    .radius(8.0)
                    .fill(t.card)
                    .border(1.0, t.card_border)
                    .padding(10.0)
                    .child(text("Left and top edge resize").font(crate::tok::type_::BODY).color(t.text)),
            )
    }))
    .headless_runtime((240.0, 160.0), Env::new(true, 1.0));
    rt.core_mut().set_srgb_blend(srgb_blend);
    rt
}

fn render_normal_offscreen(
    gpu: &Gpu,
    renderer: &mut Renderer,
    rt: &mut crate::runtime::RuntimeCore,
    size: (u32, u32),
    format: wgpu::TextureFormat,
) -> Vec<u8> {
    let (width, height) = size;
    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("normal drag-sync ref"),
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
        label: Some("normal drag-sync readback"),
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
    let mapped = slice.get_mapped_range().expect("mapped staging buffer");
    let row_bytes = (width as usize) * 4;
    let mut out = vec![0u8; row_bytes * (height as usize)];
    for y in 0..height as usize {
        let start = y * (row as usize);
        out[y * row_bytes..(y + 1) * row_bytes].copy_from_slice(&mapped[start..start + row_bytes]);
    }
    out
}

#[test]
fn offscreen_opt_in_off_creates_zero_drag_gpu_objects() {
    let Some(gpu) = Gpu::test_headless_wgpu() else { return };
    #[cfg(not(feature = "test-gpu-counters"))]
    let _ = &gpu;
    #[cfg(feature = "test-gpu-counters")]
    let base_textures = gpu.device.get_internal_counters().hal.textures.read();

    let mut sync = ResizeSync::default();
    let seam = FakeSeam::default();
    let off = Eligibility { opt_in: false, ..eligible() };
    let now = crate::time::now();

    let e1 = collect(step(&mut sync.seq, &mut sync.state, Event::EnterSizeMove { eligibility: off }));
    let e2 = collect(step(&mut sync.seq, &mut sync.state, Event::Resize { size: (320, 200), eligibility: off }));
    let e3 = collect(step(&mut sync.seq, &mut sync.state, Event::Redraw));
    let e4 = collect(step(&mut sync.seq, &mut sync.state, Event::ExitSizeMove { now }));
    assert!(e1.is_empty() && e2.is_empty() && e3.is_empty() && e4.is_empty());
    assert_eq!(sync.state, State::Idle);
    assert!(sync.device.is_none());
    assert!(sync.target.is_none());
    assert!(!seam.dcomp_detached && !seam.swap_chain_live && seam.presents.is_empty());

    #[cfg(feature = "test-gpu-counters")]
    assert_eq!(
        gpu.device.get_internal_counters().hal.textures.read(),
        base_textures,
        "opt-in off must allocate zero GPU textures"
    );
}

#[test]
fn offscreen_shared_texture_fence_bit_exact_and_counted_on_real_adapter() {
    let Some(gpu) = Gpu::test_headless_wgpu() else { return };
    for (srgb_blend, format) in [
        (true, wgpu::TextureFormat::Bgra8Unorm),
        (false, wgpu::TextureFormat::Bgra8UnormSrgb),
    ] {
        let mut rt = sample_app(srgb_blend);
        let mut renderer = Renderer::new(&gpu.device, format);
        renderer.attach_gpu(&gpu);
        renderer.set_srgb_blend(srgb_blend);

        // Warm renderer atlases/pipelines once on normal path so baseline texture count is steady.
        rt.core_mut().set_viewport((240.0, 160.0), 1.0);
        let _ = rt.frame(0.0);
        let normal_1 = render_normal_offscreen(&gpu, &mut renderer, rt.core_mut(), (240, 160), format);
        // Ensure the temporary normal texture is retired before measuring baseline.
        rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely()).expect("poll");

        #[cfg(feature = "test-gpu-counters")]
        let base_textures = gpu.device.get_internal_counters().hal.textures.read();

        let mut sync = ResizeSync::default();
        let mut seam = FakeSeam::default();

        // Enter size-move -> first drag resize (240x160).
        assert!(collect(step(&mut sync.seq, &mut sync.state, Event::EnterSizeMove { eligibility: eligible() })).is_empty());
        let effects = collect(step(&mut sync.seq, &mut sync.state, Event::Resize {
            size: (240, 160),
            eligibility: eligible(),
        }));
        assert_eq!(effects, vec![Effect::BeginDrag { size: (240, 160) }]);

        {
            let (drag, target) = sync.ensure_gpu_target(&gpu.device, (240, 160), format).expect("shared target");
            let ready = render_runtime_to_shared_target(&gpu, &mut renderer, rt.core_mut(), drag, target)
                .expect("render into shared");
            seam.present_shared(drag, target, ready).expect("present shared");
            seam.detach_dcomp().expect("detach dcomp");
            seam.set_valid_rects_active(true);
        }
        let _ = step(&mut sync.seq, &mut sync.state, Event::DragStarted { size: (240, 160) });
        assert_eq!(sync.state, State::Dragging { size: (240, 160) });
        assert!(seam.dcomp_detached && seam.valid_rects_active && seam.swap_chain_live);
        assert_eq!(
            seam.last_pixels, normal_1,
            "D3D12->D3D11 shared drag frame must match normal offscreen render bit-for-bit ({format:?})"
        );
        #[cfg(feature = "test-gpu-counters")]
        assert_eq!(
            gpu.device.get_internal_counters().hal.textures.read(),
            base_textures + 1,
            "exactly one shared wgpu texture should be live during drag"
        );

        // Second resize step (320x200): old shared texture replaced, new pixels still bit-exact.
        rt.core_mut().set_viewport((320.0, 200.0), 1.0);
        let _ = rt.frame(0.0);
        let normal_2 = render_normal_offscreen(&gpu, &mut renderer, rt.core_mut(), (320, 200), format);
        let effects = collect(step(&mut sync.seq, &mut sync.state, Event::Resize {
            size: (320, 200),
            eligibility: eligible(),
        }));
        assert_eq!(effects, vec![Effect::RenderDragFrame { size: (320, 200) }]);
        {
            let (drag, target) = sync.ensure_gpu_target(&gpu.device, (320, 200), format).expect("resized target");
            let ready = render_runtime_to_shared_target(&gpu, &mut renderer, rt.core_mut(), drag, target)
                .expect("render resized");
            seam.present_shared(drag, target, ready).expect("present resized");
        }
        assert_eq!(
            seam.last_pixels, normal_2,
            "resized D3D12->D3D11 shared drag frame must match normal offscreen render bit-for-bit ({format:?})"
        );

        // Wait for D3D12 queue idle so replaced texture is reclaimed by wgpu.
        rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely()).expect("poll");
        #[cfg(feature = "test-gpu-counters")]
        assert_eq!(
            gpu.device.get_internal_counters().hal.textures.read(),
            base_textures + 1,
            "resizing must replace the previous shared texture without leaking"
        );

        // Exit drag -> Leaving -> ExitFrameCompleted -> Idle: all drag textures released.
        let now = crate::time::now();
        let effects = collect(step(&mut sync.seq, &mut sync.state, Event::ExitSizeMove { now }));
        assert_eq!(effects.len(), 2);
        assert!(sync.is_leaving());
        let effects = collect(step(&mut sync.seq, &mut sync.state, Event::ExitFrameCompleted));
        assert_eq!(effects, vec![Effect::AttachDcomp, Effect::ReleaseDragResources]);
        seam.attach_dcomp().expect("attach dcomp");
        seam.set_valid_rects_active(false);
        seam.release_swap_chain();
        sync.release_drag_resources();
        assert_eq!(sync.state, State::Idle);
        assert!(!seam.dcomp_detached && !seam.valid_rects_active && !seam.swap_chain_live);
        assert!(sync.target.is_none());
        assert!(sync.device.is_some(), "lazy D3D11 device stays ready for subsequent drags");

        rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely()).expect("poll");
        #[cfg(feature = "test-gpu-counters")]
        assert_eq!(
            gpu.device.get_internal_counters().hal.textures.read(),
            base_textures,
            "exiting drag must release every shared wgpu texture"
        );

        // Device loss also drops the D3D11 device.
        sync.drop_d3d11_device();
        assert!(sync.device.is_none());
    }
}
