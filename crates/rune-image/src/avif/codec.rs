//! Direct native AVIF decoder built on `avif-parse`, `rav1d`, and `yuv`.
//!
//! Enforces the caller's [`ImageLimits`] in three layers:
//! 1. Before opening `rav1d`: rejects zero-pixel budgets (since `0` disables `frame_size_limit`).
//! 2. Inside `rav1d`: sets `Dav1dSettings::frame_size_limit` so oversized primary or alpha frames
//!    fail with `ERANGE` before `rav1d` allocates frame planes.
//! 3. After decoding each frame: validates dimensions and checked RGBA8 byte size before calling
//!    `Vec::try_reserve_exact`, converting straight into the final RGBA8 buffer with no intermediate
//!    full-frame RGB/RGBA16 allocation.
#![allow(unsafe_code)]

use std::borrow::Cow;
use std::ffi::c_int;
use std::mem::{MaybeUninit, align_of, size_of};
use std::ptr::{self, NonNull};

use rav1d::Dav1dResult;
use rav1d::include::dav1d::data::Dav1dData;
use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dLogger, Dav1dSettings};
use rav1d::include::dav1d::headers::{
    DAV1D_MC_BT470BG, DAV1D_MC_BT601, DAV1D_MC_BT709, DAV1D_MC_BT2020_CL, DAV1D_MC_BT2020_NCL,
    DAV1D_MC_FCC, DAV1D_MC_IDENTITY, DAV1D_MC_SMPTE_YCGCO, DAV1D_MC_SMPTE240, DAV1D_MC_UNKNOWN,
    DAV1D_PIXEL_LAYOUT_I400, DAV1D_PIXEL_LAYOUT_I420, DAV1D_PIXEL_LAYOUT_I422,
    DAV1D_PIXEL_LAYOUT_I444, Dav1dMatrixCoefficients, Dav1dPixelLayout, Dav1dSequenceHeader,
};
use rav1d::include::dav1d::picture::Dav1dPicture;
use rav1d::src::lib::{
    dav1d_close, dav1d_data_create, dav1d_data_unref, dav1d_default_settings, dav1d_get_picture,
    dav1d_open, dav1d_picture_unref, dav1d_send_data,
};
use yuv::{YuvPlanarImage, YuvRange, YuvStandardMatrix};

use super::limits::too_large;
use crate::{ImageError, ImageLimits};

fn invalid(msg: &str) -> ImageError { ImageError(format!("AVIF: {msg}")) }

/// Decodes an AVIF still to straight-alpha RGBA8 under `limits`.
pub(super) fn decode_avif(bytes: &[u8], limits: ImageLimits) -> Result<(u32, u32, Vec<u8>), ImageError> {
    let frame_size_limit = limits.rav1d_frame_size_limit().ok_or_else(too_large)?;
    let mut reader = bytes;
    let mut avif = avif_parse::read_avif(&mut reader).map_err(|e| ImageError(format!("AVIF: {e}")))?;

    let mut decoder = Rav1dDecoder::open(frame_size_limit)?;
    let primary_av1 = std::mem::take(&mut avif.primary_item);
    let primary = decoder.decode_still(&primary_av1)?;
    let color_mc = seq_hdr(primary).map(|h| h.mtrx);
    let (w, h, mut pixels) = picture_to_rgba(primary, limits)?;
    drop(primary_av1);

    if let Some(alpha_av1) = avif.alpha_item.take() {
        let alpha = decoder.decode_still(&alpha_av1)?;
        apply_alpha(alpha, w, h, &mut pixels, color_mc, avif.premultiplied_alpha)?;
    }
    Ok((w, h, pixels))
}

#[cfg(test)]
pub(super) fn decode_av1_frame_with_limit(av1_bytes: &[u8], limits: ImageLimits) -> Result<(u32, u32, Vec<u8>), ImageError> {
    let frame_size_limit = limits.rav1d_frame_size_limit().ok_or_else(too_large)?;
    let mut decoder = Rav1dDecoder::open(frame_size_limit)?;
    let pic = decoder.decode_still(av1_bytes)?;
    picture_to_rgba(pic, limits)
}

struct Rav1dDecoder {
    ctx: Option<Dav1dContext>,
    picture: Option<Dav1dPicture>,
}

impl Rav1dDecoder {
    fn open(frame_size_limit: u32) -> Result<Self, ImageError> {
        if frame_size_limit == 0 {
            return Err(too_large());
        }
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(32);
        let mut settings = MaybeUninit::<Dav1dSettings>::uninit();
        // SAFETY: `settings` points to valid uninitialized storage for `Dav1dSettings`, which
        // `dav1d_default_settings` completely initializes.
        let settings = unsafe {
            dav1d_default_settings(NonNull::from(&mut settings).cast());
            settings.assume_init_mut()
        };
        settings.n_threads = threads.min(256) as c_int;
        settings.max_frame_delay = 1;
        settings.apply_grain = 1;
        settings.frame_size_limit = frame_size_limit;
        // SAFETY: A `None` callback disables logging without calling any callback function.
        settings.logger = unsafe { Dav1dLogger::new(None, None) };

        let mut ctx = None;
        // SAFETY: Both pointers are valid, non-null, and live for the duration of `dav1d_open`.
        let res = unsafe { dav1d_open(Some(NonNull::from(&mut ctx)), Some(NonNull::from(settings))) };
        check_rav1d(res)?;
        if ctx.is_none() {
            return Err(invalid("rav1d failed to initialize decoder context"));
        }
        Ok(Self { ctx, picture: None })
    }

    /// Decodes a single AVIF item (one AV1 temporal unit) and keeps its final picture alive until
    /// the next decode call or drop.
    fn decode_still(&mut self, av1_data: &[u8]) -> Result<&Dav1dPicture, ImageError> {
        if av1_data.is_empty() {
            return Err(invalid("empty AV1 bitstream"));
        }
        if let Some(old) = &mut self.picture {
            // SAFETY: `old` was returned by `dav1d_get_picture` and has not yet been unref'd.
            unsafe { dav1d_picture_unref(Some(NonNull::from(old))) };
            self.picture = None;
        }
        let ctx = self.ctx.ok_or_else(|| invalid("decoder closed"))?;
        let pic = decode_temporal_unit(ctx, av1_data)?;
        Ok(self.picture.insert(pic))
    }
}

impl Drop for Rav1dDecoder {
    fn drop(&mut self) {
        if let Some(pic) = &mut self.picture {
            // SAFETY: `pic` was produced by `dav1d_get_picture` and is unref'd once here.
            unsafe { dav1d_picture_unref(Some(NonNull::from(pic))) };
        }
        // SAFETY: `self.ctx` came from `dav1d_open` (or is `None`) and is closed once here.
        unsafe { dav1d_close(Some(NonNull::from(&mut self.ctx))) };
    }
}

/// Feeds one AV1 temporal unit to `ctx` and returns the final decoded layer picture.
fn decode_temporal_unit(ctx: Dav1dContext, av1_data: &[u8]) -> Result<Dav1dPicture, ImageError> {
    let mut data = Dav1dData::default();
    let mut last_pic: Option<Dav1dPicture> = None;

    let send_res = (|| -> Result<(), ImageError> {
        // SAFETY: `data` is a valid default-initialized `Dav1dData`; `dav1d_data_create`
        // allocates `av1_data.len()` writable bytes and returns null on failure.
        let buf = unsafe { dav1d_data_create(Some(NonNull::from(&mut data)), av1_data.len()) };
        let Some(buf) = NonNull::new(buf) else {
            return Err(too_large());
        };
        // SAFETY: `buf` points to a newly allocated, non-overlapping buffer of `av1_data.len()`
        // bytes owned by `data`.
        unsafe { ptr::copy_nonoverlapping(av1_data.as_ptr(), buf.as_ptr(), av1_data.len()) };

        loop {
            // SAFETY: `ctx` is an open `Dav1dContext` and `data` is valid.
            let res = unsafe { dav1d_send_data(Some(ctx), Some(NonNull::from(&mut data))) };
            if res.0 == 0 {
                return Ok(());
            }
            let mut pending = Dav1dPicture::default();
            // SAFETY: `ctx` is open and `pending` is valid default-initialized storage.
            let drain = unsafe { dav1d_get_picture(Some(ctx), Some(NonNull::from(&mut pending))) };
            if drain.0 != 0 {
                // SAFETY: `pending` is valid to unref even after a non-zero `dav1d_get_picture`.
                unsafe { dav1d_picture_unref(Some(NonNull::from(&mut pending))) };
                return check_rav1d(res);
            }
            if let Some(mut prev) = last_pic.replace(pending) {
                // SAFETY: `prev` was returned by `dav1d_get_picture` and is being discarded.
                unsafe { dav1d_picture_unref(Some(NonNull::from(&mut prev))) };
            }
        }
    })();

    // SAFETY: `dav1d_data_unref` safely releases any buffer still owned by `data`.
    unsafe { dav1d_data_unref(Some(NonNull::from(&mut data))) };
    if let Err(err) = send_res {
        if let Some(mut prev) = last_pic {
            // SAFETY: `prev` was returned by `dav1d_get_picture` and must be freed on error.
            unsafe { dav1d_picture_unref(Some(NonNull::from(&mut prev))) };
        }
        return Err(err);
    }

    loop {
        let mut pic = Dav1dPicture::default();
        // SAFETY: `ctx` is open and `pic` is valid default-initialized storage.
        let res = unsafe { dav1d_get_picture(Some(ctx), Some(NonNull::from(&mut pic))) };
        if res.0 != 0 {
            // SAFETY: `pic` is valid to unref after a non-zero `dav1d_get_picture`.
            unsafe { dav1d_picture_unref(Some(NonNull::from(&mut pic))) };
            if let Some(pic) = last_pic {
                return Ok(pic);
            }
            if res.0 == -(libc::EAGAIN as c_int) {
                return Err(invalid("no frame decoded"));
            }
            check_rav1d(res)?;
            return Err(invalid("no frame decoded"));
        }
        if let Some(mut prev) = last_pic.replace(pic) {
            // SAFETY: `prev` was returned by `dav1d_get_picture` and is superseded by `pic`.
            unsafe { dav1d_picture_unref(Some(NonNull::from(&mut prev))) };
        }
    }
}

fn check_rav1d(res: Dav1dResult) -> Result<(), ImageError> {
    if res.0 == 0 {
        Ok(())
    } else if res.0 == -(libc::ERANGE as c_int) {
        Err(too_large())
    } else {
        Err(ImageError(format!("AVIF: rav1d error ({})", res.0.unsigned_abs())))
    }
}

fn seq_hdr(pic: &Dav1dPicture) -> Option<&Dav1dSequenceHeader> {
    let ptr = pic.seq_hdr?;
    // SAFETY: `pic.seq_hdr` is kept alive by `pic.seq_hdr_ref` for the lifetime of `pic`.
    Some(unsafe { ptr.as_ref() })
}

fn picture_range(pic: &Dav1dPicture) -> YuvRange {
    if seq_hdr(pic).is_some_and(|h| h.color_range != 0) {
        YuvRange::Full
    } else {
        YuvRange::Limited
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Chroma {
    Mono,
    Cs420,
    Cs422,
    Cs444,
}

fn picture_chroma(layout: Dav1dPixelLayout) -> Result<Chroma, ImageError> {
    match layout {
        DAV1D_PIXEL_LAYOUT_I400 => Ok(Chroma::Mono),
        DAV1D_PIXEL_LAYOUT_I420 => Ok(Chroma::Cs420),
        DAV1D_PIXEL_LAYOUT_I422 => Ok(Chroma::Cs422),
        DAV1D_PIXEL_LAYOUT_I444 => Ok(Chroma::Cs444),
        _ => Err(invalid("unsupported pixel layout")),
    }
}

#[derive(Clone, Copy)]
enum Matrix {
    Identity,
    YCgCo,
    Standard(YuvStandardMatrix),
}

fn picture_matrix(pic: &Dav1dPicture) -> Result<Matrix, ImageError> {
    let mc = seq_hdr(pic).map_or(DAV1D_MC_UNKNOWN, |h| h.mtrx);
    match mc {
        DAV1D_MC_IDENTITY => Ok(Matrix::Identity),
        DAV1D_MC_SMPTE_YCGCO => Ok(Matrix::YCgCo),
        DAV1D_MC_BT709 => Ok(Matrix::Standard(YuvStandardMatrix::Bt709)),
        DAV1D_MC_FCC => Ok(Matrix::Standard(YuvStandardMatrix::Fcc)),
        DAV1D_MC_BT470BG | DAV1D_MC_BT601 | DAV1D_MC_UNKNOWN => Ok(Matrix::Standard(YuvStandardMatrix::Bt601)),
        DAV1D_MC_SMPTE240 => Ok(Matrix::Standard(YuvStandardMatrix::Smpte240)),
        DAV1D_MC_BT2020_NCL | DAV1D_MC_BT2020_CL => Ok(Matrix::Standard(YuvStandardMatrix::Bt2020)),
        _ => Err(invalid("unsupported matrix coefficients")),
    }
}

fn plane_dims(w: usize, h: usize, plane: usize, chroma: Chroma) -> (usize, usize) {
    let (sx, sy) = match (plane != 0, chroma) {
        (false, _) | (true, Chroma::Cs444) => (0usize, 0usize),
        (true, Chroma::Cs420) => (1, 1),
        (true, Chroma::Cs422) => (1, 0),
        (true, Chroma::Mono) => (0, 0),
    };
    ((w + sx) >> sx, (h + sy) >> sy)
}

fn plane_u8(pic: &Dav1dPicture, plane: usize, w: usize, h: usize, chroma: Chroma) -> Result<(&[u8], usize), ImageError> {
    let ptr = pic.data[plane].ok_or_else(|| invalid("missing picture plane"))?;
    let (pw, ph) = plane_dims(w, h, plane, chroma);
    let stride_bytes = pic.stride[usize::from(plane != 0)];
    if stride_bytes <= 0 {
        return Err(invalid("invalid plane stride"));
    }
    let stride = stride_bytes as usize;
    if stride < pw || pw == 0 || ph == 0 {
        return Err(invalid("invalid plane dimensions"));
    }
    let len = stride.checked_mul(ph).ok_or_else(too_large)?;
    // SAFETY: `rav1d` guarantees each allocated plane holds at least `stride * ph` readable bytes
    // for the lifetime of `pic`.
    let slice = unsafe { std::slice::from_raw_parts(ptr.as_ptr().cast::<u8>(), len) };
    Ok((slice, stride))
}

fn plane_u16(pic: &Dav1dPicture, plane: usize, w: usize, h: usize, chroma: Chroma) -> Result<(Cow<'_, [u16]>, usize), ImageError> {
    let ptr = pic.data[plane].ok_or_else(|| invalid("missing picture plane"))?;
    let (pw, ph) = plane_dims(w, h, plane, chroma);
    let stride_bytes = pic.stride[usize::from(plane != 0)];
    if stride_bytes <= 0 || !(stride_bytes as usize).is_multiple_of(size_of::<u16>()) {
        return Err(invalid("invalid plane stride"));
    }
    let stride = stride_bytes as usize / size_of::<u16>();
    if stride < pw || pw == 0 || ph == 0 {
        return Err(invalid("invalid plane dimensions"));
    }
    let samples = stride.checked_mul(ph).ok_or_else(too_large)?;
    let byte_len = samples.checked_mul(size_of::<u16>()).ok_or_else(too_large)?;
    let raw_ptr = ptr.as_ptr().cast::<u8>();
    if (raw_ptr as usize).is_multiple_of(align_of::<u16>()) {
        // SAFETY: `raw_ptr` is 2-byte aligned and `rav1d` allocates at least `stride_bytes * ph`
        // readable bytes for the lifetime of `pic`.
        let slice = unsafe { std::slice::from_raw_parts(raw_ptr.cast::<u16>(), samples) };
        Ok((Cow::Borrowed(slice), stride))
    } else {
        // SAFETY: `rav1d` allocates at least `byte_len` readable bytes for the lifetime of `pic`.
        let bytes = unsafe { std::slice::from_raw_parts(raw_ptr, byte_len) };
        let mut owned = Vec::new();
        owned.try_reserve_exact(samples).map_err(|_| too_large())?;
        for pair in bytes.as_chunks::<2>().0 {
            owned.push(u16::from_ne_bytes(*pair));
        }
        Ok((Cow::Owned(owned), stride))
    }
}

fn picture_to_rgba(pic: &Dav1dPicture, limits: ImageLimits) -> Result<(u32, u32, Vec<u8>), ImageError> {
    let (Ok(w), Ok(h)) = (usize::try_from(pic.p.w), usize::try_from(pic.p.h)) else {
        return Err(too_large());
    };
    let (w32, h32, byte_len) = super::checked_rgba_bytes_with(w, h, limits)?;
    let chroma = picture_chroma(pic.p.layout)?;
    let bpc = match pic.p.bpc {
        8 | 10 | 12 | 16 => pic.p.bpc as u32,
        _ => return Err(invalid("unsupported bit depth")),
    };
    let range = picture_range(pic);

    let mut pixels = Vec::new();
    pixels.try_reserve_exact(byte_len).map_err(|_| too_large())?;
    pixels.resize(byte_len, 0);

    match (chroma, bpc) {
        (Chroma::Mono, 8) => {
            let (y, stride) = plane_u8(pic, 0, w, h, chroma)?;
            write_mono8(y, stride, w, h, range, &mut pixels);
        }
        (Chroma::Mono, _) => {
            let (y, stride) = plane_u16(pic, 0, w, h, chroma)?;
            write_mono16(&y, stride, w, h, bpc, range, &mut pixels);
        }
        (_, 8) => {
            let matrix = picture_matrix(pic)?;
            convert_yuv8(pic, w32, h32, chroma, matrix, range, &mut pixels)?;
        }
        (_, _) => {
            let matrix = picture_matrix(pic)?;
            convert_yuv16(pic, w32, h32, bpc, chroma, matrix, range, &mut pixels)?;
        }
    }
    Ok((w32, h32, pixels))
}

fn convert_yuv8(
    pic: &Dav1dPicture,
    w: u32,
    h: u32,
    chroma: Chroma,
    matrix: Matrix,
    range: YuvRange,
    pixels: &mut [u8],
) -> Result<(), ImageError> {
    let (y, ys) = plane_u8(pic, 0, w as usize, h as usize, chroma)?;
    let (u, us) = plane_u8(pic, 1, w as usize, h as usize, chroma)?;
    let (v, vs) = plane_u8(pic, 2, w as usize, h as usize, chroma)?;
    let planar = YuvPlanarImage {
        y_plane: y,
        y_stride: u32::try_from(ys).map_err(|_| too_large())?,
        u_plane: u,
        u_stride: u32::try_from(us).map_err(|_| too_large())?,
        v_plane: v,
        v_stride: u32::try_from(vs).map_err(|_| too_large())?,
        width: w,
        height: h,
    };
    let dst_stride = w.checked_mul(4).ok_or_else(too_large)?;
    let res = match (matrix, chroma) {
        (Matrix::Identity, Chroma::Cs444) => yuv::gbr_to_rgba(&planar, pixels, dst_stride, range),
        (Matrix::Identity, _) => return Err(invalid("identity matrix requires 4:4:4 sampling")),
        (Matrix::YCgCo, Chroma::Cs444) => yuv::ycgco444_to_rgba(&planar, pixels, dst_stride, range),
        (Matrix::YCgCo, Chroma::Cs422) => yuv::ycgco422_to_rgba(&planar, pixels, dst_stride, range),
        (Matrix::YCgCo, Chroma::Cs420) => yuv::ycgco420_to_rgba(&planar, pixels, dst_stride, range),
        (Matrix::Standard(m), Chroma::Cs444) => yuv::yuv444_to_rgba(&planar, pixels, dst_stride, range, m),
        (Matrix::Standard(m), Chroma::Cs422) => yuv::yuv422_to_rgba(&planar, pixels, dst_stride, range, m),
        (Matrix::Standard(m), Chroma::Cs420) => yuv::yuv420_to_rgba(&planar, pixels, dst_stride, range, m),
        (_, Chroma::Mono) => return Err(invalid("monochrome in color conversion")),
    };
    res.map_err(|e| ImageError(format!("AVIF: {e}")))
}

#[allow(clippy::too_many_arguments)]
fn convert_yuv16(
    pic: &Dav1dPicture,
    w: u32,
    h: u32,
    bpc: u32,
    chroma: Chroma,
    matrix: Matrix,
    range: YuvRange,
    pixels: &mut [u8],
) -> Result<(), ImageError> {
    let (y, ys) = plane_u16(pic, 0, w as usize, h as usize, chroma)?;
    let (u, us) = plane_u16(pic, 1, w as usize, h as usize, chroma)?;
    let (v, vs) = plane_u16(pic, 2, w as usize, h as usize, chroma)?;

    if matches!(matrix, Matrix::Identity) {
        if chroma != Chroma::Cs444 {
            return Err(invalid("identity matrix requires 4:4:4 sampling"));
        }
        write_gbr16(&y, ys, &u, us, &v, vs, w as usize, h as usize, bpc, range, pixels);
        return Ok(());
    }

    if matches!(matrix, Matrix::YCgCo) {
        return convert_ycgco16(&y, ys, &u, us, &v, vs, w, h, bpc, chroma, range, pixels);
    }

    let Matrix::Standard(m) = matrix else { unreachable!() };
    let planar = YuvPlanarImage {
        y_plane: &y,
        y_stride: u32::try_from(ys).map_err(|_| too_large())?,
        u_plane: &u,
        u_stride: u32::try_from(us).map_err(|_| too_large())?,
        v_plane: &v,
        v_stride: u32::try_from(vs).map_err(|_| too_large())?,
        width: w,
        height: h,
    };
    let dst_stride = w.checked_mul(4).ok_or_else(too_large)?;
    let res = match (bpc, chroma) {
        (10, Chroma::Cs444) => yuv::i410_to_rgba(&planar, pixels, dst_stride, range, m),
        (10, Chroma::Cs422) => yuv::i210_to_rgba(&planar, pixels, dst_stride, range, m),
        (10, Chroma::Cs420) => yuv::i010_to_rgba(&planar, pixels, dst_stride, range, m),
        (12, Chroma::Cs444) => yuv::i412_to_rgba(&planar, pixels, dst_stride, range, m),
        (12, Chroma::Cs422) => yuv::i212_to_rgba(&planar, pixels, dst_stride, range, m),
        (12, Chroma::Cs420) => yuv::i012_to_rgba(&planar, pixels, dst_stride, range, m),
        _ => return Err(invalid("unsupported bit depth for YUV conversion")),
    };
    res.map_err(|e| ImageError(format!("AVIF: {e}")))
}

#[allow(clippy::too_many_arguments)]
fn convert_ycgco16(
    y: &[u16],
    ys: usize,
    u: &[u16],
    us: usize,
    v: &[u16],
    vs: usize,
    w: u32,
    h: u32,
    bpc: u32,
    chroma: Chroma,
    range: YuvRange,
    pixels: &mut [u8],
) -> Result<(), ImageError> {
    if bpc != 10 && bpc != 12 {
        return Err(invalid("unsupported YCgCo bit depth"));
    }
    let w_usize = w as usize;
    let h_usize = h as usize;
    let strip_rows = if chroma == Chroma::Cs420 { 2usize } else { 1usize };
    let strip_elems = strip_rows.checked_mul(w_usize).and_then(|n| n.checked_mul(4)).ok_or_else(too_large)?;
    let mut strip = Vec::new();
    strip.try_reserve_exact(strip_elems).map_err(|_| too_large())?;
    strip.resize(strip_elems, 0u16);
    let dst_stride = w.checked_mul(4).ok_or_else(too_large)?;
    let ys32 = u32::try_from(ys).map_err(|_| too_large())?;
    let us32 = u32::try_from(us).map_err(|_| too_large())?;
    let vs32 = u32::try_from(vs).map_err(|_| too_large())?;
    let max_val = (1u32 << bpc) - 1;

    let mut row = 0usize;
    while row < h_usize {
        let rows_here = (h_usize - row).min(strip_rows);
        let c_row = if chroma == Chroma::Cs420 { row / 2 } else { row };
        let c_rows = if chroma == Chroma::Cs420 { 1usize } else { rows_here };
        let planar = YuvPlanarImage {
            y_plane: &y[row * ys..(row + rows_here) * ys],
            y_stride: ys32,
            u_plane: &u[c_row * us..(c_row + c_rows) * us],
            u_stride: us32,
            v_plane: &v[c_row * vs..(c_row + c_rows) * vs],
            v_stride: vs32,
            width: w,
            height: rows_here as u32,
        };
        let out_u16 = &mut strip[..rows_here * w_usize * 4];
        let res = match (bpc, chroma) {
            (10, Chroma::Cs444) => yuv::icgc410_to_rgba10(&planar, out_u16, dst_stride, range),
            (10, Chroma::Cs422) => yuv::icgc210_to_rgba10(&planar, out_u16, dst_stride, range),
            (10, Chroma::Cs420) => yuv::icgc010_to_rgba10(&planar, out_u16, dst_stride, range),
            (12, Chroma::Cs444) => yuv::icgc412_to_rgba12(&planar, out_u16, dst_stride, range),
            (12, Chroma::Cs422) => yuv::icgc212_to_rgba12(&planar, out_u16, dst_stride, range),
            (12, Chroma::Cs420) => yuv::icgc012_to_rgba12(&planar, out_u16, dst_stride, range),
            _ => unreachable!(),
        };
        res.map_err(|e| ImageError(format!("AVIF: {e}")))?;

        let dst_row = &mut pixels[row * w_usize * 4..(row + rows_here) * w_usize * 4];
        for (src, dst) in out_u16.as_chunks::<4>().0.iter().zip(dst_row.as_chunks_mut::<4>().0) {
            dst[0] = ((u32::from(src[0]).min(max_val) * 255 + max_val / 2) / max_val) as u8;
            dst[1] = ((u32::from(src[1]).min(max_val) * 255 + max_val / 2) / max_val) as u8;
            dst[2] = ((u32::from(src[2]).min(max_val) * 255 + max_val / 2) / max_val) as u8;
            dst[3] = 255;
        }
        row += rows_here;
    }
    Ok(())
}

fn write_mono8(y: &[u8], stride: usize, w: usize, h: usize, range: YuvRange, pixels: &mut [u8]) {
    for row in 0..h {
        let src = &y[row * stride..row * stride + w];
        let dst = &mut pixels[row * w * 4..(row + 1) * w * 4];
        for (&v, px) in src.iter().zip(dst.as_chunks_mut::<4>().0) {
            let g = sample_to_u8(u16::from(v), 8, range);
            *px = [g, g, g, 255];
        }
    }
}

fn write_mono16(y: &[u16], stride: usize, w: usize, h: usize, bpc: u32, range: YuvRange, pixels: &mut [u8]) {
    for row in 0..h {
        let src = &y[row * stride..row * stride + w];
        let dst = &mut pixels[row * w * 4..(row + 1) * w * 4];
        for (&v, px) in src.iter().zip(dst.as_chunks_mut::<4>().0) {
            let g = sample_to_u8(v, bpc, range);
            *px = [g, g, g, 255];
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn write_gbr16(
    g_plane: &[u16],
    gs: usize,
    b_plane: &[u16],
    bs: usize,
    r_plane: &[u16],
    rs: usize,
    w: usize,
    h: usize,
    bpc: u32,
    range: YuvRange,
    pixels: &mut [u8],
) {
    for row in 0..h {
        let g_row = &g_plane[row * gs..row * gs + w];
        let b_row = &b_plane[row * bs..row * bs + w];
        let r_row = &r_plane[row * rs..row * rs + w];
        let dst = &mut pixels[row * w * 4..(row + 1) * w * 4];
        for x in 0..w {
            let off = x * 4;
            dst[off] = sample_to_u8(r_row[x], bpc, range);
            dst[off + 1] = sample_to_u8(g_row[x], bpc, range);
            dst[off + 2] = sample_to_u8(b_row[x], bpc, range);
            dst[off + 3] = 255;
        }
    }
}

fn apply_alpha(
    alpha: &Dav1dPicture,
    w: u32,
    h: u32,
    pixels: &mut [u8],
    color_mc: Option<Dav1dMatrixCoefficients>,
    premultiplied: bool,
) -> Result<(), ImageError> {
    if alpha.p.w != w as c_int || alpha.p.h != h as c_int {
        return Err(invalid("alpha dimensions do not match primary image"));
    }
    if let Some(alpha_mc) = seq_hdr(alpha).map(|s| s.mtrx)
        && alpha_mc != DAV1D_MC_IDENTITY
        && alpha_mc != DAV1D_MC_UNKNOWN
        && color_mc.unwrap_or(DAV1D_MC_IDENTITY) != alpha_mc
    {
        return Err(invalid("alpha image has conflicting color matrix"));
    }
    let chroma = picture_chroma(alpha.p.layout)?;
    let bpc = match alpha.p.bpc {
        8 | 10 | 12 | 16 => alpha.p.bpc as u32,
        _ => return Err(invalid("unsupported alpha bit depth")),
    };
    let range = picture_range(alpha);
    let (w_usize, h_usize) = (w as usize, h as usize);
    if bpc == 8 {
        let (y, stride) = plane_u8(alpha, 0, w_usize, h_usize, chroma)?;
        for row in 0..h_usize {
            let src = &y[row * stride..row * stride + w_usize];
            let dst = &mut pixels[row * w_usize * 4..(row + 1) * w_usize * 4];
            for (&a, px) in src.iter().zip(dst.as_chunks_mut::<4>().0) {
                px[3] = sample_to_u8(u16::from(a), 8, range);
            }
            if premultiplied {
                unpremultiply_row(dst);
            }
        }
    } else {
        let (y, stride) = plane_u16(alpha, 0, w_usize, h_usize, chroma)?;
        for row in 0..h_usize {
            let src = &y[row * stride..row * stride + w_usize];
            let dst = &mut pixels[row * w_usize * 4..(row + 1) * w_usize * 4];
            for (&a, px) in src.iter().zip(dst.as_chunks_mut::<4>().0) {
                px[3] = sample_to_u8(a, bpc, range);
            }
            if premultiplied {
                unpremultiply_row(dst);
            }
        }
    }
    Ok(())
}

#[inline]
fn sample_to_u8(v: u16, bpc: u32, range: YuvRange) -> u8 {
    let (min, span) = match range {
        YuvRange::Full => (0u32, (1u32 << bpc) - 1),
        YuvRange::Limited => (16u32 << (bpc - 8), 219u32 << (bpc - 8)),
    };
    let clamped = u32::from(v).saturating_sub(min).min(span);
    ((clamped * 255 + span / 2) / span) as u8
}

#[cold]
fn unpremultiply_row(row: &mut [u8]) {
    for px in row.as_chunks_mut::<4>().0 {
        let a = px[3];
        if a != 0 && a != 255 {
            let a16 = u16::from(a);
            px[0] = (u16::from(px[0]) * 255 / a16).min(255) as u8;
            px[1] = (u16::from(px[1]) * 255 / a16).min(255) as u8;
            px[2] = (u16::from(px[2]) * 255 / a16).min(255) as u8;
        }
    }
}
