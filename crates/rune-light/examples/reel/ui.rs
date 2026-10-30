//! Building the reel's UI: instances, glyphs and the state at each moment.

use super::*;

pub(super) struct Ui<'a> {
    pub(super) ts: &'a mut TextSystem,
    pub(super) base: Vec<Instance>,
    pub(super) text: Vec<Instance>,
    pub(super) prims: Vec<Prim>,
    /// pt → device px: px = (o + x·k)·S
    pub(super) k: f32,
    pub(super) o: [f32; 2],
}

fn inst(rect: [f32; 4], r: f32, fill: Color, border: Option<(Color, f32)>) -> Instance {
    let (bc, bw) = border.unwrap_or((Color::TRANSPARENT, 0.0));
    Instance {
        rect,
        radii: [r; 4],
        fill: fill.premul(1.0),
        border: if bw > 0.0 { bc.premul(1.0) } else { [0.0; 4] },
        params: [bw, KIND_RECT, 0.0, 1.0],
        uv: [0.0; 4],
        clip: NO_CLIP,
        fade: [0; 4],
    }
}

impl<'a> Ui<'a> {
    pub(super) fn new(ts: &'a mut TextSystem, k: f32, o: [f32; 2]) -> Self {
        Ui { ts, base: Vec::new(), text: Vec::new(), prims: Vec::new(), k, o }
    }

    pub(super) fn px(&self, x: f32, y: f32, w: f32, h: f32) -> [f32; 4] {
        [(self.o[0] + x * self.k) * S, (self.o[1] + y * self.k) * S, w * self.k * S, h * self.k * S]
    }

    pub(super) fn d(&self, v: f32) -> f32 {
        v * self.k * S
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, fill: Color, border: Option<(Color, f32)>, light: impl FnOnce(Prim, f32) -> Prim) {
        let rect = self.px(x, y, w, h);
        let rr = self.d(r);
        self.base.push(inst(rect, rr, fill, border.map(|(c, bw)| (c, bw * S))));
        let p = Prim::rect(rect[0], rect[1], rect[2], rect[3]).radius(rr).albedo([fill.r, fill.g, fill.b]);
        self.prims.push(light(p, self.k * S));
    }

    pub(super) fn paint(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, fill: Color) {
        let rect = self.px(x, y, w, h);
        let rr = self.d(r);
        self.base.push(inst(rect, rr, fill, None));
    }

    pub(super) fn shadow(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, sigma: f32, color: Color) {
        let mut i = inst(self.px(x, y, w, h), self.d(r), color, None);
        i.params = [0.0, KIND_SHADOW, self.d(sigma), 1.0];
        self.base.push(i);
    }

    pub(super) fn text(&mut self, x: f32, y: f32, s: &str, size: f32, weight: f32, color: Color) -> f32 {
        let style = TextStyle::new(size, weight);
        let scale = S * self.k;
        let mut quads = Vec::new();
        self.ts.glyphs(s, style, scale, [((self.o[0] + x * self.k) * S).round(), ((self.o[1] + y * self.k) * S).round()], &mut quads);
        for q in quads {
            self.text.push(Instance {
                rect: q.rect,
                radii: [0.0; 4],
                fill: color.premul(1.0),
                border: [0.0; 4],
                params: [0.0, KIND_GLYPH, 0.0, 1.0],
                uv: [q.uv[0], q.uv[1], 0.0, 0.0],
                clip: NO_CLIP,
                fade: [0; 4],
            });
        }
        self.ts.measure(s, style, scale).0 / self.k
    }

    pub(super) fn measure(&mut self, s: &str, size: f32, weight: f32) -> f32 {
        self.ts.measure(s, TextStyle::new(size, weight), S * self.k).0 / self.k
    }
}

fn scaled(c: Color, k: f32) -> [f32; 3] {
    [c.r * k, c.g * k, c.b * k]
}

/// Everything animated, at one instant.
pub(super) struct St {
    pub(super) t: f32,
    pub(super) send: f32,
    pub(super) rec: f32,
    pub(super) prog: f32,
    pub(super) prog_glow: f32,
    pub(super) toast_x: f32,
    pub(super) toast_glow: f32,
    pub(super) status: f32,
    pub(super) arc: f32,
    pub(super) danger_glow: f32,
}

pub(super) fn state(t: f32) -> St {
    // Emitters warm up one after another (lights cool, never cut).
    let warm = |t0: f32| rise(t, t0, 0.9);
    // Breathing: a spring retargeted every 1.1 s between two levels.
    let mut bk = Vec::new();
    let mut k = 0.25;
    let mut up = true;
    while k < DURATION + 2.0 {
        bk.push((k, if up { 1.0 } else { 0.5 }, sp(0.9, 0.0)));
        up = !up;
        k += 1.1;
    }
    let breath = keys(0.5, &bk).at(t);
    let click = 3.3;
    // The send button flares on the click and cools exponentially back to rest.
    let flare = if t > click { (-(t - click) / 0.35).exp() * rise(t, click, 0.08) } else { 0.0 };
    let send = warm(0.1) * (1.0 + 1.6 * flare);
    let prog = keys(0.08, &[(click + 0.15, 1.0, sp(2.8, 0.0))]).at(t).min(1.0);
    let done = 6.1;
    let pflash = if t > done { (-(t - done) / 0.5).exp() * rise(t, done, 0.1) } else { 0.0 };
    // Fills, floods on completion, then cools to a low resting glow.
    let prog_glow = warm(0.5) * (0.7 + 0.3 * rise(t, click, 0.6)) * (1.0 - 0.7 * rise(t, done + 0.4, 1.6)) + 1.4 * pflash;
    let toast_x = keys(420.0, &[(1.2, 0.0, sp(0.75, 0.12))]).at(t);
    let toast_glow = warm(1.0);
    let arc_in = rise(t, T_ARC, 0.35);
    let arc_out = rise(t, T_END - 0.4, 0.6);
    St {
        t,
        send,
        rec: warm(0.7) * breath,
        prog,
        prog_glow,
        toast_x,
        toast_glow,
        status: warm(0.3),
        arc: arc_in * (1.0 - arc_out),
        danger_glow: arc_in * (1.0 - arc_out),
    }
}

pub(super) const BG: (f32, f32, f32) = (0.115, 0.012, 268.0);

fn build(ui: &mut Ui, st: &St) {
    let bg = oklch(0.155, 0.01, 268.0);
    let side = oklch(0.175, 0.01, 268.0);
    let card = oklch(0.215, 0.011, 268.0);
    let card2 = oklch(0.245, 0.012, 268.0);
    let user = oklch(0.27, 0.04, 268.0);
    let txt = oklch(0.94, 0.005, 268.0);
    let txt2 = oklch(0.70, 0.012, 268.0);
    let txt3 = oklch(0.54, 0.012, 268.0);
    let hair = oklcha(1.0, 0.0, 0.0, 0.07);
    let raise = |p: Prim, k: f32| p.raised(6.0 * k).bevel(6.0 * k, 2.0 * k).gloss(0.5, 0.42);
    let chip = |p: Prim, k: f32| p.on(6.0 * k).raised(2.5 * k).bevel(4.0 * k, 1.0 * k).gloss(0.45, 0.4);

    // Window: soft drop shadow on the desktop, then the window itself (flat: it receives light).
    ui.shadow(0.0, 18.0, 1280.0, 800.0, 18.0, 40.0, oklcha(0.0, 0.0, 0.0, 0.6));
    ui.rect(0.0, 0.0, 1280.0, 800.0, 18.0, bg, Some((oklcha(1.0, 0.0, 0.0, 0.08), 1.0)), |p, _| p);

    // Sidebar
    ui.rect(0.0, 0.0, 244.0, 800.0, 18.0, side, None, |p, _| p);
    let r = ui.d(18.0);
    ui.base.last_mut().unwrap().radii = [r, 0.0, 0.0, r];
    *ui.prims.last_mut().unwrap() = ui.prims.last().unwrap().radii([r, 0.0, 0.0, r]);
    ui.paint(244.0, 0.0, 1.0, 800.0, 0.0, hair);
    for (i, c) in [oklch(0.62, 0.17, 25.0), oklch(0.78, 0.15, 85.0), oklch(0.72, 0.17, 145.0)].iter().enumerate() {
        ui.paint(20.0 + i as f32 * 18.0, 20.0, 11.0, 11.0, 5.5, oklcha(c.to_oklab()[0], 0.02, 268.0, 0.9));
    }
    ui.text(22.0, 52.0, "OpenCode", 16.0, 600.0, txt);
    ui.rect(16.0, 86.0, 212.0, 36.0, 10.0, card, Some((hair, 1.0)), |p, k| p.raised(3.0 * k).bevel(3.0 * k, 1.2 * k).gloss(0.5, 0.45));
    ui.text(32.0, 95.0, "+  New session", 13.0, 500.0, txt);
    ui.text(22.0, 146.0, "TODAY", 11.0, 600.0, txt3);
    let sessions = ["Radiance cascades for UI", "Thick glass optics", "Electric arcs into GI", "Theme tokens codegen", "Scroll momentum", "Slug text spike"];
    for (i, s) in sessions.iter().enumerate() {
        let y = 170.0 + i as f32 * 38.0;
        if i == 0 {
            ui.rect(10.0, y - 8.0, 224.0, 34.0, 9.0, oklcha(1.0, 0.0, 0.0, 0.055), None, |p, _| p);
        }
        ui.text(24.0, y, s, 13.0, if i == 0 { 500.0 } else { 400.0 }, if i == 0 { txt } else { txt2 });
    }
    // Status pill with a green light.
    ui.rect(16.0, 740.0, 212.0, 40.0, 20.0, card, Some((hair, 1.0)), |p, k| p.raised(3.0 * k).bevel(5.0 * k, 1.5 * k).gloss(0.5, 0.4));
    let green = oklch(0.80, 0.19, 150.0);
    let g = 3.2 * st.status;
    ui.rect(34.0, 755.0, 10.0, 10.0, 5.0, green, None, |p, k| p.on(3.0 * k).emissive(scaled(green, g)));
    ui.text(54.0, 751.0, "Connected · studio", 12.0, 500.0, txt2);

    // Header
    ui.text(272.0, 22.0, "Radiance cascades for UI", 15.0, 600.0, txt);
    let model = "claude-opus-5.5";
    let mw = ui.measure(model, 12.0, 500.0);
    ui.rect(900.0 - mw - 26.0, 16.0, mw + 26.0, 28.0, 14.0, card, Some((hair, 1.0)), |p, k| p.raised(2.0 * k).bevel(4.0 * k, 1.0 * k).gloss(0.5, 0.4));
    ui.text(900.0 - mw - 13.0, 22.0, model, 12.0, 500.0, txt2);
    ui.paint(245.0, 60.0, 1035.0, 1.0, 0.0, hair);

    // Chat column
    let cx = 272.0;
    let cw = 628.0;
    let ub = "come on, do radiance cascades UI";
    let uw = ui.measure(ub, 14.0, 400.0) + 36.0;
    ui.rect(cx + cw - uw, 82.0, uw, 46.0, 16.0, user, Some((oklcha(1.0, 0.0, 0.0, 0.08), 1.0)), raise);
    ui.text(cx + cw - uw + 18.0, 95.0, ub, 14.0, 400.0, txt);

    ui.rect(cx, 150.0, cw, 140.0, 16.0, card, Some((hair, 1.0)), raise);
    let lines = [
        "Every Rune element is an analytic rounded rect, so the frame is already",
        "an exact distance field. Glowing elements are the only lights here: five",
        "cascades of probes trace that field, merge top down, and light the cards",
        "around them with soft coloured bounce and shadow.",
    ];
    for (i, l) in lines.iter().enumerate() {
        ui.text(cx + 22.0, 168.0 + i as f32 * 22.0, l, 14.0, 400.0, txt);
    }
    for (i, c) in ["Read gi.wgsl", "Edit cascades", "Run bench"].iter().enumerate() {
        let x = cx + 22.0 + [0.0, 116.0, 236.0][i];
        let w = ui.measure(c, 12.0, 500.0) + 22.0;
        ui.rect(x, 256.0 - 4.0 + 8.0, w, 24.0, 12.0, card2, Some((hair, 1.0)), chip);
        ui.text(x + 11.0, 264.0, c, 12.0, 500.0, txt2);
    }

    // Tool card with the progress bar.
    ui.rect(cx, 310.0, cw, 102.0, 16.0, card, Some((hair, 1.0)), raise);
    ui.text(cx + 22.0, 326.0, "Rendering reel", 14.0, 600.0, txt);
    let frame = (st.prog * 1860.0).round() as u32;
    let sub = format!("frame {frame} / 1860 · 2560×1600 · 60 fps");
    ui.text(cx + 22.0, 348.0, &sub, 12.0, 400.0, txt2);
    let pct = format!("{}%", (st.prog * 100.0).round() as u32);
    let pw = ui.measure(&pct, 12.0, 600.0);
    ui.text(cx + cw - 22.0 - pw, 326.0, &pct, 12.0, 600.0, txt2);
    ui.rect(cx + 22.0, 380.0, cw - 44.0, 10.0, 5.0, oklch(0.19, 0.01, 268.0), None, |p, k| p.on(6.0 * k));
    let cyan = oklch(0.82, 0.14, 205.0);
    let pg = 1.7 * st.prog_glow;
    ui.rect(cx + 22.0, 380.0, (cw - 44.0) * st.prog, 10.0, 5.0, cyan, None, |p, k| p.on(6.0 * k).emissive(scaled(cyan, pg)));

    // Code card
    ui.rect(cx, 432.0, cw, 156.0, 16.0, card, Some((hair, 1.0)), raise);
    ui.text(cx + 22.0, 448.0, "Glowing elements are lights:", 14.0, 400.0, txt);
    ui.rect(cx + 18.0, 478.0, cw - 36.0, 92.0, 10.0, oklch(0.18, 0.01, 268.0), Some((hair, 1.0)), |p, k| p.on(6.0 * k).gloss(0.2, 0.5));
    let code = [
        ("let send = Prim::from_rune(rect, radii)", oklch(0.84, 0.07, 230.0)),
        ("    .emissive(ACCENT * glow);", oklch(0.80, 0.09, 300.0)),
        ("lights.render(&Scene { prims, gi: Some(Gi::default()), .. });", oklch(0.84, 0.07, 160.0)),
    ];
    for (i, (l, c)) in code.iter().enumerate() {
        ui.text(cx + 36.0, 492.0 + i as f32 * 22.0, l, 13.0, 400.0, *c);
    }

    // Composer with the recording dot and the send button.
    ui.rect(cx, 652.0, cw, 112.0, 20.0, card, Some((oklcha(1.0, 0.0, 0.0, 0.1), 1.0)), |p, k| p.raised(9.0 * k).bevel(7.0 * k, 2.5 * k).gloss(0.6, 0.38));
    ui.text(cx + 22.0, 672.0, "Ask anything…", 14.0, 400.0, txt3);
    let red = oklch(0.66, 0.22, 25.0);
    let rg = 6.0 * st.rec;
    ui.rect(cx + 22.0, 724.0, 14.0, 14.0, 7.0, red, None, |p, k| p.on(9.0 * k).emissive(scaled(red, rg)));
    let secs = 7 + st.t as u32;
    ui.text(cx + 44.0, 722.0, &format!("Recording  0:{secs:02}"), 12.0, 500.0, oklch(0.78, 0.09, 25.0));
    let accent = oklch(0.70, 0.19, 275.0);
    let sg = 3.4 * st.send;
    ui.rect(cx + cw - 58.0, 712.0, 38.0, 38.0, 19.0, accent, None, |p, k| p.on(9.0 * k).raised(3.0 * k).emissive(scaled(accent, sg)));
    let aw = ui.measure("↑", 17.0, 700.0);
    ui.text(cx + cw - 39.0 - aw * 0.5, 719.0, "↑", 17.0, 700.0, oklch(0.99, 0.01, 275.0));

    // Right panel
    let rx = 924.0;
    let rw = 332.0;
    ui.rect(rx, 150.0, rw, 170.0, 16.0, card, Some((hair, 1.0)), raise);
    ui.text(rx + 20.0, 166.0, "Agents", 14.0, 600.0, txt);
    let agents = [("cascades", "tracing · 5 levels"), ("glass", "refraction pass"), ("review", "Argon · no blockers")];
    for (i, (a, b)) in agents.iter().enumerate() {
        let y = 198.0 + i as f32 * 38.0;
        ui.rect(rx + 20.0, y + 4.0, 8.0, 8.0, 4.0, oklch(0.5, 0.02, 268.0), None, |p, k| p.on(6.0 * k));
        ui.text(rx + 38.0, y, a, 13.0, 500.0, txt);
        let bw = ui.measure(b, 12.0, 400.0);
        ui.text(rx + rw - 20.0 - bw, y + 1.0, b, 12.0, 400.0, txt2);
    }
    ui.rect(rx, 340.0, rw, 140.0, 16.0, card, Some((hair, 1.0)), raise);
    ui.text(rx + 20.0, 356.0, "Branch rc-reel", 14.0, 600.0, txt);
    ui.text(rx + 20.0, 380.0, "12 commits ahead · 3 files changed", 12.0, 400.0, txt2);
    let danger = oklch(0.42, 0.13, 25.0);
    let dg = 1.4 * st.danger_glow;
    ui.rect(rx + 20.0, 424.0, 150.0, 36.0, 10.0, danger, Some((oklcha(0.75, 0.14, 25.0, 0.5), 1.0)), |p, k| {
        let p = p.on(6.0 * k).raised(3.0 * k).bevel(5.0 * k, 1.5 * k).gloss(0.6, 0.35);
        if dg > 0.0 { p.emissive(scaled(oklch(0.66, 0.2, 25.0), dg)) } else { p }
    });
    ui.text(rx + 43.0, 433.0, "Delete branch", 13.0, 600.0, oklch(0.95, 0.03, 25.0));
    ui.rect(rx + 182.0, 424.0, 130.0, 36.0, 10.0, card2, Some((hair, 1.0)), chip);
    ui.text(rx + 225.0, 433.0, "Cancel", 13.0, 600.0, txt);
    ui.rect(rx, 500.0, rw, 264.0, 16.0, card, Some((hair, 1.0)), raise);
    ui.text(rx + 20.0, 516.0, "GPU per frame", 14.0, 600.0, txt);
    for i in 0..10 {
        let h = 30.0 + 90.0 * (0.5 + 0.5 * ((i as f32 * 0.9).sin() * 0.7 + (i as f32 * 0.37).cos() * 0.3));
        ui.rect(rx + 24.0 + i as f32 * 29.0, 720.0 - h, 16.0, h, 4.0, oklch(0.34, 0.03, 268.0), None, |p, k| p.on(6.0 * k).raised(2.0 * k));
    }
    ui.text(rx + 20.0, 734.0, "GI · composite · glass", 11.0, 500.0, txt3);

    // Toast: slides in from the right edge; its icon glows amber.
    let tx = rx + st.toast_x;
    ui.rect(tx, 76.0, rw, 56.0, 14.0, card2, Some((oklcha(1.0, 0.0, 0.0, 0.1), 1.0)), |p, k| p.floating(10.0 * k, 16.0 * k).bevel(6.0 * k, 2.0 * k).gloss(0.6, 0.38));
    let amber = oklch(0.80, 0.16, 70.0);
    let ag = 2.8 * st.toast_glow;
    ui.rect(tx + 16.0, 92.0, 24.0, 24.0, 12.0, amber, None, |p, k| p.on(16.0 * k).emissive(scaled(amber, ag)));
    let cw2 = ui.measure("✓", 14.0, 700.0);
    if cw2 > 0.0 {
        ui.text(tx + 28.0 - cw2 * 0.5, 94.0, "✓", 14.0, 700.0, oklch(0.25, 0.05, 70.0));
    }
    ui.text(tx + 54.0, 88.0, "Build passed", 13.0, 600.0, txt);
    ui.text(tx + 54.0, 107.0, "ci · 1m 12s", 12.0, 400.0, txt2);
}

// Window placement on the 1280×800 pt canvas: a caption band sits below it.
pub(super) const UI_SCALE: f32 = 0.9;
pub(super) const UI_O: [f32; 2] = [64.0, 18.0];

pub(super) struct Built {
    pub(super) base: Vec<Instance>,
    pub(super) text: Vec<Instance>,
    pub(super) prims: Vec<Prim>,
}

pub(super) fn build_ui(ts: &mut TextSystem, st: &St) -> Built {
    let mut ui = Ui::new(ts, UI_SCALE, UI_O);
    build(&mut ui, st);
    // The electric border around the danger button may glow anywhere within this rect: tell the GI tracer.
    if st.arc > 0.0 {
        let r = ui.px(924.0 + 20.0 - 24.0, 424.0 - 24.0, 150.0 + 48.0, 36.0 + 48.0);
        ui.prims.push(Prim::rect(r[0], r[1], r[2], r[3]).radius(ui.d(30.0)).gi_hint());
    }
    Built { base: ui.base, text: ui.text, prims: ui.prims }
}

/// Device-px rect of a design-space rect.
pub(super) fn dpx(x: f32, y: f32, w: f32, h: f32) -> [f32; 4] {
    [(UI_O[0] + x * UI_SCALE) * S, (UI_O[1] + y * UI_SCALE) * S, w * UI_SCALE * S, h * UI_SCALE * S]
}

/// The same rect in points on the canvas (rune-fx shapes are in points).
pub(super) fn dpt(x: f32, y: f32, w: f32, h: f32) -> [f32; 4] {
    [UI_O[0] + x * UI_SCALE, UI_O[1] + y * UI_SCALE, w * UI_SCALE, h * UI_SCALE]
}
