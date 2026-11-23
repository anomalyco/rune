//! Scalar oracle for Rune's device-pixel instance stream, not a vector rasterizer.
//! Basic operations use strict Rust f32 semantics (no mul_add or fast-math); all
//! transcendentals use pinned libm. The canonical output is sRGB RGBA8, quantized
//! after each source-over operation, including clear. See research/24.

#![forbid(unsafe_code)]

pub mod blend;
pub mod generate;
#[cfg(feature = "gpu")]
pub mod gpu;
#[cfg(all(test, feature = "gpu"))]
mod gpu_tests;
mod layer;
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
    /// Signed per-corner rounded-clip radii `[tl, tr, br, bl]` (`Instance::fade[3]`). Layers
    /// also carry theirs in `border` for saved replays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip_radii: Option<[f32; 4]>,
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
                // Triangles (kind 6), layers (kind 8, expanded by children), layer ends (kind 9)
                // and effect slots (kind 12) may have an empty rect.
                || (!matches!(p.params[1] as u32, 6 | 8 | 9 | 12) && (p.rect[2] <= 0.0 || p.rect[3] <= 0.0))
                || p.rect[2] < 0.0
                || p.rect[3] < 0.0
                // A negative fade width is a linear ramp.
                // Negative corner radii select Apple continuous corners (`sd_continuous`).
                // A layer's `radii` are its hole, whose centre may lie anywhere.
                || (p.params[1] == 8.0 && p.params[3] == 0.0 && (p.radii[2] < 0.0 || p.radii[3] < p.radii[2]))
                // Kinds 7 (image) and 13 (backdrop blur) are not implemented in the scalar oracle
                // and stay rejected.
                || !(0..=6).chain([8, 9, 10, 11, 12]).any(|k| p.params[1] == k as f32)
                // A glyph's params[0] is a signed turn; an inset shadow's params[0] is a signed spread.
                || (p.params[0] < 0.0 && !matches!(p.params[1] as u32, 2 | 3))
                || p.params[2] < 0.0
                || !color(&p.fill)
                || (if p.params[1] == 8.0 {
                    p.border.iter().any(|&v| v < 0.0) && p.border.iter().any(|&v| v > 0.0)
                } else if p.params[1] == 12.0 {
                    // Effect slots carry their 32-bit mark as two 16-bit floats in `border[0..2]`.
                    false
                } else {
                    !color(&p.border)
                })
                || p.clip_radii.is_some_and(|r| {
                    r.iter().any(|v| !v.is_finite() || v.abs() > 1e6)
                        || (r.iter().any(|&v| v < 0.0) && r.iter().any(|&v| v > 0.0))
                })
                || p.turn.is_some_and(|t| {
                    p.params[1] != 8.0
                        || p.params[3] == 1.0
                        || t[0] == 0.0
                        || t.iter().any(|v| !v.is_finite() || v.abs() > 1e6)
                })
                || (p.params[1] == 1.0 && p.uv[2] > 0.0 && p.uv[3] <= 0.0)
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
            Target {
                origin: [0, 0],
                size: (self.width, self.height),
                scissor: [0, 0, self.width, self.height],
                pixels: TargetPixels::Srgb(&mut self.rgba),
            },
            self.owners.as_deref_mut(),
            scene,
            0..scene.primitives.len(),
            layer::LayerCtx::root((self.width, self.height)),
            false,
        );
    }

    fn render_range(
        mut target: Target<'_>,
        mut owners: Option<&mut [Option<usize>]>,
        scene: &Scene,
        range: std::ops::Range<usize>,
        ctx: layer::LayerCtx,
        additive: bool,
    ) {
        let (mut cursor, end) = (range.start, range.end);
        let [ox, oy] = target.origin;
        let (tw, th) = target.size;
        let scissor = target.scissor;
        while cursor < end {
            let p = &scene.primitives[cursor];
            if p.params[1] == 8.0 {
                let close = layer::find_layer_close(&scene.primitives, cursor, end);
                let clip_radii = p.clip_radii.unwrap_or(p.border);
                let inline = !additive
                    && p.params[3] == 0.0
                    && p.fill[3] == 1.0
                    && p.params[0] == 0.0
                    && p.params[2] == 0.0
                    && shader::pack_clip_radii(p.clip, clip_radii) == 0
                    && layer::is_scissor_clip(p.clip, p.fade)
                    && p.radii[3] == 0.0
                    && p.turn.is_none();
                if inline {
                    let sub_scissor = if p.clip == NO_CLIP {
                        scissor
                    } else {
                        let edge = |v: f32, o: i32, max: u32| (v - o as f32).clamp(0.0, max as f32) as u32;
                        [
                            edge(libm::floorf(p.clip[0]), ox, tw).max(scissor[0]),
                            edge(libm::floorf(p.clip[1]), oy, th).max(scissor[1]),
                            edge(libm::ceilf(p.clip[2]), ox, tw).min(scissor[2]),
                            edge(libm::ceilf(p.clip[3]), oy, th).min(scissor[3]),
                        ]
                    };
                    if sub_scissor[2] > sub_scissor[0] && sub_scissor[3] > sub_scissor[1] {
                        Self::render_range(
                            target.reborrow(sub_scissor),
                            owners.as_deref_mut(),
                            scene,
                            cursor + 1..close,
                            ctx.inline_child(p.clip),
                            false,
                        );
                    }
                } else if p.fill[3] > 0.0 {
                    let scope = layer::plan_scope(&scene.primitives, cursor, close, ctx);
                    if scope.visible {
                        let [lx, ly, rx, ry] = scope.target;
                        let (lw, lh) = ((rx - lx) as u32, (ry - ly) as u32);
                        // Isolated layers accumulate in linear float (matching GPU Rgba16Float);
                        // PlusLighter blends children additively inside and saturates before compositing.
                        let layer_additive = p.params[0] == 1.0;
                        let mut layer_buf = vec![[0.0f32; 4]; (lw * lh) as usize];
                        let mut layer_owners = owners.as_ref().map(|_| vec![None; (lw * lh) as usize]);
                        Self::render_range(
                            Target {
                                origin: [lx, ly],
                                size: (lw, lh),
                                scissor: [0, 0, lw, lh],
                                pixels: TargetPixels::Linear(&mut layer_buf),
                            },
                            layer_owners.as_deref_mut(),
                            scene,
                            cursor + 1..close,
                            scope.child_ctx,
                            layer_additive,
                        );
                        let turn = p.turn.filter(|t| t[0] != 0.0).map(|[a, px, py]| (a, libm::sinf(a), libm::cosf(a), [px, py]));
                        let (x0, y0, x1, y1) = match turn {
                            Some((angle, _, _, pivot)) => {
                                let quad = layer::turned_bounds(
                                    [lx as f32 - 1.0, ly as f32 - 1.0, rx as f32 + 1.0, ry as f32 + 1.0],
                                    angle,
                                    pivot,
                                );
                                let edge = |v: f32, o: i32, max: u32| (v - o as f32).clamp(0.0, max as f32) as u32;
                                (
                                    edge(libm::floorf(quad[0]), ox, tw).max(scissor[0]),
                                    edge(libm::floorf(quad[1]), oy, th).max(scissor[1]),
                                    edge(libm::ceilf(quad[2]), ox, tw).min(scissor[2]),
                                    edge(libm::ceilf(quad[3]), oy, th).min(scissor[3]),
                                )
                            }
                            None => (
                                ((lx - ox).max(0) as u32).max(scissor[0]),
                                ((ly - oy).max(0) as u32).max(scissor[1]),
                                ((rx - ox).max(0) as u32).min(scissor[2]),
                                ((ry - oy).max(0) as u32).min(scissor[3]),
                            ),
                        };
                        let alpha = p.fill[3].clamp(0.0, 1.0);
                        for y in y0..y1 {
                            let sy = (oy + y as i32) as f32 + 0.5;
                            for x in x0..x1 {
                                let sx = (ox + x as i32) as f32 + 0.5;
                                let cov = alpha * shader::layer_clip_coverage(p, [sx, sy]);
                                if cov <= 0.0 {
                                    continue;
                                }
                                let at = (y * tw + x) as usize;
                                let (src, src_owner) = match turn {
                                    Some((_, sn, cs, pivot)) => {
                                        let (qx, qy) = (sx - pivot[0], sy - pivot[1]);
                                        let (ux, uy) = (
                                            pivot[0] + cs * qx + sn * qy - lx as f32 - 0.5,
                                            pivot[1] + cs * qy - sn * qx - ly as f32 - 0.5,
                                        );
                                        let (bx, by) = (libm::floorf(ux), libm::floorf(uy));
                                        let (fx, fy) = (ux - bx, uy - by);
                                        let texel = |tx: i32, ty: i32| -> [f32; 4] {
                                            if tx < 0 || ty < 0 || tx >= lw as i32 || ty >= lh as i32 {
                                                return [0.0; 4];
                                            }
                                            let c = layer_buf[(ty as u32 * lw + tx as u32) as usize];
                                            if layer_additive { c.map(|v| v.clamp(0.0, 1.0)) } else { c }
                                        };
                                        let (ix, iy) = (bx as i32, by as i32);
                                        let (a, b) = (texel(ix, iy), texel(ix + 1, iy));
                                        let (c, d) = (texel(ix, iy + 1), texel(ix + 1, iy + 1));
                                        let color = std::array::from_fn(|k| {
                                            let top = a[k] * (1.0 - fx) + b[k] * fx;
                                            let bottom = c[k] * (1.0 - fx) + d[k] * fx;
                                            top * (1.0 - fy) + bottom * fy
                                        });
                                        let owner = layer_owners.as_ref().and_then(|l_ids| {
                                            let pick = |tx: i32, ty: i32| -> Option<usize> {
                                                if tx < 0 || ty < 0 || tx >= lw as i32 || ty >= lh as i32 {
                                                    return None;
                                                }
                                                l_ids[(ty as u32 * lw + tx as u32) as usize]
                                            };
                                            let (nx, ny) = (libm::roundf(ux) as i32, libm::roundf(uy) as i32);
                                            pick(nx, ny)
                                                .or_else(|| pick(ix, iy))
                                                .or_else(|| pick(ix + 1, iy))
                                                .or_else(|| pick(ix, iy + 1))
                                                .or_else(|| pick(ix + 1, iy + 1))
                                        });
                                        (color, owner)
                                    }
                                    None => {
                                        let l_at = (((oy + y as i32 - ly) as u32) * lw + ((ox + x as i32 - lx) as u32)) as usize;
                                        let c = layer_buf[l_at];
                                        let color = if layer_additive { c.map(|v| v.clamp(0.0, 1.0)) } else { c };
                                        let owner = layer_owners.as_ref().and_then(|l_ids| l_ids[l_at]);
                                        (color, owner)
                                    }
                                };
                                let col = src.map(|c| c * cov);
                                if col[3] <= 0.0 {
                                    continue;
                                }
                                target.blend(at, col, additive);
                                if let Some(ids) = owners.as_deref_mut() {
                                    ids[at] = src_owner.or(Some(cursor));
                                }
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
            for y in scissor[1]..scissor[3] {
                let sy = (oy + y as i32) as f32 + 0.5;
                for x in scissor[0]..scissor[2] {
                    let sx = (ox + x as i32) as f32 + 0.5;
                    let col = shader::fragment(p, &scene.atlas, [sx, sy]);
                    if col[3] <= 0.0 {
                        continue;
                    }
                    let at = (y * tw + x) as usize;
                    target.blend(at, col, additive);
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

struct Target<'a> {
    origin: [i32; 2],
    size: (u32, u32),
    scissor: [u32; 4],
    pixels: TargetPixels<'a>,
}

enum TargetPixels<'a> {
    Srgb(&'a mut [u8]),
    Linear(&'a mut [[f32; 4]]),
}

impl Target<'_> {
    fn reborrow(&mut self, scissor: [u32; 4]) -> Target<'_> {
        Target {
            origin: self.origin,
            size: self.size,
            scissor,
            pixels: match &mut self.pixels {
                TargetPixels::Srgb(s) => TargetPixels::Srgb(s),
                TargetPixels::Linear(l) => TargetPixels::Linear(l),
            },
        }
    }

    fn blend(&mut self, at: usize, col: [f32; 4], additive: bool) {
        let combine = |dst: [f32; 4]| -> [f32; 4] {
            if additive {
                std::array::from_fn(|c| col[c] + dst[c])
            } else {
                std::array::from_fn(|c| col[c] + dst[c] * (1.0 - col[3]))
            }
        };
        match &mut self.pixels {
            TargetPixels::Srgb(rgba) => {
                let dst = decode(rgba[at * 4..at * 4 + 4].try_into().unwrap());
                rgba[at * 4..at * 4 + 4].copy_from_slice(&encode(combine(dst)));
            }
            TargetPixels::Linear(buf) => {
                buf[at] = combine(buf[at]);
            }
        }
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
