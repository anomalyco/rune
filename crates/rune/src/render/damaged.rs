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

/// How `draw_armed` updated the kept frame.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeptDraw {
    /// The kept frame was already current, so only the armed rectangles were redrawn and the
    /// swap chain may present only those rectangles (`Present1` dirty rects).
    Partial,
    /// The kept frame was behind (first frame, or the previous frame drew whole — e.g. with a
    /// native surface, blur, effect or turned layer): the whole frame was drawn into the kept
    /// frame to bring it current, so the swap chain must present the whole window on this frame.
    Whole,
}

/// What the armed rectangles came to.
pub(crate) type Drawn = Result<KeptDraw, &'static str>;

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
    /// For each armed rectangle, the instances whose quads reach it (`arm_cycle`), when only
    /// those are drawn there; empty: every instance is.
    runs: Vec<Vec<std::ops::Range<u32>>>,
    /// The paint and target size `arm_cycle` last found its rectangles and runs for, and them.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    cycle: Option<((u64, (u32, u32)), Result<Cull, &'static str>)>,
    /// The paint and target size `arm_loop` last found the units of, and them.
    #[cfg(target_os = "windows")]
    loop_units: Option<((u64, (u32, u32)), Result<std::sync::Arc<Units>, &'static str>)>,
    /// Whether a run of the armed rectangles holds a layer's scope.
    layers_reached: bool,
    clear: [f32; 4],
    drawn: Option<Drawn>,
    /// The armed rectangles are a loop-only frame's (`arm_loop`), for `RUNE_LOOP_CHECK=2`.
    #[cfg(target_os = "windows")]
    pub(super) looped: bool,
    #[cfg(target_os = "windows")]
    pub(super) pixel: Option<super::pixel_check::Check>,
}

impl Renderer {
    /// The next `draw` may redraw only `rects` (`[x, y, width, height]` each, target pixels, at
    /// most `MAX_RECTS`) of a `size` drawable of `format`, cleared to `clear`.
    pub(crate) fn arm_damage(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, rects: &[[u32; 4]], clear: [f32; 4], size: (u32, u32), format: wgpu::TextureFormat) {
        let mut d = self.damage.lock().unwrap();
        // The clearing quad blends: only an opaque clear stands for `LoadOp::Clear`.
        if clear[3] != 1.0 {
            d.armed.clear();
            d.drawn = Some(Err("clear"));
            return;
        }
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
        d.runs.clear();
        d.clear = clear;
        d.drawn = Some(Err("drawn by the compositor"));
    }

    /// A loop-only frame (`runtime/paint/cycle.rs`): only the instances at `slots` changed since
    /// the frame the kept texture holds, and only in colour. Arms the rectangles they draw in,
    /// each with only the instances whose quads reach it (what the rest drew there stays as
    /// drawn). `Err` says why this frame draws whole.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn arm_cycle(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, paint: u64, instances: &[Instance], slots: &[u32], clear: [f32; 4], size: (u32, u32), format: wgpu::TextureFormat) -> Result<(), &'static str> {
        // The clearing quad blends: only an opaque clear stands for `LoadOp::Clear`.
        if clear[3] != 1.0 { return Err("clear"); }
        if slots.is_empty() { return Err("no slots"); }
        self.redraws_in_place()?;
        let key = (paint, size);
        let cached = self.damage.lock().unwrap().cycle.as_ref().filter(|c| c.0 == key).map(|c| c.1.clone());
        let (rects, runs, layers) = match cached {
            Some(c) => c?,
            None => {
                let c = cycle_cull(instances, slots, size);
                self.damage.lock().unwrap().cycle = Some((key, c.clone()));
                c?
            }
        };
        self.arm_damage(device, queue, &rects, clear, size, format);
        let mut d = self.damage.lock().unwrap();
        d.runs = runs;
        d.layers_reached = layers;
        Ok(())
    }

    /// A window's loop-only frame (`Runtime::loop_step`): only the instances at `patched`
    /// (ascending) changed since the frame the kept texture and the instance buffer hold, both
    /// paint `paint`'s. Arms the pixels they drew on and draw on now, each with only the instances
    /// that reach it (what the rest drew there stays as drawn). Returns those rectangles (x0, y0,
    /// x1, y1) to present, or why this frame draws whole.
    #[cfg(target_os = "windows")]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn arm_loop(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, paint: u64, instances: &[Instance], patched: &[u32], clear: [f32; 4], size: (u32, u32), format: wgpu::TextureFormat) -> Result<Vec<[i32; 4]>, &'static str> {
        // The clearing quad blends: only an opaque clear stands for `LoadOp::Clear`.
        if clear[3] != 1.0 { return Err("clear"); }
        self.redraws_in_place()?;
        let held = self.held_instances(paint, instances.len()).ok_or("buffer")?;
        let at = |i: &u32| *i as usize;
        // Only what differs from what the buffer holds, which is what the last frame drew (a
        // frame that missed its drawable uploaded nothing): a stepped spinner's dots between their
        // steps, while the shine drives frames, aren't redrawn; a dot a missed frame stepped is.
        let patched: Vec<u32> = patched.iter().copied().filter(|&i| bytemuck::bytes_of(&held[i as usize]) != bytemuck::bytes_of(&instances[i as usize])).collect();
        let patched = &patched[..];
        let mut rects = gather(patched.iter().map(at).flat_map(|i| [&held[i], &instances[i]]), size);
        // Nothing on screen changed: one pixel keeps the present, which DWM composes again as is.
        if rects.is_empty() { rects.push([0, 0, 1, 1]); }
        let key = (paint, size);
        let cached = self.damage.lock().unwrap().loop_units.as_ref().filter(|c| c.0 == key).map(|c| c.1.clone());
        let units = match cached {
            Some(units) => units?,
            None => {
                let units = units(instances, size).map(std::sync::Arc::new);
                self.damage.lock().unwrap().loop_units = Some((key, units.clone()));
                units?
            }
        };
        let (draw, runs, layers) = cull_runs(instances, &units, &rects, patched);
        self.arm_damage(device, queue, &draw, clear, size, format);
        if super::pixel_check::on() { self.pixel_check_arm(device, size, format); }
        let mut d = self.damage.lock().unwrap();
        d.runs = runs;
        d.layers_reached = layers;
        Ok(rects)
    }

    /// The armed frame redraws its rectangles over a current kept frame, and no layer is among
    /// what it draws there: `prepare` leaves the layers as the last frame planned and drew them.
    pub(super) fn cycle_leaves_layers(&self) -> bool {
        let d = self.damage.lock().unwrap();
        !d.armed.is_empty() && !d.runs.is_empty() && !d.layers_reached && d.retained.as_ref().is_some_and(|r| r.current)
    }

    /// After every frame: whether it redrew only the armed rectangles, or why not. A frame that
    /// drew any other way leaves the kept frame behind.
    pub(crate) fn damage_drawn(&self) -> Drawn {
        let mut d = self.damage.lock().unwrap();
        d.armed.clear();
        #[cfg(target_os = "windows")]
        {
            d.looped = false;
            if let Some(c) = d.pixel.as_mut() { c.frame_ended(); }
        }
        let drawn = d.drawn.take().unwrap_or(Err("not armed"));
        if drawn.is_err() && let Some(r) = d.retained.as_mut() { r.current = false; }
        drawn
    }

    /// Whether this frame can redraw only part of its target: nothing draws there but its own
    /// instances (no turned or blurred layer, effect slot, backdrop blur, native surface or GPU
    /// spring moves pixels the instances don't say).
    pub(super) fn redraws_in_place(&self) -> Result<(), &'static str> {
        if !self.effects.is_empty() { return Err("effect"); }
        if self.has_backdrop_blur() { return Err("backdrop blur"); }
        if self.springs.is_some() { return Err("springs"); }
        if !self.layers.in_place() { return Err("layer"); }
        if self.surfaces.as_ref().is_some_and(|s| !s.draws_nothing()) { return Err("surface"); }
        Ok(())
    }

    /// Rectangles are armed for the next `draw` (asked before it looks up its target's texture).
    pub(super) fn armed(&self) -> bool {
        !self.damage.lock().unwrap().armed.is_empty()
    }

    /// `draw`'s part, when armed: redraws the armed rectangles into the kept frame (all of it
    /// when it's behind) and copies it to `dst`. False when `draw` should draw whole.
    pub(crate) fn draw_armed(&self, encoder: &mut wgpu::CommandEncoder, dst: &wgpu::Texture) -> bool {
        let mut guard = self.damage.lock().unwrap();
        let d = &mut *guard;
        #[cfg(target_os = "windows")]
        let looped = std::mem::take(&mut d.looped);
        if d.armed.is_empty() { return false; }
        if d.clear[3] != 1.0 {
            d.armed.clear();
            d.drawn = Some(Err("clear"));
            return false;
        }
        if let Err(why) = self.redraws_in_place() {
            d.drawn = Some(Err(why));
            return false;
        }
        let rects = std::mem::take(&mut d.armed);
        let runs = std::mem::take(&mut d.runs);
        let layers_reached = d.layers_reached;
        let clear = d.clear;
        let (Some(quads), Some(kept)) = (self.clear_quad.get(), d.retained.as_mut()) else { return false };
        if (dst.width(), dst.height()) != (kept.texture.width(), kept.texture.height()) {
            d.drawn = Some(Err("size"));
            return false;
        }
        let partial = kept.current;
        if kept.current {
            // No layer reaches the rectangles: their targets aren't drawn from, nor made again.
            // The uploads go into this encoder ahead of the pass either way (`encode_layers_timed`
            // flushes them too), not in a submission of their own: a submit costs the CPU about as
            // much as the rest of a loop-only frame.
            if runs.is_empty() || layers_reached { self.encode_layers_timed(encoder, None, &mut |_, _| {}); } else { self.flush_uploads(encoder); }
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
            // frame-profile, `RUNE_LOOP_CHECK_SPOIL=1`: a loop-only frame skips its redraw, so the kept
            // picture goes stale and `RUNE_LOOP_CHECK=2` must find it (its failing row).
            #[cfg(all(target_os = "windows", feature = "frame-profile"))]
            let spoil = looped && super::pixel_check::spoiled();
            #[cfg(not(all(target_os = "windows", feature = "frame-profile")))]
            let spoil = false;
            for (n, &rect) in rects.iter().enumerate().filter(|_| !spoil) {
                let [x, y, w, h] = rect;
                pass.set_scissor_rect(x, y, w, h);
                pass.set_pipeline(self.draw_pipeline(self.format, None));
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, quads.slice(..));
                pass.draw(0..4, n as u32..n as u32 + 1);
                let mut active = ActivePass::borrowed_within(&mut pass, rect);
                active.set_scissor(None, size);
                match runs.get(n) {
                    // Only what reaches the rectangle (`arm_cycle`): none of the rest draws there.
                    Some(runs) => for run in runs { self.draw_layer_range(&mut active, self.format, run.clone(), None, &mut |_, _| {}); },
                    None => self.draw_layer_range(&mut active, self.format, 0..self.count, None, &mut |_, _| {}),
                }
            }
        } else {
            // Behind: this frame draws whole, into the kept frame.
            self.draw_timed(encoder, &kept.view, clear, None);
            kept.current = true;
        }
        // `RUNE_LOOP_CHECK=2`: this loop-only frame's picture, and the whole frame's beside it.
        #[cfg(target_os = "windows")]
        if looped && partial && let Some(c) = d.pixel.as_mut() { self.pixel_check_encode(encoder, c, &kept.texture, clear, &rects); }
        let whole = wgpu::Extent3d { width: dst.width(), height: dst.height(), depth_or_array_layers: 1 };
        encoder.copy_texture_to_texture(kept.texture.as_image_copy(), dst.as_image_copy(), whole);
        d.drawn = Some(Ok(if partial { KeptDraw::Partial } else { KeptDraw::Whole }));
        true
    }
}

/// A loop-only frame's rectangles, the runs drawn in each, and whether a layer is among them.
type Cull = (Vec<[u32; 4]>, Vec<Vec<std::ops::Range<u32>>>, bool);

/// The rectangles a loop-only frame redraws (each slot's quad 2 px wider, merged with those it
/// comes within 8 px of, `MAX_RECTS` at most) and, for each, runs of what reaches it, a pixel
/// wider still: an instance, or a layer's whole scope (its own bounds and all it holds), which
/// `draw_layer_range` draws as one.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn cycle_cull(instances: &[Instance], slots: &[u32], size: (u32, u32)) -> Result<Cull, &'static str> {
    let quads = slots.iter().map(|&s| instances.get(s as usize).ok_or("slot")).collect::<Result<Vec<_>, _>>()?;
    let rects = gather(quads.into_iter(), size);
    if rects.is_empty() { return Err("nothing drawn"); }
    let units = units(instances, size)?;
    Ok(cull_runs(instances, &units, &rects, &[]))
}

/// The pixels `quads` draw on, each 2 px wider, merged with those it comes within 8 px of,
/// `MAX_RECTS` at most (x0, y0, x1, y1).
fn gather<'a>(quads: impl Iterator<Item = &'a Instance>, size: (u32, u32)) -> Vec<[i32; 4]> {
    let (w, h) = (size.0 as i32, size.1 as i32);
    let empty = |b: &[i32; 4]| b[2] <= b[0] || b[3] <= b[1];
    let near = |a: &[i32; 4], b: &[i32; 4]| a[0] <= b[2] + 8 && b[0] <= a[2] + 8 && a[1] <= b[3] + 8 && b[1] <= a[3] + 8;
    let union = |a: [i32; 4], b: [i32; 4]| if empty(&a) { b } else if empty(&b) { a } else { [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])] };
    let mut rects: Vec<[i32; 4]> = Vec::new();
    for q in quads {
        let b = q.pixel_bounds(w, h);
        if empty(&b) { continue; }
        let mut b = [(b[0] - 2).max(0), (b[1] - 2).max(0), (b[2] + 2).min(w), (b[3] + 2).min(h)];
        while let Some(k) = rects.iter().position(|r| near(r, &b)) { b = union(b, rects.swap_remove(k)); }
        rects.push(b);
    }
    while rects.len() > MAX_RECTS {
        let (Some(a), Some(b)) = (rects.pop(), rects.pop()) else { break };
        rects.push(union(a, b));
    }
    rects
}

/// What a frame draws, in units: an instance, or a layer's whole scope (its own bounds and all it
/// holds), which `draw_layer_range` draws as one, each with its pixel bounds.
type Units = Vec<(std::ops::Range<u32>, [i32; 4])>;

fn units(instances: &[Instance], size: (u32, u32)) -> Result<Units, &'static str> {
    let (w, h) = (size.0 as i32, size.1 as i32);
    let bounds = |i: &Instance| i.pixel_bounds(w, h);
    let empty = |b: &[i32; 4]| b[2] <= b[0] || b[3] <= b[1];
    let union = |a: [i32; 4], b: [i32; 4]| if empty(&a) { b } else if empty(&b) { a } else { [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])] };
    let mut units: Units = Vec::new();
    let mut n = 0;
    while n < instances.len() {
        let start = n;
        let mut b = bounds(&instances[n]);
        if instances[n].kind() == Some(Kind::Layer) {
            let mut depth = 1;
            while depth > 0 {
                n += 1;
                let i = instances.get(n).ok_or("open layer")?;
                b = union(b, bounds(i));
                match i.kind() {
                    Some(Kind::Layer) => depth += 1,
                    Some(Kind::LayerEnd) => depth -= 1,
                    _ => {}
                }
            }
        }
        n += 1;
        units.push((start as u32..n as u32, b));
    }
    Ok(units)
}

/// For each of `rects`, runs of the units that reach it, a pixel wider still, and of any unit
/// holding one of `always` (ascending): instances whose bounds were found before they moved.
fn cull_runs(instances: &[Instance], units: &Units, rects: &[[i32; 4]], always: &[u32]) -> Cull {
    let empty = |b: &[i32; 4]| b[2] <= b[0] || b[3] <= b[1];
    let holds = |u: &std::ops::Range<u32>| always.get(always.partition_point(|&a| a < u.start)).is_some_and(|&a| a < u.end);
    let runs: Vec<Vec<std::ops::Range<u32>>> = rects.iter().map(|r| {
        let mut runs: Vec<std::ops::Range<u32>> = Vec::new();
        for (unit, b) in units {
            let reaches = !empty(b) && b[0] - 1 < r[2] && r[0] < b[2] + 1 && b[1] - 1 < r[3] && r[1] < b[3] + 1;
            if !reaches && !holds(unit) { continue; }
            // A short gap is drawn through: what's in it doesn't reach the rectangle, and one
            // draw costs less than two.
            match runs.last_mut() {
                Some(run) if unit.start - run.end <= 16 => run.end = unit.end,
                _ => runs.push(unit.clone()),
            }
        }
        runs
    }).collect();
    let layers = units.iter().any(|(u, _)| instances[u.start as usize].kind() == Some(Kind::Layer) && runs.iter().flatten().any(|r| r.start <= u.start && u.end <= r.end));
    (rects.iter().map(|r| [r[0] as u32, r[1] as u32, (r[2] - r[0]) as u32, (r[3] - r[1]) as u32]).collect(), runs, layers)
}

#[cfg(test)]
#[path = "damaged_tests.rs"]
mod tests;