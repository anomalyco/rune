//! Per-frame uploads: instances, globals, glyph and image atlas rows, and GPU spring nodes.
//!
//! Writes go through the renderer's `Staging`, so a steady frame allocates no buffers. They are
//! recorded into the frame's encoder where the renderer first uses it (`flush_uploads`, at the
//! start of `encode_layers*` and `draw_target_range_with`), or submitted ahead of a draw into a
//! caller's pass (`draw_range*`) and of `Renderer::submit`. Globals are written only when they
//! change, and instances only in the spans that changed.

use std::sync::atomic::{AtomicU64, Ordering};

use bytemuck::Zeroable;
use rune_staging::Staging;

use super::*;

/// Upload state: the staging, and what the instance and globals buffers hold.
pub(super) struct Uploads {
    /// Made on the first upload, which brings the queue.
    staging: Option<Staging>,
    /// A compositor's `Frame::write_buffer`s, recorded into the frame's encoder as they come and
    /// sealed when it's submitted. Apart from `staging`, whose `submit` would seal them early.
    frame_writes: Option<Staging>,
    /// The instance buffer's contents, so a frame writes only the span that changed.
    instances: Vec<Instance>,
    /// Whether `image_batches`, `effects` and `has_layers` were made from `instances`: false
    /// once the list's length or an instance's `params` change, which can move a batch.
    batched: bool,
    /// Whether `instances` holds a layer.
    has_layers: bool,
    /// The viewport last written to globals (`width << 32 | height`, `u64::MAX` before any).
    viewport: AtomicU64,
    /// Gamma-space blend, byte-source, and packed scroll-fade stops last written to globals.
    srgb: Option<(bool, bool, [u32; 2])>,
    /// `Staging::lost` as last seen by `prepare` and by `upload_images`.
    lost: (u64, u64),
    /// The paint the next `prepare`'s instances come from, and which of them alone changed
    /// since the last `prepare` of that paint (`Renderer::next_instances`).
    next: Option<(u64, Option<Vec<u32>>)>,
    /// The paint `instances` came from, when the last `prepare` was told it.
    held_paint: Option<u64>,
    /// The last `prepare` wrote only a loop-only frame's slots.
    patched: bool,
    #[cfg(test)]
    pub(super) globals_writes: u64,
    #[cfg(test)]
    pub(super) batch_runs: u64,
}

impl Default for Uploads {
    fn default() -> Uploads {
        Uploads { staging: None, frame_writes: None, instances: Vec::new(), batched: false, has_layers: false, viewport: AtomicU64::new(u64::MAX), srgb: None, lost: (0, 0), next: None, held_paint: None, patched: false,
            #[cfg(test)] globals_writes: 0, #[cfg(test)] batch_runs: 0 }
    }
}

impl Uploads {
    /// Forgets what the instance buffer holds, so the next frame writes and batches it all.
    fn forget_instances(&mut self) {
        self.instances.clear();
        self.batched = false;
        self.held_paint = None;
    }

    pub(super) fn staging(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Staging {
        self.staging.get_or_insert_with(|| Staging::new(device, queue)).clone()
    }

    /// Frees the staging chunks kept for reuse (`Renderer::give_back`).
    pub(super) fn trim(&self) {
        if let Some(staging) = &self.staging { staging.trim(); }
    }

    #[cfg(test)]
    pub(super) fn stats(&self) -> rune_staging::Stats {
        self.staging.as_ref().map(Staging::stats).unwrap_or_default()
    }

    #[cfg(all(test, feature = "test-gpu-counters"))]
    pub(super) fn frame_write_stats(&self) -> rune_staging::Stats {
        self.frame_writes.as_ref().map(Staging::stats).unwrap_or_default()
    }
}

/// Instances `write_instances` compares at once.
const BLOCK: usize = 64;

/// Whether two equally long instance lists hold the same bytes. Folds over words into one
/// accumulator per word of an instance, which vectorizes, at about half the cost of a `memcmp`.
/// Words are `u32`, `Instance`'s own alignment, so any list casts.
fn same(a: &[Instance], b: &[Instance]) -> bool {
    let words = |l| bytemuck::cast_slice::<Instance, [u32; INSTANCE_WORDS]>(l);
    let mut acc = [0u32; INSTANCE_WORDS];
    for (x, y) in words(a).iter().zip(words(b)) {
        for j in 0..INSTANCE_WORDS { acc[j] |= x[j] ^ y[j]; }
    }
    acc.iter().fold(0, |s, w| s | w) == 0
}

fn pack((w, h): (u32, u32)) -> u64 { u64::from(w) << 32 | u64::from(h) }

impl Renderer {
    /// Records pending uploads into `encoder`, ahead of the renderer's commands in it, and the
    /// atlas fills staged after them.
    pub(super) fn flush_uploads(&self, encoder: &mut wgpu::CommandEncoder) {
        if let Some(s) = &self.uploads.staging { s.flush(encoder); }
        self.encode_fills(encoder);
    }

    /// Submits pending uploads (and atlas fills) ahead of commands already recorded.
    pub(super) fn submit_uploads(&self) {
        if self.fills.lock().unwrap_or_else(|e| e.into_inner()).is_some() { return self.submit_fills(); }
        if let Some(s) = &self.uploads.staging { s.submit(); }
    }

    /// Readies the frame's `encoder` for submission: submits pending uploads ahead of it, and
    /// seals the compositor's writes recorded into it.
    pub(crate) fn before_submit(&self, encoder: &mut wgpu::CommandEncoder) {
        self.submit_uploads();
        if let Some(s) = &self.uploads.frame_writes { s.flush(encoder); }
    }

    /// `Queue::write_buffer` through staging chunks, recorded into `encoder` here: ahead of the
    /// commands recorded after it (`Frame::write_buffer`). A frame's writes share a chunk, which
    /// `before_submit` hands back to the GPU.
    pub(crate) fn write_buffer(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder,
        buffer: &wgpu::Buffer, offset: u64, data: &[u8]) {
        let staging = self.uploads.frame_writes.get_or_insert_with(|| Staging::new(device, queue));
        staging.write_buffer(buffer, offset, data);
        staging.record(encoder);
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
        if images.pages.is_empty() && self.images.is_empty() { return self.refresh_layer_bindings(device); }
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
                let bind = Self::bind_group(device, &self.bind_layout, &self.globals, self.uber.outlines(), &self.atlas, &texture, None);
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
        // A caller may draw these lists into several different formats with draw_range.
        self.prepare_with_precision(device, queue, atlas, instances, viewport, false);
    }

    /// The default UI pass draws the root into one known target. Its float intermediate layers
    /// have their own projection and keep their source precision in the shader.
    pub(crate) fn prepare_ui(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, atlas: &mut Atlas,
        instances: &[Instance], viewport: (u32, u32)) {
        let bytes = self.srgb_blend && matches!(self.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm);
        self.prepare_with_precision(device, queue, atlas, instances, viewport, bytes);
    }

    fn prepare_with_precision(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, atlas: &mut Atlas,
        instances: &[Instance], viewport: (u32, u32), byte_source: bool) {
        if self.springs.is_some() { assert_eq!(instances.len(), self.spring_instances); }
        // The pinned pipelines (`variant::start_pinned`): asked for at the first prepare, taken once their thread has them.
        if self.pinned.is_none() && self.pinned_building.is_none() { self.start_pinned(device); }
        self.take_pinned();
        #[cfg(feature = "frame-profile")]
        super::report_pinned();
        let staging = self.uploads.staging(device, queue);
        if staging.lost() != self.uploads.lost.0 {
            self.uploads.lost.0 = staging.lost();
            self.resend_uploads(atlas);
        }
        // The Windfoil shader, built the first time text draws from outlines (render/variant.rs),
        // fills atlas pages on the GPU, so the pages are made again as ones it can draw into.
        if atlas.outlines.has_records() && self.select(device, variant::Event::OutlinesWanted) {
            self.atlas = Self::atlas_texture(device, true, self.atlas.height());
            for page in &mut self.atlas_pages { page.0 = Self::atlas_texture(device, true, ATLAS_SIZE); }
            self.rebind_all(device);
            atlas.invalidate();
        }
        // The first page's texture grows with what its shelves hold (or explicit dirty rows); a
        // new one starts empty, so every placed row goes up again and every GPU-filled mask is
        // filled again (`invalidate`).
        let dirty_rows = atlas.dirty;
        let rows = atlas.page_rows(0).max(dirty_rows.map_or(0, |(_, y1)| y1));
        if rows > self.atlas.height() {
            let height = rows.next_power_of_two().clamp(ATLAS_START_ROWS, ATLAS_SIZE);
            self.atlas = Self::atlas_texture(device, self.uber.outlines().is_some(), height);
            self.rebind_all(device);
            atlas.invalidate();
            if let Some((a, b)) = dirty_rows {
                atlas.dirty = Some(atlas.dirty.map_or((a, b), |(c, d)| (a.min(c), b.max(d))));
            }
        }
        atlas.outlines.set_support(self.uber.support());
        self.lcd_frame = self.lcd_ready(device, atlas.lcd_wanted) && atlas.lcd;
        if let Some(rows) = atlas.dirty.take() {
            atlas.rows_uploaded(0, rows);
            Self::write_rows(&staging, &self.atlas, &atlas.data, rows);
        }
        let mut atlas_changed = self.atlas_pages.len() > atlas.pages.len();
        self.atlas_pages.truncate(atlas.pages.len());
        let filled = self.uber.outlines().is_some();
        for index in 0..atlas.pages.len() {
            if index >= self.atlas_pages.len() {
                let texture = Self::atlas_texture(device, filled, ATLAS_SIZE);
                let bind = Self::bind_group(device, &self.bind_layout, &self.globals, self.uber.outlines(), &texture, &texture, None);
                self.atlas_pages.push((texture, bind));
                atlas_changed = true;
            }
            if let Some(rows) = atlas.pages[index].dirty.take() {
                atlas.rows_uploaded(index + 1, rows);
                Self::write_rows(&staging, &self.atlas_pages[index].0, &atlas.pages[index].data, rows);
            }
        }
        if atlas_changed { self.images_changed(); }
        self.upload_outlines(device, &staging, atlas);
        self.fill_atlas(device, queue, &staging, atlas);
        if self.instances.ensure(device, instances.len()) {
            self.uploads.forget_instances();
        }
        self.write_instances(&staging, instances);
        // Geometry uploads must preserve a presentation timestamp already latched this frame.
        if self.uploads.viewport.swap(pack(viewport), Ordering::Relaxed) != pack(viewport) {
            staging.write_buffer(&self.globals, std::mem::offset_of!(Globals, viewport) as u64, bytemuck::cast_slice(&[viewport.0 as f32, viewport.1 as f32]));
            #[cfg(test)] { self.uploads.globals_writes += 1; }
        }
        let mode = (self.srgb_blend, byte_source, [self.fade_stops[0].to_bits(), self.fade_stops[1].to_bits()]);
        if self.uploads.srgb.replace(mode) != Some(mode) {
            let flags: [f32; 4] = [self.srgb_blend as u8 as f32, byte_source as u8 as f32, self.fade_stops[0], self.fade_stops[1]];
            staging.write_buffer(&self.globals, std::mem::offset_of!(Globals, flags) as u64, bytemuck::cast_slice(&flags));
            #[cfg(test)] { self.uploads.globals_writes += 1; }
        }
        self.count = instances.len() as u32;
        if !self.uploads.batched {
            self.batch(instances);
        } else {
            // Same batches: only the effects' copies of their instances can be out of date.
            for (at, effect) in &mut self.effects { *effect = instances[*at as usize]; }
        }
        let has_layers = self.uploads.has_layers;
        // A loop-only frame that draws no layer leaves them as the last frame drew them.
        #[cfg(any(target_os = "windows", target_arch = "wasm32"))]
        let layers_kept = self.uploads.patched && self.cycle_leaves_layers();
        #[cfg(not(any(target_os = "windows", target_arch = "wasm32")))]
        let layers_kept = false;
        if !layers_kept { self.prepare_layers(device, queue, if has_layers { instances } else { &[] }, viewport); }
        self.sync_resident_caches(&mut atlas.resident_caches);
        self.backdrop_used = self.effects.iter().any(|(_, i)| i.params[0] != 0.0 || i.kind() == Some(Kind::BackdropBlur));
        if self.backdrop_used {
            self.ensure_backdrop(device, self.format, viewport);
            for index in 0..self.extra.len() {
                self.ensure_backdrop(device, self.extra[index].0, viewport);
            }
            if self.layers.targets.iter().any(Option::is_some) {
                self.ensure_backdrop(device, layers::FORMAT, viewport);
            }
        }
        self.prepare_backdrop_blur(device, queue, viewport);
        #[cfg(all(feature = "alloc-sites", not(target_arch = "wasm32")))]
        self.report_pools();
    }

    /// A window's or overlay's first native surface: its renderer's pipeline compiles on a thread
    /// of its own, and `done` runs once it's ready (the window's live wake, so the next frame
    /// draws it). Until then `prepare_surfaces` draws no surface: a clip that's opening has no
    /// picture yet anyway. Without this call, `prepare_surfaces` compiles in place.
    pub(crate) fn build_surfaces(&mut self, device: &wgpu::Device, draws: &[crate::native_surface::SurfaceDraw], done: impl FnOnce() + Send + 'static) {
        if draws.is_empty() || self.surfaces.is_some() || self.surfaces_building.is_some() { return; }
        self.surfaces_building = Some(crate::native_surface::SurfaceRenderer::build(device, self.format, Box::new(done)));
    }

    /// Punches these native surfaces' places out of the window's pixels from the next
    /// `prepare_surfaces` on, for clips shown on their own layer underneath (Windows).
    #[cfg(target_os = "windows")]
    pub(crate) fn set_punched_surfaces(&mut self, keys: Vec<usize>) {
        if let Some(s) = &mut self.surfaces { s.set_punched(keys); }
    }

    /// The punch pipeline has compiled (`set_punched_surfaces`).
    #[cfg(target_os = "windows")]
    pub(crate) fn surface_punch_ready(&mut self) -> bool {
        self.surfaces.as_mut().is_some_and(crate::native_surface::SurfaceRenderer::punch_ready)
    }

    /// Whether the last frame drew surface `key` inside an isolated layer; None when it didn't
    /// draw it.
    #[cfg(target_os = "windows")]
    pub(crate) fn surface_drew_isolated(&self, key: usize) -> Option<bool> {
        self.surfaces.as_ref().and_then(|s| s.drew_isolated(key))
    }

    pub fn prepare_surfaces(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, draws: &[crate::native_surface::SurfaceDraw], viewport: (u32,u32)) {
        let built = match &self.surfaces_building {
            Some(building) => crate::native_surface::SurfaceRenderer::finish(device, building),
            None => None,
        };
        if built.is_some() { self.surfaces_building = None; }
        if self.surfaces.is_none() && self.surfaces_building.is_some() { return; }
        if (!draws.is_empty() || built.is_some()) && self.surfaces.is_none() {
            let mut surfaces = built.unwrap_or_else(|| crate::native_surface::SurfaceRenderer::new(device, self.format));
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
        const _: () = assert!(std::mem::offset_of!(Globals, viewport) == 0 && std::mem::offset_of!(Globals, pad) == 8);
        let t = crate::gpu::springs::split_time(time);
        let data = [viewport.0 as f32, viewport.1 as f32, t[0], t[1]];
        self.uploads.viewport.store(pack(viewport), Ordering::Relaxed);
        let offset = std::mem::offset_of!(Globals, viewport) as u64;
        match &self.uploads.staging {
            Some(s) => s.write_buffer(&self.globals, offset, bytemuck::cast_slice(&data)),
            None => queue.write_buffer(&self.globals, offset, bytemuck::cast_slice(&data)),
        }
    }

    /// A retarget touches one node record; caller computes inherited position/velocity on CPU.
    pub fn write_spring_node(&mut self, queue: &wgpu::Queue, id: usize, node: &crate::gpu::springs::Node) {
        let properties = self.springs.as_mut().expect("renderer has no GPU springs");
        assert!(id < properties.deadlines.len());
        properties.deadlines[id] = node.settle_at();
        properties.settle_at = properties.deadlines.iter().copied().fold(0.0, f64::max);
        let offset = (id * std::mem::size_of::<crate::gpu::springs::Node>()) as u64;
        match &self.uploads.staging {
            Some(s) => s.write_buffer(&properties.nodes, offset, bytemuck::bytes_of(node)),
            None => queue.write_buffer(&properties.nodes, offset, bytemuck::bytes_of(node)),
        }
    }

    /// Splits the instances into runs that bind the same atlas pages, and notes their effects
    /// and whether they hold a layer.
    fn batch(&mut self, instances: &[Instance]) {
        self.image_batches.clear();
        self.kinds.clear();
        self.kinds.extend(instances.iter().map(Instance::kind));
        let (mut start, mut bind) = (0, BatchBind::Image(0));
        let mut has_layers = false;
        self.effects.clear();
        for (i, instance) in instances.iter().enumerate() {
            let Some(kind) = instance.kind() else { continue };
            let next_bind = match kind {
                Kind::Rect | Kind::Shadow | Kind::Inset | Kind::Gradient | Kind::Segment
                | Kind::Triangle | Kind::LayerEnd | Kind::Dashed | Kind::Radial => continue,
                Kind::Layer => { has_layers = true; continue; }
                Kind::Effect | Kind::BackdropBlur => { self.effects.push((i as u32, *instance)); continue; }
                Kind::Image => Some(BatchBind::Image(instance.params[0] as usize)),
                Kind::Glyph => {
                    let g = instance.params[2] as usize;
                    if g == 0 {
                        match bind {
                            BatchBind::Image(_) => None,
                            BatchBind::Glyph(_) => Some(BatchBind::Image(0)),
                        }
                    } else {
                        Some(BatchBind::Glyph(g - 1))
                    }
                }
            };
            if let Some(next) = next_bind && next != bind {
                if start < i as u32 { self.image_batches.push((start..i as u32, bind)); }
                start = i as u32;
                bind = next;
            }
        }
        if start < instances.len() as u32 { self.image_batches.push((start..instances.len() as u32, bind)); }
        self.uploads.has_layers = has_layers;
        self.uploads.batched = true;
        #[cfg(test)] { self.uploads.batch_runs += 1; }
    }

    /// Writes the spans of instances that differ from what the buffer holds, comparing a block
    /// at a time. Spans less than a block apart merge, so a scroll writes one span and a frame
    /// where a few instances move writes only those. A span whose `params` changed asks `prepare`
    /// to batch again.
    fn write_instances(&mut self, staging: &Staging, instances: &[Instance]) {
        let next = self.uploads.next.take();
        self.uploads.patched = false;
        if let Some((paint, Some(patched))) = &next && self.uploads.held_paint == Some(*paint) && instances.len() == self.uploads.instances.len() {
            self.uploads.patched = true;
            return self.write_patched(staging, instances, patched);
        }
        self.uploads.held_paint = next.map(|n| n.0);
        let held = &mut self.uploads.instances;
        let shared = instances.len().min(held.len());
        let mut rebatch = instances.len() != held.len();
        held.resize(instances.len(), Instance::zeroed());
        let mut open: Option<std::ops::Range<usize>> = None;
        for start in (0..shared).step_by(BLOCK) {
            let end = (start + BLOCK).min(shared);
            let differs = |i: &usize| !same(&instances[*i..=*i], &held[*i..=*i]);
            // The first instance alone, so a block that all moved costs one compare to find.
            let first = if differs(&start) { start } else if same(&instances[start..end], &held[start..end]) { continue } else {
                (start + 1..end).find(differs).unwrap_or(start)
            };
            let last = (first..end).rfind(differs).map_or(end, |i| i + 1);
            rebatch |= Self::join(self.instances.buffer(), held, staging, instances, &mut open, first..last);
        }
        if instances.len() > shared { rebatch |= Self::join(self.instances.buffer(), held, staging, instances, &mut open, shared..instances.len()); }
        if let Some(span) = open { rebatch |= Self::write_span(self.instances.buffer(), held, staging, instances, span); }
        self.uploads.batched &= !rebatch;
    }

    /// Writes only `patched` (ascending): the instances a loop-only frame changed (a dot's colour,
    /// a sliding window's clip) since the buffer was written from the same paint, so nothing else
    /// needs comparing. Batches split on the image, kind and atlas page (`params` 0 to 2): only a
    /// change there can move one.
    fn write_patched(&mut self, staging: &Staging, instances: &[Instance], patched: &[u32]) {
        let held = &mut self.uploads.instances;
        debug_assert!(held.iter().zip(instances).enumerate().all(|(n, (h, i))| patched.binary_search(&(n as u32)).is_ok() || same(std::slice::from_ref(h), std::slice::from_ref(i))),
            "a loop-only frame changed an instance that isn't one of its slots");
        let mut rebatch = false;
        let mut k = 0;
        while k < patched.len() {
            let start = patched[k] as usize;
            let mut end = start + 1;
            while k + 1 < patched.len() && patched[k + 1] as usize == end { k += 1; end += 1; }
            k += 1;
            if end > instances.len() || same(&instances[start..end], &held[start..end]) { continue; }
            rebatch |= held[start..end].iter().zip(&instances[start..end]).any(|(h, i)| h.params[..3] != i.params[..3]);
            held[start..end].copy_from_slice(&instances[start..end]);
            let offset = (start * std::mem::size_of::<Instance>()) as u64;
            staging.write_buffer(self.instances.buffer(), offset, bytemuck::cast_slice(&instances[start..end]));
        }
        self.uploads.batched &= !rebatch;
    }

    /// The instances the buffer holds, when they are paint `paint`'s and `len` of them.
    #[cfg_attr(not(any(target_os = "windows", target_arch = "wasm32")), allow(dead_code))]
    pub(super) fn held_instances(&self, paint: u64, len: usize) -> Option<&[Instance]> {
        (self.uploads.held_paint == Some(paint) && self.uploads.instances.len() == len).then_some(&self.uploads.instances[..])
    }

    /// The next `prepare`'s instances are paint `paint`'s (`runtime/paint/cycle.rs`). With
    /// `patched`, only those instances changed since the last `prepare` of the same paint.
    pub(crate) fn next_instances(&mut self, paint: u64, patched: Option<Vec<u32>>) {
        self.uploads.next = Some((paint, patched));
    }

    /// Adds `span` to the open span if it is less than a block past it, or writes the open span
    /// and opens `span`. True if it wrote a span whose `params` changed.
    fn join(buffer: &wgpu::Buffer, held: &mut [Instance], staging: &Staging, instances: &[Instance],
        open: &mut Option<std::ops::Range<usize>>, span: std::ops::Range<usize>) -> bool {
        match open {
            Some(o) if span.start <= o.end + BLOCK => { o.end = span.end; false }
            _ => open.replace(span).is_some_and(|o| Self::write_span(buffer, held, staging, instances, o)),
        }
    }

    /// Writes `span`, and says whether any of its instances' `params`, which pick their batch,
    /// changed.
    fn write_span(buffer: &wgpu::Buffer, held: &mut [Instance], staging: &Staging, instances: &[Instance], span: std::ops::Range<usize>) -> bool {
        let params = |i: &Instance| bytemuck::cast::<[f32; 4], u128>(i.params);
        let rebatch = held[span.clone()].iter().zip(&instances[span.clone()]).any(|(h, i)| params(h) != params(i));
        held[span.clone()].copy_from_slice(&instances[span.clone()]);
        let offset = (span.start * std::mem::size_of::<Instance>()) as u64;
        staging.write_buffer(buffer, offset, bytemuck::cast_slice(&instances[span]));
        rebatch
    }

    /// The staging lost writes (an encoder holding them was dropped unsubmitted): forget what
    /// the buffers are thought to hold, so this frame writes it all again.
    fn resend_uploads(&mut self, atlas: &mut Atlas) {
        self.uploads.forget_instances();
        self.uploads.viewport.store(u64::MAX, Ordering::Relaxed);
        self.uploads.srgb = None;
        atlas.resend_rows();
        atlas.outlines.invalidate();
        self.layers.forget_blur_uniforms();
    }
}

#[cfg(test)]
mod tests;
