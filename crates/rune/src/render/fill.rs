//! Filling atlas masks on the GPU (`text::atlas_fill`): each queued mask is a Windfoil glyph
//! instance drawn into its page, R8, replacing what was there, by the uber shader that draws
//! moving text from outlines, so a mask and its glyph in motion share one coverage function.

use super::*;
use crate::text::QueuedFill;

/// The fill pass's pipeline and what it draws with: the outline store, stand-ins for the atlas
/// and images (a page can't be read while it's drawn into), and a page-sized projection.
pub(super) struct FillPass {
    pipeline: wgpu::RenderPipeline,
    projection: wgpu::Buffer,
    bind: wgpu::BindGroup,
    instances: wgpu::Buffer,
    capacity: usize,
}

/// What a fill pass binds besides its own buffers.
pub(super) struct FillBindings<'a> {
    pub layout: &'a wgpu::BindGroupLayout,
    pub globals: &'a wgpu::Buffer,
    pub outlines: &'a wgpu::Texture,
    pub stand_in: &'a wgpu::Texture,
}

impl FillPass {
    pub(super) fn new(device: &wgpu::Device, layout: &wgpu::PipelineLayout, shader: &dxbc::Shaders, bindings: FillBindings<'_>) -> FillPass {
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("atlas fill"),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: &shader.vertex,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(metallib::uber_vertex_layout())],
            },
            primitive: metallib::uber_primitive(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader.fragment,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::R8Unorm, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let projection = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("atlas fill projection"), size: 16, usage: wgpu::BufferUsages::UNIFORM, mapped_at_creation: true,
        });
        let size = ATLAS_SIZE as f32;
        if let Ok(mut range) = projection.slice(..).get_mapped_range_mut() {
            range.copy_from_slice(bytemuck::cast_slice(&[0.0f32, 0.0, size, size]));
        }
        projection.unmap();
        let bind = Self::bind(device, &bindings, &projection);
        let capacity = 64;
        FillPass { pipeline, projection, bind, instances: Renderer::instance_buffer(device, capacity), capacity }
    }

    fn bind(device: &wgpu::Device, b: &FillBindings<'_>, projection: &wgpu::Buffer) -> wgpu::BindGroup {
        Renderer::bind_group(device, b.layout, b.globals, Some(b.outlines), b.stand_in, b.stand_in, Some(projection))
    }

    /// Binds a new outline texture.
    pub(super) fn rebind(&mut self, device: &wgpu::Device, bindings: FillBindings<'_>) {
        self.bind = Self::bind(device, &bindings, &self.projection);
    }

    /// Writes the instances of `pages`' fills through `staging`, for [`FillPass::encode`]: the
    /// instance range each page draws.
    fn stage(&mut self, device: &wgpu::Device, staging: &rune_staging::Staging, pages: &[(usize, Vec<QueuedFill>)]) -> Vec<(usize, std::ops::Range<u32>)> {
        let count: usize = pages.iter().map(|(_, f)| f.len()).sum();
        if count > self.capacity {
            self.capacity = count.next_power_of_two();
            self.instances = Renderer::instance_buffer(device, self.capacity);
        }
        let instances: Vec<Instance> = pages.iter().flat_map(|(_, fills)| fills.iter().map(instance)).collect();
        staging.write_buffer(&self.instances, 0, bytemuck::cast_slice(&instances));
        let mut first = 0u32;
        pages.iter().map(|(page, fills)| {
            let last = first + fills.len() as u32;
            let range = first..last;
            first = last;
            (*page, range)
        }).collect()
    }

    /// Records the fills `stage` wrote into `encoder`: one pass a page, each drawing into it.
    fn encode(&self, encoder: &mut wgpu::CommandEncoder, pages: &[(&wgpu::Texture, std::ops::Range<u32>)]) {
        for (texture, range) in pages {
            let view = texture.create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("atlas fill"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
            pass.set_bind_group(0, &self.bind, &[]);
            pass.set_vertex_buffer(0, self.instances.slice(..));
            pass.draw(0..4, range.clone());
        }
    }
}

/// Fills staged by `prepare` and not yet recorded: recorded into the frame's encoder where it
/// first records the renderer's uploads (`flush_uploads`), so a frame that places new glyphs
/// submits once, as any frame does; or submitted on their own ahead of a caller's pass
/// (`submit_uploads`). The device and queue are for that.
pub(super) struct PendingFills {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pages: Vec<(usize, std::ops::Range<u32>)>,
}
/// A queued fill as the Windfoil glyph instance that draws it, over its mask and one-texel border
/// (which comes out clear), at zoom 1 with no clip.
fn instance(q: &QueuedFill) -> Instance {
    let [x, y, w, h] = q.rect.map(|v| v as f32);
    let f = q.fill;
    Instance {
        rect: [x - 1.0, y - 1.0, w + 2.0, h + 2.0],
        radii: [x + f.pen[0], y + f.pen[1], f.scale, 1.0],
        fill: [1.0; 4],
        border: [f.record as f32, f.hint as f32, f.radius, f.coverage as f32],
        // A negative exponent marks a fill: the shader rounds the coverage as fill_mask does.
        params: [0.0, KIND_GLYPH, 0.0, -1.0],
        uv: [0.0; 4],
        clip: NO_CLIP,
        fade: [0; 4],
    }
}

impl Renderer {
    /// A glyph atlas page; a Windfoil renderer also draws into its pages (`FillPass`).
    pub(super) fn atlas_texture(device: &wgpu::Device, filled: bool) -> wgpu::Texture {
        // A page the GPU fills can be read back to compare its fills with the CPU's (`fill_drift`).
        let render = if filled { wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC } else { wgpu::TextureUsages::empty() };
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas"),
            size: wgpu::Extent3d { width: ATLAS_SIZE, height: ATLAS_SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | render,
            view_formats: &[],
        })
    }

    /// Stages the atlas's queued fills (after this frame's row uploads, which they draw over), to
    /// be recorded into the frame's encoder after those uploads (`encode_fills`).
    pub(super) fn fill_atlas(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, staging: &rune_staging::Staging, atlas: &mut Atlas) {
        if !matches!(self.uber, variant::Uber::Windfoil { .. }) { return; }
        let fills = atlas.take_fills();
        if cfg!(debug_assertions) && std::env::var_os("RUNE_FILL_STATS").is_some() && fills.iter().any(|(_, f)| !f.is_empty()) {
            let masks: usize = fills.iter().map(|(_, f)| f.len()).sum();
            let texels: u64 = fills.iter().flat_map(|(_, f)| f.iter().map(|q| u64::from(q.rect[2] * q.rect[3]))).sum();
            eprintln!("rune: fill stats: {masks} masks, {texels} texels this frame");
        }
        let fills: Vec<(usize, Vec<QueuedFill>)> = fills.into_iter().filter(|(page, f)| !f.is_empty() && *page <= self.atlas_pages.len()).collect();
        if fills.is_empty() {
            return;
        }
        // A prepare whose fills no draw recorded: they go now, before their instances are
        // written over.
        self.submit_fills();
        let variant::Uber::Windfoil { fill, .. } = &mut self.uber else { return };
        let pages = fill.stage(device, staging, &fills);
        *self.fills.lock().unwrap_or_else(|e| e.into_inner()) = Some(PendingFills { device: device.clone(), queue: queue.clone(), pages });
    }

    /// Records the fills `prepare` staged into `encoder`, after the uploads already recorded there.
    pub(super) fn encode_fills(&self, encoder: &mut wgpu::CommandEncoder) {
        let Some(pending) = self.fills.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
        let variant::Uber::Windfoil { fill, .. } = &self.uber else { return };
        let pages: Vec<(&wgpu::Texture, std::ops::Range<u32>)> = pending.pages.iter().filter_map(|(page, range)| {
            let texture = if *page == 0 { Some(&self.atlas) } else { self.atlas_pages.get(page - 1).map(|p| &p.0) };
            texture.map(|t| (t, range.clone()))
        }).collect();
        fill.encode(encoder, &pages);
    }

    /// Submits the fills `prepare` staged on their own, the uploads ahead of them, for a draw
    /// into a pass its caller opened (`draw_range`), or a prepare no draw followed.
    pub(super) fn submit_fills(&self) {
        let (device, queue) = match &*self.fills.lock().unwrap_or_else(|e| e.into_inner()) {
            Some(p) => (p.device.clone(), p.queue.clone()),
            None => return,
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("atlas fill") });
        self.flush_uploads(&mut encoder);
        queue.submit([encoder.finish()]);
    }
}
/// How far the GPU's atlas fills are from the CPU's (`Renderer::fill_drift`): texels compared,
/// and how many differ by 1, 2, and 3 or more.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default)]
pub struct FillDrift {
    pub masks: usize,
    pub texels: u64,
    pub off_by: [u64; 3],
    pub max: u8,
}

impl Renderer {
    /// Reads every atlas page back and compares each mask the GPU filled with what
    /// `text::outline::fill_mask` gives for it, byte for byte. A diagnostic: it blocks on the GPU.
    #[doc(hidden)]
    #[cfg(not(target_arch = "wasm32"))]
    pub fn fill_drift(&self, device: &wgpu::Device, queue: &wgpu::Queue, atlas: &crate::text::Atlas) -> FillDrift {
        self.submit_uploads();
        let pages: Vec<&wgpu::Texture> = std::iter::once(&self.atlas).chain(self.atlas_pages.iter().map(|(t, _)| t)).collect();
        let read = |texture: &wgpu::Texture| -> Vec<u8> {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("fill drift"), size: u64::from(ATLAS_SIZE * ATLAS_SIZE), usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
            let mut encoder = device.create_command_encoder(&Default::default());
            encoder.copy_texture_to_buffer(texture.as_image_copy(), wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(ATLAS_SIZE), rows_per_image: Some(ATLAS_SIZE) } }, wgpu::Extent3d { width: ATLAS_SIZE, height: ATLAS_SIZE, depth_or_array_layers: 1 });
            queue.submit([encoder.finish()]);
            let slice = buffer.slice(..);
            slice.map_async(wgpu::MapMode::Read, |r| r.expect("map fill drift"));
            device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
            let out = slice.get_mapped_range().expect("mapped").to_vec();
            buffer.unmap();
            out
        };
        let data: Vec<Vec<u8>> = pages.iter().map(|t| read(t)).collect();
        let mut d = FillDrift::default();
        for (page, [x, y, w, h], cpu) in atlas.gpu_fills() {
            let Some(gpu) = data.get(page as usize) else { continue };
            d.masks += 1;
            for row in 0..h {
                for col in 0..w {
                    let g = gpu[((y + row) * ATLAS_SIZE + x + col) as usize];
                    let c = cpu[(row * w + col) as usize];
                    let off = g.abs_diff(c);
                    d.texels += 1;
                    d.max = d.max.max(off);
                    if off > 0 { d.off_by[usize::from(off.min(3)) - 1] += 1; }
                }
            }
        }
        d
    }
}
