// Surface buffer: for every pixel, the topmost primitive's own signed distance, outward gradient, top z and id.
// Painter's order, no blending: the last primitive drawn over a pixel owns it.

@group(0) @binding(0) var<uniform> F: Frame;
@group(0) @binding(1) var<storage, read> prims: array<Prim>;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) idx: u32,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VOut {
    let pr = prims[ii];
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    let p = pr.rect.xy - vec2<f32>(1.0) + corner * (pr.rect.zw + vec2<f32>(2.0));
    var o: VOut;
    o.pos = vec4<f32>(p.x / F.sf.x * 2.0 - 1.0, 1.0 - p.y / F.sf.y * 2.0, 0.0, 1.0);
    o.idx = ii;
    return o;
}

struct GOut {
    @location(0) g: vec4<f32>,
    @location(1) id: u32,
};

@fragment
fn fs(i: VOut) -> GOut {
    let pr = prims[i.idx];
    let half = pr.rect.zw * 0.5;
    // Fragment position is the rasterizer's exact pixel centre; interpolated `p` drifts by
    // subpixel ULPs on D3D12, discarding outer-edge pixels that sit on the exact `d == 0.5` cut.
    let q = i.pos.xy - (pr.rect.xy + half);
    let d = sd_rrect(q, half, pr.radii);
    if (d > 0.5 || (flags_of(pr) & F_HINT) != 0u) {
        discard;
    }
    let e = 0.5;
    let gx = sd_rrect(q + vec2<f32>(e, 0.0), half, pr.radii) - sd_rrect(q - vec2<f32>(e, 0.0), half, pr.radii);
    let gy = sd_rrect(q + vec2<f32>(0.0, e), half, pr.radii) - sd_rrect(q - vec2<f32>(0.0, e), half, pr.radii);
    var g = vec2<f32>(gx, gy);
    let gl = length(g);
    g = select(vec2<f32>(0.0), g / gl, gl > 1e-5);
    var o: GOut;
    o.g = vec4<f32>(d, g, pr.z.y);
    o.id = i.idx + 1u;
    return o;
}
