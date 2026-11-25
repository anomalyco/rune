//! Alignment and aspect-preserving placement for surfaces and pixel buffers: which edge a spare
//! or cropped side is shared from, and whether the frame is scaled at all.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SurfaceFit {
    #[default]
    Fit,
    Fill,
}

/// How an image scales when its drawn size differs from its source texels (`El::image_filter`).
///
/// Matches CSS `image-rendering: auto` ([`Self::Linear`], the default) and
/// `image-rendering: pixelated` ([`Self::Nearest`]), and `wgpu::FilterMode::{Linear, Nearest}`.
///
/// ```
/// use rune_image::ImageFilter;
///
/// assert_eq!(ImageFilter::default(), ImageFilter::Linear);
/// assert_ne!(ImageFilter::Nearest, ImageFilter::Linear);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImageFilter {
    /// Bilinear interpolation between neighbouring texels, using prefiltered mip levels when
    /// scaled down (the default).
    #[default]
    Linear,
    /// Nearest-neighbour (point) sampling from mip level 0, keeping crisp hard texel edges for
    /// pixel art, sprites, and zoomed bitmaps.
    Nearest,
}

/// How an image fills the area of an element outside its placed rectangle
/// (`El::pixels_wrap`, `El::pixels_tile`).
///
/// Matches Skia `SkTileMode::{kRepeat, kMirror, kClamp}`, wgpu
/// `AddressMode::{Repeat, MirrorRepeat, ClampToEdge}`, and Metal `MTLSamplerAddressMode`.
///
/// ```
/// use rune_image::PixelsWrap;
///
/// assert_ne!(PixelsWrap::Repeat, PixelsWrap::Mirror);
/// assert_ne!(PixelsWrap::Mirror, PixelsWrap::Extend);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PixelsWrap {
    /// Tile copies of the image side by side along both axes (CSS `background-repeat: repeat`,
    /// Skia `kRepeat`, `wgpu::AddressMode::Repeat`).
    Repeat,
    /// Reflect alternating tiles across every edge so adjacent copies meet seamlessly
    /// (Skia `kMirror`, `wgpu::AddressMode::MirrorRepeat`).
    Mirror,
    /// Extend the image's outermost edge rows, edge columns, and corner texels to the element's
    /// bounds (Skia `kClamp`, `wgpu::AddressMode::ClampToEdge`).
    Extend,
}

/// `El::surface_anchor` and `El::surface_unscaled`. The default centres and scales by `SurfaceFit`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfacePlace {
    pub anchor: [Align; 2],
    pub unscaled: bool,
}

impl SurfacePlace {
    pub const CENTRED: SurfacePlace = SurfacePlace { anchor: [Align::Center; 2], unscaled: false };
}

impl Default for SurfacePlace {
    fn default() -> Self { Self::CENTRED }
}

impl SurfacePlace {
    /// How much of the spare (or cropped) space along each axis lies before the image.
    pub fn share(self) -> [f32; 2] {
        self.anchor.map(|a| match a {
            Align::Start => 0.0,
            Align::Center => 0.5,
            Align::End => 1.0,
        })
    }
}

/// The image's rect in device pixels inside `node` ([x, y, w, h]); it may overflow the node, which
/// then crops it. `unscaled` draws one texel per device pixel and ignores `fit`.
pub fn image_rect(node: [f32; 4], source: [u32; 2], fit: SurfaceFit, place: SurfacePlace) -> [f32; 4] {
    let (sw, sh) = (source[0] as f32, source[1] as f32);
    let scale = if place.unscaled {
        1.0
    } else {
        let (a, b) = (node[2] / sw, node[3] / sh);
        match fit {
            SurfaceFit::Fit => a.min(b),
            SurfaceFit::Fill => a.max(b),
        }
    };
    let [kx, ky] = place.share();
    let (w, h) = (sw * scale, sh * scale);
    let x = node[0] + (node[2] - w) * kx;
    let y = node[1] + (node[3] - h) * ky;
    // At unit scale favour sharp shared text over a subpixel translation of its image.
    if (scale - 1.0).abs() < 1e-6 { [x.round(), y.round(), w, h] } else { [x, y, w, h] }
}
