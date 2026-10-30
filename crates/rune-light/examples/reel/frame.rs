//! One frame of the reel: glow and glass settings over time, subframes and the bench overlay.

use super::*;

/// Captions and labels drawn in screen space over everything.
pub(super) struct Overlay {
    pub(super) items: Vec<(f32, f32, String, f32, f32, Color)>,
    pub(super) rects: Vec<Instance>,
}

impl Overlay {
    pub(super) fn text(&mut self, x: f32, y: f32, s: &str, size: f32, weight: f32, c: Color) {
        self.items.push((x, y, s.to_string(), size, weight, c));
    }
}

#[derive(Default, Clone, Copy)]
pub(super) struct Bench {
    pub(super) timings: Option<Timings>,
    pub(super) glass_ms: f64,
}

pub(super) struct BenchNumbers {
    pub(super) gpu: String,
    pub(super) gi: f64,
    pub(super) field: f64,
    pub(super) scene: f64,
    pub(super) cascades: [f64; 5],
    pub(super) fluence: f64,
    pub(super) glass: f64,
    pub(super) frame: f64,
}

pub(super) fn load_bench(out: &Path) -> BenchNumbers {
    let mut b = BenchNumbers { gpu: "M5 Max".into(), gi: 2.3, field: 0.07, scene: 0.05, cascades: [0.4; 5], fluence: 0.01, glass: 0.5, frame: 4.0 };
    if let Ok(s) = std::fs::read_to_string(out.join("bench.json")) {
        let num = |k: &str| -> Option<f64> {
            let i = s.find(&format!("\"{k}\""))?;
            let rest = &s[i + k.len() + 3..];
            let rest = rest.trim_start_matches([':', ' ']);
            let end = rest.find([',', '}', ']', '\n']).unwrap_or(rest.len());
            rest[..end].trim().parse().ok()
        };
        b.gi = num("gi").unwrap_or(b.gi);
        b.field = num("field").unwrap_or(b.field);
        b.scene = num("gi_scene").unwrap_or(b.scene);
        b.fluence = num("fluence").unwrap_or(b.fluence);
        b.glass = num("glass").unwrap_or(b.glass);
        b.frame = num("frame").unwrap_or(b.frame);
        for n in 0..5 {
            b.cascades[n] = num(&format!("c{n}")).unwrap_or(b.cascades[n]);
        }
        if let Some(i) = s.find("\"gpu\": \"") {
            let rest = &s[i + 8..];
            if let Some(e) = rest.find('"') {
                b.gpu = rest[..e].to_string();
            }
        }
    }
    b
}

pub(super) struct Reel {
    pub(super) bench: BenchNumbers,
    pub(super) hp: [[f32; 2]; 2],
}

pub(super) fn glow_settings(t: f32) -> Settings {
    Settings {
        ambient: [0.2, 0.205, 0.235],
        exposure: 1.0,
        gi: Some(Gi { strength: 1.15, air: 0.10, bounce: 0.0, absorption: 0.22, emission: 0.9 }),
        text: TextPolicy { floor: 0.84, ceiling: 1.3, lit: 1.0, specular: 0.1 },
        albedo_floor: 0.035,
        contact_ao: 0.35,
        rim: 0.5,
        background_gloss: 0.35,
        time: t,
        ..Default::default()
    }
}

/// Glass shape and look at time t (None when off screen).
pub(super) fn glass_at(t: f32) -> Option<Glass> {
    if !(T_GLASS - 0.1..T_ARC + 0.4).contains(&t) {
        return None;
    }
    let x = keys(-520.0, &[(T_GLASS, 300.0, sp(1.3, 0.08)), (T_GLASS + 3.4, 330.0, sp(1.0, 0.0)), (T_ARC - 1.3, 1500.0, sp(1.4, 0.0))]).at(t);
    let y = keys(300.0, &[(T_GLASS + 3.4, 360.0, sp(1.2, 0.0))]).at(t);
    let presence = rise(t, T_GLASS, 0.9);
    let (w, h, r) = (380.0, 240.0, 56.0);
    let a = dpx(x, y, w, h);
    // A drop of glass buds off the card toward the send button and is pulled back.
    let bud = window(t, T_GLASS + 1.7, T_GLASS + 3.9, 0.9);
    let bx = x + w - 40.0 + bud * 210.0;
    let by = y + h - 50.0 + bud * 110.0;
    let br = 46.0 + 14.0 * bud;
    let b = dpx(bx, by, br * 2.0, br * 2.0);
    let frost = 0.85 * window(t, T_GLASS + 4.3, T_GLASS + 6.1, 0.7);
    let k = UI_SCALE * S;
    Some(
        Glass::rounded(a[0], a[1], a[2], a[3], r * k)
            .thickness(34.0 * k)
            .bevel(24.0 * k)
            .gap(12.0 * k)
            .dome(10.0 * k)
            .ior(1.5)
            .dispersion(0.045)
            .frost(frost)
            .presence(presence)
            .rim_glow(0.45)
            .union(b, br * k, 60.0 * k, 0.0),
    )
}

/// Sub-frames for the shutter: enough that the fastest-moving element steps at most ~2 px between them.
fn subframes(t: f32) -> u32 {
    let h = 1.0 / 480.0;
    let toast = |t: f32| state(t).toast_x * UI_SCALE * S;
    let gx = |t: f32| glass_at(t).map_or(0.0, |g| g.a[0] + g.a[1] + g.b[0] + g.b[1]);
    let v = ((toast(t + h) - toast(t - h)).abs()).max((gx(t + h) - gx(t - h)).abs()) / (2.0 * h);
    let travel = v * 0.5 / FPS as f32;
    ((travel / 2.0).ceil() as u32).clamp(1, 24)
}

impl Reel {
    /// One frame: the lit image (shutter-integrated while things move fast), the breakdown, bloom and captions.
    pub(super) fn frame(&self, ctx: &mut Ctx, t: f32, bench: bool) -> (wgpu::TextureView, Bench) {
        let n = if bench { 1 } else { subframes(t) };
        let (mut frame_tex, mut frame_view, b) = if n <= 1 {
            self.image(ctx, t, bench)
        } else {
            // 180° shutter: n sub-frames across [t − 1/240, t + 1/240], weighted by a raised cosine so trails have soft
            // ends (as in rune-fx's motion blur).
            let shutter = 0.5 / FPS as f32;
            let ws: Vec<f32> = (0..n).map(|k| 0.5 - 0.5 * (std::f32::consts::TAU * (k as f32 + 0.5) / n as f32).cos()).collect();
            let wsum: f32 = ws.iter().sum();
            let mut last = Bench::default();
            for k in 0..n {
                let tk = t + shutter * ((k as f32 + 0.5) / n as f32 - 0.5);
                let (_, view, b) = self.image(ctx, tk, false);
                last = b;
                let mut vps = [VP::default(); 1];
                vps[0].a = [ws[k as usize] / wsum, 0.0, 0.0, 0.0];
                ctx.queue.write_buffer(&ctx.viz.vp, (60 * std::mem::size_of::<VP>()) as u64, bytemuck::cast_slice(&vps));
                let bg = ctx.viz_bg(0, Some(&view));
                let mut enc = ctx.device.create_command_encoder(&Default::default());
                {
                    let mut pass = Ctx::pass(&mut enc, &ctx.accum.view, if k == 0 { Some([0.0; 4]) } else { None });
                    pass.set_pipeline(&ctx.viz.accum);
                    pass.set_bind_group(0, &bg, &[]);
                    pass.draw(0..3, 60..61);
                }
                ctx.queue.submit([enc.finish()]);
            }
            (ctx.accum.tex.clone(), ctx.accum.view.clone(), last)
        };

        // Breakdown: the 2.5D stack of real buffers.
        let mut ov = Overlay { items: Vec::new(), rects: Vec::new() };
        if (T_TILT..T_UNTILT + 1.4).contains(&t) {
            frame_view = self.breakdown(ctx, t, &frame_tex, &mut ov);
            frame_tex = ctx.stack.tex.clone();
        }
        let _ = frame_tex;
        ctx.bloom(&frame_view);
        self.captions(t, &mut ov);
        // Overlay text.
        let mut insts = ov.rects.clone();
        for (x, y, s, size, weight, c) in &ov.items {
            let style = TextStyle::new(*size, *weight);
            let mut quads = Vec::new();
            ctx.ts.glyphs(s, style, S, [(x * S).round(), (y * S).round()], &mut quads);
            for q in quads {
                insts.push(Instance {
                    rect: q.rect,
                    radii: [0.0; 4],
                    fill: c.premul(1.0),
                    border: [0.0; 4],
                    params: [0.0, KIND_GLYPH, 0.0, 1.0],
                    uv: [q.uv[0], q.uv[1], 0.0, 0.0],
                    clip: NO_CLIP,
                    fade: [0; 4],
                });
            }
        }
        ctx.draw_rune(&insts, &ctx.overlay.view.clone(), [0.0; 4]);
        (frame_view, b)
    }

    /// The lit UI with effects and glass at exactly time t.
    #[allow(clippy::too_many_lines)]
    pub(super) fn image(&self, ctx: &mut Ctx, t: f32, bench: bool) -> (wgpu::Texture, wgpu::TextureView, Bench) {
        let st = state(t);
        let built = build_ui(&mut ctx.ts, &st);
        let bgc = oklch(BG.0, BG.1, BG.2);
        ctx.draw_rune(&built.base, &ctx.base.view.clone(), [bgc.r, bgc.g, bgc.b, 1.0]);
        ctx.draw_rune(&built.text, &ctx.text.view.clone(), [0.0; 4]);

        // Effects: the electric border on the danger button. Its light is rendered once into the emission texture
        // (which the GI reads) and again over the lit frame.
        let mut fx_layers: Vec<FxLayer> = Vec::new();
        if st.arc > 0.0 {
            let b = dpt(924.0 + 20.0, 424.0, 150.0, 36.0);
            let e = ElectricParams {
                style: ElectricStyle::Border,
                color: rgb(0xff5a4a),
                core: rgb(0xfff1e8),
                cool: rgb(0x9b4dff),
                amplitude: 3.2,
                frequency: 2.6,
                speed: 3.0,
                glow: 5.5,
                forks: 0.5,
                flicker: 0.35,
                intensity: 1.15 * st.arc,
                gap: 3.0,
                ..ElectricParams::default()
            };
            fx_layers.push(Fx::electric(e).on(Shape::rounded(b[0], b[1], b[2], b[3], 10.0 * UI_SCALE)).seed(7).at(t).over());
        }
        let click = 3.3;
        let shock = (click..click + 1.2).contains(&t);
        let mut ts_glass = 0.0;
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        if !fx_layers.is_empty() {
            ctx.fx.prepare(&ctx.device, &ctx.queue, &FxFrame { viewport: [W, H], scale: S, layers: &fx_layers, backdrop: None, sources: &[] });
            let mut pass = Ctx::pass(&mut enc, &ctx.emission.view, Some([0.0; 4]));
            ctx.fx.draw_all(&mut pass, &fx_layers, None);
        }
        {
            let scene = Scene {
                prims: &built.prims,
                lights: &[],
                settings: glow_settings(t),
                emission: (!fx_layers.is_empty()).then_some(&ctx.emission.view),
            };
            ctx.light.inspect = (T_TILT - 0.1..T_UNTILT + 1.5).contains(&t) || bench;
            ctx.light.render(&ctx.device, &ctx.queue, &mut enc, &scene, &ctx.base.view, &ctx.text.view, &ctx.lit.view);
        }
        ctx.queue.submit([enc.finish()]);
        let timings = if bench { ctx.light.read_timings(&ctx.device) } else { None };
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        if !fx_layers.is_empty() {
            let mut pass = Ctx::pass(&mut enc, &ctx.lit.view, None);
            ctx.fx.draw_all(&mut pass, &fx_layers, None);
        }
        if shock {
            // The click: a shockwave ripple out of the send button.
            let sb = dpt(272.0 + 628.0 - 58.0, 712.0, 38.0, 38.0);
            let at = [sb[0] + sb[2] * 0.5, sb[1] + sb[3] * 0.5];
            let mut d = DistortParams::shockwave(at, click);
            if let DistortParams::Shockwave { reach, width, amount, chroma, duration, .. } = &mut d {
                *reach = 260.0;
                *width = 16.0;
                *amount = 9.0;
                *chroma = 0.35;
                *duration = 1.1;
            }
            let layers = [Fx::distort(d).on(Shape::rounded(at[0] - 280.0, at[1] - 280.0, 560.0, 560.0, 0.0)).at(t)];
            ctx.copy(&mut enc, &ctx.lit.tex, &ctx.backdrop.tex);
            ctx.queue.submit([enc.finish()]);
            enc = ctx.device.create_command_encoder(&Default::default());
            ctx.fx.prepare(&ctx.device, &ctx.queue, &FxFrame { viewport: [W, H], scale: S, layers: &layers, backdrop: Some(&ctx.backdrop.view), sources: &[] });
            let mut pass = Ctx::pass(&mut enc, &ctx.lit.view, None);
            ctx.fx.draw_all(&mut pass, &layers, None);
        }
        let mut frame_view = ctx.lit.view.clone();
        let mut frame_tex = ctx.lit.tex.clone();
        if let Some(g) = glass_at(t).or(if bench { glass_at(T_GLASS + 1.0) } else { None }) {
            let lights: Vec<GlassLight> = built.prims.iter().filter_map(|p| GlassLight::from_emitter(p, 18.0 * S, 0.9)).collect();
            ctx.marker(&mut enc, None, Some(0));
            let settings = GlassSettings::default();
            ctx.glass.render(&ctx.device, &ctx.queue, &mut enc, &[g], &lights, settings, &ctx.lit.view, ctx.light.inspect().fluence, &ctx.glassed.view);
            ctx.marker(&mut enc, Some(1), None);
            if let Some((qs, resolve, read)) = &ctx.query {
                enc.resolve_query_set(qs, 0..2, resolve, 0);
                enc.copy_buffer_to_buffer(resolve, 0, read, 0, 16);
            }
            frame_view = ctx.glassed.view.clone();
            frame_tex = ctx.glassed.tex.clone();
            ts_glass = 1.0;
        }
        ctx.queue.submit([enc.finish()]);
        let mut glass_ms = 0.0;
        if bench && ts_glass > 0.0 {
            if let Some((_, _, read)) = &ctx.query {
                let s = read.slice(..);
                s.map_async(wgpu::MapMode::Read, |r| r.unwrap());
                ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
                let ticks: Vec<u64> = bytemuck::cast_slice(&s.get_mapped_range().unwrap()).to_vec();
                read.unmap();
                glass_ms = ticks[1].saturating_sub(ticks[0]) as f64 * ctx.period as f64 / 1e6;
            }
        }

        (frame_tex, frame_view, Bench { timings, glass_ms })
    }

    pub(super) fn captions(&self, t: f32, ov: &mut Overlay) {
        let b = &self.bench;
        let caps: Vec<(f32, f32, String, String)> = vec![
            (1.0, T_TILT - 0.3, "Lit only by its own glowing elements".into(), "2D radiance cascades · every emitter lights its neighbours, with soft coloured shadow".into()),
            (T_TILT + 0.2, T_INPUTS - 0.1, "How it's lit".into(), "rune-light's real GI buffers, frame by frame".into()),
            (
                T_INPUTS,
                T_CASCADES - 0.1,
                "① The scene as a distance field".into(),
                format!("Rune draws analytic rounded rects, so the SDF is exact (no jump flood) · ½ res · {:.2} ms", b.field),
            ),
            (
                T_CASCADES,
                T_MERGE - 0.1,
                "② Five cascades of probes".into(),
                "each level: probes 2× sparser, rays 4× longer, 4× the directions · same memory per level".into(),
            ),
            (
                T_MERGE,
                T_FLUENCE - 0.1,
                "③ Merge, top cascade down".into(),
                "each ray continues into the four nearest upper probes (Sannikov's bilinear fix: no leaks between probes)".into(),
            ),
            (T_FLUENCE, T_UNTILT - 0.1, "④ Fluence".into(), "cascade 0's directions averaged: the light arriving at every point".into()),
            (
                T_UNTILT,
                T_GLASS - 0.2,
                "⑤ Back onto the UI".into(),
                format!("whole GI pass {:.1} ms on {} at 2560×1600 · cost fixed by the grid, not the number of lights", b.gi, b.gpu),
            ),
            (
                T_GLASS + 0.3,
                T_ARC - 0.4,
                "Thick glass, refracted per pixel".into(),
                format!("Snell n = 1.50 · exact Fresnel · 8-wavelength dispersion · total internal reflection at the bevel · frost · {:.2} ms", b.glass),
            ),
            (T_ARC + 0.2, T_END - 0.3, "Effects are light too".into(), "the electric arc is emission: it enters the cascades and lights the cards around it".into()),
        ];
        for (a, z, title, sub) in caps {
            let k = window(t, a, z, 0.35);
            if k <= 0.01 {
                continue;
            }
            let y = 754.0 + (1.0 - k) * 6.0;
            let x = UI_O[0] + 4.0;
            ov.text(x, y, &title, 15.0, 600.0, oklcha(0.95, 0.005, 268.0, k));
            ov.text(x, y + 22.0, &sub, 12.5, 400.0, oklcha(0.68, 0.012, 268.0, k));
        }
        let k = rise(t, T_END + 0.3, 0.8);
        if k > 0.01 {
            let x = 640.0 - 150.0;
            ov.text(x, 760.0 + (1.0 - k) * 6.0, "Rune  ·  radiance cascades  ·  120 Hz", 16.0, 600.0, oklcha(0.95, 0.005, 268.0, k));
        }
    }

    /// The breakdown: renders each real buffer into a layer texture and composes the 2.5D stack.
    pub(super) fn breakdown(&self, ctx: &mut Ctx, t: f32, lit: &wgpu::Texture, ov: &mut Overlay) -> wgpu::TextureView {
        let ins = ctx.light.inspect();
        let gs = ins.gi_size;
        let texel = ins.texel;
        let tilt = rise(t, T_TILT, 1.3) * (1.0 - rise(t, T_UNTILT, 1.2));
        let cam = Cam { pitch: 0.84 * tilt, yaw: -0.28 * tilt, scale: lerp(1.0, 0.6, tilt), ty: -20.0 * tilt, tx: -190.0 * tilt };
        // Per layer: (texture slot, z, alpha, kind)
        let inputs = window(t, T_INPUTS - 0.2, T_CASCADES + 0.3, 0.6);
        let ui_lift = rise(t, T_INPUTS - 0.3, 0.9) * (1.0 - rise(t, T_FLUENCE + 0.6, 1.0));
        let ui_alpha = 1.0 - 0.9 * rise(t, T_INPUTS - 0.3, 0.7) + 0.9 * rise(t, T_UNTILT - 0.2, 0.8);
        let merge_k = |n: usize| {
            // C4 merges first; each level collapses into the one below and switches to its merged radiance.
            let tn = T_MERGE + 0.45 * (CASCADES as usize - 1 - n) as f32;
            rise(t, tn, 0.5)
        };
        let gu = GU {
            size: [W as f32, H as f32, gs.0 as f32, gs.1 as f32],
            gi: [texel, t, 2.0, 0.0],
            hp: [[self.hp[0][0], self.hp[0][1], self.hp[1][0], self.hp[1][1]], [0.0; 4]],
        };
        ctx.queue.write_buffer(&ctx.viz.g, 0, bytemuck::bytes_of(&gu));
        let mut vps = vec![VP::default(); MAX_VP];
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        ctx.means(&mut enc);
        // Layer content passes: slot 0 field, 1 scene, 2..7 cascades 0..4, 7 fluence, 8 lit UI.
        let expo = [0.9, 1.6, 2.6, 4.0, 6.0];
        let mut content: Vec<(usize, &wgpu::RenderPipeline, usize)> = vec![(0, &ctx.viz.field, 0), (1, &ctx.viz.scene, 0)];
        for n in 0..CASCADES as usize {
            let own_to_merged = if n == CASCADES as usize - 1 { 1.0 } else { merge_k(n + 1) };
            let grow = rise(t, T_CASCADES + 0.25 + 0.5 * n as f32, 0.8);
            vps[2 + n] = VP {
                a: [n as f32, own_to_merged, expo[n] * lerp(1.0, 0.6, own_to_merged), 0.0],
                b: [1.0, 1.0, 1.0, 1.0],
                c: [grow, 1.0, 0.0, 3.2],
                q: [[0.0; 4]; 4],
            };
            content.push((2 + n, &ctx.viz.cascade, n));
        }
        vps[1].a = [0.0, 0.0, 0.9, 0.0];
        vps[7].a = [0.0, 0.0, 1.5, 0.0];
        content.push((7, &ctx.viz.fluence, 0));
        drop(ins);
        for (slot, pipe, level) in &content {
            let bg = ctx.viz_bg(*level, None);
            let mut pass = Ctx::pass(&mut enc, &ctx.layers[*slot].mips[0], Some([0.0, 0.0, 0.0, 1.0]));
            pass.set_pipeline(pipe);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, *slot as u32..*slot as u32 + 1);
        }
        ctx.copy(&mut enc, lit, &ctx.layers[8].t.tex);
        ctx.queue.submit([enc.finish()]);
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        for slot in 0..9 {
            ctx.build_mips(&mut enc, slot);
        }

        // Stack layout: (slot, z, alpha, label)
        let mut stack: Vec<(usize, f32, f32, String)> = Vec::new();
        let sep = 210.0;
        // Inputs: field and media sit below the UI while it lifts away.
        stack.push((0, -sep * 1.0 * inputs - 80.0 * (1.0 - inputs), inputs, "Distance field · ½ res, exact".into()));
        stack.push((1, 0.0, inputs, format!("Emitters + cards · {}×{} texels · sub-texel coverage", gs.0, gs.1)));
        // Cascades rise in from below, C0 lowest; then collapse top down.
        for n in 0..CASCADES as usize {
            let a_in = rise(t, T_CASCADES + 0.5 * n as f32, 0.7);
            let a_out = rise(t, T_FLUENCE + 0.2, 0.6);
            let base_z = (n as f32 - 2.0) * sep * 0.95;
            // Collapse: once merged into level n−1, sink onto it.
            let collapsed = if n == 0 { 0.0 } else { merge_k(n) };
            let below = (n as f32 - 3.0) * sep * 0.95;
            let z = lerp(base_z - 300.0 * (1.0 - a_in), below.min(base_z), collapsed);
            // Focus: once the next level arrives, this one recedes until the merge brings it back.
            let next_in = if n + 1 < CASCADES as usize { rise(t, T_CASCADES + 0.5 * (n + 1) as f32, 0.6) } else { 0.0 };
            let refocus = rise(t, T_MERGE - 0.2, 0.6);
            let focus = 1.0 - 0.6 * next_in * (1.0 - refocus);
            let alpha = a_in * (1.0 - a_out) * focus * (1.0 - 0.85 * if n > 0 { merge_k(n) } else { 0.0 });
            let p = (gs.0 >> n, gs.1 >> n);
            let t0 = (4f32.powi(n as i32) - 1.0) / 3.0 * texel;
            let t1 = (4f32.powi(n as i32 + 1) - 1.0) / 3.0 * texel;
            let ms = self.bench.cascades[n];
            stack.push((
                2 + n,
                z,
                alpha,
                format!("C{n} · {}×{} probes · {} rays · {:.0}–{:.0} px · {:.2} ms", p.0, p.1, 4 << (2 * n), t0, t1, ms),
            ));
        }
        let fl = window(t, T_FLUENCE, T_UNTILT + 0.5, 0.6);
        stack.push((7, -sep * 1.9 + sep * 1.9 * rise(t, T_FLUENCE, 0.8), fl, format!("Fluence · {:.2} ms", self.bench.fluence)));
        stack.push((8, sep * 3.2 * ui_lift, ui_alpha.clamp(0.0, 1.0), String::new()));

        // Back to front: sort by projected depth of the layer's centre.
        let mut order: Vec<usize> = (0..stack.len()).filter(|&i| stack[i].2 > 0.003).collect();
        order.sort_by(|&a, &b| {
            let za = project(&cam, W as f32 * 0.5, H as f32 * 0.5, stack[a].1)[3];
            let zb = project(&cam, W as f32 * 0.5, H as f32 * 0.5, stack[b].1)[3];
            zb.total_cmp(&za)
        });
        let first_vp = 16;
        for (k, &i) in order.iter().enumerate() {
            let (_, z, alpha, _) = &stack[i];
            let q = [project(&cam, 0.0, 0.0, *z), project(&cam, W as f32, 0.0, *z), project(&cam, 0.0, H as f32, *z), project(&cam, W as f32, H as f32, *z)];
            vps[first_vp + k] = VP { a: [0.0; 4], b: [0.0; 4], c: [0.0, *alpha, 0.55 * tilt, 0.0], q };
        }
        ctx.queue.write_buffer(&ctx.viz.vp, 0, bytemuck::cast_slice(&vps));
        let bgc = oklch(BG.0, BG.1, BG.2);
        {
            let bgs: Vec<wgpu::BindGroup> = order.iter().map(|&i| ctx.viz_bg(0, Some(&ctx.layers[stack[i].0].t.view))).collect();
            let mut pass = Ctx::pass(&mut enc, &ctx.stack.view, Some([bgc.r, bgc.g, bgc.b, 1.0]));
            pass.set_pipeline(&ctx.viz.layer);
            for (k, bg) in bgs.iter().enumerate() {
                pass.set_bind_group(0, bg, &[]);
                pass.draw(0..4, (first_vp + k) as u32..(first_vp + k) as u32 + 1);
            }
        }
        ctx.queue.submit([enc.finish()]);
        // Labels beside each layer's right edge, pushed apart so they never overlap.
        let mut labels: Vec<(f32, f32, f32, &str)> = Vec::new();
        for &i in &order {
            let (_, z, alpha, label) = &stack[i];
            if label.is_empty() || *alpha < 0.05 || tilt < 0.3 {
                continue;
            }
            let p = to_screen(project(&cam, W as f32, H as f32 * 0.5, *z));
            let a = alpha.min(1.0) * ((tilt - 0.3) / 0.4).clamp(0.0, 1.0);
            labels.push((p[0] / S + 44.0, p[1] / S - 8.0, a, label.as_str()));
        }
        labels.sort_by(|a, b| a.1.total_cmp(&b.1));
        for k in 1..labels.len() {
            let min = labels[k - 1].1 + 22.0;
            if labels[k].1 < min {
                labels[k].1 = min;
            }
        }
        for (x, y, a, l) in labels {
            ov.text(x, y, l, 12.0, 500.0, oklcha(0.9, 0.01, 268.0, a));
        }
        ctx.stack.view.clone()
    }
}
