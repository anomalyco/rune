// Pulses. variant: 0 radar rings, 1 breathing halo, 2 heartbeat, 3 ping (travels A -> along dir), 4 tap ripple.
// c0 colour, c1 hot/core colour
// p0 (count, period, reach, width)
// p1 (intensity, t0, duration, repeat)    t0/duration/repeat: one-shots (ping, ripple)
// p2 (travel, lag, head radius, occlusion)
// aux: origin xy, direction xy (ping) | origin xy (ripple)

// Ease with anticipation for the ping: cubic Bézier in value, P1 < 0 winds up against the travel,
// P2 = 1 lands with zero velocity (decelerates in, never stops dead).
fn shoot(u: f32) -> f32 {
    let x = clamp(u, 0.0, 1.0);
    let m = 1.0 - x;
    return 3.0 * m * m * x * (-0.12) + 3.0 * m * x * x * 1.0 + x * x * x;
}
fn beat_env(x: f32) -> f32 {
    // Fast attack, exponential release; continuous at the peak.
    return select(exp(-x / 0.085), exp(-(x / 0.018) * (x / 0.018)), x < 0.0);
}

// One ping shot at `age` seconds: (energy, white-hot share).
fn ping(c: Ctx, i: Inst, age: f32, dur: f32) -> vec2f {
    let o = i.aux.xy;
    let dir = normalize(i.aux.zw);
    let travel = i.p2.x;
    let head_r = i.p2.z;
    if (age < 0.0) { return vec2f(0.0); }
    let u = age / dur;
    if (c.reduced) {
        // No travel: the destination glows up and cools in place.
        let dest = o + dir * travel;
        let env = smoothstep(0.0, 0.2, age) * exp(-max(age - 0.2, 0.0) * 3.0);
        return vec2f(env * gauss(length(c.p - dest), head_r * 3.0), 0.0);
    }
    let s_head = shoot(u);
    let s_tail = shoot(u - i.p2.y);
    let hpos = o + dir * travel * s_head;
    let tpos = o + dir * travel * s_tail;
    let sd = segdist(c.p, tpos, hpos);
    // After arrival the head blooms out and cools (continuous at u = 1).
    let after = max(u - 1.0, 0.0);
    let cool = exp(-after * 3.5);
    let hr = head_r * (1.0 + 0.9 * (1.0 - cool));
    let tailw = head_r * 0.55 * (0.35 + 0.65 * sd.y);
    let tail = gauss(sd.x, tailw) * sd.y * sd.y * 0.8;
    let dh = length(c.p - hpos);
    let head = gauss(dh, hr) + 0.35 * bloom(dh - hr * 0.5, hr * 1.2, hr * 8.0);
    var e = (head + tail) * cool;
    let hot = gauss(dh, hr * 0.45) * cool * cool;
    // Launch flash at the origin.
    let fr = 26.0 * (1.0 - exp(-age * 7.0));
    e += exp(-age * 10.0) * smoothstep(0.0, 0.03, age) * gauss(length(c.p - o) - fr, 2.0 + age * 6.0) * 0.5;
    return vec2f(e, hot);
}

fn shade(in: VOut) -> vec4f {
    let i = insts[in.id];
    let c = ctx(i, in.px);
    let n = i32(i.p0.x + 0.5);
    let period = max(i.p0.y, 0.05);
    let reach = i.p0.z;
    let width = i.p0.w;
    let inten = i.p1.x;
    let occl = select(1.0, i.p2.w, effective_blend_mode(i) == 0u);
    let outside = smoothstep(-c.aa, c.aa, c.d);
    var e = 0.0;     // emitted energy (tint)
    var hotv = 0.0;  // white-hot share

    if (c.variant == 0) {
        if (c.reduced) {
            e = 0.35 * bloom(c.d, reach * 0.18, reach * 0.8);
        } else {
            for (var k = 0; k < n; k++) {
                let ph = fract(c.t / period + f32(k) / f32(n));
                let r = reach * (1.0 - pow(1.0 - ph, 2.2));
                let w = width * (1.0 + 2.2 * ph);
                let a = pow(1.0 - ph, 1.6) * smoothstep(0.0, 0.08, ph);
                let ring = gauss(c.d - r, w);
                // Faint wash trailing inside the ring front, like a radar sweep.
                let wash = smoothstep(r + w, r - w, c.d) * exp(-(r - c.d) / max(reach * 0.22, 1.0));
                e += a * (ring + 0.22 * wash);
            }
            e += 0.18 * bloom(c.d, reach * 0.1, reach * 0.6);
        }
        e *= outside;
    } else if (c.variant == 1) {
        var b = 0.5;
        if (!c.reduced) { b = 0.5 - 0.5 * cos(TAU * c.t / period); b = b * b * (3.0 - 2.0 * b); }
        let r = reach * (0.55 + 0.45 * b);
        e = (0.45 + 0.55 * b) * bloom(c.d, r, r * 5.0) * outside;
        hotv = 0.25 * b * gauss(max(c.d, 0.0), r * 0.25);
    } else if (c.variant == 2) {
        var beat = 0.3;
        var ring = 0.0;
        if (!c.reduced) {
            // Every beat is its own instance: the rings of earlier beats keep expanding and fading
            // under the new one instead of being cut when it starts.
            let tp = c.t / period;
            let kb = floor(tp);
            beat = 0.0;
            for (var k = -1; k < 3; k++) {
                let x = tp - (kb - f32(k));
                beat += beat_env(x) + 0.62 * beat_env(x - 0.17);
                let age = x * period; // seconds since that beat's lub
                if (age > 0.0) {
                    let rr = reach * (1.0 - exp(-age * 4.5));
                    // Finished rings fade out instead of piling up into a band at full reach.
                    ring += exp(-age * 3.2) * smoothstep(0.0, 0.03, age) * (1.0 - smoothstep(0.7, 1.3, age)) * gauss(c.d - rr, width * (1.0 + age * 3.0));
                }
            }
            // Fade the faint tail of old rings out before the quad edge.
            let edge = reach + width * 16.0;
            ring *= 1.0 - smoothstep(edge * 0.8, edge, c.d);
        }
        let r = reach * 0.22 * (1.0 + 0.35 * beat);
        e = ((0.25 + 0.75 * beat) * bloom(c.d, r, reach) + 0.55 * ring) * outside;
        hotv = 0.35 * beat * gauss(max(c.d, 0.0), r * 0.25) * outside;
    } else if (c.variant == 3) {
        let dur = max(i.p1.z, 0.05);
        let age0 = c.t - i.p1.y;
        // A repeating ping lets the previous shot finish blooming while the next one flies.
        var shots = 1;
        var age = age0;
        if (i.p1.w > 0.0 && age0 > 0.0) {
            age = age0 - floor(age0 / i.p1.w) * i.p1.w;
            shots = 3;
        }
        for (var k = 0; k < shots; k++) {
            let a = age + f32(k) * i.p1.w;
            // PING_LIFE in pack.rs: the frame loop ticks until the last shot ends here.
            if (a > age0 + 1e-4 || a > dur * 2.2) { break; }
            let pe = ping(c, i, a, dur);
            e += pe.x;
            hotv += pe.y;
        }
    } else {
        // Ripple from a tap point, confined to the shape with a soft edge.
        let dur = max(i.p1.z, 0.05);
        let age = c.t - i.p1.y;
        if (age >= 0.0 && age < dur) {
            let u = age / dur;
            let r = length(c.p - i.aux.xy);
            let inside = smoothstep(c.aa, -c.aa, c.d);
            if (c.reduced) {
                e = 0.06 * (1.0 - u) * smoothstep(0.0, 0.08, u) * inside;
            } else {
                let rr = reach * (1.0 - pow(1.0 - u, 3.0));
                let w = width * (1.0 + 3.0 * u);
                let fade = pow(1.0 - u, 1.5);
                let ring = gauss(r - rr, w) * fade * 0.55;
                let fill = smoothstep(rr + w, rr - w * 3.0, r) * 0.05 * (1.0 - u) * (1.0 - u) * smoothstep(0.0, 0.05, u);
                e = (ring * 0.7 + fill) * inside;
                hotv = ring * 0.1 * inside;
            }
        }
    }
    let col = i.c0.rgb * e + i.c1.rgb * hotv;
    return finish(vec4f(col * inten, (e + hotv) * occl * inten), in.px);
}
