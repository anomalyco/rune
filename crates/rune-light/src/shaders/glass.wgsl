// Thick glass over the lit frame. Every glass is a slab: a flat bottom floating `gap` px above the UI and a top
// whose height rises over the bevel with a quarter-circle profile (vertical at the rim), plus an optional gentle
// dome. A viewer straight above sends one ray per pixel:
//
//   enter the top (Snell, n ≈ 1.5) → cross the glass to the bottom (Beer–Lambert tint over the path) → leave into
//   the air gap (Snell) and land on the UI, or, past the critical angle, reflect off the bottom (total internal
//   reflection) and climb back to the top, where it leaves toward the environment or stays trapped.
//
// The entry splits reflection and transmission with the exact dielectric Fresnel equations (unpolarised), the exit
// takes its own Fresnel loss. Dispersion: rays are traced at the two ends of the visible range (Cauchy IOR) and
// eight wavelengths interpolate between them, each weighted into RGB, so rims fringe as a smooth spectrum. Frost
// widens every refracted ray into a cone: the UI is read from a prefiltered pyramid at the cone's footprint.
// Highlights are GGX reflections of the glowing UI elements (sphere lights, Karis widening, normal-variance
// specular AA), and light the GI carries under the glass scatters into its bevel.

struct U {
    size: vec4<f32>,     // w, h, glass count, light count
    misc: vec4<f32>,     // pyramid levels, env strength, shadow, caustic
    shadow: vec4<f32>,   // shadow offset xy, blur, unused
};

struct Glass {
    a: vec4<f32>,        // rect x, y, w, h
    b: vec4<f32>,        // second shape rect (w = 0: none)
    r: vec4<f32>,        // radius a, radius b, smooth-union k, morph a→b
    p0: vec4<f32>,       // thickness, bevel, gap, dome
    p1: vec4<f32>,       // ior, dispersion (n400 − n700), frost, presence
    tint: vec4<f32>,     // absorption per px rgb, glow from GI
};

struct GLight {
    pos: vec4<f32>,      // x, y, z, radius
    color: vec4<f32>,    // rgb intensity, unused
};

@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var<storage, read> glass: array<Glass>;
@group(0) @binding(2) var<storage, read> glights: array<GLight>;
@group(0) @binding(3) var backdrop: texture_2d<f32>;
@group(0) @binding(4) var pyramid: texture_2d<f32>;
@group(0) @binding(5) var fluence: texture_2d<f32>;
@group(0) @binding(6) var samp: sampler;

const PI: f32 = 3.14159265;

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

// Apple's continuous corner (crates/rune/src/shaders/clip.wgsl `sd_continuous` / `sd_rounded`).
const CONTINUOUS_K: f32 = 1.52866483;
fn sd_continuous(p: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    let e = CONTINUOUS_K * r;
    let q = half - abs(p);
    let v = vec2<f32>(e) - q;
    if (v.x <= 0.0 || v.y <= 0.0) { return max(-q.x, -q.y); }
    let u = atan2(v.y, v.x) * (4.0 / PI) - 1.0;
    let u2 = u * u;
    return length(v) - e * (1.14602295 + u2 * (-0.37573161 + u2 * (0.30796609 - 0.07724354 * u2)));
}

fn sd_rrect(p: vec2<f32>, half: vec2<f32>, r0: f32) -> f32 {
    if (r0 < 0.0) { return sd_continuous(p, half, min(-r0, min(half.x, half.y))); }
    let r = min(r0, min(half.x, half.y));
    let q = abs(p) - half + vec2<f32>(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - r;
}

fn smin(a: f32, b: f32, k: f32) -> f32 {
    if (k <= 0.0) {
        return min(a, b);
    }
    let h = max(k - abs(a - b), 0.0) / k;
    return min(a, b) - h * h * k * 0.25;
}

fn sdf(g: Glass, p: vec2<f32>) -> f32 {
    let da = sd_rrect(p - (g.a.xy + g.a.zw * 0.5), g.a.zw * 0.5, g.r.x);
    if (g.b.z <= 0.0) {
        return da;
    }
    let db = sd_rrect(p - (g.b.xy + g.b.zw * 0.5), g.b.zw * 0.5, g.r.y);
    return mix(smin(da, db, g.r.z), db, g.r.w);
}

// The dome rides on shape A's inscribed capsule: its medial axis is a single segment at the inscribed radius, and
// the smoothstep profile has zero slope there and at the rim, so the top has no crease anywhere.
fn dome(g: Glass, p: vec2<f32>) -> f32 {
    let half = g.a.zw * 0.5;
    let ri = min(half.x, half.y);
    let dc = sd_rrect(p - (g.a.xy + half), half, ri);
    let x = clamp(-dc / ri, 0.0, 1.0);
    return g.p0.w * g.p1.w * x * x * (3.0 - 2.0 * x) * (1.0 - g.r.w);
}

// Top surface height above the UI plane at p (device px), given the shape's distance d there: a quarter-circle
// shoulder (vertical at the rim) up to the flat top, plus the dome.
fn height_d(g: Glass, p: vec2<f32>, d: f32) -> f32 {
    let thick = g.p0.x * g.p1.w;
    let bevel = max(g.p0.y, 1.0);
    let x = clamp(-d / bevel, 0.0, 1.0);
    let prof = sqrt(max(1.0 - (1.0 - x) * (1.0 - x), 0.0));
    return g.p0.z + thick * (0.04 + 0.96 * prof) + dome(g, p);
}

fn height(g: Glass, p: vec2<f32>) -> f32 {
    return height_d(g, p, sdf(g, p));
}

fn normal_at(g: Glass, p: vec2<f32>) -> vec3<f32> {
    let e = 0.5;
    let hx = height(g, p + vec2<f32>(e, 0.0)) - height(g, p - vec2<f32>(e, 0.0));
    let hy = height(g, p + vec2<f32>(0.0, e)) - height(g, p - vec2<f32>(0.0, e));
    return normalize(vec3<f32>(-hx / (2.0 * e), -hy / (2.0 * e), 1.0));
}

// Unpolarised dielectric Fresnel reflectance for a ray from medium n1 into n2.
fn fresnel(cos_i: f32, n1: f32, n2: f32) -> f32 {
    let ci = clamp(cos_i, 0.0, 1.0);
    let s2 = (n1 / n2) * (n1 / n2) * (1.0 - ci * ci);
    if (s2 >= 1.0) {
        return 1.0;
    }
    let ct = sqrt(1.0 - s2);
    let rs = (n1 * ci - n2 * ct) / (n1 * ci + n2 * ct);
    let rp = (n1 * ct - n2 * ci) / (n1 * ct + n2 * ci);
    return 0.5 * (rs * rs + rp * rp);
}

fn env(d: vec3<f32>) -> vec3<f32> {
    // A dim studio: deep blue-grey dome, a large soft box up and to the left, a thin strip to the right.
    let up = clamp(d.z, 0.0, 1.0);
    var c = mix(vec3<f32>(0.006, 0.007, 0.011), vec3<f32>(0.030, 0.034, 0.046), up);
    let box = normalize(vec3<f32>(-0.42, -0.58, 0.70));
    c = c + vec3<f32>(0.85, 0.88, 0.95) * smoothstep(0.80, 0.97, dot(d, box)) * 4.0;
    let strip = normalize(vec3<f32>(0.80, 0.10, 0.59));
    c = c + vec3<f32>(0.55, 0.62, 0.80) * smoothstep(0.93, 0.99, dot(d, strip)) * 2.0;
    return c * u.misc.y;
}

// Cubic B-spline sample of one pyramid level (4 bilinear taps), so heavy blur never shows texel diamonds.
fn bspline(uv: vec2<f32>, lod: f32) -> vec3<f32> {
    let dim = vec2<f32>(textureDimensions(pyramid, i32(lod)));
    let st = uv * dim - 0.5;
    let i = floor(st);
    let f = st - i;
    let w0 = (1.0 - f) * (1.0 - f) * (1.0 - f) / 6.0;
    let w1 = (4.0 - 6.0 * f * f + 3.0 * f * f * f) / 6.0;
    let w2 = (1.0 + 3.0 * f + 3.0 * f * f - 3.0 * f * f * f) / 6.0;
    let w3 = f * f * f / 6.0;
    let g0 = w0 + w1;
    let g1 = w2 + w3;
    let h0 = (i + vec2<f32>(-1.0) + w1 / g0 + 0.5) / dim;
    let h1 = (i + vec2<f32>(1.0) + w3 / g1 + 0.5) / dim;
    let a = textureSampleLevel(pyramid, samp, vec2<f32>(h0.x, h0.y), lod).rgb;
    let b = textureSampleLevel(pyramid, samp, vec2<f32>(h1.x, h0.y), lod).rgb;
    let c = textureSampleLevel(pyramid, samp, vec2<f32>(h0.x, h1.y), lod).rgb;
    let d = textureSampleLevel(pyramid, samp, vec2<f32>(h1.x, h1.y), lod).rgb;
    return g0.y * (g0.x * a + g1.x * b) + g1.y * (g0.x * c + g1.x * d);
}

// The UI at p read through a blur of `radius` px (0 = sharp).
fn ui_at(p: vec2<f32>, radius: f32) -> vec3<f32> {
    let uv = p / u.size.xy;
    let sharp = textureSampleLevel(backdrop, samp, uv, 0.0).rgb;
    if (radius <= 1.0) {
        return sharp;
    }
    // Pyramid level k has a footprint of about 2^(k+1) px.
    let lod = clamp(log2(radius) - 1.0, 0.0, u.misc.x - 1.0);
    let l0 = floor(lod);
    let blur = mix(bspline(uv, l0), bspline(uv, min(l0 + 1.0, u.misc.x - 1.0)), lod - l0);
    return mix(sharp, blur, smoothstep(1.0, 3.0, radius));
}

fn glass_shadow(g: Glass, p: vec2<f32>) -> f32 {
    let blur = u.shadow.z;
    let d = sdf(g, p - u.shadow.xy * g.p1.w);
    let s = 1.0 - smoothstep(-blur, blur * 1.6, d);
    let contact = 1.0 - smoothstep(0.0, 10.0, sdf(g, p));
    return clamp((s * 0.7 + contact * 0.3) * u.misc.z * g.p1.w, 0.0, 0.9);
}

// The floor as glass sees it: the UI with the glass's own shadow and a bright caustic ring where the bevel focuses
// the light the GI carries around it.
fn floor_at(g: Glass, q: vec2<f32>, radius: f32) -> vec3<f32> {
    var c = ui_at(q, radius);
    let dq = sdf(g, q);
    let sh = glass_shadow(g, q);
    let flu = textureSampleLevel(fluence, samp, q / u.size.xy, 0.0).rgb;
    let bevel = max(g.p0.y, 1.0);
    let ring = exp(-pow((-dq - bevel * 0.45) / (bevel * 0.22), 2.0)) * select(0.0, 1.0, dq < 0.0);
    c = c * (1.0 - sh) + (flu * 0.55 + vec3<f32>(0.01)) * ring * u.misc.w * g.p1.w;
    return c;
}

struct Path {
    q: vec2<f32>,        // where it lands on the UI (floor = true)
    dir: vec3<f32>,      // or the direction it leaves the glass toward the environment
    len: f32,            // path length inside the glass
    t_out: f32,          // exit transmission (Fresnel)
    floor: bool,
};

fn trace(g: Glass, p: vec2<f32>, n: vec3<f32>, zt: f32, ior: f32) -> Path {
    var o: Path;
    let v = vec3<f32>(0.0, 0.0, -1.0);
    let r1 = refract(v, n, 1.0 / ior);
    let s = (zt - g.p0.z) / max(-r1.z, 1e-3);
    let pb = p + r1.xy * s;
    let up = vec3<f32>(0.0, 0.0, 1.0);
    let ci = -r1.z;
    let r2 = refract(r1, up, ior);
    if (dot(r2, r2) > 0.0) {
        o.floor = true;
        o.q = pb + r2.xy * (g.p0.z / max(-r2.z, 0.05));
        o.len = s;
        o.t_out = 1.0 - fresnel(ci, ior, 1.0);
        return o;
    }
    // Total internal reflection off the bottom: climb back to the top surface.
    let r1b = reflect(r1, up);
    var t = (zt - g.p0.z) / max(r1b.z, 1e-3);
    for (var k = 0; k < 4; k = k + 1) {
        t = (height(g, pb + r1b.xy * t) - g.p0.z) / max(r1b.z, 1e-3);
    }
    let pe = pb + r1b.xy * t;
    let ne = normal_at(g, pe);
    let r3 = refract(r1b, -ne, ior);
    o.floor = false;
    o.len = s + t;
    if (dot(r3, r3) > 0.0) {
        o.dir = r3;
        o.t_out = 1.0 - fresnel(dot(r1b, ne), ior, 1.0);
    } else {
        // Trapped: it rattles on until absorbed; a little leaks out as a dim mean of the environment.
        o.dir = reflect(r1b, ne);
        o.t_out = 0.25;
    }
    return o;
}

// Approximate CIE response of eight wavelengths 400…700 nm in linear sRGB, normalised so they sum to white.
fn spectrum(i: i32) -> vec3<f32> {
    var w = array<vec3<f32>, 8>(
        vec3<f32>(0.20, 0.00, 0.62),
        vec3<f32>(0.12, 0.02, 1.00),
        vec3<f32>(0.00, 0.22, 0.86),
        vec3<f32>(0.00, 0.62, 0.30),
        vec3<f32>(0.18, 0.92, 0.00),
        vec3<f32>(0.70, 0.62, 0.00),
        vec3<f32>(1.00, 0.20, 0.00),
        vec3<f32>(0.62, 0.00, 0.00),
    );
    return w[i];
}

fn ggx(nh: f32, a: f32) -> f32 {
    let a2 = a * a;
    let d = nh * nh * (a2 - 1.0) + 1.0;
    return a2 / (PI * d * d);
}

// One glass sample at p: returns the shaded colour and coverage.
fn shade(g: Glass, p: vec2<f32>, spec_var: f32, fp: f32) -> vec4<f32> {
    let d = sdf(g, p);
    let cov = clamp(0.5 - d, 0.0, 1.0);
    if (cov <= 0.0) {
        return vec4<f32>(0.0);
    }
    let zt = height_d(g, p, d);
    let n = normal_at(g, p);
    let ior = g.p1.x;
    let disp = g.p1.y;
    let frost = g.p1.z;
    // Cauchy: n(λ) = A + B/λ², with n(550) = ior and n(400) − n(700) = disp.
    let B = disp / (1.0 / (0.4 * 0.4) - 1.0 / (0.7 * 0.7));
    let A = ior - B / (0.55 * 0.55);
    let n_lo = A + B / (0.7 * 0.7);
    let n_hi = A + B / (0.4 * 0.4);
    let a = trace(g, p, n, zt, n_lo);
    let b = trace(g, p, n, zt, n_hi);
    let thick = max(g.p0.x, 1.0);
    var trans = vec3<f32>(0.0);
    var wsum = vec3<f32>(0.0);
    for (var i = 0; i < 8; i = i + 1) {
        let k = f32(i) / 7.0;          // 0 = 400 nm (n_hi), 1 = 700 nm (n_lo)
        let w = spectrum(i);
        var c: vec3<f32>;
        var len: f32;
        var tout: f32;
        if (a.floor && b.floor) {
            let q = mix(b.q, a.q, k);
            len = mix(b.len, a.len, k);
            tout = mix(b.t_out, a.t_out, k);
            c = floor_at(g, q, max(frost * (6.0 + len * 0.35), fp));
        } else if (!a.floor && !b.floor) {
            let dir = normalize(mix(b.dir, a.dir, k));
            len = mix(b.len, a.len, k);
            tout = mix(b.t_out, a.t_out, k);
            c = env(dir);
        } else {
            var pa = b;
            if (k > 0.5) {
                pa = a;
            }
            len = pa.len;
            tout = pa.t_out;
            if (pa.floor) {
                c = floor_at(g, pa.q, max(frost * (6.0 + len * 0.35), fp));
            } else {
                c = env(pa.dir);
            }
        }
        let beer = exp(-g.tint.rgb * len);
        trans = trans + w * c * beer * tout;
        wsum = wsum + w;
    }
    trans = trans / wsum;

    let v_up = vec3<f32>(0.0, 0.0, 1.0);
    let F = fresnel(n.z, 1.0, ior);
    // Reflection: upward rays see the studio, downward ones (off steep walls) the lit UI beside the glass, glowing
    // elements included: those are the coloured highlights on the bevel.
    let rr = reflect(vec3<f32>(0.0, 0.0, -1.0), n);
    let reach = min(zt * length(rr.xy) / max(-rr.z, 1e-3), zt * 4.0);
    let below = ui_at(p + normalize(rr.xy + vec2<f32>(1e-5)) * reach, 4.0 + reach * 0.08);
    let refl = mix(env(rr), below, smoothstep(0.05, -0.15, rr.z));
    // Frost scatters the light passing through into a milky veil.
    let flu = textureSampleLevel(fluence, samp, p / u.size.xy, 0.0).rgb;
    var col = trans * (1.0 - F) + refl * F;
    col = col + flu * frost * 0.10;

    // Highlights: the glowing UI elements as sphere lights.
    let P = vec3<f32>(p, zt);
    let rough = mix(0.035, 0.32, frost);
    var spec = vec3<f32>(0.0);
    let nl = i32(u.size.w);
    for (var i = 0; i < nl; i = i + 1) {
        let L = glights[i];
        let D = L.pos.xyz - P;
        let dist = length(D);
        let l = D / max(dist, 1e-3);
        let ndl = max(dot(n, l), 0.0);
        if (ndl <= 0.0) {
            continue;
        }
        let h = normalize(l + v_up);
        let a0 = sqrt(rough * rough + spec_var);
        let al = clamp(a0 + L.pos.w / (2.0 * dist), 0.02, 1.0);
        let energy = (a0 * a0) / (al * al);
        let fh = fresnel(max(dot(l, h), 0.0), 1.0, ior);
        let fall = 1.0 / (1.0 + dist * dist / (L.pos.w * L.pos.w + 1.0));
        spec = spec + L.color.rgb * ggx(max(dot(n, h), 0.0), al) * energy * fh * ndl * fall;
    }
    col = col + spec;

    // GI light that enters the bevel scatters along it: rims pick up the colour of the lights around them.
    let bevel = max(g.p0.y, 1.0);
    let rim = pow(1.0 - clamp(-d / bevel, 0.0, 1.0), 2.0);
    col = col + flu * rim * g.tint.w;
    return vec4<f32>(col, cov);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = pos.xy;
    let base = textureLoad(backdrop, vec2<i32>(p), 0).rgb;
    let ng = i32(u.size.z);
    let reach = u.shadow.z * 2.5 + length(u.shadow.xy) + 12.0;
    var col = base;
    // No `continue` in this loop: every iteration stays in uniform control flow for the derivatives.
    for (var gk = 0; gk < ng; gk = gk + 1) {
        let g = glass[gk];
        let d = sdf(g, p);
        // Normal variance for specular AA (Tokuyoshi & Kaplanyan), and the footprint of the refracted lookup: where
        // the bevel squeezes the UI into a thin band, one pixel covers many, so the lookup is prefiltered to match
        // instead of aliasing into streaks. Both come from this slab's values, computed only near it (a shaded pixel's
        // quad neighbours are all within a few px of the edge).
        var nv = vec3<f32>(0.0, 0.0, 1.0);
        var q0 = p;
        if (g.p1.w > 0.0 && d < 8.0) {
            nv = normal_at(g, p);
            let tr = trace(g, p, nv, height(g, p), g.p1.x);
            q0 = select(p + tr.dir.xy * 400.0, tr.q, tr.floor);
        }
        let dn = dpdx(nv);
        let dm = dpdy(nv);
        let spec_var = min(2.0 * 0.25 * (dot(dn, dn) + dot(dm, dm)), 0.18);
        let fpx = max(length(dpdx(q0)), length(dpdy(q0)));
        let fp = clamp(fpx * 0.5 - 0.5, 0.0, 24.0);
        if (g.p1.w > 0.0 && d <= reach) {
            // Shadow on the UI outside the glass.
            let outside = col * (1.0 - glass_shadow(g, p));
            let bevel = max(g.p0.y, 1.0);
            if (d > 1.0) {
                col = outside;
            } else if (-d < bevel + 1.0) {
                // Supersample the bevel (rotated grid), where refraction changes fastest.
                let o = array<vec2<f32>, 4>(vec2<f32>(-0.125, -0.375), vec2<f32>(0.375, -0.125), vec2<f32>(0.125, 0.375), vec2<f32>(-0.375, 0.125));
                var acc = vec4<f32>(0.0);
                for (var k = 0; k < 4; k = k + 1) {
                    let s = shade(g, p + o[k], spec_var, fp);
                    acc = acc + vec4<f32>(s.rgb * s.a, s.a);
                }
                acc = acc * 0.25;
                col = outside * (1.0 - acc.a) + acc.rgb;
            } else {
                let s = shade(g, p, spec_var, fp);
                col = mix(outside, s.rgb, s.a);
            }
        }
    }
    return vec4<f32>(col, 1.0);
}

// ---- pyramid ----
// 13-tap downsample (Jimenez, "Next generation post processing in Call of Duty: AW"): no box aliasing, no flicker.
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var ssamp: sampler;

@fragment
fn fs_down(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let sz = vec2<f32>(textureDimensions(src, 0));
    let uv = pos.xy * 2.0 / sz;
    let t = 1.0 / sz;
    let a = textureSampleLevel(src, ssamp, uv + t * vec2<f32>(-2.0, -2.0), 0.0).rgb;
    let b = textureSampleLevel(src, ssamp, uv + t * vec2<f32>(0.0, -2.0), 0.0).rgb;
    let c = textureSampleLevel(src, ssamp, uv + t * vec2<f32>(2.0, -2.0), 0.0).rgb;
    let d = textureSampleLevel(src, ssamp, uv + t * vec2<f32>(-2.0, 0.0), 0.0).rgb;
    let e = textureSampleLevel(src, ssamp, uv, 0.0).rgb;
    let f = textureSampleLevel(src, ssamp, uv + t * vec2<f32>(2.0, 0.0), 0.0).rgb;
    let g = textureSampleLevel(src, ssamp, uv + t * vec2<f32>(-2.0, 2.0), 0.0).rgb;
    let h = textureSampleLevel(src, ssamp, uv + t * vec2<f32>(0.0, 2.0), 0.0).rgb;
    let i = textureSampleLevel(src, ssamp, uv + t * vec2<f32>(2.0, 2.0), 0.0).rgb;
    let j = textureSampleLevel(src, ssamp, uv + t * vec2<f32>(-1.0, -1.0), 0.0).rgb;
    let k = textureSampleLevel(src, ssamp, uv + t * vec2<f32>(1.0, -1.0), 0.0).rgb;
    let l = textureSampleLevel(src, ssamp, uv + t * vec2<f32>(-1.0, 1.0), 0.0).rgb;
    let m = textureSampleLevel(src, ssamp, uv + t * vec2<f32>(1.0, 1.0), 0.0).rgb;
    let o = e * 0.125 + (a + c + g + i) * 0.03125 + (b + d + f + h) * 0.0625 + (j + k + l + m) * 0.125;
    return vec4<f32>(o, 1.0);
}
