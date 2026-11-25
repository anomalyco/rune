// Full-resolution resolve: the UI's colour as albedo under ambient + every light (visibility from the shadow
// pass, bevel normals from the surface buffer, GGX highlights widened by the source size), contact AO,
// one-bounce / emitter GI, god-ray air, visible bulbs, then text with a contrast floor, a soft shoulder, sRGB
// encode and dither.

@group(0) @binding(0) var<uniform> F: Frame;
@group(0) @binding(1) var<storage, read> prims: array<Prim>;
@group(0) @binding(2) var<storage, read> lights: array<Light>;
@group(0) @binding(3) var ui_base: texture_2d<f32>;
@group(0) @binding(4) var ui_text: texture_2d<f32>;
@group(0) @binding(5) var gb0: texture_2d<f32>;
@group(0) @binding(6) var gb1: texture_2d<u32>;
@group(0) @binding(7) var vis: texture_2d_array<f32>;
@group(0) @binding(8) var air: texture_2d_array<f32>;
@group(0) @binding(9) var flu: texture_2d<f32>;
@group(0) @binding(10) var field: texture_2d<f32>;
@group(0) @binding(11) var samp: sampler;

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

// Joint-bilateral upsample of the half-resolution visibility: taps on a different surface height
// (card top vs. the floor beside it) are rejected so shadows never bleed across an edge.
fn vis_at(tex: texture_2d_array<f32>, px: vec2<f32>, zr: f32, layer: i32) -> vec4<f32> {
    let fsz = vec2<i32>(F.sf.zw);
    let f = px * F.gs.z - 0.5;
    let b = vec2<i32>(floor(f));
    let w = fract(f);
    var acc = vec4<f32>(0.0);
    var wsum = 0.0;
    var nearest = vec4<f32>(1.0);
    var best = 1e9;
    for (var k = 0; k < 4; k = k + 1) {
        let o = vec2<i32>(k & 1, k >> 1u);
        let q = clamp(b + o, vec2<i32>(0), fsz - vec2<i32>(1));
        let v = textureLoad(tex, q, layer, 0);
        let qp = clamp(vec2<i32>((vec2<f32>(q) + 0.5) / F.gs.z), vec2<i32>(0), vec2<i32>(F.sf.xy) - vec2<i32>(1));
        let dz = abs(textureLoad(gb0, qp, 0).w - zr);
        let bw = select(1.0 - w.x, w.x, o.x == 1) * select(1.0 - w.y, w.y, o.y == 1);
        let ww = bw / (1.0 + dz * dz * 4.0);
        acc = acc + v * ww;
        wsum = wsum + ww;
        if (dz < best) {
            best = dz;
            nearest = v;
        }
    }
    return select(nearest, acc / wsum, wsum > 1e-3);
}

fn ggx(nh: f32, a: f32) -> f32 {
    let a2 = a * a;
    let d = nh * nh * (a2 - 1.0) + 1.0;
    return a2 / (PI * d * d);
}

fn shoulder(x: vec3<f32>) -> vec3<f32> {
    let k = 0.8;
    let over = max(x - vec3<f32>(k), vec3<f32>(0.0));
    return min(x, vec3<f32>(k)) + (1.0 - k) * (vec3<f32>(1.0) - exp(-over / (1.0 - k)));
}

fn to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

// Interleaved gradient noise (Jimenez), a stable per-pixel pattern: dither without crawling grain.
fn ign(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let px = pos.xy;
    let ip = vec2<i32>(px);
    let base = textureLoad(ui_base, ip, 0);
    let text = textureLoad(ui_text, ip, 0);
    let g = textureLoad(gb0, ip, 0);
    let id = textureLoad(gb1, ip, 0).r;
    let sd = g.x;
    let grad = g.yz;
    let zr = g.w;

    var n = vec3<f32>(0.0, 0.0, 1.0);
    var gloss = F.bg.x;
    var rough = F.bg.y;
    var fl = 0u;
    var rim = 0.0;
    var emissive = false;
    var unlit = false;
    if (id > 0u) {
        let pr = prims[id - 1u];
        fl = flags_of(pr);
        gloss = pr.mat.x;
        rough = pr.mat.y;
        emissive = (fl & F_EMIT) != 0u;
        unlit = (fl & F_UNLIT) != 0u;
        if ((fl & F_RAISED) != 0u) {
            let bw = max(pr.z.z, 0.5);
            let x = clamp(-sd / bw, 0.0, 1.0);
            let slope = pr.z.w / bw * 3.0 * (1.0 - x) * (1.0 - x);
            n = normalize(vec3<f32>(grad * slope, 1.0));
        }
        // Hairline catch on the rim, about a device pixel and a half wide.
        rim = F.bg.w * (1.0 - smoothstep(0.0, 2.0, -sd)) * select(0.0, 1.0, (fl & F_OCC) != 0u);
    }

    let P = vec3<f32>(px, zr + 0.35);
    var irr = F.ambient.rgb;
    // Contact AO on surfaces below the occluders around them (the floor next to a raised card).
    if ((F.counts.w & 8u) != 0u) {
        let f = textureSampleLevel(field, samp, px * F.gs.z / F.sf.zw, 0.0);
        let below = max(f.z - zr, 0.0);
        if (below > 0.0 && f.x > 0.0) {
            let reach = below * 3.0 + 6.0;
            let occ = 1.0 - smoothstep(0.0, reach, f.x);
            irr = irr * (1.0 - F.bg.z * occ * occ);
        }
    }

    var diffuse = vec3<f32>(0.0);
    var spec = vec3<f32>(0.0);
    var airc = vec3<f32>(0.0);
    let nl = min(F.counts.y, 8u);
    let v0 = vis_at(vis, px, zr, 0);
    var v1 = vec4<f32>(1.0);
    if (nl > 4u) {
        v1 = vis_at(vis, px, zr, 1);
    }
    let shafts = (F.counts.w & 2u) != 0u;
    var a0 = vec4<f32>(1.0);
    var a1 = vec4<f32>(1.0);
    if (shafts) {
        a0 = vis_at(air, px, zr, 0);
        if (nl > 4u) {
            a1 = vis_at(air, px, zr, 1);
        }
    }
    for (var li = 0u; li < nl; li = li + 1u) {
        let L = lights[li];
        let lt = light_term(L, P);
        let l = lt.xyz;
        var v = v0[li % 4u];
        if (li >= 4u) {
            v = v1[li % 4u];
        }
        let ndl = max(dot(n, l), 0.0);
        let e = L.color.rgb * lt.w * v;
        // A shaft light stands behind the UI: it lights nothing from the front, only rims and the air.
        let front = select(1.0, 0.0, L.extra.x > 0.0);
        diffuse = diffuse + e * ndl * front;
        // GGX toward a viewer straight above; the source radius widens the lobe (Karis), energy-kept.
        let h = normalize(l + vec3<f32>(0.0, 0.0, 1.0));
        let dist = length(L.pos.xyz - P);
        let a = clamp(rough * rough + L.pos.w / (2.0 * dist), 0.02, 1.0);
        let energy = (rough * rough) / a;
        let fres = 0.04 + 0.96 * pow(1.0 - max(dot(l, h), 0.0), 5.0);
        spec = spec + e * ndl * ggx(max(dot(n, h), 0.0), a) * energy * fres * gloss * L.extra.z * front;
        // Rim: edges whose outward normal faces the light catch a bright hairline.
        let lxy = L.pos.xy - px;
        let face = max(dot(grad, lxy / max(length(lxy), 1e-3)), 0.0);
        spec = spec + e * rim * face * face * max(l.z, 0.15);
        let d2 = length(lxy);
        if (shafts && L.extra.x > 0.0) {
            var av = a0[li % 4u];
            if (li >= 4u) {
                av = a1[li % 4u];
            }
            // Over occluders the cards in front dim the shafts.
            let over = select(1.0, F.misc.w, zr > 0.5);
            let fall = 1.5 * REF / (d2 + 1.5 * REF);
            airc = airc + L.color.rgb * L.extra.x * av * over * fall * fall;
        }
        if (L.extra.y > 0.0) {
            let r = max(L.pos.w, 1.0);
            let q = r * r / (d2 * d2 + r * r);
            airc = airc + L.color.rgb * L.extra.y * (q * sqrt(q) + 0.05 * q);
        }
    }

    var gi_irr = vec3<f32>(0.0);
    if ((F.counts.w & 1u) != 0u) {
        let flv = textureSampleLevel(flu, samp, px * F.gs.w / F.gs.xy, 0.0).rgb;
        gi_irr = flv * F.gi.x;
        airc = airc + flv * F.gi.y;
    }

    let lightf = irr + diffuse + gi_irr;
    // A dark UI colour is nearly black as a material; a small albedo floor lets light pools and shadows read
    // on it while ambient keeps the designed look.
    var col = base.rgb * irr + (base.rgb + vec3<f32>(F.ext.x)) * (diffuse + gi_irr) + spec;
    if (emissive) {
        col = base.rgb * max(lightf, vec3<f32>(1.0)) + spec * 0.25;
    }
    if (unlit) {
        col = base.rgb;
        spec = vec3<f32>(0.0);
    }

    // Text: lit like the surface under it, but clamped so shadows never crush it and highlights never wash it.
    let tf = select(mix(vec3<f32>(1.0), clamp(lightf, vec3<f32>(F.text.x), vec3<f32>(F.text.y)), F.text.z), vec3<f32>(1.0), unlit);
    col = col * (1.0 - text.a) + text.rgb * tf + spec * text.a * F.text.w;
    col = col + airc * (1.0 - 0.6 * text.a);

    var o = shoulder(col * F.ambient.w);
    if ((F.counts.w & 4u) != 0u) {
        o = to_srgb(o);
        let dn = ign(px + vec2<f32>(F.misc.z * 5.588238, 0.0)) + ign(px.yx + vec2<f32>(17.0, F.misc.z)) - 1.0;
        o = o + vec3<f32>(dn / 255.0);
    }
    return vec4<f32>(o, 1.0);
}
