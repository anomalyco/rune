//! Convert Rune's premultiplied composition to the straight alpha required by
//! wgpu's straight-alpha surfaces and PNG exports. Metal's CAMetalLayer stays premultiplied.
/// The pass's shader (`surface_alpha.wgsl`).
pub(crate) const WGSL: &str = super::uber_wgsl::SURFACE_ALPHA;

/// Render targets of `*Srgb` formats hold sRGB-encoded premultiplied linear colour. PNG uses
/// straight sRGB colour, so un-premultiply in linear light, not in the encoded byte domain.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn straight_alpha(pixels: &mut [u8]) {
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = f32::from(pixel[3]) / 255.0;
        if alpha == 0.0 {
            pixel[..3].fill(0);
        } else if alpha < 1.0 {
            for c in &mut pixel[..3] {
                *c = (crate::color::srgb_encode((crate::color::srgb_decode(f32::from(*c) / 255.0) / alpha).clamp(0.0, 1.0)) * 255.0).round() as u8;
            }
        }
    }
}

pub(crate) struct AlphaSurface {
    pub view: wgpu::TextureView,
    // Keeps the backing texture alive for view/bind; resize() reads its extent on non-wasm32.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    texture: wgpu::Texture,
    pipeline: wgpu::RenderPipeline,
    #[cfg(not(target_arch = "wasm32"))]
    layout: wgpu::BindGroupLayout,
    bind: wgpu::BindGroup,
    #[cfg(not(target_arch = "wasm32"))]
    format: wgpu::TextureFormat,
}

#[cfg(test)]
mod tests {
    #[test]
    fn gpu_straight_alpha_matches_cpu_reference() {
        use crate::render::{Gpu, OFFSCREEN_FORMAT_LINEAR, Offscreen};
        let Some(gpu) = Gpu::test_headless() else { return };
        let alpha = super::AlphaSurface::new(&gpu.device, OFFSCREEN_FORMAT_LINEAR, (4, 4));
        let target = Offscreen::with_format(&gpu.device, 4, 4, OFFSCREEN_FORMAT_LINEAR);
        for color in [[0.125_f32, 0.25, 0.375, 0.5], [0.0; 4], [0.4, 0.6, 0.1, 1.0]] {
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            {
                let _pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None, color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &alpha.view, depth_slice: None, resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: color[0] as f64, g: color[1] as f64, b: color[2] as f64, a: color[3] as f64,
                        }), store: wgpu::StoreOp::Store },
                    })], depth_stencil_attachment: None, timestamp_writes: None,
                    occlusion_query_set: None, multiview_mask: None,
                });
            }
            alpha.draw(&mut enc, &target.view);
            target.copy(&mut enc);
            gpu.queue.submit([enc.finish()]);
            let pixels = target.read(&gpu.device);
            let mut reference = [0_u8; 4];
            for i in 0..3 { reference[i] = (crate::color::srgb_encode(color[i]) * 255.0).round() as u8; }
            reference[3] = (color[3] * 255.0).round() as u8;
            crate::tooling::straight_alpha(&mut reference);
            for pixel in pixels.chunks_exact(4) {
                assert!(pixel.iter().zip(reference).all(|(a, b)| a.abs_diff(b) <= 1), "GPU {pixel:?}, CPU {reference:?}");
            }
        }
    }
}
impl AlphaSurface {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, size: (u32, u32)) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("alpha source"), entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("alpha conversion"), bind_group_layouts: &[Some(&layout)], immediate_size: 0,
        });
        let make = |shaders: &super::dxbc::Shaders| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("straight alpha"), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shaders.vertex, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(),
            fragment: Some(wgpu::FragmentState { module: &shaders.fragment, entry_point: Some("fs"), compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
            multiview_mask: None, cache: None,
        });
        // DX12 takes the bytecode regen_dxbc compiled from this WGSL; elsewhere, or when it's stale
        // or refused, the WGSL compiles here.
        #[cfg(target_os = "windows")]
        let precompiled = super::dxbc::surface_alpha(device, WGSL, &make);
        #[cfg(not(target_os = "windows"))]
        let precompiled: Option<(super::dxbc::Shaders, wgpu::RenderPipeline)> = None;
        let pipeline = precompiled.map_or_else(|| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("straight alpha"), source: wgpu::ShaderSource::Wgsl(WGSL.into()) });
            make(&super::dxbc::Shaders::one(module))
        }, |(_, pipeline)| pipeline);
        let (texture, view, bind) = Self::target(device, &layout, format, size);
        Self {
            texture,
            view,
            bind,
            pipeline,
            #[cfg(not(target_arch = "wasm32"))]
            layout,
            #[cfg(not(target_arch = "wasm32"))]
            format,
        }
    }
    fn target(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat, size: (u32, u32))
        -> (wgpu::Texture, wgpu::TextureView, wgpu::BindGroup) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("premultiplied composition"),
            size: wgpu::Extent3d { width: size.0.max(1), height: size.1.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("alpha source"), layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }],
        });
        (texture, view, bind)
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn resize(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if (self.texture.width(), self.texture.height()) == (size.0.max(1), size.1.max(1)) { return; }
        (self.texture, self.view, self.bind) = Self::target(device, &self.layout, self.format, size);
    }
    pub fn draw(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("straight alpha"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target, depth_slice: None, resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
            })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.draw(0..3, 0..1);
    }
}
