//! The effect pipelines: one per effect family, blend and target format, each built the first
//! time a frame draws it. On a cold Metal shader cache a pipeline costs 20 to 150 ms of GPU code
//! generation, a blend state or a format as much as a family, so a renderer that only ever glows
//! builds one pipeline, not all sixteen of every format. Those a frame needs at once build in
//! parallel. The mask pyramid's and the backdrop blur's passes (`Passes`) wait for first use too.

use std::sync::OnceLock;

use crate::FxBlend;
use crate::pack::KINDS;

const CLIP: &str = include_str!("../../rune/src/clip.wgsl");
const COMMON: &str = include_str!("shaders/common.wgsl");
const SOURCES: [(&str, &str); KINDS] = [
    ("glow", include_str!("shaders/glow.wgsl")),
    ("pulse", include_str!("shaders/pulse.wgsl")),
    ("electric", include_str!("shaders/electric.wgsl")),
    ("fire", include_str!("shaders/fire.wgsl")),
    ("distort", include_str!("shaders/distort.wgsl")),
    ("motion", include_str!("shaders/motion.wgsl")),
    ("extras", include_str!("shaders/extras.wgsl")),
    ("fill", include_str!("shaders/fill.wgsl")),
];

pub(crate) const BLENDS: usize = 2;

/// The pipeline index for `blend`: premultiplied over, or screen.
pub(crate) fn blend_index(blend: FxBlend) -> usize {
    match blend {
        FxBlend::Screen => 1,
        FxBlend::Normal | FxBlend::Multiply | FxBlend::Overlay | FxBlend::SoftLight => 0,
    }
}

fn blend_state(index: usize) -> wgpu::BlendState {
    if index == 0 {
        return wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING;
    }
    wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrc,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent::OVER,
    }
}

/// One target format's pipelines, by family and blend.
pub(crate) struct Target {
    pub(crate) format: wgpu::TextureFormat,
    built: [[OnceLock<wgpu::RenderPipeline>; BLENDS]; KINDS],
}

/// What every pipeline is built from, and the formats they're built for.
pub(crate) struct Pipes {
    device: wgpu::Device,
    layout: wgpu::PipelineLayout,
    modules: [OnceLock<wgpu::ShaderModule>; KINDS],
    targets: Vec<Target>,
}

impl Pipes {
    pub(crate) fn new(device: &wgpu::Device, layout: wgpu::PipelineLayout, format: wgpu::TextureFormat) -> Pipes {
        let mut pipes = Pipes { device: device.clone(), layout, modules: Default::default(), targets: Vec::new() };
        pipes.add(format);
        pipes
    }

    /// Makes room for `format`'s pipelines; none is built until a frame draws it.
    pub(crate) fn add(&mut self, format: wgpu::TextureFormat) {
        if !self.targets.iter().any(|t| t.format == format) {
            self.targets.push(Target { format, built: Default::default() });
        }
    }

    pub(crate) fn target(&self, format: wgpu::TextureFormat) -> Option<&Target> {
        self.targets.iter().find(|t| t.format == format)
    }

    /// `kind`'s pipeline for `target` and `blend` (an index from `blend_index`), built now if no
    /// frame has drawn it yet.
    pub(crate) fn get<'a>(&self, target: &'a Target, kind: usize, blend: usize) -> &'a wgpu::RenderPipeline {
        target.built[kind][blend].get_or_init(|| self.build(kind, blend, target.format))
    }

    /// Builds the pipelines `wanted` names for `format` that aren't built yet, in parallel when
    /// there's more than one.
    pub(crate) fn warm(&self, format: wgpu::TextureFormat, wanted: impl IntoIterator<Item = (usize, usize)>) {
        let Some(target) = self.target(format) else { return };
        let mut missing: Vec<(usize, usize)> = wanted.into_iter().filter(|&(k, b)| target.built[k][b].get().is_none()).collect();
        missing.sort_unstable();
        missing.dedup();
        in_parallel(&missing, |&(k, b)| {
            self.get(target, k, b);
        });
    }

    /// Every pipeline of every format added so far.
    pub(crate) fn warm_all(&self) {
        for t in &self.targets {
            self.warm(t.format, (0..KINDS).flat_map(|k| (0..BLENDS).map(move |b| (k, b))));
        }
    }

    /// How many pipelines are built, across formats.
    pub(crate) fn built(&self) -> usize {
        self.targets.iter().flat_map(|t| t.built.iter().flatten()).filter(|p| p.get().is_some()).count()
    }

    fn module(&self, kind: usize) -> &wgpu::ShaderModule {
        self.modules[kind].get_or_init(|| {
            let (name, src) = SOURCES[kind];
            self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(name),
                source: wgpu::ShaderSource::Wgsl(format!("{CLIP}\n{COMMON}\n{src}").into()),
            })
        })
    }

    fn build(&self, kind: usize, blend: usize, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
        let module = self.module(kind);
        self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(SOURCES[kind].0),
            layout: Some(&self.layout),
            vertex: wgpu::VertexState { module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: Some(blend_state(blend)), write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        })
    }
}

/// A full-screen pass's pipelines, one per fragment entry point, all built (in parallel) the
/// first time one is needed.
pub(crate) struct Passes<const N: usize> {
    device: wgpu::Device,
    layout: wgpu::PipelineLayout,
    label: &'static str,
    source: &'static str,
    entries: [&'static str; N],
    format: wgpu::TextureFormat,
    built: OnceLock<[wgpu::RenderPipeline; N]>,
}

impl<const N: usize> Passes<N> {
    pub(crate) fn new(device: &wgpu::Device, layout: wgpu::PipelineLayout, label: &'static str, source: &'static str, entries: [&'static str; N], format: wgpu::TextureFormat) -> Self {
        Passes { device: device.clone(), layout, label, source, entries, format, built: OnceLock::new() }
    }

    pub(crate) fn get(&self) -> &[wgpu::RenderPipeline; N] {
        self.built.get_or_init(|| {
            let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(self.label), source: wgpu::ShaderSource::Wgsl(self.source.into()) });
            let slots: [OnceLock<wgpu::RenderPipeline>; N] = std::array::from_fn(|_| OnceLock::new());
            let indices: [usize; N] = std::array::from_fn(|i| i);
            in_parallel(&indices, |&i| {
                let entry = self.entries[i];
                let pipeline = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(if N == 1 { self.label } else { entry }),
                    layout: Some(&self.layout),
                    vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                    primitive: Default::default(),
                    depth_stencil: None,
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some(entry),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState { format: self.format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                    }),
                    multiview_mask: None,
                    cache: None,
                });
                let _ = slots[i].set(pipeline);
            });
            slots.map(|s| s.into_inner().expect("built"))
        })
    }
}

/// Runs `f` on every item, the first on this thread and the rest on their own (Metal compiles
/// pipelines concurrently), or one after another where there are no threads.
fn in_parallel<T: Sync>(items: &[T], f: impl Fn(&T) + Sync) {
    #[cfg(not(target_arch = "wasm32"))]
    if let [first, rest @ ..] = items
        && !rest.is_empty()
    {
        let f = &f;
        std::thread::scope(|s| {
            for item in rest {
                s.spawn(move || f(item));
            }
            f(first);
        });
        return;
    }
    items.iter().for_each(f);
}
