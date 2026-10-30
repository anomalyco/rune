//! Shader modules stay naga IR until a pipeline names an entry point; then naga translates it
//! to MSL with the pipeline layout's slots and Metal compiles it, as wgpu-hal does.

use std::future::ready;
use std::pin::Pin;
use std::sync::Arc;

use objc2_foundation_video::NSString;
use objc2_metal::{MTLCompileOptions, MTLDevice, MTLFunction, MTLLibrary};
use wgpu::custom::*;
use wgpu::naga::{
    self,
    back::{msl, pipeline_constants::process_overrides},
    proc::{BoundsCheckPolicies, BoundsCheckPolicy},
    valid::{Capabilities, ValidationFlags, Validator},
};

use super::{Obj, Shared};

#[derive(Debug)]
pub(crate) struct ShaderModule {
    module: Arc<naga::Module>,
    info: Arc<naga::valid::ModuleInfo>,
    checks: wgpu::ShaderRuntimeChecks,
}

/// An entry point compiled for one pipeline, and the storage bindings whose byte sizes it reads
/// from the sizes buffer, in the order it reads them.
pub(super) struct Compiled {
    pub(super) function: Obj<dyn MTLFunction>,
    pub(super) sized: Vec<naga::ResourceBinding>,
    /// Threads per workgroup, for a compute entry point.
    pub(super) workgroup: [u32; 3],
}

/// One stage of a pipeline, as `compile` needs it.
pub(super) struct Stage<'a> {
    pub(super) module: &'a wgpu::ShaderModule,
    pub(super) entry: Option<&'a str>,
    pub(super) constants: &'a [(&'a str, f64)],
    pub(super) kind: naga::ShaderStage,
}

impl ShaderModule {
    pub(crate) fn new(desc: wgpu::ShaderModuleDescriptor<'_>, checks: wgpu::ShaderRuntimeChecks) -> Self {
        let label = desc.label.unwrap_or("shader");
        let wgpu::ShaderSource::Wgsl(source) = desc.source else { unimplemented!("RUNE_GPU_BACKEND=metal: shaders other than WGSL") };
        let module = naga::front::wgsl::parse_str(&source).unwrap_or_else(|e| panic!("{label}: {}", e.emit_to_string(&source)));
        let info = Validator::new(ValidationFlags::all(), Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{label}: {}", e.emit_to_string(&source)));
        ShaderModule { module: Arc::new(module), info: Arc::new(info), checks }
    }
}

impl ShaderModuleInterface for ShaderModule {
    fn get_compilation_info(&self) -> Pin<Box<dyn ShaderCompilationInfoFuture>> {
        Box::pin(ready(wgpu::CompilationInfo { messages: Vec::new() }))
    }
}

pub(super) fn compile(shared: &Shared, stage: &Stage<'_>, resources: &msl::EntryPointResources, points: bool) -> Result<Compiled, String> {
    let shader = stage.module.as_custom::<ShaderModule>().expect("a Metal shader module");
    let name = match stage.entry {
        Some(name) => name.to_string(),
        None => {
            let mut eps = shader.module.entry_points.iter().filter(|e| e.stage == stage.kind);
            match (eps.next(), eps.next()) {
                (Some(ep), None) => ep.name.clone(),
                _ => return Err(format!("no single {:?} entry point", stage.kind)),
            }
        }
    };
    let constants: naga::back::PipelineConstants = stage.constants.iter().map(|&(k, v)| (k.to_string(), v)).collect();
    let (module, info) = process_overrides(&shader.module, &shader.info, Some((stage.kind, &name)), &constants)
        .map_err(|e| format!("{name}: overrides: {e:?}"))?;
    let policy = if shader.checks.bounds_checks { BoundsCheckPolicy::Restrict } else { BoundsCheckPolicy::Unchecked };
    let ((major, minor), version) = shared.msl;
    let options = msl::Options {
        lang_version: (major, minor),
        inline_samplers: Default::default(),
        spirv_cross_compatibility: false,
        fake_missing_bindings: false,
        per_entry_point_map: msl::EntryPointResourceMap::from([(name.clone(), resources.clone())]),
        bounds_check_policies: BoundsCheckPolicies { index: policy, buffer: policy, image_load: policy, binding_array: BoundsCheckPolicy::Unchecked },
        zero_initialize_workgroup_memory: true,
        force_loop_bounding: shader.checks.force_loop_bounding,
        task_dispatch_limits: None,
        mesh_shader_primitive_indices_clamp: shader.checks.mesh_shader_primitive_indices_clamp,
        emit_int_div_checks: shader.checks.int_div_checks,
        ray_query_initialization_tracking: shader.checks.ray_query_initialization_tracking,
    };
    let pipeline = msl::PipelineOptions {
        entry_point: Some((stage.kind, name.clone())),
        allow_and_force_point_size: points,
        vertex_pulling_transform: false,
        vertex_buffer_mappings: Vec::new(),
        binding_array_length_map: Default::default(),
    };
    let (source, translated) = msl::write_string(&module, &info, &options, &pipeline).map_err(|e| format!("{name}: MSL: {e:?}"))?;
    let compile = MTLCompileOptions::new();
    compile.setLanguageVersion(version);
    compile.setPreserveInvariance(true);
    let library = shared.device.newLibraryWithSource_options_error(&NSString::from_str(&source), Some(&compile))
        .map_err(|e| format!("{name}: Metal: {}", e.localizedDescription()))?;
    let translated_name = translated.entry_point_names[0].as_ref().map_err(|e| format!("{name}: {e}"))?;
    let function = library.newFunctionWithName(&NSString::from_str(translated_name)).ok_or_else(|| format!("{name}: no function {translated_name}"))?;
    // The sizes buffer holds one length per global that needs it, in global order, as naga reads it.
    let sized = module.global_variables.iter()
        .filter(|(_, g)| matches!(g.space, naga::AddressSpace::Uniform | naga::AddressSpace::Storage { .. }))
        .filter(|(_, g)| module.types[g.ty].inner.needs_host_buffer_byte_size(&module.types))
        .filter_map(|(_, g)| g.binding)
        .collect();
    let workgroup = module.entry_points.iter().find(|e| e.stage == stage.kind && e.name == name).map_or([1; 3], |e| e.workgroup_size);
    Ok(Compiled { function, sized, workgroup })
}
