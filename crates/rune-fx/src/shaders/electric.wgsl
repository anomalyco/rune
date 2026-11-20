// Electricity. variant: 0 arc A -> B, 1 around the border, 2 charged border (calm UI variant).
// Bolt paths, crawl and forks are CPU tables (cpu.rs::electric); the shader only measures distance
// to them and lights it. xf = (first, N samples, forks, strands).
// c0 glow tint, c1 hot core, c2 cooled fork colour
// p0 (amplitude pt, frequency cells/100pt, speed, core width pt)
// p1 (intensity, glow radius pt, fork rate 0..1, flicker 0..1)
// p2 (strands, gap pt (border offset), fork length pt, occlusion)
// aux: A xy, B xy (arc)

struct Bolt { core: f32, halo: f32 }

fn shade_bolt(dist: f32, w: f32, gr: f32) -> Bolt {
    var b: Bolt;
    b.core = gauss(dist, w);
    let o = dist / gr;
    // A narrow coloured sheath plus a faint 1/(1+d^2) tail: the air right around the channel glows,
    // not a wide haze.
    b.halo = 0.35 / (1.0 + o * o) * (1.0 - smoothstep(gr * 2.5, gr * 5.0, dist)) + 0.65 * gauss(dist, gr * 0.6);
    return b;
}
// Lateral distance to a sampled path, slope-corrected but capped so steep jags never smear
// into streaks.
fn lat(dv: f32, a: f32, b: f32, step: f32) -> f32 {
    let s = (b - a) / step;
    // Near field: perpendicular distance to the local segment. Far field: the plain lateral offset
    // (already a good distance there), so the halo never streaks along steep jags.
    let k = inverseSqrt(1.0 + min(s * s, 3.0));
    return dv * mix(k, 1.0, smoothstep(1.5, 8.0, abs(dv)));
}
fn fork_col(i: Inst, heat: f32) -> vec3f {
    return mix(mix(i.c2.rgb, i.c0.rgb, smoothstep(0.0, 0.5, heat)), i.c1.rgb, smoothstep(0.6, 1.0, heat));
}

fn shade(i: Inst, px: vec2f) -> vec4f {
    let c = ctx(i, px);
    let w = max(i.p0.w, c.aa * 0.7);
    let inten = i.p1.x;
    let gr = max(i.p1.y, 1e-3);
    let gap = i.p2.y;
    let first = u32(i.xf.x);
    let n = u32(i.xf.y);
    let nf = u32(i.xf.z);
    let ns = u32(i.xf.w);
    let q4 = n >> 2u;
    let fl = data[first + 1u];
    let base = first + 2u;

    var core = 0.0;
    var halo = 0.0;
    var fork_hot = vec3f(0.0);

    if (c.variant == 0) {
        let a = i.aux.xy;
        let bb = i.aux.zw;
        let L = max(length(bb - a), 1.0);
        let dir = (bb - a) / L;
        let nrm = vec2f(-dir.y, dir.x);
        let rel = c.p - a;
        let u = dot(rel, dir) / L;
        let v = dot(rel, nrm);
        let uc = clamp(u, 0.0, 1.0);
        let fi = uc * f32(n - 1u);
        let i0 = min(u32(fi), n - 2u);
        let f = fi - f32(i0);
        let step = L / f32(n - 1u);
        for (var k = 0u; k < ns; k++) {
            let sb = base + k * q4;
            let d0 = dat1(sb, i0);
            let d1 = dat1(sb, i0 + 1u);
            let dd = length(vec2f((u - uc) * L, lat(v - mix(d0, d1, f), d0, d1, step)));
            let hb = base + (ns + k) * q4;
            let dh = length(vec2f((u - uc) * L, v - mix(dat1(hb, i0), dat1(hb, i0 + 1u), f)));
            let bw = select(w * 0.6, w, k == 0u);
            var bo = shade_bolt(dd, bw, gr);
            bo.halo = 0.4 * shade_bolt(dh, bw, gr).halo + 0.6 * shade_bolt(dd, bw, gr * 0.3).halo;
            let kb = select(0.32, 1.0, k == 0u) * fl[k];
            core += bo.core * kb;
            halo += bo.halo * kb;
        }
        // Electrode hot spots.
        halo += 0.6 * (gauss(length(c.p - a), gr * 0.8) + gauss(length(c.p - bb), gr * 0.8));
        core += 0.5 * (gauss(length(c.p - a), w * 2.5) + gauss(length(c.p - bb), w * 2.5));
        var fb = base + 2u * ns * q4;
        for (var j = 0u; j < nf; j++) {
            let h0 = data[fb];
            let h1 = data[fb + 1u];
            let o = h0.xy;
            let fd = h0.zw;
            let len = h1.x;
            let fr = c.p - o;
            let fu = dot(fr, fd) / len;
            let fv = dot(fr, vec2f(-fd.y, fd.x));
            let fuc = clamp(fu, 0.0, h1.y);
            let ffi = fuc * 15.0;
            let j0 = min(u32(ffi), 14u);
            let s0 = dat1(fb + 2u, j0);
            let s1 = dat1(fb + 2u, j0 + 1u);
            let fdist = length(vec2f((fu - fuc) * len, lat(fv - mix(s0, s1, ffi - f32(j0)), s0, s1, len / 15.0)));
            let taper = 1.0 - 0.7 * fuc;
            let bo = shade_bolt(fdist, w * 0.6 * taper, gr * 0.55);
            fork_hot += fork_col(i, h1.w) * (bo.core * 1.1 + bo.halo * 0.45) * h1.z * taper;
            fb += 6u;
        }
    } else {
        let pr = rrect_perim(c.q, c.half, c.r);
        let s = pr.x;
        let total = pr.y;
        let calm = c.variant == 2;
        let fi = s / total * f32(n);
        let i0 = u32(floor(fi)) % n;
        let i1 = (i0 + 1u) % n;
        let f = fi - floor(fi);
        let step = total / f32(n);
        for (var k = 0u; k < ns; k++) {
            let sb = base + k * q4;
            let d0 = dat1(sb, i0);
            let d1 = dat1(sb, i1);
            let dd = abs(lat(c.d - gap - mix(d0, d1, f), d0, d1, step));
            let hb = base + (ns + k) * q4;
            let dh = abs(c.d - gap - mix(dat1(hb, i0), dat1(hb, i1), f));
            let cb = base + (2u * ns + k) * q4;
            let crawl = mix(dat1(cb, i0), dat1(cb, i1), f);
            let bw = select(w * 0.6, w, k == 0u);
            var bo = shade_bolt(dd, bw, gr);
            bo.halo = 0.4 * shade_bolt(dh, bw, gr).halo + 0.6 * shade_bolt(dd, bw, gr * 0.3).halo;
            let kb = select(0.32, 1.0, k == 0u) * fl[k] * crawl;
            core += bo.core * kb;
            halo += bo.halo * kb;
        }
        if (calm) {
            // Beads of charge travelling one way around the border.
            let sp = i.p0.z * 0.35;
            let bu = fract(s / total - c.t * 0.07 * sp);
            for (var k = 0; k < 3; k++) {
                let x = fract(bu - f32(k) / 3.0 + 0.5) - 0.5;
                let bead = gauss(x * total, 18.0);
                halo += bead * gauss(c.d - gap, gr * 0.8) * 0.9;
                core += bead * gauss(c.d - gap, w * 1.5) * 0.6;
            }
        }
        var fb = base + 3u * ns * q4;
        for (var j = 0u; j < nf; j++) {
            let h0 = data[fb];
            let h1 = data[fb + 1u];
            let len = h1.x;
            var dsv = s - h0.x;
            dsv -= total * round(dsv / total);
            let fu = (c.d - gap - h0.z) / len;
            let fuc = clamp(fu, 0.0, h1.y);
            let ffi = fuc * 15.0;
            let j0 = min(u32(ffi), 14u);
            let s0 = dat1(fb + 2u, j0);
            let s1 = dat1(fb + 2u, j0 + 1u);
            let fdist = length(vec2f((fu - fuc) * len, lat(dsv - mix(s0, s1, ffi - f32(j0)), s0, s1, len / 15.0)));
            let taper = 1.0 - 0.7 * fuc;
            let bo = shade_bolt(fdist, w * 0.6 * taper, gr * 0.55);
            fork_hot += fork_col(i, h1.w) * (bo.core * 1.1 + bo.halo * 0.45) * h1.z * taper;
            fb += 6u;
        }
    }
    let occl = select(1.0, i.p2.w, effective_blend_mode(i) == 0u);
    let rgb = (i.c1.rgb * core * 1.3 + i.c0.rgb * halo * 0.75 + fork_hot) * inten;
    let a = (core * 0.7 + halo * occl + lum(fork_hot) * 0.5) * inten;
    return finish(vec4f(rgb, a), px);
}
