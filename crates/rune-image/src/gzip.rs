//! Bounded streaming gzip (RFC 1952) and DEFLATE (RFC 1951) decompression for SVGZ inputs.
//!
//! Stops inflating as soon as the uncompressed output would exceed `max_bytes` and caps buffer
//! reservations to `max_bytes`, so a compressed bomb cannot allocate past the limit.

use crate::ImageError;

const MAX_BITS: usize = 15;
const CODE_LEN_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145,
    8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13,
];

const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut c = i as u32;
        let mut j = 0;
        while j < 8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xedb8_8320 } else { c >> 1 };
            j += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in bytes {
        crc = CRC_TABLE[((crc ^ u32::from(b)) & 0xff) as usize] ^ (crc >> 8);
    }
    !crc
}

fn malformed() -> ImageError {
    ImageError("SVG: provided data has a malformed GZip content".into())
}

fn too_large() -> ImageError {
    ImageError("SVG is too large".into())
}

/// Decompresses one or more concatenated gzip members into a buffer capped at `max_bytes`.
pub(crate) fn decompress(bytes: &[u8], max_bytes: usize) -> Result<Vec<u8>, ImageError> {
    let init_cap = bytes.len().saturating_mul(2).min(max_bytes).min(64 * 1024);
    let mut out = Vec::with_capacity(init_cap);
    let mut pos = 0usize;
    loop {
        pos = decompress_member(bytes, pos, &mut out, max_bytes)?;
        if pos >= bytes.len() {
            break;
        }
        if !bytes[pos..].starts_with(&[0x1f, 0x8b]) {
            break;
        }
    }
    Ok(out)
}

fn decompress_member(bytes: &[u8], start: usize, out: &mut Vec<u8>, max_bytes: usize) -> Result<usize, ImageError> {
    let head = bytes.get(start..start.checked_add(10).ok_or_else(malformed)?).ok_or_else(malformed)?;
    if head[0] != 0x1f || head[1] != 0x8b || head[2] != 8 || head[3] & 0xe0 != 0 {
        return Err(malformed());
    }
    let flg = head[3];
    let mut pos = start + 10;
    if flg & 0x04 != 0 {
        let len_bytes = bytes.get(pos..pos.checked_add(2).ok_or_else(malformed)?).ok_or_else(malformed)?;
        let xlen = usize::from(u16::from_le_bytes([len_bytes[0], len_bytes[1]]));
        pos = pos.checked_add(2 + xlen).filter(|&p| p <= bytes.len()).ok_or_else(malformed)?;
    }
    if flg & 0x08 != 0 {
        pos = skip_nul(bytes, pos)?;
    }
    if flg & 0x10 != 0 {
        pos = skip_nul(bytes, pos)?;
    }
    if flg & 0x02 != 0 {
        pos = pos.checked_add(2).filter(|&p| p <= bytes.len()).ok_or_else(malformed)?;
    }

    let member_out_start = out.len();
    let mut bits = BitReader { data: bytes, pos, buf: 0, len: 0 };
    loop {
        let bfinal = bits.read_bits(1)?;
        let btype = bits.read_bits(2)?;
        match btype {
            0 => inflate_stored(&mut bits, out, max_bytes)?,
            1 => {
                let (lit, dist) = fixed_tables()?;
                inflate_huffman(&mut bits, &lit, &dist, out, max_bytes)?;
            }
            2 => {
                let (lit, dist) = read_dynamic_tables(&mut bits)?;
                inflate_huffman(&mut bits, &lit, &dist, out, max_bytes)?;
            }
            _ => return Err(malformed()),
        }
        if bfinal != 0 {
            break;
        }
    }
    bits.align_byte();
    let trailer = bytes.get(bits.pos..bits.pos.checked_add(8).ok_or_else(malformed)?).ok_or_else(malformed)?;
    let want_crc = u32::from_le_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
    let want_size = u32::from_le_bytes([trailer[4], trailer[5], trailer[6], trailer[7]]);
    let member_bytes = &out[member_out_start..];
    if crc32(member_bytes) != want_crc || (member_bytes.len() as u32) != want_size {
        return Err(malformed());
    }
    Ok(bits.pos + 8)
}

fn skip_nul(bytes: &[u8], pos: usize) -> Result<usize, ImageError> {
    let rel = bytes.get(pos..).ok_or_else(malformed)?.iter().position(|&b| b == 0).ok_or_else(malformed)?;
    Ok(pos + rel + 1)
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u64,
    len: u32,
}

impl BitReader<'_> {
    #[inline]
    fn read_bit(&mut self) -> Result<u32, ImageError> {
        if self.len == 0 {
            let b = *self.data.get(self.pos).ok_or_else(malformed)?;
            self.pos += 1;
            self.buf = u64::from(b);
            self.len = 8;
        }
        let bit = (self.buf & 1) as u32;
        self.buf >>= 1;
        self.len -= 1;
        Ok(bit)
    }

    #[inline]
    fn read_bits(&mut self, n: u32) -> Result<u32, ImageError> {
        if n == 0 {
            return Ok(0);
        }
        while self.len < n {
            let b = *self.data.get(self.pos).ok_or_else(malformed)?;
            self.pos += 1;
            self.buf |= u64::from(b) << self.len;
            self.len += 8;
        }
        let val = (self.buf & ((1u64 << n) - 1)) as u32;
        self.buf >>= n;
        self.len -= n;
        Ok(val)
    }

    fn align_byte(&mut self) {
        let discard = self.len % 8;
        self.buf >>= discard;
        self.len -= discard;
        let whole = (self.len / 8) as usize;
        self.pos -= whole;
        self.buf = 0;
        self.len = 0;
    }
}

struct Huffman {
    counts: [u16; MAX_BITS + 1],
    symbols: Vec<u16>,
    max_bits: usize,
}

impl Huffman {
    fn build(lengths: &[u8], max_bits: usize) -> Result<Self, ImageError> {
        let mut counts = [0u16; MAX_BITS + 1];
        let mut top = 0usize;
        for &len in lengths {
            let len = usize::from(len);
            if len > max_bits {
                return Err(malformed());
            }
            if len > 0 {
                counts[len] += 1;
                top = top.max(len);
            }
        }
        // Reject oversubscribed trees; incomplete 0- or 1-symbol trees are valid for distance tables.
        let mut left = 1i32;
        for &c in &counts[1..=max_bits] {
            left = (left << 1) - i32::from(c);
            if left < 0 {
                return Err(malformed());
            }
        }
        let mut offsets = [0usize; MAX_BITS + 1];
        let mut total = 0usize;
        for len in 1..=top {
            offsets[len] = total;
            total += usize::from(counts[len]);
        }
        let mut symbols = vec![0u16; total];
        for (sym, &len) in lengths.iter().enumerate() {
            let len = usize::from(len);
            if len > 0 {
                symbols[offsets[len]] = sym as u16;
                offsets[len] += 1;
            }
        }
        Ok(Self { counts, symbols, max_bits: top })
    }

    #[inline]
    fn decode(&self, bits: &mut BitReader<'_>) -> Result<u16, ImageError> {
        if self.max_bits == 0 {
            return Err(malformed());
        }
        let mut code = 0i32;
        let mut first = 0i32;
        let mut index = 0usize;
        for len in 1..=self.max_bits {
            code |= bits.read_bit()? as i32;
            let count = i32::from(self.counts[len]);
            if code - count < first {
                return Ok(self.symbols[index + (code - first) as usize]);
            }
            index += count as usize;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(malformed())
    }
}

fn fixed_tables() -> Result<(Huffman, Huffman), ImageError> {
    let mut lit_lens = [0u8; 288];
    lit_lens[..144].fill(8);
    lit_lens[144..256].fill(9);
    lit_lens[256..280].fill(7);
    lit_lens[280..288].fill(8);
    let dist_lens = [5u8; 32];
    Ok((Huffman::build(&lit_lens, MAX_BITS)?, Huffman::build(&dist_lens, MAX_BITS)?))
}

fn read_dynamic_tables(bits: &mut BitReader<'_>) -> Result<(Huffman, Huffman), ImageError> {
    let hlit = bits.read_bits(5)? as usize + 257;
    let hdist = bits.read_bits(5)? as usize + 1;
    let hclen = bits.read_bits(4)? as usize + 4;
    if hlit > 286 || hdist > 32 {
        return Err(malformed());
    }
    let mut cl_lens = [0u8; 19];
    for &slot in &CODE_LEN_ORDER[..hclen] {
        cl_lens[slot] = bits.read_bits(3)? as u8;
    }
    let cl_table = Huffman::build(&cl_lens, 7)?;
    let total = hlit + hdist;
    let mut lens = Vec::with_capacity(total);
    while lens.len() < total {
        let sym = cl_table.decode(bits)?;
        match sym {
            0..=15 => lens.push(sym as u8),
            16 => {
                let prev = *lens.last().ok_or_else(malformed)?;
                let repeat = bits.read_bits(2)? as usize + 3;
                if lens.len() + repeat > total {
                    return Err(malformed());
                }
                lens.resize(lens.len() + repeat, prev);
            }
            17 => {
                let repeat = bits.read_bits(3)? as usize + 3;
                if lens.len() + repeat > total {
                    return Err(malformed());
                }
                lens.resize(lens.len() + repeat, 0);
            }
            18 => {
                let repeat = bits.read_bits(7)? as usize + 11;
                if lens.len() + repeat > total {
                    return Err(malformed());
                }
                lens.resize(lens.len() + repeat, 0);
            }
            _ => return Err(malformed()),
        }
    }
    if lens[256] == 0 {
        return Err(malformed());
    }
    let lit = Huffman::build(&lens[..hlit], MAX_BITS)?;
    let dist = Huffman::build(&lens[hlit..], MAX_BITS)?;
    Ok((lit, dist))
}

/// Ensures `out` can hold `add` more bytes without ever reserving past `max_bytes`.
#[inline]
fn reserve_capped(out: &mut Vec<u8>, add: usize, max_bytes: usize) -> Result<(), ImageError> {
    let need = out.len().checked_add(add).filter(|&n| n <= max_bytes).ok_or_else(too_large)?;
    if need > out.capacity() {
        let target = out.capacity().saturating_mul(2).max(need).max(4096).min(max_bytes);
        out.reserve_exact(target - out.len());
    }
    Ok(())
}

fn inflate_stored(bits: &mut BitReader<'_>, out: &mut Vec<u8>, max_bytes: usize) -> Result<(), ImageError> {
    bits.align_byte();
    let head = bits.data.get(bits.pos..bits.pos.checked_add(4).ok_or_else(malformed)?).ok_or_else(malformed)?;
    let len = u16::from_le_bytes([head[0], head[1]]);
    let nlen = u16::from_le_bytes([head[2], head[3]]);
    if len != !nlen {
        return Err(malformed());
    }
    bits.pos += 4;
    let len = usize::from(len);
    let src = bits.data.get(bits.pos..bits.pos.checked_add(len).ok_or_else(malformed)?).ok_or_else(malformed)?;
    reserve_capped(out, len, max_bytes)?;
    out.extend_from_slice(src);
    bits.pos += len;
    Ok(())
}

fn inflate_huffman(
    bits: &mut BitReader<'_>,
    lit: &Huffman,
    dist_table: &Huffman,
    out: &mut Vec<u8>,
    max_bytes: usize,
) -> Result<(), ImageError> {
    loop {
        let sym = lit.decode(bits)?;
        if sym < 256 {
            reserve_capped(out, 1, max_bytes)?;
            out.push(sym as u8);
        } else if sym == 256 {
            return Ok(());
        } else if sym <= 285 {
            let idx = usize::from(sym - 257);
            let len = usize::from(LEN_BASE[idx]) + bits.read_bits(u32::from(LEN_EXTRA[idx]))? as usize;
            let dsym = dist_table.decode(bits)?;
            if dsym >= 30 {
                return Err(malformed());
            }
            let didx = usize::from(dsym);
            let dist = usize::from(DIST_BASE[didx]) + bits.read_bits(u32::from(DIST_EXTRA[didx]))? as usize;
            if dist == 0 || dist > out.len() {
                return Err(malformed());
            }
            reserve_capped(out, len, max_bytes)?;
            let start = out.len() - dist;
            if dist >= len {
                out.extend_from_within(start..start + len);
            } else {
                for i in 0..len {
                    let b = out[start + i];
                    out.push(b);
                }
            }
        } else {
            return Err(malformed());
        }
    }
}
