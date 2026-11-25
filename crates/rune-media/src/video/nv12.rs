//! Pure-Rust NV12/BGRA to upright sRGB RGBA8 conversion for video thumbnails.
//!
//! Colour equations (limited/full range, BT.601/BT.709 matrix, BT.709-to-sRGB transfer) and
//! orientation mapping match `crates/rune-media/src/native_surface/video.wgsl`.

use crate::native_surface::{ByteFrame, Format, Matrix, Orient, Transfer};
use std::sync::LazyLock;

const LUT_STEPS: usize = 4096;

/// Precomputed BT.709 optical transfer -> sRGB 8-bit encoding LUT (matches `video.wgsl`).
static BT709_TO_SRGB: LazyLock<[u8; LUT_STEPS + 1]> = LazyLock::new(|| {
    std::array::from_fn(|i| {
        let x = i as f32 / LUT_STEPS as f32;
        let linear = if x < 0.081 { x / 4.5 } else { ((x + 0.099) / 1.099).powf(1.0 / 0.45) };
        let v = linear.clamp(0.0, 1.0);
        let srgb = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (srgb.clamp(0.0, 1.0) * 255.0).round() as u8
    })
});

#[inline]
fn encode_channel(v: f32, transfer: Transfer, lut: &[u8; LUT_STEPS + 1]) -> u8 {
    let x = v.clamp(0.0, 1.0);
    match transfer {
        Transfer::Srgb => (x * 255.0).round() as u8,
        Transfer::Bt709 => lut[(x * LUT_STEPS as f32).round() as usize],
    }
}

#[inline]
fn source_coord(orient: Orient, bw: usize, bh: usize, x: usize, y: usize) -> (usize, usize) {
    let [m0, m1, m2, m3] = orient.0;
    let sx = match (m0, m1) {
        (1, 0) => x,
        (-1, 0) => bw - 1 - x,
        (0, 1) => y,
        (0, -1) => bw - 1 - y,
        _ => x.min(bw - 1),
    };
    let sy = match (m2, m3) {
        (1, 0) => x,
        (-1, 0) => bh - 1 - x,
        (0, 1) => y,
        (0, -1) => bh - 1 - y,
        _ => y.min(bh - 1),
    };
    (sx, sy)
}

/// Converts `frame` (NV12 limited/full or BGRA) into upright opaque sRGB RGBA8, applying
/// `orient` (quarter-turns and flips) in one pass.
pub(crate) fn byte_frame_to_rgba(frame: &ByteFrame, orient: Orient) -> Result<(u32, u32, Vec<u8>), String> {
    if frame.width == 0 || frame.height == 0 {
        return Err("decoded an empty frame".into());
    }
    let bw = frame.width as usize;
    let bh = frame.height as usize;
    let [uw_u32, uh_u32] = orient.upright([frame.width, frame.height]);
    let (uw, uh) = (uw_u32 as usize, uh_u32 as usize);
    let transfer = frame.color.transfer;
    let lut = &*BT709_TO_SRGB;

    match frame.format {
        Format::Nv12Video | Format::Nv12Full => {
            let luma = &frame.planes[0];
            let chroma = &frame.planes[1];
            let chroma_w = bw.div_ceil(2);
            let chroma_h = bh.div_ceil(2);
            let chroma_stride = chroma_w * 2;
            if luma.len() < bw.saturating_mul(bh) || chroma.len() < chroma_stride.saturating_mul(chroma_h) {
                return Err("decoded NV12 buffer is truncated".into());
            }
            let limited = frame.format == Format::Nv12Video;
            let (kr, kb) = match frame.color.matrix {
                Matrix::Bt601 => (0.299f32, 0.114f32),
                Matrix::Bt709 => (0.2126f32, 0.0722f32),
            };
            let kg = 1.0 - kr - kb;
            let r_cr = 2.0 * (1.0 - kr);
            let b_cb = 2.0 * (1.0 - kb);

            let mut out = vec![0u8; uw * uh * 4];
            for y in 0..uh {
                let row_off = y * uw * 4;
                for x in 0..uw {
                    let (sx, sy) = source_coord(orient, bw, bh, x, y);
                    let mut yv = luma[sy * bw + sx] as f32 / 255.0;
                    let ci = (sy / 2) * chroma_stride + (sx / 2) * 2;
                    let mut cb = chroma[ci] as f32 / 255.0 - 128.0 / 255.0;
                    let mut cr = chroma[ci + 1] as f32 / 255.0 - 128.0 / 255.0;
                    if limited {
                        yv = (yv - 16.0 / 255.0) * (255.0 / 219.0);
                        cb *= 255.0 / 224.0;
                        cr *= 255.0 / 224.0;
                    }
                    let r = yv + r_cr * cr;
                    let b = yv + b_cb * cb;
                    let g = (yv - kr * r - kb * b) / kg;
                    let dst = row_off + x * 4;
                    out[dst] = encode_channel(r, transfer, lut);
                    out[dst + 1] = encode_channel(g, transfer, lut);
                    out[dst + 2] = encode_channel(b, transfer, lut);
                    out[dst + 3] = 255;
                }
            }
            Ok((uw_u32, uh_u32, out))
        }
        Format::Bgra => {
            let src = &frame.planes[0];
            if src.len() < bw.saturating_mul(bh).saturating_mul(4) {
                return Err("decoded BGRA buffer is truncated".into());
            }
            let mut out = vec![0u8; uw * uh * 4];
            for y in 0..uh {
                let row_off = y * uw * 4;
                for x in 0..uw {
                    let (sx, sy) = source_coord(orient, bw, bh, x, y);
                    let si = (sy * bw + sx) * 4;
                    let dst = row_off + x * 4;
                    match transfer {
                        Transfer::Srgb => {
                            out[dst] = src[si + 2];
                            out[dst + 1] = src[si + 1];
                            out[dst + 2] = src[si];
                            out[dst + 3] = 255;
                        }
                        Transfer::Bt709 => {
                            out[dst] = encode_channel(src[si + 2] as f32 / 255.0, transfer, lut);
                            out[dst + 1] = encode_channel(src[si + 1] as f32 / 255.0, transfer, lut);
                            out[dst + 2] = encode_channel(src[si] as f32 / 255.0, transfer, lut);
                            out[dst + 3] = 255;
                        }
                    }
                }
            }
            Ok((uw_u32, uh_u32, out))
        }
    }
}
