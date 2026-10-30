// The reel's own passes: visualisations of rune-light's real GI buffers, the 2.5D layer stack, mips, and the final
// encode (sRGB → BT.709 Y'CbCr 4:2:0 with blue-noise TPDF dither at the exact quantisation step).

struct G {
    size: vec4<f32>,     // W, H, gi w, gi h
    gi: vec4<f32>,       // px per gi texel, time, highlighted probe count, unused
    hp: array<vec4<f32>, 2>,
};

struct VP {
    a: vec4<f32>,        // level, merged mix (0 own … 1 merged), exposure, unused
    b: vec4<f32>,        // cells, probe dots, rays, rings
    c: vec4<f32>,        // ray growth 0…1, alpha, border, unused
    q0: vec4<f32>,       // layer quad clip corners: TL, TR, BL, BR
    q1: vec4<f32>,
    q2: vec4<f32>,
    q3: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: G;
@group(0) @binding(1) var<storage, read> vp: array<VP>;
@group(0) @binding(2) var field: texture_2d<f32>;
@group(0) @binding(3) var emit: texture_2d<f32>;
@group(0) @binding(4) var occ: texture_2d<f32>;
@group(0) @binding(5) var c_own: texture_2d<f32>;
@group(0) @binding(6) var c_mrg: texture_2d<f32>;
@group(0) @binding(7) var m_own: texture_2d<f32>;
@group(0) @binding(8) var m_mrg: texture_2d<f32>;
@group(0) @binding(9) var flu: texture_2d<f32>;
@group(0) @binding(10) var lsamp: sampler;
@group(0) @binding(11) var layer: texture_2d<f32>;
@group(0) @binding(12) var asamp: sampler;

const TAU: f32 = 6.28318531;

struct FOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) k: u32,
};

@vertex
fn vs_full(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> FOut {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var o: FOut;
    o.pos = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    o.k = ii;
    return o;
}

fn tone(x: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(1.0) - exp(-max(x, vec3<f32>(0.0)));
}

// Panel colour behind every visualisation.
fn panel(p: vec2<f32>) -> vec3<f32> {
    let uv = p / g.size.xy;
    let v = 1.0 - 0.25 * dot(uv - 0.5, uv - 0.5);
    return vec3<f32>(0.0075, 0.0085, 0.013) * v;
}

fn aa_line(d: f32, w: f32) -> f32 {
    let fw = max(fwidth(d), 1e-4);
    return 1.0 - smoothstep(w * 0.5 - fw * 0.5, w * 0.5 + fw * 0.5, abs(d));
}

// ---- field: the GI distance (px, clamped at 128) as bands and contours ----
@fragment
fn fs_field(i: FOut) -> @location(0) vec4<f32> {
    let p = i.pos.xy;
    let f = textureSampleLevel(field, lsamp, p / g.size.xy, 0.0);
    let d = f.w;
    var col = panel(p);
    let fw = max(fwidth(d), 1e-3);
    if (d < 0.0) {
        let x = clamp(-d / 60.0, 0.0, 1.0);
        col = mix(vec3<f32>(0.20, 0.085, 0.03), vec3<f32>(0.06, 0.02, 0.008), x);
        let band = abs(fract(-d / 12.0 + 0.5) - 0.5) * 12.0;
        col = col + vec3<f32>(0.35, 0.16, 0.05) * (1.0 - smoothstep(0.0, fw * 1.2, band)) * 0.5;
    } else {
        let x = clamp(d / 128.0, 0.0, 1.0);
        col = col + mix(vec3<f32>(0.03, 0.10, 0.16), vec3<f32>(0.004, 0.012, 0.025), sqrt(x)) * (1.0 - smoothstep(120.0, 128.0, d));
        let band = abs(fract(d / 16.0 + 0.5) - 0.5) * 16.0;
        let line = (1.0 - smoothstep(0.0, fw * 1.2, band)) * (1.0 - smoothstep(96.0, 127.0, d));
        col = col + vec3<f32>(0.10, 0.32, 0.48) * line * (0.55 - 0.4 * x);
    }
    let zero = 1.0 - smoothstep(0.0, fw * 1.6, abs(d));
    col = col + vec3<f32>(0.95, 0.85, 0.7) * zero;
    return vec4<f32>(col, 1.0);
}

// ---- scene: the media the cascades march through, at gi resolution (nearest, so texels show) ----
@fragment
fn fs_scene(i: FOut) -> @location(0) vec4<f32> {
    let p = i.pos.xy;
    let v = vp[i.k];
    let t = vec2<i32>(p / g.gi.x);
    let em = textureLoad(emit, t, 0);
    let oc = textureLoad(occ, t, 0);
    var col = panel(p);
    // Card coverage as slate, emitters as their radiance (σ·L / σ) times coverage.
    col = col + vec3<f32>(0.045, 0.05, 0.065) * oc.a;
    let cov = clamp(em.a / 1.5, 0.0, 1.0);
    let rad = em.rgb / max(em.a, 1e-4);
    col = col + tone(rad * v.a.z) * cov;
    return vec4<f32>(col, 1.0);
}

fn ray_tex(n: u32, probe: vec2<i32>, j: u32) -> vec2<i32> {
    let b = 2u << n;
    return probe * i32(b) + vec2<i32>(i32(j % b), i32(j / b));
}

// ---- one cascade: per-probe mean radiance as cells, probe dots, and the real rays of a few probes ----
@fragment
fn fs_cascade(i: FOut) -> @location(0) vec4<f32> {
    let p = i.pos.xy;
    let v = vp[i.k];
    let n = u32(v.a.x + 0.5);
    let mixk = v.a.y;
    let expo = v.a.z;
    let s = f32(1u << n);
    let sp = s * g.gi.x;
    let nprobe = vec2<i32>(g.size.zw / s);
    var col = vec3<f32>(0.0);
    var alpha = 0.0;

    let idx = clamp(vec2<i32>(floor(p / sp)), vec2<i32>(0), nprobe - vec2<i32>(1));
    let c = (vec2<f32>(idx) + 0.5) * sp;
    let local = p - c;
    let gap = max(0.75, sp * 0.07);
    let half = sp * 0.5 - gap;
    let r = sp * 0.16;
    let q = abs(local) - vec2<f32>(half - r);
    let dcell = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
    let cell = 1.0 - smoothstep(-0.6, 0.6, dcell);
    let val = mix(textureLoad(m_own, idx, 0).rgb, textureLoad(m_mrg, idx, 0).rgb, mixk);
    col = col + (tone(val * expo) * 0.95 + vec3<f32>(0.02, 0.024, 0.034)) * cell * v.b.x;
    alpha = max(alpha, cell * v.b.x * 0.8);
    let dr = max(0.9, sp * 0.06);
    let dot_k = (1.0 - smoothstep(dr - 0.6, dr + 0.6, length(local))) * v.b.y * 0.6 * smoothstep(9.0, 16.0, sp);
    col = mix(col, vec3<f32>(0.85, 0.9, 1.0), dot_k);

    // Highlighted probes: interval ring, and every ray as a segment coloured by the radiance it carries.
    let t0 = (pow(4.0, f32(n)) - 1.0) / 3.0 * g.gi.x;
    let t1 = (pow(4.0, f32(n) + 1.0) - 1.0) / 3.0 * g.gi.x;
    let nd = 4u << (2u * n);
    let grow = v.c.x;
    let t1g = mix(t0 + 0.001, t1, grow);
    let nh = i32(g.gi.z);
    var ray = vec3<f32>(0.0);
    var ring = 0.0;
    for (var h = 0; h < nh; h = h + 1) {
        let hp4 = g.hp[h / 2];
        var hp = hp4.xy;
        if ((h & 1) == 1) {
            hp = hp4.zw;
        }
        let pidx = clamp(vec2<i32>(floor(hp / sp)), vec2<i32>(0), nprobe - vec2<i32>(1));
        let pc = (vec2<f32>(pidx) + 0.5) * sp;
        let rel = p - pc;
        let rr = length(rel);
        // Interval band and its edges.
        let band = smoothstep(t0 - 1.0, t0 + 1.0, rr) * (1.0 - smoothstep(t1g - 1.0, t1g + 1.0, rr));
        ring = ring + band * 0.025 + (aa_line(rr - t0, v.c.w) + aa_line(rr - t1g, v.c.w) * step(0.01, grow)) * 0.55;
        // Every ray of C0–C2; for C3 and C4 an evenly spaced subset, so neighbouring rays stay ≥ ~14 px apart where
        // they start (all 1024 of C4's would merge into a disc).
        let gap_px = max(t0, 1.0) * TAU / f32(nd);
        let stride = u32(max(1.0, exp2(ceil(log2(max(14.0 / gap_px, 1.0))))));
        let shown = nd / stride;
        if (rr > t0 - 3.0 && rr < t1g + 3.0) {
            var ang = atan2(rel.y, rel.x);
            if (ang < 0.0) {
                ang = ang + TAU;
            }
            let fj = ang / TAU * f32(shown) - 0.5;
            let j0 = i32(floor(fj));
            for (var o = 0; o < 2; o = o + 1) {
                let js = u32((j0 + o + i32(shown)) % i32(shown));
                let j = js * stride + stride / 2u;
                let a = (f32(j) + 0.5) / f32(nd) * TAU;
                let dir = vec2<f32>(cos(a), sin(a));
                let along = clamp(dot(rel, dir), t0, t1g);
                let dist = length(rel - dir * along);
                let rw = v.c.w;
                let fade = 1.0 - 0.6 * clamp((along - t0) / max(t1 - t0, 1.0), 0.0, 1.0);
                let w = (1.0 - smoothstep(rw * 0.5 - 0.6, rw * 0.5 + 0.6, dist)) * fade;
                if (w > 0.0) {
                    let tx = ray_tex(n, pidx, j);
                    let rv = mix(textureLoad(c_own, tx, 0).rgb, textureLoad(c_mrg, tx, 0).rgb, mixk);
                    ray = max(ray, (tone(rv * expo * 2.5) * 1.3 + vec3<f32>(0.06, 0.065, 0.085)) * w);
                }
            }
        }
        // The probe itself.
        let pd = length(rel);
        ring = ring + (1.0 - smoothstep(6.0, 8.0, pd)) * 1.2 + aa_line(pd - 14.0, v.c.w) * 0.8;
    }
    let rk = ray * v.b.z;
    let gk = clamp(ring * v.b.w, 0.0, 1.0);
    col = col + rk + vec3<f32>(0.75, 0.82, 1.0) * gk;
    alpha = clamp(max(alpha, max(max(rk.r, rk.g), rk.b) * 1.5 + gk), 0.0, 1.0);
    return vec4<f32>(col, alpha);
}

// ---- fluence (cascade 0's directions averaged) ----
@fragment
fn fs_fluence(i: FOut) -> @location(0) vec4<f32> {
    let p = i.pos.xy;
    let v = vp[i.k];
    let f = textureSampleLevel(flu, lsamp, p / g.size.xy, 0.0).rgb;
    return vec4<f32>(panel(p) + tone(f * v.a.z), 1.0);
}

// ---- accumulate a sub-frame (shutter): additive, weighted ----
@fragment
fn fs_accum(i: FOut) -> @location(0) vec4<f32> {
    let c = textureLoad(layer, vec2<i32>(i.pos.xy), 0);
    return vec4<f32>(c.rgb * vp[i.k].a.x, vp[i.k].a.x);
}

// ---- mips: 2×2 box from the level above ----
@fragment
fn fs_mip(i: FOut) -> @location(0) vec4<f32> {
    let sz = vec2<f32>(textureDimensions(layer, 0));
    let uv = i.pos.xy * 2.0 / sz;
    return textureSampleLevel(layer, lsamp, uv, 0.0);
}

// ---- 2.5D layer stack ----
struct LOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) k: u32,
};

@vertex
fn vs_layer(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> LOut {
    let v = vp[ii];
    var q = v.q0;
    if (vi == 1u) {
        q = v.q1;
    } else if (vi == 2u) {
        q = v.q2;
    } else if (vi == 3u) {
        q = v.q3;
    }
    var o: LOut;
    o.pos = q;
    o.uv = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    o.k = ii;
    return o;
}

@fragment
fn fs_layer(i: LOut) -> @location(0) vec4<f32> {
    let v = vp[i.k];
    let c = textureSample(layer, asamp, i.uv).rgb;
    // Edge distance in screen px: antialiased quad edges and a hairline frame.
    let fw = max(fwidth(i.uv), vec2<f32>(1e-6));
    let e2 = min(i.uv, vec2<f32>(1.0) - i.uv) / fw;
    let e = min(e2.x, e2.y);
    let alpha = v.c.y * clamp(e, 0.0, 1.0);
    let frame = (1.0 - smoothstep(0.8, 1.8, e)) * v.c.z;
    let rgb = c + vec3<f32>(0.55, 0.62, 0.78) * frame;
    return vec4<f32>(rgb * alpha, alpha);
}

// ---- bloom ----
// Physically based bloom (Jimenez, "Next generation post processing in Call of Duty: AW"): a 13-tap downsample
// chain (Karis luma-weighted average on the first step, so a lone bright pixel never flickers), a 3×3 tent upsample
// added back up the chain, and a small energy-conserving mix at the end.
struct BU {
    p: vec4<f32>,        // karis 0/1, tent radius (texels), unused, unused
};
@group(0) @binding(0) var<uniform> bu: BU;
@group(0) @binding(1) var bsrc: texture_2d<f32>;
@group(0) @binding(2) var bsamp: sampler;

@vertex
fn vs_b(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

fn karis(c: vec3<f32>) -> f32 {
    return 1.0 / (1.0 + dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)));
}

@fragment
fn fs_bdown(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let sz = vec2<f32>(textureDimensions(bsrc, 0));
    let uv = pos.xy * 2.0 / sz;
    let t = 1.0 / sz;
    let s = array<vec2<f32>, 13>(
        vec2<f32>(-2.0, -2.0), vec2<f32>(0.0, -2.0), vec2<f32>(2.0, -2.0),
        vec2<f32>(-2.0, 0.0), vec2<f32>(0.0, 0.0), vec2<f32>(2.0, 0.0),
        vec2<f32>(-2.0, 2.0), vec2<f32>(0.0, 2.0), vec2<f32>(2.0, 2.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, 1.0),
    );
    var c: array<vec3<f32>, 13>;
    for (var k = 0; k < 13; k = k + 1) {
        c[k] = max(textureSampleLevel(bsrc, bsamp, uv + t * s[k], 0.0).rgb, vec3<f32>(0.0));
    }
    // Five overlapping 2×2 boxes: the centre one (j k l m) weighs 0.5, the four corners 0.125 each.
    let b0 = (c[9] + c[10] + c[11] + c[12]) * 0.25;
    let b1 = (c[0] + c[1] + c[3] + c[4]) * 0.25;
    let b2 = (c[1] + c[2] + c[4] + c[5]) * 0.25;
    let b3 = (c[3] + c[4] + c[6] + c[7]) * 0.25;
    let b4 = (c[4] + c[5] + c[7] + c[8]) * 0.25;
    var w = vec4<f32>(0.5, 0.125, 0.125, 0.125);
    var w4 = 0.125;
    if (bu.p.x > 0.5) {
        w = w * vec4<f32>(karis(b0), karis(b1), karis(b2), karis(b3));
        w4 = w4 * karis(b4);
    }
    let o = (b0 * w.x + b1 * w.y + b2 * w.z + b3 * w.w + b4 * w4) / (w.x + w.y + w.z + w.w + w4);
    return vec4<f32>(o, 1.0);
}

@fragment
fn fs_bup(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let sz = vec2<f32>(textureDimensions(bsrc, 0));
    // The target is twice the source's size.
    let uv = pos.xy * 0.5 / sz;
    let t = bu.p.y / sz;
    var o = textureSampleLevel(bsrc, bsamp, uv, 0.0).rgb * 4.0;
    o = o + (textureSampleLevel(bsrc, bsamp, uv + vec2<f32>(-t.x, 0.0), 0.0).rgb + textureSampleLevel(bsrc, bsamp, uv + vec2<f32>(t.x, 0.0), 0.0).rgb
        + textureSampleLevel(bsrc, bsamp, uv + vec2<f32>(0.0, -t.y), 0.0).rgb + textureSampleLevel(bsrc, bsamp, uv + vec2<f32>(0.0, t.y), 0.0).rgb) * 2.0;
    o = o + textureSampleLevel(bsrc, bsamp, uv - t, 0.0).rgb + textureSampleLevel(bsrc, bsamp, uv + t, 0.0).rgb
        + textureSampleLevel(bsrc, bsamp, uv + vec2<f32>(t.x, -t.y), 0.0).rgb + textureSampleLevel(bsrc, bsamp, uv + vec2<f32>(-t.x, t.y), 0.0).rgb;
    return vec4<f32>(o / 16.0, 1.0);
}

// ---- per-probe mean of a cascade level ----
struct MeanU {
    n: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
};
@group(0) @binding(0) var<uniform> mu: MeanU;
@group(0) @binding(1) var csrc: texture_2d<f32>;
@group(0) @binding(2) var mout: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8)
fn cs_mean(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = textureDimensions(mout);
    let b = 2u << mu.n;
    let np = textureDimensions(csrc) / b;
    if (gid.x >= np.x || gid.y >= np.y || gid.x >= dims.x || gid.y >= dims.y) {
        return;
    }
    var acc = vec3<f32>(0.0);
    for (var y = 0u; y < b; y = y + 1u) {
        for (var x = 0u; x < b; x = x + 1u) {
            acc = acc + textureLoad(csrc, vec2<i32>(gid.xy * b + vec2<u32>(x, y)), 0).rgb;
        }
    }
    textureStore(mout, gid.xy, vec4<f32>(acc / f32(b * b), 1.0));
}

// ---- final encode ----
struct EncU {
    size: vec4<f32>,     // W, H, dither 8-bit (LSB), dither 10-bit (LSB)
    misc: vec4<f32>,     // bloom mix, bloom scale (1 / levels), unused
};
@group(0) @binding(0) var<uniform> eu: EncU;
@group(0) @binding(1) var img: texture_2d<f32>;
@group(0) @binding(2) var overlay: texture_2d<f32>;
@group(0) @binding(3) var noise: texture_2d<f32>;
@group(0) @binding(4) var<storage, read_write> y8: array<u32>;
@group(0) @binding(5) var<storage, read_write> y10: array<u32>;
@group(0) @binding(6) var<storage, read_write> rgba: array<u32>;
@group(0) @binding(7) var bloom: texture_2d<f32>;
@group(0) @binding(8) var esamp: sampler;

fn srgb(c: vec3<f32>) -> vec3<f32> {
    let x = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    let lo = x * 12.92;
    let hi = 1.055 * pow(x, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, x <= vec3<f32>(0.0031308));
}

fn final_px(p: vec2<i32>) -> vec3<f32> {
    let b = textureSampleLevel(bloom, esamp, (vec2<f32>(p) + 0.5) / eu.size.xy, 0.0).rgb;
    let c = mix(textureLoad(img, p, 0).rgb, b * eu.misc.y, eu.misc.x);
    let o = textureLoad(overlay, p, 0);
    return srgb(c * (1.0 - o.a) + o.rgb);
}

// Triangular (TPDF) dither in (−1, 1) from two channels of a 64×64 blue-noise tile; `ch` picks the pair.
fn tpdf(p: vec2<i32>, ch: i32) -> f32 {
    let n = textureLoad(noise, vec2<i32>(p.x & 63, p.y & 63), 0);
    let o = textureLoad(noise, vec2<i32>((p.x + 32) & 63, (p.y + 17) & 63), 0);
    var a = n.x;
    var b = o.y;
    if (ch == 1) {
        a = n.y;
        b = o.z;
    } else if (ch == 2) {
        a = n.z;
        b = o.x;
    }
    return a + b - 1.0;
}

@compute @workgroup_size(8, 8)
fn cs_yuv(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = u32(eu.size.x);
    let h = u32(eu.size.y);
    let x0 = gid.x * 8u;
    let yy = gid.y * 2u;
    if (x0 >= w || yy >= h) {
        return;
    }
    var cb = array<f32, 4>(0.0, 0.0, 0.0, 0.0);
    var cr = array<f32, 4>(0.0, 0.0, 0.0, 0.0);
    for (var r = 0u; r < 2u; r = r + 1u) {
        let y = yy + r;
        var w8 = array<u32, 2>(0u, 0u);
        var w10 = array<u32, 4>(0u, 0u, 0u, 0u);
        for (var i = 0u; i < 8u; i = i + 1u) {
            let p = vec2<i32>(i32(x0 + i), i32(y));
            let c = final_px(p);
            let yl = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
            cb[i / 2u] = cb[i / 2u] + (c.b - yl) / 1.8556 * 0.25;
            cr[i / 2u] = cr[i / 2u] + (c.r - yl) / 1.5748 * 0.25;
            let d = tpdf(p, 0);
            let v8 = u32(clamp(round(16.0 + 219.0 * yl + d * eu.size.z), 0.0, 255.0));
            let v10 = u32(clamp(round(64.0 + 876.0 * yl + d * eu.size.w), 0.0, 1023.0));
            w8[i / 4u] = w8[i / 4u] | (v8 << (8u * (i % 4u)));
            w10[i / 2u] = w10[i / 2u] | (v10 << (16u * (i % 2u)));
        }
        let o8 = (y * w + x0) / 4u;
        y8[o8] = w8[0];
        y8[o8 + 1u] = w8[1];
        let o10 = (y * w + x0) / 2u;
        for (var k = 0u; k < 4u; k = k + 1u) {
            y10[o10 + k] = w10[k];
        }
    }
    // Chroma, sited at the centre of each 2×2 block.
    let cw = w / 2u;
    let crow = yy / 2u;
    let cx0 = x0 / 2u;
    var u8w = 0u;
    var v8w = 0u;
    var u10 = array<u32, 2>(0u, 0u);
    var v10 = array<u32, 2>(0u, 0u);
    for (var k = 0u; k < 4u; k = k + 1u) {
        let p = vec2<i32>(i32(cx0 + k), i32(crow)) * 2;
        let du = tpdf(p, 1);
        let dv = tpdf(p, 2);
        let a8 = u32(clamp(round(128.0 + 224.0 * cb[k] + du * eu.size.z), 0.0, 255.0));
        let b8 = u32(clamp(round(128.0 + 224.0 * cr[k] + dv * eu.size.z), 0.0, 255.0));
        u8w = u8w | (a8 << (8u * k));
        v8w = v8w | (b8 << (8u * k));
        let a10 = u32(clamp(round(512.0 + 896.0 * cb[k] + du * eu.size.w), 0.0, 1023.0));
        let b10 = u32(clamp(round(512.0 + 896.0 * cr[k] + dv * eu.size.w), 0.0, 1023.0));
        u10[k / 2u] = u10[k / 2u] | (a10 << (16u * (k % 2u)));
        v10[k / 2u] = v10[k / 2u] | (b10 << (16u * (k % 2u)));
    }
    let ysz = w * h;
    let csz = cw * (h / 2u);
    y8[(ysz + crow * cw + cx0) / 4u] = u8w;
    y8[(ysz + csz + crow * cw + cx0) / 4u] = v8w;
    let b10u = (ysz + crow * cw + cx0) / 2u;
    let b10v = (ysz + csz + crow * cw + cx0) / 2u;
    y10[b10u] = u10[0];
    y10[b10u + 1u] = u10[1];
    y10[b10v] = v10[0];
    y10[b10v + 1u] = v10[1];
}

@compute @workgroup_size(8, 8)
fn cs_rgba(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = u32(eu.size.x);
    let h = u32(eu.size.y);
    if (gid.x >= w || gid.y >= h) {
        return;
    }
    let p = vec2<i32>(gid.xy);
    let c = final_px(p);
    var o = 0u;
    for (var k = 0; k < 3; k = k + 1) {
        let v = u32(clamp(round(c[k] * 255.0 + tpdf(p, k) * eu.size.z), 0.0, 255.0));
        o = o | (v << (8u * u32(k)));
    }
    rgba[gid.y * w + gid.x] = o | (255u << 24u);
}
