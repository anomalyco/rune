// Distortion of the backdrop (needs `backdrop` = target content right before this layer).
// variant: 0 heat haze, 1 shockwave, 2 liquid-glass lens, 3 CRT, 4 chromatic split.
//
// heat:   p0 (height pt, scale pt, speed, amount pt)  p1 (full rect 0/1, _, _, _)
// shock:  p0 (reach pt, width pt, amount pt, chroma)  p1 (t0, duration, _, _)  aux: origin
// lens:   p0 (bevel pt, depth pt, dispersion, magnification)  p1 (presence, frost, shadow, rim)
//         p2 (light angle rad, tint, union k pt, radius B)  p3 rect B (w = 0: none)  shape.w morph A->B
//         c0 tint
// crt:    p0 (bulge, scanline period pt, scanline depth, chroma pt)  p1 (vignette, glow, roll, feather)
// split:  p0 (offset x pt, offset y pt, glitch, feather)

// Frosted material, after Apple's "regular" glass: a saturation boost puts back the colour that
// blur and scrim take out, and a compressive transfer lifts dark backdrops while light ones
// barely move (macOS 26 Control Center measures sRGB L' = 0.58 L + 0.134; iOS keeps more of
// the colour, so this sits a little above that). k = 0..1.
fn frosted(c: vec3f, k: f32) -> vec3f {
    let l = lum(c);
    let sat = max(mix(vec3f(l), c, 1.0 + 0.9 * k), vec3f(0.0));
    let e = to_srgb(sat);
    return to_lin(e * (1.0 - 0.32 * k) + vec3f(0.11 * k));
}

fn lens_sd(c: Ctx, i: Inst, q: vec2f) -> f32 {
    let da = sd_rrect(q, c.half, c.r);
    if (i.p3.z <= 0.0) { return da; }
    let cb = i.p3.xy + i.p3.zw * 0.5 - c.c;
    let db = sd_rrect(q - cb, i.p3.zw * 0.5, i.p2.w);
    return mix(smin(da, db, i.p2.z), db, i.shape.w);
}

// Thin-surface optics (psychopomp math/optics.rs): sideways travel per unit depth through a surface
// whose tilt rises from vertical at the rim to flat (superellipse bevel, power 3).
fn bend_at(t: f32) -> f32 {
    let u = clamp(1.0 - t, 0.0, 0.9999);
    let pw = 3.0;
    let slope = pow(u, pw - 1.0) * pow(max(1.0 - pow(u, pw), 1e-6), 1.0 / pw - 1.0);
    let tilt = atan(min(slope, 1e4));
    let bent = asin(sin(tilt) / 1.5);
    return tan(tilt - bent);
}

fn shade(i: Inst, px: vec2f) -> vec4f {
    let c = ctx(i, px);
    let s = c.scale;

    if (c.variant == 0) {
        let H = i.p0.x;
        var m: f32;
        if (i.p1.x > 0.5) {
            m = smoothstep(c.aa, -c.aa * 8.0, c.d);
        } else {
            let above = i.rect.y - c.p.y;
            let side = smoothstep(c.half.x + 2.0, c.half.x - c.r - 4.0, abs(c.q.x));
            // Starts just clear of the edge so the shape itself never wobbles, and is strongest a
            // little up the column where the hot air has risen off the flame tips.
            m = smoothstep(1.0, 1.0 + H * 0.3, above) * (1.0 - smoothstep(H * 0.45, H, above)) * side;
        }
        if (m <= 0.0) { return vec4f(0.0); }
        var t = c.t;
        if (c.reduced) { t = 0.0; }
        let q = c.p / max(i.p0.y, 1.0) + vec2f(0.0, t * i.p0.z);
        let o = vec2f(fbm3(q, c.seed), fbm3(q * vec2f(1.0, 0.7) + vec2f(7.3, 2.1), c.seed + 5u)) * i.p0.w;
        let col = bd(c.p + o * m, s);
        return vec4f(col.rgb, 1.0) * smoothstep(0.0, 0.02, m);
    }
    if (c.variant == 1) {
        let age = c.t - i.p1.x;
        let dur = max(i.p1.y, 0.05);
        if (age < 0.0 || age > dur) { return vec4f(0.0); }
        let u = age / dur;
        let rel = c.p - i.aux.xy;
        let r = length(rel);
        let dir = rel / max(r, 1e-3);
        let fade = (1.0 - u) * (1.0 - u);
        if (c.reduced) {
            // No travel: a soft brightening at the origin that cools.
            let gl = gauss(r, i.p0.x * 0.25) * 0.12 * fade * smoothstep(0.0, 0.1, u);
            return vec4f(bd(c.p, s).rgb * gl, 0.0);
        }
        let R = i.p0.x * (1.0 - pow(1.0 - u, 2.6));
        let w = i.p0.y * (1.0 + 1.5 * u);
        let x = (r - R) / w;
        let f = x * exp(-x * x) * 2.33;
        let off = -dir * f * i.p0.z * fade;
        let ca = i.p0.w * abs(f) * fade;
        let rr = bd(c.p + off * (1.0 + ca), s).r;
        let gg = bd(c.p + off, s).g;
        let bb = bd(c.p + off * (1.0 - ca), s).b;
        // Highlight only where there is content to catch it (no grey donut over empty space).
        let lit = max(-f, 0.0) * 0.12 * fade * lum(vec3f(rr, gg, bb));
        let m = smoothstep(0.0, 0.01, abs(f) * fade);
        return vec4f(vec3f(rr, gg, bb) + lit, 1.0) * m;
    }
    if (c.variant == 2) {
        let d = lens_sd(c, i, c.q);
        let presence = i.p1.x;
        let e = 0.5;
        let gx = lens_sd(c, i, c.q + vec2f(e, 0.0)) - lens_sd(c, i, c.q - vec2f(e, 0.0));
        let gy = lens_sd(c, i, c.q + vec2f(0.0, e)) - lens_sd(c, i, c.q - vec2f(0.0, e));
        let nrm = vec2f(gx, gy) / max(length(vec2f(gx, gy)), 1e-5);
        let coverage = clamp(0.5 - d * s, 0.0, 1.0);
        // Drop shadow + contact (outside), scaled by presence: glass materialises by lensing.
        let sd = lens_sd(c, i, c.q - vec2f(0.0, 5.0 * presence));
        let fall = exp(-pow(max(sd + 2.0, 0.0), 2.0) / (2.0 * 9.0 * 9.0));
        let contact = exp(-pow(max(d, 0.0), 2.0) / 6.0);
        let dark = min(i.p1.z * presence * (0.55 * fall + 0.45 * contact), 1.0) * (1.0 - coverage);
        if (coverage <= 0.0) {
            return vec4f(0.0, 0.0, 0.0, dark);
        }
        let depth = -d;
        let bevel = max(i.p0.x, 0.5);
        let tt = clamp(depth / bevel, 0.0, 1.0);
        let bend = i.p0.y * bend_at(tt) * presence;
        let mag = mix(1.0, i.p0.w, presence);
        let ctr = c.c;
        let inward = -nrm;
        let dsp = i.p0.z;
        let local = c.p - ctr;
        let srcg = ctr + (local + inward * bend) / mag;
        let srcr = ctr + (local + inward * bend * (1.0 - dsp)) / mag;
        let srcb = ctr + (local + inward * bend * (1.0 + dsp)) / mag;
        var col = vec3f(bd(srcr, s).r, bd(srcg, s).g, bd(srcb, s).b);
        let frost = i.p1.y;
        if (frost > 0.0 && i.xf.x > 0.5) {
            // Frosted: the dual-filter pyramid, refracted like the clear image (so the rim still
            // bends and splits), then the material transfer and a static grain against banding.
            let blr = vec3f(bdb(srcr, s).r, bdb(srcg, s).g, bdb(srcb, s).b);
            col = mix(col, blr, i.xf.y);
            let k = presence * smoothstep(0.0, 0.6, frost);
            col = frosted(col, k);
            let ip = vec2u(px);
            let n = u01(ip.x + pcg(ip.y + 7u)) - u01(ip.x * 7919u + pcg(ip.y + 31u));
            col = to_lin(max(to_srgb(col) + vec3f(n * 1.6 / 255.0 * k), vec3f(0.0)));
        } else {
            // Rim softening where the page is squeezed into a thin band (+ a small fallback frost
            // when no pyramid was built).
            let rim = bend / max(1.118 * i.p0.y, 1e-3);
            let soften = min(pow(rim, 1.5) * 0.55, 1.0);
            if (soften > 1e-3 || frost > 0.0) {
                let rad = mix(1.2, 10.0, frost);
                let j = ign(px) * TAU;
                var acc = vec3f(0.0);
                for (var k = 0; k < 8; k++) {
                    let a = j + f32(k) * 2.39996;
                    let rr = rad * sqrt((f32(k) + 0.5) / 8.0);
                    acc += bd(srcg + vec2f(cos(a), sin(a)) * rr, s).rgb;
                }
                acc /= 8.0;
                col = mix(col, acc, clamp(max(soften, frost), 0.0, 1.0));
            }
        }
        col = mix(col, col * i.c0.rgb, i.p2.y * presence);
        // Light: sky sheen, crisp specular where the rim faces the light, a fainter counter-line, hairline.
        let la = i.p2.x;
        let L = vec2f(cos(la), sin(la));
        let facing = max(dot(nrm, L), 0.0);
        let away = max(-dot(nrm, L), 0.0);
        let sky = clamp(0.5 - 0.5 * c.q.y / c.half.y, 0.0, 1.0);
        let rimk = i.p1.w;
        let fres = pow(1.0 - tt, 3.0) * 0.05 * (0.25 + 0.75 * sky);
        let line = gauss(depth - 0.75, 0.55);
        let glow = gauss(depth - 1.4, 2.4);
        let spec = line * (0.9 * pow(facing, 2.5) + 0.35 * pow(away, 3.0)) + glow * 0.1 * pow(facing, 4.0);
        let inner = gauss(tt - 0.32, 0.18) * away * away * 0.06;
        let edge = gauss(depth - 0.3, 0.45) * 0.12;
        let light = (0.004 + fres + spec + inner + edge) * presence * rimk;
        col = col * (1.0 - 0.04 * presence) + vec3f(light);
        // Coverage blends glass over the untouched backdrop + shadow.
        let base = bd(c.p, s).rgb * (1.0 - dark);
        return vec4f(mix(base, col, coverage), 1.0);
    }
    if (c.variant == 3) {
        let feather = max(i.p1.w, c.aa);
        let m = smoothstep(feather * 0.5, -feather * 0.5, c.d);
        if (m <= 0.0) { return vec4f(0.0); }
        let uv = c.q / c.half;
        let k = i.p0.x;
        let bu = uv * (1.0 + k * dot(uv, uv)) / (1.0 + k);
        let inside = smoothstep(1.0, 0.985, max(abs(bu.x), abs(bu.y)));
        let src = c.c + bu * c.half;
        let ca = i.p0.w;
        var col = vec3f(bd(src + vec2f(ca, 0.0), s).r, bd(src, s).g, bd(src - vec2f(ca, 0.0), s).b);
        // Glow: cheap 4-tap bloom of the source.
        let gw = 3.0;
        let gl = (bd(src + vec2f(gw, 0.0), s).rgb + bd(src - vec2f(gw, 0.0), s).rgb + bd(src + vec2f(0.0, gw), s).rgb + bd(src - vec2f(0.0, gw), s).rgb) * 0.25;
        col += gl * i.p1.y;
        let per = max(i.p0.y, 3.0 * c.aa);
        let sl = 0.5 + 0.5 * cos(TAU * (c.p.y - i.rect.y) / per);
        col *= 1.0 - i.p0.z * (1.0 - sl);
        // Aperture grille, very subtle.
        let col3 = u32(floor(px.x)) % 3u;
        let grille = select(select(vec3f(0.94, 0.94, 1.0), vec3f(0.94, 1.0, 0.94), col3 == 1u), vec3f(1.0, 0.94, 0.94), col3 == 0u);
        col *= grille;
        var roll = 0.0;
        if (!c.reduced) { roll = gauss(fract((c.p.y - i.rect.y) / i.rect.w - c.t * 0.12) - 0.5, 0.06) * i.p1.z; }
        col += vec3f(roll) * 0.06;
        col *= 1.0 - i.p1.x * pow(dot(uv, uv) * 0.5, 1.5);
        col *= inside;
        return vec4f(col, 1.0) * m;
    }
    // Chromatic split with optional glitch slices.
    let feather = max(i.p0.w, c.aa);
    let m = smoothstep(feather * 0.5, -feather * 0.5, c.d);
    if (m <= 0.0) { return vec4f(0.0); }
    var o = i.p0.xy;
    var gx = 0.0;
    if (i.p0.z > 0.0 && !c.reduced) {
        let slice = floor((c.p.y - i.rect.y) / 6.0);
        let fr = floor(c.t * 12.0);
        let hsl = hu(c.seed + bitcast<u32>(i32(slice)), bitcast<u32>(i32(fr)));
        if (u01(hsl) < 0.12 * i.p0.z) { gx = (u01(hsl + 1u) - 0.5) * 16.0 * i.p0.z; }
    }
    let p = c.p + vec2f(gx, 0.0);
    let col = vec3f(bd(p + o, s).r, bd(p, s).g, bd(p - o, s).b);
    return vec4f(col, 1.0) * m;
}
