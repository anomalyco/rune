//! Measuring only (frame-profile builds): what the uber shader's generality costs each fragment.
//! `RUNE_CUT=wgsl` draws the frame's instances with Windfoil's WGSL compiled now instead of the
//! shipped bytecode (the like-for-like base); `RUNE_CUT=special` also splits each batch into runs
//! of one kind and draws rects, shadows and glyphs with that WGSL's `fs` pinned to their kind, so
//! the compiler drops every other kind's branch. They compile on a thread (seconds each from
//! WGSL); frames draw as shipped until they're in, and the log says when. Not for merge.

use super::{Blend, Renderer, dxbc};

pub(super) struct Special {
    pub(super) generic: wgpu::RenderPipeline,
    /// By kind (`KIND_*`): the pinned pipeline, for the kinds that have one.
    pub(super) by_kind: Vec<Option<wgpu::RenderPipeline>>,
    /// Plain opaque quads (kind code `PLAIN` in `special_kinds`) under `RUNE_CUT=special,plain`.
    pub(super) plain: Option<wgpu::RenderPipeline>,
}

/// `special_kinds`' code for a plain opaque rect.
pub(super) const PLAIN: u8 = 16;

/// The kinds `special` pins: rect, shadow, glyph.
const PINNED: [u32; 3] = [0, 1, 2];

impl Renderer {
    /// Starts compiling the measuring pipelines once, when `RUNE_CUT` asks for them, and takes them
    /// when they're done.
    pub(super) fn build_special(&mut self, device: &wgpu::Device) {
        if let Some(rx) = &self.special_rx && let Ok(built) = rx.try_recv() { self.special = Some(built); self.special_rx = None; }
        if self.special.is_some() || self.special_rx.is_some() || !(Renderer::cut("wgsl") || Renderer::cut("special")) { return; }
        let (device, layout, format) = (device.clone(), self.layout.clone(), self.format);
        let (sent, rx) = std::sync::mpsc::channel();
        self.special_rx = Some(rx);
        std::thread::spawn(move || {
            let source = super::variant::windfoil_wgsl();
            let entry = "@fragment\nfn fs(i: Out) -> @location(0) vec4<f32> {";
            let Some(at) = source.find(entry) else { eprintln!("rune: RUNE_CUT=special: no fs entry"); return };
            let make = |wgsl: &str| {
                let shader = dxbc::Shaders::one(device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("uber (measuring)"), source: wgpu::ShaderSource::Wgsl(wgsl.into()) }));
                Self::pipeline(&device, &layout, &shader, format, "vs", Blend::SourceOver)
            };
            let started = std::time::Instant::now();
            let generic = make(source);
            let by_kind = (0..16).map(|k| PINNED.contains(&k).then(|| make(&super::variant::pinned_wgsl(k).expect("PINNED_KIND marker")))).collect();
            // GPUI's first switch on the timeline: plain opaque quads (square, borderless, no fade or clip radius)
            // drawn as their fill under the soft clip, before any rounded-rect maths.
            let plain = super::variant::pinned_wgsl(0).filter(|w| w.matches("    let kind = fragment_kind(i);\n").count() == 1).map(|w| w.replacen("    let kind = fragment_kind(i);\n", "    let kind = fragment_kind(i);\n    if (true) { return i.fill * clip_alpha(i.p, i.clip, i.fade); }\n", 1)).map(|w| make(&w));
            eprintln!("rune: RUNE_CUT measuring pipelines compiled in {:.0} ms", started.elapsed().as_secs_f64() * 1e3);
            sent.send(Special { generic, by_kind, plain }).ok();
        });
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
    let plain = Renderer::cut("plain");
    let pipeline = |k: u8| match k {
        PLAIN if pin && plain => special.plain.as_ref().unwrap_or(&special.generic),
        PLAIN => if pin { special.by_kind[0].as_ref().unwrap_or(&special.generic) } else { &special.generic },
        _ if pin => special.by_kind.get(usize::from(k)).and_then(Option::as_ref).unwrap_or(&special.generic),
        _ => &special.generic,
    };
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
