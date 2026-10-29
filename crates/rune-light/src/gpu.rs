//! Small wgpu helpers shared by the light and glass renderers.

const COMMON: &str = include_str!("shaders/common.wgsl");

pub(crate) fn module(device: &wgpu::Device, label: &str, src: &str) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(format!("{COMMON}\n{src}").into()),
    })
}

pub(crate) fn gi_section(name: &str) -> String {
    let src = include_str!("shaders/gi.wgsl");
    let mut parts = src.split("// ---- ");
    let header = parts.next().unwrap_or_default();
    let body = parts.find(|p| p.starts_with(name)).unwrap_or_default();
    format!("{header}\n// ---- {body}")
}

pub(crate) mod bgl {
    pub fn uniform(binding: u32, vis: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
        wgpu::BindGroupLayoutEntry {
            binding,
            visibility: vis,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        }
    }
    pub fn storage(binding: u32, vis: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
        wgpu::BindGroupLayoutEntry {
            binding,
            visibility: vis,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }
    }
    pub fn tex(binding: u32, vis: wgpu::ShaderStages, kind: wgpu::TextureSampleType, dim: wgpu::TextureViewDimension) -> wgpu::BindGroupLayoutEntry {
        wgpu::BindGroupLayoutEntry {
            binding,
            visibility: vis,
            ty: wgpu::BindingType::Texture { sample_type: kind, view_dimension: dim, multisampled: false },
            count: None,
        }
    }
    pub fn f2(binding: u32, vis: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
        tex(binding, vis, wgpu::TextureSampleType::Float { filterable: true }, wgpu::TextureViewDimension::D2)
    }
    pub fn f2a(binding: u32, vis: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
        tex(binding, vis, wgpu::TextureSampleType::Float { filterable: true }, wgpu::TextureViewDimension::D2Array)
    }
    pub fn u2(binding: u32, vis: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
        tex(binding, vis, wgpu::TextureSampleType::Uint, wgpu::TextureViewDimension::D2)
    }
    pub fn store(binding: u32, dim: wgpu::TextureViewDimension) -> wgpu::BindGroupLayoutEntry {
        wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: wgpu::TextureFormat::Rgba16Float,
                view_dimension: dim,
            },
            count: None,
        }
    }
    pub fn sampler(binding: u32, vis: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
        wgpu::BindGroupLayoutEntry {
            binding,
            visibility: vis,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        }
    }
}

pub(crate) fn layout(device: &wgpu::Device, label: &str, entries: &[wgpu::BindGroupLayoutEntry]) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(label), entries })
}

pub(crate) fn compute(device: &wgpu::Device, label: &str, m: &wgpu::ShaderModule, entry: &str, bgls: &[&wgpu::BindGroupLayout]) -> wgpu::ComputePipeline {
    let bgls: Vec<Option<&wgpu::BindGroupLayout>> = bgls.iter().map(|b| Some(*b)).collect();
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(label), bind_group_layouts: &bgls, immediate_size: 0 });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: Some(&pl),
        module: m,
        entry_point: Some(entry),
        compilation_options: Default::default(),
        cache: None,
    })
}

pub(crate) fn texture(device: &wgpu::Device, label: &str, w: u32, h: u32, layers: u32, format: wgpu::TextureFormat, usage: wgpu::TextureUsages) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: layers },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

pub(crate) fn bind(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, entries: &[wgpu::BindingResource<'_>]) -> wgpu::BindGroup {
    let e: Vec<wgpu::BindGroupEntry> =
        entries.iter().enumerate().map(|(i, r)| wgpu::BindGroupEntry { binding: i as u32, resource: r.clone() }).collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout, entries: &e })
}

pub(crate) fn tv(v: &wgpu::TextureView) -> wgpu::BindingResource<'_> {
    wgpu::BindingResource::TextureView(v)
}
