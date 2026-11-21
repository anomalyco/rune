//! Clearing the corners a Wayland client-side frame leaves outside its rounded window.
//!
//! Under `sctk-adwaita` the frame draws the window's shadow and its one-point visible border on
//! subsurfaces around the content, and rounds the window's corners while it floats
//! (`vendor/sctk-adwaita`). The content's own surface is a rectangle, so its square corners would
//! show past that border. This fades the pixels outside the arc at the two bottom corners, which
//! the frame's border covers, so the surface composites them fully transparent. The top corners
//! need nothing: the headerbar's own surface covers the content's top edge.
//!
//! The fade multiplies whatever the app drew there, so it assumes the content is opaque in the two
//! corner squares: Rune's root fill is (an app's window is opaque unless it asked to be
//! translucent, and then its own alpha fades with the corners rather than behind them). A pixel the
//! arc crosses keeps its colour and loses the same share of its alpha, which is the frame's ring's
//! own antialiasing, so the two meet without a seam. On the sRGB view `srgb_blend(false)` draws
//! into, the one-pixel fade's encoded colour can sit a step above what its alpha alone would say:
//! the blend is linear while the stored value is encoded, the same rounding Rune's own blending
//! takes there.

/// A pipeline that fades out what is outside a rounded corner's arc, and leaves everything inside
/// it. It multiplies the target by the arc's coverage, so a pixel the arc crosses keeps exactly its
/// share of the frame's antialiased ring: no jagged seam between them. Only each corner's square is
/// drawn at all, and every other pixel of the target keeps what it had.
///
/// The multiply is a blend, and a blend into an sRGB view happens in linear light — the hardware
/// decodes the stored pixels, blends, and encodes the result — which is the space Rune's own
/// blending there works in. The linear view `srgb_blend(true)` draws into needs no conversion at
/// all, so the fade is the same on both; a fully faded pixel is clear either way.
pub(crate) struct CsdCorners {
    pipeline: wgpu::RenderPipeline,
    format: wgpu::TextureFormat,
    vertices: wgpu::Buffer,
    /// The radius, in device pixels, and target size `vertices` were written for.
    prepared: Option<(f32, (u32, u32))>,
}

impl CsdCorners {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rounded corners"),
            source: wgpu::ShaderSource::Wgsl(
                r#"
// `centre` is the corner arc's centre in device pixels, flat across the corner's square, and
// `radius` is the arc's radius in device pixels, so the fade is one pixel wide whatever the scale.
// Measuring from `@builtin(position).xy` uses the rasterizer's exact pixel centre instead of an
// interpolated quad coordinate that can drift by subpixel ULPs on D3D12.
struct Vertex {
    @builtin(position) at: vec4<f32>,
    @location(0) @interpolate(flat) centre: vec2<f32>,
    @location(1) @interpolate(flat) radius: f32,
};

@vertex fn vs(@location(0) at: vec2<f32>, @location(1) centre: vec2<f32>, @location(2) radius: f32) -> Vertex {
    return Vertex(vec4(at, 0.0, 1.0), centre, radius);
}

@fragment fn fs(frag: Vertex) -> @location(0) vec4<f32> {
    // How much of this pixel the arc leaves: one inside the content's corner, none outside it, and
    // a pixel's width of fade across the arc itself. `a` is what is cleared, so the target is
    // multiplied by `1 - a`.
    let distance = length(frag.at.xy - frag.centre);
    let outside = smoothstep(frag.radius - 0.5, frag.radius + 0.5, distance);
    if outside <= 0.0 {
        discard;
    }
    return vec4(0.0, 0.0, 0.0, outside);
}
"#
                .into(),
            ),
        });
        // A vertex is its clip-space position, its corner arc's centre in device pixels, and the
        // arc's radius.
        let vertex = std::mem::size_of::<f32>() as wgpu::BufferAddress;
        let stride = 5 * vertex;
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rounded corners"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: stride,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 0, shader_location: 0 },
                        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 2 * vertex, shader_location: 1 },
                        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32, offset: 4 * vertex, shader_location: 2 },
                    ],
                })],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // The target's colour and alpha both times the arc's coverage: what the
                    // fragment keeps, and nothing where it is outside the arc.
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::Zero,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::Zero,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rounded corners"),
            // Two corners, two triangles each, five values a vertex.
            size: 12 * stride,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { pipeline, format, vertices, prepared: None }
    }

    /// The format it draws into: a surface that changed its format needs another pass.
    pub fn format(&self) -> wgpu::TextureFormat { self.format }

    /// Whether it already clears `size` with arcs of `radius` device pixels.
    pub fn is_prepared_for(&self, size: (u32, u32), radius: f32) -> bool { self.prepared == Some((radius, size)) }

    /// Writes the two bottom corner squares of a `size` target, whose arcs have `radius` device
    /// pixels. A radius of zero draws nothing: production never asks for one (`window::surface::corners`).
    pub fn prepare(&mut self, queue: &wgpu::Queue, size: (u32, u32), radius: f32) {
        if self.is_prepared_for(size, radius) || radius <= 0. { return; }
        let (width, height) = (size.0.max(1) as f32, size.1.max(1) as f32);
        let mut vertices: Vec<f32> = Vec::with_capacity(12 * 5);
        // Each corner's square runs from the arc's centre out to the window's edges, a radius
        // either way; the bottom two sit a radius in from the foot's sides and off its edge, one
        // reaching left and the other right.
        for (centre, side) in [((radius, height - radius), -1.), ((width - radius, height - radius), 1.)] {
            for (u, v) in [(0., 0.), (1., 0.), (0., 1.), (1., 0.), (1., 1.), (0., 1.)] {
                let (x, y) = (centre.0 + side * u * radius, centre.1 + v * radius);
                vertices.extend_from_slice(&[
                    x / width * 2.0 - 1.0,
                    1.0 - y / height * 2.0,
                    centre.0,
                    centre.1,
                    radius,
                ]);
            }
        }
        queue.write_buffer(&self.vertices, 0, bytemuck::cast_slice(&vertices));
        self.prepared = Some((radius, size));
    }

    /// Fades out what lies outside the arcs, leaving every other pixel of `target` as it was
    /// drawn: it is a premultiplied composition, before a straight-alpha surface converts it.
    pub fn draw(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        if self.prepared.is_none() {
            return;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("rounded corners"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.draw(0..12, 0..1);
    }
}

#[cfg(test)]
#[path = "csd_corners_tests.rs"]
mod tests;
