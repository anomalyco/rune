//! Pre-decode ISO-BMFF and AV1 header validation so oversized or malformed AVIF inputs fail
//! before `rav1d` or the browser allocates raster buffers.
use std::borrow::Cow;

use crate::{ImageError, ImageLimits};
pub(super) const ALPHA_AUX_URN: &[u8] = b"urn:mpeg:mpegB:cicp:systems:auxiliary:alpha";

pub(super) fn too_large() -> ImageError { ImageError("AVIF is too large".into()) }

fn invalid(msg: &str) -> ImageError { ImageError(format!("AVIF: {msg}")) }

fn check_dimensions(w: u32, h: u32, bytes_per_pixel: u64, limits: ImageLimits) -> Result<(), ImageError> {
    if !limits.fits(w, h, bytes_per_pixel) { return Err(too_large()); }
    Ok(())
}

#[cfg(test)]
pub(super) fn check(bytes: &[u8]) -> Result<(), ImageError> { check_with(bytes, ImageLimits::new()) }

pub(super) fn check_with(bytes: &[u8], limits: ImageLimits) -> Result<(), ImageError> { inspect_with(bytes, limits).map(|_| ()) }

#[cfg(test)]
pub(super) fn inspect(bytes: &[u8]) -> Result<Option<&[u8]>, ImageError> { inspect_with(bytes, ImageLimits::new()) }

pub(super) fn inspect_with(bytes: &[u8], limits: ImageLimits) -> Result<Option<&[u8]>, ImageError> {
    let mut top = BoxIter::new(bytes, 0);
    let first = top.next_box()?.ok_or_else(|| invalid("empty file"))?;
    if first.kind != *b"ftyp" { return Err(invalid("ftyp box must occur first")); }
    let brand = first.data.get(..4).ok_or_else(|| invalid("truncated ftyp"))?;
    if brand == b"avis" { return Err(invalid("animated AVIF is not supported")); }
    if brand != b"avif" { return Err(invalid("ftyp major brand must be avif")); }

    let mut meta = None;
    let mut mdats = Vec::new();
    while let Some(b) = top.next_box()? {
        match &b.kind {
            b"meta" => {
                if meta.is_some() { return Err(invalid("duplicate meta box")); }
                meta = Some(parse_meta(b.data)?);
            }
            b"mdat" if !b.data.is_empty() => mdats.push(Mdat { offset: b.file_offset, data: b.data }),
            _ => {}
        }
    }
    let meta = meta.ok_or_else(|| invalid("missing meta box"))?;
    let primary_id = meta.primary_item_id.ok_or_else(|| invalid("missing pitm box"))?;
    let alpha_id = meta.auxl_refs.iter()
        .filter(|&&(_, to)| to == primary_id)
        .map(|&(from, _)| from)
        .find(|&id| meta.has_alpha_aux(id));

    let mut icc = None;
    for &(item_id, prop_idx) in &meta.associations {
        if item_id != primary_id && Some(item_id) != alpha_id { continue; }
        match meta.properties.get(prop_idx) {
            Some(&Property::Ispe(w, h)) => check_dimensions(w, h, 4, limits)?,
            Some(&Property::ColrIcc(p)) if item_id == primary_id && icc.is_none() => icc = Some(p),
            _ => {}
        }
    }

    let iloc = meta.iloc.ok_or_else(|| invalid("missing iloc box"))?;
    let (primary_extents, alpha_extents) = parse_iloc(iloc, primary_id, alpha_id)?;
    let primary = resolve_extents(&primary_extents, &mdats, bytes.len())?;
    let alpha = (!alpha_extents.is_empty())
        .then(|| resolve_extents(&alpha_extents, &mdats, bytes.len())).transpose()?;

    check_av1_bitstream(&primary, false, alpha.is_some(), limits)?;
    if let Some(alpha) = &alpha {
        check_av1_bitstream(alpha, true, false, limits)?;
    }
    Ok(icc)
}

struct BmffBox<'a> {
    kind: [u8; 4],
    data: &'a [u8],
    file_offset: u64,
}

struct BoxIter<'a> {
    bytes: &'a [u8],
    pos: usize,
    base: u64,
}

impl<'a> BoxIter<'a> {
    fn new(bytes: &'a [u8], base: u64) -> Self { Self { bytes, pos: 0, base } }

    fn next_box(&mut self) -> Result<Option<BmffBox<'a>>, ImageError> {
        if self.pos == self.bytes.len() { return Ok(None); }
        let header = self.bytes.get(self.pos..self.pos + 8).ok_or_else(|| invalid("truncated box header"))?;
        let size32 = u32::from_be_bytes(header[0..4].try_into().unwrap());
        let kind: [u8; 4] = header[4..8].try_into().unwrap();
        let (size64, mut hdr) = match size32 {
            0 => return Err(invalid("unknown sized box")),
            1 => {
                let wide = self.bytes.get(self.pos + 8..self.pos + 16).ok_or_else(|| invalid("truncated wide box"))?;
                let s = u64::from_be_bytes(wide.try_into().unwrap());
                if s < 16 { return Err(invalid("malformed wide box size")); }
                (s, 16usize)
            }
            s if s < 8 => return Err(invalid("malformed box size")),
            s => (u64::from(s), 8usize),
        };
        let size = usize::try_from(size64).map_err(|_| invalid("box size exceeds input"))?;
        let end = self.pos.checked_add(size).filter(|&end| end <= self.bytes.len())
            .ok_or_else(|| invalid("box extends past input"))?;
        if kind == *b"uuid" && size >= hdr + 16 { hdr += 16; }
        let file_offset = self.base + (self.pos + hdr) as u64;
        let data = &self.bytes[self.pos + hdr..end];
        self.pos = end;
        Ok(Some(BmffBox { kind, data, file_offset }))
    }
}

fn split_fullbox(data: &[u8]) -> Result<(u8, u32, &[u8]), ImageError> {
    if data.len() < 4 { return Err(invalid("truncated full box")); }
    let flags = (u32::from(data[1]) << 16) | (u32::from(data[2]) << 8) | u32::from(data[3]);
    Ok((data[0], flags, &data[4..]))
}

enum Property<'a> {
    Ispe(u32, u32),
    AlphaAux,
    ColrIcc(&'a [u8]),
    Other,
}

struct Meta<'a> {
    primary_item_id: Option<u32>,
    iloc: Option<&'a [u8]>,
    auxl_refs: Vec<(u32, u32)>,
    properties: Vec<Property<'a>>,
    associations: Vec<(u32, usize)>,
}

impl Meta<'_> {
    fn has_alpha_aux(&self, item_id: u32) -> bool {
        self.associations.iter().any(|&(id, prop_idx)| {
            id == item_id && matches!(self.properties.get(prop_idx), Some(Property::AlphaAux))
        })
    }
}

fn parse_meta(data: &[u8]) -> Result<Meta<'_>, ImageError> {
    let (version, flags, rest) = split_fullbox(data)?;
    if version != 0 || flags != 0 { return Err(invalid("unsupported meta header")); }
    let mut meta = Meta {
        primary_item_id: None,
        iloc: None,
        auxl_refs: Vec::new(),
        properties: Vec::new(),
        associations: Vec::new(),
    };
    let mut iter = BoxIter::new(rest, 0);
    while let Some(b) = iter.next_box()? {
        match &b.kind {
            b"pitm" => {
                if meta.primary_item_id.is_some() { return Err(invalid("duplicate pitm box")); }
                let (v, f, d) = split_fullbox(b.data)?;
                if f != 0 { return Err(invalid("unexpected pitm flags")); }
                let id = match v {
                    0 => u32::from(read_u16(&mut &*d)?),
                    1 => read_u32(&mut &*d)?,
                    _ => return Err(invalid("unsupported pitm version")),
                };
                meta.primary_item_id = Some(id);
            }
            b"iinf" => {
                let (v, f, mut d) = split_fullbox(b.data)?;
                if f != 0 { return Err(invalid("unexpected iinf flags")); }
                let count = match v {
                    0 => usize::from(read_u16(&mut d)?),
                    1 => usize::try_from(read_u32(&mut d)?).map_err(|_| invalid("iinf count overflow"))?,
                    _ => return Err(invalid("unsupported iinf version")),
                };
                // Each infe v2/v3 entry is at least 20 bytes; reject inflated counts before avif-parse allocates.
                if count > d.len() / 12 { return Err(invalid("iinf entry count exceeds box size")); }
            }
            b"iloc" => {
                if meta.iloc.is_some() { return Err(invalid("duplicate iloc box")); }
                meta.iloc = Some(b.data);
            }
            b"iref" => parse_iref(b.data, &mut meta.auxl_refs)?,
            b"iprp" => parse_iprp(b.data, &mut meta.properties, &mut meta.associations)?,
            _ => {}
        }
    }
    Ok(meta)
}

fn parse_iref(data: &[u8], auxl_refs: &mut Vec<(u32, u32)>) -> Result<(), ImageError> {
    let (version, flags, rest) = split_fullbox(data)?;
    if version > 1 || flags != 0 { return Err(invalid("unsupported iref header")); }
    let mut iter = BoxIter::new(rest, 0);
    while let Some(b) = iter.next_box()? {
        let mut d = b.data;
        let from = if version == 0 { u32::from(read_u16(&mut d)?) } else { read_u32(&mut d)? };
        let count = read_u16(&mut d)?;
        for _ in 0..count {
            let to = if version == 0 { u32::from(read_u16(&mut d)?) } else { read_u32(&mut d)? };
            if from == to { return Err(invalid("self-referential iref")); }
            if &b.kind == b"auxl" { auxl_refs.push((from, to)); }
        }
    }
    Ok(())
}

fn parse_iprp<'a>(data: &'a [u8], properties: &mut Vec<Property<'a>>, associations: &mut Vec<(u32, usize)>) -> Result<(), ImageError> {
    let mut iter = BoxIter::new(data, 0);
    while let Some(b) = iter.next_box()? {
        match &b.kind {
            b"ipco" => {
                properties.clear();
                let mut props = BoxIter::new(b.data, 0);
                while let Some(p) = props.next_box()? {
                    let prop = match &p.kind {
                        b"ispe" => {
                            let (v, f, mut d) = split_fullbox(p.data)?;
                            if v != 0 || f != 0 { return Err(invalid("unsupported ispe header")); }
                            let w = read_u32(&mut d)?;
                            let h = read_u32(&mut d)?;
                            Property::Ispe(w, h)
                        }
                        b"auxC" => {
                            let (v, f, d) = split_fullbox(p.data)?;
                            if v != 0 || f != 0 { return Err(invalid("unsupported auxC header")); }
                            let urn = d.split(|&b| b == 0).next().unwrap_or(d);
                            if urn == ALPHA_AUX_URN { Property::AlphaAux } else { Property::Other }
                        }
                        b"colr" => p.data.split_at_checked(4)
                            .filter(|(t, d)| matches!(*t, b"prof" | b"rICC") && !d.is_empty())
                            .map_or(Property::Other, |(_, d)| Property::ColrIcc(d)),
                        _ => Property::Other,
                    };
                    properties.push(prop);
                }
            }
            b"ipma" => {
                let (v, f, mut d) = split_fullbox(b.data)?;
                let entry_count = read_u32(&mut d)?;
                let min_entry = if v == 0 { 3usize } else { 5 };
                if usize::try_from(entry_count).ok().is_none_or(|n| n > d.len() / min_entry) {
                    return Err(invalid("ipma entry count exceeds box size"));
                }
                for _ in 0..entry_count {
                    let item_id = if v == 0 { u32::from(read_u16(&mut d)?) } else { read_u32(&mut d)? };
                    let assoc_count = read_u8(&mut d)?;
                    for _ in 0..assoc_count {
                        let prop_index = if f & 1 != 0 {
                            read_u16(&mut d)? & 0x7fff
                        } else {
                            u16::from(read_u8(&mut d)? & 0x7f)
                        };
                        if prop_index != 0 {
                            associations.push((item_id, usize::from(prop_index - 1)));
                        }
                    }
                }
            }
            _ => return Err(invalid("unexpected iprp child")),
        }
    }
    Ok(())
}

struct Extent {
    start: u64,
    len: Option<u64>,
}

fn parse_iloc(data: &[u8], primary_id: u32, alpha_id: Option<u32>) -> Result<(Vec<Extent>, Vec<Extent>), ImageError> {
    let (v, f, mut d) = split_fullbox(data)?;
    if v > 2 || f != 0 { return Err(invalid("unsupported iloc header")); }
    let b0 = read_u8(&mut d)?;
    let b1 = read_u8(&mut d)?;
    let offset_size = field_size(b0 >> 4)?;
    let length_size = field_size(b0 & 0x0f)?;
    let base_offset_size = field_size(b1 >> 4)?;
    let index_size = if v == 0 { 0 } else { field_size(b1 & 0x0f)? };
    let item_count = if v < 2 { u32::from(read_u16(&mut d)?) } else { read_u32(&mut d)? };
    let min_item = match v { 0 => 6usize, 1 => 8, _ => 10 };
    if usize::try_from(item_count).ok().is_none_or(|n| n > d.len() / min_item) {
        return Err(invalid("iloc item count exceeds box size"));
    }

    let mut primary = Vec::new();
    let mut alpha = Vec::new();
    for _ in 0..item_count {
        let item_id = if v < 2 { u32::from(read_u16(&mut d)?) } else { read_u32(&mut d)? };
        let method = if v == 0 { 0 } else { read_u16(&mut d)? & 0x0f };
        let data_ref = read_u16(&mut d)?;
        if data_ref != 0 { return Err(invalid("external iloc references are not supported")); }
        let base_offset = read_var_u64(&mut d, base_offset_size)?;
        let extent_count = read_u16(&mut d)?;
        if extent_count == 0 { return Err(invalid("iloc item has zero extents")); }
        let mut target = if item_id == primary_id {
            Some(&mut primary)
        } else if Some(item_id) == alpha_id {
            Some(&mut alpha)
        } else {
            None
        };
        if target.is_some() && method != 0 {
            return Err(invalid("unsupported iloc construction method"));
        }
        let extent_bytes = usize::from(index_size) + usize::from(offset_size) + usize::from(length_size);
        if extent_bytes == 0 && extent_count > 1 {
            return Err(too_large());
        }
        if extent_bytes > 0 && usize::from(extent_count) > d.len() / extent_bytes {
            return Err(invalid("iloc extent count exceeds box size"));
        }
        for _ in 0..extent_count {
            let _ = read_var_u64(&mut d, index_size)?;
            let offset = read_var_u64(&mut d, offset_size)?;
            let length = read_var_u64(&mut d, length_size)?;
            let start = base_offset.checked_add(offset).ok_or_else(|| invalid("iloc offset overflow"))?;
            if length != 0 {
                start.checked_add(length).ok_or_else(|| invalid("iloc end overflow"))?;
            }
            if let Some(out) = &mut target {
                out.push(Extent { start, len: (length != 0).then_some(length) });
            }
        }
    }
    if !d.is_empty() { return Err(invalid("trailing bytes in iloc")); }
    Ok((primary, alpha))
}

fn field_size(n: u8) -> Result<u8, ImageError> {
    match n {
        0 | 4 | 8 => Ok(n),
        _ => Err(invalid("invalid iloc field size")),
    }
}

struct Mdat<'a> {
    offset: u64,
    data: &'a [u8],
}

fn resolve_extents<'a>(extents: &[Extent], mdats: &[Mdat<'a>], max_len: usize) -> Result<Cow<'a, [u8]>, ImageError> {
    if extents.is_empty() { return Err(invalid("item has no iloc extents")); }
    let slice_of = |ext: &Extent| -> Result<&'a [u8], ImageError> {
        for m in mdats {
            let Some(rel) = ext.start.checked_sub(m.offset) else { continue };
            let Ok(rel) = usize::try_from(rel) else { continue };
            if rel >= m.data.len() { continue; }
            return match ext.len {
                None => Ok(&m.data[rel..]),
                Some(len) => {
                    let len = usize::try_from(len).map_err(|_| invalid("extent exceeds mdat"))?;
                    let end = rel.checked_add(len).filter(|&end| end <= m.data.len())
                        .ok_or_else(|| invalid("extent crosses mdat boundary"))?;
                    Ok(&m.data[rel..end])
                }
            };
        }
        Err(invalid("iloc extent is not in mdat"))
    };
    if extents.len() == 1 {
        return Ok(Cow::Borrowed(slice_of(&extents[0])?));
    }
    let mut out = Vec::new();
    for ext in extents {
        let chunk = slice_of(ext)?;
        if out.len().saturating_add(chunk.len()) > max_len {
            return Err(too_large());
        }
        out.extend_from_slice(chunk);
    }
    Ok(Cow::Owned(out))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AdaptiveBool {
    Off,
    On,
    Adaptive,
}

struct SeqHdr {
    reduced: bool,
    decoder_model_info_present: bool,
    equal_picture_interval: bool,
    buffer_removal_delay_length: u8,
    frame_presentation_delay_length: u8,
    operating_points: Vec<(u16, bool)>,
    width_bits: u8,
    height_bits: u8,
    max_width: u32,
    max_height: u32,
    frame_id_numbers_present: bool,
    delta_frame_id_bits: u8,
    frame_id_bits: u8,
    order_hint: bool,
    order_hint_bits: u8,
    screen_content_tools: AdaptiveBool,
    force_integer_mv: AdaptiveBool,
    high_bit_depth: bool,
    monochrome: bool,
}

fn check_av1_bitstream(mut data: &[u8], is_alpha: bool, has_alpha: bool, limits: ImageLimits) -> Result<(), ImageError> {
    let mut seq: Option<(SeqHdr, u64)> = None;
    while !data.is_empty() {
        let b0 = read_u8(&mut data)?;
        if b0 & 0x80 != 0 { return Err(invalid("invalid AV1 OBU forbidden bit")); }
        let obu_type = (b0 >> 3) & 0x0f;
        let has_extension = b0 & 0x04 != 0;
        let has_size = b0 & 0x02 != 0;
        let (temporal_id, spatial_id) = if has_extension {
            let ext = read_u8(&mut data)?;
            ((ext >> 5) & 0x07, (ext >> 3) & 0x03)
        } else {
            (0, 0)
        };
        let obu_size = if has_size {
            read_leb128(&mut data)?
        } else {
            data.len()
        };
        let payload = data.get(..obu_size).ok_or_else(|| invalid("truncated AV1 OBU"))?;
        data = &data[obu_size..];

        match obu_type {
            1 => {
                let hdr = parse_seq_hdr(payload)?;
                let sample_bytes = if hdr.high_bit_depth { 2u64 } else { 1 };
                let bpp = if is_alpha {
                    sample_bytes
                } else {
                    let channels = match (hdr.monochrome, has_alpha) {
                        (_, true) => 4u64,
                        (false, false) => 3,
                        (true, false) => 1,
                    };
                    (channels * sample_bytes).max(4)
                };
                check_dimensions(hdr.max_width, hdr.max_height, bpp, limits)?;
                seq = Some((hdr, bpp));
            }
            3 | 6 | 7 => {
                let (hdr, bpp) = seq.as_ref().ok_or_else(|| invalid("AV1 frame before sequence header"))?;
                check_frame_hdr(payload, hdr, temporal_id, spatial_id, *bpp, limits)?;
            }
            _ => {}
        }
    }
    if seq.is_none() { return Err(invalid("missing AV1 sequence header")); }
    Ok(())
}

fn parse_seq_hdr(payload: &[u8]) -> Result<SeqHdr, ImageError> {
    let mut b = Bits::new(payload);
    let profile = b.read_u32(3)? as u8;
    if profile > 2 { return Err(invalid("invalid AV1 profile")); }
    let still_picture = b.read_bit()?;
    let reduced = b.read_bit()?;
    if reduced && !still_picture { return Err(invalid("invalid AV1 reduced still header")); }

    let mut decoder_model_info_present = false;
    let mut equal_picture_interval = false;
    let mut buffer_removal_delay_length = 0u8;
    let mut frame_presentation_delay_length = 0u8;
    let mut operating_points = Vec::new();
    if reduced {
        b.skip(5)?;
    } else {
        let mut buffer_delay_length = 0u8;
        if b.read_bit()? {
            b.skip(64)?;
            equal_picture_interval = b.read_bit()?;
            if equal_picture_interval { let _ = b.read_uvlc()?; }
            decoder_model_info_present = b.read_bit()?;
            if decoder_model_info_present {
                buffer_delay_length = b.read_u32(5)? as u8 + 1;
                b.skip(32)?;
                buffer_removal_delay_length = b.read_u32(5)? as u8 + 1;
                frame_presentation_delay_length = b.read_u32(5)? as u8 + 1;
            }
        }
        let display_model_info_present = b.read_bit()?;
        let op_count = b.read_u32(5)? as usize + 1;
        operating_points.reserve(op_count);
        for _ in 0..op_count {
            let idc = b.read_u32(12)? as u16;
            let level = b.read_u32(5)? as u8;
            if level > 7 { b.skip(1)?; }
            let op_decoder_model = if decoder_model_info_present { b.read_bit()? } else { false };
            if op_decoder_model {
                b.skip(usize::from(buffer_delay_length) * 2 + 1)?;
            }
            if display_model_info_present && b.read_bit()? { b.skip(4)?; }
            operating_points.push((idc, op_decoder_model));
        }
    }

    let width_bits = b.read_u32(4)? as u8 + 1;
    let height_bits = b.read_u32(4)? as u8 + 1;
    let max_width = b.read_u32(width_bits)? + 1;
    let max_height = b.read_u32(height_bits)? + 1;

    let (frame_id_numbers_present, delta_frame_id_bits, frame_id_bits) = if !reduced && b.read_bit()? {
        let delta = b.read_u32(4)? as u8 + 2;
        let id_bits = b.read_u32(3)? as u8 + delta + 1;
        (true, delta, id_bits)
    } else {
        (false, 0, 0)
    };
    b.skip(3)?;
    let (order_hint, order_hint_bits, screen_content_tools, force_integer_mv) = if reduced {
        (false, 0, AdaptiveBool::Adaptive, AdaptiveBool::Adaptive)
    } else {
        b.skip(4)?;
        let order_hint = b.read_bit()?;
        if order_hint { b.skip(2)?; }
        let sct = if b.read_bit()? {
            AdaptiveBool::Adaptive
        } else if b.read_bit()? {
            AdaptiveBool::On
        } else {
            AdaptiveBool::Off
        };
        let fimv = if sct != AdaptiveBool::Off {
            if b.read_bit()? {
                AdaptiveBool::Adaptive
            } else if b.read_bit()? {
                AdaptiveBool::On
            } else {
                AdaptiveBool::Off
            }
        } else {
            AdaptiveBool::Adaptive
        };
        let oh_bits = if order_hint { b.read_u32(3)? as u8 + 1 } else { 0 };
        (order_hint, oh_bits, sct, fimv)
    };
    b.skip(3)?;
    let mut hbd = u8::from(b.read_bit()?);
    if profile == 2 && hbd != 0 { hbd += u8::from(b.read_bit()?); }
    let monochrome = if profile != 1 { b.read_bit()? } else { false };

    Ok(SeqHdr {
        reduced,
        decoder_model_info_present,
        equal_picture_interval,
        buffer_removal_delay_length,
        frame_presentation_delay_length,
        operating_points,
        width_bits,
        height_bits,
        max_width,
        max_height,
        frame_id_numbers_present,
        delta_frame_id_bits,
        frame_id_bits,
        order_hint,
        order_hint_bits,
        screen_content_tools,
        force_integer_mv,
        high_bit_depth: hbd != 0,
        monochrome,
    })
}

/// Checks an AV1 frame header's `frame_size_override` dimensions: rav1d reads `width_bits` /
/// `height_bits` without clamping against the sequence header's `max_width` / `max_height`.
fn check_frame_hdr(payload: &[u8], seq: &SeqHdr, temporal_id: u8, spatial_id: u8, bpp: u64, limits: ImageLimits) -> Result<(), ImageError> {
    if seq.reduced { return Ok(()); }
    let mut b = Bits::new(payload);
    if b.read_bit()? { return Ok(()); }
    let frame_type = b.read_u32(2)? as u8;
    let show_frame = b.read_bit()?;
    if show_frame {
        if seq.decoder_model_info_present && !seq.equal_picture_interval {
            b.skip(usize::from(seq.frame_presentation_delay_length))?;
        }
    } else {
        b.skip(1)?;
    }
    let error_resilient = (frame_type == 0 && show_frame) || frame_type == 3 || b.read_bit()?;
    b.skip(1)?;
    let allow_screen_content = match seq.screen_content_tools {
        AdaptiveBool::Adaptive => b.read_bit()?,
        AdaptiveBool::On => true,
        AdaptiveBool::Off => false,
    };
    if allow_screen_content && seq.force_integer_mv == AdaptiveBool::Adaptive { b.skip(1)?; }
    if seq.frame_id_numbers_present { b.skip(usize::from(seq.frame_id_bits))?; }
    let frame_size_override = if frame_type == 3 { true } else { b.read_bit()? };
    if !frame_size_override { return Ok(()); }

    if seq.order_hint { b.skip(usize::from(seq.order_hint_bits))?; }
    let is_inter_or_switch = frame_type == 1 || frame_type == 3;
    if !error_resilient && is_inter_or_switch { b.skip(3)?; }
    if seq.decoder_model_info_present && b.read_bit()? {
        for &(idc, param_present) in &seq.operating_points {
            if param_present && (idc == 0 || (((idc >> temporal_id) & 1 != 0) && ((idc >> (spatial_id + 8)) & 1 != 0))) {
                b.skip(usize::from(seq.buffer_removal_delay_length))?;
            }
        }
    }
    if !is_inter_or_switch {
        let refresh = if frame_type == 0 && show_frame { 0xff } else { b.read_u32(8)? as u8 };
        if refresh != 0xff && error_resilient && seq.order_hint {
            b.skip(8 * usize::from(seq.order_hint_bits))?;
        }
    } else {
        if frame_type != 3 { b.skip(8)?; }
        if error_resilient && seq.order_hint { b.skip(8 * usize::from(seq.order_hint_bits))?; }
        let short_signaling = seq.order_hint && b.read_bit()?;
        if short_signaling { b.skip(6)?; }
        for _ in 0..7 {
            if !short_signaling { b.skip(3)?; }
            if seq.frame_id_numbers_present { b.skip(usize::from(seq.delta_frame_id_bits))?; }
        }
        if !error_resilient {
            for _ in 0..7 {
                if b.read_bit()? { return Ok(()); }
            }
        }
    }
    let w = b.read_u32(seq.width_bits)? + 1;
    let h = b.read_u32(seq.height_bits)? + 1;
    check_dimensions(w, h, bpp, limits)
}

struct Bits<'a> {
    bytes: &'a [u8],
    bit_pos: usize,
}

impl<'a> Bits<'a> {
    fn new(bytes: &'a [u8]) -> Self { Self { bytes, bit_pos: 0 } }

    fn read_bit(&mut self) -> Result<bool, ImageError> {
        let byte = *self.bytes.get(self.bit_pos / 8).ok_or_else(|| invalid("truncated AV1 header"))?;
        let bit = (byte >> (7 - (self.bit_pos % 8))) & 1 != 0;
        self.bit_pos += 1;
        Ok(bit)
    }

    fn read_u32(&mut self, n: u8) -> Result<u32, ImageError> {
        let mut v = 0u32;
        for _ in 0..n { v = (v << 1) | u32::from(self.read_bit()?); }
        Ok(v)
    }

    fn skip(&mut self, n: usize) -> Result<(), ImageError> {
        let next = self.bit_pos.checked_add(n).ok_or_else(|| invalid("truncated AV1 header"))?;
        if next > self.bytes.len().saturating_mul(8) { return Err(invalid("truncated AV1 header")); }
        self.bit_pos = next;
        Ok(())
    }

    fn read_uvlc(&mut self) -> Result<u32, ImageError> {
        let mut zeros = 0u8;
        while !self.read_bit()? {
            zeros += 1;
            if zeros >= 32 { return Err(invalid("invalid AV1 uvlc")); }
        }
        let val = self.read_u32(zeros)?;
        Ok(val + ((1u32 << zeros) - 1))
    }
}

fn read_leb128(data: &mut &[u8]) -> Result<usize, ImageError> {
    let mut val = 0u64;
    for i in 0..8u32 {
        let b = read_u8(data)?;
        val |= u64::from(b & 0x7f) << (7 * i);
        if b & 0x80 == 0 {
            if val > u64::from(u32::MAX) { return Err(invalid("AV1 OBU size overflow")); }
            return usize::try_from(val).map_err(|_| invalid("AV1 OBU size overflow"));
        }
    }
    Err(invalid("unterminated AV1 LEB128"))
}

fn read_u8(data: &mut &[u8]) -> Result<u8, ImageError> {
    let (&b, rest) = data.split_first().ok_or_else(|| invalid("unexpected EOF"))?;
    *data = rest;
    Ok(b)
}

fn read_u16(data: &mut &[u8]) -> Result<u16, ImageError> {
    let (head, rest) = data.split_at_checked(2).ok_or_else(|| invalid("unexpected EOF"))?;
    *data = rest;
    Ok(u16::from_be_bytes(head.try_into().unwrap()))
}

fn read_u32(data: &mut &[u8]) -> Result<u32, ImageError> {
    let (head, rest) = data.split_at_checked(4).ok_or_else(|| invalid("unexpected EOF"))?;
    *data = rest;
    Ok(u32::from_be_bytes(head.try_into().unwrap()))
}

fn read_var_u64(data: &mut &[u8], bytes: u8) -> Result<u64, ImageError> {
    match bytes {
        0 => Ok(0),
        4 => Ok(u64::from(read_u32(data)?)),
        8 => {
            let (head, rest) = data.split_at_checked(8).ok_or_else(|| invalid("unexpected EOF"))?;
            *data = rest;
            Ok(u64::from_be_bytes(head.try_into().unwrap()))
        }
        _ => Err(invalid("invalid iloc field size")),
    }
}
