use crate::ImageLimits;
use super::{checked_rgba_bytes, checked_rgba_bytes_raw, checked_rgba_bytes_with, is_avif, limits::{ALPHA_AUX_URN, check as check_limits, check_with as check_limits_with, inspect, too_large}};

struct BitWriter {
    bytes: Vec<u8>,
    bit_pos: usize,
}

impl BitWriter {
    fn new() -> Self { Self { bytes: Vec::new(), bit_pos: 0 } }

    fn push_bit(&mut self, bit: bool) {
        if self.bit_pos % 8 == 0 { self.bytes.push(0); }
        if bit {
            let idx = self.bit_pos / 8;
            self.bytes[idx] |= 1 << (7 - (self.bit_pos % 8));
        }
        self.bit_pos += 1;
    }

    fn push_bits(&mut self, value: u32, n: u8) {
        for shift in (0..n).rev() {
            self.push_bit(((value >> shift) & 1) != 0);
        }
    }

    fn finish(self) -> Vec<u8> { self.bytes }
}

fn obu(obu_type: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![(obu_type << 3) | 0x02];
    let mut len = payload.len();
    loop {
        let mut b = (len & 0x7f) as u8;
        len >>= 7;
        if len != 0 { b |= 0x80; }
        out.push(b);
        if len == 0 { break; }
    }
    out.extend_from_slice(payload);
    out
}

/// Minimal AV1 sequence header OBU with `reduced_still_picture_header = true`.
fn seq_hdr_reduced(w: u32, h: u32, high_bit_depth: bool, monochrome: bool) -> Vec<u8> {
    let mut bw = BitWriter::new();
    bw.push_bits(0, 3); // seq_profile = 0 (Main)
    bw.push_bit(true); // still_picture
    bw.push_bit(true); // reduced_still_picture_header
    bw.push_bits(0, 5); // seq_level_idx[0]
    let w_bits = (32 - (w - 1).leading_zeros()).max(1) as u8;
    let h_bits = (32 - (h - 1).leading_zeros()).max(1) as u8;
    bw.push_bits(u32::from(w_bits - 1), 4);
    bw.push_bits(u32::from(h_bits - 1), 4);
    bw.push_bits(w - 1, w_bits);
    bw.push_bits(h - 1, h_bits);
    bw.push_bits(0, 3); // superblock, filter_intra, intra_edge_filter
    bw.push_bits(0, 3); // superres, cdef, restoration
    bw.push_bit(high_bit_depth);
    bw.push_bit(monochrome);
    bw.push_bit(false); // color_description_present_flag
    bw.push_bit(false); // color_range
    if !monochrome {
        bw.push_bits(0, 2); // chroma_sample_position
        bw.push_bit(false); // separate_uv_delta_q
    }
    bw.push_bit(false); // film_grain_params_present
    bw.push_bit(true); // trailing bit
    obu(1, &bw.finish())
}

/// AV1 sequence header OBU with `reduced_still_picture_header = false` and explicit bit widths.
fn seq_hdr_full(w: u32, h: u32, w_bits: u8, h_bits: u8) -> Vec<u8> {
    let mut bw = BitWriter::new();
    bw.push_bits(0, 3); // seq_profile = 0
    bw.push_bit(true); // still_picture
    bw.push_bit(false); // reduced_still_picture_header
    bw.push_bit(false); // timing_info_present_flag
    bw.push_bit(false); // initial_display_delay_present_flag
    bw.push_bits(0, 5); // operating_points_cnt_minus_1 = 0
    bw.push_bits(0, 12); // operating_point_idc
    bw.push_bits(0, 5); // seq_level_idx
    bw.push_bits(u32::from(w_bits - 1), 4);
    bw.push_bits(u32::from(h_bits - 1), 4);
    bw.push_bits(w - 1, w_bits);
    bw.push_bits(h - 1, h_bits);
    bw.push_bit(false); // frame_id_numbers_present_flag
    bw.push_bits(0, 3); // superblock, filter_intra, intra_edge_filter
    bw.push_bits(0, 4); // interintra, masked, warped, dual_filter
    bw.push_bit(false); // enable_order_hint
    bw.push_bit(true); // seq_choose_screen_content_tools = adaptive
    bw.push_bit(true); // seq_choose_integer_mv = adaptive
    bw.push_bits(0, 3); // superres, cdef, restoration
    bw.push_bit(false); // high_bitdepth
    bw.push_bit(false); // monochrome
    bw.push_bit(false); // color_description_present_flag
    bw.push_bit(false); // color_range
    bw.push_bits(0, 2); // chroma_sample_position
    bw.push_bit(false); // separate_uv_delta_q
    bw.push_bit(false); // film_grain_params_present
    bw.push_bit(true); // trailing bit
    obu(1, &bw.finish())
}

/// Key-frame `OBU_FRAME` header with `frame_size_override_flag = 1`.
fn frame_obu_override(w: u32, h: u32, w_bits: u8, h_bits: u8) -> Vec<u8> {
    let mut bw = BitWriter::new();
    bw.push_bit(false); // show_existing_frame
    bw.push_bits(0, 2); // frame_type = KEY_FRAME
    bw.push_bit(true); // show_frame (implies error_resilient = true, refresh = 0xff)
    bw.push_bit(false); // disable_cdf_update
    bw.push_bit(false); // allow_screen_content_tools
    bw.push_bit(true); // frame_size_override_flag
    bw.push_bits(w - 1, w_bits);
    bw.push_bits(h - 1, h_bits);
    bw.push_bit(false); // render_and_frame_size_different
    obu(6, &bw.finish())
}

fn bmff_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(&((8 + body.len()) as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    out
}

fn full_box(kind: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut full = Vec::with_capacity(4 + body.len());
    full.push(version);
    full.push((flags >> 16) as u8);
    full.push((flags >> 8) as u8);
    full.push(flags as u8);
    full.extend_from_slice(body);
    bmff_box(kind, &full)
}

/// Builds a minimal ISO-BMFF AVIF container with optional `ispe` and optional alpha item.
fn build_avif(
    ispe: Option<(u32, u32)>,
    primary_av1: &[u8],
    alpha: Option<(Option<(u32, u32)>, &[u8])>,
) -> Vec<u8> {
    let ftyp = bmff_box(b"ftyp", b"avif\0\0\0\0mif1avif");
    let pitm = full_box(b"pitm", 0, 0, &1u16.to_be_bytes());

    let mut infe1 = Vec::new();
    infe1.extend_from_slice(&1u16.to_be_bytes());
    infe1.extend_from_slice(&0u16.to_be_bytes());
    infe1.extend_from_slice(b"av01");
    infe1.push(0);
    let infe1 = full_box(b"infe", 2, 0, &infe1);

    let mut iinf_body = Vec::new();
    let count: u16 = if alpha.is_some() { 2 } else { 1 };
    iinf_body.extend_from_slice(&count.to_be_bytes());
    iinf_body.extend_from_slice(&infe1);
    if alpha.is_some() {
        let mut infe2 = Vec::new();
        infe2.extend_from_slice(&2u16.to_be_bytes());
        infe2.extend_from_slice(&0u16.to_be_bytes());
        infe2.extend_from_slice(b"av01");
        infe2.push(0);
        iinf_body.extend_from_slice(&full_box(b"infe", 2, 0, &infe2));
    }
    let iinf = full_box(b"iinf", 0, 0, &iinf_body);

    let mut ipco_body = Vec::new();
    let mut assocs: Vec<(u16, u8)> = Vec::new();
    let mut prop_idx = 0u8;
    if let Some((w, h)) = ispe {
        let mut d = Vec::new();
        d.extend_from_slice(&w.to_be_bytes());
        d.extend_from_slice(&h.to_be_bytes());
        ipco_body.extend_from_slice(&full_box(b"ispe", 0, 0, &d));
        prop_idx += 1;
        assocs.push((1, prop_idx));
    }
    if let Some((alpha_ispe, _)) = alpha {
        let mut auxc = ALPHA_AUX_URN.to_vec();
        auxc.push(0);
        ipco_body.extend_from_slice(&full_box(b"auxC", 0, 0, &auxc));
        prop_idx += 1;
        assocs.push((2, prop_idx));
        if let Some((w, h)) = alpha_ispe {
            let mut d = Vec::new();
            d.extend_from_slice(&w.to_be_bytes());
            d.extend_from_slice(&h.to_be_bytes());
            ipco_body.extend_from_slice(&full_box(b"ispe", 0, 0, &d));
            prop_idx += 1;
            assocs.push((2, prop_idx));
        }
    }
    let mut ipma_body = Vec::new();
    ipma_body.extend_from_slice(&(assocs.len() as u32).to_be_bytes());
    for (item_id, idx) in assocs {
        ipma_body.extend_from_slice(&item_id.to_be_bytes());
        ipma_body.push(1);
        ipma_body.push(idx);
    }
    let mut iprp_body = bmff_box(b"ipco", &ipco_body);
    iprp_body.extend_from_slice(&full_box(b"ipma", 0, 0, &ipma_body));
    let iprp = bmff_box(b"iprp", &iprp_body);

    let iref = if alpha.is_some() {
        let mut auxl = Vec::new();
        auxl.extend_from_slice(&2u16.to_be_bytes()); // from alpha item 2
        auxl.extend_from_slice(&1u16.to_be_bytes()); // reference_count = 1
        auxl.extend_from_slice(&1u16.to_be_bytes()); // to primary item 1
        full_box(b"iref", 0, 0, &bmff_box(b"auxl", &auxl))
    } else {
        Vec::new()
    };

    let build_iloc = |primary_offset: u32, alpha_offset: u32| {
        let mut d = vec![0x44, 0x00]; // offset_size=4, length_size=4, base_offset_size=0
        d.extend_from_slice(&count.to_be_bytes());
        d.extend_from_slice(&1u16.to_be_bytes()); // item_id = 1
        d.extend_from_slice(&0u16.to_be_bytes()); // data_reference_index = 0
        d.extend_from_slice(&1u16.to_be_bytes()); // extent_count = 1
        d.extend_from_slice(&primary_offset.to_be_bytes());
        d.extend_from_slice(&(primary_av1.len() as u32).to_be_bytes());
        if let Some((_, alpha_av1)) = alpha {
            d.extend_from_slice(&2u16.to_be_bytes());
            d.extend_from_slice(&0u16.to_be_bytes());
            d.extend_from_slice(&1u16.to_be_bytes());
            d.extend_from_slice(&alpha_offset.to_be_bytes());
            d.extend_from_slice(&(alpha_av1.len() as u32).to_be_bytes());
        }
        full_box(b"iloc", 0, 0, &d)
    };

    let mut mdat_body = primary_av1.to_vec();
    if let Some((_, alpha_av1)) = alpha { mdat_body.extend_from_slice(alpha_av1); }
    let mdat = bmff_box(b"mdat", &mdat_body);

    // Compute exact mdat payload offset using a placeholder iloc of the same length.
    let placeholder_iloc = build_iloc(0, 0);
    let mut meta_body = Vec::new();
    meta_body.extend_from_slice(&pitm);
    meta_body.extend_from_slice(&iinf);
    meta_body.extend_from_slice(&iprp);
    meta_body.extend_from_slice(&iref);
    meta_body.extend_from_slice(&placeholder_iloc);
    let mdat_payload_offset = (ftyp.len() + 12 + meta_body.len() + 8) as u32;

    let iloc = build_iloc(mdat_payload_offset, mdat_payload_offset + primary_av1.len() as u32);
    meta_body.truncate(meta_body.len() - placeholder_iloc.len());
    meta_body.extend_from_slice(&iloc);
    let meta = full_box(b"meta", 0, 0, &meta_body);

    let mut out = ftyp;
    out.extend_from_slice(&meta);
    out.extend_from_slice(&mdat);
    out
}

#[test]
fn rejects_oversized_ispe_dimensions() {
    let small_seq = seq_hdr_reduced(16, 16, false, false);
    for (w, h) in [(16384, 16384), (8193, 1), (1, 8193), (0, 16), (16, 0)] {
        let avif = build_avif(Some((w, h)), &small_seq, None);
        assert!(is_avif(&avif));
        assert_eq!(check_limits(&avif), Err(too_large()), "ispe {w}x{h} must fail");
    }
}

#[test]
fn rejects_oversized_av1_bitstream_even_when_ispe_is_small_or_absent() {
    let huge_seq = seq_hdr_reduced(16384, 16384, false, false);
    let spoofed = build_avif(Some((16, 16)), &huge_seq, None);
    let no_ispe = build_avif(None, &huge_seq, None);
    assert_eq!(check_limits(&spoofed), Err(too_large()));
    assert_eq!(check_limits(&no_ispe), Err(too_large()));
}

#[test]
fn rejects_oversized_alpha_item() {
    let small = seq_hdr_reduced(16, 16, false, false);
    let huge_alpha = seq_hdr_reduced(16384, 16384, false, true);
    let by_ispe = build_avif(Some((16, 16)), &small, Some((Some((16384, 16384)), &small)));
    let by_bitstream = build_avif(Some((16, 16)), &small, Some((Some((16, 16)), &huge_alpha)));
    assert_eq!(check_limits(&by_ispe), Err(too_large()));
    assert_eq!(check_limits(&by_bitstream), Err(too_large()));
}

#[test]
fn rejects_second_sequence_header_exceeding_limits() {
    let mut obus = seq_hdr_reduced(16, 16, false, false);
    obus.extend_from_slice(&seq_hdr_reduced(16384, 16384, false, false));
    let avif = build_avif(Some((16, 16)), &obus, None);
    assert_eq!(check_limits(&avif), Err(too_large()));
}

#[test]
fn rejects_frame_size_override_exceeding_limits() {
    let mut obus = seq_hdr_full(16, 16, 14, 14);
    obus.extend_from_slice(&frame_obu_override(16384, 16, 14, 14));
    let avif = build_avif(Some((16, 16)), &obus, None);
    assert_eq!(check_limits(&avif), Err(too_large()));
}

#[test]
fn enforces_256_mib_allocation_cap_for_high_bit_depth() {
    let ok_8bit_rgba = build_avif(
        Some((8192, 8192)),
        &seq_hdr_reduced(8192, 8192, false, false),
        Some((Some((8192, 8192)), &seq_hdr_reduced(8192, 8192, false, true))),
    );
    assert_eq!(check_limits(&ok_8bit_rgba), Ok(()));

    let ok_10bit_rgba = build_avif(
        Some((4096, 4096)),
        &seq_hdr_reduced(4096, 4096, true, false),
        Some((Some((4096, 4096)), &seq_hdr_reduced(4096, 4096, true, true))),
    );
    assert_eq!(check_limits(&ok_10bit_rgba), Ok(()));

    let huge_10bit_rgb = build_avif(Some((8192, 8192)), &seq_hdr_reduced(8192, 8192, true, false), None);
    assert_eq!(check_limits(&huge_10bit_rgb), Err(too_large()));

    let huge_10bit_rgba = build_avif(
        Some((8192, 8192)),
        &seq_hdr_reduced(8192, 8192, true, false),
        Some((Some((8192, 8192)), &seq_hdr_reduced(8192, 8192, true, true))),
    );
    assert_eq!(check_limits(&huge_10bit_rgba), Err(too_large()));
}

#[test]
fn rejects_inflated_box_size_and_extent_amplification() {
    let mut bogus_box = bmff_box(b"ftyp", b"avif\0\0\0\0mif1avif");
    bogus_box.extend_from_slice(&0x4000_0000u32.to_be_bytes());
    bogus_box.extend_from_slice(b"mdat");
    bogus_box.extend_from_slice(&[0u8; 8]);
    assert!(check_limits(&bogus_box).is_err());

    // iloc with repeated extents referencing the same mdat payload to exceed input length.
    let ftyp = bmff_box(b"ftyp", b"avif\0\0\0\0mif1avif");
    let pitm = full_box(b"pitm", 0, 0, &1u16.to_be_bytes());
    let payload = [0u8; 64];
    let mdat = bmff_box(b"mdat", &payload);
    let mut iloc_d = vec![0x44, 0x00, 0, 1, 0, 1, 0, 0, 0, 32];
    let offset_placeholder = 0u32;
    for _ in 0..32 {
        iloc_d.extend_from_slice(&offset_placeholder.to_be_bytes());
        iloc_d.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    }
    let meta_placeholder = full_box(b"meta", 0, 0, &[pitm.clone(), full_box(b"iloc", 0, 0, &iloc_d)].concat());
    let mdat_offset = (ftyp.len() + meta_placeholder.len() + 8) as u32;
    for i in 0..32 {
        let pos = 10 + i * 8;
        iloc_d[pos..pos + 4].copy_from_slice(&mdat_offset.to_be_bytes());
    }
    let meta = full_box(b"meta", 0, 0, &[pitm, full_box(b"iloc", 0, 0, &iloc_d)].concat());
    let mut amplified = ftyp;
    amplified.extend_from_slice(&meta);
    amplified.extend_from_slice(&mdat);
    assert_eq!(check_limits(&amplified), Err(too_large()));
}

#[test]
fn checked_rgba_bytes_enforces_dimensions_allocation_and_overflow() {
    for (w, h) in [
        (0, 0),
        (0, 16),
        (16, 0),
        (crate::MAX_SIDE as usize + 1, 1),
        (1, crate::MAX_SIDE as usize + 1),
        (16384, 16384),
        (usize::MAX, 1),
        (1, usize::MAX),
        (usize::MAX, usize::MAX),
    ] {
        assert_eq!(checked_rgba_bytes(w, h), Err(too_large()), "{w}x{h} must fail");
    }

    assert_eq!(checked_rgba_bytes(1, 1), Ok((1, 1, 4)));
    assert_eq!(checked_rgba_bytes(320, 240), Ok((320, 240, 320 * 240 * 4)));
    assert_eq!(
        checked_rgba_bytes(crate::MAX_SIDE as usize, crate::MAX_SIDE as usize),
        Ok((crate::MAX_SIDE, crate::MAX_SIDE, crate::MAX_ALLOC as usize)),
    );

    // Byte-budget and pixel-budget enforcement independent of MAX_SIDE.
    let lim_exact = ImageLimits::new().with_max_side(128).with_max_alloc(64 * 64 * 4);
    assert_eq!(checked_rgba_bytes_with(64, 64, lim_exact), Ok((64, 64, 64 * 64 * 4)));
    assert_eq!(checked_rgba_bytes_with(64, 64, lim_exact.with_max_alloc(64 * 64 * 4 - 1)), Err(too_large()));
    assert_eq!(checked_rgba_bytes_with(64, 64, ImageLimits::new().with_max_pixels(64 * 64 - 1)), Err(too_large()));
    assert_eq!(checked_rgba_bytes_with(4096, 4096, ImageLimits::new().with_max_alloc(64 * 1024 * 1024 - 1)), Err(too_large()));

    // Checked multiplication overflow does not wrap.
    assert_eq!(checked_rgba_bytes_raw(u32::MAX as usize, u32::MAX as usize, u32::MAX, u64::MAX, u64::MAX), Err(too_large()));
    assert_eq!(checked_rgba_bytes_raw(1usize << 31, 1usize << 31, u32::MAX, u64::MAX, u64::MAX), Err(too_large()));
}

#[cfg(all(feature = "avif", not(target_arch = "wasm32")))]
#[test]
fn rgba_rejects_oversized_avif_before_decoding() {
    let huge_seq = seq_hdr_reduced(16384, 16384, false, false);
    let oversized_ispe = build_avif(Some((16384, 16384)), &huge_seq, None);
    let spoofed_ispe = build_avif(Some((16, 16)), &huge_seq, None);
    assert_eq!(super::rgba(&oversized_ispe), Err(too_large()));
    assert_eq!(super::rgba(&spoofed_ispe), Err(too_large()));
    assert_eq!(super::super::decode(&oversized_ispe).err(), Some(too_large()));
    assert_eq!(super::super::decode(&spoofed_ispe).err(), Some(too_large()));
}

#[test]
fn preflight_enforces_caller_supplied_image_limits() {
    let seq = seq_hdr_reduced(64, 32, false, false);
    let avif = build_avif(Some((64, 32)), &seq, None);
    assert_eq!(check_limits_with(&avif, ImageLimits::new().with_max_side(64)), Ok(()));
    assert_eq!(check_limits_with(&avif, ImageLimits::new().with_max_side(63)), Err(too_large()));
    assert_eq!(check_limits_with(&avif, ImageLimits::new().with_max_pixels(64 * 32)), Ok(()));
    assert_eq!(check_limits_with(&avif, ImageLimits::new().with_max_pixels(64 * 32 - 1)), Err(too_large()));
    assert_eq!(check_limits_with(&avif, ImageLimits::new().with_max_alloc(64 * 32 * 4)), Ok(()));
    assert_eq!(check_limits_with(&avif, ImageLimits::new().with_max_alloc(64 * 32 * 4 - 1)), Err(too_large()));
    assert_eq!(check_limits_with(&avif, ImageLimits::new().with_max_pixels(0)), Err(too_large()));
}

#[cfg(all(feature = "avif", not(target_arch = "wasm32")))]
#[test]
fn rav1d_enforces_frame_size_limit_and_conversion_budget_directly() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../www/public/img/hello-320.avif");
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(check_limits(&bytes), Ok(()));

    let (w, h, pixels) = super::rgba(&bytes).unwrap();
    assert!(w > 0 && h > 0);
    let total_px = u64::from(w) * u64::from(h);
    let expected_bytes = w as usize * h as usize * 4;
    assert_eq!(pixels.len(), expected_bytes);
    assert_eq!(pixels.capacity(), expected_bytes, "RGBA conversion buffer should pre-allocate exact capacity");
    assert!(pixels.as_chunks::<4>().0.iter().all(|px| px[3] == 255), "opaque AVIF must decode with alpha = 255");

    // Extract the raw AV1 primary bitstream and invoke rav1d directly (bypassing ISO-BMFF/AV1
    // preflight) to prove `Dav1dSettings::frame_size_limit` rejects oversized frames inside rav1d.
    let mut reader = bytes.as_slice();
    let parsed = avif_parse::read_avif(&mut reader).unwrap();
    let av1 = &parsed.primary_item;

    // Exact pixel budget succeeds through rav1d.
    let exact_px = ImageLimits::new().with_max_pixels(total_px);
    let (dw, dh, dpixels) = super::codec::decode_av1_frame_with_limit(av1, exact_px).unwrap();
    assert_eq!((dw, dh), (w, h));
    assert_eq!(dpixels, pixels);
    assert_eq!(dpixels.capacity(), expected_bytes);

    // One pixel tighter is rejected by rav1d itself with ERANGE -> too_large().
    let too_tight_px = ImageLimits::new().with_max_pixels(total_px - 1);
    assert_eq!(super::codec::decode_av1_frame_with_limit(av1, too_tight_px), Err(too_large()));

    // Zero pixel budget is rejected before opening rav1d (never passing 0 to frame_size_limit).
    assert_eq!(super::codec::decode_av1_frame_with_limit(av1, ImageLimits::new().with_max_pixels(0)), Err(too_large()));
    assert_eq!(super::codec::decode_av1_frame_with_limit(av1, ImageLimits::new().with_max_side(0)), Err(too_large()));
    assert_eq!(super::codec::decode_av1_frame_with_limit(av1, ImageLimits::new().with_max_alloc(3)), Err(too_large()));

    // Tight byte budget (implied pixel budget = total_px - 1) is rejected inside rav1d as well.
    let too_tight_alloc = ImageLimits::new().with_max_alloc(expected_bytes as u64 - 1);
    assert_eq!(super::codec::decode_av1_frame_with_limit(av1, too_tight_alloc), Err(too_large()));

    // When total pixels fit `frame_size_limit` (w*h <= max_side^2 if non-square, or side cap on
    // the long edge) the post-decode check still rejects before allocating the RGBA buffer.
    let tighter_side = ImageLimits::new().with_max_side(w.max(h) - 1);
    assert_eq!(super::codec::decode_avif(&bytes, tighter_side), Err(too_large()));

    // End-to-end through ImageCache::load_with_limits and load_scaled_with_limits, including
    // cache-key isolation between tighter preview limits and full-budget loads.
    let cache = crate::ImageCache::new(std::sync::Arc::new(|| {}));
    let rejected = cache.load_with_limits(bytes.clone(), ImageLimits::new().with_max_pixels(total_px - 1));
    assert_eq!(rejected.wait(), Err(&too_large()));
    let accepted = cache.load_with_limits(bytes.clone(), exact_px);
    assert_eq!(accepted.wait(), Ok([w, h]));
    let full = cache.load(bytes.clone());
    assert_eq!(full.wait(), Ok([w, h]));

    let scaled_rejected = cache.load_scaled_with_limits(bytes.clone(), 64.0, 1.0, ImageLimits::new().with_max_side(w.max(h) - 1));
    assert_eq!(scaled_rejected.wait(), Err(&too_large()));
    let scaled_ok = cache.load_scaled_with_limits(bytes, 64.0, 1.0, exact_px);
    let [sw, sh] = scaled_ok.wait().unwrap();
    assert!(sw <= 64 && sh <= 64 && sw > 0 && sh > 0);
}

/// Builds a minimal AVIF container with arbitrary `colr` boxes associated to given item IDs.
fn build_avif_with_colr(colr_entries: &[(u16, &[u8])]) -> Vec<u8> {
    let ftyp = bmff_box(b"ftyp", b"avif\0\0\0\0mif1avif");
    let pitm = full_box(b"pitm", 0, 0, &1u16.to_be_bytes());
    let primary_av1 = seq_hdr_reduced(16, 16, false, false);

    let mut infe1 = Vec::new();
    infe1.extend_from_slice(&1u16.to_be_bytes());
    infe1.extend_from_slice(&0u16.to_be_bytes());
    infe1.extend_from_slice(b"av01");
    infe1.push(0);
    let mut iinf_body = Vec::new();
    iinf_body.extend_from_slice(&1u16.to_be_bytes());
    iinf_body.extend_from_slice(&full_box(b"infe", 2, 0, &infe1));
    let iinf = full_box(b"iinf", 0, 0, &iinf_body);

    let mut ipco_body = Vec::new();
    let mut assocs: Vec<(u16, u8)> = Vec::new();
    let mut ispe_d = Vec::new();
    ispe_d.extend_from_slice(&16u32.to_be_bytes());
    ispe_d.extend_from_slice(&16u32.to_be_bytes());
    ipco_body.extend_from_slice(&full_box(b"ispe", 0, 0, &ispe_d));
    assocs.push((1, 1));
    let mut prop_idx = 1u8;
    for &(item_id, colr_body) in colr_entries {
        ipco_body.extend_from_slice(&bmff_box(b"colr", colr_body));
        prop_idx += 1;
        assocs.push((item_id, prop_idx));
    }
    let mut ipma_body = Vec::new();
    ipma_body.extend_from_slice(&(assocs.len() as u32).to_be_bytes());
    for (item_id, idx) in assocs {
        ipma_body.extend_from_slice(&item_id.to_be_bytes());
        ipma_body.push(1);
        ipma_body.push(idx);
    }
    let mut iprp_body = bmff_box(b"ipco", &ipco_body);
    iprp_body.extend_from_slice(&full_box(b"ipma", 0, 0, &ipma_body));
    let iprp = bmff_box(b"iprp", &iprp_body);

    let build_iloc = |offset: u32| {
        let mut d = vec![0x44, 0x00];
        d.extend_from_slice(&1u16.to_be_bytes());
        d.extend_from_slice(&1u16.to_be_bytes());
        d.extend_from_slice(&0u16.to_be_bytes());
        d.extend_from_slice(&1u16.to_be_bytes());
        d.extend_from_slice(&offset.to_be_bytes());
        d.extend_from_slice(&(primary_av1.len() as u32).to_be_bytes());
        full_box(b"iloc", 0, 0, &d)
    };
    let mdat = bmff_box(b"mdat", &primary_av1);
    let placeholder_iloc = build_iloc(0);
    let mut meta_body = [pitm, iinf, iprp, placeholder_iloc.clone()].concat();
    let mdat_payload_offset = (ftyp.len() + 12 + meta_body.len() + 8) as u32;
    meta_body.truncate(meta_body.len() - placeholder_iloc.len());
    meta_body.extend_from_slice(&build_iloc(mdat_payload_offset));
    let meta = full_box(b"meta", 0, 0, &meta_body);

    [ftyp, meta, mdat].concat()
}

#[test]
fn avif_inspect_extracts_primary_colr_prof_and_ricc_profiles() {
    let sample_icc = b"sample-icc-payload";
    let prof_body = [b"prof".as_slice(), sample_icc].concat();
    let ricc_body = [b"rICC".as_slice(), sample_icc].concat();
    let nclx_body = [b"nclx".as_slice(), &[0, 1, 0, 13, 0, 1, 0x80]].concat();

    // Primary item with prof.
    assert_eq!(inspect(&build_avif_with_colr(&[(1, &prof_body)])), Ok(Some(sample_icc.as_slice())));
    // Primary item with rICC.
    assert_eq!(inspect(&build_avif_with_colr(&[(1, &ricc_body)])), Ok(Some(sample_icc.as_slice())));
    // nclx only returns None.
    assert_eq!(inspect(&build_avif_with_colr(&[(1, &nclx_body)])), Ok(None));
    // nclx alongside prof still returns prof.
    assert_eq!(inspect(&build_avif_with_colr(&[(1, &nclx_body), (1, &prof_body)])), Ok(Some(sample_icc.as_slice())));
    // colr on a non-primary item is ignored for the primary item.
    assert_eq!(inspect(&build_avif_with_colr(&[(2, &prof_body)])), Ok(None));
    // Empty or truncated colr box is ignored cleanly without failing validation.
    assert_eq!(inspect(&build_avif_with_colr(&[(1, b"prof")])), Ok(None));
    assert_eq!(inspect(&build_avif_with_colr(&[(1, b"pr")])), Ok(None));
}

#[cfg(all(feature = "avif", not(target_arch = "wasm32")))]
fn inject_colr_into_hello_avif(orig: &[u8], fourcc: &[u8; 4], icc: &[u8]) -> Vec<u8> {
    let u32_be = |b: &[u8]| u32::from_be_bytes(b.try_into().unwrap()) as usize;
    let ftyp_len = u32_be(&orig[0..4]);
    let meta_start = ftyp_len;
    let meta_len = u32_be(&orig[meta_start..meta_start + 4]);
    let mdat = &orig[meta_start + meta_len..];

    let colr = bmff_box(b"colr", &[&fourcc[..], icc].concat());
    let mut new_meta_children = Vec::new();
    let mut iloc_pos_in_children = None;
    let mut pos = meta_start + 12;
    let meta_end = meta_start + meta_len;
    while pos < meta_end {
        let len = u32_be(&orig[pos..pos + 4]);
        let kind = &orig[pos + 4..pos + 8];
        let raw_box = &orig[pos..pos + len];
        if kind == b"iloc" {
            iloc_pos_in_children = Some(new_meta_children.len());
            new_meta_children.extend_from_slice(raw_box);
        } else if kind == b"iprp" {
            let mut iprp_children = Vec::new();
            let mut ipos = pos + 8;
            let mut prop_count = 0u8;
            while ipos < pos + len {
                let ilen = u32_be(&orig[ipos..ipos + 4]);
                let ikind = &orig[ipos + 4..ipos + 8];
                if ikind == b"ipco" {
                    let mut ppos = ipos + 8;
                    while ppos < ipos + ilen {
                        ppos += u32_be(&orig[ppos..ppos + 4]);
                        prop_count += 1;
                    }
                    let mut ipco_body = orig[ipos + 8..ipos + ilen].to_vec();
                    ipco_body.extend_from_slice(&colr);
                    prop_count += 1;
                    iprp_children.extend_from_slice(&bmff_box(b"ipco", &ipco_body));
                } else if ikind == b"ipma" {
                    // Version 0, flags 0: append prop_count to primary item 1's associations.
                    let ipma_body = &orig[ipos + 12..ipos + ilen];
                    let entry_count = u32_be(&ipma_body[0..4]);
                    let mut out_ipma = ipma_body[0..4].to_vec();
                    let mut epos = 4;
                    for _ in 0..entry_count {
                        let item_id = u16::from_be_bytes(ipma_body[epos..epos + 2].try_into().unwrap());
                        let count = ipma_body[epos + 2];
                        let assoc_slice = &ipma_body[epos + 3..epos + 3 + usize::from(count)];
                        out_ipma.extend_from_slice(&item_id.to_be_bytes());
                        if item_id == 1 {
                            out_ipma.push(count + 1);
                            out_ipma.extend_from_slice(assoc_slice);
                            out_ipma.push(prop_count);
                        } else {
                            out_ipma.push(count);
                            out_ipma.extend_from_slice(assoc_slice);
                        }
                        epos += 3 + usize::from(count);
                    }
                    iprp_children.extend_from_slice(&full_box(b"ipma", 0, 0, &out_ipma));
                } else {
                    iprp_children.extend_from_slice(&orig[ipos..ipos + ilen]);
                }
                ipos += ilen;
            }
            new_meta_children.extend_from_slice(&bmff_box(b"iprp", &iprp_children));
        } else {
            new_meta_children.extend_from_slice(raw_box);
        }
        pos += len;
    }

    // Shift iloc base_offsets by the exact delta in meta box size.
    let delta = (12 + new_meta_children.len() - meta_len) as u32;
    let iloc_off = iloc_pos_in_children.unwrap();
    let iloc_len = u32_be(&new_meta_children[iloc_off..iloc_off + 4]);
    let item_count = u16::from_be_bytes(new_meta_children[iloc_off + 14..iloc_off + 16].try_into().unwrap());
    let mut cursor = iloc_off + 16;
    for _ in 0..item_count {
        // v0 with offset_size=4, length_size=4, base_offset_size=4:
        // item_id(2), data_ref(2), base_offset(4), extent_count(2), extents(extent_count * 8).
        let base_pos = cursor + 4;
        let old_base = u32::from_be_bytes(new_meta_children[base_pos..base_pos + 4].try_into().unwrap());
        new_meta_children[base_pos..base_pos + 4].copy_from_slice(&(old_base + delta).to_be_bytes());
        let ext_count = u16::from_be_bytes(new_meta_children[cursor + 8..cursor + 10].try_into().unwrap()) as usize;
        cursor += 10 + ext_count * 8;
    }
    assert_eq!(cursor, iloc_off + iloc_len);

    let new_meta = full_box(b"meta", 0, 0, &new_meta_children);
    [&orig[..ftyp_len], &new_meta, mdat].concat()
}

#[cfg(all(feature = "avif", not(target_arch = "wasm32")))]
#[test]
fn avif_rgba_converts_display_p3_colr_prof_and_ricc_and_falls_back_on_srgb_or_corrupt() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../www/public/img/hello-320.avif");
    let untagged = std::fs::read(&path).unwrap();
    let (w, h, base_pixels) = super::rgba(&untagged).unwrap();

    let p3_icc = moxcms::ColorProfile::new_display_p3().encode().unwrap();
    let mut expected = base_pixels.clone();
    crate::icc::convert_rgba_to_srgb(Some(&p3_icc), &mut expected);
    assert_ne!(expected, base_pixels, "hello-320.avif must have non-grey pixels that shift under Display P3");

    for fourcc in [b"prof", b"rICC"] {
        let tagged = inject_colr_into_hello_avif(&untagged, fourcc, &p3_icc);
        assert_eq!(inspect(&tagged), Ok(Some(p3_icc.as_slice())));
        let (tw, th, tpixels) = super::rgba(&tagged).unwrap();
        assert_eq!((tw, th), (w, h));
        assert_eq!(tpixels, expected, "AVIF colr {:?} must convert to sRGB", std::str::from_utf8(fourcc));
    }

    // sRGB-tagged AVIF stays byte-identical to untagged.
    let srgb_icc = moxcms::ColorProfile::new_srgb().encode().unwrap();
    let srgb_avif = inject_colr_into_hello_avif(&untagged, b"rICC", &srgb_icc);
    assert_eq!(super::rgba(&srgb_avif).unwrap().2, base_pixels);

    // Corrupt ICC profile in colr does not fail decode and leaves pixels unchanged.
    let corrupt_avif = inject_colr_into_hello_avif(&untagged, b"prof", b"corrupt-icc-data");
    assert_eq!(super::rgba(&corrupt_avif).unwrap().2, base_pixels);
}

#[cfg(all(feature = "avif", not(target_arch = "wasm32")))]
#[test]
fn avif_rgba_decodes_real_alpha_plane_and_enforces_budget_on_alpha_too() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../www/public/img/hello-320.avif");
    let orig = std::fs::read(&path).unwrap();
    let (w, h, base_pixels) = super::rgba(&orig).unwrap();

    // Reuse hello-320.avif's real AV1 bitstream as both primary and auxiliary alpha items.
    let mut reader = orig.as_slice();
    let parsed = avif_parse::read_avif(&mut reader).unwrap();
    let av1 = &parsed.primary_item;
    let with_alpha = build_avif(Some((w, h)), av1, Some((Some((w, h)), av1)));

    let (aw, ah, alpha_pixels) = super::rgba(&with_alpha).unwrap();
    assert_eq!((aw, ah), (w, h));
    assert_eq!(alpha_pixels.len(), base_pixels.len());
    // RGB channels match the opaque decode; alpha channel now carries the decoded luma plane.
    assert!(alpha_pixels.as_chunks::<4>().0.iter().any(|px| px[3] != 255), "alpha plane must populate pixel alpha");
    for (orig_px, alpha_px) in base_pixels.as_chunks::<4>().0.iter().zip(alpha_pixels.as_chunks::<4>().0) {
        assert_eq!(&orig_px[..3], &alpha_px[..3]);
    }

    // Tighter pixel budget rejects the alpha AVIF inside rav1d too.
    let too_tight = ImageLimits::new().with_max_pixels(u64::from(w) * u64::from(h) - 1);
    assert_eq!(super::codec::decode_avif(&with_alpha, too_tight), Err(too_large()));
}
