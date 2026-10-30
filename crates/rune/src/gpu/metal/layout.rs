//! Bind group layouts, pipeline layouts and bind groups. A pipeline layout hands each stage's
//! buffers, textures and samplers slots counted up group by group, as wgpu-hal does: immediates
//! first, then every group's entries in binding order, then the sizes buffer when a stage sees
//! a storage buffer. A bind group records its resources per stage at slots relative to its
//! group's first, so binding it is adding the layout's base.

use std::sync::Arc;

use objc2_metal::{MTLBuffer, MTLSamplerState, MTLTexture};
use wgpu::custom::*;
use wgpu::naga::{self, back::msl};

use super::resource::{MetalBuffer, MetalSampler, MetalView};
use super::{Obj, send_sync};

/// The stages a render pipeline binds resources for: vertex, then fragment.
pub(super) const STAGES: [wgpu::ShaderStages; 2] = [wgpu::ShaderStages::VERTEX, wgpu::ShaderStages::FRAGMENT];

/// Slots of each kind a stage has handed out.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Counts {
    pub(super) buffers: u8,
    pub(super) textures: u8,
    pub(super) samplers: u8,
}

#[derive(Debug)]
pub(crate) struct BindGroupLayout {
    /// Sorted by binding.
    entries: Arc<[wgpu::BindGroupLayoutEntry]>,
}

#[derive(Debug)]
pub(crate) struct PipelineLayout {
    pub(super) raw: Arc<LayoutRaw>,
}

#[derive(Debug)]
pub(crate) struct LayoutRaw {
    /// Each group's first slots, per stage.
    pub(super) bases: Vec<[Counts; 2]>,
    pub(super) stages: [msl::EntryPointResources; 2],
    pub(super) immediates: u32,
}

#[derive(Debug)]
pub(crate) struct BindGroup {
    pub(super) raw: Arc<GroupRaw>,
}

#[derive(Debug, Default)]
pub(crate) struct GroupRaw {
    pub(super) stages: [StageBinds; 2],
    /// Storage buffers' bound lengths by binding, which the sizes buffer reports.
    pub(super) sizes: Vec<(u32, u32)>,
}

#[derive(Debug, Default)]
pub(super) struct StageBinds {
    pub(super) buffers: Vec<BufferBind>,
    pub(super) textures: Vec<(u8, Obj<dyn MTLTexture>)>,
    pub(super) samplers: Vec<(u8, Obj<dyn MTLSamplerState>)>,
}

#[derive(Debug)]
pub(super) struct BufferBind {
    pub(super) slot: u8,
    pub(super) raw: Obj<dyn MTLBuffer>,
    pub(super) offset: u64,
    /// Its index among the group's dynamic offsets.
    pub(super) dynamic: Option<usize>,
}

send_sync!(GroupRaw);

/// The bind target an entry takes in a stage, counting its slot.
fn target(ty: &wgpu::BindingType, counts: &mut Counts) -> msl::BindTarget {
    let mut target = msl::BindTarget::default();
    match *ty {
        wgpu::BindingType::Buffer { ty, .. } => {
            target.buffer = Some(counts.buffers);
            target.mutable = matches!(ty, wgpu::BufferBindingType::Storage { read_only: false });
            counts.buffers += 1;
        }
        wgpu::BindingType::Sampler(_) => {
            target.sampler = Some(msl::BindSamplerTarget::Resource(counts.samplers));
            counts.samplers += 1;
        }
        wgpu::BindingType::Texture { .. } => {
            target.texture = Some(counts.textures);
            counts.textures += 1;
        }
        wgpu::BindingType::StorageTexture { access, .. } => {
            target.texture = Some(counts.textures);
            target.mutable = access != wgpu::StorageTextureAccess::ReadOnly;
            counts.textures += 1;
        }
        ref other => unimplemented!("RUNE_GPU_BACKEND=metal: binding {other:?}"),
    }
    target
}

impl BindGroupLayout {
    pub(crate) fn new(desc: &wgpu::BindGroupLayoutDescriptor<'_>) -> Self {
        let mut entries = desc.entries.to_vec();
        entries.sort_by_key(|e| e.binding);
        assert!(entries.iter().all(|e| e.count.is_none()), "RUNE_GPU_BACKEND=metal: binding arrays");
        BindGroupLayout { entries: entries.into() }
    }
}

impl BindGroupLayoutInterface for BindGroupLayout {}

impl PipelineLayout {
    pub(crate) fn new(desc: &wgpu::PipelineLayoutDescriptor<'_>) -> Self {
        let mut counts = [Counts::default(); 2];
        let mut stages: [msl::EntryPointResources; 2] = Default::default();
        let mut storage = [false; 2];
        if desc.immediate_size > 0 {
            for (s, resources) in stages.iter_mut().enumerate() {
                resources.immediates_buffer = Some(counts[s].buffers);
                counts[s].buffers += 1;
            }
        }
        let mut bases = Vec::new();
        for (group, layout) in desc.bind_group_layouts.iter().enumerate() {
            bases.push(counts);
            let Some(layout) = layout else { continue };
            let layout = layout.as_custom::<BindGroupLayout>().expect("a Metal bind group layout");
            for entry in layout.entries.iter() {
                let binding = naga::ResourceBinding { group: group as u32, binding: entry.binding };
                for (s, stage) in STAGES.iter().enumerate() {
                    if !entry.visibility.contains(*stage) { continue }
                    storage[s] |= matches!(entry.ty, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { .. }, .. });
                    stages[s].resources.insert(binding, target(&entry.ty, &mut counts[s]));
                }
            }
        }
        for s in 0..2 {
            if storage[s] { stages[s].sizes_buffer = Some(counts[s].buffers); }
        }
        PipelineLayout { raw: Arc::new(LayoutRaw { bases, stages, immediates: desc.immediate_size }) }
    }
}

impl PipelineLayoutInterface for PipelineLayout {}

impl BindGroup {
    pub(crate) fn new(desc: &wgpu::BindGroupDescriptor<'_>) -> Self {
        let layout = desc.layout.as_custom::<BindGroupLayout>().expect("a Metal bind group layout");
        let mut group = GroupRaw::default();
        let mut counts = [Counts::default(); 2];
        let mut dynamic = 0;
        for entry in layout.entries.iter() {
            let resource = &desc.entries.iter().find(|e| e.binding == entry.binding).expect("a bind group entry for every layout entry").resource;
            let dynamic_index = match entry.ty {
                wgpu::BindingType::Buffer { has_dynamic_offset: true, .. } => { dynamic += 1; Some(dynamic - 1) }
                _ => None,
            };
            if let (wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { .. }, .. }, wgpu::BindingResource::Buffer(b)) = (&entry.ty, resource) {
                let size = b.size.map_or_else(|| b.buffer.size() - b.offset, |s| s.get());
                group.sizes.push((entry.binding, u32::try_from(size).unwrap_or(u32::MAX)));
            }
            for (s, stage) in STAGES.iter().enumerate() {
                if !entry.visibility.contains(*stage) { continue }
                let slot = target(&entry.ty, &mut counts[s]);
                let binds = &mut group.stages[s];
                match resource {
                    wgpu::BindingResource::Buffer(b) => {
                        let raw = b.buffer.as_custom::<MetalBuffer>().expect("a Metal buffer").raw.raw.clone();
                        binds.buffers.push(BufferBind { slot: slot.buffer.unwrap(), raw, offset: b.offset, dynamic: dynamic_index });
                    }
                    wgpu::BindingResource::TextureView(v) => {
                        binds.textures.push((slot.texture.unwrap(), v.as_custom::<MetalView>().expect("a Metal texture view").raw.raw.clone()));
                    }
                    wgpu::BindingResource::Sampler(sampler) => {
                        let Some(msl::BindSamplerTarget::Resource(at)) = slot.sampler else { unreachable!() };
                        binds.samplers.push((at, sampler.as_custom::<MetalSampler>().expect("a Metal sampler").raw.0.clone()));
                    }
                    other => unimplemented!("RUNE_GPU_BACKEND=metal: bind group resource {other:?}"),
                }
            }
        }
        BindGroup { raw: Arc::new(group) }
    }
}

impl BindGroupInterface for BindGroup {}
