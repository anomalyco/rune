use super::bench::{clear, gpu};
use super::encode::MetalBuffer;
use super::*;

#[test]
fn a_frame_is_one_command_buffer_and_writes_land() {
    let (device, queue) = gpu();
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None, size: wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
        dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Bgra8UnormSrgb, usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[],
    });
    let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 16, usage: wgpu::BufferUsages::UNIFORM, mapped_at_creation: false });
    queue.write_buffer(&buffer, 4, &[1, 2, 3, 4]);
    let mapped = buffer.as_custom::<MetalBuffer>().unwrap().get_mapped_range(0..16).unwrap();
    assert_eq!(unsafe { &mapped.read_slice()[4..8] }, &[1, 2, 3, 4]);
    let shared = device.as_custom::<MetalDevice>().unwrap().0.clone();
    let empty = device.create_command_encoder(&Default::default());
    queue.submit([empty.finish()]);
    assert_eq!(shared.committed.load(Ordering::Relaxed), 0, "an empty encoder commits nothing");
    let view = target.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    drop(clear(&mut encoder, &view));
    queue.submit([encoder.finish()]);
    assert_eq!(shared.committed.load(Ordering::Relaxed), 1);
    assert!(shared.event.waitUntilSignaledValue_timeoutMS(1, 5000), "the frame's signal arrives");
}
