//! What `LightRenderer::render` allocates and records, on a real device. Skips when no adapter is found.

use rune_light::{Gi, LightRenderer, Light, Prim, Scene, Settings};

const W: u32 = 256;
const H: u32 = 160;

struct Rig {
    device: wgpu::Device,
    queue: wgpu::Queue,
    light: LightRenderer,
    base: wgpu::TextureView,
    text: wgpu::TextureView,
    target: wgpu::TextureView,
    timed: bool,
}

fn rig() -> Option<Rig> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    let features = adapter.features() & wgpu::Features::TIMESTAMP_QUERY;
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { required_features: features, ..Default::default() })).ok()?;
    let view = |format| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default())
    };
    let (base, text, target) = (view(wgpu::TextureFormat::Rgba16Float), view(wgpu::TextureFormat::Rgba16Float), view(wgpu::TextureFormat::Rgba8Unorm));
    let light = LightRenderer::new(&device, &queue, W, H, wgpu::TextureFormat::Rgba8Unorm);
    Some(Rig { device, queue, light, base, text, target, timed: !features.is_empty() })
}

impl Rig {
    fn frame(&mut self, prims: &[Prim], gi: Option<Gi>) {
        let lights = [Light::point(60.0, 40.0, 120.0)];
        let scene = Scene { prims, lights: &lights, settings: Settings { gi, ..Settings::default() }, emission: None };
        let mut encoder = self.device.create_command_encoder(&Default::default());
        assert!(self.light.render(&self.device, &self.queue, &mut encoder, &scene, &self.base, &self.text, &self.target));
        self.queue.submit([encoder.finish()]);
        self.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    }
}

fn prims(n: usize) -> Vec<Prim> {
    (0..n).map(|i| Prim::rect((i % 16) as f32 * 14.0, (i / 16) as f32 * 8.0, 10.0, 6.0).raised(4.0)).collect()
}

#[test]
fn growing_the_prim_buffer_keeps_the_render_targets() {
    let Some(mut r) = rig() else { return eprintln!("no adapter; skipped") };
    r.frame(&prims(8), None);
    let vis = r.light.visibility_texture().clone();
    r.frame(&prims(300), None);
    assert!(*r.light.visibility_texture() == vis, "a bigger scene reallocated the render targets");
}

#[test]
fn without_gi_nothing_gi_is_allocated_or_recorded() {
    let Some(mut r) = rig() else { return eprintln!("no adapter; skipped") };
    r.frame(&prims(8), None);
    {
        let ins = r.light.inspect();
        assert!(ins.emission == ins.fluence && ins.merged[0] == *ins.fluence, "GI textures exist with GI off");
    }
    // Field and shadows, plus the two timestamp markers when timing.
    assert_eq!(r.light.compute_passes(), if r.timed { 4 } else { 2 });

    r.frame(&prims(8), Some(Gi::default()));
    let ins = r.light.inspect();
    assert!(ins.emission != ins.fluence, "turning GI on made its textures");
    if r.timed {
        assert!(r.light.read_timings(&r.device).is_some_and(|t| t.gi >= 0.0));
    }
}
