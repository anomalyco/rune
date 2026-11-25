//! A decoded image's pixels read back as straight RGBA, for a copy to the clipboard or a save.
use super::Image;
use super::mips::{downsample_rgba_to, straight_rgba};

impl Image {
    /// The decoded image as straight-alpha sRGB RGBA8 (`width * height * 4` bytes, rows top
    /// first), at full resolution: a raster at its decoded size (after EXIF orientation and ICC
    /// conversion), an animation's or AVIF's first frame, and an SVG at its intrinsic size
    /// ([`Image::intrinsic_size`], rounded) when it was rasterized larger. An image from
    /// [`ImageCache::load_scaled`](crate::ImageCache::load_scaled) gives its scaled pixels.
    /// `None` while it decodes or when decoding failed.
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
        let mip = decoded.mips.first()?;
        let rgba = straight_rgba(&mip.pixels);
        let full = (mip.width, mip.height);
        let Some([iw, ih]) = decoded.intrinsic else { return Some((full.0, full.1, rgba)) };
        let side = |v: f32| if v.is_finite() { v.round().max(1.0) as u32 } else { u32::MAX };
        let (nw, nh) = (side(iw), side(ih));
        if nw > full.0 || nh > full.1 { return Some((full.0, full.1, rgba)); }
        downsample_rgba_to(full.0, full.1, rgba, nw, nh).ok()
    }
}
