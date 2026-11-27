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
    handle_key: Option<DeviceKey>,
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
        out.handle_key = f.gpu_handle().map(|gpu| gpu.key());
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
    assert_eq!(out.handle_key, out.device_key, "Frame::gpu_handle lends the frame's device");
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
const DOUBLE: &str = r#"
@group(0) @binding(0) var<storage, read_write> v: array<u32>;
@compute @workgroup_size(4)
fn main(@builtin(global_invocation_id) id: vec3<u32>) { v[id.x] = v[id.x] * 2u + 1u; }
"#;

/// A worker uploads, compiles a compute pipeline and submits on `gpu` through its handle while
/// this thread submits on the same queue; returns what the worker's dispatch wrote.
fn worker_builds_on(gpu: Gpu) -> Vec<u32> {
    use wgpu::BufferUsages as B;
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.attach_gpu(&gpu);
    let handle = renderer.gpu_handle().expect("an attached renderer lends its device");
    assert_eq!(handle.key(), renderer.device_key());
    let finished = Arc::new(AtomicBool::new(false));
    let worker = std::thread::spawn({
        let finished = Arc::clone(&finished);
        move || handle.with(|device, queue| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("double"), source: wgpu::ShaderSource::Wgsl(DOUBLE.into()) });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("double"), layout: None, module: &module, entry_point: Some("main"), compilation_options: Default::default(), cache: None,
            });
            let values = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 16, usage: B::STORAGE | B::COPY_DST | B::COPY_SRC, mapped_at_creation: false });
            queue.write_buffer(&values, 0, bytemuck::cast_slice(&[1u32, 2, 3, 4]));
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &pipeline.get_bind_group_layout(0),
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: values.as_entire_binding() }] });
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &group, &[]);
                pass.dispatch_workgroups(1, 1, 1);
            }
            queue.submit([encoder.finish()]);
            let done = Arc::new(AtomicBool::new(false));
            queue.on_submitted_work_done({ let done = Arc::clone(&done); move || done.store(true, Ordering::Release) });
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            assert!(done.load(Ordering::Acquire), "the worker waits for its own work off the frame");
            finished.store(true, Ordering::Release);
            values
        })
    });
    let busy = gpu.device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 256, usage: wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    while !finished.load(Ordering::Acquire) && !worker.is_finished() {
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        encoder.clear_buffer(&busy, 0, None);
        gpu.queue.submit([encoder.finish()]);
        gpu.device.poll(wgpu::PollType::Poll).ok();
    }
    let values = worker.join().unwrap().expect("the device is live");
    let out = gpu.device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 16, usage: B::COPY_DST | B::MAP_READ, mapped_at_creation: false });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(&values, 0, &out, 0, 16);
    gpu.queue.submit([encoder.finish()]);
    out.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    bytemuck::cast_slice(&out.slice(..).get_mapped_range().unwrap()).to_vec()
}

#[test]
fn gpu_handle_builds_and_uploads_from_a_worker_thread() {
    for gpu in [Gpu::test_headless(), Gpu::test_headless_wgpu()].into_iter().flatten() {
        let backend = gpu.device.adapter_info().driver;
        assert_eq!(worker_builds_on(gpu), [3, 5, 7, 9], "on {backend:?}");
    }
}

#[test]
fn gpu_handle_lets_go_with_its_gpu() {
    let Some(gpu) = Gpu::test_headless() else { return };
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    renderer.attach_gpu(&gpu);
    let handle = renderer.gpu_handle().unwrap();
    assert!(!handle.is_lost());
    assert!(handle.with(|_, _| ()).is_some());
    drop(gpu);
    assert!(handle.is_lost(), "Rune dropped its GPU");
    assert!(handle.with(|_, _| ()).is_none(), "a handle never brings the device back");
    assert!(renderer.gpu_handle().unwrap().is_lost(), "and neither does the renderer's");
}

#[test]
fn gpu_handle_reports_a_lost_device() {
    for gpu in [Gpu::test_headless(), Gpu::test_headless_wgpu()].into_iter().flatten() {
        let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        renderer.attach_gpu(&gpu);
        let handle = renderer.gpu_handle().unwrap();
        gpu.device.destroy();
        rune_gpu::health::poll(&gpu.device, wgpu::PollType::Poll).ok();
        assert!(handle.is_lost(), "lost while Rune still holds the GPU");
        assert!(handle.with(|_, _| ()).is_none());
    }
}
