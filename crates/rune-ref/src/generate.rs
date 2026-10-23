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
            4 => p.uv = color(10),
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
    }
}
