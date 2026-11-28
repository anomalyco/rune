//! Installed system font families (`installed_families`), queried lazily once per process
//! through the platform's own font registry (CoreText, DirectWrite, fontconfig) without walking
//! font directories.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::sync::OnceLock;

/// A font family installed on the current system, returned by [`installed_families`].
///
/// Pass [`name`](Self::name) to `AppSpec::families`, `Tx::set_font_families`,
/// [`TextSystem::set_families`](crate::TextSystem::set_families), or `Family::Named`.
///
/// ```
/// if let Some(fam) = rune_text::installed_families().first() {
///     let _ = (fam.name(), fam.monospace(), fam.weights());
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledFamily {
    name: String,
    monospace: bool,
    weights: Vec<u16>,
}

impl InstalledFamily {
    /// The family's display name (for example `"Menlo"`, `"Consolas"`, or `"DejaVu Sans"`).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether the family is monospaced (fixed pitch).
    pub fn monospace(&self) -> bool {
        self.monospace
    }

    /// Available CSS/OpenType weights in `100..=900`, sorted and deduplicated.
    ///
    /// Static faces contribute their weight; variable faces with a `wght` axis contribute every
    /// hundred-step weight (`100, 200, …, 900`) within their axis range.
    pub fn weights(&self) -> &[u16] {
        &self.weights
    }
}

/// Every public font family installed on the current system, sorted case-insensitively by name
/// and deduplicated.
///
/// Populated lazily on first call and cached for the lifetime of the process, so apps that do
/// not call it pay nothing at launch. Queries the platform's own font registry (CoreText on
/// Apple platforms, DirectWrite on Windows, fontconfig on Linux/Unix, and `font_fallback.xml` /
/// `fonts.xml` on Android) without walking font directories; returns an empty slice on `wasm32`.
/// System-private families whose names start with `.` are omitted.
///
/// ```
/// let mono: Vec<&str> = rune_text::installed_families()
///     .iter()
///     .filter(|f| f.monospace())
///     .map(|f| f.name())
///     .collect();
/// let _ = mono;
/// ```
pub fn installed_families() -> &'static [InstalledFamily] {
    static FAMILIES: OnceLock<Vec<InstalledFamily>> = OnceLock::new();
    FAMILIES.get_or_init(|| finalize(super::platform::installed_families()))
}

/// Raw family record collected by a platform backend before filtering, merging, and sorting.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(super) struct RawFamily {
    pub(super) name: String,
    pub(super) monospace: bool,
    pub(super) weights: Vec<u16>,
}

/// Clamps an OpenType/CSS numeric weight into `100..=900`, defaulting non-finite values to `400`.
#[cfg_attr(any(target_os = "android", target_arch = "wasm32", target_vendor = "apple"), allow(dead_code))]
pub(super) fn clamp_weight(weight: f32) -> u16 {
    if !weight.is_finite() {
        return 400;
    }
    weight.round().clamp(100.0, 900.0) as u16
}

/// Adds every hundred-step weight (`100..=900`) covered by a variable `wght` axis range `[min, max]`.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(super) fn push_weight_range(weights: &mut Vec<u16>, min: f32, max: f32) {
    if !min.is_finite() || !max.is_finite() || max <= min {
        return;
    }
    let mut step = 100u16;
    while step <= 900 {
        let w = f32::from(step);
        if w >= min && w <= max {
            weights.push(step);
        }
        step += 100;
    }
}

/// Maps CoreText's normalized `kCTFontWeightTrait` (`-1.0..=1.0`, Apple's `NSFontWeight` scale)
/// to the nearest CSS/OpenType hundred-step weight (`100..=900`).
#[cfg_attr(not(any(target_vendor = "apple", test)), allow(dead_code))]
pub(super) fn apple_weight_trait_to_css(trait_weight: f64) -> u16 {
    if !trait_weight.is_finite() {
        return 400;
    }
    // Apple NSFontWeight constants (AppKit NSFontDescriptor.h).
    const STOPS: [(f64, u16); 9] = [
        (-0.80, 100),
        (-0.60, 200),
        (-0.40, 300),
        (0.00, 400),
        (0.23, 500),
        (0.30, 600),
        (0.40, 700),
        (0.56, 800),
        (0.62, 900),
    ];
    STOPS
        .iter()
        .min_by(|(a, _), (b, _)| (trait_weight - a).abs().total_cmp(&(trait_weight - b).abs()))
        .map_or(400, |&(_, w)| w)
}

/// Filters hidden/blank names, merges case-insensitive duplicates, clamps and deduplicates
/// weights, and sorts case-insensitively by name (with lexicographic tie-breaking).
pub(super) fn finalize(raw: Vec<RawFamily>) -> Vec<InstalledFamily> {
    let mut by_lower: HashMap<String, usize> = HashMap::with_capacity(raw.len());
    let mut entries: Vec<(String, InstalledFamily)> = Vec::with_capacity(raw.len());

    for item in raw {
        let name = item.name.trim();
        if name.is_empty() || name.starts_with('.') {
            continue;
        }
        let lower = name.to_lowercase();
        if let Some(&idx) = by_lower.get(&lower) {
            let existing = &mut entries[idx].1;
            existing.monospace |= item.monospace;
            existing.weights.extend(item.weights.into_iter().map(|w| w.clamp(100, 900)));
        } else {
            let idx = entries.len();
            by_lower.insert(lower.clone(), idx);
            let weights = item.weights.into_iter().map(|w| w.clamp(100, 900)).collect();
            entries.push((
                lower,
                InstalledFamily { name: name.to_owned(), monospace: item.monospace, weights },
            ));
        }
    }

    for (_, family) in &mut entries {
        family.weights.sort_unstable();
        family.weights.dedup();
        if family.weights.is_empty() {
            family.weights.push(400);
        }
    }

    entries.sort_by(|(a_low, a), (b_low, b)| a_low.cmp(b_low).then_with(|| a.name.cmp(&b.name)));
    entries.into_iter().map(|(_, family)| family).collect()
}

/// Reads the raw 16-byte SFNT table directory records for face `index` of an SFNT or TTC stream.
#[cfg_attr(not(any(all(unix, not(target_vendor = "apple")), test)), allow(dead_code))]
fn sfnt_table_records(r: &mut (impl Read + Seek), index: u32) -> Option<Vec<u8>> {
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic).ok()?;
    let face_offset = if &magic == b"ttcf" {
        let mut ttc = [0u8; 8];
        r.read_exact(&mut ttc).ok()?;
        let num_fonts = u32::from_be_bytes([ttc[4], ttc[5], ttc[6], ttc[7]]);
        if index >= num_fonts {
            return None;
        }
        let mut off = [0u8; 4];
        r.seek(SeekFrom::Start(12 + u64::from(index) * 4)).ok()?;
        r.read_exact(&mut off).ok()?;
        u64::from(u32::from_be_bytes(off))
    } else if index == 0 {
        0
    } else {
        return None;
    };

    r.seek(SeekFrom::Start(face_offset)).ok()?;
    let mut header = [0u8; 12];
    r.read_exact(&mut header).ok()?;
    if !matches!(&header[..4], [0, 1, 0, 0] | b"OTTO" | b"true" | b"typ1") {
        return None;
    }
    let num_tables = u16::from_be_bytes([header[4], header[5]]) as usize;
    if num_tables == 0 || num_tables > 1024 {
        return None;
    }
    let mut records = vec![0u8; num_tables * 16];
    r.read_exact(&mut records).ok()?;
    Some(records)
}

#[cfg_attr(not(any(all(unix, not(target_vendor = "apple")), test)), allow(dead_code))]
fn tables_is_monospace(
    r: &mut (impl Read + Seek),
    post_table: Option<(u64, u32)>,
    os2_table: Option<(u64, u32)>,
) -> bool {
    if let Some((offset, length)) = post_table
        && length >= 16
        && let Some(pos) = offset.checked_add(12)
        && r.seek(SeekFrom::Start(pos)).is_ok()
    {
        let mut fixed = [0u8; 4];
        if r.read_exact(&mut fixed).is_ok() && u32::from_be_bytes(fixed) != 0 {
            return true;
        }
    }

    if let Some((offset, length)) = os2_table
        && length >= 42
        && let Some(pos) = offset.checked_add(32)
        && r.seek(SeekFrom::Start(pos)).is_ok()
    {
        let mut panose = [0u8; 10];
        if r.read_exact(&mut panose).is_ok() && panose[0] == 2 && panose[3] == 9 {
            return true;
        }
    }

    false
}

/// Whether face `index` of an SFNT or TrueType collection stream is marked fixed-pitch in its
/// `post` table (`isFixedPitch != 0`) or monospaced in its `OS/2` PANOSE classification
/// (`bFamilyType == 2 && bProportion == 9`). Reads only the table directory and small headers.
#[cfg_attr(not(any(all(unix, not(target_vendor = "apple"), not(target_os = "android")), test)), allow(dead_code))]
pub(super) fn reader_is_monospace(r: &mut (impl Read + Seek), index: u32) -> bool {
    let Some(records) = sfnt_table_records(r, index) else {
        return false;
    };
    let mut post_table = None;
    let mut os2_table = None;
    for rec in records.as_chunks::<16>().0 {
        let offset = u64::from(u32::from_be_bytes([rec[8], rec[9], rec[10], rec[11]]));
        let length = u32::from_be_bytes([rec[12], rec[13], rec[14], rec[15]]);
        match &rec[..4] {
            b"post" => post_table = Some((offset, length)),
            b"OS/2" => os2_table = Some((offset, length)),
            _ => {}
        }
    }
    tables_is_monospace(r, post_table, os2_table)
}

/// Lightweight SFNT metadata read from table headers without mapping the whole font file.
#[cfg(any(target_os = "android", all(test, not(target_arch = "wasm32"))))]
pub(super) struct SfntMeta {
    pub(super) family: String,
    pub(super) monospace: bool,
    pub(super) weight_class: Option<u16>,
    pub(super) wght_axis: Option<(f32, f32)>,
}

/// Reads the SFNT family name (preferring typographic family ID 16 over legacy ID 1), fixed-pitch
/// flag, `OS/2` weight class, and variable `wght` axis range for face `index` by seeking directly
/// to those small tables.
#[cfg(any(target_os = "android", all(test, not(target_arch = "wasm32"))))]
pub(super) fn reader_sfnt_meta(r: &mut (impl Read + Seek), index: u32) -> Option<SfntMeta> {
    const MAX_NAME_TABLE: u32 = 256 * 1024;
    const MAX_FVAR_TABLE: u32 = 64 * 1024;

    let records = sfnt_table_records(r, index)?;
    let (mut name_table, mut post_table, mut os2_table, mut fvar_table) = (None, None, None, None);
    for rec in records.as_chunks::<16>().0 {
        let offset = u64::from(u32::from_be_bytes([rec[8], rec[9], rec[10], rec[11]]));
        let length = u32::from_be_bytes([rec[12], rec[13], rec[14], rec[15]]);
        match &rec[..4] {
            b"name" => name_table = Some((offset, length)),
            b"post" => post_table = Some((offset, length)),
            b"OS/2" => os2_table = Some((offset, length)),
            b"fvar" => fvar_table = Some((offset, length)),
            _ => {}
        }
    }

    let (name_off, name_len) = name_table?;
    if !(6..=MAX_NAME_TABLE).contains(&name_len) {
        return None;
    }
    r.seek(SeekFrom::Start(name_off)).ok()?;
    let mut name_buf = vec![0u8; name_len as usize];
    r.read_exact(&mut name_buf).ok()?;
    let family = parse_sfnt_family_name(&name_buf)?;

    let monospace = tables_is_monospace(r, post_table, os2_table);

    let weight_class = os2_table.and_then(|(offset, length)| {
        if length < 6 {
            return None;
        }
        r.seek(SeekFrom::Start(offset.checked_add(4)?)).ok()?;
        let mut buf = [0u8; 2];
        r.read_exact(&mut buf).ok()?;
        let w = u16::from_be_bytes(buf);
        (1..=1000).contains(&w).then(|| w.clamp(100, 900))
    });

    let wght_axis = fvar_table.and_then(|(offset, length)| {
        if !(16..=MAX_FVAR_TABLE).contains(&length) {
            return None;
        }
        r.seek(SeekFrom::Start(offset)).ok()?;
        let mut fvar_buf = vec![0u8; length as usize];
        r.read_exact(&mut fvar_buf).ok()?;
        parse_fvar_wght_axis(&fvar_buf)
    });

    Some(SfntMeta {
        family,
        monospace,
        weight_class,
        wght_axis,
    })
}

#[cfg(any(target_os = "android", all(test, not(target_arch = "wasm32"))))]
fn parse_sfnt_family_name(table: &[u8]) -> Option<String> {
    if table.len() < 6 {
        return None;
    }
    let count = u16::from_be_bytes([table[2], table[3]]) as usize;
    let string_offset = u16::from_be_bytes([table[4], table[5]]) as usize;
    if string_offset > table.len() {
        return None;
    }
    let storage = &table[string_offset..];
    let mut best: Option<(u8, String)> = None;
    for i in 0..count {
        let base = 6 + i * 12;
        let Some(rec) = table.get(base..base + 12) else {
            break;
        };
        let platform = u16::from_be_bytes([rec[0], rec[1]]);
        let encoding = u16::from_be_bytes([rec[2], rec[3]]);
        let language = u16::from_be_bytes([rec[4], rec[5]]);
        let name_id = u16::from_be_bytes([rec[6], rec[7]]);
        if name_id != 16 && name_id != 1 {
            continue;
        }
        let len = u16::from_be_bytes([rec[8], rec[9]]) as usize;
        let off = u16::from_be_bytes([rec[10], rec[11]]) as usize;
        let Some(raw) = off.checked_add(len).and_then(|end| storage.get(off..end)) else {
            continue;
        };
        let rank_base = if name_id == 16 { 0u8 } else { 4u8 };
        let (rank, decoded) = match platform {
            3 if matches!(encoding, 1 | 10) => {
                let r = rank_base + if language == 0x0409 { 0 } else { 1 };
                (r, decode_utf16_be(raw))
            }
            0 => (rank_base + 1, decode_utf16_be(raw)),
            1 if encoding == 0 => {
                let r = rank_base + if language == 0 { 2 } else { 3 };
                let s: String = raw.iter().filter(|&&b| b != 0).map(|&b| char::from(b)).collect();
                (r, (!s.trim().is_empty()).then(|| s.trim().to_owned()))
            }
            _ => continue,
        };
        let Some(name) = decoded else { continue };
        if best.as_ref().is_none_or(|(best_rank, _)| rank < *best_rank) {
            best = Some((rank, name));
            if rank == 0 {
                break;
            }
        }
    }
    best.map(|(_, name)| name)
}

#[cfg(any(target_os = "android", all(test, not(target_arch = "wasm32"))))]
fn decode_utf16_be(raw: &[u8]) -> Option<String> {
    if raw.len() < 2 || !raw.len().is_multiple_of(2) {
        return None;
    }
    let units = raw.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c));
    let s: String = char::decode_utf16(units).filter_map(Result::ok).filter(|&c| c != ' ').collect();
    let trimmed = s.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

#[cfg(any(target_os = "android", all(test, not(target_arch = "wasm32"))))]
fn parse_fvar_wght_axis(table: &[u8]) -> Option<(f32, f32)> {
    if table.len() < 16 {
        return None;
    }
    let major = u16::from_be_bytes([table[0], table[1]]);
    if major != 1 {
        return None;
    }
    let axes_offset = u16::from_be_bytes([table[4], table[5]]) as usize;
    let axis_count = (u16::from_be_bytes([table[8], table[9]]) as usize).min(64);
    let axis_size = u16::from_be_bytes([table[10], table[11]]) as usize;
    if axis_size < 20 {
        return None;
    }
    for i in 0..axis_count {
        let base = axes_offset.checked_add(i.checked_mul(axis_size)?)?;
        let rec = table.get(base..base + 20)?;
        if &rec[0..4] == b"wght" {
            let min = i32::from_be_bytes([rec[4], rec[5], rec[6], rec[7]]) as f32 / 65536.0;
            let max = i32::from_be_bytes([rec[12], rec[13], rec[14], rec[15]]) as f32 / 65536.0;
            return Some((min, max));
        }
    }
    None
}
