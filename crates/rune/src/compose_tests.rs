//! Tests for `compose::Frame::instance`, `Frame::adapter`, and `Frame::restore_device_callbacks`.

use std::sync::{Arc, Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}};

use crate::compose::{Compositor, DeviceKey, Frame};
use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use crate::runtime::{Env, Runtime};
use crate::tooling::{AppSpec, Args};
use crate::{App, Color, Cx, El, WindowSpec, el};

#[derive(Default)]
struct ProbeResult {
    calls: usize,
    has_instance: bool,
    adapter_info: Option<wgpu::AdapterInfo>,
    device_adapter_info: Option<wgpu::AdapterInfo>,
    device_key: Option<DeviceKey>,
    cloned_ok: bool,
}

struct HostProbe(Arc<Mutex<ProbeResult>>);

impl Compositor<()> for HostProbe {
    fn render(&mut self, f: &mut Frame<'_, ()>) {
        let mut out = self.0.lock().unwrap();
        out.calls += 1;
        out.has_instance = f.instance().is_some();
        out.adapter_info = f.adapter().map(wgpu::Adapter::get_info);
        out.device_adapter_info = Some(f.device.adapter_info());
        out.device_key = Some(f.device_key());
        if let (Some(inst), Some(adap)) = (f.instance(), f.adapter()) {
            let inst_clone = inst.clone();
            let adap_clone = adap.clone();
            out.cloned_ok = adap_clone.get_info() == f.device.adapter_info();
            drop((inst_clone, adap_clone));
        }
        f.restore_device_callbacks();
        f.draw_ui();
    }
}

fn plain_view(_: &(), _: &mut Cx<()>) -> El {
    el().size(64.0, 48.0).fill(Color::WHITE)
}

#[test]
fn headless_compositor_receives_instance_and_adapter_matching_device() {
    if !Gpu::test_adapter_available() { return; }
    let _turn = crate::tooling::headless_run::test_turn();
    let probe = Arc::new(Mutex::new(ProbeResult::default()));
    let app = App::new(AppSpec::new("compose-probe", (64.0, 48.0), || (), plain_view))
        .compositor(HostProbe(Arc::clone(&probe)));
    let args = Args { scale: Some(1.0), ..Default::default() };
    crate::tooling::try_headless(app, args)
        .map_err(|e| e.message().to_owned())
        .expect("headless compositor run");
    let out = probe.lock().unwrap();
    assert!(out.calls >= 1, "compositor must render at least one frame");
    assert!(out.has_instance, "headless Frame::instance must be Some");
    assert_eq!(
        out.adapter_info,
        out.device_adapter_info,
        "headless Frame::adapter info must match Frame::device.adapter_info()",
    );
    assert!(out.cloned_ok, "cloned instance and adapter must stay valid and match the device");
}

#[test]
fn virtual_desktop_compositor_receives_instance_and_adapter() {
    if !Gpu::test_adapter_available() { return; }
    let _turn = crate::tooling::headless_run::test_turn();
    let dir = std::env::temp_dir().join(format!("rune-compose-vdesktop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let backdrop = dir.join("backdrop.png");
    ::image::RgbaImage::from_pixel(128, 96, ::image::Rgba([64, 64, 64, 255]))
        .save(&backdrop)
        .unwrap();
    let probe_main = Arc::new(Mutex::new(ProbeResult::default()));
    let probe_aux = Arc::new(Mutex::new(ProbeResult::default()));
    let aux = App::new(AppSpec::new("aux", (48.0, 32.0), || (), plain_view))
        .compositor(HostProbe(Arc::clone(&probe_aux)));
    let mut app = App::new(AppSpec::new("main", (64.0, 48.0), || (), plain_view))
        .compositor(HostProbe(Arc::clone(&probe_main)));
    let _ = app.window(WindowSpec::from_app("aux", aux));
    let args = Args {
        desktop: Some(backdrop),
        scale: Some(1.0),
        ..Default::default()
    };
    let result = crate::tooling::try_headless(app, args).map_err(|e| e.message().to_owned());
    std::fs::remove_dir_all(&dir).ok();
    result.expect("virtual desktop compositor run");
    for (label, probe) in [("main", &probe_main), ("aux", &probe_aux)] {
        let out = probe.lock().unwrap();
        assert!(out.calls >= 1, "{label}: compositor must render at least one frame");
        assert!(out.has_instance, "{label}: virtual desktop Frame::instance must be Some");
        assert_eq!(
            out.adapter_info,
            out.device_adapter_info,
            "{label}: virtual desktop Frame::adapter info must match Frame::device.adapter_info()",
        );
        assert!(out.cloned_ok);
    }
}

#[test]
fn standalone_renderer_frame_returns_none_and_restore_is_noop() {
    let Some(gpu) = Gpu::test_headless_or_noop() else { return };
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let mut rt = Runtime::new((), plain_view, (32.0, 32.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let target = Offscreen::new(&gpu.device, 32, 32);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    let foreign_called = Arc::new(AtomicBool::new(false));
    let foreign_flag = Arc::clone(&foreign_called);
    gpu.device.set_device_lost_callback(move |_, _| {
        foreign_flag.store(true, Ordering::SeqCst);
    });
    let frame = Frame {
        device: &gpu.device,
        queue: &gpu.queue,
        encoder: &mut encoder,
        target: &target.view,
        format: OFFSCREEN_FORMAT,
        size: (32, 32),
        dt: 0.0,
        update_ms: 0.0,
        rt: &mut rt,
        ui: &mut renderer,
    };
    assert!(frame.instance().is_none(), "standalone Renderer::new must report None for Frame::instance");
    assert!(frame.adapter().is_none(), "standalone Renderer::new must report None for Frame::adapter");
    frame.restore_device_callbacks();
    drop(encoder);
    gpu.device.destroy();
    rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll).ok();
    assert!(
        foreign_called.load(Ordering::SeqCst),
        "restore_device_callbacks on standalone Renderer::new must be a no-op",
    );
    assert!(
        !gpu.is_lost(),
        "standalone Renderer::new has no attached Gpu callbacks to restore",
    );
}

#[test]
fn attached_renderer_frame_restores_device_lost_callback() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.attach_gpu(&gpu);
    let mut rt = Runtime::new((), plain_view, (32.0, 32.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let target = Offscreen::new(&gpu.device, 32, 32);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());

    let foreign_calls = Arc::new(AtomicUsize::new(0));
    let foreign_cb = Arc::clone(&foreign_calls);
    gpu.device.set_device_lost_callback(move |_, _| {
        foreign_cb.fetch_add(1, Ordering::SeqCst);
    });

    {
        let frame = Frame {
            device: &gpu.device,
            queue: &gpu.queue,
            encoder: &mut encoder,
            target: &target.view,
            format: OFFSCREEN_FORMAT,
            size: (32, 32),
            dt: 0.0,
            update_ms: 0.0,
            rt: &mut rt,
            ui: &mut renderer,
        };
        assert!(frame.instance().is_some());
        assert_eq!(
            frame.adapter().map(wgpu::Adapter::get_info),
            Some(frame.device.adapter_info()),
        );
        frame.restore_device_callbacks();
    }
    drop(encoder);
    assert!(!gpu.is_lost(), "live device must not report lost before destroy");
    gpu.device.destroy();
    rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll).ok();
    assert!(
        gpu.is_lost(),
        "Gpu::is_lost must flip to true on device.destroy() after restore_device_callbacks",
    );
    assert_eq!(
        foreign_calls.load(Ordering::SeqCst),
        0,
        "overwritten foreign device-lost callback must no longer fire after restore_device_callbacks",
    );
}

#[test]
fn instance_effect_mark_decodes_full_u32_and_rejects_other_kinds() {
    use bytemuck::Zeroable;
    use crate::render::{
        Instance, KIND_BACKDROP_BLUR, KIND_EFFECT, KIND_GLYPH, KIND_LAYER, KIND_LAYER_END,
        KIND_RECT, KIND_SHADOW,
    };

    for mark in [0u32, 1, 42, 65_535, 65_536, 70_000, 0xDEAD_BEEF, u32::MAX] {
        let inst = Instance {
            border: [(mark & 0xFFFF) as f32, (mark >> 16) as f32, 0.0, 0.0],
            params: [0.0, KIND_EFFECT, 0.0, 1.0],
            ..Instance::zeroed()
        };
        assert_eq!(inst.effect_mark(), Some(mark), "mark {mark:#x} must round-trip");
    }

    for kind in [KIND_RECT, KIND_SHADOW, KIND_GLYPH, KIND_LAYER, KIND_LAYER_END, KIND_BACKDROP_BLUR, 99.0, f32::NAN] {
        let inst = Instance {
            border: [42.0, 1.0, 0.0, 0.0],
            params: [0.0, kind, 0.0, 1.0],
            ..Instance::zeroed()
        };
        assert_eq!(inst.effect_mark(), None, "non-effect kind {kind:?} must return None");
    }
}

#[test]
fn runtime_effect_slots_works_without_track_marks_and_excludes_backdrop_blur() {
    use crate::effect;

    const ID: f32 = f32::from_bits(0xDEAD_BEEF);
    const NAN: f32 = f32::from_bits(0x7FC0_1234);

    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().size(120.0, 80.0)
            .child(
                effect(7)
                    .absolute(10.0, 20.0)
                    .size(40.0, 30.0)
                    .effect_data([1.0, ID, NAN, -0.5])
                    .effect_bleed(4.0)
                    .effect_backdrop(true),
            )
            .child(el().backdrop_blur(8.0).absolute(0.0, 0.0).size(20.0, 20.0))
            .child(
                el().opacity(0.5).child(
                    effect(70_000)
                        .absolute(60.0, 10.0)
                        .size(24.0, 16.0)
                        .scale(1.5),
                ),
            )
    }

    let mut rt = Runtime::new((), view, (120.0, 80.0), Env::new(true, 2.0));
    assert!(!rt.track_marks, "standalone Runtime starts with track_marks off");
    rt.frame(0.0);
    assert!(rt.marks.is_empty(), "Runtime::marks must remain empty when track_marks is off");

    let slots: Vec<_> = rt.effect_slots().collect();
    assert_eq!(slots.len(), 2, "must list only Kind::Effect slots, excluding backdrop_blur");

    let first = slots[0];
    assert_eq!(first.mark, 7);
    assert_eq!(first.instance.effect_mark(), Some(7));
    assert_eq!(first.rect, [20.0, 40.0, 80.0, 60.0]);
    assert_eq!(first.scale, 2.0);
    assert_eq!(first.bleed, 8.0);
    assert!(first.backdrop);
    assert_eq!(first.data.map(f32::to_bits), [1.0, ID, NAN, -0.5].map(f32::to_bits));

    let second = slots[1];
    assert_eq!(second.mark, 70_000);
    assert_eq!(second.instance.effect_mark(), Some(70_000));
    assert_eq!(second.scale, 3.0, "Env::scale (2.0) * El::scale (1.5)");
    assert!(!second.backdrop);
    assert_eq!(second.bleed, 0.0);
    assert!((second.rect[2] - 72.0).abs() <= 1.0, "scaled width in device px: {:?}", second.rect);
    assert!((second.rect[3] - 48.0).abs() <= 1.0, "scaled height in device px: {:?}", second.rect);

    assert_eq!(rt.effect_slot(7).map(|s| s.index), Some(first.index));
    assert_eq!(rt.effect_slot(70_000).map(|s| s.index), Some(second.index));
    assert!(rt.effect_slot(999).is_none());
}

#[test]
fn runtime_effect_slot_updates_in_same_frame_on_resize() {
    use crate::effect;

    const VIEWPORT_MARK: u32 = 101;

    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().grow(1.0).child(effect(VIEWPORT_MARK).grow(1.0))
    }

    let mut rt = Runtime::new((), view, (100.0, 50.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let slot = rt.effect_slot(VIEWPORT_MARK).expect("slot present on initial frame");
    assert_eq!(slot.rect, [0.0, 0.0, 200.0, 100.0]);
    assert_eq!(slot.scale, 2.0);

    rt.set_viewport((160.0, 90.0), 2.0);
    rt.frame(1.0 / 60.0);
    let resized = rt.effect_slot(VIEWPORT_MARK).expect("slot present on resized frame");
    assert_eq!(resized.rect, [0.0, 0.0, 320.0, 180.0], "resized slot rect must be available in the same frame");
    assert_eq!(resized.scale, 2.0);

    rt.set_viewport((160.0, 90.0), 1.5);
    rt.frame(2.0 / 60.0);
    let rescaled = rt.effect_slot(VIEWPORT_MARK).expect("slot present on rescaled frame");
    assert_eq!(rescaled.rect, [0.0, 0.0, 240.0, 135.0], "rescaled slot rect must be available in the same frame");
    assert_eq!(rescaled.scale, 1.5);
}

#[test]
fn frame_effect_slots_before_prepare_match_draw_with_sites() {
    use crate::effect;

    const SCENE_MARK: u32 = 10;
    const FADED_MARK: u32 = 80_000;

    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().size(96.0, 64.0)
            .child(
                effect(SCENE_MARK)
                    .absolute(4.0, 6.0)
                    .size(40.0, 24.0)
                    .effect_data([0.25, 0.5, 0.75, 1.0]),
            )
            .child(
                effect(FADED_MARK)
                    .absolute(48.0, 8.0)
                    .size(32.0, 20.0)
                    .opacity(0.5)
                    .effect_data([2.0, 3.0, 4.0, 5.0]),
            )
    }

    let Some(gpu) = Gpu::test_headless_or_noop() else { return };
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let mut rt = Runtime::new((), view, (96.0, 64.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let size = (192, 128);
    let target = Offscreen::new(&gpu.device, size.0, size.1);
    let scratch = Offscreen::new(&gpu.device, 32, 32);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());

    let frame = Frame {
        device: &gpu.device,
        queue: &gpu.queue,
        encoder: &mut encoder,
        target: &target.view,
        format: OFFSCREEN_FORMAT,
        size,
        dt: 0.0,
        update_ms: 0.0,
        rt: &mut rt,
        ui: &mut renderer,
    };

    // Inspect slots BEFORE calling f.ui.prepare or opening any passes.
    let pre_slots: Vec<(u32, u32, [f32; 4], f32, [u32; 4])> = frame
        .effect_slots()
        .map(|s| (s.index, s.mark, s.rect, s.scale, s.data.map(f32::to_bits)))
        .collect();
    assert_eq!(pre_slots.len(), 2);
    assert_eq!(frame.effect_slot(SCENE_MARK).map(|s| s.rect), Some([8.0, 12.0, 80.0, 48.0]));
    assert_eq!(frame.effect_slot(FADED_MARK).map(|s| s.rect), Some([96.0, 16.0, 64.0, 40.0]));

    // Iterating frame.rt.effect_slots() leaves frame.encoder unborrowed for compositor passes.
    let mut offscreen_passes = 0usize;
    for _slot in frame.rt.effect_slots() {
        let _pass = frame.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("compositor-offscreen"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &scratch.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        offscreen_passes += 1;
    }
    assert_eq!(offscreen_passes, 2);

    // Prepare and draw Rune's pass, confirming every pre-pass slot matches EffectSite.
    let clear = frame.rt.clear_color();
    frame.ui.prepare(frame.device, frame.queue, &mut frame.rt.text.atlas, &frame.rt.instances, frame.size);
    let mut draw_sites: Vec<(u32, u32, [f32; 4], f32, [u32; 4], bool)> = Vec::new();
    frame.ui.draw_with(frame.encoder, frame.target, clear, &mut |_pass, site| {
        draw_sites.push((
            site.index,
            site.mark,
            site.instance.rect,
            site.scale,
            site.data.map(f32::to_bits),
            site.in_layer,
        ));
    });
    gpu.queue.submit([encoder.finish()]);

    // Isolated layers draw before the frame pass, so sort by painter index to compare.
    draw_sites.sort_by_key(|s| s.0);
    assert_eq!(draw_sites.len(), pre_slots.len());
    for (pre, draw) in pre_slots.iter().zip(draw_sites.iter()) {
        assert_eq!(pre.0, draw.0, "index must match EffectSite::index");
        assert_eq!(pre.1, draw.1, "mark must match EffectSite::mark");
        assert_eq!(pre.2, draw.2, "rect must match EffectSite::instance.rect");
        assert_eq!(pre.3, draw.3, "scale must match EffectSite::scale");
        assert_eq!(pre.4, draw.4, "data must match EffectSite::data bit for bit");
    }
    assert!(!draw_sites[0].5, "plain slot draws on frame target");
    assert!(draw_sites[1].5, "faded slot draws inside isolated layer");
}
