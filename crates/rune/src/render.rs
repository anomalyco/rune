//! The instanced wgpu renderer, with lazy bounded group layers and offscreen readback.

use bytemuck::{Pod, Zeroable};

use crate::pixels::ImageAtlas;
use crate::text::{ATLAS_SIZE, Atlas};

#[path = "layers.rs"]
mod layers;
pub use layers::LayerStats;

pub const KIND_RECT: f32 = 0.0;
pub const KIND_SHADOW: f32 = 1.0;
pub const KIND_GLYPH: f32 = 2.0;
/// Inset shadow inside a rounded rect: `params.x` spread, `params.z` sigma, `uv.xy` offset.
pub const KIND_INSET: f32 = 3.0;
/// Rounded rect with a vertical gradient: `fill` at the top, `uv` (premultiplied) at the bottom.
pub const KIND_GRADIENT: f32 = 4.0;
/// A stroke segment: `radii` = x0, y0, x1, y1; `params.x` half width, `params.z` 1 for round caps.
pub const KIND_SEGMENT: f32 = 5.0;
/// A filled triangle: `radii` = ax, ay, bx, by; `uv.xy` = cx, cy; `params.x` the antialiased-edge
/// mask (a→b, b→c, c→a as bits 0, 1, 2; other edges are hard so neighbours meet without seams).
pub const KIND_TRIANGLE: f32 = 6.0;
/// An RGBA image (`El::pixels`): `uv.xy` its origin in the image atlas, `uv.zw` its size in
/// texels, drawn scaled into `rect` with the element's corner `radii`; `fill.a` is the opacity.
pub const KIND_IMAGE: f32 = 7.0;
/// Balanced scope records in the painter stream. They occupy instance indices but are never
/// rasterized as ordinary quads. Keep scopes intact when partitioning compositor lists.
pub const KIND_LAYER: f32 = 8.0;
pub const KIND_LAYER_END: f32 = 9.0;

/// Composition properties of an isolated subtree. Filters can extend this contract without
/// changing painter-stream scopes or the target pool.
#[derive(Clone, Copy, Debug)]
pub struct LayerSpec {
    pub opacity: f32,
}

impl Default for LayerSpec {
    fn default() -> Self { Self { opacity: 1.0 } }
}

/// One instance, in device pixels.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Instance {
    pub rect: [f32; 4],
    pub radii: [f32; 4],
    pub fill: [f32; 4],
    pub border: [f32; 4],
    /// border width, kind, shadow sigma, glyph coverage exponent
    pub params: [f32; 4],
    pub uv: [f32; 4],
    /// x0, y0, x1, y1
    pub clip: [f32; 4],
    /// left, top, right, bottom fade widths
    pub fade: [f32; 4],
}

pub const NO_CLIP: [f32; 4] = [-1e6, -1e6, 1e6, 1e6];

impl Instance {
    /// Begin an isolated scope. `bounds` is a device-pixel [x,y,w,h] content bound, including
    /// native surfaces. Primitive bounds (including shadow support) expand it during prepare.
    pub fn begin_layer(bounds: [f32; 4], clip: [f32; 4], spec: LayerSpec) -> Self {
        Self { rect: bounds, clip, fill: [0.0, 0.0, 0.0, spec.opacity.clamp(0.0, 1.0)],
            params: [0.0, KIND_LAYER, 0.0, 0.0], ..Self::zeroed() }
    }

    pub fn end_layer() -> Self {
        Self { params: [0.0, KIND_LAYER_END, 0.0, 0.0], ..Self::zeroed() }
    }
}

/// Close a runtime scope, folding empty/single-draw groups back into the direct path. Only the
/// last one or two records move, so video insertions before this scope keep their indices.
pub(crate) fn end_layer(instances: &mut Vec<Instance>, surfaces: &mut [crate::native_surface::SurfaceDraw], marks: &mut Vec<u32>, start: usize, surface_start: usize) {
    let primitives = instances.len() - start - 1;
    let videos = surfaces.len() - surface_start;
    if primitives + videos <= 1 {
        let opacity = instances[start].fill[3];
        if primitives == 1 {
            let i = &mut instances[start + 1];
            i.fill = i.fill.map(|v| v * opacity);
            i.border = i.border.map(|v| v * opacity);
            if i.params[1] == KIND_GRADIENT { i.uv = i.uv.map(|v| v * opacity); }
        }
        if videos == 1 {
            surfaces[surface_start].opacity *= opacity;
            surfaces[surface_start].before_instance -= 1;
        }
        instances.remove(start);
        if marks.len() > start { marks.remove(start); }
    } else {
        instances.push(Instance::end_layer());
        if let Some(&mark) = marks.get(start) { marks.push(mark); }
    }
}

pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

/// Vulkan loaders crash when two threads create instances or devices at once (seen with
/// parallel GPU tests on lavapipe), so Rune creates them one at a time.
static GPU_INIT: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl Gpu {
    /// Panics without a usable adapter or device; windows use `try_new`.
    pub fn new(instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>) -> Gpu {
        Self::try_new(instance, surface).unwrap_or_else(|error| panic!("{error}"))
    }

    pub fn try_new(instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>) -> Result<Gpu, String> {
        Self::try_with_features(instance, surface, wgpu::Features::empty())
    }

    /// Like `try_new`, also enabling whichever of `wanted` the adapter supports.
    pub fn try_with_features(instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>, wanted: wgpu::Features) -> Result<Gpu, String> {
        let _serial = GPU_INIT.lock().unwrap_or_else(|e| e.into_inner());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: false,
            compatible_surface: surface,
            apply_limit_buckets: false,
        }))
        .map_err(|error| format!("no GPU adapter: {error}"))?;
        let required_features = adapter.features() & (wgpu::Features::TIMESTAMP_QUERY | wanted);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("rune"),
            required_features,
            required_limits: wgpu::Limits::default(),
            experimental_features: Default::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: Default::default(),
        }))
        .map_err(|error| format!("no GPU device: {error}"))?;
        Ok(Gpu { instance, adapter, device, queue })
    }

    pub fn headless() -> Gpu {
        Self::new(Self::instance(), None)
    }

    pub fn instance() -> wgpu::Instance {
        let _serial = GPU_INIT.lock().unwrap_or_else(|e| e.into_inner());
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle())
    }
}

pub struct Renderer {
    surfaces: Option<crate::native_surface::SurfaceRenderer>,
    pipeline: wgpu::RenderPipeline,
    format: wgpu::TextureFormat,
    shader: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    vertex_entry: &'static str,
    /// Pipelines for other target formats a compositor draws into (`add_format`).
    extra: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    bind_group: wgpu::BindGroup,
    bind_layout: wgpu::BindGroupLayout,
    globals: wgpu::Buffer,
    atlas: wgpu::Texture,
    /// Stable image pages; the initial bind group uses a 1×1 stand-in.
    images: Vec<Option<(wgpu::Texture, wgpu::BindGroup)>>,
    image_batches: Vec<(std::ops::Range<u32>, usize)>,
    instances: wgpu::Buffer,
    capacity: usize,
    count: u32,
    springs: Option<crate::gpu_springs::Bindings>,
    spring_instances: usize,
    layers: layers::Layers,
}

impl Renderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Renderer {
        Self::create(device, format, None)
    }

    /// Opt-in retained geometry. IDs map instances to shared translation/opacity nodes.
    /// Upload geometry once with `prepare`, then use `present_at` without rebuilding instances.
    pub fn with_springs(device: &wgpu::Device, format: wgpu::TextureFormat, nodes: &[crate::gpu_springs::Node], ids: &[u32]) -> Renderer {
        Self::create(device, format, Some((nodes, ids)))
    }

    fn create(device: &wgpu::Device, format: wgpu::TextureFormat, properties: Option<(&[crate::gpu_springs::Node], &[u32])>) -> Renderer {
        let source = if properties.is_some() {
            format!("{}\n{}", include_str!("shader.wgsl"), crate::gpu_springs::SHADER)
        } else { include_str!("shader.wgsl").to_owned() };
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("uber"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
            ],
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(device.limits().min_uniform_buffer_offset_alignment) + 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas"),
            size: wgpu::Extent3d { width: ATLAS_SIZE, height: ATLAS_SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let stand_in = Self::image_texture(device, 1, 1);
        let bind_group = Self::bind_group(device, &bgl, &globals, &atlas, &stand_in, None);
        let spring_layout = properties.map(|_| crate::gpu_springs::Bindings::layout(device));
        let layouts = [Some(&bgl), spring_layout.as_ref()];
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &layouts[..if properties.is_some() { 2 } else { 1 }],
            immediate_size: 0,
        });
        let vertex_entry = if properties.is_some() { "vs_springs" } else { "vs" };
        let pipeline = Self::pipeline(device, &layout, &shader, format, vertex_entry);
        let capacity = 256;
        let instances = Self::instance_buffer(device, capacity);
        let spring_instances = properties.map_or(0, |(_, ids)| ids.len());
        let springs = properties.map(|(nodes, ids)| crate::gpu_springs::Bindings::new(device, spring_layout.as_ref().unwrap(), nodes, ids));
        Renderer {
            surfaces: None,
            pipeline,
            format,
            shader,
            layout,
            vertex_entry,
            extra: Vec::new(),
            bind_group,
            bind_layout: bgl,
            globals,
            atlas,
            images: Vec::new(),
            image_batches: Vec::new(),
            instances,
            capacity,
            count: 0,
            springs,
            spring_instances,
            layers: Default::default(),
        }
    }

    fn pipeline(
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        shader: &wgpu::ShaderModule,
        format: wgpu::TextureFormat,
        vertex_entry: &str,
    ) -> wgpu::RenderPipeline {
        let attrs = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4,
            4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4
        ];
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("uber"),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some(vertex_entry),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attrs,
                })],
            },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    }

    fn image_texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("image atlas"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    fn bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        globals: &wgpu::Buffer,
        atlas: &wgpu::Texture,
        images: &wgpu::Texture,
        target: Option<&wgpu::Buffer>,
    ) -> wgpu::BindGroup {
        let atlas_view = atlas.create_view(&Default::default());
        let image_view = images.create_view(&Default::default());
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&atlas_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&image_view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: target.unwrap_or(globals),
                    offset: if target.is_some() { 0 } else { u64::from(device.limits().min_uniform_buffer_offset_alignment) },
                    size: std::num::NonZeroU64::new(16),
                }) },
            ],
        })
    }

    /// Uploads changed rows of the runtime's image atlas (`El::pixels`), creating its texture on
    /// first use.
    pub fn upload_images(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, images: &mut ImageAtlas) {
        let mut changed = self.images.len() != images.pages.len();
        self.images.resize_with(images.pages.len(), || None);
        for (index, page) in images.pages.iter_mut().enumerate() {
            if page.data.is_empty() { changed |= self.images[index].is_some(); self.images[index] = None; continue; }
            if self.images[index].as_ref().is_none_or(|(t, _)| t.width() != page.width || t.height() != page.height) {
                let texture = Self::image_texture(device, page.width, page.height);
                let bind = Self::bind_group(device, &self.bind_layout, &self.globals, &self.atlas, &texture, None);
                self.images[index] = Some((texture, bind));
                changed = true;
                page.dirty = Some((0, page.height));
            }
            let Some((y0, y1)) = page.dirty.take() else { continue };
            let row = page.width * 4;
            queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.images[index].as_ref().unwrap().0,
                mip_level: 0,
                origin: wgpu::Origin3d { x: 0, y: y0, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &page.data[(y0 * row) as usize..(y1 * row) as usize],
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None },
            wgpu::Extent3d { width: page.width, height: y1 - y0, depth_or_array_layers: 1 },
        );
        }
        if changed { self.images_changed(); }
        self.refresh_layer_bindings(device);
    }

    /// Builds the pipeline for another target format (e.g. an `Rgba16Float` layer), once.
    pub fn add_format(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        if format != self.format && !self.extra.iter().any(|(f, _)| *f == format) {
            let p = Self::pipeline(device, &self.layout, &self.shader, format, self.vertex_entry);
            self.extra.push((format, p));
        }
        if self.layer_stats().targets > 0 { self.add_layer_format(device, format); }
        if let Some(s) = &mut self.surfaces { s.add_format(device, format); }
    }

    /// Uploads several instance lists back to back (one buffer, one glyph upload) and returns
    /// each list's instance range for `draw_range`. A compositor's version of `prepare`.
    pub fn prepare_lists(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        atlas: &mut Atlas,
        lists: &[&[Instance]],
        viewport: (u32, u32),
    ) -> Vec<std::ops::Range<u32>> {
        let mut all = Vec::with_capacity(lists.iter().map(|l| l.len()).sum());
        let mut ranges = Vec::with_capacity(lists.len());
        for l in lists {
            let a = all.len() as u32;
            all.extend_from_slice(l);
            ranges.push(a..all.len() as u32);
        }
        self.prepare(device, queue, atlas, &all, viewport);
        ranges
    }

    /// Draws uploaded instances `range` into a pass whose target has `format` (the renderer's
    /// own, or one added with `add_format`). Call `encode_layers` before opening the pass.
    /// Scopes are atomic: list/range boundaries must not bisect a layer.
    pub fn draw_range(&self, pass: &mut wgpu::RenderPass<'_>, format: wgpu::TextureFormat, range: std::ops::Range<u32>) {
        if range.is_empty() && range.end != self.count { return; }
        self.validate_layer_range(&range);
        self.draw_layer_range(pass, format, range, None);
    }

    fn draw_instances(&self, pass: &mut wgpu::RenderPass<'_>, format: wgpu::TextureFormat, range: std::ops::Range<u32>, target: Option<usize>) {
        if range.is_empty() {
            return;
        }
        let pipeline = if format == self.format {
            &self.pipeline
        } else {
            &self.extra.iter().find(|(f, _)| *f == format).expect("Renderer::add_format first").1
        };
        pass.set_pipeline(pipeline);
        if let Some(properties) = &self.springs { pass.set_bind_group(1, &properties.group, &[]); }
        pass.set_vertex_buffer(0, self.instances.slice(..));
        let start = self.image_batches.partition_point(|(batch, _)| batch.end <= range.start);
        for (batch, page) in self.image_batches[start..].iter().take_while(|(batch, _)| batch.start < range.end) {
            let bind = if let Some(index) = target {
                let t = self.layers.targets[index].as_ref().unwrap();
                t.images.get(*page).and_then(Option::as_ref).unwrap_or(&t.base)
            } else { self.images.get(*page).and_then(Option::as_ref).map_or(&self.bind_group, |(_, bind)| bind) };
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..4, batch.start.max(range.start)..batch.end.min(range.end));
        }
    }

    fn instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instances"),
            size: (capacity * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Uploads instances, globals and any new glyphs.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        atlas: &mut Atlas,
        instances: &[Instance],
        viewport: (u32, u32),
    ) {
        if self.springs.is_some() { assert_eq!(instances.len(), self.spring_instances); }
        if let Some((y0, y1)) = atlas.dirty.take() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.atlas,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: y0, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                &atlas.data[(y0 * ATLAS_SIZE) as usize..(y1 * ATLAS_SIZE) as usize],
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(ATLAS_SIZE), rows_per_image: None },
                wgpu::Extent3d { width: ATLAS_SIZE, height: y1 - y0, depth_or_array_layers: 1 },
            );
        }
        if instances.len() > self.capacity {
            self.capacity = instances.len().next_power_of_two();
            self.instances = Self::instance_buffer(device, self.capacity);
        }
        queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(instances));
        // Geometry uploads must preserve a presentation timestamp already latched this frame.
        let g: [f32; 2] = [viewport.0 as f32, viewport.1 as f32];
        queue.write_buffer(&self.globals, 0, bytemuck::cast_slice(&g));
        self.count = instances.len() as u32;
        self.image_batches.clear();
        let (mut start, mut page) = (0, 0);
        let mut has_layers = false;
        for (i, instance) in instances.iter().enumerate() {
            has_layers |= instance.params[1] == KIND_LAYER;
            if instance.params[1] == KIND_IMAGE && instance.params[0] as usize != page {
                if start < i as u32 { self.image_batches.push((start..i as u32, page)); }
                start = i as u32;
                page = instance.params[0] as usize;
            }
        }
        if start < self.count { self.image_batches.push((start..self.count, page)); }
        self.prepare_layers(device, queue, if has_layers { instances } else { &[] }, viewport);
    }

    pub fn prepare_surfaces(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, draws: &[crate::native_surface::SurfaceDraw], viewport: (u32,u32)) {
        if !draws.is_empty() && self.surfaces.is_none() {
            let mut surfaces = crate::native_surface::SurfaceRenderer::new(device, self.format);
            for (format, _) in &self.extra { surfaces.add_format(device, *format); }
            self.surfaces = Some(surfaces);
        }
        if let Some(s) = &mut self.surfaces { s.prepare(device,queue,draws,viewport); }
        if self.layer_stats().targets > 0 { self.position_layer_surfaces(queue); }
    }

    pub fn submit(&self, queue: &wgpu::Queue, encoder: wgpu::CommandEncoder) -> wgpu::SubmissionIndex {
        if let Some(s) = &self.surfaces { s.submit(queue,encoder) } else { queue.submit([encoder.finish()]) }
    }

    pub fn draw(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, clear: [f32; 4]) {
        self.draw_timed(encoder, view, clear, None);
    }

    /// One uniform write per presentation-only frame; use the display's target time.
    pub fn present_at(&self, queue: &wgpu::Queue, viewport: (u32, u32), time: f64) {
        let t = crate::gpu_springs::split_time(time);
        queue.write_buffer(&self.globals, 0, bytemuck::cast_slice(&[viewport.0 as f32, viewport.1 as f32, t[0], t[1]]));
    }

    /// A retarget touches one node record; caller computes inherited position/velocity on CPU.
    pub fn write_spring_node(&mut self, queue: &wgpu::Queue, id: usize, node: &crate::gpu_springs::Node) {
        let properties = self.springs.as_mut().expect("renderer has no GPU springs");
        assert!(id < properties.count);
        properties.deadlines[id] = node.settle_at();
        properties.settle_at = properties.deadlines.iter().copied().fold(0.0, f64::max);
        queue.write_buffer(&properties.nodes, (id * std::mem::size_of::<crate::gpu_springs::Node>()) as u64, bytemuck::bytes_of(node));
    }

    /// Constant-time scheduler check. Present the terminal frame, then stop the display link.
    /// Deadlines are recomputed only on commits, so no per-node CPU sampling occurs on ticks.
    pub fn springs_animating(&self, time: f64) -> bool {
        self.springs.as_ref().is_some_and(|p| time < p.settle_at)
    }

    pub fn draw_timed(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, clear: [f32; 4], timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>) {
        let timestamps = timestamps.or_else(|| self.surfaces.as_ref().and_then(|s| s.timestamp_writes()));
        self.encode_layers_timed(encoder, timestamps.as_ref());
        let timestamps = timestamps.map(|mut t| {
            if self.layer_stats().extra_passes > 0 { t.beginning_of_pass_write_index = None; }
            t
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("frame"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: clear[0] as f64,
                        g: clear[1] as f64,
                        b: clear[2] as f64,
                        a: clear[3] as f64,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: timestamps,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        self.draw_range(&mut pass, self.format, 0..self.count);
    }
}

/// An offscreen sRGB target with CPU readback.
pub struct Offscreen {
    texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    buffer: wgpu::Buffer,
    pub width: u32,
    pub height: u32,
    padded_row: u32,
}

pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

impl Offscreen {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Offscreen {
        Self::with_format(device, width, height, OFFSCREEN_FORMAT)
    }

    pub fn with_format(device: &wgpu::Device, width: u32, height: u32, format: wgpu::TextureFormat) -> Offscreen {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let padded_row = (width * 4).div_ceil(256) * 256;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (padded_row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Offscreen { texture, view, buffer, width, height, padded_row }
    }

    pub fn copy(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );
    }

    /// Tightly packed sRGB RGBA8 after `copy` was submitted.
    pub fn read(&self, device: &wgpu::Device) -> Vec<u8> {
        let slice = self.buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback"));
        device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        let mut out = Vec::with_capacity((self.width * self.height * 4) as usize);
        {
            let data = slice.get_mapped_range().expect("mapped range");
            for row in 0..self.height {
                let start = (row * self.padded_row) as usize;
                out.extend_from_slice(&data[start..start + (self.width * 4) as usize]);
            }
        }
        self.buffer.unmap();
        out
    }
}

pub fn write_png(path: &std::path::Path, width: u32, height: u32, rgba: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    let mut w = enc.write_header().map_err(std::io::Error::other)?;
    w.write_image_data(rgba).map_err(std::io::Error::other)?;
    Ok(())
}
