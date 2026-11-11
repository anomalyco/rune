//! A frame's draw data as bytes (`--cpu-boot`): what `paint` reads, so a tool can paint it again
//! away from the app. Little-endian: `RCB1`, width, height, clear (4 × f32), flags (bit 0 lcd),
//! the instance count and instances, the glyph page count and pages (`ATLAS_SIZE`² bytes each),
//! then the image page count and each page's width, height and RGBA bytes.

use super::{Image, Scene};
use crate::render::Instance;
use crate::text::ATLAS_SIZE;

const MAGIC: &[u8; 4] = b"RCB1";

/// A decoded frame, owning what its [`Scene`] borrows.
pub struct Recording {
    instances: Vec<Instance>,
    glyph_pages: Vec<Vec<u8>>,
    images: Vec<(u32, Vec<u8>)>,
    size: (u32, u32),
    clear: [f32; 4],
    lcd: bool,
}

/// `scene` as bytes.
pub fn encode(scene: &Scene<'_>) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    for v in [scene.size.0, scene.size.1] { out.extend_from_slice(&v.to_le_bytes()); }
    for c in scene.clear { out.extend_from_slice(&c.to_le_bytes()); }
    out.extend_from_slice(&u32::from(scene.lcd).to_le_bytes());
    out.extend_from_slice(&(scene.instances.len() as u32).to_le_bytes());
    out.extend_from_slice(bytemuck::cast_slice(scene.instances));
    out.extend_from_slice(&(scene.glyph_pages.len() as u32).to_le_bytes());
    for page in &scene.glyph_pages { out.extend_from_slice(page); }
    out.extend_from_slice(&(scene.images.len() as u32).to_le_bytes());
    for image in &scene.images {
        let height = image.data.len() as u32 / (image.width * 4).max(1);
        for v in [image.width, height] { out.extend_from_slice(&v.to_le_bytes()); }
        out.extend_from_slice(image.data);
    }
    out
}

impl Recording {
    pub fn decode(bytes: &[u8]) -> Option<Recording> {
        let mut r = Reader { bytes, at: 0 };
        if r.take(4)? != MAGIC { return None; }
        let size = (r.u32()?, r.u32()?);
        let clear = [f32::from_bits(r.u32()?), f32::from_bits(r.u32()?), f32::from_bits(r.u32()?), f32::from_bits(r.u32()?)];
        let lcd = r.u32()? & 1 != 0;
        let count = r.u32()? as usize;
        let instances = bytemuck::pod_collect_to_vec(r.take(count * std::mem::size_of::<Instance>())?);
        let pages = r.u32()? as usize;
        let side = (ATLAS_SIZE * ATLAS_SIZE) as usize;
        let glyph_pages = (0..pages).map(|_| r.take(side).map(<[u8]>::to_vec)).collect::<Option<Vec<_>>>()?;
        let count = r.u32()? as usize;
        let mut images = Vec::with_capacity(count);
        for _ in 0..count {
            let (w, h) = (r.u32()?, r.u32()?);
            images.push((w, r.take((w * h * 4) as usize)?.to_vec()));
        }
        Some(Recording { instances, glyph_pages, images, size, clear, lcd })
    }

    /// The frame, ready for `paint`.
    pub fn scene(&self) -> Scene<'_> {
        Scene {
            instances: &self.instances,
            glyph_pages: self.glyph_pages.iter().map(Vec::as_slice).collect(),
            outlines: None,
            images: self.images.iter().map(|(width, data)| Image { width: *width, data, dirty: None }).collect(),
            size: self.size,
            clear: self.clear,
            lcd: self.lcd,
            srgb_blend: true,
            native_surfaces: false,
            compositor: false,
            smoothing: None,
            atlas_resets: 0,
            half: super::HalfRounding::default(),
        }
    }

    pub fn instances(&self) -> &[Instance] { &self.instances }

    pub fn size(&self) -> (u32, u32) { self.size }
}

struct Reader<'b> {
    bytes: &'b [u8],
    at: usize,
}

impl<'b> Reader<'b> {
    fn take(&mut self, n: usize) -> Option<&'b [u8]> {
        let s = self.bytes.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(s)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
}
