// Fills: styles that paint the node's shape, or a band inside its edge (aux.x = border pt, 0 = whole
// shape). aux.y = intensity. Colour lists live in the CPU table: for stop k, data[xf.x + 2k] is
// linear rgb (w: mesh point x), data[xf.x + 2k + 1] is OKLab (w: mesh point y); xf.y stops.
// variant:
// 0 liquid metal  c0 tint  c1 light  p0 (flow, scale pt, speed, bevel pt)  p1 (dome, angle, iridescence, contrast)
//                 p2 (anchor: 0 window, 1 shape, _, _, _)
// 1 mesh          p0 (drift, speed, sharpness, warp)  (points are animated on the CPU)
// 2 dither        p0 (pattern, input, pixel pt, ramp angle)  p1 (noise scale pt, speed, bias, contrast)
//                 xf.zw blue-noise tile (offset, side)
// 3 grain grad.   p0 (kind, angle, centre x, centre y)  p1 (amount, size pt, mono, fps)
// 4 fluted glass  c0 tint  p0 (rib pt, angle, refraction pt, frost)  p1 (dispersion, highlight, tint amount, _)
//                 xf.xy frost pyramid (set by FxRenderer::blur_backdrop)
// 5 metaballs     c0 base  p0 (count, size, speed, gloss)  xf.z balls: (x pt, y pt, r pt, stop)
// 6 swirl         p0 (scale pt, speed, twist, warp)
// 7 voronoi       c0 seam  p0 (cell pt, speed, seam pt, depth)
// 8 waves         p0 (amplitude, frequency, speed, angle)  p1 (softness, _, _, _)
// 9 halftone      c0 ink  c1 base  p0 (_, input, pitch pt, ramp angle)  p1 as dither  p2 (screen angle, _, _, _)
// 10 plastic      c0 colour  p0 (roughness, dome, bevel pt, light angle)

// Shape coverage: the whole shape, or (border > 0) a band of that width inside the edge.
fn fill_mask(c: Ctx, border: f32) -> f32 {
    let inside = clamp(0.5 - c.d * c.scale, 0.0, 1.0);
    if (border <= 0.0) { return inside; }
    return inside * clamp(0.5 + (c.d + border) * c.scale, 0.0, 1.0);
}

fn rot2(v: vec2f, a: f32) -> vec2f {
    let cs = cos(a);
    let sn = sin(a);
    return vec2f(cs * v.x - sn * v.y, sn * v.x + cs * v.y);
}

// ---------- colour lists ----------
fn stop_lin(f0: u32, k: u32) -> vec3f { return data[f0 + 2u * k].rgb; }
fn stop_lab(f0: u32, k: u32) -> vec3f { return data[f0 + 2u * k + 1u].rgb; }
fn stop_pt(f0: u32, k: u32) -> vec2f { return vec2f(data[f0 + 2u * k].w, data[f0 + 2u * k + 1u].w); }

fn oklab_to_lin(c: vec3f) -> vec3f {
    let l = c.x + 0.39633778 * c.y + 0.21580376 * c.z;
    let m = c.x - 0.10556135 * c.y - 0.06385417 * c.z;
    let s = c.x - 0.08948418 * c.y - 1.29148555 * c.z;
    let l3 = l * l * l;
    let m3 = m * m * m;
    let s3 = s * s * s;
    return max(vec3f(
        4.07674166 * l3 - 3.30771159 * m3 + 0.23096993 * s3,
        -1.26843800 * l3 + 2.60975740 * m3 - 0.34131940 * s3,
        -0.00419609 * l3 - 0.70341861 * m3 + 1.70761470 * s3,
    ), vec3f(0.0));
}

// The colour list as an even ramp over x in 0..1, blended in OKLab.
fn palette_ramp(f0: u32, n: u32, x: f32) -> vec3f {
    if (n <= 1u) { return stop_lin(f0, 0u); }
    let fx = clamp(x, 0.0, 1.0) * f32(n - 1u);
    let k = min(u32(fx), n - 2u);
    return oklab_to_lin(mix(stop_lab(f0, k), stop_lab(f0, k + 1u), fx - f32(k)));
}

// The colour list as a closed loop over x in 0..1: the last colour blends back into the first.
fn ramp_loop(f0: u32, n: u32, x: f32) -> vec3f {
    if (n <= 1u) { return stop_lin(f0, 0u); }
    let fx = fract(x) * f32(n);
    let k = min(u32(fx), n - 1u);
    return oklab_to_lin(mix(stop_lab(f0, k), stop_lab(f0, (k + 1u) % n), fx - f32(k)));
}

// Premultiplied output with the fill's coverage and intensity.
fn paint(col: vec3f, m: f32, i: Inst, px: vec2f) -> vec4f {
    let a = m * clamp(i.aux.y, 0.0, 1.0);
    return finish(vec4f(col * a, a), px);
}

// ---------- tone inputs (dither, halftone) ----------
// Perceptual tone 0..1 at `p` for input `kind`: 0 linear ramp, 1 radial, 2 noise, 3 backdrop, 4 image.
fn tone_at(i: Inst, c: Ctx, p: vec2f, kind: i32, angle: f32) -> f32 {
    var v: f32;
    let q = p - c.c;
    if (kind == 0) {
        let dir = vec2f(cos(angle), sin(angle));
        let ext = abs(c.half.x * dir.x) + abs(c.half.y * dir.y);
        v = dot(q, dir) / max(ext, 1e-3) * 0.5 + 0.5;
    } else if (kind == 1) {
        v = 1.0 - clamp(length(q / max(c.half, vec2f(1e-3))) / 1.3, 0.0, 1.0);
    } else if (kind == 2) {
        let t = c.t * i.p1.y;
        v = 0.5 + 0.62 * fbm3(p / max(i.p1.x, 1.0) + vec2f(t * 0.31, t * 0.17), c.seed);
    } else if (kind == 3) {
        v = to_srgb(vec3f(lum(bd(p, c.scale).rgb))).x;
    } else {
        let s = textureSampleLevel(source, samp, (p - i.aux2.xy) / max(i.aux2.zw, vec2f(1e-3)), 0.0);
        v = to_srgb(vec3f(lum(s.rgb))).x;
    }
    return clamp((v - 0.5) * i.p1.w + 0.5 + i.p1.z * 0.5, 0.0, 1.0);
}

// Ordered-dither threshold of an n×n Bayer matrix (n = 2^bits), from the bit-interleave recurrence.
fn bayer(cell: vec2u, bits: u32) -> f32 {
    var v = 0u;
    for (var b = 0u; b < bits; b++) {
        let x = (cell.x >> b) & 1u;
        let y = (cell.y >> b) & 1u;
        let sh = 2u * (bits - 1u - b);
        v |= (((x ^ y) << 1u) | y) << sh;
    }
    return (f32(v) + 0.5) / f32(1u << (2u * bits));
}

// ---------- lit slabs (liquid metal, plastic) ----------
// Slope of a slab's surface normal as (x, y) plus how far onto the rim it is (0..1). With a border
// the band is a round tube; otherwise the edge rolls off in a quarter round over `bevel` pt and
// the face is domed (negative `dome` dishes it).
fn slab_tilt(c: Ctx, border: f32, bevel: f32, dome: f32) -> vec3f {
    let depth = -c.d;
    let gv = sd_grad(c);
    var tilt = vec2f(0.0);
    var rim = 0.0;
    if (border > 0.0) {
        let x = clamp((depth - border * 0.5) / max(border * 0.5, 1e-3), -0.97, 0.97);
        tilt = -gv * x / sqrt(1.0 - x * x);
        rim = abs(x);
    } else {
        let bev = max(bevel, 0.5);
        let x = clamp(1.0 - depth / bev, 0.0, 0.92);
        tilt = gv * x / sqrt(1.0 - x * x);
        rim = x;
    }
    let qn = c.q / max(c.half, vec2f(1.0));
    tilt += vec2f(qn.x * 0.55, qn.y) * dome * select(0.8, 0.25, border > 0.0);
    return vec3f(tilt, rim);
}

// ---------- liquid metal ----------
// Height of the flowing surface (in units of `scale`), domain-warped so ripples curl and pool.
fn flow_h(u: vec2f, w: vec2f, t: f32, seed: u32) -> f32 {
    let p = u + w;
    return gnoise(p + vec2f(0.0, t * 0.11), seed + 3u) + 0.5 * gnoise(ROT * p * 2.03 - vec2f(t * 0.07, 0.0), seed + 17u);
}

// A photo studio seen from a polished object, by reflected direction r (screen y points down, +z
// toward the viewer): a bright sky that is lightest just above a hard horizon, an overhead soft
// box, a tall strip light on the right and a dim one on the left, over a dark floor that lightens
// away from the horizon. White lights; the caller tints them.
fn studio(r: vec3f, contrast: f32) -> vec3f {
    let up = -r.y;
    let e = up + 0.1; // elevation above the horizon (which sits a little below eye level)
    let w = 0.014;
    let sky = 0.2 + 0.6 * exp(-max(e, 0.0) * 3.2) + 0.45 * exp(-max(e, 0.0) * 24.0);
    let key = smoothstep(0.80, 0.95, dot(r, normalize(vec3f(-0.3, -0.82, 0.5)))) * 2.4;
    let strip = gauss(r.x - 0.6, 0.075) * smoothstep(-0.05, 0.2, e) * (1.0 - smoothstep(0.55, 0.9, up)) * 1.7;
    let fill_l = gauss(r.x + 0.72, 0.045) * smoothstep(0.0, 0.25, e) * (1.0 - smoothstep(0.4, 0.8, up)) * 0.55;
    let lights = (key + strip + fill_l) * contrast;
    // The floor: near black under the horizon, lifting toward the camera, with a soft pool of
    // bounced light off to the left.
    let fl = smoothstep(0.05, 1.0, -e);
    let pool = 0.16 * gauss(r.x + 0.25, 0.35) * smoothstep(0.25, 0.9, -e);
    let ground = (0.014 + 0.3 * fl * fl + pool + 0.05 * gauss(e + 0.06, 0.04)) / (contrast * contrast);
    let h = smoothstep(-w, w, e);
    let skyc = (sky * mix(1.0, 0.82, clamp(contrast - 1.0, 0.0, 1.0)) + lights) * vec3f(0.97, 0.99, 1.03);
    let floorc = ground * vec3f(1.04, 1.0, 0.95);
    return mix(floorc, skyc, h);
}

// Gentle shoulder so lights roll off instead of clipping (per channel: hot gold goes pale).
fn shoulder(c: vec3f) -> vec3f {
    let k = 0.78;
    return select(c, k + (1.0 - k) * (1.0 - exp(-(c - k) / (1.0 - k))), c > vec3f(k));
}

fn liquid_metal(i: Inst, c: Ctx, m: f32, px: vec2f) -> vec4f {
    let flow = i.p0.x;
    let sc = max(i.p0.y, 1.0);
    let t = c.t * i.p0.z;
    // Tilt of the surface normal (where it leans, as a slope): rim or tube + dome + liquid ripples.
    let slab = slab_tilt(c, i.aux.x, i.p0.w, i.p1.x);
    var tilt = slab.xy;
    let rim = slab.z;
    var hgt = 0.0;
    var bend = 0.0;
    if (flow > 0.0) {
        // Pinned to the window, or to the shape's centre so equal shapes ripple alike.
        let u = select(c.p, c.q, i.p2.x > 0.5) / sc;
        let wv = vec2f(gnoise(u * 0.6 + vec2f(t * 0.05, 1.7), c.seed + 7u), gnoise(u * 0.6 + vec2f(4.3, -t * 0.06), c.seed + 11u)) * 0.9;
        let e = 0.04;
        hgt = flow_h(u, wv, t, c.seed);
        let hx = flow_h(u + vec2f(e, 0.0), wv, t, c.seed) - hgt;
        let hy = flow_h(u + vec2f(0.0, e), wv, t, c.seed) - hgt;
        let grad = vec2f(hx, hy) / e;
        // Ripples calm toward the rim so the silhouette stays crisp.
        let k = flow * 0.42 * (1.0 - 0.6 * rim * rim);
        tilt -= grad * k;
        bend = length(grad) * k;
    }
    let n = normalize(vec3f(tilt, 1.0));
    var r = vec3f(2.0 * n.z * n.xy, 2.0 * n.z * n.z - 1.0);
    r = vec3f(rot2(r.xy, i.p1.y), r.z);
    let contrast = max(i.p1.w, 0.1);
    let env = studio(normalize(r), contrast) * i.c1.rgb;
    // Conductor Fresnel (Schlick): the tint at normal incidence, rising to white at grazing angles.
    let f0 = i.c0.rgb;
    let cz = clamp(1.0 - n.z, 0.0, 1.0);
    let cz2 = cz * cz;
    var f = f0 + (vec3f(1.0) - f0) * (cz2 * cz2 * cz);
    let ir = i.p1.z;
    if (ir > 0.0) {
        // Thin-film colour where the surface bends hardest.
        let ph = 1.6 * (1.0 - n.z) + 0.35 * hgt;
        let film = 0.5 + 0.5 * cos(TAU * (vec3f(ph) + vec3f(0.0, 0.33, 0.67)));
        let k = ir * clamp(smoothstep(0.04, 0.45, 1.0 - n.z) + bend * 0.8, 0.0, 1.0);
        f *= mix(vec3f(1.0), 0.55 + 0.9 * film, k);
    }
    let col = shoulder(f * env);
    return paint(col, m, i, px);
}

// ---------- plastic ----------
// A dielectric under one soft light from `angle` (50° up), seen head on. The diffuse base keeps the
// colour on a flat face, darkens away from the light and lightens toward white facing it. The
// highlight is a GGX lobe widened by the light's size (so a polished face shows a soft spot, not a
// pinpoint) and the grazing-angle reflection is a faint sky. Both fill the headroom left above the
// base, so the colour never exceeds coverage and the fill blends alike in either blend mode.
fn plastic(i: Inst, c: Ctx, m: f32, px: vec2f) -> vec4f {
    let slab = slab_tilt(c, i.aux.x, i.p0.z, i.p0.y);
    let n = normalize(vec3f(slab.xy, 1.0));
    let l = vec3f(rot2(vec2f(0.0, -1.0), i.p0.w) * 0.6428, 0.766);
    let nl = dot(n, l);
    let base = clamp(i.c0.rgb, vec3f(0.0), vec3f(1.0));
    // 0.4 ambient + 0.6 direct on a flat face (n.l = 0.766).
    let shade = 0.4 + 0.7833 * max(nl, 0.0);
    var col = mix(base * min(shade, 1.0), vec3f(1.0), clamp((shade - 1.0) * 0.6, 0.0, 1.0));
    let rough = clamp(i.p0.x, 0.02, 1.0);
    let a = min(rough * rough + 0.1, 1.0);
    let a2 = a * a;
    let h = normalize(l + vec3f(0.0, 0.0, 1.0));
    let nh = max(dot(n, h), 0.0);
    let dd = nh * nh * (a2 - 1.0) + 1.0;
    let ndf = a2 / (PI * dd * dd);
    let fh = 1.0 - h.z;
    let spec = ndf * (0.04 + 0.96 * fh * fh * fh * fh * fh) * max(nl, 0.0) * 2.5;
    let fz = 1.0 - clamp(n.z, 0.0, 1.0);
    let sky = 0.55 + 0.45 * smoothstep(-0.5, 0.5, -n.y);
    let edge = 0.96 * fz * fz * fz * fz * fz * sky * (1.0 - 0.5 * rough);
    col += (vec3f(1.0) - col) * knee(spec + edge);
    return paint(col, m, i, px);
}

// ---------- fluted glass ----------
fn fluted(i: Inst, c: Ctx, m: f32, px: vec2f) -> vec4f {
    let s = c.scale;
    let rib = max(i.p0.x, 1.0);
    let across = vec2f(cos(i.p0.y), sin(i.p0.y));
    let u = dot(c.p - c.c, across) / rib;
    let local = fract(u) - 0.5;
    let x = local * 2.0; // -1..1 across one rib
    // A convex rib acts as a cylindrical lens: the strip under it comes back squeezed and flipped.
    let o = -x * (1.0 + 0.35 * x * x) * i.p0.z;
    let dsp = i.p1.x * 0.3 * abs(x);
    let srcg = c.p + across * o;
    let srcr = c.p + across * o * (1.0 + dsp);
    let srcb = c.p + across * o * (1.0 - dsp);
    var col = vec3f(bd(srcr, s).r, bd(srcg, s).g, bd(srcb, s).b);
    let frost = i.p0.w;
    if (frost > 0.0) {
        if (i.xf.x > 0.5) {
            let blr = vec3f(bdb(srcr, s).r, bdb(srcg, s).g, bdb(srcb, s).b);
            col = mix(col, blr, i.xf.y);
        } else {
            // No pyramid this frame: a small blur along the ribs.
            let along = vec2f(-across.y, across.x);
            let j = ign(px) - 0.5;
            var acc = vec3f(0.0);
            for (var k = 0; k < 4; k++) {
                acc += bd(srcg + along * (f32(k) - 1.5 + j) * 3.0 * frost, s).rgb;
            }
            col = mix(col, acc * 0.25, frost);
        }
        let k = smoothstep(0.0, 0.6, frost);
        let l = lum(col);
        let sat = max(mix(vec3f(l), col, 1.0 + 0.5 * k), vec3f(0.0));
        col = to_lin(to_srgb(sat) * (1.0 - 0.2 * k) + vec3f(0.07 * k));
    }
    col = mix(col, col * i.c0.rgb, i.p1.z);
    // Light: a sheen down each rib's lit side, a shaded groove between ribs with a bright lip.
    let hl = i.p1.y;
    let aa = c.aa / rib;
    let groove = 1.0 - smoothstep(0.0, max(0.06, aa * 2.0), 0.5 - abs(local));
    // The sheen scales with what is behind (glass reflects little over black) plus a faint floor.
    let sheen = gauss(local + 0.22, 0.13) * 0.1 + gauss(local + 0.34, 0.03) * 0.1;
    let lip = gauss(local - 0.44, max(0.025, aa)) * 0.06;
    let lift = 0.35 + 0.65 * sqrt(clamp(lum(col), 0.0, 1.0));
    col = col * (1.0 - 0.28 * hl * groove) + vec3f((sheen + lip) * hl * lift);
    let a = m * clamp(i.aux.y, 0.0, 1.0);
    return vec4f(col * a, a);
}

// ---------- the rest ----------
fn mesh_gradient(i: Inst, c: Ctx, m: f32, px: vec2f) -> vec4f {
    let f0 = u32(i.xf.x);
    let n = u32(i.xf.y);
    var uv = (c.p - i.rect.xy) / max(i.rect.zw, vec2f(1.0));
    let t = c.t * i.p0.y;
    let warp = i.p0.w;
    if (warp > 0.0) {
        uv += warp * 0.09 * vec2f(gnoise(uv * 2.1 + vec2f(t * 0.11, 0.0), c.seed + 5u), gnoise(uv * 2.1 + vec2f(3.7, -t * 0.09), c.seed + 9u));
    }
    let aspect = i.rect.zw / max(max(i.rect.z, i.rect.w), 1.0);
    let sharp = clamp(i.p0.z, 0.1, 8.0);
    var lab = vec3f(0.0);
    var wsum = 0.0;
    for (var k = 0u; k < n; k++) {
        let d = (uv - stop_pt(f0, k)) * aspect;
        // Shepard weights: each colour owns its point, blends fall off smoothly.
        let w = pow(dot(d, d) + 0.004, -sharp);
        lab += stop_lab(f0, k) * w;
        wsum += w;
    }
    return paint(oklab_to_lin(lab / max(wsum, 1e-6)), m, i, px);
}

fn dither(i: Inst, c: Ctx, m: f32) -> vec4f {
    let f0 = u32(i.xf.x);
    let n = max(u32(i.xf.y), 1u);
    let pix = max(i.p0.z, 0.25);
    let rel = c.p - i.rect.xy;
    let cell = floor(rel / pix);
    let ci = bitcast<vec2u>(vec2i(cell));
    let tone = tone_at(i, c, i.rect.xy + (cell + 0.5) * pix, i32(i.p0.y + 0.5), i.p0.w);
    let pat = clamp(i32(i.p0.x + 0.5), 1, 5);
    var th: f32;
    if (pat <= 3) {
        th = bayer(ci, u32(pat));
    } else if (pat == 4) {
        let side = max(u32(i.xf.w), 1u);
        th = dat1(u32(i.xf.z), (ci.y % side) * side + ci.x % side);
    } else {
        th = u01(ci.x * 7919u + pcg(ci.y + c.seed));
    }
    let lv = f32(max(n, 2u) - 1u);
    let v = tone * lv;
    let k = min(u32(floor(v) + select(0.0, 1.0, fract(v) > th)), n - 1u);
    let a = m * clamp(i.aux.y, 0.0, 1.0);
    return vec4f(stop_lin(f0, k) * a, a);
}

fn grain_gradient(i: Inst, c: Ctx, m: f32, px: vec2f) -> vec4f {
    let f0 = u32(i.xf.x);
    let n = u32(i.xf.y);
    let kind = i32(i.p0.x + 0.5);
    let ang = i.p0.y;
    let ctr = i.rect.xy + i.p0.zw * i.rect.zw;
    var x: f32;
    if (kind == 0) {
        let dir = vec2f(cos(ang), sin(ang));
        let ext = abs(c.half.x * dir.x) + abs(c.half.y * dir.y);
        x = dot(c.q, dir) / max(ext, 1e-3) * 0.5 + 0.5;
    } else if (kind == 1) {
        x = length(c.p - ctr) / max(length(c.half), 1e-3);
    } else {
        let d = c.p - ctr;
        let th = select(0.0, atan2(d.y, d.x), any(d != vec2f(0.0)));
        x = fract((th - ang) / TAU);
    }
    let amount = i.p1.x;
    let size = max(i.p1.y, 0.25);
    var frame = 0u;
    if (i.p1.w > 0.0) { frame = u32(floor(c.t * i.p1.w)); }
    let gc = bitcast<vec2u>(vec2i(floor((c.p - i.rect.xy) / size)));
    let h = pcg(gc.x + pcg(gc.y + pcg(frame + c.seed * 977u)));
    // Triangular noise scatters the ramp position: colours stipple into each other.
    let nx = u01(h) + u01(h + 1u) - 1.0;
    var col = palette_ramp(f0, n, x + nx * amount * 0.2);
    if (kind == 2) {
        col = ramp_loop(f0, n, x + nx * amount * 0.2);
    }
    let nb = u01(h + 2u) + u01(h + 3u) - 1.0;
    if (i.p1.z > 0.5) {
        col *= 1.0 + nb * amount * 0.18;
    } else {
        let nc = vec3f(nb, u01(h + 4u) + u01(h + 5u) - 1.0, u01(h + 6u) + u01(h + 7u) - 1.0);
        col *= vec3f(1.0) + nc * amount * 0.18;
    }
    return paint(col, m, i, px);
}

fn metaballs(i: Inst, c: Ctx, m: f32, px: vec2f) -> vec4f {
    let f0 = u32(i.xf.x);
    let b0 = u32(i.xf.z);
    let nb = u32(i.xf.w);
    var field = 0.0;
    var grad = vec2f(0.0);
    var lab = vec3f(0.0);
    var rsum = 0.0;
    for (var k = 0u; k < nb; k++) {
        let b = data[b0 + k];
        let d = c.p - b.xy;
        let r2 = b.z * b.z;
        let dd = dot(d, d) + 1e-3;
        let f = r2 / dd;
        field += f;
        grad += -2.0 * f / dd * d;
        lab += stop_lab(f0, u32(b.w)) * f;
        rsum += b.z * f;
    }
    let base = i.c0.rgb;
    let col = select(base, oklab_to_lin(lab / max(field, 1e-6)), field > 1e-5);
    let ravg = rsum / max(field, 1e-6);
    // Distance to the iso-surface field = 1, roughly (1 - field) * r / 2 near the edge.
    let cov = select(0.0, clamp(0.5 + (field - 1.0) * ravg * 0.5 * c.scale, 0.0, 1.0), field > 1e-3);
    // Glossy body: the field's slope tilts the normal near the rim, flat in the middle.
    let steep = 1.0 - smoothstep(1.0, 2.6, field);
    let nxy = -grad / sqrt(dot(grad, grad) + 1e-6) * steep * 0.9;
    let nrm = normalize(vec3f(nxy, 1.0));
    let l = normalize(vec3f(-0.45, -0.6, 0.66));
    let diff = max(dot(nrm, l), 0.0);
    let spec = pow(max(dot(reflect(-l, nrm), vec3f(0.0, 0.0, 1.0)), 1e-4), 28.0);
    let body = col * (0.6 + 0.5 * diff) + vec3f(spec * i.p0.w * 0.8);
    // A soft glow of each blob's colour into the base.
    let halo = col * smoothstep(0.25, 1.0, field) * 0.18;
    return paint(mix(base + halo, body, cov), m, i, px);
}

fn swirl(i: Inst, c: Ctx, m: f32, px: vec2f) -> vec4f {
    let f0 = u32(i.xf.x);
    let n = u32(i.xf.y);
    let sc = max(i.p0.x, 1.0);
    let t = c.t * i.p0.y;
    let rr = length(c.q) / max(min(c.half.x, c.half.y), 1.0);
    let q = rot2(c.q, i.p0.z * TAU * exp(-rr * rr * 0.8)) / sc;
    let w1 = vec2f(fbm3(q + vec2f(t * 0.1, 0.0), c.seed), fbm3(q + vec2f(5.2, 1.3 - t * 0.08), c.seed + 3u));
    let v = fbm3(q + w1 * i.p0.w * 1.5 + vec2f(0.0, t * 0.05), c.seed + 7u);
    return paint(palette_ramp(f0, n, 0.5 + 0.75 * v), m, i, px);
}

fn voronoi(i: Inst, c: Ctx, m: f32, px: vec2f) -> vec4f {
    let f0 = u32(i.xf.x);
    let n = u32(i.xf.y);
    let sc = max(i.p0.x, 2.0);
    let t = c.t * i.p0.y;
    let t0 = fract(t * (1.0 / TAU)) * TAU;
    let t1 = fract(t * (0.8 / TAU)) * TAU;
    let p = (c.p - i.rect.xy) / sc;
    let ip = floor(p);
    // Each cell's seed point orbits its own spot.
    var best = 8.0;
    var bc = vec2i(0);
    var bo = vec2f(0.0);
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let cc = vec2i(ip) + vec2i(x, y);
            let h = hcell(cc, c.seed);
            let h2 = hcell(cc, c.seed + 1u);
            let o = vec2f(0.5) + 0.38 * vec2f(sin(t0 + TAU * h), cos(t1 + TAU * h2));
            let d = vec2f(cc) + o - p;
            let dd = dot(d, d);
            if (dd < best) { best = dd; bc = cc; bo = d; }
        }
    }
    // Distance to the nearest seam: to the bisector with each neighbour. Seed points stay within
    // 0.12..0.88 of their cell, so the eight cells round the winner are enough in practice.
    var edge = 8.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let cc = bc + vec2i(x, y);
            if (x == 0 && y == 0) { continue; }
            let h = hcell(cc, c.seed);
            let h2 = hcell(cc, c.seed + 1u);
            let o = vec2f(0.5) + 0.38 * vec2f(sin(t0 + TAU * h), cos(t1 + TAU * h2));
            let d = vec2f(cc) + o - p;
            let dv = d - bo;
            if (dot(dv, dv) > 1e-6) { edge = min(edge, dot(0.5 * (bo + d), normalize(dv))); }
        }
    }
    let k = u32(hcell(bc, c.seed + 2u) * f32(n)) % max(n, 1u);
    var col = stop_lin(f0, k) * (1.0 - i.p0.w * 0.55 * clamp(best * 1.6, 0.0, 1.0));
    let seam = i.p0.z * 0.5;
    if (seam > 0.0) {
        let e = clamp(0.5 + (edge * sc - seam) * c.scale, 0.0, 1.0);
        col = mix(i.c0.rgb, col, e);
    }
    return paint(col, m, i, px);
}

fn waves(i: Inst, c: Ctx, m: f32, px: vec2f) -> vec4f {
    let f0 = u32(i.xf.x);
    let n = u32(i.xf.y);
    let ang = i.p0.w;
    let dir = vec2f(-sin(ang), cos(ang));
    let along = vec2f(cos(ang), sin(ang));
    let ext = abs(c.half.x * dir.x) + abs(c.half.y * dir.y);
    let ext2 = abs(c.half.x * along.x) + abs(c.half.y * along.y);
    let v = dot(c.q, dir) / max(2.0 * ext, 1e-3) + 0.5;
    let s = dot(c.q, along) / max(2.0 * ext2, 1e-3) + 0.5;
    let aa = c.aa / max(2.0 * ext, 1e-3);
    let soft = max(i.p1.x, aa);
    let t = c.t * i.p0.z;
    var col = stop_lin(f0, 0u);
    for (var k = 1u; k < n; k++) {
        let fk = f32(k);
        let h = hcell(vec2i(i32(k), 3), c.seed);
        let fq = i.p0.y * (0.8 + 0.4 * h);
        let ph1 = fract(t * 0.2 * (1.0 + 0.3 * fk) + h);
        let ph2 = fract(-t * 0.13) * TAU + 1.7 * fk;
        let y = fk / f32(n) + i.p0.x * (0.7 * sin(TAU * (fq * s + ph1)) + 0.3 * sin(TAU * fq * 2.3 * s + ph2));
        // The front band shades the one behind it just above its crest.
        let shade = exp(-max(y - v, 0.0) / 0.025) * 0.18 * step(v, y);
        col *= 1.0 - shade;
        col = mix(col, stop_lin(f0, k), smoothstep(y - soft, y + soft, v));
    }
    return paint(col, m, i, px);
}

fn halftone(i: Inst, c: Ctx, m: f32, px: vec2f) -> vec4f {
    let pitch = max(i.p0.z, 1.0);
    let ang = i.p2.x;
    let r = rot2(c.p - c.c, -ang) / pitch;
    let base = floor(r - 0.5);
    let kind = i32(i.p0.y + 0.5);
    var ink = 0.0;
    // The four nearest dots, so big dots merge across cell lines.
    for (var y = 0; y <= 1; y++) {
        for (var x = 0; x <= 1; x++) {
            let cc = base + vec2f(f32(x), f32(y)) + 0.5;
            let w = c.c + rot2(cc * pitch, ang);
            let cov = 1.0 - tone_at(i, c, w, kind, i.p0.w);
            let rad = sqrt(cov / PI) * 1.12;
            let d = (length(r - cc) - rad) * pitch;
            ink = max(ink, clamp(0.5 - d * c.scale, 0.0, 1.0));
        }
    }
    return paint(mix(i.c1.rgb, i.c0.rgb, ink), m, i, px);
}

fn shade(i: Inst, px: vec2f) -> vec4f {
    let c = ctx(i, px);
    let m = fill_mask(c, i.aux.x);
    if (m <= 0.0) { return vec4f(0.0); }
    switch c.variant {
        case 0: { return liquid_metal(i, c, m, px); }
        case 1: { return mesh_gradient(i, c, m, px); }
        case 2: { return dither(i, c, m); }
        case 3: { return grain_gradient(i, c, m, px); }
        case 4: { return fluted(i, c, m, px); }
        case 5: { return metaballs(i, c, m, px); }
        case 6: { return swirl(i, c, m, px); }
        case 7: { return voronoi(i, c, m, px); }
        case 8: { return waves(i, c, m, px); }
        case 9: { return halftone(i, c, m, px); }
        default: { return plastic(i, c, m, px); }
    }
}
