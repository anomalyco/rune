//! Scalar oracle for Rune's device-pixel instance stream, not a vector rasterizer.
//! Basic operations use strict Rust f32 semantics (no mul_add or fast-math); all
//! transcendentals use pinned libm. The canonical output is sRGB RGBA8, quantized
//! after each source-over operation, including clear. See research/24.

pub mod generate;
#[cfg(feature = "gpu")]
pub mod gpu;
mod shader;

use serde::{Deserialize, Serialize};

pub const NO_CLIP: [f32; 4] = [-1e6, -1e6, 1e6, 1e6];

/// Same payload as rune::render::Instance; independent so the CPU lane has no
/// window, GPU, system-font or platform dependency. Source metadata is a sidecar.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Primitive {
    pub node: u32,
    pub source: String,
    pub rect: [f32; 4],
    pub radii: [f32; 4],
    pub fill: [f32; 4],
    pub border: [f32; 4],
    pub params: [f32; 4],
    pub uv: [f32; 4],
    pub clip: [f32; 4],
    pub fade: [f32; 4],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fade_origin: Option<[f32; 4]>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Atlas {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}
impl Atlas {
    pub(crate) fn load(&self, x: i32, y: i32) -> f32 {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return 0.0;
        }
        self.data[(y as u32 * self.width + x as u32) as usize] as f32 / 255.0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Scene {
    pub width: u32,
    pub height: u32,
    pub clear: [f32; 4],
    pub primitives: Vec<Primitive>,
    pub atlas: Atlas,
}
impl Scene {
    /// Reject malformed replay files before CPU allocation or GPU upload.
    pub fn validate(&self) -> Result<(), String> {
        if self.width == 0 || self.height == 0 || self.width > 4096 || self.height > 4096 {
            return Err("viewport must be 1..=4096".into());
        }
        if self.primitives.len() > 65536
            || self.atlas.width == 0
            || self.atlas.height == 0
            || self.atlas.width > 2048
            || self.atlas.height > 2048
            || self.atlas.data.len() != self.atlas.width as usize * self.atlas.height as usize
        {
            return Err("invalid atlas or primitive count".into());
        }
        let color = |c: &[f32; 4]| {
            c.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v))
                && c[..3].iter().all(|v| *v <= c[3])
        };
        if !color(&self.clear) {
            return Err("clear must be premultiplied linear RGBA".into());
        }
        for p in &self.primitives {
            if [
                p.rect, p.radii, p.fill, p.border, p.params, p.uv, p.clip, p.fade, p.fade_origin.unwrap_or(p.clip),
            ]
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || v.abs() > 1e6)
                || p.rect[2] <= 0.0
                || p.rect[3] <= 0.0
                || p.radii.iter().chain(p.fade.iter()).any(|v| *v < 0.0)
                || !(0..=4).any(|k| p.params[1] == k as f32)
                || p.params[0] < 0.0
                || p.params[2] < 0.0
                || !color(&p.fill)
                || !color(&p.border)
                || (p.params[1] == 4.0 && !color(&p.uv))
                || (p.params[1] == 2.0 && (p.params[3] <= 0.0 || p.uv.iter().any(|v| *v < 0.0)))
            {
                return Err(format!("invalid primitive {} ({})", p.node, p.source));
            }
        }
        Ok(())
    }
}

pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Last contributing primitive index, not nearest geometry. None = clear.
    pub owners: Option<Vec<Option<usize>>>,
}
impl Frame {
    pub fn new(width: u32, height: u32, provenance: bool) -> Self {
        assert!(width > 0 && height > 0 && width <= 4096 && height <= 4096);
        let n = width as usize * height as usize;
        Self {
            width,
            height,
            rgba: vec![0; n * 4],
            owners: provenance.then(|| vec![None; n]),
        }
    }

    /// No allocations after Frame construction. Scene must pass validate().
    pub fn render(&mut self, scene: &Scene) {
        assert_eq!((self.width, self.height), (scene.width, scene.height));
        let clear = encode(scene.clear);
        for pixel in self.rgba.as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&clear);
        }
        if let Some(ids) = &mut self.owners {
            ids.fill(None);
        }
        for (index, p) in scene.primitives.iter().enumerate() {
            for y in 0..self.height {
                for x in 0..self.width {
                    let col = shader::fragment(p, &scene.atlas, [x as f32 + 0.5, y as f32 + 0.5]);
                    if col[3] <= 0.0 {
                        continue;
                    }
                    let at = (y * self.width + x) as usize;
                    let dst = decode(self.rgba[at * 4..at * 4 + 4].try_into().unwrap());
                    let out = std::array::from_fn(|c| col[c] + dst[c] * (1.0 - col[3]));
                    self.rgba[at * 4..at * 4 + 4].copy_from_slice(&encode(out));
                    if let Some(ids) = &mut self.owners {
                        ids[at] = Some(index);
                    }
                }
            }
        }
    }

    pub fn hash(&self) -> String {
        blake3::hash(&self.rgba).to_hex().to_string()
    }

    pub fn blame<'a>(&self, scene: &'a Scene, x: u32, y: u32) -> Option<&'a Primitive> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let index = self.owners.as_ref()?[(y * self.width + x) as usize]?;
        scene.primitives.get(index)
    }
}

fn encode(c: [f32; 4]) -> [u8; 4] {
    std::array::from_fn(|i| {
        let v = c[i].clamp(0.0, 1.0);
        let v = if i == 3 {
            v
        } else if v <= 0.0031308 {
            v * 12.92
        } else {
            1.055 * libm::powf(v, 1.0 / 2.4) - 0.055
        };
        libm::roundf(v * 255.0) as u8
    })
}
fn decode(c: [u8; 4]) -> [f32; 4] {
    std::array::from_fn(|i| {
        let v = c[i] as f32 / 255.0;
        if i == 3 {
            v
        } else if v <= 0.04045 {
            v / 12.92
        } else {
            libm::powf((v + 0.055) / 1.055, 2.4)
        }
    })
}

#[derive(Debug, Serialize)]
pub struct Difference {
    pub max_channel: u8,
    pub outlier_pixels: usize,
    pub first: Option<PixelBlame>,
}
#[derive(Debug, Serialize)]
pub struct PixelBlame {
    pub x: u32,
    pub y: u32,
    pub expected: [u8; 4],
    pub actual: [u8; 4],
    pub node: Option<u32>,
    pub source: Option<String>,
}

/// Strict per-channel gate; no outlier allowance, no average that hides a bad edge.
pub fn compare(scene: &Scene, cpu: &Frame, gpu: &[u8], tolerance: u8) -> Difference {
    assert_eq!(cpu.rgba.len(), gpu.len());
    let mut diff = Difference {
        max_channel: 0,
        outlier_pixels: 0,
        first: None,
    };
    for (at, (a, b)) in cpu
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(gpu.as_chunks::<4>().0)
        .enumerate()
    {
        let d = a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        diff.max_channel = diff.max_channel.max(d);
        if d <= tolerance {
            continue;
        }
        diff.outlier_pixels += 1;
        if diff.first.is_none() {
            let (x, y) = (at as u32 % cpu.width, at as u32 / cpu.width);
            let p = cpu.blame(scene, x, y);
            diff.first = Some(PixelBlame {
                x,
                y,
                expected: *a,
                actual: *b,
                node: p.map(|p| p.node),
                source: p.map(|p| p.source.clone()),
            });
        }
    }
    diff
}
