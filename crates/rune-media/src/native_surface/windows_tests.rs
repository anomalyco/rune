#![allow(unsafe_code)]

use super::*;
use crate::native_surface::gpu_tests::{Gpu, OFFSCREEN_FORMAT, draw, render};
use ::windows::Win32::Graphics::Direct3D12::{
    D3D12_FENCE_FLAG_SHARED, D3D12_HEAP_FLAG_SHARED, D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_DEFAULT,
    D3D12_RESOURCE_DESC, D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS, D3D12_RESOURCE_STATE_COMMON,
    D3D12_TEXTURE_LAYOUT_UNKNOWN,
};
use ::windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
use std::sync::atomic::{AtomicU64, Ordering};

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

#[test]
fn shared_importer_pools_copies_and_prunes_on_resize_and_clear() {
    let Some(gpu) = Gpu::headless_wgpu() else { return };
    let mut renderer = SurfaceRenderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let surface = NativeSurface::new();
    let fence = make_shared_fence(&gpu.device, "fence");

    // 1. Streaming 10 frames cycling 2 shared textures at 16x16 must cap owned copy targets at
    // COPY_SLOTS (3) and keep Prepared::binds bounded to COPY_SLOTS - 1 (2).
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
    assert_eq!(imp.cached_texture_names().len(), 2, "only the 2 active shared handles stay cached");
    assert_eq!(imp.cached_fence_count(), 1);
    assert_eq!(imp.copy_pool_counts(), vec![(surface.key(), [16, 16], COPY_SLOTS)]);
    assert_eq!(renderer.kept_bind_counts(), vec![COPY_SLOTS - 1], "bind cache stays capped to the copy ring");
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
    assert_eq!(imp.copy_pool_counts(), vec![(surface.key(), [32, 32], COPY_SLOTS)]);
    assert_eq!(renderer.kept_bind_counts(), vec![COPY_SLOTS - 1]);

    // 4. Clearing the surface drops cached handles, fences, and copy ring on the next prepare.
    surface.clear();
    renderer.prepare(&gpu.device, &gpu.queue, &[draw(&surface, [0.0, 0.0, 32.0, 32.0])], (32, 32));
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    renderer.prepare(&gpu.device, &gpu.queue, &[], (32, 32));
    let imp = renderer.windows_importer().unwrap();
    assert!(imp.cached_texture_names().is_empty(), "cleared surface retains no shared textures");
    assert_eq!(imp.cached_fence_count(), 0, "cleared surface retains no active fences");
    assert_eq!(imp.retiring_fence_count(), 0, "completed retiring fences drained");
    assert!(imp.copy_pool_counts().is_empty(), "cleared surface retains no owned copy ring");
    assert!(renderer.kept_bind_counts().is_empty());
    // SAFETY: plain read of the completed fence value.
    assert!(unsafe { fence.fence.GetCompletedValue() } >= frame_id);
}

#[test]
fn byte_frames_do_not_retain_stale_binds() {
    let Some(gpu) = Gpu::headless_wgpu() else { return };
    let mut renderer = SurfaceRenderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let surface = NativeSurface::new();
    for id in 1..=12u64 {
        surface.push_bytes(synthetic(16, 16, id), id).unwrap();
        let _ = render(&gpu, &mut renderer, &[draw(&surface, [0.0, 0.0, 16.0, 16.0])], (16, 16));
    }
    assert_eq!(renderer.kept_bind_counts(), vec![0], "one-shot ByteFrame textures are not retained in Prepared::binds");
}
