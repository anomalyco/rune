// Premultiplied to straight alpha (render/surface_alpha.rs). Precompiled for DX12 by regen_dxbc.
@group(0) @binding(0) var source: texture_2d<f32>;
@vertex fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let points = array(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    return vec4(points[index], 0.0, 1.0);
}
@fragment fn fs(@builtin(position) point: vec4<f32>) -> @location(0) vec4<f32> {
    let color = textureLoad(source, vec2<i32>(point.xy), 0);
    if color.a > 0.0 { return vec4(color.rgb / color.a, color.a); }
    return vec4(0.0);
}
