//! PROOF ONLY, never merged (rune-proof: device-removal). Removes a D3D12 device as a GPU reset
//! does, and only on WARP, the software adapter: on anything else the process ends first.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// When the device was removed, once it was.
pub(crate) static REMOVED: OnceLock<Instant> = OnceLock::new();

/// Whether this run asked for the removal named `what` (`RUNE_PROOF_REMOVE=<what>`).
pub(crate) fn asked(what: &str) -> bool {
    #[allow(clippy::disallowed_methods)]
    let asked = std::env::var("RUNE_PROOF_REMOVE").is_ok_and(|v| v == what)
        // Or the example's own name: `gpu_safe_window`, `gpu_safe_uiwait`, `gpu_safe_snap`.
        || std::env::current_exe().ok().and_then(|e| e.file_stem().map(|s| s.to_string_lossy().into_owned())).is_some_and(|s| s == format!("gpu_safe_{what}"));
    asked
}

pub(crate) fn remove(gpu: &crate::render::Gpu, at: &str) {
    remove_on(&gpu.adapter, &gpu.device, at);
}

fn remove_on(adapter: &wgpu::Adapter, device: &wgpu::Device, at: &str) {
    let info = adapter.get_info();
    if info.device_type != wgpu::DeviceType::Cpu || !info.name.contains("Microsoft Basic Render Driver") {
        eprintln!("proof: refusing to remove a device on {:?} ({:?}): proofs remove devices on WARP only", info.name, info.device_type);
        std::process::exit(3);
    }
    #[cfg(target_os = "windows")]
    {
        use windows::core::Interface;
        let hal = unsafe { device.as_hal::<wgpu::hal::api::Dx12>() }.expect("a D3D12 device");
        let device5: windows::Win32::Graphics::Direct3D12::ID3D12Device5 = hal.raw_device().cast().expect("ID3D12Device5");
        unsafe { device5.RemoveDevice() };
    }
    let _ = REMOVED.set(Instant::now());
    eprintln!("proof: ID3D12Device5::RemoveDevice called on {:?} {at}", info.name);
}

/// Microseconds since the removal, if there was one.
pub(crate) fn since() -> Option<u128> {
    REMOVED.get().map(|at| at.elapsed().as_micros())
}

/// The UI thread waits on GPU work in flight while another thread removes the device 40 ms in:
/// how long after the removal the wait returns, and with what.
pub(crate) fn ui_wait(gpu: &crate::render::Gpu) {
    let device = &gpu.device;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("proof ui wait"),
        source: wgpu::ShaderSource::Wgsl(
            "@group(0) @binding(0) var<storage, read_write> r: array<u32>;
            @compute @workgroup_size(64) fn main(@builtin(global_invocation_id) id: vec3<u32>) {
                var x = id.x; for (var i = 0u; i < 256u; i++) { x = (x ^ (x >> 13u)) * 1664525u + 1013904223u; } r[id.x] = x; }"
                .into(),
        ),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: None, layout: None, module: &shader, entry_point: Some("main"), compilation_options: Default::default(), cache: None });
    let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 65536 * 4, usage: wgpu::BufferUsages::STORAGE, mapped_at_creation: false });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &pipeline.get_bind_group_layout(0), entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }] });
    let started = Instant::now();
    for _ in 0..32 {
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(1024, 1, 1);
        }
        gpu.queue.submit([encoder.finish()]);
    }
    let (adapter, remover) = (gpu.adapter.clone(), device.clone());
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(40));
        remove_on(&adapter, &remover, "from another thread, 40 ms into the UI thread's wait");
    });
    let polled = rune_gpu::health::poll(device, wgpu::PollType::wait_indefinitely());
    let returned = Instant::now();
    let _ = thread.join();
    eprintln!(
        "proof: the UI thread's wait returned {polled:?} {} us after it began, {:?} us after the removal (negative: before it)",
        (returned - started).as_micros(),
        REMOVED.get().map(|at| returned.saturating_duration_since(*at).as_micros() as i128 - at.saturating_duration_since(returned).as_micros() as i128)
    );
}

/// Whether D3D12 would make a new device on WARP now: a removed device still referenced in the
/// process is returned again (D3D12 keeps one device per adapter), so a replacement can't start.
pub(crate) fn probe_new_device(at: &str) {
    if REMOVED.get().is_none() {
        return;
    }
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Graphics::{Direct3D::D3D_FEATURE_LEVEL_11_0, Direct3D12::{D3D12CreateDevice, ID3D12Device}, Dxgi::{CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory4}};
        let started = Instant::now();
        let answer = (|| -> Result<String, windows::core::Error> {
            let factory: IDXGIFactory4 = unsafe { CreateDXGIFactory1() }?;
            let warp: IDXGIAdapter = unsafe { factory.EnumWarpAdapter() }?;
            let mut device: Option<ID3D12Device> = None;
            unsafe { D3D12CreateDevice(&warp, D3D_FEATURE_LEVEL_11_0, &mut device) }?;
            let device = device.expect("D3D12CreateDevice gave no device");
            Ok(match unsafe { device.GetDeviceRemovedReason() } {
                Ok(()) => "a working device".into(),
                Err(e) => format!("the removed device again ({e})"),
            })
        })()
        .unwrap_or_else(|e| format!("an error ({e})"));
        eprintln!("proof: {at}, {} us after the removal: D3D12CreateDevice on WARP gives {answer} in {} us", since().unwrap_or(0), started.elapsed().as_micros());
    }
}
