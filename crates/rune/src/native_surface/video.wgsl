struct Uniforms {
    viewport: vec4<f32>, node: vec4<f32>, image: vec4<f32>, radii: vec4<f32>,
    clip: vec4<f32>, fade: vec4<f32>, color: vec4<f32>, sampling: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var plane0: texture_2d<f32>;
@group(0) @binding(2) var plane1: texture_2d<f32>;
@group(0) @binding(3) var filtering: sampler;
struct Out { @builtin(position) pos: vec4<f32> };
@vertex fn vs(@builtin(vertex_index) vi: u32) -> Out {
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    let p = u.node.xy + corner * u.node.zw;
    let local = p - u.viewport.zw;
    return Out(vec4<f32>(local.x / u.viewport.x * 2.0 - 1.0, 1.0 - local.y / u.viewport.y * 2.0, 0.0, 1.0));
}
fn linear(v: vec3<f32>) -> vec3<f32> {
    let x = clamp(v, vec3<f32>(0.0), vec3<f32>(1.0));
    if u.color.z < 0.5 { return select(pow((x + 0.055) / 1.055, vec3<f32>(2.4)), x / 12.92, x <= vec3<f32>(0.04045)); }
    return select(pow((x + 0.099) / 1.099, vec3<f32>(1.0 / 0.45)), x / 4.5, x < vec3<f32>(0.081));
}
fn convert(a: vec4<f32>, uv: vec2<f32>) -> vec4<f32> {
    if u.color.x < 0.5 { return vec4<f32>(linear(a.rgb) * a.a, a.a); }
    var y = a.r;
    var c = uv - vec2<f32>(128.0 / 255.0);
    if u.color.x < 1.5 { y = (y - 16.0 / 255.0) * (255.0 / 219.0); c *= 255.0 / 224.0; }
    var kr = 0.299; var kb = 0.114;
    if u.color.y > 0.5 { kr = 0.2126; kb = 0.0722; }
    let r = y + 2.0 * (1.0 - kr) * c.y;
    let b = y + 2.0 * (1.0 - kb) * c.x;
    let g = (y - kr * r - kb * b) / (1.0 - kr - kb);
    return vec4<f32>(linear(vec3<f32>(r,g,b)), 1.0);
}
fn sample_linear(uv: vec2<f32>) -> vec4<f32> {
    return convert(textureSampleLevel(plane0, filtering, uv, 0.0), textureSampleLevel(plane1, filtering, uv, 0.0).rg);
}
fn ramp(d: f32, width: f32) -> f32 {
    if width <= 0.0 { return clamp(d + 0.5, 0.0, 1.0); }
    let x = clamp(d / width, 0.0, 1.0); return x*x*(3.0-2.0*x);
}
@fragment fn fs(i: Out) -> @location(0) vec4<f32> {
    let p = i.pos.xy + u.viewport.zw;
    let uv = (p-u.image.xy) / u.image.zw;
    if any(uv < vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0)) { discard; }
    var col: vec4<f32>;
    if u.sampling.x > 0.5 {
        // Integer device-pixel origin + pixel-centre positions: no interpolation, even at Retina 2x.
        let texel = vec2<i32>(floor(p - u.image.xy));
        col = convert(textureLoad(plane0, texel, 0), textureLoad(plane1, texel / 2, 0).rg);
    } else {
        let ratio = u.sampling.yz / u.image.zw;
        // Bounded area supersampling for large shrink ratios (up to 8x8 bilinear taps).
        // Convert each sample before accumulating so filtering is in linear light.
        let taps = vec2<i32>(clamp(ceil(ratio), vec2<f32>(1.0), vec2<f32>(8.0)));
        col = vec4<f32>(0.0);
        for (var y = 0; y < taps.y; y++) { for (var x = 0; x < taps.x; x++) {
            let offset = (vec2<f32>(f32(x)+0.5, f32(y)+0.5) / vec2<f32>(taps) - 0.5) / u.image.zw;
            col += sample_linear(uv + offset);
        }}
        col /= f32(taps.x * taps.y);
    }
    let half = u.node.zw*0.5; let rel = p-u.node.xy-half;
    var radius = select(u.radii.x, u.radii.y, rel.x > 0.0);
    if rel.y > 0.0 { radius = select(u.radii.w, u.radii.z, rel.x > 0.0); }
    radius = min(radius, min(half.x, half.y));
    let q = abs(rel)-half+radius;
    let distance = min(max(q.x,q.y),0.0)+length(max(q,vec2<f32>(0.0)))-radius;
    let clip = ramp(p.x-u.clip.x,u.fade.x)*ramp(p.y-u.clip.y,u.fade.y)*ramp(u.clip.z-p.x,u.fade.z)*ramp(u.clip.w-p.y,u.fade.w);
    return col * (clamp(0.5-distance,0.0,1.0)*clip*u.color.w);
}
