//! research/30's HUD frame through wgpu: acquire, `WRITES` 256-byte `Queue::write_buffer`s, one
//! pass that clears the drawable, submit, present. `BACKEND=vulkan|dx12` (default: the
//! platform's).

#[path = "../count.rs"]
mod count;
#[path = "../harness.rs"]
mod harness;

use std::sync::Arc;

use harness::{Draw, env};
use winit::window::Window;

struct Gpu {
    _window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    buffer: wgpu::Buffer,
    writes: usize,
    frame: u64,
}

impl Gpu {
    fn new(window: Arc<Window>) -> Gpu {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = match std::env::var("BACKEND").as_deref() {
            Ok("dx12") => wgpu::Backends::DX12,
            Ok("vulkan") => wgpu::Backends::VULKAN,
            _ => wgpu::Backends::PRIMARY,
        };
        desc.flags.remove(wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL);
        let instance = wgpu::Instance::new(desc);
        let surface = instance.create_surface(window.clone()).expect("surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .expect("adapter");
        eprintln!("adapter: {:?}", adapter.get_info());
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("device");
        let size = window.inner_size();
        let mut config = surface.get_default_config(&adapter, size.width.max(1), size.height.max(1)).expect("config");
        config.present_mode = wgpu::PresentMode::AutoVsync;
        config.desired_maximum_frame_latency = 2;
        surface.configure(&device, &config);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hud uniforms"),
            size: 256,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Gpu { _window: window, surface, device, queue, buffer, writes: env("WRITES", 0), frame: 0 }
    }
}

impl Draw for Gpu {
    fn frame(&mut self) {
        self.frame += 1;
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            other => panic!("no drawable: {other:?}"),
        };
        let view = frame.texture.create_view(&Default::default());
        let phase = (self.frame % 120) as f32 / 120.0;
        let data = [phase.to_bits(); 64];
        for _ in 0..self.writes {
            // SAFETY: u32 has no padding.
            let bytes = unsafe { std::slice::from_raw_parts(data.as_ptr().cast::<u8>(), 256) };
            self.queue.write_buffer(&self.buffer, 0, bytes);
        }
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hud"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.0, b: phase as f64 * 0.4, a: 0.4 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
    }
}

fn main() {
    harness::run("wgpu_frame", |window| Box::new(Gpu::new(window)));
}
