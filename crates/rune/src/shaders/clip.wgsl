// Shared clip maths: the uber shader and compositor shaders (`rune::render::CLIP_WGSL`) use these.
// `clip_alpha(p, clip, fade)` takes a window device-pixel position, an instance's `clip` and its
// `fade` words (`vec4<u32>`), and returns the coverage of its rectangular, soft-edged and rounded
// (circular or continuous) clip.

// Apple's continuous corner of radius r: its curve leaves each edge K·r from the corner, and
// from the point (K·r, K·r) inside the corner it lies K·r·g(u) away, u running -1..1 across
// the quarter turn (0 on the diagonal; within 0.32% of r). Each quadrant takes its own
// corner's whole curve, as Core Animation does when K·r passes half the shorter side.
const CONTINUOUS_K: f32 = 1.52866483;
fn sd_continuous(p: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    let e = CONTINUOUS_K * r;
    let q = half - abs(p);
    let v = vec2<f32>(e) - q;
    if (v.x <= 0.0 || v.y <= 0.0) { return max(-q.x, -q.y); }
    let u = atan2(v.y, v.x) * (4.0 / 3.14159265) - 1.0;
    let u2 = u * u;
    return length(v) - e * (1.14602295 + u2 * (-0.37573161 + u2 * (0.30796609 - 0.07724354 * u2)));
}

fn sd_rounded(p: vec2<f32>, half: vec2<f32>, radii: vec4<f32>) -> f32 {
    // radii: top-left, top-right, bottom-right, bottom-left (y down). Negative is continuous.
    var r = radii.x;
    if (p.x > 0.0) {
        r = select(radii.y, radii.z, p.y > 0.0);
    } else {
        r = select(radii.x, radii.w, p.y > 0.0);
    }
    if (r < 0.0) { return sd_continuous(p, half, min(-r, min(half.x, half.y))); }
    r = min(r, min(half.x, half.y));
    let q = abs(p) - half + vec2<f32>(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - r;
}

// A soft edge's coverage `d` in from it: smoothstep over `fade`, linear over `-fade` when it's
// negative (`El::clip_fade_linear`), and a hard, anti-aliased edge at zero.
fn ramp(d: f32, fade: f32) -> f32 {
    if (fade == 0.0) {
        return clamp(d + 0.5, 0.0, 1.0);
    }
    let x = clamp(d / abs(fade), 0.0, 1.0);
    if (fade < 0.0) {
        return x;
    }
    return x * x * (3.0 - 2.0 * x);
}

// Piecewise-linear soft edge through up to three interior `(t, alpha)` stops already unpacked
// into `ts` and `ys`, anchored at `(0, 0)` and `(1, 1)` (`ScrollFade::stops`).
fn ramp_stops(d: f32, fade: f32, ts: vec4<f32>, ys: vec4<f32>) -> f32 {
    if (fade == 0.0) {
        return clamp(d + 0.5, 0.0, 1.0);
    }
    let w = abs(fade);
    if (d >= w) {
        return 1.0;
    }
    if (d <= 0.0) {
        return 0.0;
    }
    let x = d / w;
    var px = 0.0;
    var py = 0.0;
    for (var k = 0u; k < 4u; k++) {
        let nx = ts[k];
        let ny = ys[k];
        if (x <= nx) {
            let span = nx - px;
            if (span > 1e-6) {
                return mix(py, ny, (x - px) / span);
            }
            return ny;
        }
        px = nx;
        py = ny;
    }
    return 1.0;
}

fn unpack_clip_radii(w: u32) -> vec4<f32> {
    if (w == 0u) { return vec4<f32>(0.0); }
    if ((w & 0x60000000u) != 0x60000000u) {
        return vec4<f32>(bitcast<f32>(w));
    }
    let step = select(0.5, 2.0, (w & 0x10000000u) != 0u);
    let sgn = select(1.0, -1.0, (w & 0x80000000u) != 0u);
    return vec4<f32>(
        f32(w & 0x7fu),
        f32((w >> 7u) & 0x7fu),
        f32((w >> 14u) & 0x7fu),
        f32((w >> 21u) & 0x7fu),
    ) * (step * sgn);
}

fn clip_alpha_stops(p: vec2<f32>, c: vec4<f32>, packed: vec4<u32>, stops: vec2<f32>) -> f32 {
    let f = vec4<f32>(unpack2x16float(packed.x), unpack2x16float(packed.y));
    let hard = vec4<f32>(p - c.xy, c.zw - p);
    var alpha = 1.0;
    // `packed.z` is `@interpolate(flat)`, so these branches are uniform across every pixel of an
    // instance: instances without stop-tagged edges never unpack `stops` or run `ramp_stops`.
    if (packed.z == 0u) {
        alpha = ramp(hard.x, f.x) * ramp(hard.y, f.y) * ramp(hard.z, f.z) * ramp(hard.w, f.w);
    } else if ((packed.z & 0x80808080u) == 0u) {
        for (var edge = 0u; edge < 4u; edge++) {
            let off_u = (packed.z >> (edge * 8u)) & 0x7Fu;
            var a: f32;
            if (off_u >= 127u) {
                a = ramp(hard[edge], 0.0);
            } else if (off_u > 0u) {
                let offset = f32(off_u) * (1.0 / 127.0);
                a = min(ramp(hard[edge], 0.0), ramp(hard[edge] + offset * abs(f[edge]), f[edge]));
            } else {
                a = ramp(hard[edge], f[edge]);
            }
            alpha *= a;
        }
    } else {
        // Unpack the three interior `(t, alpha)` stops once outside the edge loop (`1/200` quantization).
        let lo = u32(stops.x + 0.5);
        let hi = u32(stops.y + 0.5);
        let ts = vec4<f32>(
            f32(lo & 0xFFu),
            f32((lo >> 16u) & 0xFFu),
            f32((hi >> 8u) & 0xFFu),
            200.0,
        ) * (1.0 / 200.0);
        let ys = vec4<f32>(
            f32((lo >> 8u) & 0xFFu),
            f32(hi & 0xFFu),
            f32((hi >> 16u) & 0xFFu),
            200.0,
        ) * (1.0 / 200.0);
        for (var edge = 0u; edge < 4u; edge++) {
            let raw_off = (packed.z >> (edge * 8u)) & 0xFFu;
            let is_stops = (raw_off & 0x80u) != 0u;
            let off_u = raw_off & 0x7Fu;
            // Keep the original sub-pixel ramp exactly when its origin is the hard edge.
            var a: f32;
            if (off_u >= 127u) {
                a = ramp(hard[edge], 0.0);
            } else if (off_u > 0u) {
                let shifted = hard[edge] + (f32(off_u) * (1.0 / 127.0)) * abs(f[edge]);
                var r: f32;
                if (is_stops) {
                    r = ramp_stops(shifted, f[edge], ts, ys);
                } else {
                    r = ramp(shifted, f[edge]);
                }
                a = min(ramp(hard[edge], 0.0), r);
            } else if (is_stops) {
                a = ramp_stops(hard[edge], f[edge], ts, ys);
            } else {
                a = ramp(hard[edge], f[edge]);
            }
            alpha *= a;
        }
    }
    if packed.w != 0u {
        let half = (c.zw - c.xy) * 0.5;
        alpha *= clamp(0.5 - sd_rounded(p - c.xy - half, half, unpack_clip_radii(packed.w)), 0.0, 1.0);
    }
    return alpha;
}

fn clip_alpha(p: vec2<f32>, c: vec4<f32>, packed: vec4<u32>) -> f32 {
    return clip_alpha_stops(p, c, packed, vec2<f32>(0.0));
}
