//! The compute renderer and the compositor that paints it at the effect slot.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rune::compose::{Compositor, DeviceKey, Frame, GpuHandle};

/// The effect slot's mark and size in points.
pub const SLOT: u32 = 1;
pub const WIDTH: f32 = 320.0;
pub const HEIGHT: f32 = 200.0;

/// The texture the compute shader draws, in pixels: the slot at 2x.
const SIZE: [u32; 2] = [640, 400];

const COMPUTE: &str = r#"
struct Params { size: vec2<u32>, time: f32, pad: f32 }
@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read_write> counts: array<u32, 3>;
@group(0) @binding(2) var image: texture_storage_2d<rgba8unorm, write>;

// Writes the workgroup counts `draw` runs with: the GPU sizes the work.
@compute @workgroup_size(1)
fn size() {
    counts[0] = (params.size.x + 7u) / 8u;
    counts[1] = (params.size.y + 7u) / 8u;
    counts[2] = 1u;
}

@compute @workgroup_size(8, 8)
fn draw(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= params.size) { return; }
    let p = vec2<f32>(id.xy) / f32(params.size.y);
    let t = params.time;
    let v = sin(p.x * 9.0 + t) + sin((p.y * 7.0 - t) * 1.3) + sin((p.x + p.y) * 6.0 + t * 0.7);
    let c = 0.5 + 0.5 * cos(vec3<f32>(0.0, 2.1, 4.2) + v * 1.7);
    textureStore(image, vec2<i32>(id.xy), vec4<f32>(c, 1.0));
}
"#;

const PAINT: &str = r#"
// Where the slot is in the pass's target, in pixels, and whether to write sRGB-encoded colours.
struct Place { rect: vec4<f32>, viewport: vec2<f32>, encoded: f32, pad: f32 }
var<immediate> place: Place;
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var filtering: sampler;

struct Out { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> }

@vertex
fn vs(@builtin(vertex_index) i: u32) -> Out {
    let corner = vec2<f32>(f32(i & 1u), f32(i >> 1u));
    let at = (place.rect.xy + corner * place.rect.zw) / place.viewport * 2.0 - 1.0;
    return Out(vec4<f32>(at.x, -at.y, 0.0, 1.0), corner);
}

@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    let c = textureSample(image, filtering, in.uv).rgb;
    // The pattern is sRGB-encoded; a linear frame's target encodes on write.
    return vec4<f32>(select(pow(c, vec3<f32>(2.2)), c, place.encoded > 0.5), 1.0);
}
"#;

/// What the worker builds: pipelines, buffers and the texture, all on one device.
pub struct Engine {
    key: DeviceKey,
    size: wgpu::ComputePipeline,
    draw: wgpu::ComputePipeline,
    params: wgpu::Buffer,
    counts: wgpu::Buffer,
    size_group: wgpu::BindGroup,
    draw_group: wgpu::BindGroup,
    paint_module: wgpu::ShaderModule,
    paint_group: wgpu::BindGroup,
    /// Paint pipelines by the pass's format: a layer's differs from the window's.
    paint: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
}

impl Engine {
    fn new(device: &wgpu::Device, key: DeviceKey, cache: Option<&wgpu::PipelineCache>) -> Engine {
        use wgpu::{BufferUsages as B, TextureUsages as T};
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("pattern"), source: wgpu::ShaderSource::Wgsl(COMPUTE.into()) });
        let pipeline = |entry| device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry), layout: None, module: &module, entry_point: Some(entry), compilation_options: Default::default(), cache,
        });
        let (size, draw) = (pipeline("size"), pipeline("draw"));
        let buffer = |size, usage| device.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage, mapped_at_creation: false });
        let (params, counts) = (buffer(16, B::UNIFORM | B::COPY_DST), buffer(12, B::STORAGE | B::INDIRECT));
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pattern"), size: wgpu::Extent3d { width: SIZE[0], height: SIZE[1], depth_or_array_layers: 1 }, mip_level_count: 1,
            sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8Unorm,
            usage: T::STORAGE_BINDING | T::TEXTURE_BINDING, view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let group = |pipeline: &wgpu::ComputePipeline, last: wgpu::BindingResource<'_>| device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None, layout: &pipeline.get_bind_group_layout(0), entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: if matches!(last, wgpu::BindingResource::TextureView(_)) { 2 } else { 1 }, resource: last },
            ],
        });
        let (size_group, draw_group) = (group(&size, counts.as_entire_binding()), group(&draw, wgpu::BindingResource::TextureView(&view)));
        let paint_module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("paint"), source: wgpu::ShaderSource::Wgsl(PAINT.into()) });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let mut engine = Engine {
            key, size, draw, params, counts, size_group, draw_group,
            paint_group: device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &paint_layout(device), entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
            ] }),
            paint_module, paint: HashMap::new(),
        };
        // The window's formats, ahead of the first frame; a layer's makes its own.
        for format in [wgpu::TextureFormat::Bgra8Unorm, wgpu::TextureFormat::Bgra8UnormSrgb, wgpu::TextureFormat::Rgba8UnormSrgb] {
            engine.paint_for(device, format, cache);
        }
        engine
    }

    fn paint_for(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat, cache: Option<&wgpu::PipelineCache>) -> &wgpu::RenderPipeline {
        let module = &self.paint_module;
        self.paint.entry(format).or_insert_with(|| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None, bind_group_layouts: &[Some(&paint_layout(device))], immediate_size: 32,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("paint"), layout: Some(&layout),
                vertex: wgpu::VertexState { module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                fragment: Some(wgpu::FragmentState { module, entry_point: Some("fs"), compilation_options: Default::default(), targets: &[Some(format.into())] }),
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
                depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache,
            })
        })
    }
}

fn paint_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let entry = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty, count: None };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &[
        entry(0, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }),
        entry(1, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
    ] })
}

/// Where the pipeline cache lives between launches.
pub fn cache_file() -> Option<PathBuf> {
    Some(rune::dirs::AppDirs::new("gpu-compute")?.cache().join("pipelines.bin"))
}

/// Builds the engine on `gpu`'s device on a worker; None once the device is lost.
fn build_on(gpu: &GpuHandle, file: Option<PathBuf>) -> Option<Engine> {
    gpu.with(|device, _| build(device, gpu.key(), file))
}

/// Builds the engine on `device` with the pipeline cache saved in `file`, and saves it again if
/// building added to it.
fn build(device: &wgpu::Device, key: DeviceKey, file: Option<PathBuf>) -> Engine {
    {
        let saved = file.as_ref().and_then(|file| std::fs::read(file).ok());
        // SAFETY: the data is what `get_data` saved; Rune checks it and recompiles for anything
        // stale, foreign or damaged.
        #[allow(unsafe_code)]
        let cache = device.features().contains(wgpu::Features::PIPELINE_CACHE).then(|| unsafe {
            device.create_pipeline_cache(&wgpu::PipelineCacheDescriptor { label: Some("gpu-compute"), data: saved.as_deref(), fallback: true })
        });
        let engine = Engine::new(device, key, cache.as_ref());
        if let (Some(data), Some(file)) = (cache.and_then(|cache| cache.get_data()), file)
            && saved.as_deref() != Some(&data[..])
        {
            file.parent().map(std::fs::create_dir_all);
            std::fs::write(file, data).ok();
        }
        engine
    }
}

/// Paints the engine's texture at the effect slot; Rune draws everything else.
#[derive(Default)]
pub struct Pattern {
    /// Where the pipeline cache is saved, if anywhere.
    cache: Option<PathBuf>,
    key: Option<DeviceKey>,
    /// Where the worker leaves the engine it built.
    built: Arc<Mutex<Option<Engine>>>,
    engine: Option<Engine>,
    time: f32,
}

impl Pattern {
    pub fn new(cache: Option<PathBuf>) -> Pattern {
        Pattern { cache, ..Pattern::default() }
    }
}

impl Compositor<crate::State> for Pattern {
    fn features(&self) -> wgpu::Features {
        wgpu::Features::IMMEDIATES
    }

    fn render(&mut self, f: &mut Frame<'_, crate::State>) {
        if self.key != Some(f.device_key()) {
            self.key = Some(f.device_key());
            self.engine = None;
            match f.gpu_handle() {
                // A window builds on a worker while Rune draws its first frames.
                Some(gpu) if !rune::is_headless() => {
                    let (built, cache) = (Arc::clone(&self.built), self.cache.clone());
                    std::thread::spawn(move || {
                        if let Some(engine) = build_on(&gpu, cache) && !gpu.is_lost() { *built.lock().unwrap() = Some(engine); }
                    });
                }
                // A snapshot, or a renderer of the app's own, shows the pattern from its first frame.
                _ => self.engine = Some(build(f.device, f.device_key(), self.cache.clone())),
            }
        }
        if let Some(engine) = self.built.lock().unwrap().take() && Some(engine.key) == self.key {
            self.engine = Some(engine);
        }
        let Some(engine) = self.engine.as_mut() else { return f.draw_ui() };
        self.time += f.dt as f32;
        let mut params = [0u8; 16];
        params[..8].copy_from_slice(bytemuck::cast_slice(&SIZE));
        params[8..12].copy_from_slice(&self.time.to_le_bytes());
        f.write_buffer(&engine.params, 0, &params);
        {
            let mut pass = f.encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&engine.size);
            pass.set_bind_group(0, &engine.size_group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
            pass.set_pipeline(&engine.draw);
            pass.set_bind_group(0, &engine.draw_group, &[]);
            pass.dispatch_workgroups_indirect(&engine.counts, 0);
        }
        let (device, queue, clear) = (f.device, f.queue, f.rt.clear_color());
        f.ui.set_fade_stops(f.rt.fade_stops());
        f.ui.prepare(device, queue, &mut f.rt.text.atlas, &f.rt.instances, f.size);
        f.ui.draw_with(f.encoder, f.target, clear, &mut |pass, site| {
            if site.mark != SLOT { return }
            let [x, y, w, h] = site.instance.rect;
            let place = [x - site.origin[0], y - site.origin[1], w, h, site.size[0] as f32, site.size[1] as f32,
                if site.srgb_blend { 1.0 } else { 0.0 }, 0.0];
            pass.set_pipeline(engine.paint_for(device, site.format, None));
            pass.set_bind_group(0, &engine.paint_group, &[]);
            pass.set_immediates(0, bytemuck::cast_slice(&place));
            pass.draw(0..4, 0..1);
        });
    }

    fn release_gpu(&mut self) {
        self.engine = None;
        self.key = None;
        self.built.lock().unwrap().take();
    }

    fn animating(&self) -> bool {
        // The pattern moves; until the engine lands, frames pick it up as soon as it does.
        true
    }
}
