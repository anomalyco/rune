use super::super::*;
use super::{Imported, Prepared, SurfaceRenderer, TimingPool};
use bytemuck::{Pod, Zeroable};
use rune_core::Blend;
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    viewport: [f32; 4],
    node: [f32; 4],
    image: [f32; 4],
    radii: [f32; 4],
    clip: [f32; 4],
    fade: [u32; 4],
    // format (0 BGRA, 1 video NV12, 2 full NV12), matrix (0 601, 1 709), transfer, alpha
    color: [f32; 4],
    // Exact mapping flag, source dimensions, horizontal mirror flag.
    sampling: [f32; 4],
    // `Orient`: centred upright to centred buffer coordinates.
    orient: [f32; 4],
}

/// Bind groups a draw keeps for textures it may show again (a video decoder's ring).
const KEPT_BINDS: usize = 8;

impl SurfaceRenderer {
    fn write(&self, queue: &wgpu::Queue, buffer: &wgpu::Buffer, data: &[u8]) {
        match &self.staging {
            Some(s) => s.write_buffer(buffer, 0, data),
            None => queue.write_buffer(buffer, 0, data),
        }
    }

    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, draws: &[SurfaceDraw], viewport: (u32, u32)) {
        *self.queries_used.get_mut() = false;
        // Release an unsubmitted preparation before acquiring its replacement. Submitted
        // slots stay leased by their completion callbacks, including the deferred resolve.
        self.queries = None;
        // GPU times only for a surface someone watches: resolving them costs a submission a frame.
        if draws.iter().any(|d| d.surface.observed()) {
            self.queries = self.timing_pool.as_ref().and_then(TimingPool::take);
        }
        self.cache.retain(|key, _| draws.iter().any(|d| d.surface.key() == *key));
        // Keep uniforms/bindings on unchanged frames. The source is latched here, after scene build.
        let mut count = 0;
        for d in draws {
            let Some((frame, mirrored)) = d.surface.latest_mirrored() else {
                self.cache.remove(&d.surface.key());
                continue;
            };
            if d.rect[2] <= 0.0 || d.rect[3] <= 0.0 {
                continue;
            }
            let key = d.surface.key();
            let changed = self.cache.get(&key).is_none_or(|v| !Arc::ptr_eq(&v.frame, &frame));
            if changed {
                match self.import(device, queue, frame) {
                    Ok(imported) => {
                        self.cache.insert(key, Arc::new(imported));
                        d.surface.0.lock().unwrap().error = None;
                    }
                    Err(e) => {
                        d.surface.0.lock().unwrap().error = Some(e);
                        self.cache.remove(&key);
                        continue;
                    }
                }
            }
            let imported = self.cache[&key].clone();
            let f = &imported.frame;
            let upright = f.orient.upright(f.size);
            let image = super::super::place::image_rect(d.rect, upright, d.fit, d.surface.place());
            let exact = (image[2] - upright[0] as f32).abs() < 0.001
                && (image[3] - upright[1] as f32).abs() < 0.001
                && (image[0] - image[0].round()).abs() < 0.001
                && (image[1] - image[1].round()).abs() < 0.001;
            let u = Uniforms {
                viewport: [viewport.0 as f32, viewport.1 as f32, 0.0, 0.0],
                node: d.rect,
                image,
                radii: d.radii,
                clip: d.clip,
                fade: d.fade,
                color: [
                    match f.format {
                        Format::Bgra => 0.0,
                        Format::Nv12Video => 1.0,
                        Format::Nv12Full => 2.0,
                    },
                    if f.color.matrix == Matrix::Bt709 { 1.0 } else { 0.0 },
                    if self.srgb_blend { 2.0 } else if f.color.transfer == Transfer::Srgb { 0.0 } else { 1.0 },
                    d.opacity,
                ],
                sampling: [if exact { 1.0 } else { 0.0 }, f.size[0] as f32, f.size[1] as f32, if mirrored { 1.0 } else { 0.0 }],
                orient: f.orient.0.map(f32::from),
            };
            // The same draw with a new picture keeps its uniform buffer, and its bind group too
            // when the picture is in textures it bound before.
            if let Some(p) = self.draws.get_mut(count).filter(|p| p.surface.key() == d.surface.key() || Arc::ptr_eq(&p.imported, &imported)) {
                *p.drawn.get_mut() = false;
                match &self.staging {
                    Some(s) => s.write_buffer(&p.uniform, 0, bytemuck::bytes_of(&u)),
                    None => queue.write_buffer(&p.uniform, 0, bytemuck::bytes_of(&u)),
                }
                if !Arc::ptr_eq(&p.imported, &imported) && p.imported.textures != imported.textures {
                    let old = std::mem::replace(&mut p.bind, match p.binds.iter().position(|(t, _)| *t == imported.textures) {
                        Some(i) => p.binds.remove(i).1,
                        None => bind_group(device, &self.layout, &self.sampler, &p.uniform, &imported.textures),
                    });
                    p.binds.push((p.imported.textures.clone(), old));
                    if p.binds.len() > KEPT_BINDS {
                        p.binds.remove(0);
                    }
                }
                p.imported = imported;
                p.before = d.before_instance;
                p.surface = d.surface.clone();
            } else {
                let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("video node"),
                    contents: bytemuck::bytes_of(&u),
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                });
                let bind = bind_group(device, &self.layout, &self.sampler, &uniform, &imported.textures);
                let p = Prepared {
                    drawn: std::sync::atomic::AtomicBool::new(false),
                    before: d.before_instance,
                    bind,
                    uniform,
                    imported,
                    surface: d.surface.clone(),
                    binds: Vec::new(),
                    punch: false,
                    isolated: std::sync::atomic::AtomicBool::new(false),
                };
                if count < self.draws.len() {
                    self.draws[count] = p;
                } else {
                    self.draws.push(p);
                }
            }
            let p = &mut self.draws[count];
            p.punch = self.punch.is_some() && self.punched.contains(&key);
            *p.isolated.get_mut() = false;
            count += 1;
        }
        self.draws.truncate(count);
        #[cfg(target_os = "windows")]
        if let Some(shared) = &mut self.shared {
            shared.forget_gone();
        }
        #[cfg(target_os = "linux")]
        if let Some(dmabuf) = &mut self.dmabuf {
            dmabuf.forget_gone();
        }
        #[cfg(target_os = "macos")]
        if let Some(i) = &self.importer {
            i.flush();
        }
    }

    fn import(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: Arc<Frame>) -> Result<Imported, String> {
        let textures = match &frame.source {
            // Checks the size itself, so every failure reaches the producer.
            #[cfg(target_os = "linux")]
            Source::Dmabuf(d) => linux::import(&mut self.dmabuf, device, queue, d)?,
            _ if frame.size.iter().any(|v| *v > device.limits().max_texture_dimension_2d) => {
                return Err("video dimensions exceed this GPU device's texture limit".into());
            }
            Source::Bytes(b) => {
                let a = upload(
                    device,
                    queue,
                    b.width,
                    b.height,
                    if b.format == Format::Bgra {
                        wgpu::TextureFormat::Bgra8Unorm
                    } else {
                        wgpu::TextureFormat::R8Unorm
                    },
                    &b.planes[0],
                );
                let b = if b.format == Format::Bgra {
                    a.clone()
                } else {
                    upload(device, queue, b.width / 2, b.height / 2, wgpu::TextureFormat::Rg8Unorm, &b.planes[1])
                };
                [a, b]
            }
            #[cfg(target_os = "macos")]
            Source::PixelBuffer(b) => {
                if self.importer.is_none() {
                    self.importer = Some(macos::Importer::new(device)?);
                }
                objc2::rc::autoreleasepool(|_| {
                    self.importer.as_ref().unwrap().import(b, frame.format, frame.size)
                })?
            }
            #[cfg(target_os = "windows")]
            Source::Shared(t) => {
                if self.shared.is_none() {
                    self.shared = Some(windows::Importer::new(device)?);
                }
                self.shared.as_mut().unwrap().import(queue, t)?
            }
            #[cfg(target_os = "windows")]
            Source::Decoded(planes, value) => {
                if self.shared.is_none() {
                    self.shared = Some(windows::Importer::new(device)?);
                }
                self.shared.as_mut().unwrap().import_decoded(queue, planes, *value)?
            }
        };
        Ok(Imported { frame, textures })
    }

    pub fn len(&self) -> usize {
        self.draws.len()
    }
    pub fn is_empty(&self) -> bool {
        self.draws.is_empty()
    }
    #[doc(hidden)]
    pub fn draws_nothing(&self) -> bool {
        self.draws.is_empty()
    }

    /// Include this in the containing render pass for hardware GPU timing.
    /// Submit that encoder through [`Self::submit`] before the next [`Self::prepare`].
    /// Plain `Queue::submit` does not retain the timing slot and may let it be reused
    /// while the recorded pass is still in flight.
    pub fn timestamp_writes(&self) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        if self.draws.is_empty() {
            return None;
        }
        self.queries.as_ref().map(|slot| {
            self.queries_used.store(true, std::sync::atomic::Ordering::Relaxed);
            wgpu::RenderPassTimestampWrites {
                query_set: &slot.pool.queries,
                beginning_of_pass_write_index: Some(slot.start),
                end_of_pass_write_index: Some(slot.start + 1),
            }
        })
    }

    #[cfg(target_os = "windows")]
    #[doc(hidden)]
    /// Whether the last frame drew surface `key` inside an isolated layer, where a punch
    /// can't reach the window; None when it didn't draw it.
    pub fn drew_isolated(&self, key: usize) -> Option<bool> {
        self.draws.iter().find(|d| d.surface.key() == key).map(|d| d.isolated.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Draws `index` in the window's own pass: punched out when asked, else as any other.
    #[doc(hidden)]
    pub fn draw_main(&self, pass: &mut wgpu::RenderPass<'_>, index: usize, format: wgpu::TextureFormat, blend: Blend) {
        match (&self.punch, self.draws[index].punch) {
            (Some(punch), true) => {
                self.draws[index].drawn.store(true, std::sync::atomic::Ordering::Relaxed);
                pass.set_pipeline(punch);
                pass.set_bind_group(0, &self.draws[index].bind, &[]);
                pass.draw(0..4, 0..1);
            }
            _ => self.draw_blended(pass, index, format, blend),
        }
    }

    /// Draws `index` inside an isolated layer.
    #[doc(hidden)]
    pub fn draw_isolated(&self, pass: &mut wgpu::RenderPass<'_>, index: usize, format: wgpu::TextureFormat, blend: Blend) {
        self.draws[index].isolated.store(true, std::sync::atomic::Ordering::Relaxed);
        self.draw_blended(pass, index, format, blend);
    }

    pub fn before_instance(&self, index: usize) -> u32 {
        self.draws[index].before
    }
    /// Reproject one draw into a bounded layer without moving its screen-space clip or UVs.
    #[doc(hidden)]
    pub fn target(&self, queue: &wgpu::Queue, index: usize, bounds: [f32; 4]) {
        self.write(queue, &self.draws[index].uniform, bytemuck::cast_slice(&[bounds[2], bounds[3], bounds[0], bounds[1]]));
    }
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, index: usize) {
        self.draw_format(pass, index, self.format);
    }
    #[doc(hidden)]
    pub fn draw_format(&self, pass: &mut wgpu::RenderPass<'_>, index: usize, format: wgpu::TextureFormat) {
        self.draw_blended(pass, index, format, Blend::SourceOver);
    }
    #[doc(hidden)]
    pub fn draw_blended(&self, pass: &mut wgpu::RenderPass<'_>, index: usize, format: wgpu::TextureFormat, blend: Blend) {
        self.draws[index].drawn.store(true, std::sync::atomic::Ordering::Relaxed);
        pass.set_pipeline(if blend == Blend::PlusLighter {
            let (f, pipeline) = self.additive.as_ref().expect("prepared dissolve surface");
            assert_eq!(*f, format);
            pipeline
        } else if format == self.format { &self.pipeline } else {
            &self.extra.iter().find(|(f, _)| *f == format).expect("SurfaceRenderer::add_format first").1
        });
        pass.set_bind_group(0, &self.draws[index].bind, &[]);
        pass.draw(0..4, 0..1);
    }
    #[cfg(target_arch = "wasm32")]
    pub fn submit(&self, queue: &wgpu::Queue, encoder: wgpu::CommandEncoder) -> wgpu::SubmissionIndex {
        assert!(self.draws.is_empty(), "native video surfaces are unsupported on web");
        queue.submit([encoder.finish()])
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn submit(&self, queue: &wgpu::Queue, encoder: wgpu::CommandEncoder) -> wgpu::SubmissionIndex {
        let submitted_at = Instant::now();
        let frames: Vec<_> = self
            .draws
            .iter()
            .filter(|p| p.drawn.load(std::sync::atomic::Ordering::Relaxed))
            .map(|p| {
                (
                    p.surface.clone(),
                    p.imported.clone(),
                    FrameTiming {
                        frame_id: p.imported.frame.id,
                        submitted_at,
                        completed_at: None,
                        gpu_start_seconds: None,
                        gpu_end_seconds: None,
                    },
                )
            })
            .collect();
        if frames.is_empty() {
            return queue.submit([encoder.finish()]);
        }
        // Notify before installing the GPU handler: completion may race queue.submit returning.
        for (surface, _, t) in &frames {
            surface.report(t.clone());
        }
        // A caller may use Renderer::draw_timed with its own query set. Never resolve our
        // unwritten counters in that case; CPU submission/completion notifications still work.
        if let Some(queries) =
            self.queries.as_ref().filter(|_| self.queries_used.load(std::sync::atomic::Ordering::Relaxed))
        {
            let resolve = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("video timestamps"),
                size: 16,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("video timestamp readback"),
                size: 16,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let seconds_per_tick = queue.get_timestamp_period() as f64 * 1e-9;
            let queries = queries.clone();
            let device = self.device.clone();
            let queue = queue.clone();
            let completion = self.completion.clone();
            // Metal counter resolves can race the render pass's final fragment sample when
            // encoded into the same command buffer. Resolve after its completion fence; this
            // is timestamp metadata only, never a readback or wait on the video pixel path.
            encoder.on_submitted_work_done(move || {
                let completed_at = Instant::now();
                let mut timestamps = device.create_command_encoder(&Default::default());
                timestamps.resolve_query_set(&queries.pool.queries, queries.start..queries.start + 2, &resolve, 0);
                timestamps.copy_buffer_to_buffer(&resolve, 0, &readback, 0, 16);
                let result = readback.clone();
                timestamps.map_buffer_on_submit(&readback, wgpu::MapMode::Read, .., move |status| {
                    let times = if status.is_ok() {
                        let bytes = result.slice(..).get_mapped_range().expect("mapped timestamps");
                        let a = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
                        let b = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
                        (b >= a && b != 0).then_some((a as f64 * seconds_per_tick, b as f64 * seconds_per_tick))
                    } else {
                        None
                    };
                    if status.is_ok() {
                        result.unmap();
                    }
                    // The sample pair must survive both GPU submissions and CPU mapping.
                    drop(queries);
                    for (s, _, mut t) in frames {
                        t.completed_at = Some(completed_at);
                        if let Some((a, b)) = times {
                            t.gpu_start_seconds = Some(a);
                            t.gpu_end_seconds = Some(b);
                        }
                        s.report(t);
                    }
                });
                queue.submit([timestamps.finish()]);
                let _ = completion.try_send(());
            });
        } else {
            encoder.on_submitted_work_done(move || {
                for (s, _, mut t) in frames {
                    t.completed_at = Some(Instant::now());
                    s.report(t);
                }
            });
        }
        let index = queue.submit([encoder.finish()]);
        self.release_decoded(queue);
        let _ = self.completion.try_send(());
        index
    }

    /// Decoded video pictures this submission drew may be written again once it's done.
    #[cfg(not(target_arch = "wasm32"))]
    fn release_decoded(&self, queue: &wgpu::Queue) {
        #[cfg(target_os = "windows")]
        for p in self.draws.iter().filter(|p| p.drawn.load(std::sync::atomic::Ordering::Relaxed)) {
            if let Source::Decoded(planes, value) = &p.imported.frame.source {
                windows::release_decoded(queue, planes, *value);
            }
        }
        #[cfg(not(target_os = "windows"))]
        let _ = queue;
    }
}

fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    bytes: &[u8],
) -> wgpu::Texture {
    let mut desc = super::descriptor(width, height, format);
    desc.usage |= wgpu::TextureUsages::COPY_DST;
    let texture = device.create_texture(&desc);
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * format.block_copy_size(None).unwrap()),
            rows_per_image: None,
        },
        desc.size,
    );
    texture
}

fn bind_group(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, sampler: &wgpu::Sampler, uniform: &wgpu::Buffer, textures: &[wgpu::Texture; 2]) -> wgpu::BindGroup {
    let views = textures.each_ref().map(|t| t.create_view(&Default::default()));
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("video frame"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&views[0]) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&views[1]) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(sampler) },
        ],
    })
}
