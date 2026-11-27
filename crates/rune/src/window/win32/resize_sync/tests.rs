use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use super::frame::{DragFrameConsumers, run_drag_frame};
use super::gpu::{CAPACITY_STEP, DragDevice, DragSeam, SharedDragTarget, dxgi_format_for, grow_capacity};
use super::machine::{
    Effect, Eligibility, Event, LEAVING_TIMEOUT, State, step, valid_rects,
};
use super::{DragResizeSync, commit_or_rollback_config};

use crate::frame::clock::FrameClock;
use crate::frame::encoded::FrameReports;
use crate::frame::surface_stats::SurfaceMeter;
use crate::machine::{Attempt, Effects, Seq};
use crate::render::{Gpu, Renderer};
use crate::window::host::app::TypedWindowApp;
use crate::window::state::Rest;
use crate::window::surface::damage::Damage;
use crate::{AppSpec, Color, Env, FrameOutcome, Runtime, col, el, text};

fn eligible() -> Eligibility {
    Eligibility {
        opt_in: true,
        opaque: true,
        overlay: false,
        dxgi_from_visual: true,
        handed_over: true,
        d3d12_live: true,
        hardware_adapter: true,
        supported_format: true,
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
        Eligibility { hardware_adapter: false, ..eligible() },
        Eligibility { supported_format: false, ..eligible() },
    ];
    // Verify HDR / non-8-bit formats are rejected by `dxgi_format_for`.
    assert!(dxgi_format_for(wgpu::TextureFormat::Rgba16Float).is_err());
    assert!(dxgi_format_for(wgpu::TextureFormat::Rgb10a2Unorm).is_err());
    assert!(dxgi_format_for(wgpu::TextureFormat::Bgra8Unorm).is_ok());
    assert!(dxgi_format_for(wgpu::TextureFormat::Bgra8UnormSrgb).is_ok());
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

    assert!(collect(step(&mut seq, &mut state, Event::EnterSizeMove { eligibility: eligible() })).is_empty());
    assert_eq!(state, State::Entering);
    assert!(collect(step(&mut seq, &mut state, Event::ExitSizeMove { now: t0 })).is_empty());
    assert_eq!(state, State::Idle);

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

    assert!(collect(step(&mut seq, &mut state, Event::TimerDue {
        attempt,
        now: t0 + Duration::from_millis(100),
    })).is_empty());
    assert!(state.is_leaving());

    assert!(collect(step(&mut seq, &mut state, Event::TimerDue {
        attempt: Attempt(attempt.0.wrapping_add(1)),
        now: deadline + Duration::from_millis(10),
    })).is_empty());
    assert!(state.is_leaving());

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

    let mut seq = Seq::default();
    let mut state = State::Idle;
    let _ = step(&mut seq, &mut state, Event::EnterSizeMove { eligibility: eligible() });
    let _ = step(&mut seq, &mut state, Event::Resize { size: (700, 500), eligibility: eligible() });
    assert_eq!(
        collect(step(&mut seq, &mut state, Event::DragFailed)),
        vec![Effect::AttachDcomp, Effect::ReleaseDragResources]
    );
    assert_eq!(state, State::Idle);

    // Mid-drag failure in `State::Dragging` must also restore DirectComposition and free drag
    // resources instead of falling through to a no-op.
    enter_dragging(&mut seq, &mut state);
    assert_eq!(
        collect(step(&mut seq, &mut state, Event::DragFailed)),
        vec![Effect::AttachDcomp, Effect::ReleaseDragResources]
    );
    assert_eq!(state, State::Idle);

    enter_dragging(&mut seq, &mut state);
    assert_eq!(
        collect(step(&mut seq, &mut state, Event::DeviceLost)),
        vec![Effect::AttachDcomp, Effect::ReleaseDragResources, Effect::DropD3d11Device]
    );
    assert_eq!(state, State::Idle);

    let _ = step(&mut seq, &mut state, Event::EnterSizeMove { eligibility: eligible() });
    assert_eq!(collect(step(&mut seq, &mut state, Event::DeviceLost)), vec![Effect::DropD3d11Device]);
    assert_eq!(state, State::Idle);

    enter_dragging(&mut seq, &mut state);
    let _ = step(&mut seq, &mut state, Event::ExitSizeMove { now: t0 });
    assert_eq!(
        collect(step(&mut seq, &mut state, Event::DeviceLost)),
        vec![Effect::AttachDcomp, Effect::ReleaseDragResources, Effect::DropD3d11Device]
    );
    assert_eq!(state, State::Idle);

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

    enter_dragging(&mut seq, &mut state);
    assert_eq!(collect(step(&mut seq, &mut state, Event::Destroy)), vec![Effect::ReleaseDragResources]);
    assert_eq!(state, State::Idle);

    enter_dragging(&mut seq, &mut state);
    let _ = step(&mut seq, &mut state, Event::ExitSizeMove { now: t0 });
    assert_eq!(collect(step(&mut seq, &mut state, Event::Destroy)), vec![Effect::ReleaseDragResources]);
    assert_eq!(state, State::Idle);

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
    for (new_r, want) in [
        ([80, 100, 900, 700], [100, 100, 900, 700]),
        ([120, 100, 900, 700], [120, 100, 900, 700]),
        ([100, 60, 900, 700], [100, 100, 900, 700]),
        ([70, 50, 900, 700], [100, 100, 900, 700]),
        ([100, 100, 960, 740], [100, 100, 900, 700]),
        ([1000, 1000, 1400, 1300], [1000, 1000, 1000, 1000]),
        ([100, 100, 100, 700], [100, 100, 100, 100]),
    ] {
        let (dest, src) = valid_rects(new_r, old);
        assert_eq!(dest, want);
        assert_eq!(src, dest);
    }
}

#[test]
fn grow_capacity_reuses_within_bounds_and_clamps_to_max_dim() {
    let cap = grow_capacity(None, (800, 600), 16_384, false);
    assert!(cap.0 >= 800 + CAPACITY_STEP && cap.1 >= 600 + CAPACITY_STEP);
    assert_eq!(cap.0 % CAPACITY_STEP, 0);
    assert_eq!(cap.1 % CAPACITY_STEP, 0);
    // Shrinking or growing inside capacity keeps the exact same allocation.
    assert_eq!(grow_capacity(Some(cap), (640, 480), 16_384, false), cap);
    assert_eq!(grow_capacity(Some(cap), (cap.0, cap.1), 16_384, false), cap);
    // Overflowing only width keeps height unchanged and grows width.
    let wider = grow_capacity(Some(cap), (cap.0 + 10, 500), 16_384, false);
    assert!(wider.0 > cap.0);
    assert_eq!(wider.1, cap.1);
    // Clamped to max_dim and exact mode returns exact size.
    assert_eq!(grow_capacity(None, (2000, 1000), 1024, false), (1024, 1024));
    assert_eq!(grow_capacity(Some(cap), (320, 200), 16_384, true), (320, 200));
}

#[allow(dead_code)]
struct PresentedStep {
    copy: (u32, u32),
    cap: (u32, u32),
    ready: u64,
}

#[derive(Default)]
struct FakeSeam {
    presents: Vec<PresentedStep>,
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
        copy_size: (u32, u32),
        ready_value: u64,
    ) -> Result<u64, String> {
        self.swap_chain_live = true;
        let (pixels, release_value) = drag.readback_via_d3d11(target, copy_size, ready_value)?;
        self.last_pixels = pixels;
        self.presents.push(PresentedStep { copy: copy_size, cap: target.size, ready: ready_value });
        Ok(release_value)
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

fn sample_view(_: &(), _: &mut crate::Cx<()>) -> crate::El {
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
}

fn sample_app(srgb_blend: bool) -> Runtime<()> {
    let mut rt = crate::App::new(AppSpec::new("drag-sync", (240.0, 160.0), || (), sample_view))
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

fn render_into_ring_slot(
    sync: &mut DragResizeSync,
    gpu: &Gpu,
    renderer: &mut Renderer,
    rt: &mut crate::runtime::RuntimeCore,
    seam: &mut FakeSeam,
    size: (u32, u32),
    format: wgpu::TextureFormat,
) {
    let (slot_idx, slot_release) = {
        let (_drag, idx, slot) = sync.ensure_gpu_target(&gpu.device, size, format, false).expect("ring slot");
        (idx, slot.last_release)
    };
    crate::frame::host_step::prepare_gpu_resources_with(renderer, gpu, rt, size, true, |r, _| {
        r.set_punched_surfaces(Vec::new());
    });
    renderer.next_instances(rt.paint_count(), None);
    let (drag, target) = sync.borrow_slot(slot_idx).expect("borrow slot");
    drag.wait_before_d3d12(&gpu.queue, slot_release).expect("wait release");
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    let clear = rt.clear_color();
    crate::frame::host_step::draw_ui(renderer, gpu, &mut encoder, &target.view, rt, size, clear);
    target.transition_for_d3d11(&mut encoder);
    renderer.submit(&gpu.queue, encoder);
    let ready = drag.signal_after_d3d12(&gpu.queue).expect("signal ready");
    let release = seam.present_shared(drag, target, size, ready).expect("present shared");
    sync.note_slot_release(slot_idx, release);
}

#[test]
fn offscreen_opt_in_off_creates_zero_drag_gpu_objects() {
    #[cfg_attr(not(feature = "test-gpu-counters"), allow(unused_variables))]
    let Some(gpu) = Gpu::test_headless_wgpu() else { return };
    #[cfg(feature = "test-gpu-counters")]
    let base_textures = gpu.device.get_internal_counters().hal.textures.read();

    let mut sync = DragResizeSync::default();
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
    assert!(sync.ring.is_empty());
    assert!(!seam.dcomp_detached && !seam.swap_chain_live && seam.presents.is_empty());

    #[cfg(feature = "test-gpu-counters")]
    assert_eq!(
        gpu.device.get_internal_counters().hal.textures.read(),
        base_textures,
        "opt-in off must allocate zero GPU textures"
    );
}

#[test]
fn offscreen_shared_texture_ring_subrect_bit_exact_and_zero_per_step_alloc() {
    let Some(gpu) = Gpu::test_headless_wgpu() else { return };
    for (srgb_blend, format) in [
        (true, wgpu::TextureFormat::Bgra8Unorm),
        (false, wgpu::TextureFormat::Bgra8UnormSrgb),
    ] {
        let mut rt = sample_app(srgb_blend);
        let mut renderer = Renderer::new(&gpu.device, format);
        renderer.attach_gpu(&gpu);
        renderer.set_srgb_blend(srgb_blend);

        rt.core_mut().set_viewport((240.0, 160.0), 1.0);
        let _ = rt.frame(0.0);
        let normal_1 = render_normal_offscreen(&gpu, &mut renderer, rt.core_mut(), (240, 160), format);
        rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely()).expect("poll");

        #[cfg(feature = "test-gpu-counters")]
        let base_textures = gpu.device.get_internal_counters().hal.textures.read();

        let mut sync = DragResizeSync::default();
        let mut seam = FakeSeam::default();

        // Step 1 (240x160): allocates slot 0 at capacited size (> 240x160).
        assert!(collect(step(&mut sync.seq, &mut sync.state, Event::EnterSizeMove { eligibility: eligible() })).is_empty());
        assert_eq!(
            collect(step(&mut sync.seq, &mut sync.state, Event::Resize { size: (240, 160), eligibility: eligible() })),
            vec![Effect::BeginDrag { size: (240, 160) }]
        );
        render_into_ring_slot(&mut sync, &gpu, &mut renderer, rt.core_mut(), &mut seam, (240, 160), format);
        seam.detach_dcomp().expect("detach dcomp");
        seam.set_valid_rects_active(true);
        let _ = step(&mut sync.seq, &mut sync.state, Event::DragStarted { size: (240, 160) });
        assert_eq!(sync.ring.allocations, 1);
        let cap = sync.ring.capacity().expect("capacity");
        assert!(cap.0 > 240 && cap.1 > 160, "capacited shared texture should have headroom: {cap:?}");
        assert_eq!(
            seam.last_pixels, normal_1,
            "sub-rect render from capacited shared texture must match normal render bit-for-bit ({format:?})"
        );
        #[cfg(feature = "test-gpu-counters")]
        assert_eq!(gpu.device.get_internal_counters().hal.textures.read(), base_textures + 1);

        // Step 2 (320x200): allocates slot 1 in the 2-slot ring; still bit-exact.
        rt.core_mut().set_viewport((320.0, 200.0), 1.0);
        let _ = rt.frame(0.0);
        let normal_2 = render_normal_offscreen(&gpu, &mut renderer, rt.core_mut(), (320, 200), format);
        render_into_ring_slot(&mut sync, &gpu, &mut renderer, rt.core_mut(), &mut seam, (320, 200), format);
        assert_eq!(sync.ring.allocations, 2);
        assert_eq!(
            seam.last_pixels, normal_2,
            "resized sub-rect from slot 1 must match normal render bit-for-bit ({format:?})"
        );

        // Steps 3..22: 20 more grow & shrink resize steps within capacity -> ZERO new textures!
        for step_idx in 0..20u32 {
            let w = if step_idx < 10 { 324 + step_idx * 12 } else { 440 - (step_idx - 10) * 16 };
            let h = 200 + (step_idx % 5) * 12;
            rt.core_mut().set_viewport((w as f32, h as f32), 1.0);
            let _ = rt.frame(0.0);
            render_into_ring_slot(&mut sync, &gpu, &mut renderer, rt.core_mut(), &mut seam, (w, h), format);
        }
        assert_eq!(
            sync.ring.allocations, 2,
            "20 resize steps inside capacity must allocate zero new shared textures"
        );
        rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely()).expect("poll");
        #[cfg(feature = "test-gpu-counters")]
        assert_eq!(gpu.device.get_internal_counters().hal.textures.read(), base_textures + 2);

        // Final bit-exact check on a shrunk sub-rect.
        rt.core_mut().set_viewport((260.0, 172.0), 1.0);
        let _ = rt.frame(0.0);
        let normal_3 = render_normal_offscreen(&gpu, &mut renderer, rt.core_mut(), (260, 172), format);
        render_into_ring_slot(&mut sync, &gpu, &mut renderer, rt.core_mut(), &mut seam, (260, 172), format);
        assert_eq!(seam.last_pixels, normal_3);

        // Exit drag releases both ring slots cleanly.
        seam.attach_dcomp().expect("attach dcomp");
        seam.set_valid_rects_active(false);
        seam.release_swap_chain();
        sync.release_drag_resources();
        assert!(sync.ring.is_empty());
        assert!(sync.device.is_some());
        rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely()).expect("poll");
        #[cfg(feature = "test-gpu-counters")]
        assert_eq!(
            gpu.device.get_internal_counters().hal.textures.read(),
            base_textures,
            "exiting drag must release every shared wgpu texture"
        );

        sync.drop_d3d11_device();
        assert!(sync.device.is_none());
    }
}

#[test]
fn drag_frame_feeds_every_presented_frame_consumer_behind_seam() {
    let Some(gpu) = Gpu::test_headless_wgpu() else { return };
    let format = wgpu::TextureFormat::Bgra8Unorm;
    let mut renderer = Renderer::new(&gpu.device, format);
    renderer.attach_gpu(&gpu);
    renderer.set_srgb_blend(true);

    let now = crate::time::now();
    let mut rt = Runtime::new((), sample_view, (240.0, 160.0), Env::new(true, 1.0));
    rt.wall_start = Some(now);
    rt.set_srgb_blend(true);
    rt.frame(0.0);

    let drawn_hooks = Rc::new(RefCell::new(0usize));
    let drawn_hooks_cb = drawn_hooks.clone();
    let mut app = TypedWindowApp::for_test(
        rt,
        Some(Box::new(move |_rt| { *drawn_hooks_cb.borrow_mut() += 1; })),
    );

    let encoded_reports = Rc::new(RefCell::new(Vec::new()));
    let encoded_cb = encoded_reports.clone();
    let mut frame_reports = FrameReports::new(
        Some(Box::new(move |f| {
            encoded_cb.borrow_mut().push((f.outcome(), f.seq(), f.refresh_hz(), f.update_ms(), f.encode_ms()));
        })),
        Some("main"),
    );
    let mut meter = SurfaceMeter::new("main", Arc::default());
    let mut clock = FrameClock::new(now);
    let mut rest = Rest::default();
    let mut damage = Damage::default();
    let trace_path = std::env::temp_dir().join(format!("rune_drag_sync_trace_{}.json", std::process::id()));
    let mut trace = Some(crate::tooling::Trace::open_clock(&trace_path, (240.0, 160.0), 239.0, 1.0, "wall"));
    let mut trace_frames = 0usize;
    let mut idle = true;
    let mut sync = DragResizeSync::default();
    let mut seam = FakeSeam::default();
    let mut pre_presents = 0usize;

    let mut run_steps = |
        sync: &mut DragResizeSync,
        app: &mut TypedWindowApp<()>,
        clock: &mut FrameClock,
        meter: &mut SurfaceMeter,
        frame_reports: &mut FrameReports,
        rest: &mut Rest,
        damage: &mut Damage,
        trace: &mut Option<crate::tooling::Trace>,
        trace_frames: &mut usize,
        idle: &mut bool,
        seam: &mut FakeSeam,
        pre_presents: &mut usize,
    | {
        for (idx, (w, h)) in [(240u32, 160u32), (280, 176), (320, 192)].into_iter().enumerate() {
            app.rt.core_mut().set_viewport((w as f32, h as f32), 1.0);
            let mut consumers = DragFrameConsumers {
                app,
                clock,
                meter,
                frame_reports,
                observability: None,
                rest,
                damage,
                trace,
                trace_start: Some(now),
                trace_frames,
                trace_limit: None,
                frame_moving: false,
                idle,
                refresh_hz: Some(239.0),
                paced_for_stats: true,
                next_tick: None,
                base_format: format,
            };
            run_drag_frame(
                sync,
                &gpu,
                &mut renderer,
                &mut consumers,
                seam,
                (w, h),
                idx == 0,
                || { *pre_presents += 1; },
            )
            .expect("drag frame succeeds");
        }
    };

    #[cfg(feature = "frame-profile")]
    let logged_frames = crate::frame::live_log::capture_frames(|| {
        run_steps(
            &mut sync, &mut app, &mut clock, &mut meter, &mut frame_reports,
            &mut rest, &mut damage, &mut trace, &mut trace_frames, &mut idle, &mut seam, &mut pre_presents,
        );
    }).1;
    #[cfg(not(feature = "frame-profile"))]
    run_steps(
        &mut sync, &mut app, &mut clock, &mut meter, &mut frame_reports,
        &mut rest, &mut damage, &mut trace, &mut trace_frames, &mut idle, &mut seam, &mut pre_presents,
    );

    // 1. Frame log & frame-profile stages (under `frame-profile`).
    #[cfg(feature = "frame-profile")]
    {
        assert_eq!(logged_frames.len(), 3, "every drag frame must reach RUNE_FRAME_LOG");
        for record in &logged_frames {
            assert_eq!(record["whole"], "drag resize sync");
            assert_eq!(record["loop_why"], "drag resize sync");
            assert!(record["nodes"].as_u64().unwrap_or(0) > 0);
            assert!(record["instances"].as_u64().unwrap_or(0) > 0);
            assert!(record["total"].is_u64());
            assert!(record["build"].is_u64());
            assert!(record["present"].is_u64());
        }
    }

    // 2. SurfaceMeter counts both redraws and presents.
    let totals = meter.totals();
    assert_eq!(totals.redraws, 3);
    assert_eq!(totals.presents, 3);

    // 3. FrameReports (`App::on_frame_encoded`).
    let reports = encoded_reports.borrow();
    assert_eq!(reports.len(), 3);
    for (i, (outcome, seq, hz, update_ms, encode_ms)) in reports.iter().enumerate() {
        assert_eq!(*outcome, FrameOutcome::Drawn);
        assert_eq!(*seq, i as u64);
        assert_eq!(*hz, Some(239.0));
        assert!(update_ms.is_some() && encode_ms.is_some());
    }

    // 4. `App::on_frame_drawn` hook & `pre_present_notify`.
    assert_eq!(*drawn_hooks.borrow(), 3);
    assert_eq!(pre_presents, 3);

    // 5. Trace recorded all 3 drag frames.
    assert_eq!(trace_frames, 3);
    drop(trace);
    let trace_text = std::fs::read_to_string(&trace_path).unwrap_or_default();
    std::fs::remove_file(&trace_path).ok();
    assert!(!trace_text.is_empty(), "trace file must contain recorded drag frames");

    // 6. PaintKeep debug check has zero mismatches and damage history stays reset.
    assert_eq!(app.rt.core().paint.keep_check_mismatches(), 0);
    assert!(damage.seen.is_none());

    // 7. Rest received `RestEvent::Drew` (deadline armed in the future, not idle-expired).
    assert!(rest.deadline().is_some());

    // 8. Seam detached DComp once and presented 3 times using only 2 shared textures.
    assert!(seam.dcomp_detached);
    assert!(seam.valid_rects_active);
    assert_eq!(seam.presents.len(), 3);
    assert_eq!(sync.ring.allocations, 2);

    sync.drop_d3d11_device();
}

#[test]
fn failed_drag_step_rolls_back_config_so_normal_swap_chain_reconfigures() {
    let mut config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: wgpu::TextureFormat::Bgra8Unorm,
        width: 800,
        height: 600,
        present_mode: wgpu::PresentMode::AutoVsync,
        desired_maximum_frame_latency: 1,
        alpha_mode: wgpu::CompositeAlphaMode::Opaque,
        view_formats: vec![],
        color_space: Default::default(),
    };

    // Case 1: BeginDrag fails on first step (800x600 -> 820x600): config stays at 800x600 so
    // `Win::resize_surface` sees (800, 600) != (820, 600) and calls `ResizeBuffers`.
    commit_or_rollback_config(&mut config, (820, 600), (800, 600), false);
    assert_eq!((config.width, config.height), (800, 600));

    // Case 2: Steps 1..2 succeed (800x600 -> 840x600 -> 880x600), then step 3 fails at 900x600:
    // config rolls back to the hidden D3D12 swap chain's pre-drag size (800x600) so normal
    // fallback reconfigures the swap chain even if the failed step returned to a prior drag size.
    commit_or_rollback_config(&mut config, (840, 600), (800, 600), true);
    assert_eq!((config.width, config.height), (840, 600));
    commit_or_rollback_config(&mut config, (880, 600), (800, 600), true);
    assert_eq!((config.width, config.height), (880, 600));
    commit_or_rollback_config(&mut config, (880, 600), (800, 600), false);
    assert_eq!((config.width, config.height), (800, 600));
}

#[test]
fn offscreen_gpu_only_rapid_resize_ring_allocates_at_most_two_textures() {
    let Some(gpu) = Gpu::test_headless_wgpu() else { return };
    let format = wgpu::TextureFormat::Bgra8Unorm;
    let mut rt = sample_app(true);
    let mut renderer = Renderer::new(&gpu.device, format);
    renderer.attach_gpu(&gpu);
    renderer.set_srgb_blend(true);
    rt.core_mut().set_viewport((640.0, 480.0), 1.0);
    let _ = rt.frame(0.0);

    let mut sync = DragResizeSync::default();
    let (drag, _, _) = sync.ensure_gpu_target(&gpu.device, (640, 480), format, false).expect("init");
    let dst = drag.create_test_dst_texture((1024, 768)).expect("dst texture");

    for i in 0..60u32 {
        let w = 640 + (i % 30) * 8;
        let h = 480 + (i % 15) * 8;
        rt.core_mut().set_viewport((w as f32, h as f32), 1.0);
        let _ = rt.frame(0.0);
        let (slot_idx, slot_release) = {
            let (_drag, idx, slot) = sync.ensure_gpu_target(&gpu.device, (w, h), format, false).expect("slot");
            (idx, slot.last_release)
        };
        crate::frame::host_step::prepare_gpu_resources_with(&mut renderer, &gpu, rt.core_mut(), (w, h), true, |r, _| {
            r.set_punched_surfaces(Vec::new());
        });
        renderer.next_instances(rt.core().paint_count(), None);
        let (drag, target) = sync.borrow_slot(slot_idx).expect("borrow");
        drag.wait_before_d3d12(&gpu.queue, slot_release).expect("wait");
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        let clear = rt.core().clear_color();
        crate::frame::host_step::draw_ui(&mut renderer, &gpu, &mut encoder, &target.view, rt.core_mut(), (w, h), clear);
        target.transition_for_d3d11(&mut encoder);
        renderer.submit(&gpu.queue, encoder);
        let ready = drag.signal_after_d3d12(&gpu.queue).expect("signal");
        let release = drag.gpu_copy_only_via_d3d11(target, &dst, (w, h), ready).expect("copy");
        sync.note_slot_release(slot_idx, release);
        rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll).expect("non-blocking poll");
    }
    assert_eq!(sync.ring.allocations, 2, "60 rapid resize steps inside capacity must allocate only 2 ring textures");
    sync.drop_d3d11_device();
}
