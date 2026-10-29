//! `FxRenderer::prepare`: cull, pack and upload a frame's effects.

use super::*;

impl FxRenderer {
    /// Cull, pack and upload. Layer `i` of `frame.layers` can then be drawn with `draw_layer(pass, i)`.
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: &FxFrame) -> FxStats {
        self.prepare_with_slots(device, queue, frame, &[])
    }

    /// Like `prepare`, with device scale and clipping for each layer's effect slot.
    /// Entries correspond to `frame.layers`; missing entries use `FxSlot::default()`.
    pub fn prepare_with_slots(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: &FxFrame, slots: &[FxSlot]) -> FxStats {
        if let Some(srgb) = self.srgb {
            let target = if srgb { self.format.remove_srgb_suffix() } else { self.format.add_srgb_suffix() };
            self.set_format(device, target);
        }
        let mut stats = FxStats::default();
        self.scratch.clear();
        self.scratch_xf.clear();
        self.prepared.clear();
        self.binds.clear();
        let vw = frame.viewport[0] as f32;
        let vh = frame.viewport[1] as f32;
        self.viewport = frame.viewport;
        self.blur_slot = 0;
        let wants_blur = |l: &FxLayer| match l.fx {
            Fx::Distort(DistortParams::Lens(p)) => p.frost > 0.0,
            Fx::Fill(FillParams { style: FillStyle::FlutedGlass(p), .. }) => p.frost > 0.0,
            _ => false,
        };
        if frame.layers.iter().any(wants_blur) && self.blur.as_ref().is_none_or(|b| b.size != frame.viewport) {
            self.blur = Some(BlurPyramid::new(device, frame.viewport, &self.blur_bgl, &self.dummy, &self.sampler, &self.blur_uniforms));
        }
        if let Some(sims) = self.sims.as_mut() {
            sims.begin_frame();
        }
        // Bind groups keyed by (needs backdrop, source slot, blurred backdrop, fire sim grid) so equal layers share one.
        let mut keys: Vec<(bool, Option<u32>, bool, Option<usize>, bool)> = Vec::new();
        for (index, layer) in frame.layers.iter().enumerate() {
            let slot = slots.get(index).copied().unwrap_or_default();
            let Some((kind, mut inst, b)) = layer.pack() else {
                stats.culled_idle += 1;
                self.prepared.push(None);
                continue;
            };
            let s = if slot.scale > 0.0 { slot.scale } else { frame.scale };
            let x0 = (b.x * s).floor().max(slot.clip[0].floor()).max(0.0);
            let y0 = (b.y * s).floor().max(slot.clip[1].floor()).max(0.0);
            let x1 = ((b.x + b.w) * s).ceil().min(slot.clip[2].ceil()).min(vw);
            let y1 = ((b.y + b.h) * s).ceil().min(slot.clip[3].ceil()).min(vh);
            if x1 <= x0 || y1 <= y0 {
                stats.culled_offscreen += 1;
                self.prepared.push(None);
                continue;
            }
            layer.fill(&mut inst, &mut self.scratch_xf);
            let grid = match (&layer.fx, self.sims.as_mut()) {
                (Fx::Fire(p), Some(sims)) if matches!(p.style, FireStyle::Top | FireStyle::Heat) => {
                    let (g, ext) = sims.plan(device, layer, p, b, layer.reduced_motion);
                    inst.t[2] = 4.0;
                    inst.aux = [b.x, b.y, ext[0], ext[1]];
                    inst.p3[1] = b.w;
                    inst.p3[2] = b.h;
                    Some(g)
                }
                _ => None,
            };
            inst.bounds = [x0, y0, x1, y1];
            inst.clip = slot.clip;
            inst.fade = slot.fade;
            inst.shape[1] = s;
            inst.t[3] = ((inst.t[3] as u32) | (slot.blend.code() << 1)) as f32;
            let needs_bd = layer.fx.needs_backdrop() || slot.blend.needs_backdrop();
            stats.needs_backdrop |= needs_bd;
            let blur_px = match layer.fx {
                Fx::Distort(DistortParams::Lens(p)) => (p.frost.clamp(0.0, 1.0) * LENS_FROST_SIGMA * s).max(0.0),
                Fx::Fill(FillParams { style: FillStyle::FlutedGlass(p), .. }) => (p.frost.clamp(0.0, 1.0) * LENS_FROST_SIGMA * s).max(0.0),
                _ => 0.0,
            };
            let key = (needs_bd && frame.backdrop.is_some(), layer.source.map(|s| s.slot), blur_px > 0.0 && frame.backdrop.is_some(), grid, slot.backdrop_srgb.unwrap_or(false));
            let bind = match keys.iter().position(|k| *k == key) {
                Some(p) => p,
                None => {
                    let bd = if key.0 { frame.backdrop.unwrap() } else { &self.dummy };
                    let src = match (key.3, self.sims.as_ref()) {
                        (Some(g), Some(sims)) => sims.final_view(g),
                        _ => key.1.and_then(|slot| frame.sources.get(slot as usize).copied()).unwrap_or(&self.dummy),
                    };
                    let blurred = if key.2 { self.blur.as_ref().map_or(&self.dummy, |b| &b.result) } else { &self.dummy };
                    let enc_idx = (usize::from(key.0) << 1) | usize::from(key.4);
                    self.binds.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("fx1"),
                        layout: &self.bgl1,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(bd) },
                            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(src) },
                            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(blurred) },
                            wgpu::BindGroupEntry { binding: 4, resource: self.backdrop_encoding[enc_idx].as_entire_binding() },
                        ],
                    }));
                    keys.push(key);
                    keys.len() - 1
                }
            };
            stats.drawn += 1;
            stats.pixels += ((x1 - x0) * (y1 - y0)) as u64;
            self.prepared.push(Some(Prepared { kind, instance: self.scratch.len() as u32, bind, bounds: [x0, y0, x1, y1], blur_px, needs_backdrop: needs_bd,
                backdrop_srgb: slot.backdrop_srgb.unwrap_or(frame.backdrop.is_none() && self.srgb.unwrap_or(false)), blend: slot.blend }));
            self.scratch.push(inst);
        }
        let mut regrow = self.bg0.is_none();
        if self.scratch.len() > self.inst_cap {
            self.inst_cap = self.scratch.len().next_power_of_two();
            self.insts = Self::buffer(device, "fx insts", self.inst_cap * INST_SIZE);
            regrow = true;
        }
        if self.scratch_xf.len() > self.xform_cap {
            self.xform_cap = self.scratch_xf.len().next_power_of_two();
            self.xforms = Self::buffer(device, "fx xforms", self.xform_cap * 16);
            regrow = true;
        }
        if regrow {
            self.bg0 = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fx0"),
                layout: &self.bgl0,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.globals.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: self.insts.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: self.xforms.as_entire_binding() },
                ],
            }));
        }
        if !self.scratch.is_empty() {
            queue.write_buffer(&self.insts, 0, bytemuck::cast_slice(&self.scratch));
        }
        if !self.scratch_xf.is_empty() {
            queue.write_buffer(&self.xforms, 0, bytemuck::cast_slice(&self.scratch_xf));
        }
        let g = [vw, vh, if self.opts.dither { 1.0 } else { 0.0 }, if self.srgb.unwrap_or(false) { 1.0 } else { 0.0 }];
        queue.write_buffer(&self.globals, 0, bytemuck::cast_slice(&g));
        stats
    }
}
