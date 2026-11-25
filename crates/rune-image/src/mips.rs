//! Linear-light mip chains, built whole on a cache worker or a slice at a time where the
//! same thread also draws frames (the browser's main thread), so it can stop between slices.
use super::{Decoded, ImageError, Mip};

pub(super) fn mip_chain(width: u32, height: u32, bytes: &[u8]) -> Result<Decoded, ImageError> {
    mip_chain_with_intrinsic(width, height, bytes, None)
}

pub(super) fn mip_chain_with_intrinsic(
    width: u32,
    height: u32,
    bytes: &[u8],
    intrinsic: Option<[f32; 2]>,
) -> Result<Decoded, ImageError> {
    let mut steps = MipSteps::new(width, height, bytes)?;
    while !steps.step(usize::MAX) {}
    let mut decoded = steps.finish();
    decoded.intrinsic = intrinsic;
    Ok(decoded)
}

/// Target dimensions fitting `(w, h)` inside `max_px` square while preserving aspect ratio.
/// Never upscales and never rounds either side below 1.
pub fn scaled_dimensions(w: u32, h: u32, max_px: u32) -> (u32, u32) {
    let longest = w.max(h);
    if max_px == 0 || longest <= max_px {
        return (w, h);
    }
    if w >= h {
        let nh = ((f64::from(h) * f64::from(max_px)) / f64::from(w)).round().clamp(1.0, f64::from(max_px)) as u32;
        (max_px, nh)
    } else {
        let nw = ((f64::from(w) * f64::from(max_px)) / f64::from(h)).round().clamp(1.0, f64::from(max_px)) as u32;
        (nw, max_px)
    }
}

/// Validates straight-alpha RGBA8 `bytes` and resamples them in linear-premultiplied light to
/// `(nw, nh)`. Returns `bytes` unchanged when `(nw, nh) == (w, h)`.
pub fn downsample_rgba_to(
    w: u32,
    h: u32,
    bytes: Vec<u8>,
    nw: u32,
    nh: u32,
) -> Result<(u32, u32, Vec<u8>), ImageError> {
    if w == 0 || h == 0 || nw == 0 || nh == 0
        || w > super::MAX_SIDE || h > super::MAX_SIDE
        || nw > super::MAX_SIDE || nh > super::MAX_SIDE
        || bytes.len() != w as usize * h as usize * 4
    {
        return Err(ImageError("RGBA image dimensions or byte length are invalid".into()));
    }
    if (nw, nh) == (w, h) {
        return Ok((w, h, bytes));
    }
    Ok((nw, nh, downsample_rgba(w, h, &bytes, nw, nh)))
}

/// Validates straight-alpha RGBA8 `bytes` and, when `max_px` is `Some` and smaller than the
/// longest side, downsamples them in linear-premultiplied light to fit `max_px`.
pub(super) fn maybe_downsample_rgba(
    w: u32,
    h: u32,
    bytes: Vec<u8>,
    max_px: Option<u32>,
) -> Result<(u32, u32, Vec<u8>), ImageError> {
    let (nw, nh) = match max_px {
        Some(max_px) => scaled_dimensions(w, h, max_px),
        None => (w, h),
    };
    downsample_rgba_to(w, h, bytes, nw, nh)
}

/// Area-resamples straight-alpha sRGB RGBA8 `(w × h)` down to `(nw × nh)` in linear-premultiplied
/// light, returning straight-alpha sRGB RGBA8.
fn downsample_rgba(w: u32, h: u32, src: &[u8], nw: u32, nh: u32) -> Vec<u8> {
    let srgb = &*SRGB;
    let src = src.as_chunks::<4>().0;
    let mut out = Vec::with_capacity(nw as usize * nh as usize * 4);
    let mut row_sum = vec![[0.0f32; 4]; nw as usize];
    let wf = w as f32;
    let hf = h as f32;
    let nwf = nw as f32;
    let nhf = nh as f32;
    let w_usize = w as usize;
    for y in 0..nh {
        row_sum.fill([0.0; 4]);
        let y0 = (y as f32 * hf / nhf).min(hf);
        let y1 = ((y + 1) as f32 * hf / nhf).min(hf);
        let sy_start = y0.floor() as u32;
        let sy_end = (y1.ceil() as u32).min(h);
        for sy in sy_start..sy_end {
            let wy = (((sy + 1) as f32).min(y1) - (sy as f32).max(y0)).max(0.0);
            if wy == 0.0 { continue; }
            let src_row = &src[sy as usize * w_usize..(sy as usize + 1) * w_usize];
            for x in 0..nw {
                let x0 = (x as f32 * wf / nwf).min(wf);
                let x1 = ((x + 1) as f32 * wf / nwf).min(wf);
                let sx_start = x0.floor() as u32;
                let sx_end = (x1.ceil() as u32).min(w);
                let acc = &mut row_sum[x as usize];
                for sx in sx_start..sx_end {
                    let wx = (((sx + 1) as f32).min(x1) - (sx as f32).max(x0)).max(0.0);
                    let weight = wx * wy;
                    let p = &src_row[sx as usize];
                    let a = p[3] as f32 / 255.0;
                    let aw = a * weight;
                    acc[0] += srgb.decode[p[0] as usize] * aw;
                    acc[1] += srgb.decode[p[1] as usize] * aw;
                    acc[2] += srgb.decode[p[2] as usize] * aw;
                    acc[3] += aw;
                }
            }
        }
        let y_span = (y1 - y0).max(f32::EPSILON);
        for (x, sum) in row_sum.iter().enumerate() {
            let x0 = (x as f32 * wf / nwf).min(wf);
            let x1 = ((x + 1) as f32 * wf / nwf).min(wf);
            let area = (x1 - x0).max(f32::EPSILON) * y_span;
            let a = byte(sum[3] / area);
            if a == 0 || sum[3] <= 0.0 {
                out.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                let inv_a = 1.0 / sum[3];
                out.extend_from_slice(&[
                    srgb.encode(sum[0] * inv_a, 255),
                    srgb.encode(sum[1] * inv_a, 255),
                    srgb.encode(sum[2] * inv_a, 255),
                    a,
                ]);
            }
        }
    }
    out
}

/// A mip chain under construction: mip 0 at once, then the smaller levels a slice of rows
/// at a time.
pub(super) struct MipSteps<'a> {
    /// The straight-alpha source, which mip 1 filters.
    source: &'a [u8],
    mips: Vec<Mip>,
    /// The level being built, and the row it continues from.
    next: Vec<u8>,
    row: u32,
}

impl<'a> MipSteps<'a> {
    pub(super) fn new(width: u32, height: u32, bytes: &'a [u8]) -> Result<Self, ImageError> {
        if width == 0 || height == 0 || width > super::MAX_SIDE || height > super::MAX_SIDE
            || bytes.len() != width as usize * height as usize * 4
        { return Err(ImageError("RGBA image dimensions or byte length are invalid".into())); }
        let srgb = &*SRGB;
        // Mip 0 is the source with RGB premultiplied in linear light; an opaque pixel is unchanged.
        let mut top = bytes.to_vec();
        for p in top.as_chunks_mut::<4>().0 {
            if p[3] == 255 { continue; }
            let a = p[3] as f32 / 255.0;
            for c in 0..3 { p[c] = srgb.encode(srgb.decode[p[c] as usize] * a, p[3]); }
        }
        Ok(MipSteps { source: bytes, mips: vec![Mip { width, height, pixels: top }], next: Vec::new(), row: 0 })
    }

    /// Build at most about `budget` more pixels (whole rows, at least one); true once the
    /// chain reaches 1×1.
    pub(super) fn step(&mut self, mut budget: usize) -> bool {
        let srgb = &*SRGB;
        loop {
            let last = self.mips.last().expect("mip 0 is built first");
            let (w, h) = (last.width, last.height);
            if w == 1 && h == 1 { return true; }
            if budget == 0 { return false; }
            let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
            if self.row == 0 { self.next.reserve_exact(nw as usize * nh as usize * 4); }
            let rows = (budget / nw as usize).clamp(1, (nh - self.row) as usize) as u32;
            let range = self.row..self.row + rows;
            // Mip 1 filters the straight source, so a translucent edge is premultiplied once,
            // not quantized first. Each later mip filters the bytes of the one above it.
            if self.mips.len() == 1 {
                downsample_rows(w, h, self.source, range, &mut self.next, |p| {
                    let a = p[3] as f32 / 255.0;
                    [srgb.decode[p[0] as usize] * a, srgb.decode[p[1] as usize] * a, srgb.decode[p[2] as usize] * a, a]
                });
            } else {
                downsample_rows(w, h, &last.pixels, range, &mut self.next, |p| {
                    [srgb.decode[p[0] as usize], srgb.decode[p[1] as usize], srgb.decode[p[2] as usize], p[3] as f32 / 255.0]
                });
            }
            self.row += rows;
            budget = budget.saturating_sub(rows as usize * nw as usize);
            if self.row == nh {
                self.mips.push(Mip { width: nw, height: nh, pixels: std::mem::take(&mut self.next) });
                self.row = 0;
            }
        }
    }

    pub(super) fn finish(self) -> Decoded { Decoded { mips: self.mips, motion: None, intrinsic: None } }
}

/// Rows `rows` of `w`×`h` RGBA8 `src` halved by area, appended to `out`, summing `linear`
/// (premultiplied linear RGBA) of each source pixel. Area resampling includes the last
/// row/column of odd-sized sources.
fn downsample_rows(w: u32, h: u32, src: &[u8], rows: std::ops::Range<u32>, out: &mut Vec<u8>, linear: impl Fn(&[u8; 4]) -> [f32; 4]) {
    let src = src.as_chunks::<4>().0;
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut emit = |sum: [f32; 4], area: f32| {
        let [r, g, b, a] = sum.map(|v| v / area);
        let a = byte(a);
        out.extend_from_slice(&[SRGB.encode(r, a), SRGB.encode(g, a), SRGB.encode(b, a), a]);
    };
    if w % 2 == 0 && h % 2 == 0 {
        // Each target pixel is exactly a 2×2 block.
        let w = w as usize;
        for y in rows.start as usize..rows.end as usize { for x in 0..nw as usize {
            let i = 2 * y * w + 2 * x;
            let mut sum = [0.0; 4];
            for p in [i, i + 1, i + w, i + w + 1].map(|i| linear(&src[i])) {
                for c in 0..4 { sum[c] += p[c]; }
            }
            emit(sum, 4.0);
        }}
        return;
    }
    for y in rows { for x in 0..nw {
        let (x0, x1) = (x as f32 * w as f32 / nw as f32, (x + 1) as f32 * w as f32 / nw as f32);
        let (y0, y1) = (y as f32 * h as f32 / nh as f32, (y + 1) as f32 * h as f32 / nh as f32);
        let mut sum = [0.0; 4];
        for sy in y0.floor() as u32..(y1.ceil() as u32).min(h) {
            for sx in x0.floor() as u32..(x1.ceil() as u32).min(w) {
                let weight = ((sx + 1) as f32).min(x1) - (sx as f32).max(x0);
                let weight = weight * (((sy + 1) as f32).min(y1) - (sy as f32).max(y0));
                let p = linear(&src[(sy * w + sx) as usize]);
                for c in 0..4 { sum[c] += p[c] * weight; }
            }
        }
        emit(sum, (x1 - x0) * (y1 - y0));
    }}
}
/// Mip 0's linear-premultiplied bytes back to straight-alpha sRGB RGBA8, undoing
/// [`MipSteps::new`]: opaque pixels are unchanged, and a translucent one keeps the colour its
/// premultiplied bytes still hold.
pub(super) fn straight_rgba(pixels: &[u8]) -> Vec<u8> {
    let srgb = &*SRGB;
    let mut out = pixels.to_vec();
    for p in out.as_chunks_mut::<4>().0 {
        match p[3] {
            255 => {}
            0 => *p = [0; 4],
            alpha => {
                let a = alpha as f32 / 255.0;
                for c in 0..3 { p[c] = srgb.encode(srgb.decode[p[c] as usize] / a, 255); }
            }
        }
    }
    out
}

pub(super) fn byte(v: f32) -> u8 { (v.clamp(0.0, 1.0) * 255.0).round() as u8 }

/// sRGB transfer tables, so mip generation runs no `powf` per pixel.
struct Srgb {
    /// Linear light of each sRGB byte.
    decode: [f32; 256],
    /// `up[k]`: the linear value at and above which the sRGB encoding rounds past byte `k`.
    up: [f32; 256],
    /// The byte at the low edge of each 1/`STEPS` of linear light. The curve's steepest
    /// slope (12.92) crosses under one byte per step, so one `up` comparison finishes it.
    coarse: [u8; STEPS + 1],
}
const STEPS: usize = 4096;
static SRGB: std::sync::LazyLock<Srgb> = std::sync::LazyLock::new(|| {
    let mut up: [f32; 256] = std::array::from_fn(|k| rune_core::color::srgb_decode((k as f32 + 0.5) / 255.0));
    up[255] = f32::INFINITY;
    Srgb {
        decode: std::array::from_fn(|i| rune_core::color::srgb_decode(i as f32 / 255.0)),
        coarse: std::array::from_fn(|i| up.partition_point(|&t| t <= i as f32 / STEPS as f32) as u8),
        up,
    }
});
impl Srgb {
    /// The sRGB byte of premultiplied linear `v`, capped by its quantized alpha `a` so a pixel
    /// whose alpha rounded to zero emits no light.
    fn encode(&self, v: f32, a: u8) -> u8 {
        let v = v.clamp(0.0, a as f32 / 255.0);
        let b = self.coarse[(v * STEPS as f32) as usize];
        b + u8::from(v >= self.up[b as usize])
    }
}
