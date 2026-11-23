use super::{is_avif, limits::{ALPHA_AUX_URN, check as check_limits, too_large}};

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
