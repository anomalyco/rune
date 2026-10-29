//! Per-frame uploads: instances, globals, glyph and image atlas rows, and GPU spring nodes.
//!
//! Writes go through the renderer's `Staging`, so a steady frame allocates no buffers. They are
//! recorded into the frame's encoder where the renderer first uses it (`flush_uploads`, at the
//! start of `encode_layers*` and `draw_target_range_with`), or submitted ahead of a draw into a
//! caller's pass (`draw_range*`) and of `Renderer::submit`. Globals are written only when they
//! change, and instances only from the first to the last one that changed.

use std::sync::atomic::{AtomicU64, Ordering};

use rune_staging::Staging;

use super::*;

/// Upload state: the staging, and what the instance and globals buffers hold.
pub(super) struct Uploads {
    /// Made on the first upload, which brings the queue.
    staging: Option<Staging>,
    /// The instance buffer's contents, so a frame writes only the span that changed.
    instances: Vec<Instance>,
    /// The viewport last written to globals (`width << 32 | height`, `u64::MAX` before any).
    viewport: AtomicU64,
    /// The sRGB flag last written to globals.
    srgb: Option<bool>,
    /// `Staging::lost` as last seen by `prepare` and by `upload_images`.
    lost: (u64, u64),
    #[cfg(test)]
    pub(super) globals_writes: u64,
}

impl Default for Uploads {
    fn default() -> Uploads {
        Uploads { staging: None, instances: Vec::new(), viewport: AtomicU64::new(u64::MAX), srgb: None, lost: (0, 0),
            #[cfg(test)] globals_writes: 0 }
    }
}

impl Uploads {
    pub(super) fn staging(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Staging {
        self.staging.get_or_insert_with(|| Staging::new(device, queue)).clone()
    }

    #[cfg(test)]
    pub(super) fn stats(&self) -> rune_staging::Stats {
        self.staging.as_ref().map(Staging::stats).unwrap_or_default()
    }
}

fn pack((w, h): (u32, u32)) -> u64 { u64::from(w) << 32 | u64::from(h) }

impl Renderer {
    /// Records pending uploads into `encoder`, ahead of the renderer's commands in it.
    pub(super) fn flush_uploads(&self, encoder: &mut wgpu::CommandEncoder) {
        if let Some(s) = &self.uploads.staging { s.flush(encoder); }
    }

    /// Submits pending uploads ahead of commands already recorded.
    pub(crate) fn submit_uploads(&self) {
        if let Some(s) = &self.uploads.staging { s.submit(); }
    }

    /// `Queue::write_buffer` through the renderer's staging chunks, recorded into `encoder` here:
    /// ahead of the commands recorded after it (`Frame::write_buffer`).
    pub(crate) fn write_buffer(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder,
        buffer: &wgpu::Buffer, offset: u64, data: &[u8]) {
        let staging = self.uploads.staging(device, queue);
        staging.write_buffer(buffer, offset, data);
        staging.flush(encoder);
    }

    /// Writes rows `y0..y1` of `texture` from its tightly packed CPU copy `data`.
    fn write_rows(staging: &Staging, texture: &wgpu::Texture, data: &[u8], (y0, y1): (u32, u32)) {
        if y1 <= y0 { return; }
        let row = texture.width() * texture.format().block_copy_size(None).unwrap();
        staging.write_texture_rows(texture, y0, &data[(y0 * row) as usize..(y1 * row) as usize]);
    }

    /// Uploads changed rows of the runtime's image atlas (`El::pixels`), creating its texture on
    /// first use.
    pub fn upload_images(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, images: &mut ImageAtlas) {
        let staging = self.uploads.staging(device, queue);
        if staging.lost() != self.uploads.lost.1 {
            self.uploads.lost.1 = staging.lost();
            for page in &mut images.pages { page.dirty = Some((0, page.height)); }
        }
        let mut changed = self.images.len() != images.pages.len();
        self.images.resize_with(images.pages.len(), || None);
        for (index, page) in images.pages.iter_mut().enumerate() {
            if page.data.is_empty() { changed |= self.images[index].is_some(); self.images[index] = None; continue; }
            if self.images[index].as_ref().is_none_or(|(t, _)| t.width() != page.width || t.height() != page.height) {
                let texture = Self::image_texture(device, page.width, page.height);
                let bind = Self::bind_group(device, &self.bind_layout, &self.globals, &self.atlas, &texture, None);
                self.images[index] = Some((texture, bind));
                changed = true;
                page.dirty = Some((0, page.height));
            }
            let Some(rows) = page.dirty.take() else { continue };
            Self::write_rows(&staging, &self.images[index].as_ref().unwrap().0, &page.data, rows);
        }
        if changed { self.images_changed(); }
        self.refresh_layer_bindings(device);
    }

    /// Uploads several instance lists back to back (one buffer, one glyph upload) and returns
    /// each list's instance range for `draw_range`. A compositor's version of `prepare`.
    pub fn prepare_lists(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        atlas: &mut Atlas,
        lists: &[&[Instance]],
        viewport: (u32, u32),
    ) -> Vec<std::ops::Range<u32>> {
        let mut all = Vec::with_capacity(lists.iter().map(|l| l.len()).sum());
        let mut ranges = Vec::with_capacity(lists.len());
        for l in lists {
            let a = all.len() as u32;
            all.extend_from_slice(l);
            ranges.push(a..all.len() as u32);
        }
        self.prepare(device, queue, atlas, &all, viewport);
        ranges
    }

    pub(super) fn instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instances"),
            size: (capacity * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Uploads instances, globals and any new glyphs. They land ahead of the renderer's next draw:
    /// recorded into the encoder it draws into, which must then be submitted, or submitted ahead
    /// of a `draw_range` and of `submit`.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        atlas: &mut Atlas,
        instances: &[Instance],
        viewport: (u32, u32),
    ) {
        if self.springs.is_some() { assert_eq!(instances.len(), self.spring_instances); }
        let staging = self.uploads.staging(device, queue);
        if staging.lost() != self.uploads.lost.0 {
            self.uploads.lost.0 = staging.lost();
            self.resend_uploads(atlas);
        }
        if let Some(rows) = atlas.dirty.take() { Self::write_rows(&staging, &self.atlas, &atlas.data, rows); }
        let mut atlas_changed = self.atlas_pages.len() > atlas.pages.len();
        self.atlas_pages.truncate(atlas.pages.len());
        for (index, page) in atlas.pages.iter_mut().enumerate() {
            if index >= self.atlas_pages.len() {
                let texture = Self::texture(device, "glyph atlas", wgpu::TextureFormat::R8Unorm, ATLAS_SIZE, ATLAS_SIZE);
                let bind = Self::bind_group(device, &self.bind_layout, &self.globals, &texture, &texture, None);
                self.atlas_pages.push((texture, bind));
                atlas_changed = true;
            }
            if let Some(rows) = page.dirty.take() {
                Self::write_rows(&staging, &self.atlas_pages[index].0, &page.data, rows);
            }
        }
        if atlas_changed { self.images_changed(); }
        if instances.len() > self.capacity {
            self.capacity = instances.len().next_power_of_two();
            self.instances = Self::instance_buffer(device, self.capacity);
            self.uploads.instances.clear();
        }
        self.write_instances(&staging, instances);
        // Geometry uploads must preserve a presentation timestamp already latched this frame.
        if self.uploads.viewport.swap(pack(viewport), Ordering::Relaxed) != pack(viewport) {
            staging.write_buffer(&self.globals, 0, bytemuck::cast_slice(&[viewport.0 as f32, viewport.1 as f32]));
            #[cfg(test)] { self.uploads.globals_writes += 1; }
        }
        if self.uploads.srgb.replace(self.srgb_blend) != Some(self.srgb_blend) {
            let flags: [f32; 4] = [if self.srgb_blend { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0];
            staging.write_buffer(&self.globals, 16, bytemuck::cast_slice(&flags));
            #[cfg(test)] { self.uploads.globals_writes += 1; }
        }
        self.count = instances.len() as u32;
        self.image_batches.clear();
        let (mut start, mut bind) = (0, BatchBind::Image(0));
        let mut has_layers = false;
        self.effects.clear();
        for (i, instance) in instances.iter().enumerate() {
            has_layers |= instance.params[1] == KIND_LAYER;
            if instance.params[1] == KIND_EFFECT || instance.params[1] == KIND_BACKDROP_BLUR { self.effects.push((i as u32, *instance)); }
            let next_bind = if instance.params[1] == KIND_IMAGE {
                Some(BatchBind::Image(instance.params[0] as usize))
            } else if instance.params[1] == KIND_GLYPH {
                let g = instance.params[2] as usize;
                if g == 0 {
                    match bind {
                        BatchBind::Image(_) => None,
                        BatchBind::Glyph(_) => Some(BatchBind::Image(0)),
                    }
                } else {
                    Some(BatchBind::Glyph(g - 1))
                }
            } else {
                None
            };
            if let Some(next) = next_bind && next != bind {
                if start < i as u32 { self.image_batches.push((start..i as u32, bind)); }
                start = i as u32;
                bind = next;
            }
        }
        if start < self.count { self.image_batches.push((start..self.count, bind)); }
        self.prepare_layers(device, queue, if has_layers { instances } else { &[] }, viewport);
        self.sync_resident_caches(&mut atlas.resident_caches);
        if self.effects.iter().any(|(_, i)| i.params[0] != 0.0 || i.params[1] == KIND_BACKDROP_BLUR) {
            self.ensure_backdrop(device, self.format, viewport);
            for index in 0..self.extra.len() {
                self.ensure_backdrop(device, self.extra[index].0, viewport);
            }
            if self.layers.targets.iter().any(Option::is_some) {
                self.ensure_backdrop(device, layers::FORMAT, viewport);
            }
        }
        self.prepare_backdrop_blur(device, queue, viewport);
    }

    pub fn prepare_surfaces(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, draws: &[crate::native_surface::SurfaceDraw], viewport: (u32,u32)) {
        if !draws.is_empty() && self.surfaces.is_none() {
            let mut surfaces = crate::native_surface::SurfaceRenderer::new(device, self.format);
            surfaces.set_srgb_blend(self.srgb_blend);
            surfaces.set_staging(self.uploads.staging(device, queue));
            for (format, _) in &self.extra { surfaces.add_format(device, *format); }
            self.surfaces = Some(surfaces);
        }
        if let Some(s) = &mut self.surfaces {
            s.set_srgb_blend(self.srgb_blend);
            if self.layers.additive.is_some() { s.add_plus_lighter(device, layers::FORMAT); }
            s.prepare(device,queue,draws,viewport);
        }
        if self.layer_stats().targets > 0 { self.position_layer_surfaces(queue); }
    }

    /// One uniform write per presentation-only frame; use the display's target time.
    pub fn present_at(&self, queue: &wgpu::Queue, viewport: (u32, u32), time: f64) {
        let t = crate::gpu_springs::split_time(time);
        let data = [viewport.0 as f32, viewport.1 as f32, t[0], t[1]];
        self.uploads.viewport.store(pack(viewport), Ordering::Relaxed);
        match &self.uploads.staging {
            Some(s) => s.write_buffer(&self.globals, 0, bytemuck::cast_slice(&data)),
            None => queue.write_buffer(&self.globals, 0, bytemuck::cast_slice(&data)),
        }
    }

    /// A retarget touches one node record; caller computes inherited position/velocity on CPU.
    pub fn write_spring_node(&mut self, queue: &wgpu::Queue, id: usize, node: &crate::gpu_springs::Node) {
        let properties = self.springs.as_mut().expect("renderer has no GPU springs");
        assert!(id < properties.deadlines.len());
        properties.deadlines[id] = node.settle_at();
        properties.settle_at = properties.deadlines.iter().copied().fold(0.0, f64::max);
        let offset = (id * std::mem::size_of::<crate::gpu_springs::Node>()) as u64;
        match &self.uploads.staging {
            Some(s) => s.write_buffer(&properties.nodes, offset, bytemuck::bytes_of(node)),
            None => queue.write_buffer(&properties.nodes, offset, bytemuck::bytes_of(node)),
        }
    }

    /// Writes the instances from the first to the last that differs from what the buffer holds.
    fn write_instances(&mut self, staging: &Staging, instances: &[Instance]) {
        let held = &mut self.uploads.instances;
        let same = |(a, b): (&Instance, &Instance)| bytemuck::bytes_of(a) == bytemuck::bytes_of(b);
        let shared = instances.len().min(held.len());
        let pairs = || instances[..shared].iter().zip(&held[..shared]);
        let first = pairs().position(|p| !same(p)).unwrap_or(shared);
        let end = if instances.len() > held.len() {
            instances.len()
        } else {
            pairs().rposition(|p| !same(p)).map_or(first, |last| last + 1)
        };
        held.resize(instances.len(), Instance::zeroed());
        if first >= end { return; }
        held[first..end].copy_from_slice(&instances[first..end]);
        let offset = (first * std::mem::size_of::<Instance>()) as u64;
        staging.write_buffer(&self.instances, offset, bytemuck::cast_slice(&instances[first..end]));
    }

    /// The staging lost writes (an encoder holding them was dropped unsubmitted): forget what
    /// the buffers are thought to hold, so this frame writes it all again.
    fn resend_uploads(&mut self, atlas: &mut Atlas) {
        self.uploads.instances.clear();
        self.uploads.viewport.store(u64::MAX, Ordering::Relaxed);
        self.uploads.srgb = None;
        atlas.dirty = Some((0, ATLAS_SIZE));
        for page in &mut atlas.pages { page.dirty = Some((0, ATLAS_SIZE)); }
        self.layers.forget_blur_uniforms();
    }
}

#[cfg(test)]
#[path = "render_upload_tests.rs"]
mod tests;
