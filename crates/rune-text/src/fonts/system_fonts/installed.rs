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
/// Apple platforms, DirectWrite on Windows, fontconfig on Linux/Unix) without walking font
/// directories; returns an empty slice on `wasm32` and Android. System-private families whose
/// names start with `.` are omitted.
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
#[cfg_attr(any(target_os = "android", target_arch = "wasm32"), allow(dead_code))]
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
#[cfg_attr(any(target_os = "android", target_arch = "wasm32"), allow(dead_code))]
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

/// Whether face `index` of an SFNT or TrueType collection stream is marked fixed-pitch in its
/// `post` table (`isFixedPitch != 0`) or monospaced in its `OS/2` PANOSE classification
/// (`bFamilyType == 2 && bProportion == 9`). Reads only the table directory and small headers.
#[cfg_attr(not(any(all(unix, not(target_vendor = "apple"), not(target_os = "android")), test)), allow(dead_code))]
pub(super) fn reader_is_monospace(r: &mut (impl Read + Seek), index: u32) -> bool {
    let mut magic = [0u8; 4];
    if r.read_exact(&mut magic).is_err() {
        return false;
    }
    let face_offset = if &magic == b"ttcf" {
        let mut ttc = [0u8; 8];
        if r.read_exact(&mut ttc).is_err() {
            return false;
        }
        let num_fonts = u32::from_be_bytes([ttc[4], ttc[5], ttc[6], ttc[7]]);
        if index >= num_fonts {
            return false;
        }
        let mut off = [0u8; 4];
        if r.seek(SeekFrom::Start(12 + u64::from(index) * 4)).is_err() || r.read_exact(&mut off).is_err() {
            return false;
        }
        u64::from(u32::from_be_bytes(off))
    } else if index == 0 {
        0
    } else {
        return false;
    };

    if r.seek(SeekFrom::Start(face_offset)).is_err() {
        return false;
    }
    let mut header = [0u8; 12];
    if r.read_exact(&mut header).is_err() {
        return false;
    }
    if !matches!(&header[..4], [0, 1, 0, 0] | b"OTTO" | b"true" | b"typ1") {
        return false;
    }
    let num_tables = u16::from_be_bytes([header[4], header[5]]) as usize;
    if num_tables == 0 || num_tables > 1024 {
        return false;
    }
    let mut records = vec![0u8; num_tables * 16];
    if r.read_exact(&mut records).is_err() {
        return false;
    }

    let mut post_table = None;
    let mut os2_table = None;
    for rec in records.chunks_exact(16) {
        let offset = u64::from(u32::from_be_bytes([rec[8], rec[9], rec[10], rec[11]]));
        let length = u32::from_be_bytes([rec[12], rec[13], rec[14], rec[15]]);
        match &rec[..4] {
            b"post" => post_table = Some((offset, length)),
            b"OS/2" => os2_table = Some((offset, length)),
            _ => {}
        }
    }

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
