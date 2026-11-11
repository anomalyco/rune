//! The GPU's premultiplied source-over on a `Bgra8Unorm` target, byte for byte: the fragment's
//! colour converted to the target's 8-bit steps first (round to nearest, ties to even), then
//! `src + dst × (1 − src.a)`, rounded the same way. On x86-64 one pixel's four channels go through
//! SSE2 at once; elsewhere the same operations run a channel at a time, in the same order, so
//! both produce the same bits.

/// Adding 1.5 × 2²³ to a float in [0, 255] rounds it to an integer, ties to even, in the default
/// rounding mode, and leaves that integer in the low mantissa bits.
const MAGIC: f32 = 12_582_912.0;

/// A float in 0..=255 to the byte the GPU stores.
#[inline]
pub(super) fn q(v: f32) -> u8 {
    (v.clamp(0.0, 255.0) + MAGIC).to_bits() as u8
}

/// `v` (0..=1) in the target's 8-bit steps, as a float in 0..=255.
#[inline]
fn steps(v: f32) -> f32 {
    ((v * 255.0).clamp(0.0, 255.0) + MAGIC) - MAGIC
}

/// Premultiplied RGBA `c` (0..=1) as the blender takes it for a B, G, R, A pixel: in 8-bit steps,
/// with the destination's weight `1 − a`.
#[derive(Clone, Copy)]
pub(super) struct Src {
    bgra: [f32; 4],
    keep: f32,
}

impl Src {
    #[inline]
    pub(super) fn new(c: [f32; 4]) -> Src {
        let bgra = [steps(c[2]), steps(c[1]), steps(c[0]), steps(c[3])];
        Src { bgra, keep: 1.0 - bgra[3] / 255.0 }
    }

    /// The pixel it writes over anything when it's opaque.
    #[inline]
    pub(super) fn opaque(&self) -> Option<[u8; 4]> {
        (self.keep == 0.0).then(|| self.bgra.map(|v| v as u8))
    }
}

/// Blends `s` over B, G, R, A pixel `p`.
#[inline]
pub(super) fn over(p: &mut [u8], s: &Src) {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: SSE2 is part of x86-64; `p` holds at least four bytes (checked).
    unsafe {
        use std::arch::x86_64::*;
        assert!(p.len() >= 4);
        let zero = _mm_setzero_si128();
        let px = _mm_cvtsi32_si128(i32::from_ne_bytes([p[0], p[1], p[2], p[3]]));
        let d = _mm_cvtepi32_ps(_mm_unpacklo_epi16(_mm_unpacklo_epi8(px, zero), zero));
        let src = _mm_loadu_ps(s.bgra.as_ptr());
        let r = _mm_add_ps(src, _mm_mul_ps(d, _mm_set1_ps(s.keep)));
        let r = _mm_add_ps(_mm_min_ps(_mm_max_ps(r, _mm_setzero_ps()), _mm_set1_ps(255.0)), _mm_set1_ps(MAGIC));
        let bytes = _mm_and_si128(_mm_castps_si128(r), _mm_set1_epi32(0xff));
        let packed = _mm_packus_epi16(_mm_packs_epi32(bytes, zero), zero);
        p[..4].copy_from_slice(&_mm_cvtsi128_si32(packed).to_ne_bytes());
    }
    #[cfg(not(target_arch = "x86_64"))]
    for ch in 0..4 {
        p[ch] = q(s.bgra[ch] + p[ch] as f32 * s.keep);
    }
}

/// Blends premultiplied RGBA `c` (0..=1) over B, G, R, A pixel `p`: `over(p, &Src::new(c))` in
/// one go.
#[inline]
pub(super) fn over_rgba(p: &mut [u8], c: [f32; 4]) {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: SSE2 is part of x86-64; `p` holds at least four bytes (checked).
    unsafe {
        use std::arch::x86_64::*;
        assert!(p.len() >= 4);
        let zero = _mm_setzero_si128();
        let (lo, hi, magic) = (_mm_setzero_ps(), _mm_set1_ps(255.0), _mm_set1_ps(MAGIC));
        // R, G, B, A to B, G, R, A, then `steps`: × 255, clamped, rounded.
        let c = _mm_loadu_ps(c.as_ptr());
        let c = _mm_shuffle_ps::<0b11_00_01_10>(c, c);
        let s = _mm_sub_ps(_mm_add_ps(_mm_min_ps(_mm_max_ps(_mm_mul_ps(c, hi), lo), hi), magic), magic);
        let a = _mm_shuffle_ps::<0b11_11_11_11>(s, s);
        let keep = _mm_sub_ps(_mm_set1_ps(1.0), _mm_div_ps(a, hi));
        let px = _mm_cvtsi32_si128(i32::from_ne_bytes([p[0], p[1], p[2], p[3]]));
        let d = _mm_cvtepi32_ps(_mm_unpacklo_epi16(_mm_unpacklo_epi8(px, zero), zero));
        let r = _mm_add_ps(_mm_min_ps(_mm_max_ps(_mm_add_ps(s, _mm_mul_ps(d, keep)), lo), hi), magic);
        let bytes = _mm_and_si128(_mm_castps_si128(r), _mm_set1_epi32(0xff));
        let packed = _mm_packus_epi16(_mm_packs_epi32(bytes, zero), zero);
        p[..4].copy_from_slice(&_mm_cvtsi128_si32(packed).to_ne_bytes());
    }
    #[cfg(not(target_arch = "x86_64"))]
    over(p, &Src::new(c));
}

/// The B, G, R, A bytes an opaque premultiplied RGBA `c` (`c[3] == 1`) writes over any pixel.
#[inline]
pub(super) fn opaque(c: [f32; 4]) -> [u8; 4] {
    [q(c[2] * 255.0), q(c[1] * 255.0), q(c[0] * 255.0), q(c[3] * 255.0)]
}

/// A layer's row composited whole (inside its clip, no hole) over B, G, R, A pixels: each texel
/// by `opacity`, then `over_rgba`. An opaque texel at opacity 1 comes out as `opaque`, a clear
/// one leaves the pixel, so neither needs its own case.
#[inline]
pub(super) fn composite_row(row: &mut [u8], src: &[[f32; 4]]) {
    composite_row_by(row, src, 1.0)
}

/// `composite_row` for a layer of any opacity. On x86-64 four pixels go at once, a channel to a
/// register, through `over_rgba`'s operations in its order, so the bytes are the same.
#[inline]
pub(super) fn composite_row_by(row: &mut [u8], src: &[[f32; 4]], opacity: f32) {
    let n = (row.len() / 4).min(src.len());
    #[cfg(target_arch = "x86_64")]
    let done = {
        let quads = n / 4;
        // SAFETY: SSE2 is part of x86-64; `quads * 4` pixels and texels are in bounds.
        unsafe { over4(&mut row[..quads * 16], &src[..quads * 4], opacity) };
        quads * 4
    };
    #[cfg(not(target_arch = "x86_64"))]
    let done = 0;
    for (p, s) in row[done * 4..n * 4].chunks_exact_mut(4).zip(&src[done..n]) {
        over_rgba(p, s.map(|v| v * opacity));
    }
}

/// Four texels at a time over four B, G, R, A pixels: `over_rgba` on each, by lanes.
#[cfg(target_arch = "x86_64")]
unsafe fn over4(row: &mut [u8], src: &[[f32; 4]], opacity: f32) {
    use std::arch::x86_64::*;
    // SAFETY (whole body): SSE2 is baseline on x86-64; every load and store is inside `row`
    // (16 bytes a step) and `src` (four texels a step).
    unsafe {
        let (lo, hi, magic, one) = (_mm_setzero_ps(), _mm_set1_ps(255.0), _mm_set1_ps(MAGIC), _mm_set1_ps(1.0));
        let k = _mm_set1_ps(opacity);
        let byte = _mm_set1_epi32(0xff);
        for (p, s) in row.chunks_exact_mut(16).zip(src.chunks_exact(4)) {
            let mut t0 = _mm_mul_ps(_mm_loadu_ps(s[0].as_ptr()), k);
            let mut t1 = _mm_mul_ps(_mm_loadu_ps(s[1].as_ptr()), k);
            let mut t2 = _mm_mul_ps(_mm_loadu_ps(s[2].as_ptr()), k);
            let mut t3 = _mm_mul_ps(_mm_loadu_ps(s[3].as_ptr()), k);
            _MM_TRANSPOSE4_PS(&mut t0, &mut t1, &mut t2, &mut t3);
            // t0..t3: R, G, B, A of the four texels. `steps`: × 255, clamped, rounded.
            let steps = |v: __m128| _mm_sub_ps(_mm_add_ps(_mm_min_ps(_mm_max_ps(_mm_mul_ps(v, hi), lo), hi), magic), magic);
            let (r, g, b, a) = (steps(t0), steps(t1), steps(t2), steps(t3));
            let keep = _mm_sub_ps(one, _mm_div_ps(a, hi));
            let px = _mm_loadu_si128(p.as_ptr() as *const __m128i);
            let ch = |shift: i32| _mm_cvtepi32_ps(_mm_and_si128(_mm_srl_epi32(px, _mm_cvtsi32_si128(shift)), byte));
            let out = |s: __m128, d: __m128| _mm_and_si128(_mm_castps_si128(_mm_add_ps(_mm_min_ps(_mm_max_ps(_mm_add_ps(s, _mm_mul_ps(d, keep)), lo), hi), magic)), byte);
            let (ob, og, or, oa) = (out(b, ch(0)), out(g, ch(8)), out(r, ch(16)), out(a, ch(24)));
            let packed = _mm_or_si128(_mm_or_si128(ob, _mm_slli_epi32::<8>(og)), _mm_or_si128(_mm_slli_epi32::<16>(or), _mm_slli_epi32::<24>(oa)));
            _mm_storeu_si128(p.as_mut_ptr() as *mut __m128i, packed);
        }
    }
}
/// Blends `s` over every pixel of `row`.
#[inline]
pub(super) fn over_row(row: &mut [u8], s: &Src) {
    if let Some(px) = s.opaque() { return fill_row(row, px); }
    if s.bgra == [0.0; 4] { return; }
    for p in row.chunks_exact_mut(4) { over(p, s); }
}

/// The dual-source blend of subpixel text: each colour channel kept by its own `1 − k`, where
/// `k` (0..=1, red to blue) is the second output, in 8-bit steps like the first.
#[inline]
pub(super) fn over_lcd(p: &mut [u8], c: [f32; 4], k: [f32; 3]) {
    let s = [steps(c[2]), steps(c[1]), steps(c[0]), steps(c[3])];
    let keep = [1.0 - steps(k[2]) / 255.0, 1.0 - steps(k[1]) / 255.0, 1.0 - steps(k[0]) / 255.0, 1.0 - s[3] / 255.0];
    for ch in 0..4 { p[ch] = q(s[ch] + p[ch] as f32 * keep[ch]); }
}

/// Writes pixel `px` over every pixel of `row`, as words when the row is word aligned.
#[inline]
pub(super) fn fill_row(row: &mut [u8], px: [u8; 4]) {
    match bytemuck::try_cast_slice_mut::<u8, u32>(row) {
        Ok(words) => words.fill(u32::from_ne_bytes(px)),
        Err(_) => for p in row.chunks_exact_mut(4) { p.copy_from_slice(&px) },
    }
}
