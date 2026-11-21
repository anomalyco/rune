//! Bounded layer planning matching `rune::render::layers` and `Instance::layer_bounds`.
use crate::{NO_CLIP, Primitive};
use libm::{ceilf, cosf, floorf, roundf, sinf};

/// Maximum offscreen dimension for a cached subtree (`layers::MAX_CACHE_DIM`).
const MAX_CACHE_DIM: f32 = 4096.0;

#[derive(Clone, Copy)]
pub(crate) struct LayerCtx {
    pub(crate) viewport: (u32, u32),
    pub(crate) limit: [f32; 4],
    pub(crate) free: bool,
    pub(crate) frame: [f32; 4],
}

impl LayerCtx {
    pub(crate) fn root(viewport: (u32, u32)) -> Self {
        Self {
            viewport,
            limit: NO_CLIP,
            free: false,
            frame: [0.0; 4],
        }
    }

    pub(crate) fn inline_child(self, clip: [f32; 4]) -> Self {
        if self.free {
            Self {
                frame: [
                    self.frame[0].max(clip[0]),
                    self.frame[1].max(clip[1]),
                    self.frame[2].min(clip[2]),
                    self.frame[3].min(clip[3]),
                ],
                ..self
            }
        } else {
            Self {
                limit: [
                    clip[0].max(self.limit[0]),
                    clip[1].max(self.limit[1]),
                    clip[2].min(self.limit[2]),
                    clip[3].min(self.limit[3]),
                ],
                ..self
            }
        }
    }
}

pub(crate) struct PlannedScope {
    pub(crate) child_ctx: LayerCtx,
    /// Integer-aligned target edges `[x0, y0, x1, y1]` in device pixels.
    pub(crate) target: [i32; 4],
    /// Edges propagated to an enclosing uncached parent (after any turn).
    pub(crate) outer: [f32; 4],
    pub(crate) visible: bool,
}

pub(crate) fn find_layer_close(primitives: &[Primitive], start: usize, end: usize) -> usize {
    let mut depth = 1usize;
    let mut close = start + 1;
    while close < end && depth > 0 {
        if primitives[close].params[1] == 8.0 {
            depth += 1;
        } else if primitives[close].params[1] == 9.0 {
            depth -= 1;
        }
        if depth > 0 {
            close += 1;
        }
    }
    close
}

/// Whether a layer's clip matches a hardware scissor (`layers/plan.rs` `is_scissor_clip`).
pub(crate) fn is_scissor_clip(clip: [f32; 4], fade: [f32; 4]) -> bool {
    clip == NO_CLIP || (fade == [0.0; 4] && clip.iter().all(|&v| (v - roundf(v)).abs() < 1e-3))
}

/// Upright bounds of edges `r` (`[x0, y0, x1, y1]`) turned `angle` radians clockwise about `p`.
pub(crate) fn turned_bounds(r: [f32; 4], angle: f32, p: [f32; 2]) -> [f32; 4] {
    let (sn, c) = (sinf(angle), cosf(angle));
    let mut out = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
    for (x, y) in [(r[0], r[1]), (r[2], r[1]), (r[0], r[3]), (r[2], r[3])] {
        let (dx, dy) = (x - p[0], y - p[1]);
        let (qx, qy) = (p[0] + c * dx - sn * dy, p[1] + sn * dx + c * dy);
        out = [out[0].min(qx), out[1].min(qy), out[2].max(qx), out[3].max(qy)];
    }
    out
}

fn union(a: &mut [f32; 4], b: [f32; 4]) {
    if b[2] <= b[0] || b[3] <= b[1] {
        return;
    }
    a[0] = a[0].min(b[0]);
    a[1] = a[1].min(b[1]);
    a[2] = a[2].max(b[2]);
    a[3] = a[3].max(b[3]);
}

fn clamp_band(rect: [f32; 4], uv: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let x0 = b[0].max(rect[0] + uv[0]);
    let y0 = b[1].max(rect[1] + uv[1]);
    let x1 = b[2].min(rect[0] - uv[2]).max(x0);
    let y1 = b[3].min(rect[1] + uv[3]).max(y0);
    [x0, y0, x1, y1]
}

/// Device-pixel bounds `[x0, y0, x1, y1]` a non-scope primitive contributes to an enclosing
/// uncached layer (`instance.rs` `layer_bounds_non_scope`).
fn primitive_layer_bounds(p: &Primitive) -> [f32; 4] {
    let kind = p.params[1] as u32;
    let b = if kind == 2 {
        if p.params[0] != 0.0 {
            let (sn, cs) = (sinf(p.params[0]), cosf(p.params[0]));
            let w = cs.abs() * p.rect[2] + sn.abs() * p.rect[3];
            let h = sn.abs() * p.rect[2] + cs.abs() * p.rect[3];
            let x0 = p.rect[0] + (p.rect[2] - w) * 0.5;
            let y0 = p.rect[1] + (p.rect[3] - h) * 0.5;
            [x0, y0, x0 + w, y0 + h]
        } else {
            [p.rect[0], p.rect[1], p.rect[0] + p.rect[2], p.rect[1] + p.rect[3]]
        }
    } else {
        let grow = match kind {
            1 => 1.0 + p.params[2] * 3.0,
            3 | 5 | 6 => 1.0,
            12 => 1.0 + p.params[2],
            _ => 1.0 + p.params[2] * 2.0,
        };
        let grown = [
            p.rect[0] - grow,
            p.rect[1] - grow,
            p.rect[0] + p.rect[2] + grow,
            p.rect[1] + p.rect[3] + grow,
        ];
        if kind <= 1 && p.uv[2] < 0.0 {
            clamp_band(p.rect, p.uv, grown)
        } else {
            grown
        }
    };
    [
        b[0].max(p.clip[0]),
        b[1].max(p.clip[1]),
        b[2].min(p.clip[2]),
        b[3].min(p.clip[3]),
    ]
}

/// Plans a layer scope at `start` (`close` is its matching kind 9) matching `layers.rs`.
pub(crate) fn plan_scope(
    primitives: &[Primitive],
    start: usize,
    close: usize,
    parent: LayerCtx,
) -> PlannedScope {
    let p = &primitives[start];
    let turn = p.turn.filter(|t| t[0] != 0.0).map(|[a, px, py]| (a, [px, py]));
    let free = turn.is_some() || parent.free;
    let frame = if free {
        let outer = if parent.free {
            parent.frame
        } else {
            [0.0, 0.0, parent.viewport.0 as f32, parent.viewport.1 as f32]
        };
        let shown = [
            outer[0].max(p.clip[0]),
            outer[1].max(p.clip[1]),
            outer[2].min(p.clip[2]),
            outer[3].min(p.clip[3]),
        ];
        match turn {
            Some((angle, pivot)) if shown[2] > shown[0] && shown[3] > shown[1] => {
                turned_bounds(shown, -angle, pivot)
            }
            _ => shown,
        }
    } else {
        [0.0; 4]
    };
    let limit = if parent.free { NO_CLIP } else { parent.limit };
    let child_limit = if free {
        NO_CLIP
    } else {
        [
            p.clip[0].max(limit[0]),
            p.clip[1].max(limit[1]),
            p.clip[2].min(limit[2]),
            p.clip[3].min(limit[3]),
        ]
    };
    let child_ctx = LayerCtx {
        viewport: parent.viewport,
        limit: child_limit,
        free,
        frame,
    };

    let cached = p.params[3] == 1.0;
    let mut edges = [p.rect[0], p.rect[1], p.rect[0] + p.rect[2], p.rect[1] + p.rect[3]];
    if !cached {
        let mut cursor = start + 1;
        while cursor < close {
            let child = &primitives[cursor];
            if child.params[1] == 8.0 {
                let sub_close = find_layer_close(primitives, cursor, close);
                let sub = plan_scope(primitives, cursor, sub_close, child_ctx);
                union(&mut edges, sub.outer);
                cursor = sub_close + 1;
            } else if child.params[1] == 9.0 {
                cursor += 1;
            } else {
                union(&mut edges, primitive_layer_bounds(child));
                cursor += 1;
            }
        }
    }

    let vis = if free {
        [
            floorf(edges[0].max(frame[0])),
            floorf(edges[1].max(frame[1])),
            ceilf(edges[2].min(frame[2])),
            ceilf(edges[3].min(frame[3])),
        ]
    } else {
        [
            floorf(edges[0].max(p.clip[0].max(limit[0])).max(0.0)),
            floorf(edges[1].max(p.clip[1].max(limit[1])).max(0.0)),
            ceilf(edges[2].min(p.clip[2].min(limit[2])).min(parent.viewport.0 as f32)),
            ceilf(edges[3].min(p.clip[3].min(limit[3])).min(parent.viewport.1 as f32)),
        ]
    };

    let mut visible = true;
    let target_edges = if cached {
        if vis[2] <= vis[0] || vis[3] <= vis[1] {
            visible = false;
        }
        let full = [floorf(edges[0]), floorf(edges[1]), ceilf(edges[2]), ceilf(edges[3])];
        if full[2] - full[0] <= MAX_CACHE_DIM && full[3] - full[1] <= MAX_CACHE_DIM {
            full
        } else {
            vis
        }
    } else {
        vis
    };
    if target_edges[2] <= target_edges[0] || target_edges[3] <= target_edges[1] {
        visible = false;
    }

    let outer = match turn {
        Some((angle, pivot)) if target_edges[2] > target_edges[0] && target_edges[3] > target_edges[1] => {
            turned_bounds(target_edges, angle, pivot)
        }
        _ => target_edges,
    };

    PlannedScope {
        child_ctx,
        target: [
            target_edges[0] as i32,
            target_edges[1] as i32,
            target_edges[2] as i32,
            target_edges[3] as i32,
        ],
        outer,
        visible,
    }
}
