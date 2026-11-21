// rune-fx common prelude. Concatenated in front of every effect module.
// Invariants:
// - All effect maths runs in points (pt); `px = pt * scale`. Params arrive in pt.
// - Colours are linear-light; output is premultiplied (rgb may exceed a: additive light).
// - Everything is a pure function of (pixel, instance, time, seed): integer hashes only, no sin-hash,
//   so goldens are deterministic on every GPU modulo FMA.

struct Globals {
    viewport: vec2f, // target size in px
    dither: f32,     // 1 = add TPDF dither (8-bit targets)
    srgb: f32,       // 1 = sRGB-blended frame: encode the premultiplied output for a non-sRGB target
}

struct Inst {
    rect: vec4f,   // node rect pt (x, y, w, h)
    shape: vec4f,  // corner radius pt, scale px/pt, layer opacity, morph
    bounds: vec4f, // drawn quad px (x0, y0, x1, y1), already clipped to the viewport
    c0: vec4f,
    c1: vec4f,
    c2: vec4f,
    p0: vec4f,
    p1: vec4f,
    p2: vec4f,
    p3: vec4f,
    t: vec4f,      // time s, seed, variant, flags (bit0 reduced motion)
    aux: vec4f,
    aux2: vec4f,   // source rect pt (x, y, w, h): what the bound source texture covers
    xf: vec4f,     // CPU table: first vec4, counts... (per effect)
    clip: vec4f,   // window device-px clip rect (x0, y0, x1, y1)
    fade: vec4u,   // packed soft-edge and rounded-clip words (CLIP_WGSL)
}

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var<storage, read> insts: array<Inst>;
// Per-layer CPU tables (bolt paths, forks, embers, motion poses...), indexed from Inst.xf.x.
@group(0) @binding(2) var<storage, read> data: array<vec4f>;
fn dat1(base: u32, i: u32) -> f32 { return data[base + (i >> 2u)][i & 3u]; }
@group(1) @binding(0) var backdrop: texture_2d<f32>;
@group(1) @binding(1) var source: texture_2d<f32>;
@group(1) @binding(2) var samp: sampler;
// The backdrop's blur pyramid result (frosted glass only; see FxRenderer::blur_backdrop).
@group(1) @binding(3) var blurred: texture_2d<f32>;
@group(1) @binding(4) var<uniform> backdrop_encoding: vec4f;
// ---------- colour ----------
fn to_srgb(c: vec3f) -> vec3f {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3f(0.0)), vec3f(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3f(0.0031308));
}
fn to_lin(c: vec3f) -> vec3f {
    let lo = c / 12.92;
    let hi = pow((max(c, vec3f(0.0)) + 0.055) / 1.055, vec3f(2.4));
    return select(hi, lo, c <= vec3f(0.04045));
}
// In an sRGB-blended frame (`g.srgb > 0.5`), both a non-sRGB frame target and an `Rgba16Float`
// isolated layer hold sRGB-encoded premultiplied values (`to_srgb(straight) * a, a`). Decode to
// linear premultiplied light on sample so glass, lens, distort and backdrop dither/halftone shade
// in linear light before `fs` encodes once for the target.
fn bd_lin(c: vec4f) -> vec4f {
    if (backdrop_encoding.x > 0.5) {
        let a = max(c.a, 0.0);
        let straight = select(vec3f(0.0), c.rgb / a, a > 1e-6);
        return vec4f(to_lin(straight) * a, a);
    }
    return c;
}
// The backdrop at p (pt), sharp and blurred.
fn bd(p: vec2f, scale: f32) -> vec4f {
    return bd_lin(textureSampleLevel(backdrop, samp, p * scale / g.viewport, 0.0));
}
fn bdb(p: vec2f, scale: f32) -> vec3f {
    return bd_lin(textureSampleLevel(blurred, samp, p * scale / g.viewport, 0.0)).rgb;
}

struct VOut {
    @builtin(position) pos: vec4f,
    @location(0) @interpolate(flat) id: u32,
    @location(1) px: vec2f,
}

@vertex
fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VOut {
    let b = insts[ii].bounds;
    let corner = vec2f(f32(vi & 1u), f32((vi >> 1u) & 1u));
    let px = mix(b.xy, b.zw, corner);
    var o: VOut;
    o.pos = vec4f(px.x / g.viewport.x * 2.0 - 1.0, 1.0 - px.y / g.viewport.y * 2.0, 0.0, 1.0);
    o.id = ii;
    o.px = px;
    return o;
}

// The fragment's pixel centre in px, exactly: the interpolated `px` drifts by a few ulps that
// differ between quads, so two equal shapes at different places would shade a little apart. The
// draw origin is recovered from it to 1/256 px.
fn pixel(in: VOut) -> vec2f {
    return in.pos.xy + round((in.px - in.pos.xy) * 256.0) / 256.0;
}

fn effective_blend_mode(i: Inst) -> u32 {
    let m = (u32(i.t.w) >> 1u) & 7u;
    if ((m == 1u || m == 3u || m == 4u) && backdrop_encoding.y < 0.5) {
        return 0u;
    }
    return m;
}
fn blend_overlay_ch(b: f32, s: f32) -> f32 {
    return select(1.0 - 2.0 * (1.0 - b) * (1.0 - s), 2.0 * b * s, b <= 0.5);
}
fn blend_overlay(b: vec3f, s: vec3f) -> vec3f {
    return vec3f(blend_overlay_ch(b.r, s.r), blend_overlay_ch(b.g, s.g), blend_overlay_ch(b.b, s.b));
}
fn blend_soft_light_ch(b: f32, s: f32) -> f32 {
    if (s <= 0.5) {
        return b - (1.0 - 2.0 * s) * b * (1.0 - b);
    }
    let d = select(sqrt(max(b, 0.0)), ((16.0 * b - 12.0) * b + 4.0) * b, b <= 0.25);
    return b + (2.0 * s - 1.0) * (d - b);
}
fn blend_soft_light(b: vec3f, s: vec3f) -> vec3f {
    return vec3f(blend_soft_light_ch(b.r, s.r), blend_soft_light_ch(b.g, s.g), blend_soft_light_ch(b.b, s.b));
}

// Every effect module defines `shade`; the layer's opacity (shape.z) scales its premultiplied
// output, so a retiring one-shot fades instead of cutting.
@fragment
fn fs(in: VOut) -> @location(0) vec4f {
    let i = insts[in.id];
    let px = pixel(in);
    let ca = clip_alpha(px, i.clip, i.fade);
    if (ca <= 0.0) { return vec4f(0.0); }
    let c = shade(i, px) * i.shape.z;
    let mode = effective_blend_mode(i);
    if (mode == 0u) {
        if (g.srgb > 0.5) {
            // Premultiplied light may exceed alpha: a covering colour (up to alpha) plus added light.
            // Encode the straight covering colour times alpha, and the added light as over black, so
            // light doesn't dim as a glow's occlusion takes its alpha from zero to a little.
            let cover = min(max(c.rgb, vec3f(0.0)), vec3f(c.a));
            let straight = select(vec3f(0.0), cover / c.a, c.a > 1e-6);
            return vec4f(to_srgb(straight) * c.a + to_srgb(max(c.rgb, vec3f(0.0)) - cover), c.a) * ca;
        }
        return c * ca;
    }
    let m = max(c.a, max(c.r, max(c.g, c.b)));
    let as_ = clamp(m * ca, 0.0, 1.0);
    if (as_ <= 0.0) { return vec4f(0.0); }
    let cs_lin = select(vec3f(0.0), clamp(c.rgb / m, vec3f(0.0), vec3f(1.0)), m > 1e-6);
    let cs = select(cs_lin, to_srgb(cs_lin), g.srgb > 0.5);
    if (mode == 2u) {
        return vec4f(cs * as_, as_);
    }
    let b_lin = bd(px, 1.0);
    let ab = clamp(b_lin.a, 0.0, 1.0);
    let cb_lin = select(vec3f(0.0), clamp(b_lin.rgb / ab, vec3f(0.0), vec3f(1.0)), ab > 1e-6);
    let cb = select(cb_lin, to_srgb(cb_lin), g.srgb > 0.5);
    var blended = cb * cs;
    if (mode == 3u) { blended = blend_overlay(cb, cs); }
    if (mode == 4u) { blended = blend_soft_light(cb, cs); }
    return vec4f(mix(cs, blended, ab) * as_, as_);
}

const TAU: f32 = 6.2831853;
const PI: f32 = 3.14159265;

// ---------- hashing / noise (integer, deterministic) ----------
fn pcg(v: u32) -> u32 {
    let s = v * 747796405u + 2891336453u;
    let w = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
    return (w >> 22u) ^ w;
}
fn hu(a: u32, b: u32) -> u32 { return pcg(a ^ pcg(b + 0x9E3779B9u)); }
fn u01(n: u32) -> f32 { return f32(pcg(n) >> 8u) * (1.0 / 16777216.0); }
fn hcell(c: vec2i, seed: u32) -> f32 {
    return f32(pcg(bitcast<u32>(c.x) ^ pcg(bitcast<u32>(c.y) ^ pcg(seed))) >> 8u) * (1.0 / 16777216.0);
}
fn grad2(c: vec2i, seed: u32) -> vec2f {
    let a = hcell(c, seed) * TAU;
    return vec2f(cos(a), sin(a));
}
// Gradient noise, ~[-1, 1].
fn gnoise(p: vec2f, seed: u32) -> f32 {
    let i = vec2i(floor(p));
    let f = p - floor(p);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let a = dot(grad2(i, seed), f);
    let b = dot(grad2(i + vec2i(1, 0), seed), f - vec2f(1.0, 0.0));
    let c = dot(grad2(i + vec2i(0, 1), seed), f - vec2f(0.0, 1.0));
    let d = dot(grad2(i + vec2i(1, 1), seed), f - vec2f(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y) * 1.41421;
}
// Gradient noise periodic in x with integer period `per` (> 0).
fn pgnoise(p: vec2f, per: i32, seed: u32) -> f32 {
    let i = vec2i(floor(p));
    let f = p - floor(p);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let x0 = ((i.x % per) + per) % per;
    let x1 = (x0 + 1) % per;
    let a = dot(grad2(vec2i(x0, i.y), seed), f);
    let b = dot(grad2(vec2i(x1, i.y), seed), f - vec2f(1.0, 0.0));
    let c = dot(grad2(vec2i(x0, i.y + 1), seed), f - vec2f(0.0, 1.0));
    let d = dot(grad2(vec2i(x1, i.y + 1), seed), f - vec2f(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y) * 1.41421;
}
const ROT: mat2x2f = mat2x2f(0.8, 0.6, -0.6, 0.8);
fn fbm3(p0: vec2f, seed: u32) -> f32 {
    var p = p0; var a = 0.5; var s = 0.0;
    for (var o = 0u; o < 3u; o++) { s += a * gnoise(p, seed + o * 131u); p = ROT * p * 2.02; a *= 0.5; }
    return s / 0.875;
}
fn fbm5(p0: vec2f, seed: u32) -> f32 {
    var p = p0; var a = 0.5; var s = 0.0;
    for (var o = 0u; o < 5u; o++) { s += a * gnoise(p, seed + o * 131u); p = ROT * p * 2.02; a *= 0.5; }
    return s / 0.96875;
}
// Interleaved gradient noise (Jimenez), static per pixel: jitter that never shimmers.
fn ign(px: vec2f) -> f32 { return fract(52.9829189 * fract(dot(floor(px), vec2f(0.06711056, 0.00583715)))); }

// ---------- shapes ----------
fn sd_rrect(q: vec2f, half: vec2f, r: f32) -> f32 {
    let rr = clamp(r, 0.0, min(half.x, half.y));
    let k = abs(q) - half + rr;
    return length(max(k, vec2f(0.0))) + min(max(k.x, k.y), 0.0) - rr;
}
fn smin(a: f32, b: f32, k: f32) -> f32 {
    if (k <= 0.0) { return min(a, b); }
    let h = max(k - abs(a - b), 0.0) / k;
    return min(a, b) - h * h * k * 0.25;
}
// Arc-length coordinate of the closest boundary point of a centred rounded rect.
// Returns (s, total perimeter). Continuous everywhere except the single seam at s = 0 = total.
fn rrect_perim(q: vec2f, half: vec2f, r0: f32) -> vec2f {
    let r = clamp(r0, 0.0, min(half.x, half.y));
    let b = half - r;
    let a = abs(q);
    let lq = b.y + r * PI * 0.5 + b.x;
    var s: f32;
    if (a.x > b.x && a.y > b.y) {
        s = b.y + r * atan2(a.y - b.y, a.x - b.x);
    } else if ((a.x - b.x) > (a.y - b.y)) {
        s = min(a.y, b.y);
    } else {
        s = b.y + r * PI * 0.5 + (b.x - min(a.x, b.x));
    }
    var o: f32;
    if (q.x >= 0.0 && q.y >= 0.0) { o = s; }
    else if (q.x < 0.0 && q.y >= 0.0) { o = 2.0 * lq - s; }
    else if (q.x < 0.0) { o = 2.0 * lq + s; }
    else { o = 4.0 * lq - s; }
    return vec2f(o, 4.0 * lq);
}

// Shape context for the current instance.
struct Ctx {
    p: vec2f,      // pt
    q: vec2f,      // pt relative to node centre
    c: vec2f,      // node centre pt
    half: vec2f,
    r: f32,
    scale: f32,
    aa: f32,       // one px in pt
    d: f32,        // signed distance to the node shape, pt
    t: f32,
    seed: u32,
    variant: i32,
    reduced: bool,
}
fn ctx(i: Inst, px: vec2f) -> Ctx {
    var c: Ctx;
    c.scale = i.shape.y;
    c.p = px / c.scale;
    c.half = i.rect.zw * 0.5;
    c.c = i.rect.xy + c.half;
    c.q = (px - c.c * c.scale) / c.scale;
    c.r = i.shape.x;
    c.aa = 1.0 / c.scale;
    c.d = sd_rrect(c.q, c.half, c.r);
    c.t = i.t.x;
    c.seed = u32(i.t.y);
    c.variant = i32(i.t.z + 0.5);
    c.reduced = (u32(i.t.w) & 1u) != 0u;
    return c;
}
fn sd_grad(c: Ctx) -> vec2f {
    let e = 0.35;
    let dx = sd_rrect(c.q + vec2f(e, 0.0), c.half, c.r) - sd_rrect(c.q - vec2f(e, 0.0), c.half, c.r);
    let dy = sd_rrect(c.q + vec2f(0.0, e), c.half, c.r) - sd_rrect(c.q - vec2f(0.0, e), c.half, c.r);
    let gv = vec2f(dx, dy);
    return gv / max(length(gv), 1e-5);
}

// ---------- light profiles ----------
fn erf_approx(x: f32) -> f32 {
    let x2 = x * x;
    let a = 0.147;
    let v = sqrt(1.0 - exp(-x2 * (1.2732395 + a * x2) / (1.0 + a * x2)));
    return select(-v, v, x >= 0.0);
}
// Coverage of a half-plane blurred by a Gaussian of sigma s, at signed distance d.
fn cov(d: f32, s: f32) -> f32 { return 0.5 - 0.5 * erf_approx(d / (max(s, 1e-3) * 1.41421)); }
// Bloom of a shape edge: a tight lobe that carries most of the light plus a wider one, both
// Gaussian so the light is gone by ~2.5 r (heavy tails read as a uniform haze over dark UI).
// Windowed to `reach` so the quad edge never shows. Continuous through d = 0 (inside saturates).
fn bloom(d: f32, r: f32, reach: f32) -> f32 {
    let o = max(d, 0.0);
    let w = 1.0 - smoothstep(reach * 0.5, reach, o);
    return (0.65 * gauss(o, r * 0.35) + 0.35 * gauss(o, r * 1.2)) * w;
}
// Soft knee for light energy: linear when dim, approaching 1 when bright, so a tinted glow
// keeps its hue instead of clipping toward white.
fn knee(x: f32) -> f32 { return 1.0 - exp(-max(x, 0.0)); }
fn gauss(x: f32, w: f32) -> f32 { return exp(-(x * x) / max(w * w, 1e-6)); }
fn lum(c: vec3f) -> f32 { return dot(c, vec3f(0.2126, 0.7152, 0.0722)); }
// A colour as its light cools: dims and deepens (saturates) instead of just fading.
fn cooled(c: vec3f, heat: f32) -> vec3f {
    let deep = c * c / max(max(c.r, max(c.g, c.b)), 0.02);
    return mix(deep * 0.55, c, smoothstep(0.0, 0.75, heat));
}
fn segdist(p: vec2f, a: vec2f, b: vec2f) -> vec2f {
    let ab = b - a;
    let h = clamp(dot(p - a, ab) / max(dot(ab, ab), 1e-6), 0.0, 1.0);
    return vec2f(length(p - a - ab * h), h);
}

// Final write: clamp, optional TPDF dither (applied in sRGB space, only where there is signal).
fn finish(c0: vec4f, px: vec2f) -> vec4f {
    var c = max(c0, vec4f(0.0));
    c.a = min(c.a, 1.0);
    if (g.dither > 0.5) {
        let ip = vec2u(px);
        let n = (u01(ip.x + pcg(ip.y)) + u01(ip.x * 7919u + pcg(ip.y + 104729u))) - 1.0;
        let s = to_srgb(c.rgb);
        let gate = smoothstep(0.0, 3.0 / 255.0, max(s.r, max(s.g, s.b)));
        c = vec4f(to_lin(max(s + vec3f(n / 255.0 * gate), vec3f(0.0))), c.a);
    }
    return c;
}
