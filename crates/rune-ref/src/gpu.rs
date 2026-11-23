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
            // A layer has no border paint; preserve its signed per-corner rounded-clip radii here,
            // and its turn in `turn`.
            border: if p.params[1] == 8.0 { rune::render::unpack_clip_radii(p.fade[3]) } else { p.border },
            params: p.params,
            uv: p.uv,
            clip: p.clip,
            fade,
            fade_origin: Some(std::array::from_fn(|e| p.clip[e] + offset[e] * fade[e].abs() * if e < 2 { -1.0 } else { 1.0 })),
            turn: (p.params[1] == 8.0 && p.border[1] != 0.0).then_some([p.border[1], p.border[2], p.border[3]]),
            clip_radii: (p.params[1] != 8.0 && p.fade[3] != 0).then(|| rune::render::unpack_clip_radii(p.fade[3])),
        }
    }
}

impl From<&Primitive> for Instance {
    fn from(p: &Primitive) -> Self {
        let mut instance = Self {
            rect: p.rect,
            // `Scene` carries one grayscale R8 glyph atlas and no outline store; clear legacy
            // non-zero `radii` and `params[2]` on glyphs so Windfoil never routes them to
            // `outline_glyph`, `lcd_mask`, or secondary atlas pages.
            radii: if p.params[1] == 2.0 { [0.0; 4] } else { p.radii },
            fill: p.fill,
            border: if p.params[1] == 8.0 {
                let [a, x, y] = p.turn.unwrap_or([0.0; 3]);
                [0.0, a, x, y]
            } else {
                p.border
            },
            params: if p.params[1] == 2.0 { [p.params[0], p.params[1], 0.0, p.params[3]] } else { p.params },
            uv: p.uv,
            clip: p.clip,
            fade: rune::render::pack_fade(p.fade, p.fade_origin.unwrap_or(p.clip), p.clip),
        };
        if p.params[1] == 8.0 {
            instance.fade[3] = rune::render::pack_clip_radii(p.clip, p.clip_radii.unwrap_or(p.border));
        } else if let Some(radii) = p.clip_radii {
            instance.fade[3] = rune::render::pack_clip_radii(p.clip, radii);
        }
        instance
    }
}

/// Default per-channel tolerance for a single draw and for exact-blending composites.
pub const DEFAULT_TOLERANCE: u8 = 2;

/// `(backend, pci_vendor_id, composite_tolerance)` entries measured on hardware whose fixed-function
/// sRGB ROP pre-quantizes linear fragment output to 8 bits before blending.
///
/// Direct3D 11.3 Functional Specification §17.5 permits fixed-point render target blending at
/// output-format precision (8 bits). On NVIDIA under DX12, four to five overlapping translucent
/// draws into `Rgba8UnormSrgb` accumulate up to 3 LSB against the CPU oracle, whereas AMD Radeon
/// DX12, WARP DX12, and Apple M4 Pro / Metal stay within 2 LSB. Single draws stay within 2 LSB on
/// every measured adapter. NVIDIA under Vulkan is not measured and stays at 2 until measured.
const MEASURED_COMPOSITE_TOLERANCE: [(wgpu::Backend, u32, u8); 1] =
    [(wgpu::Backend::Dx12, 0x10DE, 3)];

/// Composite per-channel tolerance for `info`: 3 for NVIDIA under DX12 (vendor `0x10DE`), and 2
/// everywhere else. Single-primitive checks always stay at [`DEFAULT_TOLERANCE`] (2).
pub fn composite_tolerance(info: &wgpu::AdapterInfo) -> u8 {
    MEASURED_COMPOSITE_TOLERANCE
        .iter()
        .find(|&&(backend, vendor, _)| backend == info.backend && vendor == info.vendor)
        .map_or(DEFAULT_TOLERANCE, |&(_, _, tol)| tol)
}

pub struct GpuRenderer {
    gpu: Gpu,
    renderer: Renderer,
    text: TextSystem,
}
impl Default for GpuRenderer {
    fn default() -> Self {
        let gpu = Gpu::new(Gpu::instance(), None);
        let info = gpu.adapter.get_info();
        let composite_tol = composite_tolerance(&info);
        eprintln!(
            "reference adapter: {} ({:?}, vendor=0x{:04x}) single_tolerance={DEFAULT_TOLERANCE} composite_tolerance={composite_tol}",
            info.name, info.backend, info.vendor
        );
        let renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        Self {
            gpu,
            renderer,
            text: TextSystem::default(),
        }
    }
}
impl GpuRenderer {
    pub fn adapter_info(&self) -> wgpu::AdapterInfo {
        self.gpu.adapter.get_info()
    }

    pub fn composite_tolerance(&self) -> u8 {
        composite_tolerance(&self.adapter_info())
    }

    /// Tolerance for `scene`: [`DEFAULT_TOLERANCE`] (2) for a single draw, and
    /// [`Self::composite_tolerance`] when several draws can blend into a pixel.
    pub fn scene_tolerance(&self, scene: &Scene) -> u8 {
        if scene.primitives.len() > 1 {
            self.composite_tolerance()
        } else {
            DEFAULT_TOLERANCE
        }
    }

    pub fn render(&mut self, scene: &Scene) -> Vec<u8> {
        scene.validate().expect("valid differential scene");
        let atlas = &mut self.text.atlas;
        atlas.data.clear();
        atlas.data.resize((ATLAS_SIZE * ATLAS_SIZE) as usize, 0);
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
            turn: None,
            clip_radii: None,
        });
    }
    scene.atlas = crate::Atlas {
        width: ATLAS_SIZE,
        height: ATLAS_SIZE,
        data: text.atlas.data,
    };
}
