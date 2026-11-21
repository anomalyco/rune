//! One bounded decoder shared by proptest, corpus generation and CLI replay.
//! Each 32-byte record shrinks independently; deleting records deletes primitives.
use crate::{Atlas, NO_CLIP, Primitive, Scene};

pub fn scene(records: &[[u8; 32]]) -> Scene {
    let mut scene = Scene {
        width: 64,
        height: 64,
        clear: [0.025, 0.04, 0.06, 1.0],
        primitives: Vec::new(),
        atlas: Atlas {
            width: 32,
            height: 32,
            data: (0..1024)
                .map(|n| ((n * 73 + n / 32 * 19) % 256) as u8)
                .collect(),
        },
    };
    for (index, b) in records.iter().take(32).enumerate() {
        let a = b[9] as f32 / 255.0;
        let color = |start: usize| {
            [
                b[start] as f32 / 255.0 * a,
                b[start + 1] as f32 / 255.0 * a,
                b[start + 2] as f32 / 255.0 * a,
                a,
            ]
        };
        let kind = (b[0] % 5) as f32;
        let mut p = Primitive {
            node: index as u32 + 1,
            source: format!("generated/{index}"),
            rect: [
                b[1] as f32 / 4.0 - 8.0,
                b[2] as f32 / 4.0 - 8.0,
                1.0 + (b[3] % 128) as f32 / 4.0,
                1.0 + (b[4] % 128) as f32 / 4.0,
            ],
            radii: std::array::from_fn(|n| b[20 + n] as f32 / 8.0),
            fill: color(6),
            border: color(10),
            params: [
                b[13] as f32 / 32.0,
                kind,
                b[14] as f32 / 32.0,
                0.7 + b[15] as f32 / 255.0,
            ],
            uv: [0.0; 4],
            clip: NO_CLIP,
            fade: [0.0; 4],
            fade_origin: None,
            turn: None,
            clip_radii: None,
        };
        if b[16] & 1 != 0 {
            p.clip = [
                b[17] as f32 / 8.0,
                b[18] as f32 / 8.0,
                32.0 + b[19] as f32 / 8.0,
                32.0 + b[24] as f32 / 8.0,
            ];
            p.fade = std::array::from_fn(|n| (b[25 + n] % 32) as f32 / 4.0);
        }
        match kind as u32 {
            1 => p.radii = [p.radii[0]; 4],
            2 => {
                // Atlas glyphs carry neither corner radii nor outline/LCD flags and read page 0.
                p.radii = [0.0; 4];
                // params[0] turns a glyph. At arbitrary angles a rotated edge can pass within
                // the rasterizer's sub-pixel precision of a pixel centre, where it and the
                // oracle's inside test may disagree by a whole pixel. Quarter turns have their
                // own test.
                p.params[0] = 0.0;
                p.params[2] = 0.0;
                // Guard texels keep all textureLoad taps defined on every backend.
                p.uv = [1.0, 1.0, 0.0, 0.0];
                p.rect[2] = 1.0 + (b[3] % 24) as f32;
                p.rect[3] = 1.0 + (b[4] % 24) as f32;
                if b[16] & 2 != 0 {
                    p.uv[2] = p.rect[2];
                    p.uv[3] = p.rect[3];
                    let scale = 0.5 + (b[29] % 8) as f32 / 4.0;
                    p.rect[2] *= scale;
                    p.rect[3] *= scale;
                }
            }
            3 => {
                p.uv = [
                    b[30] as f32 / 32.0 - 4.0,
                    b[31] as f32 / 32.0 - 4.0,
                    0.0,
                    0.0,
                ]
            }
            // Kinds 2 and 3 are the only other readers of bytes 29..32.
            4 => p.uv = color(29),
            _ => {}
        }
        scene.primitives.push(p);
    }
    scene
}

/// Stable SplitMix64 corpus, independent of proptest's evolving RNG algorithm.
pub fn seeded(seed: u64, count: usize) -> Scene {
    let mut state = seed;
    let records: Vec<[u8; 32]> = (0..count.min(32))
        .map(|_| {
            std::array::from_fn(|_| {
                state = state.wrapping_add(0x9e3779b97f4a7c15);
                let mut z = state;
                z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
                (z ^ (z >> 31)) as u8
            })
        })
        .collect();
    scene(&records)
}

/// `seeded`, with every primitive redrawn as a dashed border ring (kind 10). A separate corpus,
/// so adding the kind left the canonical hashes of `seeded` alone.
pub fn dashed(seed: u64, count: usize) -> Scene {
    let mut scene = seeded(seed, count);
    for p in &mut scene.primitives {
        let b = |n: usize| (p.radii[n % 4] * 8.0 + p.rect[n % 4] * 4.0) as u32;
        let bw = 0.5 + (b(0) % 8) as f32 * 0.25;
        p.params = [bw, 10.0, if b(1) % 4 == 0 { p.params[2] } else { 0.0 }, 1.0];
        let dash = 1.0 + (b(2) % 24) as f32 * 0.5;
        let mut gap = 1.0 + (b(3) % 16) as f32 * 0.5;
        // Avoid exact half-integer `total / nominal`, where a 1-ULP GPU reciprocal division
        // can flip `floor(total / nominal + 0.5)` by a whole dash period.
        let h = bw * 0.5;
        let m = (p.rect[2] * 0.5).min(p.rect[3] * 0.5);
        let rc = p.radii.map(|r| (r.abs().min(m) - h).max(0.0));
        let total = (p.rect[2] - bw - rc[0] - rc[1]).max(0.0)
            + (p.rect[3] - bw - rc[1] - rc[2]).max(0.0)
            + (p.rect[2] - bw - rc[2] - rc[3]).max(0.0)
            + (p.rect[3] - bw - rc[3] - rc[0]).max(0.0)
            + rc.iter().map(|r| r * 1.5707964).sum::<f32>();
        let periods = total / (dash + gap);
        if (periods - libm::floorf(periods) - 0.5).abs() < 1e-3 {
            gap += 0.25;
        }
        p.uv = [dash, gap, (b(1) % 32) as f32 * 0.75 - 6.0, 0.0];
        if b(0) % 3 == 0 { p.border = p.fill; }
    }
    scene
}

/// `seeded`, with every primitive redrawn as a radial or angled linear gradient (kind 11),
/// including multi-stop open band edges (`+ 2.0` on `t0` / `t1`).
pub fn radial(seed: u64, count: usize) -> Scene {
    let mut scene = seeded(seed, count);
    let mut out = Vec::with_capacity(scene.primitives.len() * 2);
    for mut p in scene.primitives {
        let b = |n: usize| (p.radii[n % 4] * 8.0 + p.rect[n % 4] * 4.0) as u32;
        let t0 = (b(0) % 4) as f32 * 0.1;
        let t1 = (t0 + 0.35 + (b(1) % 6) as f32 * 0.1).min(1.0);
        let soft = if b(1) % 4 == 0 { p.params[2] } else { 0.0 };
        p.uv = if b(0) % 4 == 0 {
            let angle = (b(2) % 16) as f32 * (std::f32::consts::TAU / 16.0);
            let (dx, dy) = (libm::sinf(angle), -libm::cosf(angle));
            let span = (dx.abs() * p.rect[2] + dy.abs() * p.rect[3]).max(1.0);
            [dx / span, dy / span, 0.0, 0.0]
        } else {
            let cx = (0.2 + (b(2) % 7) as f32 * 0.1) * p.rect[2];
            let cy = (0.2 + (b(3) % 7) as f32 * 0.1) * p.rect[3];
            let rx = (4.0 + (b(0) % 24) as f32).max(1.0);
            let ry = if b(1) % 2 == 0 { rx } else { (4.0 + (b(1) % 24) as f32).max(1.0) };
            [cx, cy, rx, ry]
        };
        if b(2) % 2 == 0 {
            let t_mid = 0.5 * (t0 + t1);
            let mid = [
                0.5 * (p.fill[0] + p.border[0]),
                0.5 * (p.fill[1] + p.border[1]),
                0.5 * (p.fill[2] + p.border[2]),
                0.5 * (p.fill[3] + p.border[3]),
            ];
            let mut inner = p.clone();
            inner.border = mid;
            inner.params = [t0, 11.0, soft, t_mid + 2.0];
            out.push(inner);
            p.fill = mid;
            p.params = [t_mid + 2.0, 11.0, soft, t1];
            out.push(p);
        } else {
            p.params = [t0, 11.0, soft, t1];
            out.push(p);
        }
    }
    scene.primitives = out;
    scene
}

pub fn translate(scene: &mut Scene, x: f32, y: f32) {
    for p in &mut scene.primitives {
        p.rect[0] += x;
        p.rect[1] += y;
        if p.clip != NO_CLIP {
            p.clip[0] += x;
            p.clip[2] += x;
            p.clip[1] += y;
            p.clip[3] += y;
        }
        if let Some(origin) = &mut p.fade_origin
            && *origin != NO_CLIP
        {
            origin[0] += x;
            origin[2] += x;
            origin[1] += y;
            origin[3] += y;
        }
        if let Some(turn) = &mut p.turn {
            turn[1] += x;
            turn[2] += y;
        }
        match p.params[1] as u32 {
            5 => {
                p.radii[0] += x;
                p.radii[1] += y;
                p.radii[2] += x;
                p.radii[3] += y;
            }
            6 => {
                p.radii[0] += x;
                p.radii[1] += y;
                p.radii[2] += x;
                p.radii[3] += y;
                p.uv[0] += x;
                p.uv[1] += y;
            }
            8 if p.params[3] == 0.0 && p.radii[3] > 0.0 => {
                p.radii[0] += x;
                p.radii[1] += y;
            }
            _ => {}
        }
    }
}
