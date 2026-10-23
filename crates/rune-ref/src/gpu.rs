//! Adapter to the production renderer. No copied WGSL or substitute GPU pipeline.
use crate::{Primitive, Scene};
use rune::{
    render::{Gpu, Instance, OFFSCREEN_FORMAT, Offscreen, Renderer},
    text::{ATLAS_SIZE, TextSystem},
};

impl Primitive {
    /// Capture an application's emitted primitive with its inspector metadata.
    pub fn from_instance(p: &Instance, node: u32, source: String) -> Self {
        let (fade, offset) = rune::render::unpack_fade(p.fade);
        Self {
            node,
            source,
            rect: p.rect,
            radii: p.radii,
            fill: p.fill,
            border: p.border,
            params: p.params,
            uv: p.uv,
            clip: p.clip,
            fade,
            fade_origin: Some(std::array::from_fn(|e| p.clip[e] + offset[e] * fade[e] * if e < 2 { -1.0 } else { 1.0 })),
        }
    }
}

impl From<&Primitive> for Instance {
    fn from(p: &Primitive) -> Self {
        Self {
            rect: p.rect,
            radii: p.radii,
            fill: p.fill,
            border: p.border,
            params: p.params,
            uv: p.uv,
            clip: p.clip,
            fade: rune::render::pack_fade(p.fade, p.fade_origin.unwrap_or(p.clip), p.clip),
        }
    }
}

pub struct GpuRenderer {
    gpu: Gpu,
    renderer: Renderer,
    text: TextSystem,
}
impl Default for GpuRenderer {
    fn default() -> Self {
        let gpu = Gpu::new(Gpu::instance(), None);
        eprintln!("reference adapter: {:?}", gpu.adapter.get_info());
        let renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        Self {
            gpu,
            renderer,
            text: TextSystem::default(),
        }
    }
}
impl GpuRenderer {
    pub fn render(&mut self, scene: &Scene) -> Vec<u8> {
        scene.validate().expect("valid differential scene");
        let atlas = &mut self.text.atlas;
        atlas.data.fill(0);
        for y in 0..scene.atlas.height as usize {
            let n = scene.atlas.width as usize;
            atlas.data[y * ATLAS_SIZE as usize..y * ATLAS_SIZE as usize + n]
                .copy_from_slice(&scene.atlas.data[y * n..(y + 1) * n]);
        }
        atlas.dirty = Some((0, ATLAS_SIZE));
        let instances: Vec<Instance> = scene.primitives.iter().map(Instance::from).collect();
        self.renderer.prepare(
            &self.gpu.device,
            &self.gpu.queue,
            atlas,
            &instances,
            (scene.width, scene.height),
        );
        let target = Offscreen::new(&self.gpu.device, scene.width, scene.height);
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        self.renderer.draw(&mut encoder, &target.view, scene.clear);
        target.copy(&mut encoder);
        self.gpu.queue.submit([encoder.finish()]);
        target.read(&self.gpu.device)
    }
}

/// Add a pinned-font text run, then freeze its atlas into the replay scene.
/// Shaping is deliberately outside the oracle: this lane tests glyph sampling.
pub fn text_run(scene: &mut Scene, seed: u64) {
    let mut text = TextSystem::default();
    let strings = ["Rune ffi", "Ág jW", "012345", "a\u{301} test"];
    let style = rune::tok::TextStyle::new(9.0 + (seed % 15) as f32, 400.0);
    let layout = text.layout_rich(
        strings[seed as usize % strings.len()],
        &[],
        style,
        1.0,
        Some(60.0),
    );
    let mut quads = Vec::new();
    text.glyphs_for_layout(&layout, 1.0, [2.0, 2.0], true, &mut quads);
    // Keep synthetic atlas texels used by earlier generated glyphs in a separate region.
    for p in &mut scene.primitives {
        if p.params[1] == 2.0 {
            p.uv[0] += 960.0;
            p.uv[1] += 960.0;
        }
    }
    for y in 0..scene.atlas.height.min(32) as usize {
        let n = scene.atlas.width.min(32) as usize;
        let start = (y + 960) * ATLAS_SIZE as usize + 960;
        text.atlas.data[start..start + n].copy_from_slice(
            &scene.atlas.data[y * scene.atlas.width as usize..y * scene.atlas.width as usize + n],
        );
    }
    let first = scene.primitives.len() as u32 + 1;
    for (n, q) in quads.iter().enumerate() {
        scene.primitives.push(Primitive {
            node: first + n as u32,
            source: format!("text/{seed}/byte-{}", q.byte),
            rect: q.rect,
            radii: [0.0; 4],
            fill: [0.7, 0.6, 0.5, 0.8],
            border: [0.0; 4],
            params: [0.0, 2.0, 0.0, 0.85],
            uv: [q.uv[0], q.uv[1], 0.0, 0.0],
            clip: crate::NO_CLIP,
            fade: [0.0; 4],
            fade_origin: None,
        });
    }
    scene.atlas = crate::Atlas {
        width: ATLAS_SIZE,
        height: ATLAS_SIZE,
        data: text.atlas.data,
    };
}
