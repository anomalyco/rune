//! The compositor: Rune's instances (tagged by `El::mark`) become rune-light occluders and anchor
//! rune-fx layers. One HDR frame, one resolve into the window or the offscreen target.
//!
//! Per frame: partition the instances (lit canvases → base/text/prims, a moving card → its node
//! layer, masks → the mask pyramid, everything else → the main list), light the lit canvases,
//! then draw the main list into the HDR scene with each effect layer inserted under or over the
//! node it belongs to (copying the backdrop before distortion), and resolve with dither.

mod gpu;
mod scenes;

use std::collections::HashMap;
use std::time::Instant;

use rune::compose::{Compositor, Frame, is_glyph};
use rune::render::{Instance, KIND_GRADIENT, KIND_LAYER, KIND_LAYER_END, KIND_RECT};
use rune_fx::{BlurMode, Fx, FxFrame, FxLayer, MotionBlurParams, MotionSamples, Pose, Rect, Shape};
use rune_light::{Light, LightRenderer, Prim, Scene as LightScene, Settings};
use rune_motion::Vec2;

use crate::knobs::{ALL, FX_COLORS, Scene};
use crate::stage::{self, mark_material, mark_node, mark_scene, node};
use crate::{Grab, Lab};

use gpu::{HDR, Res, Timer, pass, pass_ts};
use scenes::{Live, PrimCx, prim};

pub(self) const SRC_MASK: u32 = 0;
const SRC_NODE: u32 = 1;

#[derive(Clone, Copy)]
enum Anchor {
    Under(u8),
    Over(u8),
    /// Over everything in the canvas.
    Top,
}

struct Lay {
    layer: FxLayer,
    scene: Scene,
    anchor: Anchor,
}

/// One canvas on screen this frame.
struct Cv<'a> {
    s: &'a Lab,
    sc: Scene,
    t: f32,
    now: f64,
    scale: f32,
    origin: [f32; 2],
    /// Canvas opacity (presence): lights and effects scale with it.
    a: f32,
    pointer: Option<[f32; 2]>,
    reduced: bool,
    viewport: Rect,
}

impl Cv<'_> {
    fn k(&self, id: &str) -> f32 {
        self.s.k(self.sc, id)
    }
    fn on(&self, id: &str) -> bool {
        self.s.on(self.sc, id)
    }
    /// Canvas points → window points.
    fn w(&self, p: [f32; 2]) -> [f32; 2] {
        [self.origin[0] + p[0], self.origin[1] + p[1]]
    }
    /// Canvas points → device pixels.
    fn px(&self, p: [f32; 2]) -> [f32; 2] {
        let q = self.w(p);
        [q[0] * self.scale, q[1] * self.scale]
    }
    fn rounded(&self, r: Rect, radius: f32) -> Shape {
        Shape::rounded(self.origin[0] + r.x, self.origin[1] + r.y, r.w, r.h, radius)
    }
    /// The whole viewport; the stage scissor clips it, so full-stage effects don't stop at the canvas edge.
    fn everywhere(&self) -> Shape {
        let v = self.viewport;
        Shape::rounded(v.x, v.y, v.w, v.h, 0.0)
    }
    fn capsule(&self, r: Rect) -> Shape {
        Shape::capsule(self.origin[0] + r.x, self.origin[1] + r.y, r.w, r.h)
    }
    fn circle(&self, c: [f32; 3]) -> Shape {
        let q = self.w([c[0], c[1]]);
        Shape::circle(q[0], q[1], c[2])
    }
    fn swatch(&self) -> u32 {
        FX_COLORS[(self.k("color") as usize).min(FX_COLORS.len() - 1)]
    }
}

#[derive(Default)]
pub struct LabGfx {
    res: Option<Res>,
    live: Live,
    animating: bool,
    view_pending: bool,
    cpu: (f64, u32),
    gpu: (f64, u32),
    last_push: Option<f64>,
    shown: Option<(i32, Option<i32>, bool)>,
    last_pointer: Option<(f32, f32)>,
    total_gpu: (f64, u32),
}

impl Compositor<Lab> for LabGfx {
    fn features(&self) -> wgpu::Features {
        wgpu::Features::TIMESTAMP_QUERY
    }

    fn animating(&self) -> bool {
        self.animating || self.view_pending
    }

    fn summary(&mut self, _device: &wgpu::Device) -> Option<String> {
        let (sum, n) = self.total_gpu;
        Some(if n > 0 {
            format!("lab: gpu frame mean {:.3} ms over {n} timed frames (timestamp markers around the whole compositor)", sum / n as f64)
        } else {
            "lab: no GPU timestamps (adapter lacks TIMESTAMP_QUERY or too few frames)".to_owned()
        })
    }

    fn render(&mut self, f: &mut Frame<'_, Lab>) {
        let started = Instant::now();
        let rebuilt = self.res.as_ref().is_none_or(|r| r.size != f.size || r.format != f.format);
        if rebuilt {
            self.res = Some(Res::new(f.device, f.queue, f.size, f.format, f.ui));
        }
        self.view_pending = false;
        let _ = f.device.poll(wgpu::PollType::Poll);
        let res = self.res.as_mut().unwrap();
        let gpu_ms = res.timer.as_mut().and_then(|t| t.begin(f.encoder));
        let rt = &mut *f.rt;
        let scale = rt.env.scale;
        let now = rt.t;
        let t = now as f32;
        let (w, h) = f.size;
        let vp = Rect::new(0.0, 0.0, w as f32 / scale, h as f32 / scale);
        let stage_r = rt.presented("stage").map_or([vp.x, vp.y, vp.w, vp.h], |(r, _)| r);
        let in_stage = |p: (f32, f32)| p.0 >= stage_r[0] && p.0 <= stage_r[0] + stage_r[2] && p.1 >= stage_r[1] && p.1 <= stage_r[1] + stage_r[3];
        let pointer = rt.pointer().filter(|p| in_stage(*p));
        let canvases: Vec<(Scene, [f32; 2], f32)> = ALL
            .iter()
            .filter_map(|sc| rt.presented(&format!("canvas-{}", sc.id())).map(|(r, a)| (*sc, [r[0], r[1]], a)))
            .filter(|c| c.2 > 0.001)
            .collect();
        let s = &rt.state;
        let mut anim = false;

        // ---- lights and effect layers per visible canvas
        let mut lights: Vec<Light> = Vec::new();
        let mut settings: Option<(f32, Settings)> = None;
        let mut lit_alpha = 0.0f32;
        let mut pc = PrimCx::default();
        let mut lays: Vec<Lay> = Vec::new();
        for &(sc, origin, a) in &canvases {
            let cv = Cv {
                s,
                sc,
                t,
                now,
                scale,
                origin,
                a,
                pointer: pointer.map(|p| [p.0 - origin[0], p.1 - origin[1]]),
                reduced: s.reduced,
                viewport: vp,
            };
            if sc.lit() {
                lit_alpha = lit_alpha.max(a);
                let st = self.live.lights(&cv, &mut lights, &mut pc, &mut anim);
                if settings.as_ref().is_none_or(|(sa, _)| a > *sa) {
                    settings = Some((a, st));
                }
            } else {
                self.live.layers(&cv, &mut lays, &mut anim);
            }
        }
        for l in &lays {
            anim |= l.layer.is_animating();
        }

        // ---- the motion card: rest rect (where Rune drew it) and its pose along the track
        let card_drawn = rt.presented("card").map(|(r, _)| r);
        let mut card_blur: Option<(MotionBlurParams, MotionSamples, Rect)> = None;
        let mut card_off = [0.0f32; 2];
        if let (Some(r), Some(&(_, origin, _))) = (card_drawn, canvases.iter().find(|c| c.0 == Scene::Motion)) {
            let rest = Rect::new(r[0], r[1], r[2], r[3]);
            let dragging = matches!(s.grab, Some(Grab::Card(_)));
            let pose_at = |tt: f32| -> Pose {
                let c = if dragging {
                    let c = s.card.target();
                    let v = s.card_vt.velocity();
                    let dt = (tt as f64 - now) as f32;
                    Vec2 { x: c.x + v.x * dt, y: c.y + v.y * dt }
                } else {
                    s.card.sample(tt as f64)
                };
                Pose { x: origin[0] + c.x - rest.x, y: origin[1] + c.y - rest.y, ..Default::default() }
            };
            let p = MotionBlurParams {
                mode: if s.on(Scene::Motion, "directional") { BlurMode::Directional } else { BlurMode::Shutter },
                shutter_angle: if s.reduced { 0.0 } else { s.k(Scene::Motion, "shutter_angle") },
                smooth_shutter: s.on(Scene::Motion, "smooth"),
                ..Default::default()
            };
            let samples = MotionSamples::from_track(&p, rest, t, scale, pose_at);
            let pm = pose_at(t);
            card_off = [pm.x * scale, pm.y * scale];
            anim |= dragging || !s.card.is_settled(now);
            if samples.is_moving() {
                card_blur = Some((p, samples, rest));
            }
        }
        if let Some((p, samples, rest)) = card_blur {
            let layer = Fx::motion_blur(p, samples).on(Shape::rounded(rest.x, rest.y, rest.w, rest.h, 14.0)).source(SRC_NODE, vp).at(t);
            lays.push(Lay { layer, scene: Scene::Motion, anchor: Anchor::Over(node::CARD) });
        }

        // ---- partition Rune's instances
        let inst = &rt.instances;
        let marks = &rt.marks;
        let mut main: Vec<Instance> = Vec::with_capacity(inst.len());
        let (mut base, mut text, mut nodel, mut mask) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut prims: Vec<Prim> = Vec::new();
        let mut first_lit: Option<usize> = None;
        let mut first: HashMap<(usize, u8), usize> = HashMap::new();
        let mut last: HashMap<(usize, u8), usize> = HashMap::new();
        let mut top: HashMap<usize, usize> = HashMap::new();
        let mut stage_fill = [0.0, 0.0, 0.0, 1.0];
        let needs_mask = lays.iter().any(|l| l.layer.source.is_some_and(|s| s.slot == SRC_MASK));
        // This demo owns a different effect graph: it splits glyphs from their backgrounds
        // and inserts effects between individual nodes. Preserve its existing flattened
        // opacity semantics until that graph can composite an entire scope after lighting.
        // Ordinary Rune compositors instead preserve scopes and call encode_layers.
        let mut opacity = 1.0;
        let mut opacity_stack = Vec::new();
        for (i, ins) in inst.iter().enumerate() {
            if ins.params[1] == KIND_LAYER {
                opacity_stack.push(opacity);
                opacity *= ins.fill[3];
                continue;
            }
            if ins.params[1] == KIND_LAYER_END {
                opacity = opacity_stack.pop().expect("balanced Rune layer scopes");
                continue;
            }
            let mut flattened = *ins;
            flattened.fill = flattened.fill.map(|v| v * opacity);
            flattened.border = flattened.border.map(|v| v * opacity);
            if flattened.params[1] == KIND_GRADIENT { flattened.uv = flattened.uv.map(|v| v * opacity); }
            let ins = &flattened;
            let m = marks.get(i).copied().unwrap_or(0);
            if m == stage::STAGE_MARK && ins.params[1] == KIND_RECT {
                stage_fill = ins.fill;
            }
            let sc = mark_scene(m);
            let nd = mark_node(m);
            if let Some(sc) = sc {
                first.entry((sc.idx(), nd)).or_insert(main.len());
            }
            match sc {
                Some(sc) if sc.lit() => {
                    first_lit.get_or_insert(main.len());
                    if is_glyph(ins) {
                        text.push(*ins);
                    } else {
                        base.push(*ins);
                        if ins.params[1] == KIND_RECT
                            && let Some(p) = prim(ins, mark_material(m), nd, &pc, scale)
                        {
                            prims.push(p);
                        }
                    }
                }
                Some(Scene::Motion) if nd == node::CARD => {
                    if card_blur.is_some() {
                        nodel.push(*ins);
                    } else {
                        let mut c = *ins;
                        c.rect[0] += card_off[0];
                        c.rect[1] += card_off[1];
                        main.push(c);
                    }
                }
                _ => main.push(*ins),
            }
            if needs_mask && matches!(nd, node::GLOW_TEXT | node::SKELETON | node::THINKING) && sc.is_some_and(|s| !s.lit()) {
                mask.push(*ins);
            }
            if let Some(sc) = sc {
                last.insert((sc.idx(), nd), main.len());
                top.insert(sc.idx(), main.len());
            }
        }
        let light_on = !base.is_empty() && settings.is_some();
        if light_on && lights.is_empty() {
            // GI off with no lights: an ambient-only pass keeps the canvas lit by its settings.
            lights.push(Light::point(0.0, 0.0, 1.0).intensity(0.0));
        }

        // ---- ops: where each effect layer goes in the main list
        enum Op {
            Ui,
            Blit,
            Fx(usize),
        }
        let mut ops: Vec<(usize, Op)> = Vec::new();
        if light_on && let Some(p) = first_lit {
            ops.push((p, Op::Blit));
        }
        let mut layers: Vec<FxLayer> = Vec::new();
        let mut needs_bd: Vec<bool> = Vec::new();
        for l in lays {
            let key = |n: u8| (l.scene.idx(), n);
            let pos = match l.anchor {
                Anchor::Under(n) => first.get(&key(n)).copied(),
                Anchor::Over(n) => last.get(&key(n)).copied(),
                Anchor::Top => top.get(&l.scene.idx()).copied(),
            };
            let Some(pos) = pos else { continue };
            needs_bd.push(l.layer.fx.needs_backdrop());
            ops.push((pos, Op::Fx(layers.len())));
            layers.push(l.layer.reduced(s.reduced));
        }
        ops.sort_by_key(|o| o.0);
        ops.push((main.len(), Op::Ui));

        // ---- upload and draw
        let clear = rt.clear_color();
        let ranges = f.ui.prepare_lists(f.device, f.queue, &mut rt.text.atlas, &[&main, &base, &text, &nodel, &mask], f.size);
        let sources = [&res.pyramid.view, &res.node.view];
        let stats = res.fx.prepare(
            f.device,
            f.queue,
            &FxFrame { viewport: [w, h], scale, layers: &layers, backdrop: Some(&res.backdrop.view), sources: &sources },
        );
        let enc = &mut *f.encoder;
        // GPU time spans the first pass the compositor records through the end of the resolve pass.
        let tq = res.timer.as_ref().and_then(Timer::query);
        let mut began = false;
        let mut stamp = |end: bool| {
            let first = !std::mem::replace(&mut began, true);
            tq.as_ref().filter(|_| first || end).map(|q| wgpu::RenderPassTimestampWrites {
                query_set: q,
                beginning_of_pass_write_index: first.then_some(0),
                end_of_pass_write_index: end.then_some(1),
            })
        };
        if needs_mask && !mask.is_empty() {
            {
                let mut p = pass_ts(enc, &res.pyramid.levels[0], Some([0.0; 4]), stamp(false));
                f.ui.draw_range(&mut p, HDR, ranges[4].clone());
            }
            res.fx.build_mips(f.device, enc, &res.pyramid);
        }
        if !nodel.is_empty() {
            let mut p = pass_ts(enc, &res.node.view, Some([0.0; 4]), stamp(false));
            f.ui.draw_range(&mut p, HDR, ranges[3].clone());
        }
        let mut lit_drawn = false;
        if light_on {
            let (_, mut st) = settings.unwrap();
            st.time = t;
            let a = stage_fill[3].max(1e-4);
            {
                let mut p = pass_ts(enc, &res.base.view, Some([stage_fill[0] / a, stage_fill[1] / a, stage_fill[2] / a, 1.0]), stamp(false));
                f.ui.draw_range(&mut p, HDR, ranges[1].clone());
            }
            {
                let mut p = pass(enc, &res.text.view, Some([0.0; 4]));
                f.ui.draw_range(&mut p, HDR, ranges[2].clone());
            }
            let light = res.light.get_or_insert_with(|| LightRenderer::new(f.device, f.queue, w, h, HDR));
            lit_drawn = light.render(
                f.device,
                f.queue,
                enc,
                &LightScene { prims: &prims, lights: &lights, settings: st, emission: None },
                &res.base.view,
                &res.text.view,
                &res.lit.view,
            );
            f.queue.write_buffer(&res.blit_alpha, 0, bytemuck::cast_slice(&[lit_alpha, 0.0, 0.0, 0.0]));
        }
        let px = |v: f32| (v * scale).round().max(0.0) as u32;
        let (sx, sy) = (px(stage_r[0]).min(w), px(stage_r[1]).min(h));
        let (sw, sh) = (px(stage_r[0] + stage_r[2]).min(w) - sx, px(stage_r[1] + stage_r[3]).min(h) - sy);
        if res.fx.simulate_pending() {
            let ts = stamp(false).map(|w| wgpu::ComputePassTimestampWrites {
                query_set: w.query_set,
                beginning_of_pass_write_index: w.beginning_of_pass_write_index,
                end_of_pass_write_index: w.end_of_pass_write_index,
            });
            res.fx.simulate(f.queue, enc, ts);
        }
        let mut cursor = 0usize;
        let mut rp = pass_ts(enc, &res.scene.view, Some(clear), stamp(false)).forget_lifetime();
        for (pos, op) in ops {
            if pos > cursor {
                rp.set_scissor_rect(0, 0, w, h);
                f.ui.draw_range(&mut rp, HDR, ranges[0].start + cursor as u32..ranges[0].start + pos as u32);
                cursor = pos;
            }
            match op {
                Op::Ui => {}
                Op::Blit if lit_drawn && sw > 0 && sh > 0 => {
                    rp.set_scissor_rect(sx, sy, sw, sh);
                    rp.set_pipeline(&res.blit.pipeline);
                    rp.set_bind_group(0, &res.blit_bg, &[]);
                    rp.draw(0..3, 0..1);
                }
                Op::Blit => {}
                Op::Fx(i) => {
                    if sw == 0 || sh == 0 {
                        continue;
                    }
                    if needs_bd[i] {
                        drop(rp);
                        let o = wgpu::Origin3d { x: sx, y: sy, z: 0 };
                        enc.copy_texture_to_texture(
                            wgpu::TexelCopyTextureInfo { texture: &res.scene.tex, mip_level: 0, origin: o, aspect: wgpu::TextureAspect::All },
                            wgpu::TexelCopyTextureInfo { texture: &res.backdrop.tex, mip_level: 0, origin: o, aspect: wgpu::TextureAspect::All },
                            wgpu::Extent3d { width: sw, height: sh, depth_or_array_layers: 1 },
                        );
                        res.fx.blur_backdrop(f.device, f.queue, enc, &res.backdrop.view, i);
                        rp = pass(enc, &res.scene.view, None).forget_lifetime();
                    }
                    rp.set_scissor_rect(sx, sy, sw, sh);
                    res.fx.draw_layer(&mut rp, i);
                }
            }
        }
        drop(rp);
        {
            let mut p = pass_ts(enc, f.target, Some([0.0, 0.0, 0.0, 1.0]), stamp(true));
            p.set_pipeline(&res.resolve.pipeline);
            p.set_bind_group(0, &res.resolve_bg, &[]);
            p.draw(0..3, 0..1);
        }
        if let Some(tm) = res.timer.as_mut() {
            tm.end();
        }
        self.animating = anim;

        // ---- readout: averaged over a quarter second of Rune's clock. It asks for a redraw only on frames that
        // were drawing anyway (animating, the display link flipping, the pointer moving), so a scene at rest never
        // draws frames for its own readout.
        let moved = std::mem::replace(&mut self.last_pointer, rt.pointer()) != self.last_pointer;
        // A frame that (re)built resources pays pipeline compiles; it would stick in an at-rest readout.
        if !rebuilt {
            self.cpu.0 += f.update_ms + started.elapsed().as_secs_f64() * 1000.0;
            self.cpu.1 += 1;
        }
        if let Some(g) = gpu_ms {
            self.gpu.0 += g as f64;
            self.gpu.1 += 1;
            self.total_gpu.0 += g as f64;
            self.total_gpu.1 += 1;
        }
        let due = self.last_push.is_none_or(|p| now - p >= 0.25);
        let live_flip = self.shown.is_some_and(|(_, _, l)| l != anim);
        if due && (anim || moved) || live_flip {
            let c = if self.cpu.1 > 0 { (self.cpu.0 / self.cpu.1 as f64) as f32 } else { rt.state.perf.cpu_ms };
            let g = (self.gpu.1 > 0).then(|| (self.gpu.0 / self.gpu.1 as f64) as f32).or(rt.state.perf.gpu_ms);
            let key = ((c * 100.0).round() as i32, g.map(|g| (g * 100.0).round() as i32), anim);
            if self.shown != Some(key) {
                rt.state.perf = crate::Perf { cpu_ms: c, gpu_ms: g, draws: stats.drawn, live: anim };
                rt.invalidate();
                self.view_pending = true;
                self.shown = Some(key);
            }
            self.cpu = (0.0, 0);
            self.gpu = (0.0, 0);
            self.last_push = Some(now);
        }
        rt.state.now = now;
    }
}
