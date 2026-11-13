//! Building the renderer and its uber pipelines: the pipeline for each target format and blend,
//! the sampled textures the CPU fills, and the bind group the uber shader reads.

use super::{Blend, CLIP_WGSL, Renderer, dxbc, metallib, variant};

impl Renderer {
    pub(super) fn create(device: &wgpu::Device, format: wgpu::TextureFormat, properties: Option<(&[crate::gpu::springs::Node], &[u32])>, classic: bool) -> Renderer {
        let spring_shader = if properties.is_some() {
            let source = format!("{CLIP_WGSL}\n{}\n{}", include_str!("../shader.wgsl"), crate::gpu::springs::SHADER);
            Some(device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("uber"),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            }))
        } else {
            None
        };
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(device.limits().min_uniform_buffer_offset_alignment).max(32) + 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let stand_in = Self::image_texture(device, 1, 1);
        // Subpixel text waits for the subpixel pipeline (`lcd_ready`), not just the feature.
        if !device.features().contains(wgpu::Features::DUAL_SOURCE_BLENDING) { crate::text::note_dual_source(false); }
        let spring_layout = properties.map(|_| crate::gpu::springs::Bindings::layout(device));
        let vertex_entry = if properties.is_some() { "vs_springs" } else { "vs" };
        // The shader machine starts Classic; everywhere but macOS (whose default text keeps the
        // Classic shader it always drew) and GPU springs, it asks for Windfoil at once.
        let (uber, windfoil) = if classic {
            (variant::Uber::Classic, None)
        } else {
            Self::start_windfoil(device, &variant::BuildContext { format, vertex_entry, globals: &globals, stand_in: &stand_in })
        };
        let atlas = Self::atlas_texture(device, windfoil.is_some());
        let (bgl, layout, shader, pipeline) = match windfoil {
            Some(built) => (built.bind_layout, built.layout, built.shader, built.pipeline),
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
                let (shader, pipeline) = match spring_shader {
                    Some(shader) => { let shader = dxbc::Shaders::one(shader); let pipeline = make_pipeline(&shader); (shader, pipeline) }
                    None => metallib::uber_shader_and_pipeline(device, make_pipeline),
                };
                (bgl, layout, shader, pipeline)
            }
        };
        let bind_group = Self::bind_group(device, &bgl, &globals, uber.outlines(), &atlas, &stand_in, None);
        let capacity = 256;
        let instances = Self::instance_buffer(device, capacity);
        let spring_instances = properties.map_or(0, |(_, ids)| ids.len());
        let springs = properties.map(|(nodes, ids)| crate::gpu::springs::Bindings::new(device, spring_layout.as_ref().unwrap(), nodes, ids));
        Renderer {
            surfaces: None,
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
            lcd_frame: false,
            frame_wake: None,
            stand_in,
            atlas_pages: Vec::new(),
            images: Vec::new(),
            image_batches: Vec::new(),
            instances,
            capacity,
            count: 0,
            springs,
            spring_instances,
            layers: Default::default(),
            srgb_blend: false,
            effects: Vec::new(),
            backdrops: Vec::new(),
            backdrop_blur: None,
            breakers: Default::default(),
            uploads: Default::default(),
            fills: Default::default(),
            device_key: crate::compose::DeviceKey::next(),
            #[cfg(target_os = "windows")]
            clear_quad: std::sync::OnceLock::new(),
            #[cfg(target_os = "windows")]
            damage: Default::default(),
        }
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
                    blend: Some(blend.state()),
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
                offset: if target.is_some() { 0 } else { u64::from(device.limits().min_uniform_buffer_offset_alignment).max(32) },
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
        if !self.backdrops.is_empty() && vw > 0 && vh > 0 {
            self.ensure_backdrop(device, format, (vw, vh));
        }
        if let Some(s) = &mut self.surfaces { s.add_format(device, format); }
    }

    /// Switches the frame pass's target format (`!format.is_srgb()` turns `srgb_blend` on),
    /// keeping the glyph and image atlases, offscreen layer targets and `extra` pipelines.
    pub fn set_format(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        if self.format == format { return; }
        let next = match self.extra.iter().position(|(f, _)| *f == format) {
            Some(i) => self.extra.swap_remove(i).1,
            None => Self::pipeline(device, &self.layout, &self.shader, format, self.vertex_entry, Blend::SourceOver),
        };
        let old = std::mem::replace(&mut self.pipeline, next);
        self.extra.push((self.format, old));
        self.format = format;
        self.lcd = variant::Lcd::Unbuilt;
        self.srgb_blend = !format.is_srgb();
        if self.layer_stats().targets > 0 { self.add_layer_format(device, format); }
        let [vw, vh] = self.target_frame(None).1;
        if !self.backdrops.is_empty() && vw > 0 && vh > 0 {
            self.ensure_backdrop(device, format, (vw, vh));
        }
        if let Some(s) = &mut self.surfaces {
            s.set_srgb_blend(self.srgb_blend);
            s.add_format(device, format);
        }
    }
}
