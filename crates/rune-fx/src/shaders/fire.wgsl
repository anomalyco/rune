// Fire / plasma. variant: 0 flames off the top edge (also Heat), 1 plasma hugging the border, 3 embers only,
// 4 simulated flames + smoke (`source` = the sim state: velocity, temperature, smoke; aux = its rect in pt).
// Embers are a CPU particle table (cpu.rs::embers): xf = (first, count).
// c0 ramp low (deep red / plasma envelope), c1 ramp mid, c2 ramp hot
// p0 (height pt, scale pt, speed (scale units/s), intensity)
// p1 (ember count, ember rise pt/s, ember size pt, ember life s)
// p2 (warp, sides (plasma band thickness share), base glow, occlusion)
// p3 (swirl, _, _, _)

// Temperature to colour: smoke-dark -> deep red -> orange -> yellow -> white-yellow core.
fn fire_ramp(T: f32, c0: vec3f, c1: vec3f, c2: vec3f) -> vec3f {
    var col = mix(vec3f(0.012, 0.009, 0.008), c0, smoothstep(0.04, 0.22, T));
    col = mix(col, c1, smoothstep(0.2, 0.5, T));
    col = mix(col, c2, smoothstep(0.45, 0.78, T));
    col = mix(col, vec3f(1.0, 0.92, 0.74), smoothstep(0.8, 1.08, T));
    return col;
}
fn fbm4(p0: vec2f, seed: u32) -> f32 {
    var p = p0; var a = 0.5; var s = 0.0;
    for (var o = 0u; o < 4u; o++) { s += a * gnoise(p, seed + o * 131u); p = ROT * p * 2.03; a *= 0.5; }
    return s / 0.9375;
}

fn flames(c: Ctx, i: Inst) -> vec4f {
    let H = i.p0.x;
    let sc = max(i.p0.y, 1.0);
    let tr = c.t * i.p0.z;
    let nrm = sd_grad(c);
    let up = clamp(-nrm.y, 0.0, 1.0);
    // Top edge only: fades over the rounded corners and never past the sides.
    let span = smoothstep(c.half.x + 2.0, c.half.x - c.r * 0.9 - 2.0, abs(c.q.x));
    let hl = H * smoothstep(0.15, 0.95, up) * span;
    if (hl < 0.5) { return vec4f(0.0); }
    let y = c.d + 1.5; // height above the edge (the base tucks under the node)
    let yn = y / hl;
    if (yn > 1.6 || yn < -0.2) { return vec4f(0.0); }
    let x = c.p.x / sc;
    // Flicker: tongue heights breathe slowly and jitter at ~8 Hz, per column.
    let fl = 1.0 + 0.2 * gnoise(vec2f(x * 0.45, c.t * 1.3), c.seed + 7u) + 0.07 * gnoise(vec2f(x * 1.1 + 9.0, c.t * 8.0), c.seed + 8u);
    let yh = yn / max(fl, 0.4);
    // Rising, domain-warped turbulence; the warp grows with height, so tongues curl and tear.
    var sw = vec2f(0.0);
    if (i.p3.x > 0.0) { sw = i.p3.x * vec2f(sin(y / sc * 0.7 + tr * 0.4), 0.0); }
    let wq = vec2f(x * 0.7, y / sc * 0.45 - tr * 1.05);
    let w = vec2f(gnoise(wq, c.seed), gnoise(wq + vec2f(3.7, 1.9), c.seed + 1u)) * i.p2.x * (0.2 + 0.8 * yh);
    let n = fbm4(vec2f(x * 1.05, y / sc * 0.5 - tr * 1.6) + w + sw, c.seed + 2u);
    // Density: full at the base, falling with height; the noise carves it into tongues that narrow
    // as only the strongest plumes survive higher up.
    let dens = (1.0 - yh) * 1.2 + n * 0.8 - 0.08;
    let body = smoothstep(0.0, 0.06, dens) * smoothstep(-0.2, 0.05, yn);
    // Hottest low down and inside a plume; the tips cool to red.
    // Only the dense heart of a plume near the base reaches the white-yellow core.
    let T = clamp(dens * (0.95 - 0.5 * yh), 0.0, 1.1);
    let col = fire_ramp(T, i.c0.rgb, i.c1.rgb, i.c2.rgb);
    let e = body * (0.3 + 0.75 * T * T);
    var emis = col * e;
    // Smoke: a faint dark veil just past the tips (only visible over lighter content).
    let smoke = smoothstep(-0.32, -0.02, dens) * (1.0 - body) * smoothstep(0.35, 0.9, yh) * 0.12;
    // A little of the fire's light on the edge it sits on.
    let bg = i.p2.z * 0.22 * exp(-max(c.d, 0.0) / max(hl * 0.12, 1.0)) * smoothstep(-1.0, 0.5, c.d) * span;
    emis += i.c1.rgb * bg;
    let occl = select(1.0, i.p2.w, effective_blend_mode(i) == 0u);
    return vec4f(emis, body * occl + smoke);
}

// Blackbody colour (linear) for a flame temperature in kelvin (Tanner Helland's fit, 1000-4000 K).
fn blackbody(k: f32) -> vec3f {
    let t = k / 100.0;
    let g = clamp((99.4708 * log(t) - 161.1196) / 255.0, 0.0, 1.0);
    let b = select(clamp((138.5177 * log(max(t - 10.0, 1.0)) - 305.0448) / 255.0, 0.0, 1.0), 0.0, t <= 19.0);
    return to_lin(vec3f(1.0, g, b));
}

// Light a flame of temperature T (0..~1.2) emits: colour from 1000 K (dim red tips) to ~2700 K
// (near-white core), brightness rising steeply with temperature, then a per-channel shoulder so
// the hottest parts go pale yellow-white the way a camera sees them instead of clipping flat.
fn flame_light(T: f32, gain: f32) -> vec3f {
    let x = max(T, 0.0);
    let e = gain * x * x * x * smoothstep(0.08, 0.3, x);
    return vec3f(1.0) - exp(-blackbody(1000.0 + 1700.0 * min(x, 1.3)) * e);
}

// Cubic B-spline sample of the sim state from 4 bilinear taps: smooth isolines, no grid diamonds.
fn sim_at(uv: vec2f) -> vec4f {
    let dim = vec2f(textureDimensions(source, 0));
    let g = uv * dim - 0.5;
    let i = floor(g);
    let f = g - i;
    let w0 = (1.0 - f) * (1.0 - f) * (1.0 - f) / 6.0;
    let w1 = (4.0 - 6.0 * f * f + 3.0 * f * f * f) / 6.0;
    let w3 = f * f * f / 6.0;
    let w2 = 1.0 - w0 - w1 - w3;
    let s0 = w0 + w1;
    let s1 = w2 + w3;
    let o0 = (i - 0.5 + w1 / s0) / dim;
    let o1 = (i + 1.5 + w3 / s1) / dim;
    let a = textureSampleLevel(source, samp, vec2f(o0.x, o0.y), 0.0);
    let b = textureSampleLevel(source, samp, vec2f(o1.x, o0.y), 0.0);
    let c = textureSampleLevel(source, samp, vec2f(o0.x, o1.y), 0.0);
    let d = textureSampleLevel(source, samp, vec2f(o1.x, o1.y), 0.0);
    return mix(mix(a, b, s1.x), mix(c, d, s1.x), s1.y);
}

// Simulated fire: flames are additive light from the temperature field; smoke is a separate
// density that partly covers what is behind it and is lit warm from below by the flames.
fn sim_flames(c: Ctx, i: Inst) -> vec4f {
    let ext = i.aux.zw;
    let uv = (c.p - i.aux.xy) / ext;
    if (any(uv < vec2f(0.0)) || any(uv > vec2f(1.0))) { return vec4f(0.0); }
    let s = sim_at(uv);
    let T = s.z;
    // Fade out well before the drawn quad's top and sides (p3.yz = its size).
    let m = min(min(c.p.x - i.aux.x, i.aux.x + i.p3.y - c.p.x), c.p.y - i.aux.y);
    let win = smoothstep(0.0, 18.0, m);
    var rgb = flame_light(T, 6.0) * win;
    // Smoke, lit by the flame below it: march a few taps down toward the fire.
    let sm = s.w * win;
    var a = 0.0;
    if (sm > 0.002) {
        a = (1.0 - exp(-sm * 1.2)) * i.p2.w * 1.6;
        var lit = 0.0;
        for (var k = 1; k < 5; k++) {
            let tk = sim_at(uv + vec2f(0.0, f32(k) * 7.0 / ext.y)).z;
            lit += tk * tk / f32(k);
        }
        let warm = vec3f(1.0, 0.45, 0.18) * min(lit, 1.0) * 0.3;
        rgb += a * (vec3f(0.11, 0.11, 0.115) + warm);
    }
    // Flames themselves cover a little of what is behind them (occlusion).
    let vis = smoothstep(0.12, 0.35, T) * win;
    return vec4f(rgb, min(a + vis * i.p2.w * 0.5, 1.0));
}

// Plasma: contained energy hugging the shape. A thin band centred just outside the edge, with
// ridged filaments flowing along the perimeter, a tight outer falloff and slow breathing.
fn plasma(c: Ctx, i: Inst) -> vec4f {
    let sc = max(i.p0.y, 1.0);
    let thick = max(i.p0.x * i.p2.y * 0.3, 2.5);
    let pr = rrect_perim(c.q, c.half, c.r);
    let u = pr.x / pr.y;
    let d = c.d - thick * 0.45;
    if (abs(d) > thick * 4.0) { return vec4f(0.0); }
    let P = max(round(pr.y / (sc * 2.5)), 3.0);
    let flow = c.t * i.p0.z * (0.35 + 0.25 * i.p3.x);
    let warp = 0.6 * pgnoise(vec2f(u * P - flow * 0.6, c.t * 0.35), i32(P), c.seed + 3u);
    // Ridges stretched along the perimeter: filaments run with the flow instead of netting.
    let r1 = 1.0 - abs(pgnoise(vec2f(u * P - flow * 1.4, d / sc * 3.2 + warp), i32(P), c.seed));
    let r2 = 1.0 - abs(pgnoise(vec2f(u * P * 2.0 + flow * 1.1, d / sc * 4.5 - warp - c.t * 0.6), i32(P * 2.0), c.seed + 1u));
    let fil = pow(r1, 10.0) + 0.45 * pow(r2, 12.0);
    let breathe = 0.78 + 0.22 * pgnoise(vec2f(u * 3.0 - c.t * 0.25, c.t * 0.2), 3, c.seed + 5u);
    let env = exp(-(d * d) / (thick * thick));
    let halo = exp(-max(d, 0.0) / (thick * 0.9)) * smoothstep(-thick, 0.0, d);
    let E = env * (0.18 + 1.2 * fil) * breathe;
    let rgb = i.c0.rgb * halo * 0.28 + i.c1.rgb * E * 0.55 + i.c2.rgb * fil * env * breathe * 0.9
            + vec3f(1.0) * pow(fil, 3.0) * env * breathe * 0.35;
    return vec4f(rgb, (E * 0.5 + halo * 0.1) * i.p2.w * 2.0);
}

fn shade(i: Inst, px: vec2f) -> vec4f {
    let c = ctx(i, px);
    let inten = i.p0.w;
    var o = vec4f(0.0);
    if (c.variant == 0) {
        o = flames(c, i);
    } else if (c.variant == 1) {
        o = plasma(c, i);
    } else if (c.variant == 4) {
        o = sim_flames(c, i);
    }
    var emis = o.rgb;
    var alpha = o.a;

    // Embers: short shutter streaks with a hot core, cooling white -> orange -> red as they rise.
    let ef = u32(i.xf.x);
    let en = u32(i.xf.y);
    for (var k = 0u; k < en; k++) {
        let a = data[ef + k * 2u];
        let b = data[ef + k * 2u + 1u];
        let sd = segdist(c.p, a.xy - a.zw, a.xy);
        let T = b.y;
        let core = gauss(sd.x, b.x);
        let halo = gauss(sd.x, b.x * 3.0) * 0.12;
        emis += flame_light(0.45 + T * 0.75, 6.0) * (core * 1.4 + halo) * T;
        alpha += core * T * 0.5;
    }
    return finish(vec4f(emis * inten, min(alpha, 1.0) * inten), px);
}
