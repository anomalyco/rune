//! Scalar oracle for Rune's device-pixel instance stream, not a vector rasterizer.
//! Basic operations use strict Rust f32 semantics (no mul_add or fast-math); all
//! transcendentals use pinned libm. The canonical output is sRGB RGBA8, quantized
//! after each source-over operation, including clear. See research/24.
#![deny(unsafe_code)]

pub mod generate;
pub mod blend;
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
    /// Border paint; for a layer (kind 8), signed per-corner clip radii `[tl, tr, br, bl]`.
    pub border: [f32; 4],
    pub params: [f32; 4],
    pub uv: [f32; 4],
    pub clip: [f32; 4],
    pub fade: [f32; 4],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fade_origin: Option<[f32; 4]>,
    /// For a layer (kind 8), `[turn, pivot_x, pivot_y]`: the composite turns `turn` radians
    /// clockwise about the pivot (device pixels).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<[f32; 3]>,
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
/// The deepest nesting of layer scopes `validate` accepts.
const MAX_LAYER_DEPTH: usize = 16;

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
        let mut layer_depth = 0usize;
        for p in &self.primitives {
            if p.params[1] == 8.0 {
                // Each open scope holds a full-frame buffer, and the compositor has no layer blur.
                if layer_depth == MAX_LAYER_DEPTH || p.params[2] != 0.0 {
                    return Err(format!("unsupported layer {} ({})", p.node, p.source));
                }
                if !matches!(p.params[0], 0.0 | 1.0) || !matches!(p.params[3], 0.0 | 1.0)
                    || (p.params[3] == 1.0 && p.radii.iter().chain(p.uv.iter())
                        .any(|v| !v.is_finite() || !(0.0..=65535.0).contains(v) || v.fract() != 0.0))
                {
                    return Err(format!("invalid layer {} ({})", p.node, p.source));
                }
                layer_depth += 1;
            } else if p.params[1] == 9.0 {
                if layer_depth == 0 {
                    return Err(format!("unbalanced layer end {} ({})", p.node, p.source));
                }
                layer_depth -= 1;
            }
            if [
                p.rect, p.radii, p.fill, p.border, p.params, p.uv, p.clip, p.fade, p.fade_origin.unwrap_or(p.clip),
            ]
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || v.abs() > 1e6)
                // An effect slot (kind 12) or layer end (kind 9) draws nothing and may be empty.
                || (!matches!(p.params[1] as u32, 9 | 12) && (p.rect[2] <= 0.0 || p.rect[3] <= 0.0))
                || p.rect[2] < 0.0
                || p.rect[3] < 0.0
                || p.radii.iter().chain(p.fade.iter()).any(|v| *v < 0.0)
                || !(0..=4).chain([8, 9, 10, 11, 12]).any(|k| p.params[1] == k as f32)
                // A glyph's params[0] is a signed turn; negative is counter-clockwise.
                || (p.params[0] < 0.0 && p.params[1] != 2.0)
                || p.params[2] < 0.0
                || !color(&p.fill)
                || (if p.params[1] == 8.0 {
                    p.border.iter().any(|&v| v < 0.0) && p.border.iter().any(|&v| v > 0.0)
                } else {
                    !color(&p.border)
                })
                || p.turn.is_some_and(|t| {
                    p.params[1] != 8.0
                        || p.params[3] == 1.0
                        || t[0] == 0.0
                        || t.iter().any(|v| !v.is_finite() || v.abs() > 1e6)
                })
                || (p.params[1] == 4.0 && !color(&p.uv))
                || (p.params[1] == 10.0 && (p.uv[0] <= 0.0 || p.uv[1] <= 0.0))
                || (p.params[1] == 11.0 && (p.uv[2] < 0.0 || p.uv[3] < 0.0 || (p.uv[2] > 0.0) != (p.uv[3] > 0.0)))
                || (p.params[1] == 2.0 && (p.params[3] <= 0.0 || p.uv.iter().any(|v| *v < 0.0)))
            {
                return Err(format!("invalid primitive {} ({})", p.node, p.source));
            }
        }
        if layer_depth != 0 {
            return Err("unclosed layer scope".into());
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

    /// No allocations after Frame construction unless isolated/cached layers are present. Scene must pass validate().
    pub fn render(&mut self, scene: &Scene) {
        assert_eq!((self.width, self.height), (scene.width, scene.height));
        let clear = encode(scene.clear);
        for pixel in self.rgba.as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&clear);
        }
        if let Some(ids) = &mut self.owners {
            ids.fill(None);
        }
        Self::render_range(
            self.width,
            self.height,
            &mut self.rgba,
            self.owners.as_deref_mut(),
            scene,
            0,
            scene.primitives.len(),
        );
    }

    fn render_range(
        width: u32,
        height: u32,
        rgba: &mut [u8],
        mut owners: Option<&mut [Option<usize>]>,
        scene: &Scene,
        mut cursor: usize,
        end: usize,
    ) {
        while cursor < end {
            let p = &scene.primitives[cursor];
            if p.params[1] == 8.0 {
                let mut depth = 1usize;
                let mut close = cursor + 1;
                while close < end && depth > 0 {
                    if scene.primitives[close].params[1] == 8.0 {
                        depth += 1;
                    } else if scene.primitives[close].params[1] == 9.0 {
                        depth -= 1;
                    }
                    if depth > 0 {
                        close += 1;
                    }
                }
                if p.params[3] == 0.0 && p.fill[3] == 1.0 && p.params[0] == 0.0 && p.params[2] == 0.0 && p.clip == NO_CLIP && p.border == [0.0; 4]
                    && p.turn.is_none()
                {
                    Self::render_range(width, height, rgba, owners.as_deref_mut(), scene, cursor + 1, close);
                } else if p.fill[3] > 0.0 {
                    let mut layer_rgba = vec![0u8; (width * height * 4) as usize];
                    let mut layer_owners = owners.as_ref().map(|o| vec![None; o.len()]);
                    Self::render_range(width, height, &mut layer_rgba, layer_owners.as_deref_mut(), scene, cursor + 1, close);
                    let turn = p.turn.filter(|t| t[0] != 0.0).map(|[a, px, py]| (libm::sinf(a), libm::cosf(a), [px, py]));
                    // A turned layer can reach anywhere; its contents fill the frame-sized buffer.
                    let (x0, y0, x1, y1) = if turn.is_some() {
                        (0, 0, width, height)
                    } else {
                        (libm::floorf(p.rect[0]).max(0.0) as u32, libm::floorf(p.rect[1]).max(0.0) as u32,
                            (libm::ceilf(p.rect[0] + p.rect[2]).max(0.0) as u32).min(width),
                            (libm::ceilf(p.rect[1] + p.rect[3]).max(0.0) as u32).min(height))
                    };
                    let alpha = p.fill[3].clamp(0.0, 1.0);
                    for y in y0..y1 {
                        for x in x0..x1 {
                            let cov = alpha * shader::layer_clip_coverage(p, [x as f32 + 0.5, y as f32 + 0.5]);
                            if cov <= 0.0 {
                                continue;
                            }
                            let at = (y * width + x) as usize;
                            let src = match turn {
                                Some((sn, cs, pivot)) => {
                                    let (qx, qy) = (x as f32 + 0.5 - pivot[0], y as f32 + 0.5 - pivot[1]);
                                    let (ux, uy) = (pivot[0] + cs * qx + sn * qy - 0.5, pivot[1] + cs * qy - sn * qx - 0.5);
                                    let (bx, by) = (libm::floorf(ux), libm::floorf(uy));
                                    let (fx, fy) = (ux - bx, uy - by);
                                    let texel = |tx: f32, ty: f32| -> [f32; 4] {
                                        if tx < 0.0 || ty < 0.0 || tx >= width as f32 || ty >= height as f32 {
                                            return [0.0; 4];
                                        }
                                        let at = (ty as u32 * width + tx as u32) as usize;
                                        decode(layer_rgba[at * 4..at * 4 + 4].try_into().unwrap())
                                    };
                                    let (a, b) = (texel(bx, by), texel(bx + 1.0, by));
                                    let (c, d) = (texel(bx, by + 1.0), texel(bx + 1.0, by + 1.0));
                                    std::array::from_fn(|k| {
                                        let top = a[k] * (1.0 - fx) + b[k] * fx;
                                        let bottom = c[k] * (1.0 - fx) + d[k] * fx;
                                        top * (1.0 - fy) + bottom * fy
                                    })
                                }
                                None => decode(layer_rgba[at * 4..at * 4 + 4].try_into().unwrap()),
                            };
                            let col = src.map(|c| c * cov);
                            if col[3] <= 0.0 {
                                continue;
                            }
                            let dst = decode(rgba[at * 4..at * 4 + 4].try_into().unwrap());
                            let out = if p.params[0] == 1.0 {
                                std::array::from_fn(|c| col[c] + dst[c])
                            } else {
                                std::array::from_fn(|c| col[c] + dst[c] * (1.0 - col[3]))
                            };
                            rgba[at * 4..at * 4 + 4].copy_from_slice(&encode(out));
                            if let (Some(ids), Some(l_ids)) = (owners.as_deref_mut(), layer_owners.as_ref()) {
                                ids[at] = l_ids[at].or(Some(cursor));
                            }
                        }
                    }
                }
                cursor = close + 1;
                continue;
            }
            if p.params[1] == 9.0 {
                cursor += 1;
                continue;
            }
            for y in 0..height {
                for x in 0..width {
                    let col = shader::fragment(p, &scene.atlas, [x as f32 + 0.5, y as f32 + 0.5]);
                    if col[3] <= 0.0 {
                        continue;
                    }
                    let at = (y * width + x) as usize;
                    let dst = decode(rgba[at * 4..at * 4 + 4].try_into().unwrap());
                    let out = std::array::from_fn(|c| col[c] + dst[c] * (1.0 - col[3]));
                    rgba[at * 4..at * 4 + 4].copy_from_slice(&encode(out));
                    if let Some(ids) = owners.as_deref_mut() {
                        ids[at] = Some(cursor);
                    }
                }
            }
            cursor += 1;
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
