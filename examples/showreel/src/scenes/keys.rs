//! A pad controller playing a boom-bap pattern at 90 BPM: glossy plastic keys, two liquid-metal
//! keys, each press springing down and lighting up.

use rune::{Cx, El, col, el, row};

use crate::gfx::{CHROME, GLOW, PLASTIC};
use crate::kit::*;
use crate::State;

const EIGHTH: f64 = 60.0 / 90.0 / 2.0;
/// (colour index, label) per key; colour 99 is liquid metal.
const KEYS: [(u32, &str); 16] = [
    (10, "A"), (9, "B"), (9, "C"), (9, "D"), (10, "E"), (12, "F"), (99, "G"), (10, "H"),
    (11, "KICK"), (10, "I"), (9, "J"), (10, "K"), (12, "SNR"), (9, "L"), (99, "M"), (11, "FX"),
];
const LIGHT: [u32; 16] = [0, 9, 9, 9, 0, 12, 0, 0, 11, 0, 9, 0, 12, 9, 0, 11];

fn hits(n: i64) -> Vec<usize> {
    let mut v = Vec::new();
    let m = n.rem_euclid(16);
    if [0, 3, 5, 8, 11, 13].contains(&m) {
        v.push(8);
    }
    if m % 4 == 2 {
        v.push(12);
    }
    v.push(if m % 2 == 0 { 2 } else { 1 });
    if m == 7 || m == 15 {
        v.push(6);
    }
    if m == 14 {
        v.push(14);
    }
    if m == 10 {
        v.push(5);
    }
    if m == 6 {
        v.push(13);
    }
    v
}

fn press_of(t: f64, key: usize) -> f32 {
    let now = (t / EIGHTH).floor() as i64;
    let mut p = 0.0f32;
    for n in (now - 3)..=now {
        if n < 0 {
            continue;
        }
        if hits(n).contains(&key) {
            p = p.max(pressed(t, n as f64 * EIGHTH));
        }
    }
    p
}

pub fn view(s: &State, _: &mut Cx<State>) -> El {
    let t_ = s.t;
    let (bw, bh) = (864.0, 384.0);
    let (bx, by) = ((W - bw) / 2.0, (H - bh) / 2.0 + 4.0);
    let key = 86.0;
    let gap = 12.0;
    let kx0 = bx + (bw - 8.0 * key - 7.0 * gap) / 2.0;
    let ky0 = by + 152.0;

    let bar = 12 + (t_ / (EIGHTH * 8.0)) as i64;
    let display = col().absolute(kx0, by + 38.0).w(230.0).h(70.0).px(16.0).justify_center().gap(4.0).radius(12.0).fill(c(0x050506)).border(1.0, ca(0xffffff, 0.05))
        .child(row().gap(8.0).items_end().child(mono("90", 30.0, 0x5df2a8)).child(mono("BPM", 13.0, 0x5df2a8).mb(5.0)))
        .child(mono(format!("pattern A · bar {bar}"), 11.0, 0x3c9b6c));

    let mut root = el().w(W).h(H).linear_gradient(1.57, [(0.0, c(0x17161a)), (1.0, c(0x0a0a0b))])
        .child(el().absolute(bx, by).size(bw, bh).radius(30.0).linear_gradient(1.57, [(0.0, c(0x232227)), (1.0, c(0x161619))]).border(1.0, ca(0xffffff, 0.08)).shadow(30.0, 60.0, ca(0x000000, 0.6)))
        .child(display)
        .child(col().absolute(bx + bw - (kx0 - bx) - 120.0, by + 44.0).w(120.0).items_end().gap(8.0).child(t("PAD-16", 13.0, 600.0, 0x8a8a93)).child(row().gap(6.0).center_y().child(dot(0xff5a4d, 7.0)).child(t("REC", 12.0, 600.0, 0xff5a4d))));

    // Knobs that turn slowly.
    for i in 0..4 {
        let kx = kx0 + 280.0 + i as f32 * 70.0;
        let ky = by + 46.0;
        let ang = (t_ as f32 * 0.9 + i as f32 * 1.3).sin() * 2.2 - 1.5708;
        let (dx, dy) = (ang.cos() * 16.0, ang.sin() * 16.0);
        root = root.child(fx(PLASTIC, [10.0, 0.35, 0.7, 10.0]).absolute(kx, ky).size(54.0, 54.0).radius(27.0))
            .child(dot(0xf1ece1, 6.0).absolute(kx + 27.0 + dx - 3.0, ky + 27.0 + dy - 3.0));
    }

    for (i, (col_, label)) in KEYS.iter().enumerate() {
        let (cx, cy) = ((i % 8) as f32, (i / 8) as f32);
        let p = press_of(t_, i);
        let x = kx0 + cx * (key + gap);
        let y = ky0 + cy * (key + gap) + 3.0 * p;
        let size = key * (1.0 - 0.04 * p);
        let off = (key - size) / 2.0;
        let dark = matches!(*col_, 10 | 99);
        let face = if *col_ == 99 {
            fx(CHROME, [0.0, 0.0, 1.4, 1.0])
        } else {
            fx(PLASTIC, [*col_ as f32, 0.22, 0.35, 7.0])
        };
        root = root
            .child(el().absolute(x, ky0 + cy * (key + gap) + 5.0).size(key, key).radius(14.0).fill(ca(0x000000, 0.45)))
            .child(fx(GLOW, [0.0, LIGHT[i] as f32, 1.4 * p, 16.0]).absolute(x + off, y + off).size(size, size).radius(14.0).effect_bleed(50.0))
            .child(face.absolute(x + off, y + off).size(size, size).radius(14.0))
            .child(el().absolute(x + off + 10.0, y + off + 8.0).child(ta(*label, 11.5, 600.0, if dark && *col_ != 99 { 0xb9b9c2 } else { 0x2a2a2e }, 0.85)));
    }
    root
}
