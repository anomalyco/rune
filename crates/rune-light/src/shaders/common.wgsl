// Shared by every rune-light pass. All positions are output device pixels unless a name says otherwise.

struct Frame {
    sf: vec4<f32>,       // output size xy, field size zw
    gs: vec4<f32>,       // gi probe grid xy, field scale (field px per output px), gi scale
    ambient: vec4<f32>,  // rgb ambient irradiance, exposure
    counts: vec4<u32>,   // prims, lights, cascades, flags (1 gi, 2 shafts, 4 encode sRGB, 8 ao, 16 extra emission)
    misc: vec4<f32>,     // max occluder top z, time, seed, shafts over occluders
    text: vec4<f32>,     // floor, ceiling, how much light text takes, specular on text
    gi: vec4<f32>,       // strength, air glow, bounce, absorption per gi texel
    bg: vec4<f32>,       // background gloss, background roughness, contact AO strength, rim strength
    ext: vec4<f32>,      // albedo floor, extra emission gain, unused
};

struct Prim {
    rect: vec4<f32>,     // x, y, w, h
    radii: vec4<f32>,    // tl, tr, br, bl
    z: vec4<f32>,        // bottom, top, bevel width, bevel height
    mat: vec4<f32>,      // gloss, roughness, flags, ao reach
    emit: vec4<f32>,     // linear radiance rgb, unused
    albedo: vec4<f32>,   // rgb albedo for bounce, unused
};

struct Light {
    pos: vec4<f32>,      // x, y, z, source radius
    color: vec4<f32>,    // linear rgb × intensity, range
    axis: vec4<f32>,     // spot axis xyz (unit), half-angle in radians (0 = point light)
    extra: vec4<f32>,    // shafts, bulb glow, specular, softness multiplier
};

const PI: f32 = 3.14159265;
const TAU: f32 = 6.28318531;
// Intensity 1 gives irradiance 1 at this distance (device px) from a small source.
const REF: f32 = 200.0;
const F_OCC: u32 = 1u;
const F_EMIT: u32 = 2u;
const F_RAISED: u32 = 4u;
// Seen only by the GI tracer's distance (marks where extra emission may be); never a surface.
const F_HINT: u32 = 8u;

fn flags_of(p: Prim) -> u32 {
    return u32(p.mat.z + 0.5);
}

// Rune's rounded rect (crates/rune/src/shaders/clip.wgsl `sd_continuous` / `sd_rounded`):
// radii tl, tr, br, bl, y down; negative selects Apple's continuous corner.
const CONTINUOUS_K: f32 = 1.52866483;
fn sd_continuous(p: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    let e = CONTINUOUS_K * r;
    let q = half - abs(p);
    let v = vec2<f32>(e) - q;
    if (v.x <= 0.0 || v.y <= 0.0) { return max(-q.x, -q.y); }
    let u = atan2(v.y, v.x) * (4.0 / PI) - 1.0;
    let u2 = u * u;
    return length(v) - e * (1.14602295 + u2 * (-0.37573161 + u2 * (0.30796609 - 0.07724354 * u2)));
}

fn sd_rrect(p: vec2<f32>, half: vec2<f32>, radii: vec4<f32>) -> f32 {
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

fn prim_sd(pr: Prim, p: vec2<f32>) -> f32 {
    let half = pr.rect.zw * 0.5;
    return sd_rrect(p - (pr.rect.xy + half), half, pr.radii);
}

// Torch profile from Arcade (packages/kit/gpu/lights/beam.ts:71-83, Outbreak's flashlight), between its
// Darkwood and hot-core presets: soft cone, a hot core, faint reflector rings and a little spill.
fn beam_angle(c: f32) -> f32 {
    let omc = max(0.0, 1.0 - clamp(c, -1.0, 1.0));
    return sqrt(2.0 * omc) * (1.0 + 0.083333 * omc);
}
fn beam_bell(x: f32) -> f32 { return 1.0 / (1.0 + x * x * (1.0 + 0.35 * x * x)); }
fn beam_lorentz(x: f32) -> f32 { return 1.0 / (1.0 + x * x * (1.0 + 0.48 * x * x)); }
fn beam_profile(t: f32) -> f32 {
    let inner = 0.5;
    let core = 0.45;
    let core_w = 0.34;
    let spill = 0.035;
    let spill_w = 1.6;
    let rings = 0.18;
    if (t >= spill_w * 1.7) { return 0.0; }
    let ring = 1.0 + rings * (0.2 * beam_bell((t - 0.84) * 12.5) - 0.18 * beam_bell((t - core_w * 1.35) * 9.0));
    return (1.0 - smoothstep(inner, 1.0, t)) * ring + core * beam_lorentz(t / core_w) + spill * beam_lorentz(t / spill_w);
}

// Direction to the light and its unshadowed irradiance scale at P (before n·l): Karis' windowed inverse
// square with the source radius as the near clamp (Arcade packages/kit/gpu/lights/physical.ts:159-163).
fn light_term(L: Light, P: vec3<f32>) -> vec4<f32> {
    let D = L.pos.xyz - P;
    let d2 = dot(D, D);
    let d = sqrt(d2);
    let l = D / max(d, 1e-4);
    let x = d / max(L.color.w, 1.0);
    let w = clamp(1.0 - x * x * x * x, 0.0, 1.0);
    var a = REF * REF / (d2 + L.pos.w * L.pos.w) * w * w;
    if (L.axis.w > 0.0) {
        a = a * beam_profile(beam_angle(dot(-l, L.axis.xyz)) / L.axis.w);
    }
    return vec4<f32>(l, a);
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}
