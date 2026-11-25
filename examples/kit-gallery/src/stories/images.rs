//! Scaled raster decoding (`ImageCache::load_scaled`), intrinsic SVG dimensions
//! (`Image::intrinsic_size`), and embedded Display P3 ICC-to-sRGB colour conversion.

use std::sync::Arc;

use rune::{El, Image, ImageCache, ImageMenu, SurfaceFit, Waker, col, el, row, text};
use rune_kit::{Badge, BadgeVariant, Kit, Markdown, SegmentedControl};

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
    icc_untagged: Option<Image>,
    icc_display_p3: Option<Image>,
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
            let swatch_untagged = encode_swatch_png(160, 48, None);
            let swatch_p3 = encode_swatch_png(160, 48, Some(&display_p3_icc_profile()));
            self.raster_full = Some(cache.load(png.clone()));
            self.raster_scaled = Some(cache.load_scaled(png.clone(), target_pt, 2.0));
            self.icc_untagged = Some(cache.load(swatch_untagged));
            self.icc_display_p3 = Some(cache.load(swatch_p3));
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
                self.icc_untagged.as_ref(),
                self.icc_display_p3.as_ref(),
                self.svg_default.as_ref(),
                self.svg_scaled.as_ref(),
                self.svg_capped.as_ref(),
            ]
            .into_iter()
            .flatten()
            {
                // Story fixture assets decode synchronously before snapping; errors surface on the card.
                let _decoded = img.wait();
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
        .child(
            row()
                .gap(12.0)
                .child(image_card(
                    k,
                    "Untagged PNG swatches",
                    "Raw sRGB bytes",
                    demo.icc_untagged.as_ref(),
                    "Untagged PNG colour swatches",
                    160.0,
                    48.0,
                ))
                .child(image_card(
                    k,
                    "Display P3 PNG (iCCP)",
                    "Converted to sRGB",
                    demo.icc_display_p3.as_ref(),
                    "Display P3 PNG colour swatches converted to sRGB",
                    160.0,
                    48.0,
                )),
        )
        .child(svg_row)
        .child(
            col()
                .padding(12.0)
                .gap(6.0)
                .radius(k.radius.md)
                .fill(c.surface_base)
                .border(1.0, c.border_weak)
                .child(text("Opt-in image context menu (Markdown::image_frame + El::image_menu)").font(k.ty.ui_medium).color(c.text_strong))
                .child(
                    text("Right-click the Markdown image below for Save Image As… (verbatim SVG), Copy Image, and Copy Image Address.")
                        .font(k.ty.small)
                        .color(c.text_weak),
                )
                .child({
                    let badge_img = demo.svg_default.clone();
                    let badge_bytes: Arc<[u8]> = Arc::from(BADGE_SVG);
                    Markdown::new("![Demo SVG badge](https://rune.dev/assets/badge.svg)")
                        .images(move |_| badge_img.clone())
                        .image_frame(move |el, frame| {
                            el.image_menu(
                                ImageMenu::new(frame.image)
                                    .save_bytes(badge_bytes.clone())
                                    .address(frame.src),
                            )
                        })
                        .build(k)
                }),
        )
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

/// Encodes a 5-band colour swatch PNG with an optional embedded `iCCP` profile so the Display P3
/// to sRGB colour shift is directly visible alongside the same raw bytes untagged.
fn encode_swatch_png(width: u32, height: u32, icc: Option<&[u8]>) -> Vec<u8> {
    const SWATCHES: [[u8; 4]; 5] = [
        [200, 120, 60, 255],
        [0, 200, 0, 255],
        [30, 200, 220, 255],
        [200, 60, 120, 255],
        [60, 120, 200, 255],
    ];
    let mut scanline = Vec::with_capacity(width as usize * 4 + 1);
    scanline.push(0); // filter: None
    for x in 0..width {
        let band = ((x as usize * SWATCHES.len()) / width as usize).min(SWATCHES.len() - 1);
        scanline.extend_from_slice(&SWATCHES[band]);
    }
    let raw = scanline.repeat(height as usize);
    let zlib_stored = |data: &[u8]| {
        let mut z = Vec::with_capacity(data.len() + 32);
        z.extend_from_slice(&[0x78, 0x01]);
        let mut offset = 0usize;
        while offset < data.len() {
            let chunk_len = (data.len() - offset).min(65_535);
            let is_final = offset + chunk_len == data.len();
            z.push(u8::from(is_final));
            let len16 = chunk_len as u16;
            z.extend_from_slice(&len16.to_le_bytes());
            z.extend_from_slice(&(!len16).to_le_bytes());
            z.extend_from_slice(&data[offset..offset + chunk_len]);
            offset += chunk_len;
        }
        let (mut s1, mut s2) = (1u32, 0u32);
        for &b in data {
            s1 = (s1 + u32::from(b)) % 65_521;
            s2 = (s2 + s1) % 65_521;
        }
        z.extend_from_slice(&((s2 << 16) | s1).to_be_bytes());
        z
    };
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
    let mut png = Vec::with_capacity(raw.len() + 512);
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = [0u8; 13];
    ihdr[0..4].copy_from_slice(&width.to_be_bytes());
    ihdr[4..8].copy_from_slice(&height.to_be_bytes());
    ihdr[8] = 8;
    ihdr[9] = 6;
    push_chunk(&mut png, b"IHDR", &ihdr);
    if let Some(profile) = icc {
        let mut iccp = b"Display P3\0".to_vec();
        iccp.push(0); // deflate compression method
        iccp.extend_from_slice(&zlib_stored(profile));
        push_chunk(&mut png, b"iCCP", &iccp);
    }
    push_chunk(&mut png, b"IDAT", &zlib_stored(&raw));
    push_chunk(&mut png, b"IEND", &[]);
    png
}

/// Minimal ICC v4.3 Display P3 RGB matrix-shaper profile (D50-adapted primaries + sRGB parametric
/// transfer curve) built directly without extra crate dependencies.
fn display_p3_icc_profile() -> Vec<u8> {
    let s15f16 = |v: f64| (v * 65_536.0).round() as i32;
    let xyz_tag = |x: f64, y: f64, z: f64| -> [u8; 20] {
        let mut t = [0u8; 20];
        t[0..4].copy_from_slice(b"XYZ ");
        t[8..12].copy_from_slice(&s15f16(x).to_be_bytes());
        t[12..16].copy_from_slice(&s15f16(y).to_be_bytes());
        t[16..20].copy_from_slice(&s15f16(z).to_be_bytes());
        t
    };
    // Parametric curve function type 3 (sRGB transfer function).
    let mut trc = [0u8; 32];
    trc[0..4].copy_from_slice(b"para");
    trc[8..10].copy_from_slice(&3u16.to_be_bytes());
    for (i, v) in [2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045].into_iter().enumerate() {
        trc[12 + i * 4..16 + i * 4].copy_from_slice(&s15f16(v).to_be_bytes());
    }

    let r_xyz = xyz_tag(0.515_121, 0.241_196, -0.001_053);
    let g_xyz = xyz_tag(0.291_977, 0.692_245, 0.041_885);
    let b_xyz = xyz_tag(0.157_104, 0.066_574, 0.784_073);
    let wtpt = xyz_tag(0.964_203, 1.0, 0.824_905);

    // 128-byte header + 4-byte tag count + 7 * 12-byte entries = 216 bytes before tag data.
    const DATA_OFF: u32 = 128 + 4 + 7 * 12;
    const TOTAL_SIZE: u32 = DATA_OFF + 4 * 20 + 32;
    let mut icc = vec![0u8; TOTAL_SIZE as usize];
    icc[0..4].copy_from_slice(&TOTAL_SIZE.to_be_bytes());
    icc[8..12].copy_from_slice(&0x0430_0000u32.to_be_bytes()); // v4.3.0
    icc[12..16].copy_from_slice(b"mntr");
    icc[16..20].copy_from_slice(b"RGB ");
    icc[20..24].copy_from_slice(b"XYZ ");
    icc[36..40].copy_from_slice(b"acsp");
    icc[64..68].copy_from_slice(&1u32.to_be_bytes()); // Relative colorimetric
    icc[68..72].copy_from_slice(&s15f16(0.964_203).to_be_bytes());
    icc[72..76].copy_from_slice(&s15f16(1.0).to_be_bytes());
    icc[76..80].copy_from_slice(&s15f16(0.824_905).to_be_bytes());

    let trc_off = DATA_OFF + 80;
    let tags: [(&[u8; 4], u32, u32); 7] = [
        (b"rXYZ", DATA_OFF, 20),
        (b"gXYZ", DATA_OFF + 20, 20),
        (b"bXYZ", DATA_OFF + 40, 20),
        (b"wtpt", DATA_OFF + 60, 20),
        (b"rTRC", trc_off, 32),
        (b"gTRC", trc_off, 32),
        (b"bTRC", trc_off, 32),
    ];
    icc[128..132].copy_from_slice(&(tags.len() as u32).to_be_bytes());
    for (i, (sig, off, len)) in tags.into_iter().enumerate() {
        let p = 132 + i * 12;
        icc[p..p + 4].copy_from_slice(sig);
        icc[p + 4..p + 8].copy_from_slice(&off.to_be_bytes());
        icc[p + 8..p + 12].copy_from_slice(&len.to_be_bytes());
    }
    icc[DATA_OFF as usize..DATA_OFF as usize + 20].copy_from_slice(&r_xyz);
    icc[DATA_OFF as usize + 20..DATA_OFF as usize + 40].copy_from_slice(&g_xyz);
    icc[DATA_OFF as usize + 40..DATA_OFF as usize + 60].copy_from_slice(&b_xyz);
    icc[DATA_OFF as usize + 60..DATA_OFF as usize + 80].copy_from_slice(&wtpt);
    icc[trc_off as usize..trc_off as usize + 32].copy_from_slice(&trc);
    icc
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
        assert_eq!(demo.icc_untagged.as_ref().unwrap().size(), Some([160, 48]));
        assert_eq!(demo.icc_display_p3.as_ref().unwrap().size(), Some([160, 48]));
        // Verify the embedded Display P3 profile actually shifts decoded sRGB bytes vs untagged.
        let untagged_rgba = rune::image::png_rgba8(&encode_swatch_png(160, 48, None)).unwrap();
        let p3_rgba = rune::image::png_rgba8(&encode_swatch_png(160, 48, Some(&display_p3_icc_profile()))).unwrap();
        assert_eq!(&untagged_rgba.as_raw()[0..4], &[200, 120, 60, 255]);
        assert!(
            p3_rgba.as_raw()[0] >= 212 && p3_rgba.as_raw()[2] <= 44,
            "Display P3 swatch must convert toward sRGB (213, 115, 42..43): {:?}",
            &p3_rgba.as_raw()[0..4],
        );
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
