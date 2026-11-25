//! A decoded image's pixels read back as straight RGBA, for a copy to the clipboard or a save.
use std::borrow::Cow;
use super::{Decoded, Image};
use super::mips::{downsample_rgba_to, straight_rgba};

/// Borrowed or owned pixel data ready for off-thread PNG encoding without running
/// linear-premultiplied un-premultiplication on the UI thread.
#[doc(hidden)]
pub enum CopyPixels<'a> {
    /// Straight-alpha sRGB RGBA8 (`width * height * 4` bytes, rows top first).
    Straight { width: u32, height: u32, bytes: Cow<'a, [u8]> },
    /// Mip-0 linear-premultiplied sRGB RGBA8 (`width * height * 4` bytes, rows top first)
    /// requiring `straight_rgba` un-premultiplication off the UI thread.
    PremulMip0 { width: u32, height: u32, bytes: &'a [u8] },
}

pub(crate) fn with_decoded_copy_pixels<R>(decoded: &Decoded, f: impl FnOnce(CopyPixels<'_>) -> R) -> Option<R> {
    let mip = decoded.mips.first()?;
    let full = (mip.width, mip.height);
    if let Some([iw, ih]) = decoded.intrinsic {
        let side = |v: f32| if v.is_finite() { v.round().max(1.0) as u32 } else { u32::MAX };
        let (nw, nh) = (side(iw), side(ih));
        if nw <= full.0 && nh <= full.1 && (nw, nh) != full {
            let (w, h, rgba) = downsample_rgba_to(full.0, full.1, straight_rgba(&mip.pixels), nw, nh).ok()?;
            return Some(f(CopyPixels::Straight { width: w, height: h, bytes: Cow::Owned(rgba) }));
        }
    }
    Some(f(CopyPixels::PremulMip0 { width: full.0, height: full.1, bytes: &mip.pixels }))
}

/// Converts a decoded image's top mip (linear-premultiplied sRGB RGBA8) back to straight-alpha
/// sRGB RGBA8, downsampling a 2x-rasterized SVG to its rounded intrinsic size when smaller.
pub(crate) fn decoded_to_rgba(decoded: &Decoded) -> Option<(u32, u32, Vec<u8>)> {
    let mip = decoded.mips.first()?;
    let rgba = straight_rgba(&mip.pixels);
    let full = (mip.width, mip.height);
    let Some([iw, ih]) = decoded.intrinsic else { return Some((full.0, full.1, rgba)) };
    let side = |v: f32| if v.is_finite() { v.round().max(1.0) as u32 } else { u32::MAX };
    let (nw, nh) = (side(iw), side(ih));
    if nw > full.0 || nh > full.1 { return Some((full.0, full.1, rgba)); }
    downsample_rgba_to(full.0, full.1, rgba, nw, nh).ok()
}

impl Image {
    /// The decoded image as straight-alpha sRGB RGBA8 (`width * height * 4` bytes, rows top
    /// first), at full resolution: a raster at its decoded size (after EXIF orientation and ICC
    /// conversion), an animated GIF/WebP/APNG's frame 0 (not the currently displayed frame),
    /// a native AVIF still (animated `avis` is rejected natively; first frame on web), and an
    /// SVG loaded via [`ImageCache::load`](crate::ImageCache::load) at its rounded intrinsic
    /// size ([`Image::intrinsic_size`], capped at 4096 px/side) when rasterized larger. An SVG
    /// rasterized smaller via [`ImageCache::svg`](crate::ImageCache::svg) or an image from
    /// [`ImageCache::load_scaled`](crate::ImageCache::load_scaled) returns its rasterized/
    /// scaled size. `None` while decoding or when decoding failed.
    ///
    /// Opaque 8-bit sRGB pixels (`alpha == 255`) round-trip bit-exactly (`0` error);
    /// translucent pixels (`1..=254`) round-trip through premultiplied 8-bit sRGB mip 0
    /// (worst channel error ±1 for `alpha >= 128`, ±4 for `alpha >= 32`, up to 55 at
    /// `alpha == 1`), and fully transparent pixels (`alpha == 0`) zero RGB. Wide-gamut ICC
    /// profiles are converted to sRGB and >8-bit HDR sources are reduced to 8-bit sRGB.
    ///
    /// It copies and converts every pixel, so call it off the event thread for a large image
    /// (`Tx::copy_image` in `rune` does).
    ///
    /// ```
    /// use std::sync::Arc;
    /// use rune_image::ImageCache;
    ///
    /// let cache = ImageCache::new(Arc::new(|| {}));
    /// let image = cache.rgba(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]);
    /// image.wait().unwrap();
    /// assert_eq!(image.to_rgba(), Some((2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255])));
    /// ```
    pub fn to_rgba(&self) -> Option<(u32, u32, Vec<u8>)> {
        let decoded = self.decoded()?.ok()?;
        decoded_to_rgba(&decoded)
    }

    /// Borrows mip-0 bytes (or straight RGBA for 2x SVGs downsampled to intrinsic size) for
    /// off-thread un-premultiplication and PNG encoding on web.
    #[doc(hidden)]
    pub fn with_copy_pixels<R>(&self, f: impl FnOnce(CopyPixels<'_>) -> R) -> Option<R> {
        let decoded = self.decoded()?.ok()?;
        with_decoded_copy_pixels(&decoded, f)
    }
}
