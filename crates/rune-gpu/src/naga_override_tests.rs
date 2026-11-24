//! Regression tests for `naga::back::pipeline_constants::process_overrides` on valid WGSL shaders
//! whose `let`-bound composites or overrides constant-fold to earlier expression handles during
//! override resolution (`vendor/naga.patch`).

use wgpu::naga::{
    self,
    back::pipeline_constants::process_overrides,
    valid::{Capabilities, ValidationFlags, Validator},
};

fn resolve_overrides(wgsl: &str, stage: naga::ShaderStage, entry: &str, constants: &[(&str, f64)]) {
    let module = naga::front::wgsl::parse_str(wgsl)
        .unwrap_or_else(|e| panic!("WGSL parse failed:\n{}", e.emit_to_string(wgsl)));
    let info = Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .unwrap_or_else(|e| panic!("WGSL validation failed:\n{}", e.emit_to_string(wgsl)));
    let map: naga::back::PipelineConstants =
        constants.iter().map(|&(k, v)| (k.to_string(), v)).collect();
    let (resolved, resolved_info) = process_overrides(&module, &info, Some((stage, entry)), &map)
        .unwrap_or_else(|e| panic!("process_overrides failed for {entry} ({constants:?}): {e:?}"));
    // Re-validate the resolved module independently to confirm Emit ranges, scopes and
    // resolved overrides are valid.
    Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate_resolved_overrides(&resolved)
        .unwrap_or_else(|e| panic!("post-override validation failed: {}", e.emit_to_string(wgsl)));
    for (_, fun) in resolved.functions.iter() {
        assert!(
            !fun.expressions.iter().any(|(_, e)| matches!(e, naga::Expression::Override(_))),
            "unresolved Expression::Override left in function"
        );
    }
    for ep in &resolved.entry_points {
        assert!(
            !ep.function.expressions.iter().any(|(_, e)| matches!(e, naga::Expression::Override(_))),
            "unresolved Expression::Override left in entry point {}",
            ep.name
        );
    }
    let _ = resolved_info;
}

#[test]
fn reduced_compute_and_fragment_let_vector_overrides() {
    let compute = r#"
override o: u32 = 0;
@compute @workgroup_size(1) fn f() {
    var x = o;
    let v = vec2u(0);
    _ = v.x + x + v.x;
}
"#;
    resolve_overrides(compute, naga::ShaderStage::Compute, "f", &[]);
    resolve_overrides(compute, naga::ShaderStage::Compute, "f", &[("o", 7.0)]);

    let fragment = r#"
override o: f32 = 0.0;
@fragment fn f(@builtin(position) p: vec4f) -> @location(0) vec4f {
    let v = vec2f(0.0);
    return vec4f(v.x + p.x + v.x + o);
}
"#;
    resolve_overrides(fragment, naga::ShaderStage::Fragment, "f", &[]);
    resolve_overrides(fragment, naga::ShaderStage::Fragment, "f", &[("o", 1.5)]);

    let issue_74 = r#"
override linear_target: bool = false;
@fragment fn fs(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let er = vec2f(0.148, 0.208);
    let top = -er.y + (1.0 - pos.z) * 1.65 * er.y;
    return vec4f(vec2f(top), select(0.0, 1.0, linear_target), 1.0);
}
"#;
    resolve_overrides(issue_74, naga::ShaderStage::Fragment, "fs", &[]);
    resolve_overrides(issue_74, naga::ShaderStage::Fragment, "fs", &[("linear_target", 0.0)]);
    resolve_overrides(issue_74, naga::ShaderStage::Fragment, "fs", &[("linear_target", 1.0)]);
}

#[test]
fn folded_composite_and_inverted_emit_ranges() {
    // AccessIndex on a Compose of Splats folds to an already-emitted non-pre-emit Splat handle.
    let array_of_vecs = r#"
override k: f32 = 1.0;
@group(0) @binding(0) var<storage, read_write> o: array<vec4f>;
@compute @workgroup_size(1) fn main() {
    let a = array(vec2f(1.0), vec2f(2.0));
    o[0] = vec4f(a[0], k, k);
}
"#;
    resolve_overrides(array_of_vecs, naga::ShaderStage::Compute, "main", &[]);
    resolve_overrides(array_of_vecs, naga::ShaderStage::Compute, "main", &[("k", 2.5)]);

    // Indexing a literal-and-override vector without a let binding folds the trailing handle
    // backwards to an earlier literal.
    let inline_index = r#"
override k: f32 = 1.0;
@group(0) @binding(0) var<storage, read_write> o: array<vec4f>;
@compute @workgroup_size(1) fn main() {
    o[0] = vec4f(vec4f(1.0, k, 2.0, 3.0)[0]);
}
"#;
    resolve_overrides(inline_index, naga::ShaderStage::Compute, "main", &[]);
    resolve_overrides(inline_index, naga::ShaderStage::Compute, "main", &[("k", 4.0)]);

    // A statement whose entire Emit range folds to an earlier handle produces an empty range.
    let all_folded = r#"
override k: u32 = 1;
@group(0) @binding(0) var<storage, read_write> o: array<u32>;
@compute @workgroup_size(1) fn main() {
    let v = vec2u(3u, 5u);
    let a = v.x;
    let b = v.y;
    o[0] = a + b + k;
}
"#;
    resolve_overrides(all_folded, naga::ShaderStage::Compute, "main", &[]);
    resolve_overrides(all_folded, naga::ShaderStage::Compute, "main", &[("k", 9.0)]);
}

#[test]
fn overrides_across_loops_helper_calls_and_const_composites() {
    // Override used across multiple expressions, inside a loop, in a helper called twice,
    // and with both vectors and scalars plus a module-level const composite.
    let wgsl = r#"
override scale: f32 = 2.0;
override count: u32 = 4;
override flip: bool = false;
const BASE: vec2f = vec2f(0.25, 0.75);

fn helper(p: vec2f, bias: f32) -> vec2f {
    let offset = vec2f(0.5, 1.5);
    let picked = select(offset.x, offset.y, flip);
    let m = mat2x2f(BASE, vec2f(scale, bias));
    return m * p + vec2f(offset.x + picked + offset.y, bias * scale);
}

@fragment fn fs(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let weights = vec4f(0.1, 0.2, 0.3, 0.4);
    var acc = helper(pos.xy, weights.x + scale);
    for (var i: u32 = 0u; i < count; i++) {
        let step = vec2f(0.125);
        acc += vec2f(step.x + f32(i) * weights.y + step.y, weights.z * scale + step.x);
    }
    let final_v = helper(acc, weights.w);
    return vec4f(final_v, weights.x + weights.w, 1.0);
}
"#;
    resolve_overrides(wgsl, naga::ShaderStage::Fragment, "fs", &[]);
    resolve_overrides(
        wgsl,
        naga::ShaderStage::Fragment,
        "fs",
        &[("scale", 3.5), ("count", 2.0), ("flip", 1.0)],
    );
}

#[test]
fn rune_layer_blur_overrides_pass_across_all_modes() {
    let wgsl = include_str!("../../rune/src/shaders/layer_blur.wgsl");
    resolve_overrides(wgsl, naga::ShaderStage::Vertex, "vs", &[]);
    resolve_overrides(wgsl, naga::ShaderStage::Fragment, "fs", &[]);
    for &(mode, additive) in &[(0.0, 0.0), (0.0, 1.0), (2.0, 0.0), (2.0, 1.0), (3.0, 0.0)] {
        resolve_overrides(
            wgsl,
            naga::ShaderStage::Fragment,
            "fs",
            &[("MODE", mode), ("ADDITIVE", additive)],
        );
    }
}
