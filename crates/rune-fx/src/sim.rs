//! Fire and smoke simulation: one small stable-fluids grid per simulated fire layer (shaders/firesim.wgsl).
//!
//! `FxRenderer::prepare` plans each grid's steps for the frame (fixed 1/60 s steps, capped, so the
//! look doesn't depend on frame rate), `FxRenderer::simulate` records them as compute dispatches,
//! and the fire layer samples the resulting state texture.

use crate::*;

pub(crate) const DT: f32 = 1.0 / 60.0;
const JACOBI: usize = 14;
/// Steps per frame while playing; beyond it the sim skips time instead of stalling the frame.
const MAX_STEPS: usize = 4;
/// A grid that starts (or goes back in time) warms up this long, so its first frame is already burning.
const WARMUP: f32 = 1.2;
const SLOTS: usize = 80;
const SLOT: usize = 256;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct SimUniform {
    dims: [f32; 4],
    origin: [f32; 4],
    shape: [f32; 4],
    shape2: [f32; 4],
    fire: [f32; 4],
    fire2: [f32; 4],
}

/// Which fire a grid serves: grids are matched by (seed, style, size) and serve one fire per frame.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Claim {
    key: (u32, u32, u32, u32),
    /// Where the fire's domain was when it last claimed the grid (pt).
    origin: [f32; 2],
    /// The frame it last claimed the grid.
    used: u64,
}

impl Claim {
    /// A grid whose fire wasn't drawn last frame is gone; its fire would start over anyway.
    fn alive(&self, frame: u64) -> bool {
        self.used + 1 >= frame
    }
}

/// The grid a fire with `key` at `origin` takes this frame: an unclaimed one with the same key,
/// the nearest to where it was, so identical fires each keep their own state. None: make one.
fn pick(claims: impl Iterator<Item = Claim>, key: (u32, u32, u32, u32), origin: [f32; 2], frame: u64) -> Option<usize> {
    let dist = |c: &Claim| (c.origin[0] - origin[0]).powi(2) + (c.origin[1] - origin[1]).powi(2);
    claims.enumerate().filter(|(_, c)| c.key == key && c.used != frame).min_by(|a, b| dist(&a.1).total_cmp(&dist(&b.1))).map(|(i, _)| i)
}

/// Steps to run for a grid last stepped to `prev` (None: never), to reach `target`:
/// (steps, sim time before the first, start over). A hitch runs at most `MAX_STEPS` and skips the
/// rest; only a new grid or a jump back in time warms up.
fn schedule(prev: Option<f32>, target: f32, frozen: bool) -> (usize, f32, bool) {
    match prev {
        Some(t) if frozen => (0, t, false),
        Some(t) if target >= t - 1e-4 => {
            let due = ((target - t) / DT + 1e-3).floor() as usize;
            let steps = due.min(MAX_STEPS);
            (steps, target - steps as f32 * DT, false)
        }
        _ => {
            let steps = ((WARMUP / DT).round() as usize).min(SLOTS);
            (steps, target - steps as f32 * DT, true)
        }
    }
}

pub(crate) struct Grid {
    claim: Claim,
    n: [u32; 2],
    pub views: [wgpu::TextureView; 2],
    binds: [[wgpu::BindGroup; 7]; 2],
    uniforms: wgpu::Buffer,
    /// Which state texture holds the latest step.
    pub cur: usize,
    /// Sim time of `views[cur]`; None until the first step.
    t: Option<f32>,
    plan: Option<Plan>,
}

#[derive(Clone, Copy)]
struct Plan {
    steps: usize,
    t0: f32,
    reset: bool,
    u: SimUniform,
}

pub(crate) struct Sims {
    pipes: [wgpu::ComputePipeline; 6],
    bgl: wgpu::BindGroupLayout,
    pub grids: Vec<Grid>,
    frame: u64,
}

const CURL: usize = 0;
const FORCES: usize = 1;
const DIV: usize = 2;
const JAC: usize = 3;
const PROJECT: usize = 4;
const ADVECT: usize = 5;

impl Sims {
    pub fn new(device: &wgpu::Device) -> Sims {
        let cs = wgpu::ShaderStages::COMPUTE;
        let e = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: cs, ty, count: None };
        let tex = wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        };
        let store = |format| wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format,
            view_dimension: wgpu::TextureViewDimension::D2,
        };
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fx firesim"),
            entries: &[
                e(0, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: None }),
                e(1, tex),
                e(3, store(wgpu::TextureFormat::Rgba16Float)),
                e(4, tex),
                e(5, tex),
                e(6, store(wgpu::TextureFormat::R32Float)),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fx firesim"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fx firesim"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/firesim.wgsl").into()),
        });
        let pipe = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        Sims {
            pipes: [pipe("curl"), pipe("forces"), pipe("divergence"), pipe("jacobi"), pipe("project"), pipe("advect")],
            bgl,
            grids: Vec::new(),
            frame: 0,
        }
    }

    pub fn begin_frame(&mut self) {
        self.frame += 1;
        let f = self.frame;
        self.grids.retain(|g| g.claim.alive(f));
        for g in &mut self.grids {
            g.plan = None;
        }
    }

    /// Find or make the grid for a fire layer covering `domain` (pt) and plan this frame's steps.
    /// Call once per fire per frame, after `begin_frame`.
    /// Returns the grid index (see `final_view`) and the grid's extent in pt (whole cells, so a
    /// little larger than `domain`).
    pub fn plan(&mut self, device: &wgpu::Device, layer: &FxLayer, p: &FireParams, domain: Rect, frozen: bool) -> (usize, [f32; 2]) {
        let area = (domain.w * domain.h).max(1.0);
        let cell = (area / 16_000.0).sqrt().clamp(1.0, 3.0);
        let n = [((domain.w / cell).ceil() as u32).max(8), ((domain.h / cell).ceil() as u32).max(8)];
        let style = p.style as u32;
        let key = (layer.seed, style, n[0], n[1]);
        let origin = [domain.x, domain.y];
        let claim = Claim { key, origin, used: self.frame };
        let idx = match pick(self.grids.iter().map(|g| g.claim), key, origin, self.frame) {
            Some(i) => i,
            None => {
                self.grids.push(self.grid(device, n, claim));
                self.grids.len() - 1
            }
        };
        let g = &mut self.grids[idx];
        g.claim = claim;
        let (steps, t0, reset) = schedule(g.t, layer.time, frozen);
        let s = layer.shape;
        let r = s.rect;
        let speed = (p.speed / 1.5).max(0.05);
        // Rise speed and buoyancy set how fast it climbs; cooling sets where the visible tips end
        // (the flame is visible until T falls to ~0.15, after about 1.9 / cooling seconds).
        let rise = (p.height * 2.2).clamp(20.0, 140.0) * speed.sqrt();
        // Visible until T falls to ~0.35: ln(1 / 0.35) / (height / rise).
        let cooling = 1.15 * rise / p.height.max(4.0);
        let u = SimUniform {
            dims: [n[0] as f32, n[1] as f32, cell, DT],
            origin: [domain.x, domain.y, 0.0, 0.0],
            shape: [r.x + r.w * 0.5, r.y + r.h * 0.5, r.w * 0.5, r.h * 0.5],
            shape2: [s.radius, (layer.seed % 65_536) as f32, rise, p.scale.max(2.0)],
            fire: [rise * 3.0, cooling, p.smoke.max(0.0) * 0.6, 0.25 + 0.6 * p.warp],
            fire2: [rise * (0.3 + 1.5 * p.swirl), speed, 1.4, 2.0],
        };
        g.plan = Some(Plan { steps, t0, reset, u });
        (idx, [n[0] as f32 * cell, n[1] as f32 * cell])
    }

    /// The state view a grid's layer should sample this frame (after its planned steps).
    pub fn final_view(&self, i: usize) -> &wgpu::TextureView {
        let g = &self.grids[i];
        let steps = g.plan.map_or(0, |p| p.steps);
        &g.views[g.cur ^ (steps & 1)]
    }

    pub fn pending(&self) -> bool {
        self.grids.iter().any(|g| g.plan.is_some_and(|p| p.steps > 0))
    }

    pub fn record(&mut self, staging: Option<&rune_staging::Staging>, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, timestamps: Option<wgpu::ComputePassTimestampWrites<'_>>) -> bool {
        if !self.pending() {
            return false;
        }
        for g in &self.grids {
            let Some(plan) = g.plan.filter(|p| p.steps > 0) else { continue };
            let mut bytes = vec![0u8; plan.steps * SLOT];
            for k in 0..plan.steps {
                let mut u = plan.u;
                u.origin[2] = plan.t0 + (k + 1) as f32 * DT;
                u.origin[3] = if plan.reset && k == 0 { 1.0 } else { 0.0 };
                bytes[k * SLOT..k * SLOT + std::mem::size_of::<SimUniform>()].copy_from_slice(bytemuck::bytes_of(&u));
            }
            match staging {
                Some(s) => s.write_buffer(&g.uniforms, 0, &bytes),
                None => queue.write_buffer(&g.uniforms, 0, &bytes),
            }
        }
        if let Some(s) = staging { s.flush(encoder); }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("fx firesim"), timestamp_writes: timestamps });
        for g in &mut self.grids {
            let Some(plan) = g.plan.filter(|p| p.steps > 0) else { continue };
            let wg = [g.n[0].div_ceil(8), g.n[1].div_ceil(8)];
            for k in 0..plan.steps {
                let off = (k * SLOT) as u32;
                let b = &g.binds[g.cur];
                let mut run = |pipe: usize, bind: &wgpu::BindGroup| {
                    pass.set_pipeline(&self.pipes[pipe]);
                    pass.set_bind_group(0, bind, &[off]);
                    pass.dispatch_workgroups(wg[0], wg[1], 1);
                };
                run(CURL, &b[0]);
                run(FORCES, &b[1]);
                run(DIV, &b[2]);
                for j in 0..JACOBI {
                    run(JAC, &b[3 + (j & 1)]);
                }
                run(PROJECT, &b[5]);
                run(ADVECT, &b[6]);
                g.cur ^= 1;
            }
            g.t = Some(plan.t0 + plan.steps as f32 * DT);
            g.plan = Some(Plan { steps: 0, ..plan });
        }
        true
    }

    fn grid(&self, device: &wgpu::Device, n: [u32; 2], claim: Claim) -> Grid {
        let tex = |label, format| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: n[0], height: n[1], depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        let views = [
            tex("fx sim state a", wgpu::TextureFormat::Rgba16Float).create_view(&Default::default()),
            tex("fx sim state b", wgpu::TextureFormat::Rgba16Float).create_view(&Default::default()),
        ];
        // Scalars: curl, divergence, pressure ping-pong.
        let r: Vec<wgpu::TextureView> = ["fx sim curl", "fx sim div", "fx sim p0", "fx sim p1"]
            .iter()
            .map(|l| tex(l, wgpu::TextureFormat::R32Float).create_view(&Default::default()))
            .collect();
        let (curl, div, p0, p1) = (&r[0], &r[1], &r[2], &r[3]);
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fx sim params"),
            size: (SLOTS * SLOT) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = |s_in: &wgpu::TextureView, s_out: &wgpu::TextureView, a: &wgpu::TextureView, b: &wgpu::TextureView, out: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fx firesim"),
                layout: &self.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &uniforms, offset: 0, size: wgpu::BufferSize::new(std::mem::size_of::<SimUniform>() as u64) }),
                    },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(s_in) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(s_out) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(a) },
                    wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(b) },
                    wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(out) },
                ],
            })
        };
        // Starting from state X (other one Y), a step ends in Y:
        // curl X->c, forces X->Y, div Y->d, jacobi p0<->p1 (even count: ends in p0), project Y->X, advect X->Y.
        let set = |x: &wgpu::TextureView, y: &wgpu::TextureView| {
            [
                bind(x, y, p1, div, curl),
                bind(x, y, curl, div, p1),
                bind(y, x, curl, p0, div),
                bind(y, x, p0, div, p1),
                bind(y, x, p1, div, p0),
                bind(y, x, p0, div, curl),
                bind(x, y, curl, div, p1),
            ]
        };
        let binds = [set(&views[0], &views[1]), set(&views[1], &views[0])];
        Grid { claim, n, views, binds, uniforms, cur: 0, t: None, plan: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Sims::plan`'s bookkeeping without the GPU.
    fn claim(grids: &mut Vec<Claim>, key: (u32, u32, u32, u32), origin: [f32; 2], frame: u64) -> usize {
        let i = pick(grids.iter().copied(), key, origin, frame).unwrap_or_else(|| {
            grids.push(Claim { key, origin, used: frame });
            grids.len() - 1
        });
        grids[i] = Claim { key, origin, used: frame };
        i
    }

    #[test]
    fn identical_fires_get_their_own_grids_and_free_them() {
        let key = (1, 0, 40, 30);
        let mut grids = Vec::new();
        let (a, b) = (claim(&mut grids, key, [0.0, 0.0], 1), claim(&mut grids, key, [300.0, 0.0], 1));
        assert_ne!(a, b, "same seed and size, two grids");
        // Next frame, listed the other way round and moved a little: each keeps its grid.
        assert_eq!(claim(&mut grids, key, [302.0, 1.0], 2), b);
        assert_eq!(claim(&mut grids, key, [1.0, 0.0], 2), a);
        assert_eq!(grids.len(), 2);
        // The second fire goes away: its grid is dropped at the next frame, the first's stays.
        claim(&mut grids, key, [1.0, 0.0], 3);
        grids.retain(|c| c.alive(4));
        assert_eq!(grids.len(), 1);
        assert_eq!(grids[0].origin, [1.0, 0.0]);
    }

    #[test]
    fn hitches_skip_time_and_only_new_or_rewound_grids_warm_up() {
        let warm = (WARMUP / DT).round() as usize;
        let (n, _, reset) = schedule(None, 3.0, false);
        assert!(reset && n == warm);
        let (n, t0, reset) = schedule(Some(1.0), 1.0 + 2.5 * DT, false);
        assert!(!reset && n == 2 && (t0 - (1.0 + 0.5 * DT)).abs() < 1e-5);
        let (n, t0, reset) = schedule(Some(1.0), 3.0, false);
        assert!(!reset && n == MAX_STEPS && (t0 + MAX_STEPS as f32 * DT - 3.0).abs() < 1e-5, "a 2 s hitch: {n} steps");
        assert!(schedule(Some(2.0), 1.0, false).2, "back in time starts over");
        assert_eq!(schedule(Some(1.0), 3.0, true).0, 0, "frozen");
    }
}
