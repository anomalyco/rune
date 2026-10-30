//! Thick refracting glass over a lit frame (see `shaders/glass.wgsl` for the optics).
//!
//! ```ignore
//! let card = Glass::rounded(x, y, w, h, 40.0 * s).thickness(36.0 * s).bevel(28.0 * s).gap(10.0 * s);
//! glass.render(&device, &queue, &mut encoder, &[card], &lights, GlassSettings::default(), &lit, lights.inspect().fluence, &target);
//! ```

use bytemuck::{Pod, Zeroable};

use crate::{FLAG_EMISSIVE, Prim};

/// One glass slab, in device px.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct Glass {
    pub a: [f32; 4],
    pub b: [f32; 4],
    /// radius a, radius b, smooth-union k, morph a → b
    pub r: [f32; 4],
    /// thickness, bevel, gap above the UI, dome
    pub p0: [f32; 4],
    /// ior at 550 nm, dispersion n(400) − n(700), frost 0…1, presence 0…1
    pub p1: [f32; 4],
    /// absorption per px rgb (Beer–Lambert), rim glow from GI
    pub tint: [f32; 4],
}

impl Glass {
    /// Crown glass (n = 1.5 at 550 nm, a little stronger dispersion than real for visibility), faintly green in depth.
    pub fn rounded(x: f32, y: f32, w: f32, h: f32, radius: f32) -> Glass {
        Glass {
            a: [x, y, w, h],
            b: [0.0; 4],
            r: [radius, 0.0, 0.0, 0.0],
            p0: [40.0, 30.0, 12.0, 0.0],
            p1: [1.5, 0.03, 0.0, 1.0],
            tint: [0.0016, 0.00045, 0.0011, 0.35],
        }
    }

    pub fn thickness(mut self, t: f32) -> Glass {
        self.p0[0] = t;
        self
    }

    /// Width of the rounded shoulder from the rim to the flat top.
    pub fn bevel(mut self, b: f32) -> Glass {
        self.p0[1] = b;
        self
    }

    /// Height of the bottom face above the UI.
    pub fn gap(mut self, g: f32) -> Glass {
        self.p0[2] = g;
        self
    }

    /// Extra height in the middle: a convex top that magnifies.
    pub fn dome(mut self, d: f32) -> Glass {
        self.p0[3] = d;
        self
    }

    pub fn ior(mut self, n: f32) -> Glass {
        self.p1[0] = n;
        self
    }

    pub fn dispersion(mut self, d: f32) -> Glass {
        self.p1[1] = d;
        self
    }

    pub fn frost(mut self, f: f32) -> Glass {
        self.p1[2] = f;
        self
    }

    /// Scales thickness, dome and shadow together: glass appears by lensing, not by fading.
    pub fn presence(mut self, p: f32) -> Glass {
        self.p1[3] = p;
        self
    }

    pub fn tint(mut self, absorb: [f32; 3]) -> Glass {
        self.tint[..3].copy_from_slice(&absorb);
        self
    }

    pub fn rim_glow(mut self, g: f32) -> Glass {
        self.tint[3] = g;
        self
    }

    /// Smooth-union a second rounded rect with blend radius `k`; `morph` moves the whole shape toward it.
    pub fn union(mut self, rect: [f32; 4], radius: f32, k: f32, morph: f32) -> Glass {
        self.b = rect;
        self.r[1] = radius;
        self.r[2] = k;
        self.r[3] = morph;
        self
    }
}

/// A glowing UI element as a sphere light for the glass's highlights.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct GlassLight {
    /// x, y, z, radius
    pub pos: [f32; 4],
    pub color: [f32; 4],
}

impl GlassLight {
    /// From an emissive primitive: a sphere of the element's size, `z` px above the UI, radiance × `gain`.
    pub fn from_emitter(p: &Prim, z: f32, gain: f32) -> Option<GlassLight> {
        if p.flags() & FLAG_EMISSIVE == 0 {
            return None;
        }
        let r = 0.5 * (p.rect[2] * p.rect[3]).sqrt();
        Some(GlassLight {
            pos: [p.rect[0] + p.rect[2] * 0.5, p.rect[1] + p.rect[3] * 0.5, z, r],
            color: [p.emit[0] * gain, p.emit[1] * gain, p.emit[2] * gain, 0.0],
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GlassSettings {
    /// Environment (studio) reflection strength.
    pub env: f32,
    /// Drop shadow strength, offset and blur (px).
    pub shadow: f32,
    pub shadow_offset: [f32; 2],
    pub shadow_blur: f32,
    /// Caustic ring strength.
    pub caustic: f32,
}

impl Default for GlassSettings {
    fn default() -> Self {
        GlassSettings { env: 1.0, shadow: 0.35, shadow_offset: [8.0, 22.0], shadow_blur: 30.0, caustic: 0.6 }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GlassU {
    size: [f32; 4],
    misc: [f32; 4],
    shadow: [f32; 4],
}

const MAX_GLASS: usize = 8;
const MAX_GLIGHTS: usize = 32;
const LEVELS: u32 = 7;

pub struct GlassRenderer {
    /// The size it was made for; make a new one when the target changes size.
    pub width: u32,
    pub height: u32,
    /// Mips in the frost pyramid (fewer than `LEVELS` for small targets).
    mips: u32,
    uniform: wgpu::Buffer,
    glass: wgpu::Buffer,
    lights: wgpu::Buffer,
    sampler: wgpu::Sampler,
    pipe: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    down: wgpu::RenderPipeline,
    down_bgl: wgpu::BindGroupLayout,
    pyramid: wgpu::TextureView,
    levels: Vec<wgpu::TextureView>,
    down_bgs: Vec<wgpu::BindGroup>,
    /// The pyramid's first level, keyed on the backdrop.
    first_bg: crate::BindCache<wgpu::TextureView>,
    /// Keyed on the backdrop and fluence.
    bg: crate::BindCache<[wgpu::TextureView; 2]>,
    staging: Option<rune_staging::Staging>,
}

fn section(name: &str) -> String {
    let src = include_str!("shaders/glass.wgsl");
    let (main, pyr) = src.split_once("// ---- pyramid ----").unwrap_or((src, ""));
    if name == "pyramid" {
        format!(
            "@vertex\nfn vs(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {{\n    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));\n    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);\n}}\n{pyr}"
        )
    } else {
        main.to_string()
    }
}

impl GlassRenderer {
    pub fn new(device: &wgpu::Device, width: u32, height: u32, format: wgpu::TextureFormat) -> GlassRenderer {
        let f = wgpu::ShaderStages::FRAGMENT;
        let buf = |label, size, usage| device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false });
        use wgpu::BufferUsages as B;
        let uniform = buf("glass u", std::mem::size_of::<GlassU>() as u64, B::UNIFORM | B::COPY_DST);
        let glass = buf("glass", (MAX_GLASS * std::mem::size_of::<Glass>()) as u64, B::STORAGE | B::COPY_DST);
        let lights = buf("glass lights", (MAX_GLIGHTS * std::mem::size_of::<GlassLight>()) as u64, B::STORAGE | B::COPY_DST);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("glass linear"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let bgl = crate::layout(
            device,
            "glass",
            &[
                crate::bgl::uniform(0, f),
                crate::bgl::storage(1, f),
                crate::bgl::storage(2, f),
                crate::bgl::f2(3, f),
                crate::bgl::f2(4, f),
                crate::bgl::f2(5, f),
                crate::bgl::sampler(6, f),
            ],
        );
        let pipeline = |label: &str, src: String, entry: &str, layout: &wgpu::BindGroupLayout, fmt: wgpu::TextureFormat| {
            let m = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(src.into()) });
            let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(layout)], immediate_size: 0 });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState { module: &m, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &m,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: fmt, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipe = pipeline("glass", section("glass"), "fs", &bgl, format);
        let down_bgl = crate::layout(device, "glass down", &[crate::bgl::f2(0, f), crate::bgl::sampler(1, f)]);
        let down = pipeline("glass down", section("pyramid"), "fs_down", &down_bgl, wgpu::TextureFormat::Rgba16Float);
        let half = (width.div_ceil(2).max(1), height.div_ceil(2).max(1));
        let mips = LEVELS.min(32 - half.0.max(half.1).leading_zeros());
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glass pyramid"),
            size: wgpu::Extent3d { width: half.0, height: half.1, depth_or_array_layers: 1 },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let pyramid = tex.create_view(&Default::default());
        let levels: Vec<wgpu::TextureView> = (0..mips)
            .map(|k| tex.create_view(&wgpu::TextureViewDescriptor { base_mip_level: k, mip_level_count: Some(1), ..Default::default() }))
            .collect();
        let down_bgs = (1..mips as usize)
            .map(|k| crate::bind(device, &down_bgl, [crate::tv(&levels[k - 1]), wgpu::BindingResource::Sampler(&sampler)]))
            .collect();
        GlassRenderer { width, height, mips, uniform, glass, lights, sampler, pipe, bgl, down, down_bgl, pyramid, levels, down_bgs, first_bg: crate::BindCache::new(), bg: crate::BindCache::new(), staging: None }
    }

    fn pass<'a>(encoder: &'a mut wgpu::CommandEncoder, target: &'a wgpu::TextureView, label: &'a str) -> wgpu::RenderPass<'a> {
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        })
    }

    /// Draw `backdrop` (the lit frame, linear, filterable) with `glass` over it into `target` (a different texture).
    /// `fluence` is the GI's fluence ([`crate::Inspect::fluence`]), or any black texture.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        glass: &[Glass],
        lights: &[GlassLight],
        settings: GlassSettings,
        backdrop: &wgpu::TextureView,
        fluence: &wgpu::TextureView,
        target: &wgpu::TextureView,
    ) {
        let ng = glass.len().min(MAX_GLASS);
        let nl = lights.len().min(MAX_GLIGHTS);
        let staging = self.staging.get_or_insert_with(|| rune_staging::Staging::new(device, queue));
        staging.write_buffer(&self.glass, 0, bytemuck::cast_slice(&glass[..ng]));
        staging.write_buffer(&self.lights, 0, bytemuck::cast_slice(&lights[..nl]));
        let u = GlassU {
            size: [self.width as f32, self.height as f32, ng as f32, nl as f32],
            misc: [self.mips as f32, settings.env, settings.shadow, settings.caustic],
            shadow: [settings.shadow_offset[0], settings.shadow_offset[1], settings.shadow_blur, 0.0],
        };
        staging.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&u));
        staging.flush(encoder);
        if ng > 0 {
            let first = self.first_bg.get(backdrop.clone(), || {
                crate::bind(device, &self.down_bgl, [crate::tv(backdrop), wgpu::BindingResource::Sampler(&self.sampler)])
            });
            for k in 0..self.mips as usize {
                let mut pass = Self::pass(encoder, &self.levels[k], "glass pyramid");
                pass.set_pipeline(&self.down);
                pass.set_bind_group(0, if k == 0 { first } else { &self.down_bgs[k - 1] }, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        let bg = self.bg.get([backdrop.clone(), fluence.clone()], || {
            crate::bind(
                device,
                &self.bgl,
                [
                    self.uniform.as_entire_binding(),
                    self.glass.as_entire_binding(),
                    self.lights.as_entire_binding(),
                    crate::tv(backdrop),
                    crate::tv(&self.pyramid),
                    crate::tv(fluence),
                    wgpu::BindingResource::Sampler(&self.sampler),
                ],
            )
        });
        let mut pass = Self::pass(encoder, target, "glass");
        pass.set_pipeline(&self.pipe);
        pass.set_bind_group(0, bg, &[]);
        pass.draw(0..3, 0..1);
    }
}
