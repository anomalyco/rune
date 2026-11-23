use super::imp::{MAX_SVG_BYTES, MAX_SVG_DEPTH, MAX_SVG_SIDE, svg_exceeds_depth};
use super::{svg_rgba, svg_rgba_cover};
use crate::gzip;

const TINY_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="#00ff00"/></svg>"##;

fn crc32(data: &[u8]) -> u32 {
    !data.iter().fold(!0u32, |c, &b| {
        (0..8).fold(c ^ u32::from(b), |c, _| (c >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(c & 1)))
    })
}

/// Minimal LSB-first bit writer for constructing valid DEFLATE/gzip streams in unit tests.
struct BitWriter {
    out: Vec<u8>,
    buf: u64,
    len: u32,
}

impl BitWriter {
    fn new() -> Self {
        Self { out: Vec::new(), buf: 0, len: 0 }
    }
    fn write_bits(&mut self, val: u32, n: u32) {
        self.buf |= u64::from(val) << self.len;
        self.len += n;
        while self.len >= 8 {
            self.out.push((self.buf & 0xff) as u8);
            self.buf >>= 8;
            self.len -= 8;
        }
    }
    fn write_huff(&mut self, code: u32, n: u32) {
        for i in (0..n).rev() {
            self.write_bits((code >> i) & 1, 1);
        }
    }
    fn finish(mut self) -> Vec<u8> {
        if self.len > 0 {
            self.out.push((self.buf & 0xff) as u8);
        }
        self.out
    }
}

/// Builds a valid gzip stream using a fixed-Huffman DEFLATE block that inflates to `byte` repeated
/// `total` times (e.g. 65 MiB in ~40 KiB of compressed bytes).
fn gzip_bomb(byte: u8, total: usize) -> Vec<u8> {
    assert!(total >= 1);
    let mut bw = BitWriter::new();
    bw.write_bits(1, 1); // BFINAL = 1
    bw.write_bits(1, 2); // BTYPE = 1 (fixed Huffman)
    // Emit one literal byte (0..=143 use 8-bit code 0x30 + byte).
    assert!(byte <= 143);
    bw.write_huff(0x30 + u32::from(byte), 8);
    let mut rem = total - 1;
    // Repeat with symbol 285 (length 258, 8-bit code 0b1100_0101 = 0xc5), distance 1 (5-bit code 0).
    while rem >= 258 {
        bw.write_huff(0xc5, 8);
        bw.write_huff(0, 5);
        rem -= 258;
    }
    while rem > 0 {
        bw.write_huff(0x30 + u32::from(byte), 8);
        rem -= 1;
    }
    bw.write_huff(0, 7); // symbol 256 (end of block)
    let deflate = bw.finish();

    let mut gz = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];
    gz.extend_from_slice(&deflate);
    // Trailer CRC32 and ISIZE (not even reached when capped below `total`, computed accurately anyway).
    let mut crc = !0u32;
    for _ in 0..total.min(1024) {
        crc = (0..8).fold(crc ^ u32::from(byte), |c, _| (c >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(c & 1)));
    }
    gz.extend_from_slice(&(!crc).to_le_bytes());
    gz.extend_from_slice(&(total as u32).to_le_bytes());
    gz
}

/// Encodes `data` as a valid gzip stream using a fixed-Huffman DEFLATE block (ASCII literals).
fn gzip_fixed_ascii(data: &[u8]) -> Vec<u8> {
    let mut bw = BitWriter::new();
    bw.write_bits(1, 1);
    bw.write_bits(1, 2);
    for &b in data {
        assert!(b <= 143);
        bw.write_huff(0x30 + u32::from(b), 8);
    }
    bw.write_huff(0, 7);
    let mut gz = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];
    gz.extend_from_slice(&bw.finish());
    gz.extend_from_slice(&crc32(data).to_le_bytes());
    gz.extend_from_slice(&(data.len() as u32).to_le_bytes());
    gz
}

/// Encodes `data` as a valid gzip stream using a dynamic-Huffman DEFLATE block.
fn gzip_dynamic_ascii(data: &[u8]) -> Vec<u8> {
    // 257 literal/end symbols: assign 8 bits to symbols 0..=127 and 256, and 1 unused 1-bit distance.
    // Even simpler: use 9-bit codes for 0..=256 (257 symbols -> 257/512 < 1? No, canonical tree is valid
    // when not oversubscribed, and 257 * 2^-9 = 257/512 < 1).
    let mut bw = BitWriter::new();
    bw.write_bits(1, 1); // BFINAL = 1
    bw.write_bits(2, 2); // BTYPE = 2 (dynamic)
    bw.write_bits(0, 5); // HLIT = 257
    bw.write_bits(0, 5); // HDIST = 1
    bw.write_bits(15, 4); // HCLEN = 19
    // Code-length alphabet: give symbols 0, 9, 17, 18 two-bit codes (2 bits each -> 4/4 = 1.0).
    // Order: [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15]
    let cl_lens = [0u32, 2, 2, 2, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    for &l in &cl_lens {
        bw.write_bits(l, 3);
    }
    // Canonical 2-bit codes sorted by symbol: 0 -> 00, 9 -> 01, 17 -> 10, 18 -> 11.
    // Emit 257 lengths of 9 (for literals 0..=255 and EOB 256), plus 1 length of 0 (dist 0).
    for _ in 0..257 {
        bw.write_huff(0b01, 2); // symbol 9
    }
    bw.write_huff(0b00, 2); // symbol 0 (distance 0 unused)
    // Canonical 9-bit codes for symbols 0..=256 are simply `sym` in 9 bits.
    for &b in data {
        bw.write_huff(u32::from(b), 9);
    }
    bw.write_huff(256, 9);
    let mut gz = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];
    gz.extend_from_slice(&bw.finish());
    gz.extend_from_slice(&crc32(data).to_le_bytes());
    gz.extend_from_slice(&(data.len() as u32).to_le_bytes());
    gz
}

fn png_header(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(24);
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    out.extend_from_slice(&13u32.to_be_bytes());
    out.extend_from_slice(b"IHDR");
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
    out
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(*chunk.get(1).unwrap_or(&0));
        let b2 = u32::from(*chunk.get(2).unwrap_or(&0));
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 { TABLE[((triple >> 6) & 0x3f) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[(triple & 0x3f) as usize] as char } else { '=' });
    }
    out
}

#[test]
fn svgz_decompresses_fixed_and_dynamic_streams_and_rejects_bombs_without_inflating_past_cap() {
    let fixed = gzip_fixed_ascii(TINY_SVG);
    assert_eq!(gzip::decompress(&fixed, MAX_SVG_BYTES).unwrap(), TINY_SVG);
    assert_eq!(svg_rgba(&fixed, 8).unwrap().0, 8);

    let dynamic = gzip_dynamic_ascii(TINY_SVG);
    assert_eq!(gzip::decompress(&dynamic, MAX_SVG_BYTES).unwrap(), TINY_SVG);
    assert_eq!(svg_rgba(&dynamic, 8).unwrap().0, 8);

    // Optional gzip header fields (FEXTRA, FNAME, FCOMMENT) and concatenated members decode cleanly.
    let mut with_hdr = vec![0x1f, 0x8b, 8, 0x04 | 0x08 | 0x10, 0, 0, 0, 0, 0, 0xff];
    with_hdr.extend_from_slice(&2u16.to_le_bytes()); // FEXTRA len
    with_hdr.extend_from_slice(b"ab");
    with_hdr.extend_from_slice(b"icon.svg\0"); // FNAME
    with_hdr.extend_from_slice(b"comment\0"); // FCOMMENT
    with_hdr.extend_from_slice(&fixed[10..]);
    assert_eq!(gzip::decompress(&with_hdr, MAX_SVG_BYTES).unwrap(), TINY_SVG);

    // 65 MiB gzip bomb compressed into ~420 KiB stops at the cap and fails immediately.
    let bomb = gzip_bomb(b' ', 65 << 20);
    assert!(bomb.len() < 1_000_000, "bomb stays compact: {} bytes", bomb.len());
    assert!(gzip::decompress(&bomb, 4096).is_err());
    let err = svg_rgba(&bomb, 16).unwrap_err();
    assert!(err.0.contains("too large"), "unexpected error: {err}");

    // Bad CRC is rejected.
    let mut bad_crc = fixed.clone();
    let last = bad_crc.len() - 5;
    bad_crc[last] ^= 0xff;
    assert!(gzip::decompress(&bad_crc, MAX_SVG_BYTES).is_err());
}

#[test]
fn svg_clamps_non_zero_raster_size_and_refuses_huge_patterns_and_embedded_rasters() {
    // Direct non-zero px past 4096 is clamped (e.g. a 20000w entry from ImageCache::url_set).
    let (w, h, rgba) = svg_rgba(TINY_SVG, 20_000).unwrap();
    assert_eq!((w, h), (MAX_SVG_SIDE, MAX_SVG_SIDE));
    assert_eq!(rgba.len(), (MAX_SVG_SIDE as usize) * (MAX_SVG_SIDE as usize) * 4);
    assert!(svg_rgba_cover(TINY_SVG, 20_000).is_err());

    // A 20,000 px pattern tile inside an 8x8 SVG is refused before resvg allocates a 1.6 GB pixmap.
    let huge_pattern = br##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8">
        <defs>
            <pattern id="p" width="20000" height="20000" patternUnits="userSpaceOnUse">
                <rect width="1" height="1" fill="#ff0000"/>
            </pattern>
        </defs>
        <rect width="8" height="8" fill="url(#p)"/>
    </svg>"##;
    assert!(svg_rgba(huge_pattern, 8).is_err(), "20,000 px pattern tile must be refused");

    // A tiny pattern scaled by patternTransform into a huge tile is also refused.
    let scaled_pattern = br##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8">
        <defs>
            <pattern id="p" width="10" height="10" patternUnits="userSpaceOnUse" patternTransform="scale(2000)">
                <rect width="1" height="1" fill="#ff0000"/>
            </pattern>
        </defs>
        <rect width="8" height="8" fill="url(#p)"/>
    </svg>"##;
    assert!(svg_rgba(scaled_pattern, 8).is_err(), "scaled huge pattern tile must be refused");

    // An ordinary small pattern still renders.
    let small_pattern = br##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16">
        <defs>
            <pattern id="p" width="4" height="4" patternUnits="userSpaceOnUse">
                <rect width="4" height="4" fill="#ff0000"/>
            </pattern>
        </defs>
        <rect width="16" height="16" fill="url(#p)"/>
    </svg>"##;
    assert_eq!(svg_rgba(small_pattern, 16).unwrap().0, 16);

    // Embedded data: PNG headers claiming 100,000 px or u32::MAX px are refused against MAX_SIDE/MAX_ALLOC.
    for (label, w, h) in [("100k", 100_000, 100_000), ("u32_max", u32::MAX, u32::MAX)] {
        let b64 = base64_encode(&png_header(w, h));
        let doc = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><image width="8" height="8" href="data:image/png;base64,{b64}"/></svg>"#
        );
        assert!(svg_rgba(doc.as_bytes(), 8).is_err(), "{label} embedded PNG must be refused");
    }

    // Valid small embedded PNG data: URL is accepted.
    let mut small_png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]))
        .write_to(&mut small_png, image::ImageFormat::Png)
        .unwrap();
    let b64 = base64_encode(&small_png.into_inner());
    let ok_doc = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><image width="8" height="8" href="data:image/png;base64,{b64}"/></svg>"#
    );
    assert!(svg_rgba(ok_doc.as_bytes(), 8).is_ok());
}

#[test]
fn svg_refuses_use_fanout_and_deep_nesting_including_comments_and_entities() {
    // Run on a 2 MiB thread so a stack-overflow regression aborts rather than silently passing.
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            // 1. 1,000-deep raw nesting.
            let deep = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8">{}{}</svg>"#,
                "<g>".repeat(1_000),
                "</g>".repeat(1_000)
            );
            assert!(svg_exceeds_depth(&deep, MAX_SVG_DEPTH));
            assert!(svg_rgba(deep.as_bytes(), 8).is_err());

            // 2. Comment-hidden close tags cannot fool the depth scan.
            let comment_trick = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8">{}<!-- {} -->{}</svg>"#,
                "<g>".repeat(600),
                "</g>".repeat(600),
                "</g>".repeat(600)
            );
            assert!(svg_exceeds_depth(&comment_trick, MAX_SVG_DEPTH));
            assert!(svg_rgba(comment_trick.as_bytes(), 8).is_err());

            // 3. Deep nesting hidden inside a reachable DTD entity is caught before roxmltree.
            let entity_deep = format!(
                r#"<!DOCTYPE svg [
                    <!ENTITY g10 "<g><g><g><g><g><g><g><g><g><g>">
                    <!ENTITY g100 "&g10;&g10;&g10;&g10;&g10;&g10;&g10;&g10;&g10;&g10;">
                    <!ENTITY c10 "</g></g></g></g></g></g></g></g></g></g>">
                    <!ENTITY c100 "&c10;&c10;&c10;&c10;&c10;&c10;&c10;&c10;&c10;&c10;">
                ]>
                <svg xmlns="http://www.w3.org/2000/svg" width="8" height="8">
                    &g100;&g100;&g100;&g100;&g100;&g100;
                    <rect width="8" height="8"/>
                    &c100;&c100;&c100;&c100;&c100;&c100;
                </svg>"#
            );
            assert!(svg_exceeds_depth(&entity_deep, MAX_SVG_DEPTH));
            assert!(svg_rgba(entity_deep.as_bytes(), 8).is_err());

            // 4. An unreachable DTD entity with deep tags does not trigger false refusal.
            let unused_entity = format!(
                r##"<!DOCTYPE svg [<!ENTITY unused "{}">]>
                <svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="#fff"/></svg>"##,
                "<g>".repeat(600)
            );
            assert!(!svg_exceeds_depth(&unused_entity, MAX_SVG_DEPTH));
            assert!(svg_rgba(unused_entity.as_bytes(), 8).is_ok());

            // 5. <use> fan-out expanding to 222,222+ nodes is refused.
            let fanout = r##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8">
                <defs>
                    <rect id="r0" width="1" height="1"/>
                    <g id="r1"><use href="#r0"/><use href="#r0"/><use href="#r0"/><use href="#r0"/><use href="#r0"/><use href="#r0"/><use href="#r0"/><use href="#r0"/><use href="#r0"/><use href="#r0"/></g>
                    <g id="r2"><use href="#r1"/><use href="#r1"/><use href="#r1"/><use href="#r1"/><use href="#r1"/><use href="#r1"/><use href="#r1"/><use href="#r1"/><use href="#r1"/><use href="#r1"/></g>
                    <g id="r3"><use href="#r2"/><use href="#r2"/><use href="#r2"/><use href="#r2"/><use href="#r2"/><use href="#r2"/><use href="#r2"/><use href="#r2"/><use href="#r2"/><use href="#r2"/></g>
                    <g id="r4"><use href="#r3"/><use href="#r3"/><use href="#r3"/><use href="#r3"/><use href="#r3"/><use href="#r3"/><use href="#r3"/><use href="#r3"/><use href="#r3"/><use href="#r3"/></g>
                    <g id="r5"><use href="#r4"/><use href="#r4"/><use href="#r4"/><use href="#r4"/><use href="#r4"/><use href="#r4"/><use href="#r4"/><use href="#r4"/><use href="#r4"/><use href="#r4"/></g>
                </defs>
                <use href="#r5"/><use href="#r5"/>
            </svg>"##;
            assert!(svg_rgba(fanout.as_bytes(), 8).is_err(), "<use> fan-out must be refused");

            // 6. Exponential 10-level <use> bomb (10^10 expanded nodes) is rejected before usvg expands it.
            let mut exp_use = String::from(r##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><defs><rect id="u0" width="1" height="1"/>"##);
            for lvl in 1..=10 {
                exp_use.push_str(&format!(r##"<g id="u{lvl}">"##));
                for _ in 0..10 {
                    exp_use.push_str(&format!(r##"<use href="#u{}"/>"##, lvl - 1));
                }
                exp_use.push_str("</g>");
            }
            exp_use.push_str(r##"</defs><use href="#u10"/></svg>"##);
            assert!(svg_rgba(exp_use.as_bytes(), 8).is_err(), "exponential <use> bomb must be refused");

            // 7. Ordinary small <use> succeeds.
            let ok_use = r##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8">
                <defs><rect id="r" width="8" height="8" fill="#ff0000"/></defs>
                <use href="#r"/>
            </svg>"##;
            assert!(svg_rgba(ok_use.as_bytes(), 8).is_ok());
        })
        .unwrap()
        .join()
        .unwrap();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn svg_refuses_local_file_references_and_file_loader_rejects_non_regular_paths() {
    let dir = std::env::temp_dir().join(format!("rune-svg-local-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let secret = dir.join("secret.png");
    image::RgbaImage::from_pixel(8, 8, image::Rgba([255, 0, 0, 255])).save(&secret).unwrap();

    // Even when a valid local PNG exists on disk, an SVG <image href> cannot read or draw it.
    let doc = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><image width="8" height="8" href="{}"/></svg>"#,
        secret.display()
    );
    let (_, _, rgba) = svg_rgba(doc.as_bytes(), 8).unwrap();
    assert!(rgba.iter().all(|&b| b == 0), "local file href must never be drawn");

    // Non-regular paths (directories, and device paths on Windows) are refused by read_regular_file
    // without blocking or reading.
    let dir_err = crate::read_regular_file(&dir).unwrap_err();
    assert!(dir_err.0.contains("not a regular file"), "unexpected error: {dir_err}");
    #[cfg(windows)]
    {
        assert!(crate::read_regular_file(std::path::Path::new(r"\\.\NUL")).is_err());
    }
    assert!(crate::read_regular_file(&secret).is_ok(), "regular files still load");

    // ImageCache::url_set with a 20000w SVG source clamps its rasterized size to MAX_SVG_SIDE.
    let svg_file = dir.join("wide.svg");
    std::fs::write(&svg_file, TINY_SVG).unwrap();
    let cache = crate::ImageCache::new(std::sync::Arc::new(|| {}));
    let wide = cache.url_set(&[(svg_file.to_str().unwrap(), 20_000)], 500.0, 2.0);
    assert_eq!(wide.wait(), Ok([MAX_SVG_SIDE, MAX_SVG_SIDE]));

    std::fs::remove_dir_all(&dir).unwrap();
}
