use super::*;

// ---------------------------------------------------------------- demos

pub type Frame = (Vec<Prim>, Vec<Instance>, Vec<Instance>, Color, Vec<Light>, Settings);

pub(super) trait Demo {
    fn name(&self) -> &'static str;
    fn seconds(&self) -> f32 {
        4.0
    }
    fn still(&self) -> f32;
    fn frame(&mut self, ts: &mut TextSystem, t: f32) -> Frame;
}

fn finish(ui: Ui<'_>, lights: Vec<Light>, s: Settings) -> Frame {
    (ui.prims, ui.base, ui.text, ui.clear, lights, s)
}

/// Linear colour from a colour temperature-ish triplet in sRGB.
pub fn lin(r: f32, g: f32, b: f32) -> [f32; 3] {
    [srgb_decode(r), srgb_decode(g), srgb_decode(b)]
}

/// A smooth closed path through `pts` (Catmull–Rom), u in [0, 1).
fn loop_path(pts: &[[f32; 2]], u: f32) -> [f32; 2] {
    let n = pts.len();
    let x = u.rem_euclid(1.0) * n as f32;
    let i = x.floor() as usize;
    let f = x - i as f32;
    let p = |k: isize| pts[((i as isize + k).rem_euclid(n as isize)) as usize];
    let (p0, p1, p2, p3) = (p(-1), p(0), p(1), p(2));
    let cr = |a: f32, b: f32, c: f32, d: f32| {
        0.5 * ((2.0 * b) + (-a + c) * f + (2.0 * a - 5.0 * b + 4.0 * c - d) * f * f + (-a + 3.0 * b - 3.0 * c + d) * f * f * f)
    };
    [cr(p0[0], p1[0], p2[0], p3[0]), cr(p0[1], p1[1], p2[1], p3[1])]
}

/// A dragged light: waypoints the "hand" jumps between, followed by springs (Kit's palette "calm" .4/0 → softer .7/0
/// for a heavy lamp), sampled at absolute time so frames are deterministic.
struct Drag {
    x: Track,
    y: Track,
    stops: Vec<(f32, [f32; 2])>,
    next: usize,
    spring: Spring,
}

impl Drag {
    fn new(stops: Vec<(f32, [f32; 2])>, spring: Spring) -> Drag {
        let p0 = stops[0].1;
        Drag { x: Track::at(p0[0]), y: Track::at(p0[1]), stops, next: 1, spring }
    }

    pub fn at(&mut self, t: f32) -> [f32; 2] {
        while self.next < self.stops.len() && self.stops[self.next].0 <= t {
            let (ts, p) = self.stops[self.next];
            self.x.retarget(ts as f64, p[0], self.spring);
            self.y.retarget(ts as f64, p[1], self.spring);
            self.next += 1;
        }
        [self.x.value(t as f64), self.y.value(t as f64)]
    }
}

// 1. Chat timeline lit by a lamp ------------------------------------------------

struct Chat {
    drag: Drag,
}

impl Chat {
    fn new() -> Chat {
        let stops = vec![
            (0.0, [1085.0, 250.0]),
            (0.2, [1060.0, 268.0]),
            (0.9, [928.0, 382.0]),
            (1.6, [1100.0, 530.0]),
            (2.35, [905.0, 657.0]),
            (3.0, [560.0, 658.0]),
            (3.55, [1085.0, 250.0]),
        ];
        Chat { drag: Drag::new(stops, Spring::new(0.85, 0.0)) }
    }
}

fn bubble_text(ui: &mut Ui, x: f32, y: f32, lines: &[&str], color: Color) {
    ui.lines(x, y, 21.0, lines, 14.0, 400.0, color);
}

impl Demo for Chat {
    fn name(&self) -> &'static str {
        "chat"
    }
    fn still(&self) -> f32 {
        1.6
    }
    fn frame(&mut self, ts: &mut TextSystem, t: f32) -> Frame {
        let p = self.drag.at(t);
        chat_scene(ts, t, p)
    }
}

/// The chat timeline with the warm lamp at `p` (points).
pub fn chat_scene(ts: &mut TextSystem, t: f32, p: [f32; 2]) -> Frame {
    {
        let bg = oklch(0.17, 0.006, 265.0);
        let mut ui = Ui::new(ts, bg);
        let side = oklch(0.195, 0.006, 265.0);
        let txt = oklch(0.93, 0.004, 265.0);
        let txt2 = oklch(0.66, 0.01, 265.0);
        let txt3 = oklch(0.52, 0.01, 265.0);
        let hair = oklcha(1.0, 0.0, 0.0, 0.06);
        let card = oklch(0.235, 0.007, 265.0);
        let user = oklch(0.3, 0.045, 258.0);
        let code = oklch(0.195, 0.008, 265.0);
        let accent = oklch(0.72, 0.15, 255.0);

        // Sidebar: flat, it receives light but doesn't stand proud.
        ui.rect(0.0, 0.0, 268.0, 800.0, 0.0, side, None, |p| p);
        ui.paint(268.0, 0.0, 1.0, 800.0, 0.0, hair);
        ui.text(24.0, 22.0, "OpenCode", 15.0, 600.0, txt);
        ui.rect(20.0, 58.0, 228.0, 34.0, 9.0, card, Some((hair, 1.0)), |p| p.raised(3.0 * sc()).bevel(3.0 * sc(), 1.2 * sc()).gloss(0.5, 0.45));
        ui.text(36.0, 66.0, "+  New session", 13.0, 500.0, txt);
        ui.text(24.0, 116.0, "TODAY", 11.0, 600.0, txt3);
        let sessions = [
            "rune-light: lighting for UIs",
            "Scroll momentum on iOS",
            "Theme tokens codegen",
            "Hex settings screen",
            "Slug text spike",
            "Radiance cascades notes",
            "Motion blur from tracks",
        ];
        for (i, s) in sessions.iter().enumerate() {
            let y = 138.0 + i as f32 * 36.0;
            if i == 0 {
                ui.rect(14.0, y - 7.0, 240.0, 32.0, 8.0, oklcha(1.0, 0.0, 0.0, 0.06), None, |p| p);
            }
            ui.text(26.0, y, s, 13.0, if i == 0 { 500.0 } else { 400.0 }, if i == 0 { txt } else { txt2 });
        }

        // Header
        ui.text(300.0, 22.0, "rune-light: lighting for UIs", 14.0, 600.0, txt);
        let chip = "claude-opus-5.5";
        let cw = ui.measure(chip, 12.0, 500.0);
        ui.rect(1240.0 - cw - 24.0, 16.0, cw + 24.0, 28.0, 14.0, card, Some((hair, 1.0)), |p| p.raised(2.0 * sc()).bevel(4.0 * sc(), 1.0 * sc()).gloss(0.5, 0.4));
        ui.text(1240.0 - cw - 12.0, 22.0, chip, 12.0, 500.0, txt2);
        ui.paint(269.0, 60.0, 1011.0, 1.0, 0.0, hair);

        let raise = |p: Prim| p.raised(7.0 * sc()).bevel(6.0 * sc(), 2.0 * sc()).gloss(0.55, 0.42);
        // User bubble
        let l1 = ["what if lighting effects. like you could put a light and", "it would raytrace around the ui elements…"];
        ui.rect(800.0, 88.0, 440.0, 68.0, 16.0, user, Some((oklcha(1.0, 0.0, 0.0, 0.08), 1.0)), raise);
        bubble_text(&mut ui, 818.0, 101.0, &l1, txt);

        // Assistant
        ui.rect(300.0, 182.0, 600.0, 152.0, 16.0, card, Some((hair, 1.0)), raise);
        bubble_text(
            &mut ui,
            322.0,
            198.0,
            &[
                "Every Rune element is an analytic rounded rect, so the frame is",
                "already an exact distance field. A light can sphere-trace soft",
                "shadows straight through it: contact-hard near each bubble,",
                "wide and soft where the light is far.",
            ],
            txt,
        );
        for (i, chipt) in ["Read shader.wgsl", "Edit rune-light/src/lib.rs", "Run bench"].iter().enumerate() {
            let x = 322.0 + [0.0, 140.0, 330.0][i];
            let w = ui.measure(chipt, 12.0, 500.0) + 22.0;
            ui.rect(x, 292.0, w, 26.0, 13.0, oklch(0.27, 0.008, 265.0), Some((hair, 1.0)), |p| p.on(7.0 * sc()).raised(2.5 * sc()).bevel(4.0 * sc(), 1.0 * sc()).gloss(0.4, 0.4));
            ui.text(x + 11.0, 297.0, chipt, 12.0, 500.0, txt2);
        }

        // User
        ui.rect(960.0, 360.0, 280.0, 44.0, 16.0, user, Some((oklcha(1.0, 0.0, 0.0, 0.08), 1.0)), raise);
        bubble_text(&mut ui, 980.0, 372.0, &["MAKE THIS SOOO GOOOD"], txt);

        // Assistant with code
        ui.rect(300.0, 430.0, 640.0, 196.0, 16.0, card, Some((hair, 1.0)), raise);
        bubble_text(&mut ui, 322.0, 446.0, &["Here's the API: lights are springs like everything else."], txt);
        ui.rect(318.0, 480.0, 604.0, 126.0, 10.0, code, Some((hair, 1.0)), |p| p.on(7.0 * sc()).gloss(0.2, 0.5));
        let mono = oklch(0.8, 0.06, 200.0);
        ui.lines(
            336.0,
            494.0,
            20.0,
            &[
                "let lamp = Light::point(x, y, 60.0)",
                "    .color(WARM).intensity(1.6).radius(10.0);",
                "",
                "lights.render(&scene, &[lamp], target);",
                "",
            ],
            13.0,
            400.0,
            mono,
        );

        // Composer
        ui.rect(300.0, 690.0, 940.0, 78.0, 18.0, card, Some((oklcha(1.0, 0.0, 0.0, 0.09), 1.0)), |p| {
            p.raised(10.0 * sc()).bevel(7.0 * sc(), 2.5 * sc()).gloss(0.6, 0.38)
        });
        ui.text(322.0, 708.0, "Ask anything…", 14.0, 400.0, txt3);
        ui.rect(1192.0, 724.0, 32.0, 32.0, 16.0, accent, None, |p| p.on(10.0 * sc()).raised(4.0 * sc()).bevel(10.0 * sc(), 3.0 * sc()).gloss(0.8, 0.3));
        ui.paint(1203.0, 739.0, 10.0, 2.0, 1.0, oklch(0.2, 0.02, 255.0));

        let lamp = Light::point(p[0] * sc(), p[1] * sc(), 40.0 * sc())
            .color(lin(1.0, 0.78, 0.55))
            .intensity(2.6)
            .radius(11.0 * sc())
            .range(1100.0 * sc())
            .bulb(0.75)
            .specular(1.2);
        let fill = Light::point(-300.0 * sc(), -200.0 * sc(), 900.0 * sc())
            .color(lin(0.55, 0.65, 1.0))
            .intensity(0.35)
            .radius(220.0 * sc())
            .range(6000.0 * sc())
            .specular(0.4);
        let s = Settings {
            ambient: [0.62, 0.63, 0.7],
            albedo_floor: 0.07,
            gi: None,
            rim: 0.9,
            contact_ao: 0.45,
            time: t,
            ..Default::default()
        };
        finish(ui, vec![lamp, fill], s)
    }
}

// 2. Cursor flashlight ------------------------------------------------------------

struct Flashlight;

/// Control ids for hit-testing: 5 toggles, 5 accent dots, the volume slider, 7 press buttons
/// (Dark pill, ⌥ Space chip, ⌘ ⇧ K keycaps, Reset, Record shortcut).
pub const TOGGLE0: usize = 0;
pub const DOT0: usize = 5;
pub const SLIDER: usize = 10;
pub const BUTTON0: usize = 11;
pub const N_CTL: usize = 18;
pub const DOT_HUES: [f32; 5] = [255.0, 300.0, 350.0, 40.0, 150.0];

pub fn accent_for(hue: f32) -> Color {
    oklch(0.7, 0.15, hue)
}

/// Sampled (already animated) state of the settings panel. `Panel::default()` is the headless demo.
#[derive(Clone)]
pub struct Panel {
    /// Toggle knob positions, 0 off … 1 on.
    pub toggles: [f32; 5],
    pub accent: Color,
    pub selected: usize,
    /// Opacity of the ring around the selected accent dot (0 in the headless demo).
    pub ring: f32,
    pub volume: f32,
    pub hover: [f32; N_CTL],
    pub press: [f32; N_CTL],
}

impl Default for Panel {
    fn default() -> Panel {
        Panel {
            toggles: [1.0, 0.0, 1.0, 0.0, 1.0],
            accent: accent_for(255.0),
            selected: 0,
            ring: 0.0,
            volume: 82.0 / 130.0,
            hover: [0.0; N_CTL],
            press: [0.0; N_CTL],
        }
    }
}

impl Panel {
    /// Height multiplier: hover lifts a little, pressing pushes it nearly flat.
    fn lift(&self, id: usize) -> f32 {
        (1.0 + 0.45 * self.hover[id]) * (1.0 - 0.92 * self.press[id].clamp(0.0, 1.2))
    }
}

/// A clickable region in points.
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub id: usize,
    pub rect: [f32; 4],
}

fn mix(a: Color, b: Color, k: f32) -> Color {
    let k = k.clamp(0.0, 1.0);
    Color { r: a.r + (b.r - a.r) * k, g: a.g + (b.g - a.g) * k, b: a.b + (b.b - a.b) * k, a: a.a + (b.a - a.a) * k }
}

fn settings_panel(ui: &mut Ui, card: Color, txt: Color, txt2: Color, hair: Color, st: &Panel) -> Vec<Hit> {
    let accent = st.accent;
    let mut hits = Vec::new();
    let raise_card = |p: Prim| p.raised(4.0 * sc()).bevel(5.0 * sc(), 1.5 * sc()).gloss(0.45, 0.45);
    let top = 4.0 * sc();
    ui.text(160.0, 70.0, "Settings", 24.0, 600.0, txt);
    ui.text(160.0, 106.0, "Appearance, sound and shortcuts", 13.0, 400.0, txt2);
    let rows: [(&str, &str, u8); 9] = [
        ("Appearance", "Dark", 2),
        ("Accent colour", "", 3),
        ("Reduce motion", "", 1),
        ("Sound effects", "", 0),
        ("Notification volume", "", 4),
        ("Push to talk", "⌥ Space", 2),
        ("Auto-update", "", 1),
        ("Send crash reports", "", 0),
        ("Launch at login", "", 1),
    ];
    let (mut toggle, mut button) = (0usize, 0usize);
    let y = 150.0;
    for (gi, group) in [&rows[0..3], &rows[3..6], &rows[6..9]].iter().enumerate() {
        let x = 160.0 + gi as f32 * 330.0;
        let h = group.len() as f32 * 56.0;
        ui.rect(x, y, 310.0, h, 14.0, card, Some((hair, 1.0)), raise_card);
        for (ri, (label, value, kind)) in group.iter().enumerate() {
            let ry = y + ri as f32 * 56.0;
            if ri > 0 {
                ui.paint(x + 16.0, ry, 278.0, 1.0, 0.0, hair);
            }
            ui.text(x + 18.0, ry + 19.0, label, 13.0, 500.0, txt);
            let rx = x + 310.0 - 18.0;
            match kind {
                0 | 1 => {
                    let id = TOGGLE0 + toggle;
                    toggle += 1;
                    let k = st.toggles[id - TOGGLE0];
                    let m = st.lift(id);
                    let track = mix(oklch(0.33, 0.006, 265.0), accent, k);
                    ui.rect(rx - 42.0, ry + 16.0, 42.0, 24.0, 12.0, track, None, |p| p.on(top).gloss(0.4, 0.4));
                    let kx = rx - 40.0 + 18.0 * k;
                    ui.rect(kx, ry + 18.0, 20.0, 20.0, 10.0, oklch(0.96, 0.0, 0.0), None, |p| {
                        p.on(top).raised(5.0 * sc() * m).bevel(10.0 * sc(), 4.0 * sc() * m).gloss(0.7, 0.3)
                    });
                    hits.push(Hit { id, rect: [rx - 42.0, ry + 16.0, 42.0, 24.0] });
                }
                2 => {
                    let id = BUTTON0 + button;
                    button += 1;
                    let m = st.lift(id);
                    let w = ui.measure(value, 12.0, 500.0) + 20.0;
                    ui.rect(rx - w, ry + 14.0, w, 28.0, 8.0, oklch(0.3, 0.006, 265.0), Some((hair, 1.0)), |p| {
                        p.on(top).raised(3.0 * sc() * m).bevel(4.0 * sc(), 1.2 * sc() * m).gloss(0.5, 0.4)
                    });
                    ui.text(rx - w + 10.0, ry + 20.0, value, 12.0, 500.0, txt);
                    hits.push(Hit { id, rect: [rx - w, ry + 14.0, w, 28.0] });
                }
                3 => {
                    for (k, h) in DOT_HUES.iter().enumerate() {
                        let id = DOT0 + k;
                        let m = st.lift(id);
                        let c = oklch(0.68, 0.15, *h);
                        let cx = rx - 18.0 - k as f32 * 26.0;
                        if k == st.selected && st.ring > 0.0 {
                            let o = 3.0;
                            let ring = oklcha(0.95, 0.0, 0.0, 0.8 * st.ring);
                            let r = [(cx - o) * sc(), (ry + 19.0 - o) * sc(), (18.0 + 2.0 * o) * sc(), (18.0 + 2.0 * o) * sc()];
                            ui.base.push(inst(r, (9.0 + o) * sc(), Color::TRANSPARENT, Some((ring, 1.5 * sc()))));
                        }
                        ui.rect(cx, ry + 19.0, 18.0, 18.0, 9.0, c, None, |p| p.on(top).raised(3.5 * sc() * m).bevel(9.0 * sc(), 3.0 * sc() * m).gloss(0.7, 0.3));
                        hits.push(Hit { id, rect: [cx - 4.0, ry + 15.0, 26.0, 26.0] });
                    }
                }
                _ => {
                    let m = st.lift(SLIDER);
                    let v = st.volume.clamp(0.0, 1.0);
                    ui.rect(rx - 130.0, ry + 26.0, 130.0, 4.0, 2.0, oklch(0.33, 0.006, 265.0), None, |p| p.on(top));
                    if v > 0.0 {
                        ui.rect(rx - 130.0, ry + 26.0, 130.0 * v, 4.0, 2.0, accent, None, |p| p.on(top));
                    }
                    ui.rect(rx - 140.0 + 130.0 * v, ry + 18.0, 20.0, 20.0, 10.0, oklch(0.96, 0.0, 0.0), None, |p| {
                        p.on(top).raised(6.0 * sc() * m).bevel(10.0 * sc(), 4.0 * sc() * m).gloss(0.7, 0.3)
                    });
                    // The slider's hit rect spans exactly the track in x, so value = (x - rect.x) / rect.w.
                    hits.push(Hit { id: SLIDER, rect: [rx - 130.0, ry + 12.0, 130.0, 32.0] });
                }
            }
        }
    }
    // A wide card below with buttons.
    let by = 340.0;
    ui.rect(160.0, by, 970.0, 150.0, 14.0, card, Some((hair, 1.0)), raise_card);
    ui.text(180.0, by + 20.0, "Keyboard shortcuts", 15.0, 600.0, txt);
    ui.text(180.0, by + 46.0, "Press a key combination to record a new shortcut.", 13.0, 400.0, txt2);
    for (i, k) in ["⌘", "⇧", "K"].iter().enumerate() {
        let id = BUTTON0 + 2 + i;
        let m = st.lift(id);
        ui.rect(180.0 + i as f32 * 52.0, by + 82.0, 44.0, 44.0, 10.0, oklch(0.3, 0.006, 265.0), Some((hair, 1.0)), |p| {
            p.on(top).raised(6.0 * sc() * m).bevel(7.0 * sc(), 2.5 * sc() * m).gloss(0.6, 0.35)
        });
        ui.text(195.0 + i as f32 * 52.0, by + 94.0, k, 15.0, 600.0, txt);
        hits.push(Hit { id, rect: [180.0 + i as f32 * 52.0, by + 82.0, 44.0, 44.0] });
    }
    let m = st.lift(BUTTON0 + 6);
    ui.rect(940.0, by + 90.0, 170.0, 36.0, 10.0, accent, None, |p| p.on(top).raised(5.0 * sc() * m).bevel(6.0 * sc(), 2.0 * sc() * m).gloss(0.7, 0.3));
    ui.text(975.0, by + 99.0, "Record shortcut", 13.0, 600.0, oklch(0.18, 0.03, 255.0));
    hits.push(Hit { id: BUTTON0 + 6, rect: [940.0, by + 90.0, 170.0, 36.0] });
    let m = st.lift(BUTTON0 + 5);
    ui.rect(780.0, by + 90.0, 140.0, 36.0, 10.0, oklch(0.3, 0.006, 265.0), Some((hair, 1.0)), |p| {
        p.on(top).raised(5.0 * sc() * m).bevel(6.0 * sc(), 2.0 * sc() * m).gloss(0.6, 0.35)
    });
    ui.text(828.0, by + 99.0, "Reset", 13.0, 600.0, txt);
    hits.push(Hit { id: BUTTON0 + 5, rect: [780.0, by + 90.0, 140.0, 36.0] });
    // Lower row of tiles.
    for i in 0..5 {
        let x = 160.0 + i as f32 * 198.0;
        ui.rect(x, 520.0, 180.0, 120.0, 14.0, card, Some((hair, 1.0)), raise_card);
        let names = ["Microphone", "Speakers", "Camera", "Display", "Battery"];
        ui.text(x + 18.0, 538.0, names[i], 13.0, 600.0, txt);
        ui.text(x + 18.0, 560.0, ["MacBook Pro", "Studio Display", "FaceTime HD", "Built-in", "92 %"][i], 12.0, 400.0, txt2);
        ui.rect(x + 18.0, 600.0, 144.0, 18.0, 9.0, oklch(0.27, 0.006, 265.0), None, |p| p.on(top));
        let fill = [0.7, 0.45, 0.0, 0.85, 0.92][i];
        if fill > 0.0 {
            ui.rect(x + 18.0, 600.0, 144.0 * fill, 18.0, 9.0, accent, None, |p| p.on(top).raised(2.0 * sc()).bevel(9.0 * sc(), 2.0 * sc()).gloss(0.6, 0.3));
        }
    }
    hits
}

impl Demo for Flashlight {
    fn name(&self) -> &'static str {
        "flashlight"
    }
    fn still(&self) -> f32 {
        1.15
    }
    fn frame(&mut self, ts: &mut TextSystem, t: f32) -> Frame {
        // The pointer wanders like a hand: one smooth closed path at an even pace.
        let path = [[300.0, 230.0], [700.0, 180.0], [1020.0, 300.0], [860.0, 470.0], [560.0, 600.0], [260.0, 520.0]];
        let p = loop_path(&path, t / 4.0 + 0.02);
        flashlight_scene(ts, t, p, &Panel::default(), false).0
    }
}

/// The dark settings panel lit by a torch aimed at the pointer `p` (points). `overhead` holds the torch straight
/// above the pointer so the hotspot sits exactly under it (the live window); otherwise it's held low and to the
/// right (the headless demo). Returns the panel's hit regions too.
pub fn flashlight_scene(ts: &mut TextSystem, t: f32, p: [f32; 2], panel: &Panel, overhead: bool) -> (Frame, Vec<Hit>) {
    let bg = oklch(0.16, 0.005, 265.0);
    let mut ui = Ui::new(ts, bg);
    let hits = settings_panel(
        &mut ui,
        oklch(0.225, 0.006, 265.0),
        oklch(0.94, 0.003, 265.0),
        oklch(0.66, 0.01, 265.0),
        oklcha(1.0, 0.0, 0.0, 0.06),
        panel,
    );
    let lp = if overhead { [p[0], p[1], 260.0] } else { [p[0] + 80.0, p[1] + 130.0, 260.0] };
    let axis = [p[0] - lp[0], p[1] - lp[1], -lp[2]];
    let torch = Light::point(lp[0] * sc(), lp[1] * sc(), lp[2] * sc())
        .color(lin(1.0, 0.95, 0.88))
        .intensity(7.0)
        .radius(9.0 * sc())
        .range(1800.0 * sc())
        .spot(axis, 0.58)
        .specular(1.4);
    let s = Settings {
        ambient: [0.07, 0.075, 0.09],
        albedo_floor: 0.04,
        text: TextPolicy { floor: 0.32, ceiling: 1.3, lit: 1.0, specular: 0.1 },
        rim: 1.0,
        contact_ao: 0.3,
        background_gloss: 0.5,
        time: t,
        ..Default::default()
    };
    (finish(ui, vec![torch], s), hits)
}

// 3. Emissive UI lights its neighbours ----------------------------------------------

struct Emissive {
    breath: Track,
    flip: f32,
    up: bool,
}

impl Demo for Emissive {
    fn name(&self) -> &'static str {
        "emissive"
    }
    fn still(&self) -> f32 {
        2.4
    }
    fn frame(&mut self, ts: &mut TextSystem, t: f32) -> Frame {
        // Breathing: a spring retargets between two levels every 1.1 s ("slow" .85/0), so the glow eases in and
        // out with no phase jump; lights cool, never cut.
        while t >= self.flip {
            self.up = !self.up;
            self.breath.retarget(self.flip as f64, if self.up { 1.0 } else { 0.55 }, Spring::new(0.85, 0.0));
            self.flip += 1.1;
        }
        let b = self.breath.value(t as f64);
        let bg = oklch(0.15, 0.006, 265.0);
        let mut ui = Ui::new(ts, bg);
        let card = oklch(0.23, 0.006, 265.0);
        let txt = oklch(0.94, 0.003, 265.0);
        let txt2 = oklch(0.64, 0.01, 265.0);
        let hair = oklcha(1.0, 0.0, 0.0, 0.06);
        let raise = |p: Prim| p.raised(6.0 * sc()).bevel(6.0 * sc(), 2.0 * sc()).gloss(0.5, 0.42);

        ui.text(140.0, 64.0, "Voice memo", 24.0, 600.0, txt);
        ui.text(140.0, 100.0, "Recording · Studio Display microphone", 13.0, 400.0, txt2);

        // Left column of cards around the recorder
        for (i, (title, sub)) in [("Transcript", "Live, 2 speakers"), ("Markers", "3 added"), ("Noise", "Voice isolation on")].iter().enumerate() {
            let y = 150.0 + i as f32 * 128.0;
            ui.rect(140.0, y, 300.0, 108.0, 16.0, card, Some((hair, 1.0)), raise);
            ui.text(162.0, y + 20.0, title, 14.0, 600.0, txt);
            ui.text(162.0, y + 44.0, sub, 13.0, 400.0, txt2);
        }
        // The recorder card in the middle
        ui.rect(470.0, 150.0, 400.0, 364.0, 20.0, card, Some((hair, 1.0)), raise);
        ui.text(494.0, 172.0, "00:42.18", 34.0, 600.0, txt);
        ui.text(494.0, 220.0, "Recording", 13.0, 500.0, oklch(0.75, 0.12, 25.0));
        // Waveform bars (flat, receive light)
        for k in 0..40 {
            let h = 8.0 + 46.0 * (0.5 + 0.5 * ((k as f32 * 0.7).sin() * (k as f32 * 0.23 + 1.0).cos())).abs();
            ui.rect(494.0 + k as f32 * 8.8, 290.0 - h * 0.5, 4.6, h, 2.3, oklch(0.5, 0.01, 265.0), None, |p| p.on(12.0).raised(2.0 * sc()));
        }
        // Glowing record button with a spring breath
        let red = oklch(0.66, 0.2, 25.0);
        let red_lin = [red.r, red.g, red.b];
        let glow = 7.0 * b;
        ui.rect(620.0, 370.0, 100.0, 100.0, 50.0, oklch(0.27, 0.008, 265.0), Some((hair, 1.0)), |p| {
            p.on(12.0).bevel(16.0 * sc(), 4.0 * sc()).gloss(0.6, 0.35)
        });
        ui.rect(640.0, 390.0, 60.0, 60.0, 30.0, red, None, |p| p.on(12.0).emissive([red_lin[0] * glow, red_lin[1] * glow, red_lin[2] * glow]));

        // Progress bar under it, emitting cyan, growing
        let prog = 0.25 + 0.5 * (t / 4.0).min(1.0);
        let cyan = oklch(0.78, 0.13, 210.0);
        ui.rect(140.0, 560.0, 730.0, 10.0, 5.0, oklch(0.24, 0.006, 265.0), None, |p| p);
        let g2 = 1.9 * (0.85 + 0.15 * b);
        ui.rect(140.0, 560.0, 730.0 * prog, 10.0, 5.0, cyan, None, |p| p.emissive([cyan.r * g2, cyan.g * g2, cyan.b * g2]));
        ui.text(140.0, 584.0, "Uploading to OpenCode · 3 of 5 files", 12.0, 500.0, txt2);

        // Right column: notifications with a small green status dot (emissive)
        let green = oklch(0.78, 0.17, 150.0);
        for (i, (title, sub)) in [("Agent finished", "rune-light demos rendered"), ("Build passed", "ci · 1m 12s"), ("Review", "Argon: no blockers")].iter().enumerate() {
            let y = 150.0 + i as f32 * 128.0;
            ui.rect(900.0, y, 300.0, 108.0, 16.0, card, Some((hair, 1.0)), raise);
            ui.text(940.0, y + 20.0, title, 14.0, 600.0, txt);
            ui.text(940.0, y + 44.0, sub, 13.0, 400.0, txt2);
            let gd = if i == 0 { 2.2 * (0.6 + 0.4 * b) } else { 0.0 };
            let c = if i == 0 { green } else { oklch(0.45, 0.01, 265.0) };
            ui.rect(918.0, y + 24.0, 10.0, 10.0, 5.0, c, None, |p| p.on(12.0).emissive([c.r * gd, c.g * gd, c.b * gd]));
        }
        // Bottom dock of chips
        for i in 0..6 {
            let x = 140.0 + i as f32 * 178.0;
            ui.rect(x, 640.0, 160.0, 90.0, 16.0, card, Some((hair, 1.0)), raise);
            ui.text(x + 18.0, 660.0, ["Pause", "Marker", "Trim", "Share", "Export", "Delete"][i], 13.0, 600.0, txt);
        }

        let s = Settings {
            ambient: [0.3, 0.31, 0.35],
            gi: Some(Gi { strength: 1.0, air: 0.09, bounce: 0.0, absorption: 0.24, ..Default::default() }),
            contact_ao: 0.4,
            rim: 0.6,
            time: t,
            ..Default::default()
        };
        finish(ui, vec![], s)
    }
}

// 4. Raised material -------------------------------------------------------------------

struct Raised;

impl Demo for Raised {
    fn name(&self) -> &'static str {
        "raised"
    }
    fn still(&self) -> f32 {
        1.4
    }
    fn frame(&mut self, ts: &mut TextSystem, t: f32) -> Frame {
        let bg = oklch(0.2, 0.004, 265.0);
        let mut ui = Ui::new(ts, bg);
        let txt = oklch(0.92, 0.003, 265.0);
        let txt2 = oklch(0.62, 0.008, 265.0);
        let body = oklch(0.255, 0.004, 265.0);
        let key = oklch(0.29, 0.004, 265.0);
        // Device body
        ui.rect(120.0, 90.0, 1040.0, 620.0, 36.0, body, Some((oklcha(1.0, 0.0, 0.0, 0.05), 1.0)), |p| {
            p.raised(10.0 * sc()).bevel(22.0 * sc(), 7.0 * sc()).gloss(0.7, 0.32)
        });
        ui.text(170.0, 130.0, "OP-1 Field · Control surface", 15.0, 600.0, txt);
        ui.text(170.0, 156.0, "Bevels from the SDF catch the light", 12.0, 400.0, txt2);
        // Four encoder knobs
        let hues = [255.0, 150.0, 60.0, 25.0];
        for i in 0..4 {
            let cx = 200.0 + i as f32 * 140.0;
            ui.rect(cx - 48.0, 210.0, 96.0, 96.0, 48.0, key, None, |p| p.floating(10.0 * sc(), 22.0 * sc()).bevel(30.0 * sc(), 10.0 * sc()).gloss(0.9, 0.26));
            let c = oklch(0.7, 0.14, hues[i]);
            ui.rect(cx - 5.0, 222.0, 10.0, 10.0, 5.0, c, None, |p| p);
        }
        // Screen (flat, recessed look by being darker; glossy)
        ui.rect(780.0, 200.0, 330.0, 200.0, 18.0, oklch(0.14, 0.01, 265.0), None, |p| p.gloss(1.0, 0.18));
        ui.text(806.0, 226.0, "128 BPM", 28.0, 600.0, oklch(0.85, 0.12, 150.0));
        ui.text(806.0, 270.0, "Pattern A · bar 12", 13.0, 400.0, oklch(0.6, 0.06, 150.0));
        // Keys: two rows
        for r in 0..2 {
            for i in 0..12 {
                let x = 170.0 + i as f32 * 78.0;
                let y = 450.0 + r as f32 * 110.0;
                let accent = (r == 0 && (i == 2 || i == 7)) || (r == 1 && i == 10);
                let c = if accent { oklch(0.62, 0.17, 35.0) } else { key };
                ui.rect(x, y, 68.0, 92.0, 14.0, c, None, |p| p.floating(10.0 * sc(), 18.0 * sc()).bevel(12.0 * sc(), 4.5 * sc()).gloss(0.85, 0.28));
                ui.text(x + 12.0, y + 12.0, ["C", "D", "E", "F", "G", "A", "B", "C", "D", "E", "F", "G"][i], 12.0, 600.0, if accent { oklch(0.2, 0.03, 35.0) } else { txt2 });
            }
        }
        // The light sweeps across on a slow arc (one smooth path, no reversal mid-clip).
        let u = (t / self.seconds()).clamp(0.0, 1.0);
        let e = u * u * (3.0 - 2.0 * u);
        let x = 40.0 + e * 1200.0;
        let y = 560.0 - 300.0 * (1.0 - (2.0 * e - 1.0).powi(2));
        let key_l = Light::point(x * sc(), y * sc(), 150.0 * sc()).color(lin(1.0, 0.93, 0.85)).intensity(3.4).radius(26.0 * sc()).range(2200.0 * sc()).specular(2.4);
        let fill = Light::point(1400.0 * sc(), 900.0 * sc(), 700.0 * sc())
            .color(lin(0.5, 0.62, 1.0))
            .intensity(0.4)
            .radius(200.0 * sc())
            .range(5000.0 * sc())
            .specular(0.6);
        let s = Settings { ambient: [0.3, 0.31, 0.34], rim: 1.1, contact_ao: 0.5, time: t, ..Default::default() };
        finish(ui, vec![key_l, fill], s)
    }
}

// 5. God rays --------------------------------------------------------------------------

struct GodRays;

impl Demo for GodRays {
    fn name(&self) -> &'static str {
        "godrays"
    }
    fn still(&self) -> f32 {
        1.0
    }
    fn frame(&mut self, ts: &mut TextSystem, t: f32) -> Frame {
        let bg = oklch(0.11, 0.014, 265.0);
        let mut ui = Ui::new(ts, bg);
        let card = oklch(0.21, 0.008, 265.0);
        let txt = oklch(0.93, 0.003, 265.0);
        let txt2 = oklch(0.6, 0.01, 265.0);
        let hair = oklcha(1.0, 0.0, 0.0, 0.06);
        let names = [
            "Inbox", "Calendar", "Notes", "Music", "Photos", "Maps", "Files", "Terminal", "Mail", "Weather", "Clock", "Books", "Stocks", "Podcasts",
            "Home", "Wallet", "Health", "Camera", "Code", "Fleet", "Chat", "Voice", "Launcher", "Rune",
        ];
        let (cols, rows) = (8usize, 3usize);
        let (cw, ch, gx, gy) = (124.0, 124.0, 28.0, 28.0);
        let x0 = (1280.0 - (cols as f32 * cw + (cols - 1) as f32 * gx)) * 0.5;
        let y0 = 290.0;
        for r in 0..rows {
            for c in 0..cols {
                let x = x0 + c as f32 * (cw + gx);
                let y = y0 + r as f32 * (ch + gy);
                let i = r * cols + c;
                ui.rect(x, y, cw, ch, 24.0, card, Some((hair, 1.0)), |p| p.raised(14.0 * sc()).bevel(8.0 * sc(), 2.5 * sc()).gloss(0.5, 0.4));
                let hue = (i as f32 * 47.0) % 360.0;
                ui.rect(x + 20.0, y + 20.0, 40.0, 40.0, 12.0, oklch(0.62, 0.12, hue), None, |p| p);
                ui.text(x + 20.0, y + 84.0, names[i], 13.0, 600.0, txt);
            }
        }
        ui.text(x0, 120.0, "Good evening", 30.0, 600.0, txt);
        ui.text(x0, 164.0, "24 apps · light from behind the grid", 14.0, 400.0, txt2);
        // The source sits behind the grid's top edge, lower than the cards, and drifts once across: every gap
        // throws a shaft and the fan swings with it.
        let u = (t / self.seconds()).clamp(0.0, 1.0);
        let e = u * u * (3.0 - 2.0 * u);
        let lx = 488.0 + e * 304.0;
        let warm = lin(1.0, 0.8, 0.55);
        let sun = Light::point(lx * sc(), 428.0 * sc(), 4.0 * sc())
            .color(warm)
            .intensity(1.6)
            .radius(12.0 * sc())
            .range(3000.0 * sc())
            .shafts(1.5)
            .softness(2.2);
        let s = Settings { ambient: [0.5, 0.5, 0.58], shafts_over: 0.12, rim: 0.8, contact_ao: 0.35, time: t, ..Default::default() };
        finish(ui, vec![sun], s)
    }
}


pub(super) fn all_demos() -> Vec<Box<dyn Demo>> {
    vec![
        Box::new(Chat::new()),
        Box::new(Flashlight),
        Box::new(Emissive { breath: Track::at(0.8), flip: 0.0, up: false }),
        Box::new(Raised),
        Box::new(GodRays),
    ]
}
