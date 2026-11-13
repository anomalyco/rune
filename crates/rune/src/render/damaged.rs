//! Redrawing part of a frame: the renderer keeps the window's last frame in a texture of its own
//! (`Retained`), redraws only the rectangles that changed into it, and copies it to the
//! drawable. wgpu treats a drawable as holding nothing when it's acquired (a `LoadOp::Load` into
//! it loads zeros), so the frame can't be drawn over the drawable itself; the copy costs the GPU
//! a few hundred microseconds where the whole frame's raster costs milliseconds.
//!
//! The window arms the rectangles before the frame draws (`arm_damage`); `draw` (and so a
//! compositor's `Frame::draw_ui`) redraws only them when nothing but the instances draws there,
//! and the window reads back whether it did (`damage_drawn`), every frame: any frame drawn
//! another way leaves the kept texture behind, and the next armed one draws it whole. Every
//! instance still goes through the pass, each scissor kept inside the rectangle
//! (`ActivePass::borrowed_within`), so what lands there is exactly what a whole frame draws there.

use super::*;

/// Rectangles a frame redraws at most.
pub(crate) const MAX_RECTS: usize = 4;

/// What the armed rectangles came to.
pub(super) type Drawn = Result<(), &'static str>;

/// The window's last frame, and whether it's current.
pub(super) struct Retained {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    current: bool,
}

/// The renderer's side of a partial frame.
#[derive(Default)]
pub(super) struct DamageState {
    retained: Option<Retained>,
    armed: Vec<[u32; 4]>,
    clear: [f32; 4],
    drawn: Option<Drawn>,
}

impl Renderer {
    /// The next `draw` may redraw only `rects` (`[x, y, width, height]` each, target pixels, at
    /// most `MAX_RECTS`) of a `size` drawable of `format`, cleared to `clear`.
    pub(crate) fn arm_damage(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, rects: &[[u32; 4]], clear: [f32; 4], size: (u32, u32), format: wgpu::TextureFormat) {
        let mut d = self.damage.lock().unwrap();
        if d.retained.as_ref().is_none_or(|r| (r.texture.width(), r.texture.height()) != size || r.texture.format() != format) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("last frame"),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[self.format],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor { format: Some(self.format), ..Default::default() });
            d.retained = Some(Retained { texture, view, current: false });
        }
        let fills: Vec<Instance> = rects.iter().take(MAX_RECTS).map(|&[x, y, w, h]| Instance {
            rect: [x as f32, y as f32, w as f32, h as f32],
            radii: [0.0; 4],
            fill: clear,
            border: [0.0; 4],
            params: [0.0; 4],
            uv: [0.0; 4],
            clip: [x as f32, y as f32, (x + w) as f32, (y + h) as f32],
            fade: [0; 4],
        }).collect();
        let quads = self.clear_quad.get_or_init(|| device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("damage clear"),
            size: (std::mem::size_of::<Instance>() * MAX_RECTS) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        // Through the renderer's staging, as a layer's quad: `Queue::write_buffer` makes a buffer each call.
        if !fills.is_empty() { self.uploads.staging(device, queue).write_buffer(quads, 0, bytemuck::cast_slice(&fills)); }
        d.armed = rects.iter().take(MAX_RECTS).copied().collect();
        d.clear = clear;
        d.drawn = Some(Err("drawn by the compositor"));
    }

    /// After every frame: whether it redrew only the armed rectangles, or why not. A frame that
    /// drew any other way leaves the kept frame behind.
    pub(crate) fn damage_drawn(&self) -> Drawn {
        let mut d = self.damage.lock().unwrap();
        d.armed.clear();
        let drawn = d.drawn.take().unwrap_or(Err("not armed"));
        if drawn.is_err() && let Some(r) = d.retained.as_mut() { r.current = false; }
        drawn
    }

    /// Whether this frame can redraw only part of its target: nothing draws there but its own
    /// instances (no turned or blurred layer, effect slot, backdrop blur, native surface or GPU
    /// spring moves pixels the instances don't say).
    pub(super) fn redraws_in_place(&self) -> Drawn {
        if !self.effects.is_empty() { return Err("effect"); }
        if self.has_backdrop_blur() { return Err("backdrop blur"); }
        if self.springs.is_some() { return Err("springs"); }
        if !self.layers.in_place() { return Err("layer"); }
        if self.surfaces.as_ref().is_some_and(|s| !s.draws_nothing()) { return Err("surface"); }
        Ok(())
    }

    /// `draw`'s part, when armed: redraws the armed rectangles into the kept frame (all of it
    /// when it's behind) and copies it to `view`'s texture. False when `draw` should draw whole.
    pub(super) fn draw_armed(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) -> bool {
        let mut d = self.damage.lock().unwrap();
        if d.armed.is_empty() { return false; }
        if let Err(why) = self.redraws_in_place() {
            d.drawn = Some(Err(why));
            return false;
        }
        let rects = std::mem::take(&mut d.armed);
        let clear = d.clear;
        let (Some(quads), Some(kept)) = (self.clear_quad.get(), d.retained.as_mut()) else { return false };
        let dst = view.texture();
        if (dst.width(), dst.height()) != (kept.texture.width(), kept.texture.height()) {
            d.drawn = Some(Err("size"));
            return false;
        }
        if kept.current {
            self.encode_layers_timed(encoder, None, &mut |_, _| {});
            self.submit_uploads();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame (damage)"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &kept.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let size = [dst.width(), dst.height()];
            for (n, &rect) in rects.iter().enumerate() {
                let [x, y, w, h] = rect;
                pass.set_scissor_rect(x, y, w, h);
                pass.set_pipeline(self.draw_pipeline(self.format, None));
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, quads.slice(..));
                pass.draw(0..4, n as u32..n as u32 + 1);
                let mut active = ActivePass::borrowed_within(&mut pass, rect);
                active.set_scissor(None, size);
                self.draw_layer_range(&mut active, self.format, 0..self.count, None, &mut |_, _| {});
            }
        } else {
            // Behind: this frame draws whole, into the kept frame.
            self.draw_timed(encoder, &kept.view, clear, None);
            kept.current = true;
        }
        let whole = wgpu::Extent3d { width: dst.width(), height: dst.height(), depth_or_array_layers: 1 };
        encoder.copy_texture_to_texture(kept.texture.as_image_copy(), dst.as_image_copy(), whole);
        d.drawn = Some(Ok(()));
        true
    }
}
