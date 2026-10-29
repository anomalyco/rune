//! `LightRenderer::render`: upload a scene and record its lighting passes.

use super::*;

impl LightRenderer {
    /// Light `base` (the UI without text, linear, filterable) and composite `text` (premultiplied linear) over it into
    /// `target`. Returns false and records nothing when there's nothing to light.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        scene: &Scene<'_>,
        base: &wgpu::TextureView,
        text: &wgpu::TextureView,
        target: &wgpu::TextureView,
    ) -> bool {
        let any_emit = scene.prims.iter().any(|p| p.flags() & FLAG_EMISSIVE != 0) || scene.emission.is_some();
        if scene.lights.is_empty() && !(any_emit && scene.settings.gi.is_some()) {
            return false;
        }
        let s = &scene.settings;
        if scene.prims.len() > self.prim_cap {
            self.prim_cap = scene.prims.len().next_power_of_two();
            self.prims = Self::prim_buffer(device, self.prim_cap);
            self.rebind_prims(device);
        }
        if !scene.prims.is_empty() {
            self.staging.write_buffer(&self.prims, 0, bytemuck::cast_slice(scene.prims));
        }
        let nl = scene.lights.len().min(MAX_LIGHTS);
        if nl > 0 {
            self.staging.write_buffer(&self.lights, 0, bytemuck::cast_slice(&scene.lights[..nl]));
        }
        let zmax = scene.prims.iter().filter(|p| p.flags() & FLAG_OCCLUDER != 0).fold(0.0f32, |m, p| m.max(p.z[1]));
        let shafts = scene.lights.iter().any(|l| l.extra[0] > 0.0);
        let gi = s.gi.filter(|_| any_emit || s.gi.is_some_and(|g| g.bounce > 0.0 && nl > 0));
        let float_target = matches!(self.output_format, wgpu::TextureFormat::Rgba16Float | wgpu::TextureFormat::Rgba32Float);
        let encode = !self.output_format.is_srgb() && !float_target;
        let extra = gi.is_some() && scene.emission.is_some();
        let flags = gi.is_some() as u32
            | (shafts as u32) << 1
            | (encode as u32) << 2
            | ((s.contact_ao > 0.0) as u32) << 3
            | (extra as u32) << 4;
        if self.inspect && gi.is_some() {
            self.own_targets(device);
        }
        let g = gi.unwrap_or_default();
        let fu = FrameU {
            sf: [self.width as f32, self.height as f32, self.field_size.0 as f32, self.field_size.1 as f32],
            gs: [self.gi_size.0 as f32, self.gi_size.1 as f32, 0.5, GI_SCALE],
            ambient: [s.ambient[0], s.ambient[1], s.ambient[2], s.exposure],
            counts: [scene.prims.len() as u32, nl as u32, CASCADES, flags],
            misc: [zmax, s.time, s.seed, s.shafts_over],
            text: [s.text.floor, s.text.ceiling, s.text.lit, s.text.specular],
            gi: [g.strength, g.air, g.bounce, g.absorption],
            bg: [s.background_gloss, s.background_roughness, s.contact_ao, s.rim],
            ext: [s.albedo_floor, g.emission, 0.0, 0.0],
        };
        self.staging.write_buffer(&self.frame, 0, bytemuck::bytes_of(&fu));
        self.staging.flush(encoder);

        let ts = |i: u32| {
            self.query.as_ref().map(|(qs, _, _)| wgpu::ComputePassTimestampWrites {
                query_set: qs,
                beginning_of_pass_write_index: Some(i),
                end_of_pass_write_index: Some(i + 1),
            })
        };
        // Render-pass timestamps are unreliable on tile GPUs, so the two render passes are timed between the end of
        // an empty marker pass before them and the start of the compute pass (or marker) after them.
        let marker = |encoder: &mut wgpu::CommandEncoder, begin: Option<u32>, end: Option<u32>| {
            if let Some((qs, _, _)) = &self.query {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("light ts marker"),
                    timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                        query_set: qs,
                        beginning_of_pass_write_index: begin,
                        end_of_pass_write_index: end,
                    }),
                });
                pass.set_pipeline(&self.noop_pipe);
                pass.dispatch_workgroups(1, 1, 1);
            }
        };
        marker(encoder, None, Some(0));

        {
            fn clear(v: &wgpu::TextureView, c: wgpu::Color) -> wgpu::RenderPassColorAttachment<'_> {
                wgpu::RenderPassColorAttachment {
                    view: v,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(c), store: wgpu::StoreOp::Store },
                }
            }
            let gb0 = clear(&self.t.gb0, wgpu::Color { r: 1e4, g: 0.0, b: 0.0, a: 0.0 });
            let gb1 = clear(&self.t.gb1, wgpu::Color::TRANSPARENT);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("light surface"),
                color_attachments: &[Some(gb0), Some(gb1)],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if !scene.prims.is_empty() {
                pass.set_pipeline(&self.gbuf_pipe);
                pass.set_bind_group(0, &self.gbuf_bg, &[]);
                pass.draw(0..4, 0..scene.prims.len() as u32);
            }
        }
        let wg = |w: u32, h: u32| (w.div_ceil(8), h.div_ceil(8));
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("light field"), timestamp_writes: ts(1) });
            pass.set_pipeline(&self.field_pipe);
            pass.set_bind_group(0, &self.t.field_bg, &[]);
            let (x, y) = (self.field_size.0.div_ceil(16), self.field_size.1.div_ceil(16));
            pass.dispatch_workgroups(x, y, 1);
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("light shadows"), timestamp_writes: ts(3) });
            pass.set_pipeline(&self.shadow_pipe);
            pass.set_bind_group(0, &self.t.shadow_bg, &[]);
            let (x, y) = wg(self.field_size.0, self.field_size.1);
            pass.dispatch_workgroups(x, y, 1);
        }
        // GI: media, then each cascade top down, then fluence, each in its own timed pass. With GI off the passes
        // run as no-ops only when timing, so the timestamps stay valid.
        let gi_pass = |encoder: &mut wgpu::CommandEncoder, label: &str, q: u32, f: &dyn Fn(&mut wgpu::ComputePass<'_>)| {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some(label), timestamp_writes: ts(q) });
            if gi.is_some() {
                pass.set_bind_group(0, &self.gi_bg0, &[]);
                f(&mut pass);
            } else if self.query.is_some() {
                pass.set_pipeline(&self.noop_pipe);
                pass.dispatch_workgroups(1, 1, 1);
            }
        };
        let smp = wgpu::BindingResource::Sampler(&self.sampler);
        if gi.is_some() || self.query.is_some() {
            let scene_bg = gi.is_some().then(|| {
                bind(
                    device,
                    &self.scene_bgl,
                    &[
                        self.prims.as_entire_binding(),
                        self.lights.as_entire_binding(),
                        tv(&self.t.vis),
                        tv(scene.emission.unwrap_or(&self.black)),
                        smp.clone(),
                        tv(&self.t.scene_emit),
                        tv(&self.t.scene_occ),
                    ],
                )
            });
            gi_pass(encoder, "gi scene", 5, &|pass| {
                pass.set_pipeline(&self.scene_pipe);
                pass.set_bind_group(1, scene_bg.as_ref(), &[]);
                let (x, y) = wg(self.gi_size.0, self.gi_size.1);
                pass.dispatch_workgroups(x, y, 1);
            });
            for (k, bg) in self.t.cascade_bgs.iter().enumerate() {
                let n = CASCADES - 1 - k as u32;
                gi_pass(encoder, "gi cascade", 7 + 2 * n, &|pass| {
                    pass.set_pipeline(&self.cascade_pipe);
                    pass.set_bind_group(1, bg, &[]);
                    let (x, y) = wg(self.gi_size.0 * 2, self.gi_size.1 * 2);
                    pass.dispatch_workgroups(x, y, 1);
                });
            }
            let fq = 7 + 2 * CASCADES;
            gi_pass(encoder, "gi fluence", fq, &|pass| {
                pass.set_pipeline(&self.fluence_pipe);
                pass.set_bind_group(1, &self.t.fluence_bg, &[]);
                let (x, y) = wg(self.gi_size.0, self.gi_size.1);
                pass.dispatch_workgroups(x, y, 1);
            });
        }
        if self.inspect && gi.is_some() {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("gi inspect"), timestamp_writes: None });
            pass.set_bind_group(0, &self.gi_bg0, &[]);
            pass.set_pipeline(&self.cascade_pipe);
            for bg in &self.t.own_bgs {
                pass.set_bind_group(1, bg, &[]);
                let (x, y) = wg(self.gi_size.0 * 2, self.gi_size.1 * 2);
                pass.dispatch_workgroups(x, y, 1);
            }
        }
        let comp_bg = bind(
            device,
            &self.comp_bgl,
            &[
                self.frame.as_entire_binding(),
                self.prims.as_entire_binding(),
                self.lights.as_entire_binding(),
                tv(base),
                tv(text),
                tv(&self.t.gb0),
                tv(&self.t.gb1),
                tv(&self.t.vis),
                tv(&self.t.air),
                tv(&self.t.fluence),
                tv(&self.t.field),
                smp,
            ],
        );
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("light composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.comp_pipe);
            pass.set_bind_group(0, &comp_bg, &[]);
            pass.draw(0..3, 0..1);
        }
        marker(encoder, Some(QUERIES - 1), None);
        if let Some((qs, resolve, read)) = &self.query {
            encoder.resolve_query_set(qs, 0..QUERIES, resolve, 0);
            encoder.copy_buffer_to_buffer(resolve, 0, read, 0, QUERIES as u64 * 8);
        }
        true
    }
}
