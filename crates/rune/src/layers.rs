//! Bounded painter scopes. The ordinary path retains its instance buffer, batches and pass;
//! only scopes requiring isolation acquire a target. Scope records are also compositor fences.
use super::*;
use std::ops::Range;

// Linear premultiplied intermediates preserve HDR colours and avoid accumulating 8-bit
// quantization through nested layers. The destination format is applied only at composition.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

#[cfg(test)]
#[path = "layer_tests.rs"]
mod tests;

#[derive(Default)]
pub(super) struct Layers {
    plan: Vec<Layer>,
    pub targets: Vec<Option<Target>>,
    stack: Vec<usize>,
    pipelines: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    viewport: (u32, u32),
    image_generation: u64,
    allocations: u64,
}

struct Layer {
    begin: u32,
    end: u32,
    parent: Option<usize>,
    bounds: [f32; 4], // edges while planning, xywh after preparation
    clip: [f32; 4],
    opacity: f32,
}

pub(super) struct Target {
    identity: u64,
    composite_parent: Option<u64>,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    projection: wgpu::Buffer,
    quad: wgpu::Buffer,
    composite: wgpu::BindGroup,
    pub base: wgpu::BindGroup,
    pub images: Vec<Option<wgpu::BindGroup>>,
    image_generation: u64,
}

/// Resource/pass counters for diagnostics. The ordinary path creates no layer resources.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LayerStats {
    pub targets: usize,
    pub bytes: u64,
    pub extra_passes: usize,
    pub allocations: u64,
}

fn union(a: &mut [f32; 4], b: [f32; 4]) {
    if b[2] <= b[0] || b[3] <= b[1] { return; }
    a[0] = a[0].min(b[0]); a[1] = a[1].min(b[1]);
    a[2] = a[2].max(b[2]); a[3] = a[3].max(b[3]);
}

impl Renderer {
    pub fn layer_stats(&self) -> LayerStats {
        let mut stats = LayerStats { allocations: self.layers.allocations, ..Default::default() };
        for t in self.layers.targets.iter().flatten() {
            stats.targets += 1;
            stats.bytes += u64::from(t.texture.width()) * u64::from(t.texture.height())
                * u64::from(FORMAT.block_copy_size(None).unwrap());
        }
        stats.extra_passes = stats.targets;
        stats
    }

    pub(super) fn prepare_layers(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, instances: &[Instance], viewport: (u32, u32)) {
        self.layers.viewport = viewport;
        let had_layers = !self.layers.plan.is_empty();
        self.layers.plan.clear();
        self.layers.stack.clear();
        for (index, i) in instances.iter().enumerate() {
            if i.params[1] == KIND_LAYER {
                let parent = self.layers.stack.last().copied();
                let hidden = parent.is_some_and(|p| self.layers.plan[p].opacity == 0.0);
                let clip = parent.map_or(i.clip, |p| {
                    let c = self.layers.plan[p].clip;
                    [i.clip[0].max(c[0]), i.clip[1].max(c[1]), i.clip[2].min(c[2]), i.clip[3].min(c[3])]
                });
                self.layers.stack.push(self.layers.plan.len());
                self.layers.plan.push(Layer {
                    begin: index as u32, end: 0, parent,
                    bounds: [i.rect[0], i.rect[1], i.rect[0] + i.rect[2], i.rect[1] + i.rect[3]],
                    clip,
                    opacity: if hidden { 0.0 } else { i.fill[3].clamp(0.0, 1.0) },
                });
            } else if i.params[1] == KIND_LAYER_END {
                let at = self.layers.stack.pop().expect("unbalanced layer end");
                let l = &mut self.layers.plan[at];
                l.end = index as u32;
                l.bounds = [l.bounds[0].max(l.clip[0]).max(0.0).floor(),
                    l.bounds[1].max(l.clip[1]).max(0.0).floor(),
                    l.bounds[2].min(l.clip[2]).min(viewport.0 as f32).ceil(),
                    l.bounds[3].min(l.clip[3]).min(viewport.1 as f32).ceil()];
                let bounds = l.bounds;
                if let Some(parent) = l.parent { union(&mut self.layers.plan[parent].bounds, bounds); }
            } else if let Some(&at) = self.layers.stack.last() {
                let mut rect = i.rect;
                if i.params[1] == KIND_GLYPH && i.params[0] != 0.0 {
                    let (sin, cos) = i.params[0].sin_cos();
                    let width = cos.abs() * rect[2] + sin.abs() * rect[3];
                    let height = sin.abs() * rect[2] + cos.abs() * rect[3];
                    rect = [rect[0] + (rect[2] - width) * 0.5, rect[1] + (rect[3] - height) * 0.5, width, height];
                }
                let grow = match i.params[1] {
                    KIND_GLYPH => 0.0,
                    KIND_SHADOW => i.params[2] * 3.0 + 1.0,
                    KIND_RECT | KIND_GRADIENT => 1.0 + i.params[2] * 2.0,
                    _ => 1.0,
                };
                union(&mut self.layers.plan[at].bounds, [
                    (rect[0] - grow).max(i.clip[0]), (rect[1] - grow).max(i.clip[1]),
                    (rect[0] + rect[2] + grow).min(i.clip[2]),
                    (rect[1] + rect[3] + grow).min(i.clip[3]),
                ]);
            }
        }
        assert!(self.layers.stack.is_empty(), "unclosed layer scope");
        self.layers.targets.resize_with(self.layers.plan.len(), || None);
        for index in 0..self.layers.plan.len() {
            let l = &mut self.layers.plan[index];
            l.bounds[2] = (l.bounds[2] - l.bounds[0]).max(0.0);
            l.bounds[3] = (l.bounds[3] - l.bounds[1]).max(0.0);
            if l.bounds[2] == 0.0 || l.bounds[3] == 0.0 { l.opacity = 0.0; }
            if l.opacity == 0.0 || l.opacity == 1.0 {
                self.layers.targets[index] = None;
                continue;
            }
            let bounds = l.bounds;
            let (width, height) = (bounds[2] as u32, bounds[3] as u32);
            let bucket = (width.div_ceil(64) * 64, height.div_ceil(64) * 64);
            if self.layers.targets[index].as_ref().is_none_or(|t| {
                let size = (t.texture.width(), t.texture.height());
                // The minimum bucket is unavoidable for tiny controls. Do not recreate that
                // same bucket every frame when rounding alone exceeds the 2x area limit.
                size.0 < width || size.1 < height || (size != bucket
                    && u64::from(size.0) * u64::from(size.1) > 2 * u64::from(width) * u64::from(height))
            }) {
                let (width, height) = bucket;
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("isolated group"), size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                    mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
                    format: FORMAT, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                });
                let view = texture.create_view(&Default::default());
                let projection = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("layer projection"), size: 16, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let base = Self::bind_group(device, &self.bind_layout, &self.globals, &self.atlas, &self.atlas, Some(&projection));
                let composite = Self::bind_group(device, &self.bind_layout, &self.globals, &self.atlas, &texture, None);
                self.layers.targets[index] = Some(Target { texture, view, projection,
                    identity: self.layers.allocations + 1, composite_parent: None,
                    quad: Self::instance_buffer(device, 1), base, composite, images: Vec::new(), image_generation: u64::MAX });
                self.layers.allocations += 1;
            }
            let t = self.layers.targets[index].as_ref().unwrap();
            queue.write_buffer(&t.projection, 0, bytemuck::cast_slice(&bounds));
            let quad = Instance::begin_layer(bounds, NO_CLIP, LayerSpec { opacity: self.layers.plan[index].opacity });
            queue.write_buffer(&t.quad, 0, bytemuck::bytes_of(&quad));
        }
        // Rebind child composites to their nearest materialized parent (opaque scopes flatten).
        for index in 0..self.layers.plan.len() {
            if self.layers.targets[index].is_none() { continue; }
            let parent = self.layer_target(self.layers.plan[index].parent);
            let t = self.layers.targets[index].as_ref().unwrap();
            let parent_identity = parent.map(|p| self.layers.targets[p].as_ref().unwrap().identity);
            if t.composite_parent == parent_identity { continue; }
            let composite = Self::bind_group(device, &self.bind_layout, &self.globals, &self.atlas, &t.texture,
                parent.map(|p| &self.layers.targets[p].as_ref().unwrap().projection));
            let t = self.layers.targets[index].as_mut().unwrap();
            t.composite = composite;
            t.composite_parent = parent_identity;
        }
        if self.layers.targets.iter().any(Option::is_some) {
            self.add_format(device, FORMAT);
            self.add_layer_format(device, self.format);
            for index in 0..self.extra.len() { self.add_layer_format(device, self.extra[index].0); }
        }
        self.refresh_layer_bindings(device);
        if had_layers || !self.layers.plan.is_empty() { self.position_layer_surfaces(queue); }
    }

    pub(super) fn add_layer_format(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        if !self.layers.pipelines.iter().any(|(f, _)| *f == format) {
            self.layers.pipelines.push((format, Self::pipeline(device, &self.layout, &self.shader, format, "vs_layer")));
        }
    }

    fn layer_target(&self, mut layer: Option<usize>) -> Option<usize> {
        while let Some(index) = layer {
            if self.layers.targets[index].is_some() { return Some(index); }
            layer = self.layers.plan[index].parent;
        }
        None
    }

    pub(super) fn images_changed(&mut self) { self.layers.image_generation += 1; }

    pub(super) fn refresh_layer_bindings(&mut self, device: &wgpu::Device) {
        for t in self.layers.targets.iter_mut().flatten() {
            if t.image_generation == self.layers.image_generation { continue; }
            t.images.clear();
            t.images.extend(self.images.iter().map(|page| page.as_ref().map(|(texture, _)|
                Self::bind_group(device, &self.bind_layout, &self.globals, &self.atlas, texture, Some(&t.projection)))));
            t.image_generation = self.layers.image_generation;
        }
    }

    pub(super) fn position_layer_surfaces(&self, queue: &wgpu::Queue) {
        let Some(s) = &self.surfaces else { return };
        for index in 0..s.len() {
            let before = s.before_instance(index);
            let layer = self.layers.plan.iter().rposition(|l| before > l.begin && before <= l.end);
            let bounds = self.layer_target(layer).map_or(
                [0.0, 0.0, self.layers.viewport.0 as f32, self.layers.viewport.1 as f32],
                |p| self.layers.plan[p].bounds);
            // prepare_surfaces may precede prepare on the first frame.
            if bounds[2] > 0.0 && bounds[3] > 0.0 { s.target(queue, index, bounds); }
        }
    }

    /// Encode isolated scopes before opening a compositor render pass that calls `draw_range`.
    /// `draw`/`draw_timed` do this automatically. Contents are redrawn each presentation so GPU
    /// springs and newly latched native surfaces stay live; textures, not content, are pooled.
    pub fn encode_layers(&self, encoder: &mut wgpu::CommandEncoder) {
        self.encode_layers_timed(encoder, None);
    }

    pub(super) fn encode_layers_timed(&self, encoder: &mut wgpu::CommandEncoder, timestamps: Option<&wgpu::RenderPassTimestampWrites<'_>>) {
        let mut first = true;
        for (index, l) in self.layers.plan.iter().enumerate().rev() {
            let Some(target) = &self.layers.targets[index] else { continue };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("isolated group"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view, depth_slice: None, resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })], depth_stencil_attachment: None,
                timestamp_writes: if first { timestamps.map(|t| wgpu::RenderPassTimestampWrites {
                    query_set: t.query_set, beginning_of_pass_write_index: t.beginning_of_pass_write_index,
                    end_of_pass_write_index: None,
                }) } else { None },
                occlusion_query_set: None, multiview_mask: None,
            });
            first = false;
            pass.set_viewport(0.0, 0.0, l.bounds[2], l.bounds[3], 0.0, 1.0);
            self.draw_layer_range(&mut pass, FORMAT, l.begin + 1..l.end, Some(index));
        }
    }

    pub(super) fn draw_layer_range(&self, pass: &mut wgpu::RenderPass<'_>, format: wgpu::TextureFormat, range: Range<u32>, target: Option<usize>) {
        let mut cursor = range.start;
        for (index, l) in self.layers.plan.iter().enumerate() {
            if l.begin < cursor || l.begin >= range.end { continue; }
            assert!(l.end < range.end, "draw_range must not split a layer scope");
            self.draw_plain(pass, format, cursor..l.begin, target, true);
            if let Some(t) = &self.layers.targets[index] {
                let pipeline = &self.layers.pipelines.iter().find(|(f, _)| *f == format).expect("Renderer::add_format first").1;
                pass.set_pipeline(pipeline);
                if let Some(properties) = &self.springs { pass.set_bind_group(1, &properties.group, &[]); }
                pass.set_bind_group(0, &t.composite, &[]);
                pass.set_vertex_buffer(0, t.quad.slice(..));
                pass.draw(0..4, 0..1);
            } else if l.opacity == 1.0 {
                self.draw_layer_range(pass, format, l.begin + 1..l.end, target);
            }
            cursor = l.end + 1;
        }
        let include_end = range.end == self.count || self.layers.plan.iter().any(|l| l.end == range.end);
        self.draw_plain(pass, format, cursor..range.end, target, include_end);
    }

    pub(super) fn validate_layer_range(&self, range: &Range<u32>) {
        for l in &self.layers.plan {
            assert!(!(l.begin < range.start && range.start <= l.end)
                && !(l.begin < range.end && range.end <= l.end), "draw_range must not split a layer scope");
        }
    }

    fn draw_plain(&self, pass: &mut wgpu::RenderPass<'_>, format: wgpu::TextureFormat, range: Range<u32>, target: Option<usize>, include_end: bool) {
        let mut cursor = range.start;
        if let Some(s) = &self.surfaces {
            for index in 0..s.len() {
                let before = s.before_instance(index);
                if before < range.start || before > range.end || (before == range.end && !include_end) { continue; }
                self.draw_instances(pass, format, cursor..before, target);
                s.draw_format(pass, index, format);
                cursor = before;
            }
        }
        self.draw_instances(pass, format, cursor..range.end, target);
    }
}
