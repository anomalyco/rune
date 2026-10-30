//! The GPU context: textures, pipelines, bloom and the Y'CbCr encode.

use super::*;

pub(super) struct Tex {
    pub(super) tex: wgpu::Texture,
    pub(super) view: wgpu::TextureView,
}

fn tex(device: &wgpu::Device, label: &str, w: u32, h: u32, mips: u32, format: wgpu::TextureFormat, usage: wgpu::TextureUsages) -> Tex {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    Tex { tex, view }
}

pub(super) struct Layer {
    pub(super) t: Tex,
    pub(super) mips: Vec<wgpu::TextureView>,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct GU {
    pub(super) size: [f32; 4],
    pub(super) gi: [f32; 4],
    pub(super) hp: [[f32; 4]; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct VP {
    pub(super) a: [f32; 4],
    pub(super) b: [f32; 4],
    pub(super) c: [f32; 4],
    pub(super) q: [[f32; 4]; 4],
}

pub(super) const MAX_VP: usize = 64;

pub(super) struct Viz {
    pub(super) g: wgpu::Buffer,
    pub(super) vp: wgpu::Buffer,
    pub(super) bgl: wgpu::BindGroupLayout,
    pub(super) field: wgpu::RenderPipeline,
    pub(super) scene: wgpu::RenderPipeline,
    pub(super) cascade: wgpu::RenderPipeline,
    pub(super) fluence: wgpu::RenderPipeline,
    pub(super) mip: wgpu::RenderPipeline,
    pub(super) layer: wgpu::RenderPipeline,
    pub(super) accum: wgpu::RenderPipeline,
    pub(super) mean_bgl: wgpu::BindGroupLayout,
    pub(super) mean: wgpu::ComputePipeline,
    pub(super) mean_u: Vec<wgpu::Buffer>,
    pub(super) means_own: Vec<Tex>,
    pub(super) means_mrg: Vec<Tex>,
    pub(super) lin: wgpu::Sampler,
    pub(super) aniso: wgpu::Sampler,
    pub(super) dummy: Tex,
}

pub(super) struct Enc {
    pub(super) u: wgpu::Buffer,
    pub(super) bgl: wgpu::BindGroupLayout,
    pub(super) yuv: wgpu::ComputePipeline,
    pub(super) rgba: wgpu::ComputePipeline,
    pub(super) noise: Tex,
    pub(super) y8: wgpu::Buffer,
    pub(super) y10: wgpu::Buffer,
    pub(super) rgba_buf: wgpu::Buffer,
    pub(super) read8: wgpu::Buffer,
    pub(super) read10: wgpu::Buffer,
    pub(super) read_rgba: wgpu::Buffer,
}

pub(super) struct Ctx {
    pub(super) device: wgpu::Device,
    pub(super) queue: wgpu::Queue,
    pub(super) rune: Renderer,
    pub(super) ts: TextSystem,
    pub(super) light: LightRenderer,
    pub(super) glass: GlassRenderer,
    pub(super) fx: FxRenderer,
    pub(super) viz: Viz,
    pub(super) enc: Enc,
    pub(super) base: Tex,
    pub(super) text: Tex,
    pub(super) emission: Tex,
    pub(super) lit: Tex,
    pub(super) backdrop: Tex,
    pub(super) glassed: Tex,
    pub(super) stack: Tex,
    pub(super) overlay: Tex,
    pub(super) accum: Tex,
    pub(super) bloom: Bloom,
    pub(super) layers: Vec<Layer>,
    pub(super) timestamps: bool,
    pub(super) query: Option<(wgpu::QuerySet, wgpu::Buffer, wgpu::Buffer)>,
    pub(super) period: f32,
    pub(super) noop: wgpu::ComputePipeline,
    pub(super) gpu: String,
}

fn bgl_entry(binding: u32, vis: wgpu::ShaderStages, ty: wgpu::BindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: vis, ty, count: None }
}

fn ftex() -> wgpu::BindingType {
    wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }
}

fn ubuf() -> wgpu::BindingType {
    wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }
}

fn sbuf(read_only: bool) -> wgpu::BindingType {
    wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None }
}

fn bind(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, entries: &[wgpu::BindingResource<'_>]) -> wgpu::BindGroup {
    let e: Vec<wgpu::BindGroupEntry> = entries.iter().enumerate().map(|(i, r)| wgpu::BindGroupEntry { binding: i as u32, resource: r.clone() }).collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout, entries: &e })
}

fn tv(v: &wgpu::TextureView) -> wgpu::BindingResource<'_> {
    wgpu::BindingResource::TextureView(v)
}

fn wgsl_part(name: &str) -> String {
    let src = include_str!("reel.wgsl");
    let (viz, rest) = src.split_once("// ---- bloom ----").unwrap();
    let (bloom, rest) = rest.split_once("// ---- per-probe mean of a cascade level ----").unwrap();
    let (mean, enc) = rest.split_once("// ---- final encode ----").unwrap();
    match name {
        "viz" => viz.to_string(),
        "bloom" => bloom.to_string(),
        "mean" => mean.to_string(),
        _ => enc.to_string(),
    }
}

/// 64×64 blue noise, three independent channels (void and cluster, Ulichney 1993).
fn blue_noise() -> Vec<u8> {
    const N: usize = 64;
    let sigma = 1.9f32;
    let mut kernel = vec![0f32; N * N];
    for y in 0..N {
        for x in 0..N {
            let dx = (x.min(N - x)) as f32;
            let dy = (y.min(N - y)) as f32;
            kernel[y * N + x] = (-(dx * dx + dy * dy) / (2.0 * sigma * sigma)).exp();
        }
    }
    let mut out = vec![255u8; N * N * 4];
    let mut seed = 0x9e3779b9u32;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for ch in 0..3 {
        let mut on = vec![false; N * N];
        let mut energy = vec![0f32; N * N];
        let toggle = |on: &mut Vec<bool>, energy: &mut Vec<f32>, i: usize, add: bool| {
            on[i] = add;
            let (ix, iy) = (i % N, i / N);
            let s = if add { 1.0 } else { -1.0 };
            for y in 0..N {
                for x in 0..N {
                    let kx = (x + N - ix) % N;
                    let ky = (y + N - iy) % N;
                    energy[y * N + x] += s * kernel[ky * N + kx];
                }
            }
        };
        let ones = N * N / 10;
        let mut placed = 0;
        while placed < ones {
            let i = (rnd() as usize) % (N * N);
            if !on[i] {
                toggle(&mut on, &mut energy, i, true);
                placed += 1;
            }
        }
        let tightest = |on: &Vec<bool>, energy: &Vec<f32>| (0..N * N).filter(|&i| on[i]).max_by(|&a, &b| energy[a].total_cmp(&energy[b])).unwrap();
        let voidest = |on: &Vec<bool>, energy: &Vec<f32>| (0..N * N).filter(|&i| !on[i]).min_by(|&a, &b| energy[a].total_cmp(&energy[b])).unwrap();
        loop {
            let c = tightest(&on, &energy);
            toggle(&mut on, &mut energy, c, false);
            let v = voidest(&on, &energy);
            toggle(&mut on, &mut energy, v, true);
            if v == c {
                break;
            }
        }
        let mut rank = vec![0usize; N * N];
        let (proto_on, proto_e) = (on.clone(), energy.clone());
        for r in (0..ones).rev() {
            let c = tightest(&on, &energy);
            toggle(&mut on, &mut energy, c, false);
            rank[c] = r;
        }
        on = proto_on;
        energy = proto_e;
        for r in ones..N * N {
            let v = voidest(&on, &energy);
            toggle(&mut on, &mut energy, v, true);
            rank[v] = r;
        }
        for i in 0..N * N {
            out[i * 4 + ch] = ((rank[i] as f32 + 0.5) / (N * N) as f32 * 255.0).round() as u8;
        }
    }
    out
}

impl Ctx {
    pub(super) fn new() -> Ctx {
        let rune::render::Gpu { adapter, device, queue, .. } = rune::render::Gpu::headless();
        let ts_feat = device.features() & wgpu::Features::TIMESTAMP_QUERY;
        eprintln!("adapter: {:?}, timestamps: {}", adapter.get_info().name, !ts_feat.is_empty());
        use wgpu::TextureUsages as U;
        let rt = U::RENDER_ATTACHMENT | U::TEXTURE_BINDING;
        let full = |label: &str| tex(&device, label, W, H, 1, F16, rt | U::COPY_SRC | U::COPY_DST);
        let base = full("ui base");
        let text = full("ui text");
        let emission = full("emission");
        let lit = full("lit");
        let backdrop = full("backdrop");
        let glassed = full("glassed");
        let stack = full("stack");
        let overlay = full("overlay");
        let accum = full("accum");
        let bloom = Bloom::new(&device);
        let layers = (0..10)
            .map(|i| {
                let t = tex(&device, &format!("layer {i}"), W, H, MIPS, F16, rt | U::COPY_DST);
                let mips = (0..MIPS)
                    .map(|m| t.tex.create_view(&wgpu::TextureViewDescriptor { base_mip_level: m, mip_level_count: Some(1), ..Default::default() }))
                    .collect();
                Layer { t, mips }
            })
            .collect();
        let rune = Renderer::new(&device, F16);
        let light = LightRenderer::new(&device, &queue, W, H, F16);
        let glass = GlassRenderer::new(&device, W, H, F16);
        let fx = FxRenderer::new(&device, F16, FxOptions { dither: false });
        let viz = Self::viz(&device, light.inspect().gi_size);
        let enc = Self::enc(&device, &queue);
        let query = (!ts_feat.is_empty()).then(|| {
            let qs = device.create_query_set(&wgpu::QuerySetDescriptor { label: Some("reel ts"), ty: wgpu::QueryType::Timestamp, count: 4 });
            let resolve = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 32, usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
            let read = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 32, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
            (qs, resolve, read)
        });
        let period = queue.get_timestamp_period();
        let nm = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl("@compute @workgroup_size(1) fn noop() {}".into()) });
        let noop = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("noop"),
            layout: None,
            module: &nm,
            entry_point: Some("noop"),
            compilation_options: Default::default(),
            cache: None,
        });
        Ctx {
            device,
            queue,
            rune,
            ts: TextSystem::default(),
            light,
            glass,
            fx,
            viz,
            enc,
            base,
            text,
            emission,
            lit,
            backdrop,
            glassed,
            stack,
            overlay,
            accum,
            bloom,
            layers,
            timestamps: !ts_feat.is_empty(),
            query,
            period,
            noop,
            gpu: adapter.get_info().name.trim_start_matches("Apple ").to_string(),
        }
    }

    /// An empty compute pass that writes a timestamp at its start or end (render-pass timestamps aren't ordered on
    /// tile GPUs, so render work is timed between two of these).
    pub(super) fn marker(&self, enc: &mut wgpu::CommandEncoder, begin: Option<u32>, end: Option<u32>) {
        if let Some((qs, _, _)) = &self.query {
            let mut p = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ts marker"),
                timestamp_writes: Some(wgpu::ComputePassTimestampWrites { query_set: qs, beginning_of_pass_write_index: begin, end_of_pass_write_index: end }),
            });
            p.set_pipeline(&self.noop);
            p.dispatch_workgroups(1, 1, 1);
        }
    }

    pub(super) fn viz(device: &wgpu::Device, gs: (u32, u32)) -> Viz {
        use wgpu::ShaderStages as Ss;
        let vf = Ss::VERTEX_FRAGMENT;
        let f = Ss::FRAGMENT;
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viz"),
            entries: &[
                bgl_entry(0, vf, ubuf()),
                bgl_entry(1, vf, sbuf(true)),
                bgl_entry(2, f, ftex()),
                bgl_entry(3, f, ftex()),
                bgl_entry(4, f, ftex()),
                bgl_entry(5, f, ftex()),
                bgl_entry(6, f, ftex()),
                bgl_entry(7, f, ftex()),
                bgl_entry(8, f, ftex()),
                bgl_entry(9, f, ftex()),
                bgl_entry(10, f, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
                bgl_entry(11, f, ftex()),
                bgl_entry(12, f, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
            ],
        });
        let m = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("reel viz"), source: wgpu::ShaderSource::Wgsl(wgsl_part("viz").into()) });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&bgl)], immediate_size: 0 });
        let pipe = |vs: &str, fs: &str, blend: Option<wgpu::BlendState>, strip: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs),
                layout: Some(&pl),
                vertex: wgpu::VertexState { module: &m, entry_point: Some(vs), compilation_options: Default::default(), buffers: &[] },
                primitive: wgpu::PrimitiveState {
                    topology: if strip { wgpu::PrimitiveTopology::TriangleStrip } else { wgpu::PrimitiveTopology::TriangleList },
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &m,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: F16, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let field = pipe("vs_full", "fs_field", None, false);
        let scene = pipe("vs_full", "fs_scene", None, false);
        let cascade = pipe("vs_full", "fs_cascade", None, false);
        let fluence = pipe("vs_full", "fs_fluence", None, false);
        let mip = pipe("vs_full", "fs_mip", None, false);
        let layer = pipe("vs_layer", "fs_layer", Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), true);
        let add = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
        let accum = pipe("vs_full", "fs_accum", Some(wgpu::BlendState { color: add, alpha: add }), false);
        let mean_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mean"),
            entries: &[
                bgl_entry(0, Ss::COMPUTE, ubuf()),
                bgl_entry(1, Ss::COMPUTE, ftex()),
                bgl_entry(
                    2,
                    Ss::COMPUTE,
                    wgpu::BindingType::StorageTexture { access: wgpu::StorageTextureAccess::WriteOnly, format: F16, view_dimension: wgpu::TextureViewDimension::D2 },
                ),
            ],
        });
        let mm = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("reel mean"), source: wgpu::ShaderSource::Wgsl(wgsl_part("mean").into()) });
        let mpl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&mean_bgl)], immediate_size: 0 });
        let mean = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("mean"),
            layout: Some(&mpl),
            module: &mm,
            entry_point: Some("cs_mean"),
            compilation_options: Default::default(),
            cache: None,
        });
        let mean_u = (0..CASCADES)
            .map(|n| {
                use wgpu::util::DeviceExt;
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&[n, 0, 0, 0]), usage: wgpu::BufferUsages::UNIFORM })
            })
            .collect();
        use wgpu::TextureUsages as U;
        let mk = |n: u32| tex(device, "mean", gs.0 >> n, gs.1 >> n, 1, F16, U::STORAGE_BINDING | U::TEXTURE_BINDING);
        let means_own = (0..CASCADES).map(mk).collect();
        let means_mrg = (0..CASCADES).map(mk).collect();
        let lin = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("viz lin"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let aniso = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("viz aniso"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 16,
            ..Default::default()
        });
        let g = device.create_buffer(&wgpu::BufferDescriptor { label: Some("viz g"), size: std::mem::size_of::<GU>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let vp = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viz vp"),
            size: (MAX_VP * std::mem::size_of::<VP>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let dummy = tex(device, "dummy", 1, 1, 1, F16, U::TEXTURE_BINDING);
        Viz { g, vp, bgl, field, scene, cascade, fluence, mip, layer, accum, mean_bgl, mean, mean_u, means_own, means_mrg, lin, aniso, dummy }
    }

    pub(super) fn enc(device: &wgpu::Device, queue: &wgpu::Queue) -> Enc {
        use wgpu::ShaderStages as Ss;
        let c = Ss::COMPUTE;
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("enc"),
            entries: &[
                bgl_entry(0, c, ubuf()),
                bgl_entry(1, c, ftex()),
                bgl_entry(2, c, ftex()),
                bgl_entry(3, c, ftex()),
                bgl_entry(4, c, sbuf(false)),
                bgl_entry(5, c, sbuf(false)),
                bgl_entry(6, c, sbuf(false)),
                bgl_entry(7, c, ftex()),
                bgl_entry(8, c, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
            ],
        });
        let m = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("reel enc"), source: wgpu::ShaderSource::Wgsl(wgsl_part("enc").into()) });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&bgl)], immediate_size: 0 });
        let cp = |e: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some(e), layout: Some(&pl), module: &m, entry_point: Some(e), compilation_options: Default::default(), cache: None })
        };
        let yuv = cp("cs_yuv");
        let rgba = cp("cs_rgba");
        let noise = tex(device, "blue noise", 64, 64, 1, wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &noise.tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &blue_noise(),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(256), rows_per_image: Some(64) },
            wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 },
        );
        let n = (W * H) as u64;
        let buf = |label: &str, size: u64, usage: wgpu::BufferUsages| device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false });
        use wgpu::BufferUsages as B;
        let y8 = buf("y8", n * 3 / 2, B::STORAGE | B::COPY_SRC);
        let y10 = buf("y10", n * 3, B::STORAGE | B::COPY_SRC);
        let rgba_buf = buf("rgba", n * 4, B::STORAGE | B::COPY_SRC);
        let read8 = buf("read8", n * 3 / 2, B::COPY_DST | B::MAP_READ);
        let read10 = buf("read10", n * 3, B::COPY_DST | B::MAP_READ);
        let read_rgba = buf("read rgba", n * 4, B::COPY_DST | B::MAP_READ);
        let u = buf("enc u", 32, B::UNIFORM | B::COPY_DST);
        Enc { u, bgl, yuv, rgba, noise, y8, y10, rgba_buf, read8, read10, read_rgba }
    }

    pub(super) fn draw_rune(&mut self, insts: &[Instance], target: &wgpu::TextureView, clear: [f32; 4]) {
        self.rune.prepare(&self.device, &self.queue, &mut self.ts.atlas, insts, (W, H));
        let mut enc = self.device.create_command_encoder(&Default::default());
        self.rune.draw(&mut enc, target, clear);
        self.queue.submit([enc.finish()]);
    }

    pub(super) fn pass<'a>(enc: &'a mut wgpu::CommandEncoder, view: &'a wgpu::TextureView, load: Option<[f32; 4]>) -> wgpu::RenderPass<'a> {
        let ops = match load {
            Some(c) => wgpu::LoadOp::Clear(wgpu::Color { r: c[0] as f64, g: c[1] as f64, b: c[2] as f64, a: c[3] as f64 }),
            None => wgpu::LoadOp::Load,
        };
        enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: ops, store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        })
    }

    pub(super) fn copy(&self, enc: &mut wgpu::CommandEncoder, src: &wgpu::Texture, dst: &wgpu::Texture) {
        enc.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo { texture: src, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyTextureInfo { texture: dst, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        );
    }

    /// Viz bind group with the GI buffers of `level` (or level 0) and `layer` as the sampled layer texture.
    pub(super) fn viz_bg(&self, level: usize, layer: Option<&wgpu::TextureView>) -> wgpu::BindGroup {
        let ins = self.light.inspect();
        let v = &self.viz;
        bind(
            &self.device,
            &v.bgl,
            &[
                v.g.as_entire_binding(),
                v.vp.as_entire_binding(),
                tv(ins.field),
                tv(ins.emission),
                tv(ins.occluders),
                tv(&ins.own[level]),
                tv(&ins.merged[level]),
                tv(&v.means_own[level].view),
                tv(&v.means_mrg[level].view),
                tv(ins.fluence),
                wgpu::BindingResource::Sampler(&v.lin),
                tv(layer.unwrap_or(&v.dummy.view)),
                wgpu::BindingResource::Sampler(&v.aniso),
            ],
        )
    }

    pub(super) fn means(&self, enc: &mut wgpu::CommandEncoder) {
        let ins = self.light.inspect();
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("means"), timestamp_writes: None });
        pass.set_pipeline(&self.viz.mean);
        for n in 0..CASCADES as usize {
            for (src, dst) in [(&ins.own[n], &self.viz.means_own[n]), (&ins.merged[n], &self.viz.means_mrg[n])] {
                let bg = bind(&self.device, &self.viz.mean_bgl, &[self.viz.mean_u[n].as_entire_binding(), tv(src), tv(&dst.view)]);
                pass.set_bind_group(0, &bg, &[]);
                pass.dispatch_workgroups((ins.gi_size.0 >> n).div_ceil(8), (ins.gi_size.1 >> n).div_ceil(8), 1);
            }
        }
    }

    pub(super) fn build_mips(&self, enc: &mut wgpu::CommandEncoder, layer: usize) {
        for m in 1..MIPS as usize {
            let bg = self.viz_bg(0, Some(&self.layers[layer].mips[m - 1]));
            let mut pass = Self::pass(enc, &self.layers[layer].mips[m], Some([0.0; 4]));
            pass.set_pipeline(&self.viz.mip);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// Encode the final image (+ overlay) to yuv420p (8-bit) and yuv420p10le, and optionally RGBA8.
    pub(super) fn encode(&mut self, frame: &wgpu::TextureView, rgba: bool, yuv: bool) -> (Option<Vec<u8>>, Option<Vec<u8>>, Option<Vec<u8>>) {
        let e = &self.enc;
        let bloom_mix = 0.045;
        self.queue.write_buffer(&e.u, 0, bytemuck::cast_slice(&[W as f32, H as f32, 1.0, 1.0, bloom_mix, 1.0 / BLOOM_LEVELS as f32, 0.0, 0.0]));
        let bg = bind(
            &self.device,
            &e.bgl,
            &[
                e.u.as_entire_binding(),
                tv(frame),
                tv(&self.overlay.view),
                tv(&e.noise.view),
                e.y8.as_entire_binding(),
                e.y10.as_entire_binding(),
                e.rgba_buf.as_entire_binding(),
                tv(&self.bloom.levels[0]),
                wgpu::BindingResource::Sampler(&self.bloom.sampler),
            ],
        );
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("encode"), timestamp_writes: None });
            pass.set_bind_group(0, &bg, &[]);
            if yuv {
                pass.set_pipeline(&e.yuv);
                pass.dispatch_workgroups((W / 8).div_ceil(8), (H / 2).div_ceil(8), 1);
            }
            if rgba {
                pass.set_pipeline(&e.rgba);
                pass.dispatch_workgroups(W.div_ceil(8), H.div_ceil(8), 1);
            }
        }
        let n = (W * H) as u64;
        if yuv {
            enc.copy_buffer_to_buffer(&e.y8, 0, &e.read8, 0, n * 3 / 2);
            enc.copy_buffer_to_buffer(&e.y10, 0, &e.read10, 0, n * 3);
        }
        if rgba {
            enc.copy_buffer_to_buffer(&e.rgba_buf, 0, &e.read_rgba, 0, n * 4);
        }
        self.queue.submit([enc.finish()]);
        let read = |b: &wgpu::Buffer| {
            let s = b.slice(..);
            s.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
            self.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
            let v = s.get_mapped_range().expect("range").to_vec();
            b.unmap();
            v
        };
        let (a, b) = if yuv { (Some(read(&e.read8)), Some(read(&e.read10))) } else { (None, None) };
        let c = if rgba { Some(read(&e.read_rgba)) } else { None };
        (a, b, c)
    }
}

const BLOOM_LEVELS: u32 = 7;

pub(super) struct Bloom {
    pub(super) levels: Vec<wgpu::TextureView>,
    pub(super) bgl: wgpu::BindGroupLayout,
    pub(super) down: wgpu::RenderPipeline,
    pub(super) up: wgpu::RenderPipeline,
    pub(super) karis: wgpu::Buffer,
    pub(super) plain: wgpu::Buffer,
    pub(super) sampler: wgpu::Sampler,
}

impl Bloom {
    pub(super) fn new(device: &wgpu::Device) -> Bloom {
        use wgpu::util::DeviceExt;
        let f = wgpu::ShaderStages::FRAGMENT;
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bloom"),
            entries: &[bgl_entry(0, f, ubuf()), bgl_entry(1, f, ftex()), bgl_entry(2, f, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering))],
        });
        let m = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("bloom"), source: wgpu::ShaderSource::Wgsl(wgsl_part("bloom").into()) });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&bgl)], immediate_size: 0 });
        let pipe = |fs: &str, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs),
                layout: Some(&pl),
                vertex: wgpu::VertexState { module: &m, entry_point: Some("vs_b"), compilation_options: Default::default(), buffers: &[] },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &m,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: F16, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let add = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
        let down = pipe("fs_bdown", None);
        let up = pipe("fs_bup", Some(wgpu::BlendState { color: add, alpha: add }));
        let t = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bloom"),
            size: wgpu::Extent3d { width: W / 2, height: H / 2, depth_or_array_layers: 1 },
            mip_level_count: BLOOM_LEVELS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: F16,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let levels = (0..BLOOM_LEVELS)
            .map(|k| t.create_view(&wgpu::TextureViewDescriptor { base_mip_level: k, mip_level_count: Some(1), ..Default::default() }))
            .collect();
        let karis = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&[1.0f32, 1.0, 0.0, 0.0]), usage: wgpu::BufferUsages::UNIFORM });
        let plain = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&[0.0f32, 1.0, 0.0, 0.0]), usage: wgpu::BufferUsages::UNIFORM });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("bloom"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        Bloom { levels, bgl, down, up, karis, plain, sampler }
    }
}

impl Ctx {
    pub(super) fn bloom(&self, src: &wgpu::TextureView) {
        let b = &self.bloom;
        let mut enc = self.device.create_command_encoder(&Default::default());
        let bg = |u: &wgpu::Buffer, v: &wgpu::TextureView| bind(&self.device, &b.bgl, &[u.as_entire_binding(), tv(v), wgpu::BindingResource::Sampler(&b.sampler)]);
        for k in 0..BLOOM_LEVELS as usize {
            let g = if k == 0 { bg(&b.karis, src) } else { bg(&b.plain, &b.levels[k - 1]) };
            let mut pass = Self::pass(&mut enc, &b.levels[k], Some([0.0; 4]));
            pass.set_pipeline(&b.down);
            pass.set_bind_group(0, &g, &[]);
            pass.draw(0..3, 0..1);
        }
        for k in (1..BLOOM_LEVELS as usize).rev() {
            let g = bg(&b.plain, &b.levels[k]);
            let mut pass = Self::pass(&mut enc, &b.levels[k - 1], None);
            pass.set_pipeline(&b.up);
            pass.set_bind_group(0, &g, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([enc.finish()]);
    }
}
