use rune::Env;
use rune::compose::{Compositor, Frame};
use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use rune::runtime::Runtime;

use crate::knobs::{Scene, specs};
use crate::{Lab, ui};

use super::LabGfx;

fn draw_on(gfx: &mut LabGfx, rt: &mut Runtime<Lab>, gpu: &Gpu, ui_ren: &mut Renderer, off: &Offscreen, dt: f64) {
    rt.frame(dt);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    let mut frame = Frame {
        device: &gpu.device,
        queue: &gpu.queue,
        encoder: &mut encoder,
        target: &off.view,
        format: OFFSCREEN_FORMAT,
        size: (off.width, off.height),
        dt,
        update_ms: 0.0,
        rt,
        ui: ui_ren,
    };
    gfx.render(&mut frame);
    ui_ren.submit(&gpu.queue, encoder);
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();
}

static GPU_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn lab_gfx_rebuilds_resources_and_hdr_format_when_device_key_changes() {
    let _lock = GPU_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let size = (320, 200);
    let mut lab = Lab::new();
    lab.scene = Scene::Lens;
    let frost = specs(Scene::Lens).iter().position(|k| k.id == "frost").unwrap();
    lab.vals[Scene::Lens.idx()][frost] = 1.0;

    let mut rt = Runtime::new(lab, ui::view, (size.0 as f32, size.1 as f32), Env::default());
    rt.track_marks = true;

    let mut gfx = LabGfx::default();

    // First frame on device 1 (lens-frost scene).
    let gpu1 = Gpu::headless();
    let mut ui1 = Renderer::new(&gpu1.device, OFFSCREEN_FORMAT);
    ui1.attach_text(&mut rt.text.atlas);
    let off1 = Offscreen::new(&gpu1.device, size.0, size.1);
    draw_on(&mut gfx, &mut rt, &gpu1, &mut ui1, &off1, 0.0);
    let key1 = ui1.device_key();
    assert_eq!(gfx.res.as_ref().unwrap().device_key, key1);

    // Replace the device and renderer at the same viewport size and target format,
    // then switch to the lit Lamp scene.
    let gpu2 = Gpu::headless();
    let mut ui2 = Renderer::new(&gpu2.device, OFFSCREEN_FORMAT);
    let key2 = ui2.device_key();
    assert_ne!(key1, key2);
    let off2 = Offscreen::new(&gpu2.device, size.0, size.1);
    drop((off1, ui1, gpu1));
    rt.text.atlas.invalidate();
    rt.images.invalidate();
    ui2.attach_text(&mut rt.text.atlas);
    rt.state.scene = Scene::Lamp;
    rt.invalidate();

    draw_on(&mut gfx, &mut rt, &gpu2, &mut ui2, &off2, 1.0 / 60.0);
    assert_eq!(gfx.res.as_ref().unwrap().device_key, key2);
    assert!(!gpu2.is_lost());
}

#[test]
fn lab_gfx_release_gpu_drops_resources_and_rebuilds_cleanly() {
    let _lock = GPU_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let size = (320, 200);
    let mut lab = Lab::new();
    lab.scene = Scene::Lens;
    let frost = specs(Scene::Lens).iter().position(|k| k.id == "frost").unwrap();
    lab.vals[Scene::Lens.idx()][frost] = 1.0;

    let mut rt = Runtime::new(lab, ui::view, (size.0 as f32, size.1 as f32), Env::default());
    rt.track_marks = true;

    let mut gfx = LabGfx::default();
    let gpu1 = Gpu::headless();
    let mut ui1 = Renderer::new(&gpu1.device, OFFSCREEN_FORMAT);
    ui1.attach_text(&mut rt.text.atlas);
    let off1 = Offscreen::new(&gpu1.device, size.0, size.1);
    draw_on(&mut gfx, &mut rt, &gpu1, &mut ui1, &off1, 0.0);
    assert!(gfx.res.is_some());

    drop((off1, ui1, gpu1));
    gfx.release_gpu();
    assert!(gfx.res.is_none(), "release_gpu must drop LabGfx::res before a replacement device is created");

    let gpu2 = Gpu::headless();
    let mut ui2 = Renderer::new(&gpu2.device, OFFSCREEN_FORMAT);
    rt.text.atlas.invalidate();
    rt.images.invalidate();
    ui2.attach_text(&mut rt.text.atlas);
    let off2 = Offscreen::new(&gpu2.device, size.0, size.1);
    draw_on(&mut gfx, &mut rt, &gpu2, &mut ui2, &off2, 1.0 / 60.0);
    assert_eq!(gfx.res.as_ref().unwrap().device_key, ui2.device_key());
    assert!(!gpu2.is_lost());
}

/// Off-screen Windows D3D12 proof on an AMD hardware adapter: after `ID3D12Device5::RemoveDevice`,
/// D3D12 keeps the removed singleton device alive while `LabGfx::res` still holds resources on it,
/// blocking recreation on AMD; calling `Compositor::release_gpu` drops `LabGfx::res` so the next
/// device on the same AMD hardware adapter succeeds and renders cleanly without falling back to WARP.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
#[test]
fn amd_removed_device_recreation_needs_release_gpu() {
    use windows::Win32::Graphics::Direct3D12::ID3D12Device5;
    use windows::core::Interface;

    let _lock = GPU_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let instance = Gpu::instance();
    let find_amd = || {
        pollster::block_on(instance.enumerate_adapters(wgpu::Backends::DX12))
            .into_iter()
            .find(|a| {
                let info = a.get_info();
                info.vendor == 0x1002 && info.device_type != wgpu::DeviceType::Cpu
            })
    };
    let Some(amd1) = find_amd() else {
        eprintln!("skipped: no AMD DX12 hardware adapter on this machine");
        return;
    };

    let request_amd = |adapter: &wgpu::Adapter| -> Result<(wgpu::Device, wgpu::Queue), String> {
        let optional = wgpu::Features::TIMESTAMP_QUERY
            | wgpu::Features::DUAL_SOURCE_BLENDING
            | wgpu::Features::PASSTHROUGH_SHADERS;
        let required_features = adapter.features() & optional;
        let limits = adapter.limits();
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("lab-amd-release-test"),
            required_features,
            required_limits: wgpu::Limits::default()
                .using_resolution(limits.clone())
                .using_alignment(limits),
            experimental_features: Default::default(),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: Default::default(),
        }))
        .map_err(|e| e.to_string())
    };

    let device_removed_reason = |device: &wgpu::Device| -> Option<String> {
        // SAFETY: inspecting the live DX12 HAL device's removal reason.
        unsafe { device.as_hal::<wgpu::hal::api::Dx12>() }
            .and_then(|hal| unsafe { hal.raw_device().GetDeviceRemovedReason() }.err().map(|e| format!("{e:?}")))
    };
    let device_is_removed = |device: &wgpu::Device| -> bool {
        device_removed_reason(device).is_some()
    };

    let size = (320, 200);
    let mut lab = Lab::new();
    lab.scene = Scene::Lens;
    let frost = specs(Scene::Lens).iter().position(|k| k.id == "frost").unwrap();
    lab.vals[Scene::Lens.idx()][frost] = 1.0;

    let mut rt = Runtime::new(lab, ui::view, (size.0 as f32, size.1 as f32), Env::default());
    rt.track_marks = true;
    let mut gfx = LabGfx::default();

    // 1. Render a lens-frost frame on the real AMD hardware DX12 adapter.
    let (dev1, queue1) = request_amd(&amd1).expect("initial AMD device creation must succeed");
    let mut ui1 = Renderer::new(&dev1, OFFSCREEN_FORMAT);
    ui1.attach_text(&mut rt.text.atlas);
    let off1 = Offscreen::new(&dev1, size.0, size.1);
    rt.frame(0.0);
    {
        let mut encoder = dev1.create_command_encoder(&Default::default());
        let mut frame = Frame {
            device: &dev1,
            queue: &queue1,
            encoder: &mut encoder,
            target: &off1.view,
            format: OFFSCREEN_FORMAT,
            size: (off1.width, off1.height),
            dt: 0.0,
            update_ms: 0.0,
            rt: &mut rt,
            ui: &mut ui1,
        };
        gfx.render(&mut frame);
        ui1.submit(&queue1, encoder);
    }
    dev1.poll(wgpu::PollType::wait_indefinitely()).ok();
    assert!(gfx.res.is_some());

    // 2. Remove the AMD D3D12 device on purpose, then drop Rune's own surface/renderer/device
    // while LabGfx still owns LabGfx::res (the exact pre-fix Host::reattach state).
    // SAFETY: triggering intentional device removal on the test's own DX12 device.
    unsafe {
        let hal = dev1.as_hal::<wgpu::hal::api::Dx12>().expect("DX12 device");
        let dev5: ID3D12Device5 = hal.raw_device().cast().expect("ID3D12Device5");
        dev5.RemoveDevice();
    }
    // Drop the old adapter too, just like `self.gpu = None` drops `Gpu { adapter, device, queue }`:
    // `wgpu_hal::dx12::Adapter` holds an `ID3D12Device` ref, so reattach re-exposes the adapter.
    drop((off1, ui1, queue1, dev1, amd1));

    // While LabGfx::res is still held, re-exposing and creating a device on the same AMD adapter
    // either fails or hands back the removed singleton device.
    let blocked_while_held = match find_amd() {
        None => true,
        Some(probe_amd) => match request_amd(&probe_amd) {
            Err(_) => {
                drop(probe_amd);
                true
            }
            Ok((probe_dev, probe_q)) => {
                let removed = device_is_removed(&probe_dev);
                drop((probe_q, probe_dev, probe_amd));
                removed
            }
        },
    };
    assert!(blocked_while_held, "AMD D3D12 device recreation must be blocked while LabGfx::res holds removed-device resources");

    // 3. Call Compositor::release_gpu() (as Host::reattach now does after self.gpu = None), then
    // recreate on the same AMD hardware adapter and render cleanly.
    gfx.release_gpu();
    assert!(gfx.res.is_none(), "release_gpu must drop LabGfx::res");

    let amd2 = find_amd().expect("AMD adapter must be re-exposable after release_gpu");
    let (dev2, queue2) = request_amd(&amd2).expect("AMD device recreation must succeed after release_gpu");
    assert!(!device_is_removed(&dev2), "new AMD device must be live, not removed");
    let info2 = amd2.get_info();
    assert_eq!(info2.vendor, 0x1002);
    assert_ne!(info2.device_type, wgpu::DeviceType::Cpu, "recovered device must stay on hardware AMD, not WARP");

    let mut ui2 = Renderer::new(&dev2, OFFSCREEN_FORMAT);
    rt.text.atlas.invalidate();
    rt.images.invalidate();
    ui2.attach_text(&mut rt.text.atlas);
    let off2 = Offscreen::new(&dev2, size.0, size.1);
    // Render a lens-frost frame on the recovered AMD hardware device.
    rt.frame(1.0 / 60.0);
    {
        let mut encoder = dev2.create_command_encoder(&Default::default());
        let mut frame = Frame {
            device: &dev2,
            queue: &queue2,
            encoder: &mut encoder,
            target: &off2.view,
            format: OFFSCREEN_FORMAT,
            size: (off2.width, off2.height),
            dt: 1.0 / 60.0,
            update_ms: 0.0,
            rt: &mut rt,
            ui: &mut ui2,
        };
        gfx.render(&mut frame);
        ui2.submit(&queue2, encoder);
    }
    dev2.poll(wgpu::PollType::wait_indefinitely()).ok();
    let reason = device_removed_reason(&dev2);
    assert!(reason.is_none(), "recovered AMD device failed to render lens-frost: {reason:?}");
    assert_eq!(gfx.res.as_ref().unwrap().device_key, ui2.device_key());
}

/// Off-screen Windows D3D12 proof on the AMD hardware adapter: 3 `ID3D12Device5::RemoveDevice`
/// removals separated by 10 s on a synthetic clock (with `release_gpu()` and a healthy `lens-frost`
/// frame between each) keep attempts 1-2 on AMD hardware, switch attempt 3 to WARP
/// (`DeviceType::Cpu`) via `rune_gpu::pick::pick_software` and render cleanly, and 3 subsequent
/// WARP removals 10 s apart reach `LossEffect::GiveUp`.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
#[test]
fn amd_repeated_removals_fall_back_to_warp_then_give_up() {
    use std::time::Duration;
    use rune_gpu::fallback::{GpuMode, LossEffect, LossEvent, LossState, Policy, step_loss};
    use windows::Win32::Graphics::Direct3D12::ID3D12Device5;
    use windows::core::Interface;

    let _lock = GPU_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let instance = Gpu::instance();
    let find_amd = || {
        pollster::block_on(instance.enumerate_adapters(wgpu::Backends::DX12))
            .into_iter()
            .find(|a| {
                let info = a.get_info();
                info.vendor == 0x1002 && info.device_type != wgpu::DeviceType::Cpu
            })
    };
    let Some(initial_amd) = find_amd() else {
        eprintln!("skipped: no AMD DX12 hardware adapter on this machine");
        return;
    };

    let open_device = |adapter: &wgpu::Adapter| -> (wgpu::Device, wgpu::Queue) {
        let optional = wgpu::Features::TIMESTAMP_QUERY
            | wgpu::Features::DUAL_SOURCE_BLENDING
            | wgpu::Features::PASSTHROUGH_SHADERS;
        let required_features = adapter.features() & optional;
        let limits = adapter.limits();
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("lab-amd-fallback-test"),
            required_features,
            required_limits: wgpu::Limits::default()
                .using_resolution(limits.clone())
                .using_alignment(limits),
            experimental_features: Default::default(),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: Default::default(),
        }))
        .expect("device creation must succeed")
    };

    let remove_d3d12 = |device: &wgpu::Device| {
        // SAFETY: triggering intentional device removal on the test's own DX12 device.
        unsafe {
            let hal = device.as_hal::<wgpu::hal::api::Dx12>().expect("DX12 device");
            let dev5: ID3D12Device5 = hal.raw_device().cast().expect("ID3D12Device5");
            dev5.RemoveDevice();
        }
    };

    let is_removed = |device: &wgpu::Device| -> bool {
        // SAFETY: inspecting the test's own DX12 device removal status.
        unsafe { device.as_hal::<wgpu::hal::api::Dx12>() }
            .is_some_and(|hal| unsafe { hal.raw_device().GetDeviceRemovedReason() }.is_err())
    };

    let size = (320, 200);
    let mut lab = Lab::new();
    lab.scene = Scene::Lens;
    let frost = specs(Scene::Lens).iter().position(|k| k.id == "frost").unwrap();
    lab.vals[Scene::Lens.idx()][frost] = 1.0;
    let mut rt = Runtime::new(lab, ui::view, (size.0 as f32, size.1 as f32), Env::default());
    rt.track_marks = true;
    let mut gfx = LabGfx::default();

    let render_frame = |gfx: &mut LabGfx, rt: &mut Runtime<Lab>, dev: &wgpu::Device, queue: &wgpu::Queue, ui_ren: &mut Renderer, off: &Offscreen, dt: f64| {
        rt.frame(dt);
        let mut encoder = dev.create_command_encoder(&Default::default());
        let mut frame = Frame {
            device: dev,
            queue,
            encoder: &mut encoder,
            target: &off.view,
            format: OFFSCREEN_FORMAT,
            size: (off.width, off.height),
            dt,
            update_ms: 0.0,
            rt,
            ui: ui_ren,
        };
        gfx.render(&mut frame);
        ui_ren.submit(queue, encoder);
        dev.poll(wgpu::PollType::wait_indefinitely()).ok();
        assert!(!is_removed(dev), "healthy frame must complete without device removal");
    };

    // Start healthy on AMD hardware.
    let mut adapter = initial_amd;
    let (mut dev, mut queue) = open_device(&adapter);
    let mut ui_ren = Renderer::new(&dev, OFFSCREEN_FORMAT);
    ui_ren.attach_text(&mut rt.text.atlas);
    let mut off = Offscreen::new(&dev, size.0, size.1);
    render_frame(&mut gfx, &mut rt, &dev, &queue, &mut ui_ren, &off, 0.0);

    let mut now = rune::time::now();
    let mut loss_state = LossState::default();

    // 3 AMD removals separated by 10 s with a healthy off-screen frame between each:
    // attempts 1-2 stay on AMD hardware, attempt 3 switches to WARP (DeviceType::Cpu).
    for attempt in 1u8..=3 {
        now += Duration::from_secs(10);
        remove_d3d12(&dev);
        drop((off, ui_ren, queue, dev, adapter));
        gfx.release_gpu();
        rt.text.atlas.invalidate();
        rt.images.invalidate();

        let effect = step_loss(
            &mut loss_state,
            LossEvent::Replace { now, policy: Policy::Automatic, software_available: true },
        )
        .into_iter()
        .next()
        .expect("step_loss must emit an effect");

        if attempt < 3 {
            assert_eq!(effect, LossEffect::Replace { mode: GpuMode::Hardware, switched_to_software: false });
            adapter = find_amd().expect("AMD adapter must recover after release_gpu");
            let info = adapter.get_info();
            assert_eq!(info.vendor, 0x1002);
            assert_ne!(info.device_type, wgpu::DeviceType::Cpu, "attempt {attempt} must stay on AMD hardware");
        } else {
            assert_eq!(effect, LossEffect::Replace { mode: GpuMode::Software, switched_to_software: true });
            let pick = rune_gpu::pick::pick_software(&instance, None)
                .expect("Windows DX12 must pick WARP on software fallback");
            adapter = pick.adapter;
            assert_eq!(adapter.get_info().device_type, wgpu::DeviceType::Cpu, "attempt 3 must switch to WARP");
        }

        let (next_dev, next_queue) = open_device(&adapter);
        dev = next_dev;
        queue = next_queue;
        ui_ren = Renderer::new(&dev, OFFSCREEN_FORMAT);
        ui_ren.attach_text(&mut rt.text.atlas);
        off = Offscreen::new(&dev, size.0, size.1);
        render_frame(&mut gfx, &mut rt, &dev, &queue, &mut ui_ren, &off, 1.0 / 60.0);
    }

    // 3 WARP removals separated by 10 s: attempts 1-2 stay on WARP, attempt 3 gives up.
    for warp_attempt in 1u8..=3 {
        now += Duration::from_secs(10);
        remove_d3d12(&dev);
        drop((off, ui_ren, queue, dev, adapter));
        gfx.release_gpu();
        rt.text.atlas.invalidate();
        rt.images.invalidate();

        let effect = step_loss(
            &mut loss_state,
            LossEvent::Replace { now, policy: Policy::Automatic, software_available: true },
        )
        .into_iter()
        .next()
        .expect("step_loss must emit an effect");

        if warp_attempt < 3 {
            assert_eq!(effect, LossEffect::Replace { mode: GpuMode::Software, switched_to_software: false });
            let pick = rune_gpu::pick::pick_software(&instance, None)
                .expect("WARP must remain available for retries");
            adapter = pick.adapter;
            assert_eq!(adapter.get_info().device_type, wgpu::DeviceType::Cpu);
            let (next_dev, next_queue) = open_device(&adapter);
            dev = next_dev;
            queue = next_queue;
            ui_ren = Renderer::new(&dev, OFFSCREEN_FORMAT);
            ui_ren.attach_text(&mut rt.text.atlas);
            off = Offscreen::new(&dev, size.0, size.1);
            render_frame(&mut gfx, &mut rt, &dev, &queue, &mut ui_ren, &off, 1.0 / 60.0);
        } else {
            assert_eq!(effect, LossEffect::GiveUp, "3rd WARP removal must reach GiveUp");
            assert_eq!(loss_state, LossState::GivenUp);
            break;
        }
    }
}
