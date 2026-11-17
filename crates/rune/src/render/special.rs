//! Measuring only (frame-profile builds): what the uber shader's generality costs each fragment.
//! `RUNE_CUT=wgsl` draws the frame's instances with Windfoil's WGSL compiled now instead of the
//! shipped bytecode (the like-for-like base); `RUNE_CUT=special` also splits each batch into runs
//! of one kind and draws rects, shadows and glyphs with that WGSL's `fs` pinned to their kind, so
//! the compiler drops every other kind's branch. Not for merge.

use super::{Blend, Renderer, dxbc};

pub(super) struct Special {
    pub(super) generic: wgpu::RenderPipeline,
    /// By kind (`KIND_*`): the pinned pipeline, for the kinds that have one.
    pub(super) by_kind: Vec<Option<wgpu::RenderPipeline>>,
}

/// The kinds `special` pins: rect, shadow, glyph.
const PINNED: [u32; 3] = [0, 1, 2];

impl Renderer {
    /// Builds the measuring pipelines once, when `RUNE_CUT` asks for them.
    pub(super) fn build_special(&mut self, device: &wgpu::Device) {
        if self.special.is_some() || !(Renderer::cut("wgsl") || Renderer::cut("special")) { return; }
        let source = super::variant::windfoil_wgsl();
        let entry = "@fragment\nfn fs(i: Out) -> @location(0) vec4<f32> {";
        let Some(at) = source.find(entry) else { eprintln!("rune: RUNE_CUT=special: no fs entry"); return };
        let make = |wgsl: &str| {
            let shader = dxbc::Shaders::one(device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("uber (measuring)"), source: wgpu::ShaderSource::Wgsl(wgsl.into()) }));
            Self::pipeline(device, &self.layout, &shader, self.format, "vs", Blend::SourceOver)
        };
        let started = std::time::Instant::now();
        let generic = make(source);
        let by_kind = (0..16).map(|k| PINNED.contains(&k).then(|| {
            // Only the fragment entry's reads of the kind: the vertex stage still reads its own.
            let (head, fs) = source.split_at(at);
            let pinned = fs.replacen("if (i.params.y == 8.0) {", &format!("if ({k}.0 == 8.0) {{"), 1)
                .replacen("let kind = i.params.y;", &format!("let kind = {k}.0;"), 1);
            make(&format!("{head}{pinned}"))
        })).collect();
        eprintln!("rune: RUNE_CUT measuring pipelines compiled in {:.0} ms", started.elapsed().as_secs_f64() * 1e3);
        self.special = Some(Special { generic, by_kind });
    }

    /// The measuring pipelines for this draw, when asked for and the frame's own pipeline would draw.
    pub(super) fn special_for(&self, format: wgpu::TextureFormat, target: Option<usize>) -> Option<&Special> {
        let own = std::ptr::eq(self.draw_pipeline(format, target), &self.pipeline);
        self.special.as_ref().filter(|_| own && target.is_none() && format == self.format)
    }
}

/// `range` split into runs of instances of one kind (`kinds` by instance), each with the pipeline
/// that draws it: the pinned one under `special`, else the generic one.
pub(super) fn runs<'a>(special: &'a Special, kinds: &[u8], range: std::ops::Range<u32>) -> Vec<(std::ops::Range<u32>, &'a wgpu::RenderPipeline)> {
    let pin = Renderer::cut("special");
    let pipeline = |k: u8| if pin { special.by_kind.get(usize::from(k)).and_then(Option::as_ref).unwrap_or(&special.generic) } else { &special.generic };
    let mut out: Vec<(std::ops::Range<u32>, &wgpu::RenderPipeline)> = Vec::new();
    for i in range {
        let p = pipeline(kinds.get(i as usize).copied().unwrap_or(255));
        match out.last_mut() {
            Some((r, last)) if std::ptr::eq(*last, p) && r.end == i => r.end = i + 1,
            _ => out.push((i..i + 1, p)),
        }
    }
    out
}
