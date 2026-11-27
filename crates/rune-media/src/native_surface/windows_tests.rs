#![allow(unsafe_code)]

use super::*;
use crate::native_surface::gpu_tests::{Gpu, OFFSCREEN_FORMAT, Offscreen, draw, render};
use ::windows::Win32::Graphics::Direct3D12::{
    D3D12_FENCE_FLAG_SHARED, D3D12_HEAP_FLAG_SHARED, D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_DEFAULT,
    D3D12_RESOURCE_DESC, D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS, D3D12_RESOURCE_STATE_COMMON,
    D3D12_TEXTURE_LAYOUT_UNKNOWN,
};
use ::windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static TEST_ID: AtomicU64 = AtomicU64::new(1);

fn unique_name(prefix: &str) -> String {
    let pid = std::process::id();
    let seq = TEST_ID.fetch_add(1, Ordering::Relaxed);
    format!("Local\\rune-media-test-{prefix}-{pid}-{seq}")
}

struct ProducerTexture {
    name: String,
    size: [u32; 2],
    _resource: ID3D12Resource,
    handle: HANDLE,
}

impl Drop for ProducerTexture {
    fn drop(&mut self) {
        // SAFETY: `handle` was created by `CreateSharedHandle` and is owned uniquely here.
        unsafe { CloseHandle(self.handle) }.ok();
    }
}

struct ProducerFence {
    name: String,
    fence: ID3D12Fence,
    handle: HANDLE,
}

impl Drop for ProducerFence {
    fn drop(&mut self) {
        // SAFETY: `handle` was created by `CreateSharedHandle` and is owned uniquely here.
        unsafe { CloseHandle(self.handle) }.ok();
    }
}

fn make_shared_texture(device: &wgpu::Device, prefix: &str, width: u32, height: u32) -> ProducerTexture {
    // SAFETY: borrows the raw DX12 device to create a shared BGRA8 texture for testing.
    let hal = unsafe { device.as_hal::<Dx12>() }.expect("DX12 backend");
    let raw = hal.raw_device();
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Alignment: 0,
        Width: u64::from(width),
        Height: height,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
        Flags: D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS,
    };
    let mut resource: Option<ID3D12Resource> = None;
    // SAFETY: valid descriptors and out-pointer on live D3D12 device.
    unsafe {
        raw.CreateCommittedResource(
            &heap,
            D3D12_HEAP_FLAG_SHARED,
            &desc,
            D3D12_RESOURCE_STATE_COMMON,
            None,
            &mut resource,
        )
    }
    .expect("CreateCommittedResource shared");
    let resource = resource.expect("shared resource");
    let name = unique_name(prefix);
    // SAFETY: creates a named shared NT handle for the live resource.
    let handle = unsafe { raw.CreateSharedHandle(&resource, None, GENERIC_ALL.0, &HSTRING::from(&name)) }
        .expect("CreateSharedHandle texture");
    ProducerTexture { name, size: [width, height], _resource: resource, handle }
}

fn make_shared_fence(device: &wgpu::Device, prefix: &str) -> ProducerFence {
    // SAFETY: borrows the raw DX12 device to create a shared fence for testing.
    let hal = unsafe { device.as_hal::<Dx12>() }.expect("DX12 backend");
    let raw = hal.raw_device();
    // SAFETY: creates a shared fence starting at 0 on a live D3D12 device.
    let fence: ID3D12Fence = unsafe { raw.CreateFence(0, D3D12_FENCE_FLAG_SHARED) }.expect("CreateFence shared");
    let name = unique_name(prefix);
    // SAFETY: creates a named shared NT handle for the live fence.
    let handle = unsafe { raw.CreateSharedHandle(&fence, None, GENERIC_ALL.0, &HSTRING::from(&name)) }
        .expect("CreateSharedHandle fence");
    ProducerFence { name, fence, handle }
}

fn wait_zero(gpu: &Gpu, counter: &std::sync::atomic::AtomicUsize, label: &str) {
    let deadline = rune_core::time::now() + Duration::from_secs(5);
    while counter.load(Ordering::SeqCst) != 0 {
        gpu.assert_alive();
        assert!(rune_core::time::now() < deadline, "{label} stayed non-zero");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn shared_importer_pools_copies_and_prunes_on_resize_and_clear() {
    let Some(gpu) = Gpu::headless_wgpu() else { return };
    let mut renderer = SurfaceRenderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let surface = NativeSurface::new();
    let fence = make_shared_fence(&gpu.device, "fence");

    // 1. Streaming 10 synchronous frames cycling 2 shared textures at 16x16 reuses leased
    // slots (2 slots when frames complete synchronously, always <= COPY_SLOTS) and keeps
    // Prepared::binds bounded well below KEPT_BINDS.
    let ring_16 = [
        make_shared_texture(&gpu.device, "t16a", 16, 16),
        make_shared_texture(&gpu.device, "t16b", 16, 16),
    ];
    let mut frame_id = 0u64;
    for i in 0..10usize {
        frame_id += 1;
        let tex = &ring_16[i % ring_16.len()];
        surface
            .push_shared(
                SharedTexture::named(&tex.name, tex.size[0], tex.size[1]).release_fence(&fence.name, frame_id),
                frame_id,
            )
            .unwrap();
        let _ = render(&gpu, &mut renderer, &[draw(&surface, [0.0, 0.0, 16.0, 16.0])], (16, 16));
    }
    let imp = renderer.windows_importer().expect("windows importer created");
    let in_flight = imp.in_flight_fences_counter();
    assert_eq!(imp.cached_texture_names().len(), 2, "only the 2 active shared handles stay cached");
    assert_eq!(imp.cached_fence_count(), 1);
    let pools = imp.copy_pool_counts();
    assert_eq!(pools.len(), 1);
    assert_eq!(pools[0].0, surface.key());
    assert_eq!(pools[0].1, [16, 16]);
    assert!(pools[0].2 <= COPY_SLOTS && pools[0].2 >= 2, "expected 2..={COPY_SLOTS} pooled slots, got {}", pools[0].2);
    assert!(renderer.kept_bind_counts()[0] < COPY_SLOTS, "bind cache stays capped to the copy ring");
    // SAFETY: plain read of the completed fence value after render() polled the device.
    assert!(unsafe { fence.fence.GetCompletedValue() } >= frame_id);

    // 2. Resizing to 32x32 immediately evicts old 16x16 shared handles, resets the owned copy
    // ring to 1 slot at 32x32, and clears stale 16x16 entries from Prepared::binds.
    let tex_32_old = make_shared_texture(&gpu.device, "t32old", 32, 32);
    frame_id += 1;
    surface
        .push_shared(
            SharedTexture::named(&tex_32_old.name, 32, 32).release_fence(&fence.name, frame_id),
            frame_id,
        )
        .unwrap();
    let _ = render(&gpu, &mut renderer, &[draw(&surface, [0.0, 0.0, 32.0, 32.0])], (32, 32));
    let imp = renderer.windows_importer().unwrap();
    assert_eq!(imp.cached_texture_names(), vec![tex_32_old.name.as_str()], "old-size shared handles evicted immediately");
    assert_eq!(imp.copy_pool_counts(), vec![(surface.key(), [32, 32], 1)], "copy ring reset lazily on resize");
    assert_eq!(renderer.kept_bind_counts(), vec![0], "old-size bind groups cleared on resize");

    // 3. Switching to new texture names at the same 32x32 size evicts abandoned names after
    // STALE_IMPORTS imports.
    let tex_32_new = make_shared_texture(&gpu.device, "t32new", 32, 32);
    for _ in 0..=STALE_IMPORTS {
        frame_id += 1;
        surface
            .push_shared(
                SharedTexture::named(&tex_32_new.name, 32, 32).release_fence(&fence.name, frame_id),
                frame_id,
            )
            .unwrap();
        let _ = render(&gpu, &mut renderer, &[draw(&surface, [0.0, 0.0, 32.0, 32.0])], (32, 32));
    }
    let imp = renderer.windows_importer().unwrap();
    assert_eq!(imp.cached_texture_names(), vec![tex_32_new.name.as_str()], "abandoned same-size handle evicted");
    assert!(imp.copy_pool_counts()[0].2 <= COPY_SLOTS);
    assert!(renderer.kept_bind_counts()[0] < COPY_SLOTS);

    // 4. Clearing the surface drops cached handles, fences, and copy ring on the next prepare,
    // and wakes CompletionWorker so the retired ID3D12Fence drops on an idle window without
    // requiring any further frames or manual device.poll().
    surface.clear();
    renderer.prepare(&gpu.device, &gpu.queue, &[draw(&surface, [0.0, 0.0, 32.0, 32.0])], (32, 32));
    let imp = renderer.windows_importer().unwrap();
    assert!(imp.cached_texture_names().is_empty(), "cleared surface retains no shared textures");
    assert_eq!(imp.cached_fence_count(), 0, "cleared surface retains no active fences");
    assert!(imp.copy_pool_counts().is_empty(), "cleared surface retains no owned copy ring");
    assert!(renderer.kept_bind_counts().is_empty());
    wait_zero(&gpu, &in_flight, "retired fence on idle window");
    // SAFETY: plain read of the completed fence value.
    assert!(unsafe { fence.fence.GetCompletedValue() } >= frame_id);
}

#[test]
fn shared_fence_zero_and_renderer_drop_retire_safely() {
    let Some(gpu) = Gpu::headless_wgpu() else { return };
    let surface = NativeSurface::new();
    let tex = make_shared_texture(&gpu.device, "zero", 16, 16);
    let fence = make_shared_fence(&gpu.device, "zero-fence");

    // Signal value 0 (already <= initial GetCompletedValue() == 0) then immediately drop the
    // SurfaceRenderer while the copy + Signal are queued on the GPU.
    let in_flight = {
        let mut renderer = SurfaceRenderer::new(&gpu.device, OFFSCREEN_FORMAT);
        surface
            .push_shared(SharedTexture::named(&tex.name, 16, 16).release_fence(&fence.name, 0), 0)
            .unwrap();
        let target = Offscreen::new(&gpu.device, 16, 16);
        renderer.prepare(&gpu.device, &gpu.queue, &[draw(&surface, [0.0, 0.0, 16.0, 16.0])], (16, 16));
        assert_eq!(surface.last_error(), None);
        let in_flight = renderer.windows_importer().unwrap().in_flight_fences_counter();
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            renderer.draw(&mut pass, 0);
        }
        renderer.submit(&gpu.queue, encoder);

        // Also queue a higher value right before dropping SurfaceRenderer with no wait.
        surface
            .push_shared(SharedTexture::named(&tex.name, 16, 16).release_fence(&fence.name, 7), 7)
            .unwrap();
        renderer.prepare(&gpu.device, &gpu.queue, &[draw(&surface, [0.0, 0.0, 16.0, 16.0])], (16, 16));
        assert_eq!(surface.last_error(), None);
        in_flight
        // `renderer` drops here, retiring the signaled fence behind an empty wgpu barrier.
    };

    // Once the device polls, the barrier submission completes and drops the retained fence.
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    gpu.assert_alive();
    assert_eq!(in_flight.load(Ordering::SeqCst), 0, "dropped importer's retired fence must release after GPU work");
    // SAFETY: plain read of the completed fence value.
    assert!(unsafe { fence.fence.GetCompletedValue() } >= 7);
}
