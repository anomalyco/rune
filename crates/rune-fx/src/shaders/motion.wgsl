// Motion blur of a node layer (`source` = the node rendered at rest, covering aux2 with a transparent margin).
// xf (first, count, mode, _). mode 0 = shutter: `count` exact sub-frame poses, each two vec4s:
//   (ia, ib, ic, id) inverse linear part, (itx, ity, weight*opacity, _): local = M^-1 * screen.
// mode 1 = directional: pose 0 is the current pose; aux.xy is the blur vector (pt, whole shutter);
//   `count` taps along it with a smooth window.

fn src_at(i: Inst, l: vec2f) -> vec4f {
    let uv = (l - i.aux2.xy) / i.aux2.zw;
    return textureSampleLevel(source, samp, uv, 0.0);
}
fn pose(base: u32, k: u32, p: vec2f) -> vec3f {
    let m = data[base + k * 2u];
    let tw = data[base + k * 2u + 1u];
    return vec3f(m.x * p.x + m.z * p.y + tw.x, m.y * p.x + m.w * p.y + tw.y, tw.z);
}

fn shade(i: Inst, px: vec2f) -> vec4f {
    let p = px / i.shape.y;
    let first = u32(i.xf.x);
    let n = u32(i.xf.y);
    var acc = vec4f(0.0);
    if (i.xf.z < 0.5) {
        for (var k = 0u; k < n; k++) {
            let l = pose(first, k, p);
            acc += src_at(i, l.xy) * l.z;
        }
    } else {
        // Taps are <= ~2 px apart at typical speeds; a fixed (un-jittered) set avoids the hatching
        // that per-pixel jitter leaves on edges.
        let j = 0.5;
        let b = i.aux.xy;
        var wsum = 0.0;
        for (var k = 0u; k < n; k++) {
            let o = (f32(k) + j) / f32(n) - 0.5;
            let w = 1.0 - 4.0 * o * o * 0.85;
            let l = pose(first, 0u, p - b * o);
            acc += src_at(i, l.xy) * (w * l.z);
            wsum += w;
        }
        acc /= max(wsum, 1e-4);
    }
    return finish(acc, px);
}
