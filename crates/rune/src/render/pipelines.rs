//! Building the renderer and its uber pipelines: the pipeline for each target format and blend,
//! the sampled textures the CPU fills, and the bind group the uber shader reads.

use super::{
    ATLAS_START_ROWS, Blend, CLIP_WGSL, Globals, Instance, Renderer, dxbc, metallib,
    shader_check::{self, ShaderError, UberCheck},
    variant,
    warmup::WarmRenderer,
};

/// Default blend mode for a target format: sRGB-encoded blending for 8-bit non-sRGB targets
/// (`Rgba8Unorm` / `OFFSCREEN_FORMAT` and `Bgra8Unorm`), and linear light for `*Srgb` targets
/// (`OFFSCREEN_FORMAT_LINEAR`, `Bgra8UnormSrgb`) and floating-point HDR targets (`Rgba16Float`).
#[inline]
pub(super) fn default_srgb_blend(format: wgpu::TextureFormat) -> bool {
    matches!(format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm)
}

impl Renderer {
    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(super) fn new_warm(device: &wgpu::Device, format: wgpu::TextureFormat) -> WarmRenderer {
        Self::try_new_warm(device, format).unwrap_or_else(|error| panic!("{error}"))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn try_new_warm(device: &wgpu::Device, format: wgpu::TextureFormat) -> Result<WarmRenderer, ShaderError> {
        Self::try_create_warm(device, format, None, cfg!(target_os = "macos"))
    }

    pub(super) fn try_create(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        properties: Option<(&[crate::gpu::springs::Node], &[u32])>,
        classic: bool,
    ) -> Result<Renderer, ShaderError> {
        Ok(Self::from_warm(Self::try_create_warm(device, format, properties, classic)?))
    }

    /// True when the renderer was created with the Windfoil uber shader rather than Classic.
    #[cfg(target_arch = "wasm32")]
    #[cfg_attr(not(feature = "webgl"), allow(dead_code))]
    pub(crate) fn is_windfoil(&self) -> bool {
        matches!(self.uber, variant::Uber::Windfoil { .. })
    }

    fn try_create_warm(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        properties: Option<(&[crate::gpu::springs::Node], &[u32])>,
        classic: bool,
    ) -> Result<WarmRenderer, ShaderError> {
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(device.limits().min_uniform_buffer_offset_alignment).max(std::mem::size_of::<Globals>() as u64) + 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let stand_in = Self::image_texture(device, 1, 1);
        // Subpixel text waits for the subpixel pipeline (`lcd_ready`), not just the feature.
        if !device.features().contains(wgpu::Features::DUAL_SOURCE_BLENDING) { crate::text::note_dual_source(false); }
        let spring_layout = properties.map(|_| crate::gpu::springs::Bindings::layout(device));
        let vertex_entry = if properties.is_some() { "vs_springs" } else { "vs" };
        #[cfg_attr(not(target_arch = "wasm32"), allow(unused_mut))]
        let mut uber_check = UberCheck::default();
        #[cfg_attr(not(target_arch = "wasm32"), allow(unused_mut))]
        let mut uber_check_seq = crate::machine::Seq::default();
        #[cfg(target_arch = "wasm32")]
        let uber_check_wake = shader_check::WakeSlot::default();
        #[cfg(target_arch = "wasm32")]
        let web_scopes = crate::platform::web::health::Scopes::push(device);
        // The shader machine starts Classic; everywhere but macOS (whose default text keeps the
        // Classic shader it always drew) and GPU springs, it asks for Windfoil at once.
        let (uber, windfoil) = if classic {
            (variant::Uber::Classic, None)
        } else {
            Self::start_windfoil(device, &variant::BuildContext { format, vertex_entry, globals: &globals, stand_in: &stand_in })
        };
        let built = match windfoil {
            Some(built) => Ok((built.bind_layout, built.layout, built.shader, built.pipeline)),
            None => {
                let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: None,
                    entries: &metallib::uber_bind_entries(),
                });
                let layouts = [Some(&bgl), spring_layout.as_ref()];
                let layout = device.create_pipeline_layout(&metallib::uber_pipeline_layout(
                    &layouts, properties.is_some(),
                ));
                let make_pipeline = |shader: &dxbc::Shaders| Self::pipeline(device, &layout, shader, format, vertex_entry, Blend::SourceOver);
                let res = if properties.is_some() {
                    let source = format!("{CLIP_WGSL}\n{}\n{}", include_str!("../shaders/shader.wgsl"), crate::gpu::springs::SHADER);
                    shader_check::checked_sync(device, || {
                        let shader = dxbc::Shaders::one(device.create_shader_module(wgpu::ShaderModuleDescriptor {
                            label: Some("uber"),
                            source: wgpu::ShaderSource::Wgsl(source.into()),
                        }));
                        let pipeline = make_pipeline(&shader);
                        (shader, pipeline)
                    })
                } else {
                    metallib::uber_shader_and_pipeline(device, make_pipeline)
                };
                res.map(|(shader, pipeline)| (bgl, layout, shader, pipeline))
            }
        };
        #[cfg(target_arch = "wasm32")]
        if let Some(scopes) = web_scopes {
            let answer = shader_check::Answer::default();
            let to = answer.clone();
            let wake = uber_check_wake.clone();
            scopes.pop(move |error| {
                *to.lock().unwrap_or_else(|e| e.into_inner()) = Some(error);
                if let Some(wake) = wake.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                    wake();
                }
            });
            let _ = uber_check.step(&mut uber_check_seq, shader_check::UberCheckEvent::Started(answer));
        }
        let (bgl, layout, shader, pipeline) = built?;
        let atlas = Self::atlas_texture(device, matches!(uber, variant::Uber::Windfoil { .. }), ATLAS_START_ROWS);
        let bind_group = Self::bind_group(device, &bgl, &globals, uber.outlines(), &atlas, &stand_in, None);
        let instances = rune_staging::GrowBuffer::new(
            device,
            "instances",
            wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            std::mem::size_of::<Instance>(),
            256,
            0,
        );
        let spring_instances = properties.map_or(0, |(_, ids)| ids.len());
        let springs = properties.map(|(nodes, ids)| crate::gpu::springs::Bindings::new(device, spring_layout.as_ref().unwrap(), nodes, ids));
        Ok(WarmRenderer {
            effect_warm: Default::default(),
            first_frame_submitted: std::sync::atomic::AtomicBool::new(false),
            surfaces_building: None,
            pipeline,
            format,
            shader,
            layout,
            vertex_entry,
            extra: Vec::new(),
            bind_group,
            bind_layout: bgl,
            globals,
            atlas,
            uber,
            lcd: variant::Lcd::Unbuilt,
            pinned: None,
            pinned_extra: Vec::new(),
            pinned_building: None,
            kinds: Vec::new(),
            lcd_frame: false,
            frame_wake: None,
            stand_in,
            atlas_pages: Vec::new(),
            images: Vec::new(),
            image_batches: Vec::new(),
            instances,
            count: 0,
            springs,
            spring_instances,
            layers: Default::default(),
            srgb_blend: default_srgb_blend(format),
            effects: Vec::new(),
            backdrops: Vec::new(),
            backdrop_used: false,
            backdrop_blur: None,
            breakers: Default::default(),
            uber_check,
            uber_check_seq,
            #[cfg(target_arch = "wasm32")]
            uber_check_wake,
            uploads: Default::default(),
            fills: Default::default(),
            device_key: crate::compose::DeviceKey::next(),
            max_texture_dim: {
                let max = device.limits().max_texture_dimension_2d;
                crate::pixels::limit_side(max);
                max
            },
            #[cfg(any(target_os = "windows", target_arch = "wasm32"))]
            clear_quad: std::sync::OnceLock::new(),
            #[cfg(any(target_os = "windows", target_arch = "wasm32"))]
            damage: Default::default(),
        })
    }

    pub(super) fn pipeline(
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        shader: &dxbc::Shaders,
        format: wgpu::TextureFormat,
        vertex_entry: &str,
        blend: Blend,
    ) -> wgpu::RenderPipeline {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("uber"),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: &shader.vertex,
                entry_point: Some(vertex_entry),
                compilation_options: Default::default(),
                buffers: &[Some(metallib::uber_vertex_layout())],
            },
            primitive: metallib::uber_primitive(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader.fragment,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(super::blend_state(blend)),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    }

    pub(super) fn image_texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
        Self::texture(device, "image atlas", wgpu::TextureFormat::Rgba8UnormSrgb, width, height)
    }

    /// A sampled texture the CPU fills with `write_rows`.
    pub(super) fn texture(device: &wgpu::Device, label: &str, format: wgpu::TextureFormat, width: u32, height: u32) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    pub(super) fn bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        globals: &wgpu::Buffer,
        outlines: Option<&wgpu::Texture>,
        atlas: &wgpu::Texture,
        images: &wgpu::Texture,
        target: Option<&wgpu::Buffer>,
    ) -> wgpu::BindGroup {
        let atlas_view = atlas.create_view(&Default::default());
        let image_view = images.create_view(&Default::default());
        let outline_view = outlines.map(|t| t.create_view(&Default::default()));
        let mut entries = vec![
            wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&atlas_view) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&image_view) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: target.unwrap_or(globals),
                offset: if target.is_some() { 0 } else { u64::from(device.limits().min_uniform_buffer_offset_alignment).max(std::mem::size_of::<Globals>() as u64) },
                size: std::num::NonZeroU64::new(16),
            }) },
        ];
        // The Windfoil shader's outline store (render/variant.rs); Classic has no binding 4.
        if let Some(view) = &outline_view {
            entries.push(wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(view) });
        }
        device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout, entries: &entries })
    }

    /// The pipeline that draws into a `format` target: the layer's additive one inside a
    /// `Blend::PlusLighter` layer, else the frame's or the `add_format` one.
    pub(super) fn draw_pipeline(&self, format: wgpu::TextureFormat, target: Option<usize>) -> &wgpu::RenderPipeline {
        if self.layer_blend(target) == Blend::PlusLighter {
            &self.layers.additive.as_ref().expect("prepared dissolve").0
        } else if let (variant::Lcd::Built(lcd), true, None) = (&self.lcd, self.lcd_frame && format == self.format, target) {
            // Subpixel text blends by channel over the window; inside layers it draws grayscale.
            lcd
        } else if format == self.format {
            &self.pipeline
        } else {
            &self.extra.iter().find(|(f, _)| *f == format).expect("Renderer::add_format first").1
        }
    }

    /// Builds the pipeline for another target format (e.g. an `Rgba16Float` layer), once.
    pub fn add_format(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        if format != self.format && !self.extra.iter().any(|(f, _)| *f == format) {
            let p = Self::pipeline(device, &self.layout, &self.shader, format, self.vertex_entry, Blend::SourceOver);
            self.extra.push((format, p));
        }
        if self.layer_stats().targets > 0 { self.add_layer_format(device, format); }
        let [vw, vh] = self.target_frame(None).1;
        // A format that joins while backdrops are pooled gets its copy before the next draw, at
        // the place and size of the ones the last frame made.
        if let Some((origin, size)) = self.backdrops.first().map(|b| (b.origin, [b.texture.width(), b.texture.height()])) && vw > 0 && vh > 0 {
            self.ensure_backdrop(device, format, origin, size);
        }
        if let Some(s) = &mut self.surfaces { s.add_format(device, format); }
    }

    /// Switches the frame pass's target format (setting `srgb_blend` from `format`: `true` for
    /// `Rgba8Unorm` / `Bgra8Unorm`, `false` for `*Srgb` and floating-point targets), keeping the
    /// glyph and image atlases, offscreen layer targets and `extra` pipelines.
    pub fn set_format(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        self.try_set_format(device, format).unwrap_or_else(|error| panic!("{error}"));
    }

    /// Like [`Self::set_format`], returning an error and leaving the renderer unchanged if the
    /// GPU rejects the pipeline for `format`.
    pub(crate) fn try_set_format(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) -> Result<(), ShaderError> {
        if self.format == format { return Ok(()); }
        let next = match self.extra.iter().position(|(f, _)| *f == format) {
            Some(i) => self.extra.swap_remove(i).1,
            None => {
                #[cfg(target_arch = "wasm32")]
                let web_scopes = crate::platform::web::health::Scopes::push(device);
                let built = shader_check::checked_sync(device, || {
                    Self::pipeline(device, &self.layout, &self.shader, format, self.vertex_entry, Blend::SourceOver)
                });
                #[cfg(target_arch = "wasm32")]
                if let Some(scopes) = web_scopes {
                    let answer = shader_check::Answer::default();
                    let to = answer.clone();
                    let wake = self.uber_check_wake.clone();
                    scopes.pop(move |error| {
                        *to.lock().unwrap_or_else(|e| e.into_inner()) = Some(error);
                        if let Some(wake) = wake.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                            wake();
                        }
                    });
                    let _ = self.uber_check.step(&mut self.uber_check_seq, shader_check::UberCheckEvent::Started(answer));
                }
                built?
            }
        };
        let old = std::mem::replace(&mut self.pipeline, next);
        self.extra.push((self.format, old));
        // The pinned pipelines go with the frame pipeline: kept for the old format, made on their thread for a new one.
        if let Some(pinned) = self.pinned.take() { self.pinned_extra.push((self.format, pinned)); }
        self.format = format;
        self.lcd = variant::Lcd::Unbuilt;
        match self.pinned_extra.iter().position(|(f, _)| *f == format) {
            Some(i) => self.pinned = Some(self.pinned_extra.swap_remove(i).1),
            None => self.start_pinned(device),
        }
        self.srgb_blend = default_srgb_blend(format);
        if self.layer_stats().targets > 0 { self.add_layer_format(device, format); }
        let [vw, vh] = self.target_frame(None).1;
        // A format that joins while backdrops are pooled gets its copy before the next draw, at
        // the place and size of the ones the last frame made.
        if let Some((origin, size)) = self.backdrops.first().map(|b| (b.origin, [b.texture.width(), b.texture.height()])) && vw > 0 && vh > 0 {
            self.ensure_backdrop(device, format, origin, size);
        }
        if let Some(s) = &mut self.surfaces {
            s.set_srgb_blend(self.srgb_blend);
            s.add_format(device, format);
        }
        Ok(())
    }

    /// Wakes the web host once any async browser WebGPU error-scope check (primary shader or
    /// lazy feature breaker) resolves. Fires immediately if an answer already arrived.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn set_uber_check_wake(&self, wake: crate::Waker) {
        let already_answered = self.uber_check.has_answer() || self.breakers.has_answer();
        *self.uber_check_wake.lock().unwrap_or_else(|e| e.into_inner()) = Some(wake.clone());
        if already_answered {
            wake();
        }
    }

    /// Advances the async primary-shader check and returns the rejection reason if the browser
    /// rejected the uber shader or pipeline.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn poll_uber_check(&mut self) -> Option<String> {
        if let Some(event) = self.uber_check.answered()
            && let Some(shader_check::UberCheckEffect::Fail(reason)) =
                self.uber_check.step(&mut self.uber_check_seq, event).into_iter().next()
        {
            return Some(reason);
        }
        self.uber_check.failed_reason().map(str::to_owned)
    }
}
