// Glow / bloom. variant: 0 soft, 1 neon (hot core rim), 2 aura (spectrum around the border), 3 mask (texture).
// c0 tint, c1 hot core, c2 aura third colour
// p0 (radius, intensity, inner, occlusion)
// p1 (breathe_amp, breathe_period, heat, hot)
// p2 (aura_speed, rim_width, reach_mul, _)

fn aura_palette(u: f32, a: vec3f, b: vec3f, c: vec3f) -> vec3f {
    // Three colours around a loop, smooth (cosine) blend.
    let x = fract(u) * 3.0;
    let w0 = smoothstep(1.0, 0.0, min(x, 3.0 - x));
    let w1 = smoothstep(1.0, 0.0, abs(x - 1.0));
    let w2 = smoothstep(1.0, 0.0, abs(x - 2.0));
    return (a * w0 + b * w1 + c * w2) / max(w0 + w1 + w2, 1e-4);
}

fn mask_tent(uv: vec2f, lod: f32) -> f32 {
    // Bilinear taps at ±0.5 texel of the chosen level: a tent upsample, no blocky mip seams.
    let dims = vec2f(textureDimensions(source, 0));
    let texel = exp2(lod) / dims;
    let o = texel * 0.5;
    var s = textureSampleLevel(source, samp, uv + vec2f(o.x, o.y), lod).a;
    s += textureSampleLevel(source, samp, uv + vec2f(-o.x, o.y), lod).a;
    s += textureSampleLevel(source, samp, uv + vec2f(o.x, -o.y), lod).a;
    s += textureSampleLevel(source, samp, uv + vec2f(-o.x, -o.y), lod).a;
    return s * 0.25;
}

fn shade(i: Inst, px: vec2f) -> vec4f {
    let c = ctx(i, px);
    var radius = i.p0.x;
    var intensity = i.p0.y;
    let inner = i.p0.z;
    let occl = select(1.0, i.p0.w, effective_blend_mode(i) == 0u);
    let heat = i.p1.z;
    let hot = i.p1.w;

    // Breathing: phase comes from the caller's continuous phase clock (t), so speed changes never jump.
    var b = 0.0;
    if (!c.reduced && i.p1.y > 0.0) {
        b = 0.5 - 0.5 * cos(TAU * c.t / i.p1.y);
        b = b * b * (3.0 - 2.0 * b);
    }
    let amp = i.p1.x;
    intensity *= (1.0 - amp) + amp * b;
    radius *= 1.0 + 0.22 * amp * b;
    // Cooling: shrink and deepen as the light loses heat.
    radius *= mix(0.72, 1.0, heat);
    intensity *= pow(heat, 1.6);
    let tint = cooled(i.c0.rgb, heat);
    let reach = radius * i.p2.z;

    var light = vec3f(0.0);
    var alpha = 0.0;
    let edge = smoothstep(-c.aa, c.aa, c.d); // 0 inside, 1 outside

    if (c.variant == 3) {
        // Mask glow from the mip pyramid in `source` (covers aux2 rect).
        let uv = (c.p - i.aux2.xy) / i.aux2.zw;
        let rpx = radius * c.scale;
        let base = log2(max(rpx, 1.0)) - 0.6;
        let m = 0.45 * mask_tent(uv, max(base - 1.0, 0.0)) + 0.35 * mask_tent(uv, max(base, 0.0))
              + 0.2 * mask_tent(uv, base + 1.3);
        let core = textureSampleLevel(source, samp, uv, 0.0).a;
        let g = knee(m * 1.25 * intensity) * 1.1;
        let hotc = core * hot * smoothstep(0.55, 1.0, heat);
        light = tint * g + i.c1.rgb * hotc * intensity * 0.6;
        alpha = min(g * occl, 1.0);
        return finish(vec4f(light, alpha), px);
    }

    if (c.variant == 0) {
        let out_g = bloom(c.d, radius, reach);
        let in_g = inner * 0.7 * exp(min(c.d, 0.0) / (0.4 * radius));
        let gl = mix(in_g, out_g, edge);
        let corew = gauss(max(c.d, 0.0), 0.3 * radius) * hot * smoothstep(0.6, 1.0, heat);
        let col = mix(tint, i.c1.rgb, corew * 0.8);
        let e = knee(gl * intensity * 1.1);
        light = col * e;
        alpha = e * occl;
    } else if (c.variant == 1) {
        // Neon: a white-hot hairline core on the border, coloured bloom both sides.
        let w = max(i.p2.y, c.aa * 0.75);
        let core = gauss(c.d, w) ;
        let out_g = bloom(c.d, radius, reach) * edge;
        let in_g = inner * bloom(-c.d, radius * 0.6, reach) * (1.0 - edge);
        let corec = mix(tint, i.c1.rgb, 0.7 * hot * smoothstep(0.5, 1.0, heat));
        let e = knee((out_g + in_g) * intensity);
        light = corec * core * 1.4 * intensity + tint * e * 0.9;
        alpha = core * 0.6 * intensity + e * occl;
    } else {
        // Aura: spectrum flowing around the perimeter, inner rim light + outer bloom + hairline.
        let pr = rrect_perim(c.q, c.half, c.r);
        var u = pr.x / pr.y;
        var flow = 0.0;
        if (!c.reduced) { flow = c.t * i.p2.x; }
        let col = aura_palette(u - flow, tint, i.c1.rgb, i.c2.rgb);
        // Slow brightness variation along the loop (periodic noise), never below 70%.
        let wob = 0.85 + 0.15 * pgnoise(vec2f(u * 6.0, flow * 1.7 + 3.0), 6, c.seed);
        let out_g = bloom(c.d, radius, reach) * edge;
        let in_g = inner * exp(min(c.d, 0.0) / (0.55 * radius)) * (1.0 - edge);
        let hair = gauss(c.d + 0.6, max(i.p2.y, c.aa)) * 0.9;
        let lit = knee((out_g + in_g) * wob * intensity * 1.1);
        light = col * lit + mix(col, vec3f(1.0), 0.6) * hair * intensity * hot;
        alpha = lit * occl;
    }
    return finish(vec4f(light, alpha), px);
}
