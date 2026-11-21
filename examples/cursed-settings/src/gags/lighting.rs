//! Scene lighting for the gags: brightness, dark mode (lights out and torch), spotlight,
//! contrast, and the lightning strike's flash.

use rune_light::{Light, Settings, TextPolicy};
use rune_motion::{Animated, Spring, Vec2};

use super::{Gags, In, mix, smooth};

impl Gags {
    pub(super) fn lighting(
        &mut self,
        i: &In,
        storm: f32,
        flash: f32,
        flash_at: [f32; 2],
        anim: &mut bool,
    ) -> (Vec<Light>, Settings, f32) {
        let p = &i.s.p;
        let t = i.t;
        let rm = p.reduce_motion;
        let tg = &i.targets;
        let px = |q: [f32; 2]| [q[0] * i.scale, q[1] * i.scale];
        let mut lights: Vec<Light> = Vec::new();

        let mut amb = 0.1 + 0.9 * p.brightness.clamp(0.0, 1.0);
        amb *= mix(1.0, 0.32, storm);

        let dark_on = p.appearance == 1;
        self.dark.set(dark_on, t, rm);
        let (darkness, torch) = if self.dark.is_on() {
            let u = self.dark.since(t);
            let flick = if rm {
                smooth(0.0, 0.2, u)
            } else if u < 0.05 {
                0.85
            } else if u < 0.11 {
                0.1
            } else if u < 0.17 {
                0.92
            } else if u < 0.23 {
                0.45
            } else {
                1.0
            };
            let from = self.dark.from;
            let d = mix(from, 1.0, flick);
            (d, smooth(0.0, 0.18, u - if rm { 0.2 } else { 0.6 }))
        } else {
            let d = self.dark.at(t);
            (d, 0.0)
        };
        *anim |= self.dark.moving(t) || self.dark.is_on() && self.dark.since(t) < 1.0;
        amb *= mix(1.0, 0.006, darkness);
        if torch > 0.0 {
            if let Some(q) = i.pointer {
                self.torch = q;
            }
            let q = self.torch;
            let lp = [q[0] + 80.0, q[1] + 130.0];
            let (a, b) = (px(q), px(lp));
            let h = 260.0;
            out_light(&mut lights, Light::point(b[0], b[1], h * i.scale)
                .color([1.0, 0.95, 0.87])
                .intensity(7.0)
                .radius(9.0 * i.scale)
                .range(1800.0 * i.scale)
                .spot([a[0] - b[0], a[1] - b[1], -h * i.scale], 0.56)
                .specular(1.3)
                .scaled(torch));
        }

        let spot_switched = self.spot.set(p.spotlight, t, rm);
        let sv = self.spot.at(t);
        *anim |= self.spot.moving(t);
        if let Some((r, _)) = tg.search {
            let target = Vec2 { x: r.x + r.w * 0.5, y: r.y + r.h * 0.5 };
            let sp = self.spot_pos.get_or_insert_with(|| Animated::new(target));
            if spot_switched && p.spotlight && !rm {
                // Swing in from off-stage, upper right.
                sp.set(Vec2 { x: i.vp.x + i.vp.w + 60.0, y: i.vp.y + 40.0 });
            }
            if rm {
                sp.set(target);
            } else {
                sp.animate_to(target, Spring::new(1.15, 0.16), i.now);
            }
            *anim |= sv > 0.0 && !sp.is_settled(i.now);
            if sv > 0.003 {
                let q = sp.sample(i.now);
                let q = [q.x, q.y];
                let lp = [q[0] + 150.0, q[1] - 90.0];
                let h = 420.0;
                let (a, b) = (px(q), px(lp));
                out_light(&mut lights, Light::point(b[0], b[1], h * i.scale)
                    .color([1.0, 0.93, 0.8])
                    .intensity(4.5)
                    .radius(5.0 * i.scale)
                    .range(2400.0 * i.scale)
                    .spot([a[0] - b[0], a[1] - b[1], -h * i.scale], 0.27)
                    .specular(1.5)
                    .scaled(sv));
            }
        }
        amb *= mix(1.0, 0.025, sv);

        self.contrast.set(p.contrast, t, rm);
        let cv = self.contrast.at(t);
        *anim |= self.contrast.moving(t);
        if cv > 0.003 {
            let lp = px([i.vp.x - 90.0, i.vp.y + i.vp.h * 0.32]);
            out_light(&mut lights, Light::point(lp[0], lp[1], 26.0 * i.scale)
                .color([1.0, 0.88, 0.72])
                .intensity(6.0)
                .radius(12.0 * i.scale)
                .range(4000.0 * i.scale)
                .specular(1.6)
                .scaled(cv));
        }
        amb *= mix(1.0, 0.28, cv);

        if flash > 0.003 {
            let q = px([flash_at[0] + 30.0, flash_at[1] - 120.0]);
            out_light(&mut lights, Light::point(q[0], q[1], 300.0 * i.scale)
                .color([0.85, 0.9, 1.0])
                .intensity(6.0)
                .radius(30.0 * i.scale)
                .range(5000.0 * i.scale)
                .scaled(flash));
        }

        let gloom = darkness.max(sv * 0.85).max(cv * 0.6);
        let settings = Settings {
            ambient: [amb, amb, amb * 1.05],
            text: TextPolicy { floor: mix(0.82, 0.08, gloom), ceiling: 1.3, lit: 1.0, specular: 0.12 },
            rim: 0.8 + 0.8 * cv,
            contact_ao: 0.35 + 0.2 * cv,
            albedo_floor: 0.05,
            background_gloss: 0.35,
            ..Default::default()
        };
        let lit = ((1.0 - amb) * 8.0 + (flash + torch + sv + cv) * 8.0).clamp(0.0, 1.0);
        (lights, settings, lit)
    }
}

fn out_light(v: &mut Vec<Light>, l: Light) {
    v.push(l);
}
