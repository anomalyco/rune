// One instanced "uber-quad": rounded rects with per-corner radius and border (kind 0), analytic
// Gaussian rounded-rect shadows (kind 1), glyph-atlas text (kind 2), inset shadows (kind 3) and
// vertical-gradient rounded rects (kind 4), stroke segments (kind 5) and filled triangles
// (kind 6) for vector shapes, RGBA images (kind 7), and premultiplied layer composites (kind 8).
// Scope-end records (kind 9) never reach this shader. Every ordinary instance carries a
// soft-edge clip. Output is premultiplied linear colour into an sRGB target (linear blending).

struct Globals {
    viewport: vec2<f32>,
    pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var images: texture_2d<f32>;
// Zero size selects the window viewport. Layer targets keep all geometry/clip coordinates
// absolute and change only the final projection, including GPU-spring translations.
@group(0) @binding(3) var<uniform> projection: vec4<f32>;

struct In {
    @builtin(vertex_index) vi: u32,
    @location(0) rect: vec4<f32>,
    @location(1) radii: vec4<f32>,
    @location(2) fill: vec4<f32>,
    @location(3) border: vec4<f32>,
    @location(4) params: vec4<f32>,
    @location(5) uv: vec4<f32>,
    @location(6) clip: vec4<f32>,
    @location(7) fade: vec4<u32>,
};

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) p: vec2<f32>,
    @location(1) @interpolate(flat) rect: vec4<f32>,
    @location(2) @interpolate(flat) radii: vec4<f32>,
    @location(3) @interpolate(flat) fill: vec4<f32>,
    @location(4) @interpolate(flat) border: vec4<f32>,
    @location(5) @interpolate(flat) params: vec4<f32>,
    @location(6) @interpolate(flat) uv: vec4<f32>,
    @location(7) @interpolate(flat) clip: vec4<f32>,
    @location(8) @interpolate(flat) fade: vec4<u32>,
};

@vertex
fn vs(i: In) -> Out {
    return base_vs(i);
}

// Composites deliberately bypass retained instance tracks: the layer's contents already
// sampled those tracks using their original absolute instance indices.
@vertex
fn vs_layer(i: In) -> Out {
    return base_vs(i);
}

fn base_vs(i: In) -> Out {
    let corner = vec2<f32>(f32(i.vi & 1u), f32(i.vi >> 1u));
    let kind = i.params.y;
    var grow = 1.0 + i.params.z * 2.0;
    if (kind > 0.5 && kind < 1.5) {
        grow = i.params.z * 3.0 + 1.0;
    } else if (kind > 1.5 && kind < 2.5) {
        grow = 0.0;
    } else if (kind > 2.5 && kind < 3.5) {
        grow = 1.0;
    } else if (kind > 4.5) {
        grow = 1.0;
    }
    let lo = i.rect.xy - vec2<f32>(grow);
    let size = i.rect.zw + vec2<f32>(2.0 * grow);
    let p = lo + corner * size;
    var screen = p;
    // Icon loops rotate the quad, not its atlas allocation. Local p still samples the glyph.
    if (kind > 1.5 && kind < 2.5 && i.params.x != 0.0) {
        let center = i.rect.xy + i.rect.zw * 0.5;
        let q = p - center;
        let c = cos(i.params.x);
        let s = sin(i.params.x);
        screen = center + vec2<f32>(c * q.x - s * q.y, s * q.x + c * q.y);
    }
    var o: Out;
    let size_px = select(g.viewport, projection.zw, projection.z > 0.0);
    let local = screen - projection.xy;
    o.pos = vec4<f32>(local.x / size_px.x * 2.0 - 1.0, 1.0 - local.y / size_px.y * 2.0, 0.0, 1.0);
    o.p = p;
    o.rect = i.rect;
    o.radii = i.radii;
    o.fill = i.fill;
    o.border = i.border;
    o.params = i.params;
    o.uv = i.uv;
    o.clip = i.clip;
    o.fade = i.fade;
    return o;
}

fn sd_rounded(p: vec2<f32>, half: vec2<f32>, radii: vec4<f32>) -> f32 {
    // radii: top-left, top-right, bottom-right, bottom-left (y down).
    var r = radii.x;
    if (p.x > 0.0) {
        r = select(radii.y, radii.z, p.y > 0.0);
    } else {
        r = select(radii.x, radii.w, p.y > 0.0);
    }
    r = min(r, min(half.x, half.y));
    let q = abs(p) - half + vec2<f32>(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - r;
}

fn ramp(d: f32, fade: f32) -> f32 {
    if (fade <= 0.0) {
        return clamp(d + 0.5, 0.0, 1.0);
    }
    let x = clamp(d / fade, 0.0, 1.0);
    return x * x * (3.0 - 2.0 * x);
}

fn clip_alpha(p: vec2<f32>, c: vec4<f32>, packed: vec4<u32>) -> f32 {
    let f = vec4<f32>(unpack2x16float(packed.x), unpack2x16float(packed.y));
    let offset = unpack4x8unorm(packed.z);
    let hard = vec4<f32>(p - c.xy, c.zw - p);
    var alpha = 1.0;
    for (var edge = 0u; edge < 4u; edge++) {
        // Keep the original sub-pixel ramp exactly when its origin is the hard edge.
        var a = ramp(hard[edge], f[edge]);
        if offset[edge] >= 1.0 {
            a = ramp(hard[edge], 0.0);
        } else if offset[edge] > 0.0 {
            a = min(ramp(hard[edge], 0.0), ramp(hard[edge] + offset[edge] * f[edge], f[edge]));
        }
        alpha *= a;
    }
    return alpha;
}

// Evan Wallace's analytic rounded-box shadow (as in GPUI): exact along x, 4 samples along y.
fn gaussian(x: f32, sigma: f32) -> f32 {
    return exp(-(x * x) / (2.0 * sigma * sigma)) / (sqrt(2.0 * 3.14159265) * sigma);
}

fn erf2(x: vec2<f32>) -> vec2<f32> {
    let s = sign(x);
    let a = abs(x);
    var r = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
    r = r * r;
    return s - s / (r * r);
}

fn shadow_x(x: f32, y: f32, sigma: f32, corner: f32, half: vec2<f32>) -> f32 {
    let delta = min(half.y - corner - abs(y), 0.0);
    let curved = half.x - corner + sqrt(max(0.0, corner * corner - delta * delta));
    let integral = 0.5 + 0.5 * erf2((x + vec2<f32>(-curved, curved)) * (sqrt(0.5) / sigma));
    return integral.y - integral.x;
}

fn shadow(p: vec2<f32>, half: vec2<f32>, sigma: f32, corner: f32) -> f32 {
    let lo = p.y - half.y;
    let hi = p.y + half.y;
    let start = clamp(-3.0 * sigma, lo, hi);
    let end = clamp(3.0 * sigma, lo, hi);
    let step = (end - start) / 4.0;
    var y = start + step * 0.5;
    var v = 0.0;
    for (var i = 0; i < 4; i = i + 1) {
        v = v + shadow_x(p.x, p.y - y, sigma, corner, half) * gaussian(y, sigma) * step;
        y = y + step;
    }
    return v;
}

// Coverage of a stroke segment from a to b, `hw` half width, round or butt caps. Strokes
// thinner than a pixel draw a pixel wide at proportionally lower coverage.
fn segment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, hw: f32, round: bool) -> f32 {
    let w = max(hw, 0.5);
    let weight = min(hw * 2.0, 1.0);
    let ab = b - a;
    let len = length(ab);
    let ap = p - a;
    var d: f32;
    if (len < 1e-4) {
        if (!round) {
            return 0.0;
        }
        d = length(ap) - w;
    } else {
        let dir = ab / len;
        let along = dot(ap, dir);
        if (round) {
            d = length(ap - dir * clamp(along, 0.0, len)) - w;
        } else {
            let across = abs(dir.x * ap.y - dir.y * ap.x);
            let q = vec2<f32>(abs(along - len * 0.5) - len * 0.5, across - w);
            d = min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0)));
        }
    }
    return clamp(0.5 - d, 0.0, 1.0) * weight;
}

// Signed distance from p to the edge a->b, positive on the side `c` lies on.
fn edge(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, c: vec2<f32>) -> f32 {
    let e = b - a;
    let l = max(length(e), 1e-6);
    let n = vec2<f32>(-e.y, e.x) / l;
    let s = select(-1.0, 1.0, dot(c - a, n) >= 0.0);
    return dot(p - a, n) * s;
}

// Whether p is on c's side of the edge a-b, decided so that the two triangles sharing the edge
// compute the same value with opposite signs and exactly one of them owns every pixel: the
// endpoints are put in a fixed order and ties go to the positive side.
fn owns(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, c: vec2<f32>) -> bool {
    let swap = a.x > b.x || (a.x == b.x && a.y > b.y);
    let lo = select(a, b, swap);
    let hi = select(b, a, swap);
    let e = hi - lo;
    let side = e.x * (c.y - lo.y) - e.y * (c.x - lo.x);
    let v = e.x * (p.y - lo.y) - e.y * (p.x - lo.x);
    if (side > 0.0) {
        return v >= 0.0;
    }
    return v < 0.0;
}

// Coverage across a polygon edge a->b. Only the edge itself is antialiased: past its ends
// (where a concave polygon's interior continues along the edge's line) the edge cuts hard, so
// the neighbouring triangle that owns those pixels is not shaded through.
fn soft(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, c: vec2<f32>) -> f32 {
    let d = edge(p, a, b, c);
    let e = b - a;
    let t = dot(p - a, e) / max(dot(e, e), 1e-6);
    if (t < 0.0 || t > 1.0) {
        return select(0.0, 1.0, d >= 0.0);
    }
    return clamp(d + 0.5, 0.0, 1.0);
}

// Coverage of a filled triangle; `aa` marks the polygon-boundary edges, which are antialiased
// (bits 0, 1, 2 for a->b, b->c, c->a). Other edges are shared with a neighbouring triangle and
// cut hard, with exactly one of the two owning each pixel.
fn triangle(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, c: vec2<f32>, aa: u32) -> f32 {
    var cov = 1.0;
    if ((aa & 1u) == 1u) {
        cov = cov * soft(p, a, b, c);
    } else if (!owns(p, a, b, c)) {
        return 0.0;
    }
    if ((aa & 2u) == 2u) {
        cov = cov * soft(p, b, c, a);
    } else if (!owns(p, b, c, a)) {
        return 0.0;
    }
    if ((aa & 4u) == 4u) {
        cov = cov * soft(p, c, a, b);
    } else if (!owns(p, c, a, b)) {
        return 0.0;
    }
    return cov;
}

/// One image texel, premultiplied (the atlas holds straight alpha) so filtering doesn't darken edges.
fn image_texel(t: vec2<i32>, premultiplied: bool) -> vec4<f32> {
    let c = textureLoad(images, t, 0);
    if premultiplied { return c; }
    return vec4<f32>(c.rgb * c.a, c.a);
}

@fragment
fn fs(i: Out) -> @location(0) vec4<f32> {
    if (i.params.y == 8.0) {
        let p = i.pos.xy + projection.xy - i.rect.xy;
        if (any(p < vec2<f32>(0.0)) || any(p >= i.rect.zw)) { discard; }
        let color = textureLoad(images, vec2<i32>(floor(p)), 0);
        // Float intermediates accumulate One/One without saturation. Clamp before applying
        // this scope's opacity so nesting preserves plus-lighter's bounded coverage.
        return select(color, clamp(color, vec4(0.0), vec4(1.0)), i.params.x == 1.0) * i.fill.a;
    }
    let kind = i.params.y;
    let half = i.rect.zw * 0.5;
    let center = i.rect.xy + half;
    var col = vec4<f32>(0.0);
    if (kind < 0.5) {
        let d = sd_rounded(i.p - center, half, i.radii);
        let soft = max(i.params.z, 0.5);
        let cov = clamp(0.5 - d / (soft * 2.0), 0.0, 1.0);
        let bw = i.params.x;
        if (bw > 0.0) {
            let inner = clamp(0.5 - (d + bw) / (soft * 2.0), 0.0, 1.0);
            col = i.fill * inner + i.border * (cov - inner);
        } else {
            col = i.fill * cov;
        }
    } else if (kind < 1.5) {
        let sigma = max(i.params.z, 0.01);
        let corner = min(i.radii.x, min(half.x, half.y));
        col = i.fill * shadow(i.p - center, half, sigma, corner);
    } else if (kind > 6.5) {
        // Bilinear, clamped to the image's own texels so neighbours in the atlas never bleed.
        let local = (i.p - i.rect.xy) * i.uv.zw / i.rect.zw - vec2<f32>(0.5);
        let lo = vec2<i32>(i.uv.xy);
        let hi = lo + vec2<i32>(i.uv.zw) - vec2<i32>(1);
        let base = floor(local);
        let f = local - base;
        let t0 = clamp(lo + vec2<i32>(base), lo, hi);
        let t1 = clamp(lo + vec2<i32>(base) + vec2<i32>(1, 1), lo, hi);
        let premultiplied = i.params.z > 0.5;
        let c = mix(mix(image_texel(t0, premultiplied), image_texel(vec2<i32>(t1.x, t0.y), premultiplied), f.x),
                    mix(image_texel(vec2<i32>(t0.x, t1.y), premultiplied), image_texel(t1, premultiplied), f.x), f.y);
        let d = sd_rounded(i.p - center, half, i.radii);
        let cov = clamp(0.5 - d, 0.0, 1.0);
        col = c * (i.fill.a * cov);
    } else if (kind > 4.5 && kind < 5.5) {
        col = i.fill * segment(i.pos.xy + projection.xy, i.radii.xy, i.radii.zw, i.params.x, i.params.z > 0.5);
    } else if (kind > 5.5 && kind < 6.5) {
        // The pixel centre itself, not `p`: `p` is interpolated over each triangle's own quad, so
        // neighbours would disagree in the last bits about who owns a pixel on a shared edge.
        col = i.fill * triangle(i.pos.xy + projection.xy, i.radii.xy, i.radii.zw, i.uv.xy, u32(i.params.x + 0.5));
    } else if (kind > 2.5) {
        let d = sd_rounded(i.p - center, half, i.radii);
        let cov = clamp(0.5 - d, 0.0, 1.0);
        if (kind < 3.5) {
            // Inset shadow: the element's shape minus a blurred copy of it shrunk by the spread
            // and moved by the offset.
            let spread = i.params.x;
            let ih = max(half - vec2<f32>(spread), vec2<f32>(0.0));
            let q = i.p - center - i.uv.xy;
            var lit: f32;
            if (i.params.z < 0.3) {
                lit = clamp(0.5 - sd_rounded(q, ih, max(i.radii - vec4<f32>(spread), vec4<f32>(0.0))), 0.0, 1.0);
            } else {
                lit = shadow(q, ih, i.params.z, max(min(i.radii.x - spread, min(ih.x, ih.y)), 0.0));
            }
            col = i.fill * cov * (1.0 - clamp(lit, 0.0, 1.0));
        } else {
            let t = clamp((i.p.y - i.rect.y) / max(i.rect.w, 1.0), 0.0, 1.0);
            col = mix(i.fill, i.uv, t) * cov;
        }
    } else {
        // Interpolated screen coordinates can round below an integer texel
        // boundary. Fragment position is the rasterizer's exact pixel center.
        let p = i.pos.xy + projection.xy;
        var cov: f32;
        if i.uv.z > 0.0 {
            // Flying text keeps the original atlas footprint while its quad scales uniformly.
            let local = (p - i.rect.xy) * i.uv.zw / i.rect.zw - vec2<f32>(0.5);
            let uv = vec2<i32>(i.uv.xy + floor(local));
            let f = fract(local);
            cov = mix(mix(textureLoad(atlas, uv, 0).r, textureLoad(atlas, uv + vec2<i32>(1, 0), 0).r, f.x),
                      mix(textureLoad(atlas, uv + vec2<i32>(0, 1), 0).r, textureLoad(atlas, uv + vec2<i32>(1, 1), 0).r, f.x), f.y);
        } else {
            let texel = vec2<i32>(i.uv.xy + floor(p - i.rect.xy));
            cov = textureLoad(atlas, texel, 0).r;
        }
        col = i.fill * pow(cov, i.params.w);
    }
    var clip_p = i.p;
    if (kind > 1.5 && kind < 2.5 && i.params.x != 0.0) { clip_p = i.pos.xy + projection.xy; }
    return col * clip_alpha(clip_p, i.clip, i.fade);
}
