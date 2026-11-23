//! Verifies every WGSL shader shipped by Rune (and `rune-media`'s `video.wgsl`) translates
//! cleanly through Naga's GLSL ES 3.00 WebGL2 backend without constructs WebGL2 rejects.
//!
//! Specifically, Naga 30.0.1 lowers any vector-condition WGSL `select(reject, accept, cond)`
//! to GLSL `mix(reject, accept, cond)`. GLSL ES 3.00 (WebGL2) defines boolean-`mix` only for
//! floating-point vectors (`vec2`/`vec3`/`vec4`); integer and boolean vector `mix` requires
//! GLSL ES 3.10 / `GL_EXT_shader_integer_mix` and fails shader compilation in Chromium WebGL2.

use naga::{
    Expression, Function, Module, ScalarKind, TypeInner,
    back::{glsl, pipeline_constants::process_overrides},
    front::wgsl,
    proc::{BoundsCheckPolicies, BoundsCheckPolicy},
    valid::{Capabilities, FunctionInfo, ModuleInfo, ValidationFlags, Validator},
};

use super::*;

fn check_webgl2_shader(label: &str, source: &str, constants: &[(&str, f64)]) -> Result<Vec<(String, String)>, String> {
    let module = wgsl::parse_str(source)
        .map_err(|e| format!("{label}: WGSL parse failed:\n{}", e.emit_to_string(source)))?;
    let info = Validator::new(ValidationFlags::all(), Capabilities::default())
        .validate(&module)
        .map_err(|e| format!("{label}: WGSL validation failed: {e}"))?;

    let overrides: naga::back::PipelineConstants =
        constants.iter().map(|&(k, v)| (k.to_owned(), v)).collect();
    let options = glsl::Options {
        version: glsl::Version::Embedded { version: 300, is_webgl: true },
        writer_flags: glsl::WriterFlags::ADJUST_COORDINATE_SPACE | glsl::WriterFlags::FORCE_POINT_SIZE,
        binding_map: Default::default(),
        zero_initialize_workgroup_memory: true,
    };
    let policies = BoundsCheckPolicies {
        index: BoundsCheckPolicy::Unchecked,
        buffer: BoundsCheckPolicy::Unchecked,
        image_load: BoundsCheckPolicy::Unchecked,
        binding_array: BoundsCheckPolicy::Unchecked,
    };

    let mut outputs = Vec::new();
    for ep in &module.entry_points {
        let (ep_module, ep_info) =
            process_overrides(&module, &info, Some((ep.stage, &ep.name)), &overrides)
                .map_err(|e| format!("{label} ({}): process_overrides failed: {e}", ep.name))?;

        check_no_non_float_vector_select(label, &ep.name, &ep_module, &ep_info)?;

        let pipeline_options = glsl::PipelineOptions {
            shader_stage: ep.stage,
            entry_point: ep.name.clone(),
            multiview: None,
        };
        let mut glsl_src = String::new();
        let mut writer = glsl::Writer::new(
            &mut glsl_src,
            &ep_module,
            &ep_info,
            &options,
            &pipeline_options,
            policies,
        )
        .map_err(|e| format!("{label} ({}): glsl::Writer::new failed: {e}", ep.name))?;
        writer
            .write()
            .map_err(|e| format!("{label} ({}): glsl::Writer::write failed: {e}", ep.name))?;

        if !glsl_src.starts_with("#version 300 es") {
            return Err(format!("{label} ({}): expected #version 300 es header", ep.name));
        }
        if let Some(line) = glsl_src.lines().find(|l| l.trim_start().starts_with("#extension")) {
            return Err(format!("{label} ({}): emitted GLSL extension unsupported in WebGL2: {line}", ep.name));
        }
        for bad in ["mix(uvec", "mix(ivec", "mix(bvec"] {
            if let Some(line) = glsl_src.lines().find(|l| l.contains(bad)) {
                return Err(format!("{label} ({}): emitted invalid GLSL ES 3.00 `{bad}`: {line}", ep.name));
            }
        }
        outputs.push((ep.name.clone(), glsl_src));
    }
    assert!(!outputs.is_empty(), "{label}: shader has no entry points");
    Ok(outputs)
}

fn check_no_non_float_vector_select(
    label: &str,
    entry_point: &str,
    module: &Module,
    info: &ModuleInfo,
) -> Result<(), String> {
    let inspect = |fn_name: &str, func: &Function, fn_info: &FunctionInfo| -> Result<(), String> {
        for (_, expr) in func.expressions.iter() {
            let Expression::Select { condition, accept, .. } = *expr else { continue };
            if !matches!(fn_info[condition].ty.inner_with(&module.types), TypeInner::Vector { .. }) {
                continue;
            }
            let accept_ty = fn_info[accept].ty.inner_with(&module.types);
            let is_float_vec = matches!(
                accept_ty,
                TypeInner::Vector { scalar, .. } if scalar.kind == ScalarKind::Float
            );
            if !is_float_vec {
                return Err(format!(
                    "{label} ({entry_point}): `{fn_name}` uses vector-condition `select` on {accept_ty:?}, \
                     which Naga lowers to GLSL `mix(...)` with non-float vectors (rejected in WebGL2 / GLSL ES 3.00)"
                ));
            }
        }
        Ok(())
    };

    for (handle, func) in module.functions.iter() {
        let name = func.name.as_deref().unwrap_or("<anon>");
        inspect(name, func, &info[handle])?;
    }
    for (idx, ep) in module.entry_points.iter().enumerate() {
        inspect(&ep.name, &ep.function, info.get_entry_point(idx))?;
    }
    Ok(())
}

#[test]
fn shipped_shaders_translate_to_valid_webgl2_glsl_es_300() {
    let composite = backdrop_blur::composite_wgsl();
    let video = include_str!("../../../rune-media/src/native_surface/video.wgsl");

    let mut cases: Vec<(String, String, Vec<(&str, f64)>)> = vec![
        ("uber_wgsl::CLASSIC".into(), uber_wgsl::CLASSIC.into(), Vec::new()),
        ("uber_wgsl::WINDFOIL".into(), uber_wgsl::WINDFOIL.into(), Vec::new()),
        ("backdrop_blur.wgsl".into(), backdrop_blur::BLUR_WGSL.into(), Vec::new()),
        ("backdrop_composite.wgsl".into(), composite, Vec::new()),
        ("surface_alpha.wgsl".into(), surface_alpha::WGSL.into(), Vec::new()),
        ("video.wgsl".into(), video.into(), Vec::new()),
    ];
    for &kind in &variant::PINNED {
        let src = variant::pinned_wgsl(kind).expect("pinned_wgsl");
        cases.push((format!("uber_pinned_k{kind}"), src, Vec::new()));
    }
    for (i, &(mode, additive)) in layer_blur::MODES.iter().enumerate() {
        cases.push((
            format!("layer_blur.wgsl[{i}]"),
            layer_blur::WGSL.into(),
            vec![("MODE", mode), ("ADDITIVE", additive)],
        ));
    }

    for (label, source, constants) in &cases {
        check_webgl2_shader(label, source, constants).unwrap_or_else(|err| panic!("{err}"));
    }
}

#[test]
fn webgl2_glsl_check_rejects_integer_vector_select() {
    let bad = r#"
@fragment
fn fs() -> @location(0) vec4<f32> {
    let a = vec2<u32>(1u, 2u);
    let b = vec2<u32>(3u, 4u);
    let c = select(a, b, a == vec2<u32>(1u));
    return vec4<f32>(f32(c.x), f32(c.y), 0.0, 1.0);
}
"#;
    let err = check_webgl2_shader("bad_uvec2_select", bad, &[])
        .expect_err("vector select on uvec2 must be rejected for WebGL2 GLSL ES 3.00");
    assert!(err.contains("vector-condition `select`"), "unexpected error: {err}");
}
