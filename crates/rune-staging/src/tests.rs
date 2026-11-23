use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use super::*;

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(out) = future.as_mut().poll(&mut cx) {
            return out;
        }
        std::thread::yield_now();
    }
}

fn gpu() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .expect("a wgpu adapter");
    block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("a wgpu device")
}

fn texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("staging test texture"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn flush_and_wait(device: &wgpu::Device, queue: &wgpu::Queue, staging: &Staging) {
    let mut encoder = device.create_command_encoder(&Default::default());
    staging.flush(&mut encoder);
    queue.submit([encoder.finish()]);
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
}

/// 2 MiB and 4 MiB uploads are kept on return and reused within `KEEP_BYTES` without allocating
/// a new chunk on every upload, while chunks above `KEEP_BYTES` are still freed on return.
#[test]
fn large_chunks_up_to_keep_bytes_are_reused() {
    let (device, queue) = gpu();
    let tex = texture(&device, 1024, 1024);

    // 2 MiB: 512 rows of 1024-wide RGBA8 (4096 bytes/row).
    let staging_2m = Staging::new(&device, &queue);
    let rows_2m = vec![0xAAu8; 2 << 20];
    for _ in 0..4 {
        staging_2m.write_texture_rows(&tex, 0, &rows_2m);
        flush_and_wait(&device, &queue, &staging_2m);
    }
    assert_eq!(staging_2m.stats().chunks, 1, "2 MiB chunk should be kept and reused");

    // 4 MiB: full 1024x1024 RGBA8 page, equal to KEEP_BYTES.
    let staging_4m = Staging::new(&device, &queue);
    let rows_4m = vec![0xBBu8; 4 << 20];
    for _ in 0..4 {
        staging_4m.write_texture_rows(&tex, 0, &rows_4m);
        flush_and_wait(&device, &queue, &staging_4m);
    }
    assert_eq!(staging_4m.stats().chunks, 1, "4 MiB chunk should be kept and reused");

    // 8 MiB: exceeds KEEP_BYTES, so it is freed on return rather than kept.
    let staging_8m = Staging::new(&device, &queue);
    let buf_8m = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("staging test 8m"),
        size: 8 << 20,
        usage: wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let data_8m = vec![0xCCu8; 8 << 20];
    for _ in 0..2 {
        staging_8m.write_buffer(&buf_8m, 0, &data_8m);
        flush_and_wait(&device, &queue, &staging_8m);
    }
    assert_eq!(staging_8m.stats().chunks, 2, "8 MiB chunk exceeds KEEP_BYTES and must not be kept");
}

/// When both a large and a small chunk are free, a small write takes the smallest fitting chunk
/// so the large chunk stays free for the next large write instead of causing a second allocation.
#[test]
fn stage_picks_smallest_fitting_free_chunk() {
    let (device, queue) = gpu();
    let tex = texture(&device, 1024, 512);
    let staging = Staging::new(&device, &queue);
    let large = vec![1u8; 2 << 20];
    let small = vec![2u8; 4096];

    // Push a 2 MiB chunk onto the free list first, then a 128 KiB chunk.
    staging.write_texture_rows(&tex, 0, &large);
    flush_and_wait(&device, &queue, &staging);
    // Use a separate buffer so the small write allocates a 128 KiB chunk while the 2 MiB chunk
    // is in active use, then both return with the 2 MiB chunk first in `free`.
    staging.write_texture_rows(&tex, 0, &large);
    staging.write_texture_rows(&tex, 0, &small);
    flush_and_wait(&device, &queue, &staging);
    assert_eq!(staging.stats().chunks, 2, "one 2 MiB chunk and one 128 KiB chunk");

    // Next frame writes small first, then large: small must pick the 128 KiB chunk so large can
    // reuse the 2 MiB chunk without allocating a third chunk.
    staging.write_texture_rows(&tex, 0, &small);
    staging.write_texture_rows(&tex, 0, &large);
    flush_and_wait(&device, &queue, &staging);
    assert_eq!(staging.stats().chunks, 2, "small write must pick the 128 KiB chunk, leaving the 2 MiB chunk free");
}
