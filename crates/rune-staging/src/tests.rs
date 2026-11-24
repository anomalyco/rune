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

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = match block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())) {
        Ok(adapter) => adapter,
        Err(error) => {
            if std::env::var_os("RUNE_ALLOW_NO_GPU")
                .is_some_and(|v| matches!(v.to_str(), Some("1" | "true" | "TRUE" | "yes")))
            {
                eprintln!("skipped: no GPU adapter: {error} (RUNE_ALLOW_NO_GPU=1)");
                return None;
            }
            panic!("a wgpu adapter: {error}");
        }
    };
    Some(block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("a wgpu device"))
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
    let Some((device, queue)) = gpu() else { return };
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
    let Some((device, queue)) = gpu() else { return };
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

fn kept_normal_bytes(staging: &Staging) -> u64 {
    let mut s = staging.lock();
    s.receive();
    s.free.iter().map(|c| c.buffer.size()).sum()
}

fn kept_oversized(staging: &Staging) -> (usize, u64) {
    let mut s = staging.lock();
    s.receive();
    (s.oversized_free.len(), s.oversized_bytes())
}

/// First oversized upload (8 MiB or 1080x2400 = 10.44 MiB) is one-shot: it allocates at 64 KiB
/// alignment (10.0 MiB, not 16 MiB), retains 0 oversized bytes immediately, has no in-flight
/// recyclable chunks, and still increments `Staging::lost()` if its encoder is dropped unsubmitted.
#[test]
fn one_shot_oversized_retains_zero_bytes_and_preserves_lost_counter() {
    let Some((device, queue)) = gpu() else { return };
    let tex = texture(&device, 1080, 2400);
    let staging = Staging::new(&device, &queue);
    let frame = vec![0x5Au8; 1080 * 2400 * 4];

    staging.write_texture_rows(&tex, 0, &frame);
    {
        let s = staging.lock();
        assert_eq!(s.active.len(), 1);
        assert_eq!(s.active[0].buffer.size(), 10 << 20, "1080x2400 padded allocates 10.0 MiB, not 16 MiB");
        assert!(!s.active[0].recyclable, "first oversized write is one-shot");
    }
    flush_and_wait(&device, &queue, &staging);
    assert!(!staging.has_oversized_in_flight(), "one-shot chunk does not remap or track in-flight");
    assert_eq!(kept_oversized(&staging), (0, 0), "one-shot oversized write retains 0 bytes");
    assert_eq!(staging.lost(), 0, "submitted one-shot write is not lost");

    // Dropped unsubmitted encoder with a one-shot oversized write still increments lost().
    let staging_lost = Staging::new(&device, &queue);
    staging_lost.write_texture_rows(&tex, 0, &frame);
    let mut enc = device.create_command_encoder(&Default::default());
    staging_lost.flush(&mut enc);
    drop(enc);
    assert_eq!(staging_lost.lost(), 1, "dropped unsubmitted one-shot encoder counts as lost");
    assert_eq!(kept_oversized(&staging_lost), (0, 0));
}

/// Recurring 1080x2400 uploads allocate 10.0 MiB chunks and reach 0 new chunks/frame in both
/// serial (1 chunk = 10.0 MiB held) and 2-in-flight pipelined (2 chunks = 20.0 MiB held) loops.
#[test]
fn recurring_oversized_1080x2400_reuses_10mib_chunks_serial_and_pipelined() {
    let Some((device, queue)) = gpu() else { return };
    let tex = texture(&device, 1080, 2400);
    let frame = vec![0x3Cu8; 1080 * 2400 * 4];

    // Serial loop: frame 1 one-shot (dropped), frame 2 allocates 10.0 MiB recyclable chunk,
    // frames 3..10 reuse it with 0 new chunks.
    let serial = Staging::new(&device, &queue);
    for _ in 0..10 {
        serial.write_texture_rows(&tex, 0, &frame);
        flush_and_wait(&device, &queue, &serial);
    }
    assert_eq!(serial.stats().chunks, 2, "1 one-shot + 1 reused 10.0 MiB chunk across 10 serial frames");
    assert_eq!(kept_oversized(&serial), (1, 10 << 20), "serial stream retains 1 chunk (10.0 MiB)");

    // 2-in-flight pipelined loop: keep previous submission in flight while submitting the next.
    let pipelined = Staging::new(&device, &queue);
    let mut prev: Option<wgpu::SubmissionIndex> = None;
    for i in 0..12 {
        pipelined.write_texture_rows(&tex, 0, &frame);
        let mut enc = device.create_command_encoder(&Default::default());
        pipelined.flush(&mut enc);
        let sub = queue.submit([enc.finish()]);
        if let Some(old) = prev.replace(sub) {
            device.poll(wgpu::PollType::Wait { submission_index: Some(old), timeout: None }).unwrap();
        }
        if i == 4 {
            // By frame 5 (index 4), warmup (1 one-shot + at most 2 pipelined chunks) is done.
            let chunks_at_4 = pipelined.stats().chunks;
            assert!(chunks_at_4 <= 3, "pipelined warmup created at most 3 chunks, got {chunks_at_4}");
        }
    }
    if let Some(old) = prev.take() {
        device.poll(wgpu::PollType::Wait { submission_index: Some(old), timeout: None }).unwrap();
    }
    let chunks_before_steady = pipelined.stats().chunks;
    for _ in 0..6 {
        pipelined.write_texture_rows(&tex, 0, &frame);
        let mut enc = device.create_command_encoder(&Default::default());
        pipelined.flush(&mut enc);
        let sub = queue.submit([enc.finish()]);
        if let Some(old) = prev.replace(sub) {
            device.poll(wgpu::PollType::Wait { submission_index: Some(old), timeout: None }).unwrap();
        }
    }
    if let Some(old) = prev.take() {
        device.poll(wgpu::PollType::Wait { submission_index: Some(old), timeout: None }).unwrap();
    }
    assert_eq!(pipelined.stats().chunks, chunks_before_steady, "steady 2-in-flight pipelined stream allocates 0 new chunks/frame");
    let (count, bytes) = kept_oversized(&pipelined);
    assert!(count <= oversized::OVERSIZED_KEEP_CHUNKS && bytes <= oversized::OVERSIZED_KEEP_BYTES);
    assert!(count >= 1 && bytes <= 20 << 20, "pipelined 1080x2400 holds at most 2x10 MiB = 20 MiB (got {count}, {bytes})");
}

/// Oversized pool enforces (2 chunks, 32 MiB) bounds, never keeps > 32 MiB chunks, never lets
/// <= 4 MiB writes steal oversized chunks, and refuses reuse beyond 2x slack.
#[test]
fn oversized_pool_bounds_and_2x_slack_limit() {
    let Some((device, queue)) = gpu() else { return };

    // > 32 MiB is never kept even when repeated.
    let buf_36m = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("staging test 36m"),
        size: 36 << 20,
        usage: wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let data_36m = vec![0x11u8; 36 << 20];
    let st_36m = Staging::new(&device, &queue);
    for _ in 0..3 {
        st_36m.write_buffer(&buf_36m, 0, &data_36m);
        flush_and_wait(&device, &queue, &st_36m);
    }
    assert_eq!(st_36m.stats().chunks, 3, "> 32 MiB writes are always one-shot");
    assert_eq!(kept_oversized(&st_36m), (0, 0));

    // Byte cap: two 20 MiB chunks cannot both stay under 32 MiB, so oldest is evicted.
    let buf_24m = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("staging test 24m"),
        size: 24 << 20,
        usage: wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let st = Staging::new(&device, &queue);
    let data_20m = vec![0x22u8; 20 << 20];
    let data_8m = vec![0x33u8; 8 << 20];
    let small = vec![0x44u8; 4096];

    // Arm pool with one-shot + first recyclable 20 MiB chunk.
    st.write_buffer(&buf_24m, 0, &data_20m);
    flush_and_wait(&device, &queue, &st);
    st.write_buffer(&buf_24m, 0, &data_20m);
    flush_and_wait(&device, &queue, &st);
    assert_eq!(kept_oversized(&st), (1, 20 << 20));

    // Submit two 20 MiB chunks concurrently (both recyclable): when both return, total 40 MiB > 32 MiB
    // so only 1 chunk (20 MiB) stays kept.
    let mut enc1 = device.create_command_encoder(&Default::default());
    st.write_buffer(&buf_24m, 0, &data_20m);
    st.flush(&mut enc1);
    let mut enc2 = device.create_command_encoder(&Default::default());
    st.write_buffer(&buf_24m, 0, &data_20m);
    st.flush(&mut enc2);
    queue.submit([enc1.finish(), enc2.finish()]);
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let (count, bytes) = kept_oversized(&st);
    assert_eq!((count, bytes), (1, 20 << 20), "32 MiB byte cap evicts oldest 20 MiB chunk");

    // Small <= 4 MiB write must NOT steal the 20 MiB oversized chunk.
    let chunks_before = st.stats().chunks;
    st.write_buffer(&buf_24m, 0, &small);
    flush_and_wait(&device, &queue, &st);
    assert_eq!(st.stats().chunks, chunks_before + 1, "small write allocates normal chunk, not oversized");
    assert_eq!(kept_oversized(&st), (1, 20 << 20), "oversized chunk remains untouched by small write");

    // Shrinking stream: 8 MiB write must NOT reuse a 20 MiB chunk (20 > 2 * 8); it allocates an
    // 8 MiB chunk instead, and both (20 + 8 = 28 MiB <= 32 MiB) fit in the pool.
    let chunks_before = st.stats().chunks;
    st.write_buffer(&buf_24m, 0, &data_8m);
    flush_and_wait(&device, &queue, &st);
    assert_eq!(st.stats().chunks, chunks_before + 1, "8 MiB write must not pin 20 MiB chunk (> 2x slack)");
    assert_eq!(kept_oversized(&st), (2, 28 << 20));
}

/// Oversized chunks evict after `OVERSIZED_IDLE_FLUSHES` (60) non-oversized flushes and on
/// `trim_oversized()`, while normal <= 4 MiB chunks stay kept.
#[test]
fn oversized_evicts_after_idle_flushes_and_on_trim_oversized() {
    let Some((device, queue)) = gpu() else { return };
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("staging idle test"),
        size: 10 << 20,
        usage: wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let big = vec![0x77u8; 8 << 20];
    let small = vec![0x88u8; 4096];

    // 1. Flush-count TTL eviction after 60 non-oversized flushes.
    let st = Staging::new(&device, &queue);
    // Prime normal pool (128 KiB) + recurring oversized pool (8 MiB).
    st.write_buffer(&buf, 0, &small);
    flush_and_wait(&device, &queue, &st);
    for _ in 0..2 {
        st.write_buffer(&buf, 0, &big);
        flush_and_wait(&device, &queue, &st);
    }
    assert_eq!(kept_oversized(&st), (1, 8 << 20));
    assert_eq!(kept_normal_bytes(&st), CHUNK);

    for i in 0..oversized::OVERSIZED_IDLE_FLUSHES - 1 {
        st.write_buffer(&buf, 0, &small);
        flush_and_wait(&device, &queue, &st);
        assert_eq!(kept_oversized(&st), (1, 8 << 20), "still kept after {} non-oversized flushes", i + 1);
    }
    // 60th non-oversized flush evicts the oversized chunk while keeping the normal 128 KiB chunk.
    st.write_buffer(&buf, 0, &small);
    flush_and_wait(&device, &queue, &st);
    assert_eq!(kept_oversized(&st), (0, 0), "evicted after OVERSIZED_IDLE_FLUSHES non-oversized flushes");
    assert_eq!(kept_normal_bytes(&st), CHUNK, "normal <= 4 MiB chunk stays kept");

    // Even empty flushes (e.g. Metal Direct path where staging is not busy) age out the pool.
    for _ in 0..2 {
        st.write_buffer(&buf, 0, &big);
        flush_and_wait(&device, &queue, &st);
    }
    assert_eq!(kept_oversized(&st), (1, 8 << 20));
    for _ in 0..oversized::OVERSIZED_IDLE_FLUSHES {
        let mut enc = device.create_command_encoder(&Default::default());
        st.flush(&mut enc);
        drop(enc);
    }
    assert_eq!(kept_oversized(&st), (0, 0), "empty flushes also age out oversized chunks");

    // 2. trim_oversized + has_oversized_in_flight rest flow:
    let st2 = Staging::new(&device, &queue);
    st2.write_buffer(&buf, 0, &small);
    flush_and_wait(&device, &queue, &st2);
    st2.write_buffer(&buf, 0, &big);
    flush_and_wait(&device, &queue, &st2);
    // Second oversized write is recyclable and left submitted without polling yet.
    st2.write_buffer(&buf, 0, &big);
    let mut enc = device.create_command_encoder(&Default::default());
    st2.flush(&mut enc);
    let sub = queue.submit([enc.finish()]);
    assert!(st2.has_oversized_in_flight(), "recyclable oversized chunk reports in-flight before poll");

    device.poll(wgpu::PollType::Wait { submission_index: Some(sub), timeout: None }).unwrap();
    assert!(!st2.has_oversized_in_flight(), "in-flight clears once GPU completes and returns chunk");
    st2.trim_oversized();
    assert_eq!(kept_oversized(&st2), (0, 0), "trim_oversized drops returned and free oversized chunks");
    assert_eq!(kept_normal_bytes(&st2), CHUNK, "trim_oversized keeps normal <= 4 MiB chunks");
}
