//! rune-light: real-time lighting for Rune UIs.
//!
//! Every Rune element is an analytic rounded rect, so a frame is an exact 2D distance field for free. rune-light
//! extrudes the elements marked as occluders into short slabs standing on the UI plane and lights them:
//!
//! 1. **Surface buffer** (full res, instanced quads): topmost primitive's own SDF, outward gradient, top z, id.
//! 2. **Field** (half res, compute): exact union SDF of occluders + the nearest slab's z range, and a GI distance.
//! 3. **Soft shadows** (half res, compute): per light, sphere tracing the field from each receiver toward a
//!    spherical source with Quílez/Aaltonen's penumbra estimate; contact-hardening comes out of the geometry.
//!    Optional "air" trace at the light's height for god rays.
//! 4. **GI** (optional, quarter res): 2D radiance cascades; emitters light their neighbours, cards bounce.
//! 5. **Composite** (full res): UI colour as albedo, bevel normals from the SDF, GGX highlights, rim hairlines,
//!    contact AO, text with a contrast floor, soft shoulder, sRGB + dither.
//!
//! Zero lights and no emitters: [`LightRenderer::render`] records nothing and returns `false`; draw the UI as usual.

pub mod cpu;
pub mod glass;
mod gpu;
mod render;
mod targets;

pub use glass::{Glass, GlassLight, GlassRenderer, GlassSettings};
use gpu::{bgl, bind, compute, gi_section, layout, module, texture, tv};
use targets::Targets;

use bytemuck::{Pod, Zeroable};

pub const MAX_LIGHTS: usize = 8;
pub const FLAG_OCCLUDER: u32 = 1;
pub const FLAG_EMISSIVE: u32 = 2;
pub const FLAG_RAISED: u32 = 4;
/// Seen only by the GI tracer's distance field: marks where [`Scene::emission`] may glow. Never a surface.
pub const FLAG_GI_HINT: u32 = 8;

/// One lit primitive, in device pixels. Mirrors Rune's `Instance` rect/radii plus lighting fields.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct Prim {
    /// x, y, w, h
    pub rect: [f32; 4],
    /// top-left, top-right, bottom-right, bottom-left
    pub radii: [f32; 4],
    /// bottom z, top z, bevel width, bevel height
    pub z: [f32; 4],
    /// gloss, roughness, flags, unused
    pub mat: [f32; 4],
    /// linear emitted radiance rgb (GI), unused
    pub emit: [f32; 4],
    /// albedo rgb for bounce, unused
    pub albedo: [f32; 4],
}

impl Prim {
    /// A flat surface (receives light, casts nothing).
    pub fn rect(x: f32, y: f32, w: f32, h: f32) -> Prim {
        Prim {
            rect: [x, y, w, h],
            radii: [0.0; 4],
            z: [0.0, 0.0, 0.0, 0.0],
            mat: [0.0, 0.6, 0.0, 0.0],
            emit: [0.0; 4],
            albedo: [0.5, 0.5, 0.5, 0.0],
        }
    }

    pub fn radius(mut self, r: f32) -> Prim {
        self.radii = [r; 4];
        self
    }

    pub fn radii(mut self, r: [f32; 4]) -> Prim {
        self.radii = r;
        self
    }

    /// Sit on a surface at height `z` (a chip on a card): the base for `raised`, and the receiver height.
    pub fn on(mut self, z: f32) -> Prim {
        self.z[0] = z;
        self.z[1] = z;
        self
    }

    /// Stand `height` px proud of its base and cast shadows.
    pub fn raised(mut self, height: f32) -> Prim {
        self.z[1] = self.z[0] + height;
        self.set(FLAG_OCCLUDER, height > 0.0)
    }

    /// Float between `bottom` and `top` (a popover): its shadow detaches from it.
    pub fn floating(mut self, bottom: f32, top: f32) -> Prim {
        self.z[0] = bottom;
        self.z[1] = top;
        self.set(FLAG_OCCLUDER, true)
    }

    /// Bevel the edge: `width` px of rounded shoulder rising `height` px, for normals and highlights.
    pub fn bevel(mut self, width: f32, height: f32) -> Prim {
        self.z[2] = width;
        self.z[3] = height;
        self.set(FLAG_RAISED, width > 0.0)
    }

    pub fn gloss(mut self, gloss: f32, roughness: f32) -> Prim {
        self.mat[0] = gloss;
        self.mat[1] = roughness;
        self
    }

    /// Glow with linear radiance `rgb` (feeds the GI pass; the pixel itself is never darkened).
    pub fn emissive(mut self, rgb: [f32; 3]) -> Prim {
        self.emit = [rgb[0], rgb[1], rgb[2], 0.0];
        let on = rgb.iter().any(|c| *c > 0.0);
        self.set(FLAG_EMISSIVE, on)
    }

    /// Turn this rect into a GI hint: the region where the extra emission texture may glow (around an electric
    /// border, say), so the cascades' sphere tracing slows down there instead of stepping over it.
    pub fn gi_hint(mut self) -> Prim {
        self.mat[2] = FLAG_GI_HINT as f32;
        self
    }

    pub fn albedo(mut self, rgb: [f32; 3]) -> Prim {
        self.albedo = [rgb[0], rgb[1], rgb[2], 0.0];
        self
    }

    pub fn flags(&self) -> u32 {
        self.mat[2] as u32
    }

    fn set(mut self, flag: u32, on: bool) -> Prim {
        let f = if on { self.flags() | flag } else { self.flags() & !flag };
        self.mat[2] = f as f32;
        self
    }

    /// From a Rune uber-quad instance's rect and radii (device px); lighting fields start flat.
    pub fn from_rune(rect: [f32; 4], radii: [f32; 4]) -> Prim {
        Prim::rect(rect[0], rect[1], rect[2], rect[3]).radii(radii)
    }
}

/// A light, in device pixels. `z` is height above the UI plane.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct Light {
    /// x, y, z, source radius
    pub pos: [f32; 4],
    /// linear rgb × intensity, range
    pub color: [f32; 4],
    /// spot axis (unit), half-angle (0 = point)
    pub axis: [f32; 4],
    /// shafts, bulb glow, specular, softness
    pub extra: [f32; 4],
}

impl Light {
    pub fn point(x: f32, y: f32, z: f32) -> Light {
        Light {
            pos: [x, y, z, 12.0],
            color: [1.0, 1.0, 1.0, 2400.0],
            axis: [0.0, 0.0, -1.0, 0.0],
            extra: [0.0, 0.0, 1.0, 1.0],
        }
    }

    /// Linear colour; intensity is separate.
    pub fn color(mut self, rgb: [f32; 3]) -> Light {
        let i = self.intensity_of();
        self.color[..3].copy_from_slice(&rgb);
        self.intensity(i)
    }

    /// Irradiance 1.0 at 200 device px from the source (before n·l).
    pub fn intensity(mut self, i: f32) -> Light {
        let cur = self.intensity_of();
        let s = if cur > 0.0 { i / cur } else { 0.0 };
        if cur > 0.0 {
            for c in &mut self.color[..3] {
                *c *= s;
            }
        } else {
            self.color[..3].copy_from_slice(&[i, i, i]);
        }
        self
    }

    fn intensity_of(&self) -> f32 {
        self.color[0].max(self.color[1]).max(self.color[2])
    }

    /// Source radius: bigger sources throw softer, wider penumbrae and broader highlights.
    pub fn radius(mut self, r: f32) -> Light {
        self.pos[3] = r;
        self
    }

    /// Where the light fades to nothing (windowed inverse square).
    pub fn range(mut self, r: f32) -> Light {
        self.color[3] = r;
        self
    }

    /// Aim a torch-profile cone (Arcade's Outbreak beam) along `axis` with `half_angle` radians.
    pub fn spot(mut self, axis: [f32; 3], half_angle: f32) -> Light {
        let l = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt().max(1e-6);
        self.axis = [axis[0] / l, axis[1] / l, axis[2] / l, half_angle];
        self
    }

    /// God rays: in-scatter strength of the air term (flatland wedges at the light's height).
    pub fn shafts(mut self, s: f32) -> Light {
        self.extra[0] = s;
        self
    }

    /// Show the source itself as a soft bloom.
    pub fn bulb(mut self, g: f32) -> Light {
        self.extra[1] = g;
        self
    }

    pub fn specular(mut self, s: f32) -> Light {
        self.extra[2] = s;
        self
    }

    /// Multiplies the radius for shadows only.
    pub fn softness(mut self, s: f32) -> Light {
        self.extra[3] = s;
        self
    }

    /// Scales the whole light (fade in/out: lights cool, never cut).
    pub fn scaled(mut self, k: f32) -> Light {
        for c in &mut self.color[..3] {
            *c *= k;
        }
        self.extra[0] *= k;
        self.extra[1] *= k;
        self
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TextPolicy {
    /// Text never gets darker than this share of its design colour.
    pub floor: f32,
    /// Nor brighter than this.
    pub ceiling: f32,
    /// 0 = text ignores lighting, 1 = text takes the surface's light (clamped).
    pub lit: f32,
    /// Share of the surface highlight that shows over glyphs.
    pub specular: f32,
}

impl Default for TextPolicy {
    fn default() -> Self {
        TextPolicy { floor: 0.82, ceiling: 1.25, lit: 1.0, specular: 0.15 }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Gi {
    /// Fluence → irradiance on surfaces.
    pub strength: f32,
    /// Fluence added as glow in the air (visible on dark backgrounds).
    pub air: f32,
    /// Share of the direct light a card re-emits (one bounce).
    pub bounce: f32,
    /// Absorption per gi texel inside cards (how far light bleeds into them).
    pub absorption: f32,
    /// Gain on [`Scene::emission`] (linear radiance per gi texel).
    pub emission: f32,
}

impl Default for Gi {
    fn default() -> Self {
        Gi { strength: 1.0, air: 0.12, bounce: 0.25, absorption: 0.35, emission: 1.0 }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    /// Linear ambient irradiance; the UI looks exactly as designed at 1.0.
    pub ambient: [f32; 3],
    pub exposure: f32,
    pub text: TextPolicy,
    pub gi: Option<Gi>,
    pub background_gloss: f32,
    pub background_roughness: f32,
    /// Contact darkening on the floor around raised cards.
    pub contact_ao: f32,
    /// Hairline rim catch on occluder edges facing a light.
    pub rim: f32,
    /// God-ray strength over occluders relative to the gaps.
    pub shafts_over: f32,
    /// Added to the UI colour as albedo for direct light and GI only (linear). Dark UI colours are nearly black as
    /// materials; this lets light pools and shadows read on them while ambient keeps the designed look.
    pub albedo_floor: f32,
    /// Seed for the dither pattern; output is a pure function of inputs, time and seed.
    pub seed: f32,
    pub time: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            ambient: [0.55, 0.55, 0.58],
            exposure: 1.0,
            text: TextPolicy::default(),
            gi: None,
            background_gloss: 0.4,
            background_roughness: 0.55,
            contact_ao: 0.35,
            rim: 0.6,
            shafts_over: 0.35,
            albedo_floor: 0.025,
            seed: 0.0,
            time: 0.0,
        }
    }
}

/// What to light this frame.
pub struct Scene<'a> {
    pub prims: &'a [Prim],
    pub lights: &'a [Light],
    pub settings: Settings,
    /// Extra linear emission at output resolution (effect layers such as electric arcs) that enters the GI as
    /// glowing medium. Mark where it can be with [`Prim::gi_hint`] rects.
    pub emission: Option<&'a wgpu::TextureView>,
}

/// GPU time per pass in milliseconds (needs `Features::TIMESTAMP_QUERY`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Timings {
    pub field: f64,
    pub shadows: f64,
    pub gi: f64,
    /// GI breakdown: media from the primitives, each cascade (index = level), fluence.
    pub gi_scene: f64,
    pub cascades: [f64; CASCADES as usize],
    pub fluence: f64,
    /// Surface buffer + composite render passes (frame total minus the compute passes).
    pub raster: f64,
    /// First pass start to last pass end.
    pub total: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameU {
    sf: [f32; 4],
    gs: [f32; 4],
    ambient: [f32; 4],
    counts: [u32; 4],
    misc: [f32; 4],
    text: [f32; 4],
    gi: [f32; 4],
    bg: [f32; 4],
    ext: [f32; 4],
}

pub const CASCADES: u32 = 5;
const QUERIES: u32 = 12 + 2 * CASCADES;
const GI_SCALE: f32 = 0.125;

pub struct LightRenderer {
    pub width: u32,
    pub height: u32,
    field_size: (u32, u32),
    gi_size: (u32, u32),
    frame: wgpu::Buffer,
    prims: wgpu::Buffer,
    prim_cap: usize,
    lights: wgpu::Buffer,
    cascade_u: Vec<wgpu::Buffer>,
    own_u: Vec<wgpu::Buffer>,
    sampler: wgpu::Sampler,
    gbuf_pipe: wgpu::RenderPipeline,
    gbuf_bgl: wgpu::BindGroupLayout,
    gbuf_bg: wgpu::BindGroup,
    field_pipe: wgpu::ComputePipeline,
    field_bgl: wgpu::BindGroupLayout,
    shadow_pipe: wgpu::ComputePipeline,
    shadow_bgl: wgpu::BindGroupLayout,
    gi_bg0: wgpu::BindGroup,
    scene_pipe: wgpu::ComputePipeline,
    scene_bgl: wgpu::BindGroupLayout,
    cascade_pipe: wgpu::ComputePipeline,
    cascade_bgl: wgpu::BindGroupLayout,
    fluence_pipe: wgpu::ComputePipeline,
    fluence_bgl: wgpu::BindGroupLayout,
    comp_pipe: wgpu::RenderPipeline,
    noop_pipe: wgpu::ComputePipeline,
    comp_bgl: wgpu::BindGroupLayout,
    black: wgpu::TextureView,
    t: Targets,
    query: Option<(wgpu::QuerySet, wgpu::Buffer, wgpu::Buffer)>,
    period: f32,
    staging: rune_staging::Staging,
    /// Formats the composite can write; float formats get linear light, other non-sRGB formats get a manual sRGB
    /// encode + dither.
    pub output_format: wgpu::TextureFormat,
    /// Also trace every cascade's own interval without the merge, for inspection ([`LightRenderer::inspect`]).
    pub inspect: bool,
}

/// The GI pass's intermediate buffers, for inspection and visualisation. All are `Rgba16Float`.
pub struct Inspect<'a> {
    /// Half resolution: x occluder distance, y/z nearest occluder's z range, w distance to anything the GI sees.
    pub field: &'a wgpu::TextureView,
    /// gi resolution: rgb σ·L of emitters, a σ.
    pub emission: &'a wgpu::TextureView,
    /// gi resolution: rgb coverage × bounce radiance, a card coverage.
    pub occluders: &'a wgpu::TextureView,
    /// Per level, merged with every level above: probe (px, py) direction d at texel (px·b + d mod b, py·b + d / b),
    /// b = 2^(n+1); rgb radiance, a transmittance.
    pub merged: &'a [wgpu::TextureView],
    /// Per level, its own interval only (valid when `inspect` was on for the frame).
    pub own: &'a [wgpu::TextureView],
    /// gi resolution fluence (cascade 0's directions averaged).
    pub fluence: &'a wgpu::TextureView,
    /// Probe grid of cascade 0 in gi texels.
    pub gi_size: (u32, u32),
    /// Output px per gi texel.
    pub texel: f32,
    pub field_size: (u32, u32),
}

impl LightRenderer {
    /// `width`×`height` device px output in `output_format`. Timestamps are recorded when the device has
    /// `Features::TIMESTAMP_QUERY`.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32, output_format: wgpu::TextureFormat) -> LightRenderer {
        use wgpu::ShaderStages as S;
        let c = S::COMPUTE;
        let f = S::FRAGMENT;
        let vf = S::VERTEX_FRAGMENT;
        let frame = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("light frame"),
            size: std::mem::size_of::<FrameU>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let prim_cap = 256;
        let prims = Self::prim_buffer(device, prim_cap);
        let lights = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lights"),
            size: (MAX_LIGHTS * std::mem::size_of::<Light>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cascade_buf = |n: u32, merge: u32| {
            let b = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("cascade"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&b, 0, bytemuck::cast_slice(&[n, CASCADES, merge, 0]));
            b
        };
        let cascade_u: Vec<wgpu::Buffer> = (0..CASCADES).map(|n| cascade_buf(n, 1)).collect();
        let own_u: Vec<wgpu::Buffer> = (0..CASCADES).map(|n| cascade_buf(n, 0)).collect();
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("light linear"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let gbuf_bgl = layout(device, "gbuf", &[bgl::uniform(0, vf), bgl::storage(1, vf)]);
        let gm = module(device, "light gbuffer", include_str!("shaders/gbuffer.wgsl"));
        let gpl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&gbuf_bgl)], immediate_size: 0 });
        let gbuf_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("light gbuffer"),
            layout: Some(&gpl),
            vertex: wgpu::VertexState { module: &gm, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &gm,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[
                    Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba16Float, blend: None, write_mask: wgpu::ColorWrites::ALL }),
                    Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::R32Uint, blend: None, write_mask: wgpu::ColorWrites::ALL }),
                ],
            }),
            multiview_mask: None,
            cache: None,
        });

        let field_bgl = layout(device, "field", &[bgl::uniform(0, c), bgl::storage(1, c), bgl::store(2, wgpu::TextureViewDimension::D2)]);
        let fm = module(device, "light field", include_str!("shaders/field.wgsl"));
        let field_pipe = compute(device, "light field", &fm, "main", &[&field_bgl]);

        let shadow_bgl = layout(
            device,
            "shadow",
            &[
                bgl::uniform(0, c),
                bgl::storage(1, c),
                bgl::f2(2, c),
                bgl::sampler(3, c),
                bgl::f2(4, c),
                bgl::store(5, wgpu::TextureViewDimension::D2Array),
                bgl::store(6, wgpu::TextureViewDimension::D2Array),
            ],
        );
        let sm = module(device, "light shadow", include_str!("shaders/shadow.wgsl"));
        let shadow_pipe = compute(device, "light shadow", &sm, "main", &[&shadow_bgl]);

        let gi_bgl0 = layout(device, "gi0", &[bgl::uniform(0, c)]);
        let scene_bgl = layout(
            device,
            "gi scene",
            &[
                bgl::storage(0, c),
                bgl::storage(1, c),
                bgl::f2a(2, c),
                bgl::f2(3, c),
                bgl::sampler(4, c),
                bgl::store(5, wgpu::TextureViewDimension::D2),
                bgl::store(6, wgpu::TextureViewDimension::D2),
            ],
        );
        let scene_pipe = compute(device, "gi scene", &module(device, "gi scene", &gi_section("scene")), "scene", &[&gi_bgl0, &scene_bgl]);
        let cascade_bgl = layout(
            device,
            "gi cascade",
            &[
                bgl::uniform(0, c),
                bgl::f2(1, c),
                bgl::f2(2, c),
                bgl::f2(3, c),
                bgl::sampler(4, c),
                bgl::f2(5, c),
                bgl::store(6, wgpu::TextureViewDimension::D2),
                bgl::f2(7, c),
            ],
        );
        let cascade_pipe =
            compute(device, "gi cascade", &module(device, "gi cascade", &gi_section("cascades")), "cascade", &[&gi_bgl0, &cascade_bgl]);
        let fluence_bgl = layout(device, "gi fluence", &[bgl::f2(0, c), bgl::store(1, wgpu::TextureViewDimension::D2)]);
        let fluence_pipe =
            compute(device, "gi fluence", &module(device, "gi fluence", &gi_section("fluence")), "fluence", &[&gi_bgl0, &fluence_bgl]);

        let comp_bgl = layout(
            device,
            "composite",
            &[
                bgl::uniform(0, f),
                bgl::storage(1, f),
                bgl::storage(2, f),
                bgl::f2(3, f),
                bgl::f2(4, f),
                bgl::f2(5, f),
                bgl::u2(6, f),
                bgl::f2a(7, f),
                bgl::f2a(8, f),
                bgl::f2(9, f),
                bgl::f2(10, f),
                bgl::sampler(11, f),
            ],
        );
        let cm = module(device, "light composite", include_str!("shaders/composite.wgsl"));
        let cpl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&comp_bgl)], immediate_size: 0 });
        let comp_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("light composite"),
            layout: Some(&cpl),
            vertex: wgpu::VertexState { module: &cm, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &cm,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: output_format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let noop_pipe = compute(device, "light ts marker", &module(device, "noop", "@compute @workgroup_size(1) fn noop() {}"), "noop", &[]);
        let gbuf_bg = bind(device, &gbuf_bgl, &[frame.as_entire_binding(), prims.as_entire_binding()]);
        let gi_bg0 = bind(device, &gi_bgl0, &[frame.as_entire_binding()]);
        let black = texture(device, "light black", 1, 1, 1, wgpu::TextureFormat::Rgba16Float, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &black, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &[0u8; 8],
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(8), rows_per_image: Some(1) },
            wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        );
        let black = black.create_view(&Default::default());

        let query = device.features().contains(wgpu::Features::TIMESTAMP_QUERY).then(|| {
            let qs = device.create_query_set(&wgpu::QuerySetDescriptor { label: Some("light ts"), ty: wgpu::QueryType::Timestamp, count: QUERIES });
            let resolve = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("light ts resolve"),
                size: QUERIES as u64 * 8,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let read = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("light ts read"),
                size: QUERIES as u64 * 8,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            (qs, resolve, read)
        });

        let field_size = (width.div_ceil(2), height.div_ceil(2));
        let gi_size = ((width.div_ceil(8)).div_ceil(16) * 16, (height.div_ceil(8)).div_ceil(16) * 16);
        let t = Self::targets(
            device,
            width,
            height,
            field_size,
            gi_size,
            &frame,
            &prims,
            &lights,
            &cascade_u,
            &sampler,
            &black,
            [&field_bgl, &shadow_bgl, &cascade_bgl, &fluence_bgl],
        );
        LightRenderer {
            width,
            height,
            field_size,
            gi_size,
            frame,
            prims,
            prim_cap,
            lights,
            cascade_u,
            own_u,
            sampler,
            gbuf_pipe,
            gbuf_bgl,
            gbuf_bg,
            field_pipe,
            field_bgl,
            shadow_pipe,
            shadow_bgl,
            gi_bg0,
            scene_pipe,
            scene_bgl,
            cascade_pipe,
            cascade_bgl,
            fluence_pipe,
            fluence_bgl,
            comp_pipe,
            noop_pipe,
            comp_bgl,
            black,
            t,
            query,
            period: queue.get_timestamp_period(),
            staging: rune_staging::Staging::new(device, queue),
            output_format,
            inspect: false,
        }
    }

    fn prim_buffer(device: &wgpu::Device, cap: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("light prims"),
            size: (cap * std::mem::size_of::<Prim>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Blocking read of the last submitted frame's pass times. Call after the encoder from `render` was submitted.
    pub fn read_timings(&self, device: &wgpu::Device) -> Option<Timings> {
        let (_, _, read) = self.query.as_ref()?;
        let slice = read.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map timestamps"));
        device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        let ticks: Vec<u64> = {
            let data = slice.get_mapped_range().ok()?;
            bytemuck::cast_slice(&data).to_vec()
        };
        read.unmap();
        let ms = |a: u32, b: u32| ticks[b as usize].saturating_sub(ticks[a as usize]) as f64 * self.period as f64 / 1e6;
        let (field, shadows, total) = (ms(1, 2), ms(3, 4), ms(0, QUERIES - 1));
        let gi_scene = ms(5, 6);
        let mut cascades = [0.0; CASCADES as usize];
        for (n, c) in cascades.iter_mut().enumerate() {
            *c = ms(7 + 2 * n as u32, 8 + 2 * n as u32);
        }
        let fluence = ms(7 + 2 * CASCADES, 8 + 2 * CASCADES);
        let gi = gi_scene + cascades.iter().sum::<f64>() + fluence;
        // The surface and composite render passes overlap the compute work on tile GPUs, so they are reported
        // together as the rest of the frame.
        Some(Timings { field, shadows, gi, gi_scene, cascades, fluence, raster: (total - field - shadows - gi).max(0.0), total })
    }

    /// The half-resolution visibility texture (2 layers × 4 lights), for debugging and the CPU reference check.
    pub fn visibility_texture(&self) -> &wgpu::Texture {
        &self.t.vis_tex
    }

    pub fn field_size(&self) -> (u32, u32) {
        self.field_size
    }

    /// The GI pass's intermediate buffers from the last recorded frame.
    pub fn inspect(&self) -> Inspect<'_> {
        Inspect {
            field: &self.t.field,
            emission: &self.t.scene_emit,
            occluders: &self.t.scene_occ,
            merged: &self.t.levels,
            own: &self.t.own,
            fluence: &self.t.fluence,
            gi_size: self.gi_size,
            texel: 1.0 / GI_SCALE,
            field_size: self.field_size,
        }
    }

    /// The surface buffer: x own signed distance, yz outward gradient, w top z (output resolution).
    pub fn surface_view(&self) -> &wgpu::TextureView {
        &self.t.gb0
    }
}
