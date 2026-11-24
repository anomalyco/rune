//! Scaled raster decoding (`ImageCache::load_scaled`) and intrinsic SVG dimensions
//! (`Image::intrinsic_size`).

use std::sync::Arc;

use rune::{El, Image, ImageCache, SurfaceFit, Waker, col, el, row, text};
use rune_kit::{Badge, BadgeVariant, Kit, SegmentedControl};

use crate::G;

const TARGET_PTS: [f32; 3] = [32.0, 64.0, 128.0];
const TARGET_LABELS: [&str; 3] = ["32 pt @ 2x", "64 pt @ 2x", "128 pt @ 2x"];

const BADGE_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="12" viewBox="0 0 24 12">
  <rect x="0.5" y="0.5" width="23" height="11" rx="2.5" fill="#1f6feb" stroke="#79c0ff" stroke-width="1"/>
  <circle cx="6.5" cy="6" r="2.5" fill="#ffffff"/>
  <rect x="11.5" y="4.5" width="8.5" height="3" rx="1.5" fill="#ffffff" fill-opacity="0.88"/>
</svg>"##;

const WIDE_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="5000" height="250" viewBox="0 0 5000 250">
  <rect width="5000" height="250" rx="24" fill="#161b22"/>
  <rect x="40" y="40" width="1560" height="170" rx="16" fill="#238636"/>
  <rect x="1720" y="40" width="1560" height="170" rx="16" fill="#1f6feb"/>
  <rect x="3400" y="40" width="1560" height="170" rx="16" fill="#8957e5"/>
</svg>"##;

#[derive(Default)]
pub struct ImageDemo {
    waker: Option<Waker>,
    cache: Option<ImageCache>,
    png: Option<Arc<[u8]>>,
    target_idx: usize,
    raster_full: Option<Image>,
    raster_scaled: Option<Image>,
    svg_default: Option<Image>,
    svg_scaled: Option<Image>,
    svg_capped: Option<Image>,
}

impl ImageDemo {
    pub(crate) fn set_waker(&mut self, waker: Waker) {
        self.waker = Some(waker);
    }

    pub(crate) fn is_loaded(&self) -> bool {
        self.cache.is_some()
    }

    pub(crate) fn enter(&mut self, wait: bool) {
        if self.cache.is_none() {
            let waker = self.waker.clone().unwrap_or_else(|| Arc::new(|| {}));
            let cache = ImageCache::new(waker);
            let png: Arc<[u8]> = encode_demo_png(256, 128).into();
            let target_pt = TARGET_PTS[self.target_idx];
            self.raster_full = Some(cache.load(png.clone()));
            self.raster_scaled = Some(cache.load_scaled(png.clone(), target_pt, 2.0));
            self.svg_default = Some(cache.load(BADGE_SVG));
            self.svg_scaled = Some(cache.load_scaled(BADGE_SVG, 48.0, 2.0));
            self.svg_capped = Some(cache.load(WIDE_SVG));
            self.png = Some(png);
            self.cache = Some(cache);
        }
        if wait {
            for img in [
                self.raster_full.as_ref(),
                self.raster_scaled.as_ref(),
                self.svg_default.as_ref(),
                self.svg_scaled.as_ref(),
                self.svg_capped.as_ref(),
            ]
            .into_iter()
            .flatten()
            {
                let _ = img.wait();
            }
        }
    }

    pub(crate) fn pick_target(&mut self, idx: usize) {
        let idx = idx.min(TARGET_PTS.len() - 1);
        self.target_idx = idx;
        if let (Some(cache), Some(png)) = (&self.cache, &self.png) {
            self.raster_scaled = Some(cache.load_scaled(png.clone(), TARGET_PTS[idx], 2.0));
        }
    }
}

pub(super) fn decode(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let c = &k.c;
    let demo = &s.images;

    let raster_full_card = image_card(
        k,
        "ImageCache::load",
        "Full source decode",
        demo.raster_full.as_ref(),
        "Full-resolution raster image",
        128.0,
        64.0,
    );
    let raster_scaled_card = image_card(
        k,
        "ImageCache::load_scaled",
        TARGET_LABELS[demo.target_idx],
        demo.raster_scaled.as_ref(),
        "Scaled thumbnail raster image",
        128.0,
        64.0,
    );

    let svg_row = row()
        .gap(12.0)
        .child(image_card(
            k,
            "SVG load (24x12)",
            "2x default raster",
            demo.svg_default.as_ref(),
            "Default 2x SVG badge",
            96.0,
            48.0,
        ))
        .child(image_card(
            k,
            "SVG load_scaled (48 pt @ 2x)",
            "Custom raster size",
            demo.svg_scaled.as_ref(),
            "Scaled SVG badge",
            96.0,
            48.0,
        ))
        .child(image_card(
            k,
            "SVG load (5000x250)",
            "4096 px capped raster",
            demo.svg_capped.as_ref(),
            "Wide capped SVG banner",
            144.0,
            24.0,
        ));

    col()
        .key("image-decode-story")
        .w(672.0)
        .gap(16.0)
        .child(
            row()
                .justify_between()
                .center_y()
                .child(
                    col()
                        .gap(2.0)
                        .child(text("Scaled raster decode & SVG intrinsic size").font(k.ty.ui_medium).color(c.text_strong))
                        .child(
                            text("Compare full decode against load_scaled, and inspect SVG intrinsic_size independent of raster resolution.")
                                .font(k.ty.small)
                                .color(c.text_weak),
                        ),
                )
                .child(
                    SegmentedControl::new(&TARGET_LABELS, Some(demo.target_idx))
                        .on_pick(|s: &mut G, i, _| s.images.pick_target(i))
                        .build(k)
                        .key("scaled-target"),
                ),
        )
        .child(row().gap(12.0).child(raster_full_card).child(raster_scaled_card))
        .child(svg_row)
}

fn image_card(
    k: &Kit,
    title: &str,
    tag: &str,
    image: Option<&Image>,
    label: &str,
    box_w: f32,
    box_h: f32,
) -> El {
    let c = &k.c;
    let size_text = match image.and_then(Image::size) {
        Some([w, h]) => format!("size(): {w}x{h} px (~{:.1} KiB)", approx_mip_kib(w, h)),
        None => "size(): pending".into(),
    };
    let intrinsic_text = match image.and_then(Image::intrinsic_size) {
        Some([w, h]) => format!("intrinsic_size(): {w:.0}x{h:.0}"),
        None => "intrinsic_size(): pending".into(),
    };
    let mut preview = el()
        .w(box_w)
        .h(box_h)
        .radius(k.radius.sm)
        .clip_rounded(k.radius.sm)
        .fill(c.layer_02)
        .border(1.0, c.border_weak)
        .role("img")
        .label(label);
    if let Some(img) = image {
        preview = preview.pixels_fit(img.pixels(), SurfaceFit::Fit);
    }
    col()
        .grow(1.0)
        .padding(12.0)
        .gap(8.0)
        .radius(k.radius.md)
        .fill(c.surface_base)
        .border(1.0, c.border_weak)
        .child(
            row()
                .justify_between()
                .center_y()
                .gap(8.0)
                .child(text(title).font(k.ty.small_medium).color(c.text_strong))
                .child(Badge::new(tag).variant(BadgeVariant::Subtle).build(k)),
        )
        .child(preview)
        .child(text(size_text).font(k.ty.mono).color(c.text_base))
        .child(text(intrinsic_text).font(k.ty.mono).color(c.text_weak))
}

fn approx_mip_kib(w: u32, h: u32) -> f64 {
    let mip0 = f64::from(w) * f64::from(h) * 4.0;
    (mip0 * 4.0 / 3.0) / 1024.0
}

/// Encodes a deterministic 256x128 RGBA test pattern as an uncompressed PNG with zero extra
/// crate dependencies.
fn encode_demo_png(width: u32, height: u32) -> Vec<u8> {
    let row_bytes = width as usize * 4 + 1;
    let mut raw = Vec::with_capacity(row_bytes * height as usize);
    for y in 0..height {
        raw.push(0); // filter: None
        for x in 0..width {
            let cell = ((x / 16) + (y / 16)) % 2 == 0;
            let stripe = ((x + y) / 8) % 4 == 0;
            let r: u8 = if x < width / 2 { 38 } else { 224 };
            let g: u8 = if y < height / 2 { 132 } else { 96 };
            let b: u8 = if cell { 232 } else { 156 };
            let boost = if stripe { 32u8 } else { 0 };
            raw.extend_from_slice(&[r.saturating_add(boost), g.saturating_add(boost), b, 255]);
        }
    }

    // Wrap raw scanlines in a valid zlib stream using uncompressed stored deflate blocks.
    let mut zlib = Vec::with_capacity(raw.len() + 32);
    zlib.extend_from_slice(&[0x78, 0x01]);
    let mut offset = 0usize;
    while offset < raw.len() {
        let chunk_len = (raw.len() - offset).min(65_535);
        let is_final = offset + chunk_len == raw.len();
        zlib.push(u8::from(is_final));
        let len16 = chunk_len as u16;
        zlib.extend_from_slice(&len16.to_le_bytes());
        zlib.extend_from_slice(&(!len16).to_le_bytes());
        zlib.extend_from_slice(&raw[offset..offset + chunk_len]);
        offset += chunk_len;
    }
    let (mut s1, mut s2) = (1u32, 0u32);
    for &b in &raw {
        s1 = (s1 + u32::from(b)) % 65_521;
        s2 = (s2 + s1) % 65_521;
    }
    zlib.extend_from_slice(&((s2 << 16) | s1).to_be_bytes());

    let push_chunk = |out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]| {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        let mut c = !0u32;
        for &b in kind.iter().chain(body.iter()) {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = (c >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(c & 1));
            }
        }
        out.extend_from_slice(&(!c).to_be_bytes());
    };

    let mut png = Vec::with_capacity(zlib.len() + 64);
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = [0u8; 13];
    ihdr[0..4].copy_from_slice(&width.to_be_bytes());
    ihdr[4..8].copy_from_slice(&height.to_be_bytes());
    ihdr[8] = 8; // bit depth
    ihdr[9] = 6; // RGBA
    push_chunk(&mut png, b"IHDR", &ihdr);
    push_chunk(&mut png, b"IDAT", &zlib);
    push_chunk(&mut png, b"IEND", &[]);
    png
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_demo_decodes_scaled_rasters_and_svg_intrinsic_dimensions() {
        let mut demo = ImageDemo::default();
        demo.enter(true);
        assert_eq!(demo.raster_full.as_ref().unwrap().size(), Some([256, 128]));
        assert_eq!(demo.raster_scaled.as_ref().unwrap().size(), Some([64, 32]));
        assert_eq!(demo.svg_default.as_ref().unwrap().size(), Some([48, 24]));
        assert_eq!(demo.svg_default.as_ref().unwrap().intrinsic_size(), Some([24.0, 12.0]));
        assert_eq!(demo.svg_scaled.as_ref().unwrap().size(), Some([96, 48]));
        assert_eq!(demo.svg_scaled.as_ref().unwrap().intrinsic_size(), Some([24.0, 12.0]));
        assert_eq!(demo.svg_capped.as_ref().unwrap().size(), Some([4096, 205]));
        assert_eq!(demo.svg_capped.as_ref().unwrap().intrinsic_size(), Some([5000.0, 250.0]));

        demo.pick_target(1);
        assert_eq!(demo.raster_scaled.as_ref().unwrap().wait(), Ok([128, 64]));
        demo.pick_target(2);
        assert_eq!(demo.raster_scaled.as_ref().unwrap().wait(), Ok([256, 128]));
    }
}
