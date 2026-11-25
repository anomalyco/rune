//! Shaper-verified tabular-digit template fast path for single-line `LiveText` updates.
//!
//! When a short single-line ASCII `LiveText` updates only its ASCII digits (`'0'..='9'`) in a
//! tabular-figures (`'tnum' 1`) or monospace style, every value sharing the same digit mask
//! has the exact same line metrics, run geometry, cluster bounds and glyph positions, differing
//! only in the digit glyph IDs.
//!
//! Invariance is proven directly from Parley's own shaping output and the font's OpenType tables
//! before the fast path is ever enabled for a mask:
//! 1. Lazy activation: the first string of a mask is shaped normally; on the second distinct
//!    string of the same mask, if `full_width()` differs, the mask is marked `Ineligible` with
//!    zero extra shaping calls.
//! 2. All 10 uniform digit fills (`'0'..='9'`) are shaped and compared bit-for-bit across
//!    layout metrics, line metrics, run metadata, 1-to-1 cluster boundaries, non-digit glyph IDs
//!    and positions, and digit slot positions/advances.
//! 3. When any adjacent digit slots exist, a 102-byte order-2 De Bruijn sequence over `'0'..='9'`
//!    (covering all 100 ordered digit pairs strictly interior) is shaped to prove zero digit-pair
//!    kerning, ligatures or GPOS adjustments.
//! 4. Each run's OpenType `GSUB`/`GPOS`/`GDEF`/`kern` tables (and absence of AAT `morx`/`mort`/`kerx`)
//!    are checked fail-closed (`tables::verify_run_tables`) to rule out mixed-digit ligatures
//!    (such as `1/2` under `frac`/`dlig`) and multi-glyph contextual rules.

use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use parley::{Alignment, Layout, PositionedLayoutItem};
use rune_core::HashMap;
use rune_core::tok::{Family, OpticalSize, TextStyle};

use crate::atlas::glyph_emit::Face;
use crate::{Ink, Keep, TextSystem};

mod tables;
#[cfg(test)]
mod tests;

/// Maximum byte length eligible for tabular-digit templating.
pub(crate) const MAX_TABULAR_LEN: usize = 64;
/// Maximum cached masks (observed, verified, or ineligible) kept per `TextSystem`.
pub(crate) const MAX_TABULAR_TEMPLATES: usize = 256;
/// Byte budget for cached tabular templates.
const MAX_TABULAR_BYTES: usize = 2 << 20;

/// 102-byte order-2 De Bruijn sequence over ASCII `'0'..='9'` with a leading `'0'` so all 100
/// ordered digit pairs `(a, b)` occur strictly interior (at indices `0..101`, never depending on
/// trailing end-of-line letter-spacing handling at index `101`).
const DE_BRUIJN_DIGITS: &str = "009989796959493929190887868584838281807767574737271706656463626160554535251504434241403323130221201100";

/// Process-wide monotonic value ID for `TabularGlyphs`, starting at 1 (0 is reserved for
/// non-tabular text in paint's copied-quad cache key).
static NEXT_VALUE_ID: AtomicU64 = AtomicU64::new(1);

fn next_value_id() -> u64 {
    NEXT_VALUE_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct OpticalSizeKey(u8, u32);

impl OpticalSizeKey {
    fn from(o: OpticalSize) -> Self {
        match o {
            OpticalSize::Auto => OpticalSizeKey(0, 0),
            OpticalSize::FontDefault => OpticalSizeKey(1, 0),
            OpticalSize::Fixed(v) => OpticalSizeKey(2, v.to_bits()),
        }
    }
}

/// Exact collision-free cache key: digit mask bytes + every field of `TextStyle` + `scale`.
#[derive(Clone, PartialEq, Eq, Hash)]
struct TabularKey {
    mask: [u8; MAX_TABULAR_LEN],
    len: u8,
    size_bits: u32,
    weight_bits: u32,
    family: Family,
    line_height_bits: u32,
    tracking_bits: u32,
    variations: &'static str,
    features: &'static str,
    optical_size: OpticalSizeKey,
    italic: bool,
    scale_bits: u32,
}

impl TabularKey {
    fn from_text(text: &str, style: TextStyle, scale: f32) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.is_empty() || bytes.len() > MAX_TABULAR_LEN || !scale.is_finite() || scale <= 0.0 {
            return None;
        }
        let mut mask = [0u8; MAX_TABULAR_LEN];
        let mut has_digit = false;
        for (i, &b) in bytes.iter().enumerate() {
            // Printable ASCII only: rejects newlines, tabs, control chars, and all non-ASCII
            // (including RTL scripts and combining marks).
            if !(0x20..=0x7e).contains(&b) {
                return None;
            }
            if b.is_ascii_digit() {
                mask[i] = b'0';
                has_digit = true;
            } else {
                mask[i] = b;
            }
        }
        if !has_digit {
            return None;
        }
        Some(TabularKey {
            mask,
            len: bytes.len() as u8,
            size_bits: style.size.to_bits(),
            weight_bits: style.weight.to_bits(),
            family: style.family,
            line_height_bits: style.line_height.to_bits(),
            tracking_bits: style.tracking.to_bits(),
            variations: style.variations,
            features: style.features,
            optical_size: OpticalSizeKey::from(style.optical_size),
            italic: style.italic,
            scale_bits: scale.to_bits(),
        })
    }

    fn mask_bytes(&self) -> &[u8] {
        &self.mask[..self.len as usize]
    }
}

pub(crate) struct TabularFace {
    pub(crate) face: Face,
    pub(crate) coords: Vec<i16>,
}

pub(crate) struct TabularSlot {
    pub(crate) face: u16,
    /// Fixed glyph ID for non-digit slots, or digit `'0'`'s glyph ID for digit slots.
    pub(crate) glyph_id: u32,
    /// Cumulative run pen offset before adding `x` (matches `glyphs_in_band`'s `rel` to the bit).
    pub(crate) rel: f32,
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) byte: u32,
    /// Index into `TabularTemplate::digit_glyphs`, or `u8::MAX` for non-digit slots.
    pub(crate) digit_slot: u8,
}

pub(crate) struct TabularTemplate {
    /// Verified `'0'`-fill layout, shared as `node.text_layout = Some(TextLayout::Single(...))`.
    pub(crate) layout: Rc<Layout<Ink>>,
    pub(crate) block_min: f32,
    pub(crate) block_max: f32,
    pub(crate) faces: Vec<TabularFace>,
    pub(crate) slots: Vec<TabularSlot>,
    pub(crate) digit_glyphs: Vec<[u32; 10]>,
    pub(crate) digit_positions: Vec<u8>,
    bytes: usize,
}

impl TabularTemplate {
    fn instantiate(self: &Rc<Self>, text: &str) -> TabularGlyphs {
        let mut digits = [0u8; MAX_TABULAR_LEN];
        fill_digits(&mut digits, &self.digit_positions, text.as_bytes());
        TabularGlyphs { template: self.clone(), digits, value_id: next_value_id() }
    }
}

fn fill_digits(dst: &mut [u8; MAX_TABULAR_LEN], positions: &[u8], bytes: &[u8]) {
    for (slot, &pos) in positions.iter().enumerate() {
        dst[slot] = bytes[pos as usize] - b'0';
    }
}

/// Verified tabular-digit glyph substitution over a shared template layout.
#[doc(hidden)]
#[derive(Clone)]
pub struct TabularGlyphs {
    pub(crate) template: Rc<TabularTemplate>,
    pub(crate) digits: [u8; MAX_TABULAR_LEN],
    value_id: u64,
}

impl TabularGlyphs {
    /// Monotonic non-zero ID distinguishing this digit sequence from every other value sharing
    /// the same template layout, for folding into paint's copied-quad cache key.
    #[inline]
    pub fn value_id(&self) -> u64 {
        self.value_id
    }

    /// The shared template layout whose metrics and cluster positions match this value bit-for-bit.
    #[inline]
    pub fn layout(&self) -> &Rc<Layout<Ink>> {
        &self.template.layout
    }

    /// Updates `self` in place when the incoming value uses the same verified template, avoiding
    /// a fresh `Rc` allocation on every live tick.
    fn update_if_same_template(&mut self, template: &Rc<TabularTemplate>, bytes: &[u8]) -> bool {
        if !Rc::ptr_eq(&self.template, template) {
            return false;
        }
        fill_digits(&mut self.digits, &template.digit_positions, bytes);
        self.value_id = next_value_id();
        true
    }
}

enum EntryState {
    Observed {
        first_width_bits: u32,
        first_bytes: [u8; MAX_TABULAR_LEN],
        len: u8,
    },
    Verified(Rc<TabularTemplate>),
    Ineligible,
}

impl EntryState {
    fn bytes(&self) -> usize {
        size_of::<TabularKey>()
            + size_of::<CacheEntry>()
            + match self {
                EntryState::Verified(t) => t.bytes,
                EntryState::Observed { .. } | EntryState::Ineligible => 0,
            }
    }
}

struct CacheEntry {
    state: EntryState,
    used: u64,
}

/// Bounded cache of tabular-digit templates, cleared on font/language changes (`drop_shaped`).
#[derive(Default)]
pub(crate) struct TabularCache {
    entries: HashMap<TabularKey, CacheEntry>,
    bytes: usize,
    tick: u64,
}

impl TabularCache {
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    fn bump_tick(&mut self) -> u64 {
        self.tick = self.tick.wrapping_add(1);
        self.tick
    }

    fn insert(&mut self, key: TabularKey, state: EntryState) {
        let added = state.bytes();
        let tick = self.bump_tick();
        if let Some(old) = self.entries.insert(key, CacheEntry { state, used: tick }) {
            self.bytes = self.bytes.saturating_sub(old.state.bytes());
        }
        self.bytes = self.bytes.saturating_add(added);
        self.evict_if_over();
    }

    fn evict_if_over(&mut self) {
        while self.entries.len() > MAX_TABULAR_TEMPLATES || self.bytes > MAX_TABULAR_BYTES {
            let Some(oldest) = self.entries.iter().min_by_key(|(_, e)| e.used).map(|(k, _)| k.clone()) else {
                break;
            };
            if let Some(removed) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(removed.state.bytes());
            }
        }
    }
}

/// Snapshot of one shaped single-line layout extracted from Parley for bit-exact verification.
struct ProbeSnapshot {
    full_width_bits: u32,
    width_bits: u32,
    height_bits: u32,
    line_metrics: [u32; 12],
    block_min: f32,
    block_max: f32,
    runs: Vec<RunMeta>,
    faces: Vec<TabularFace>,
    glyphs: Vec<GlyphSnap>,
    clusters: Vec<ClusterSnap>,
}

#[derive(PartialEq, Eq)]
struct RunMeta {
    text_start: usize,
    text_end: usize,
    font_id: u64,
    font_index: u32,
    font_size_bits: u32,
    synthesis_skew_bits: Option<u32>,
    synthesis_embolden: bool,
    coords: Vec<i16>,
    offset_bits: u32,
    baseline_bits: u32,
    brush: u32,
}

#[derive(Clone, Copy)]
struct GlyphSnap {
    face: u16,
    id: u32,
    rel: f32,
    x: f32,
    y: f32,
    advance: f32,
    byte: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ClusterSnap {
    start: u32,
    end: u32,
    advance_bits: u32,
    visual_offset_bits: Option<u32>,
    word_boundary: bool,
    space_or_nbsp: bool,
}

fn line_metrics_bits(m: &parley::LineMetrics) -> [u32; 12] {
    let parley::LineMetrics {
        line_height,
        baseline,
        offset,
        indent,
        advance,
        hanging_advance,
        inline_min_coord,
        inline_max_coord,
        block_min_coord,
        block_max_coord,
        content_block_min_coord,
        content_block_max_coord,
    } = *m;
    [
        line_height,
        baseline,
        offset,
        indent,
        advance,
        hanging_advance,
        inline_min_coord,
        inline_max_coord,
        block_min_coord,
        block_max_coord,
        content_block_min_coord,
        content_block_max_coord,
    ]
    .map(f32::to_bits)
}

fn extract_snapshot(layout: &Layout<Ink>, expected_len: usize) -> Option<ProbeSnapshot> {
    if layout.len() != 1 || layout.is_rtl() || layout.inline_boxes().next().is_some() {
        return None;
    }
    let line = layout.get(0)?;
    if line.text_range() != (0..expected_len) {
        return None;
    }
    let m = line.metrics();
    let line_metrics = line_metrics_bits(m);
    let mut runs = Vec::new();
    let mut faces = Vec::new();
    let mut glyphs = Vec::with_capacity(expected_len);
    let mut clusters = Vec::with_capacity(expected_len);
    let mut expected_byte = 0usize;

    for item in line.items() {
        let PositionedLayoutItem::GlyphRun(gr) = item else { return None };
        let run = gr.run();
        if run.is_rtl() || crate::is_hidden_size(run.font_size()) {
            return None;
        }
        let mut coords = Vec::new();
        let face = Face::of(&gr, &mut coords);
        let synthesis = run.synthesis();
        let meta = RunMeta {
            text_start: run.text_range().start,
            text_end: run.text_range().end,
            font_id: face.font.data.id(),
            font_index: face.font.index,
            font_size_bits: face.size.to_bits(),
            synthesis_skew_bits: synthesis.skew().map(f32::to_bits),
            synthesis_embolden: synthesis.embolden(),
            coords: coords.clone(),
            offset_bits: gr.offset().to_bits(),
            baseline_bits: gr.baseline().to_bits(),
            brush: gr.style().brush.0,
        };
        let face_idx = u16::try_from(faces.len()).ok()?;
        runs.push(meta);
        faces.push(TabularFace { face, coords });

        // Verify strict 1-to-1 LTR cluster-to-glyph correspondence for every byte in this run.
        let mut cluster_count = 0usize;
        for cl in run.visual_clusters() {
            if cl.is_rtl() || cl.is_hard_line_break() {
                return None;
            }
            let r = cl.text_range();
            if r.start != expected_byte || r.end != expected_byte + 1 {
                return None;
            }
            if cl.glyphs().count() != 1 {
                return None;
            }
            clusters.push(ClusterSnap {
                start: r.start as u32,
                end: r.end as u32,
                advance_bits: cl.advance().to_bits(),
                visual_offset_bits: cl.visual_offset().map(f32::to_bits),
                word_boundary: cl.is_word_boundary(),
                space_or_nbsp: cl.is_space_or_nbsp(),
            });
            expected_byte += 1;
            cluster_count += 1;
        }

        let mut rel = gr.offset();
        let mut glyph_count = 0usize;
        let run_start_byte = expected_byte - cluster_count;
        for g in gr.glyphs() {
            if glyph_count >= cluster_count {
                return None;
            }
            glyphs.push(GlyphSnap {
                face: face_idx,
                id: g.id,
                rel,
                x: g.x,
                y: g.y,
                advance: g.advance,
                byte: (run_start_byte + glyph_count) as u32,
            });
            rel += g.advance;
            glyph_count += 1;
        }
        if glyph_count != cluster_count {
            return None;
        }
    }

    if expected_byte != expected_len || glyphs.len() != expected_len || clusters.len() != expected_len {
        return None;
    }

    Some(ProbeSnapshot {
        full_width_bits: layout.full_width().to_bits(),
        width_bits: layout.width().to_bits(),
        height_bits: layout.height().to_bits(),
        line_metrics,
        block_min: m.block_min_coord,
        block_max: m.block_max_coord,
        runs,
        faces,
        glyphs,
        clusters,
    })
}

impl TextSystem {
    /// Shapes a single-line `LiveText` string, using a shaper-verified tabular-digit template
    /// when proven bit-identical to a full reshape. Returns the layout to store in
    /// `node.text_layout` and, on a verified hit, the `TabularGlyphs` for paint emission.
    #[doc(hidden)]
    pub fn layout_live_tabular(
        &mut self,
        text: &str,
        style: TextStyle,
        scale: f32,
    ) -> (Rc<Layout<Ink>>, Option<Rc<TabularGlyphs>>) {
        let mut slot = None;
        let layout = self.layout_live_tabular_into(text, style, scale, &mut slot);
        (layout, slot)
    }

    /// Like [`Self::layout_live_tabular`], updating `slot` in place when the previous value
    /// already holds a unique `Rc<TabularGlyphs>` on the same template so per-tick updates
    /// allocate nothing.
    #[doc(hidden)]
    pub fn layout_live_tabular_into(
        &mut self,
        text: &str,
        style: TextStyle,
        scale: f32,
        slot: &mut Option<Rc<TabularGlyphs>>,
    ) -> Rc<Layout<Ink>> {
        self.follow_language();
        let Some(key) = TabularKey::from_text(text, style, scale) else {
            *slot = None;
            return self.layout_live(text, style, scale);
        };

        let tick = self.tabular.bump_tick();
        enum Action {
            First,
            SameAsFirst,
            WidthCheck { first_width_bits: u32 },
            Hit(Rc<TabularTemplate>),
            Skip,
        }
        let action = match self.tabular.entries.get_mut(&key) {
            None => Action::First,
            Some(entry) => {
                entry.used = tick;
                match &entry.state {
                    EntryState::Verified(tpl) => Action::Hit(tpl.clone()),
                    EntryState::Ineligible => Action::Skip,
                    EntryState::Observed { first_width_bits, first_bytes, len } => {
                        if text.as_bytes() == &first_bytes[..*len as usize] {
                            Action::SameAsFirst
                        } else {
                            Action::WidthCheck { first_width_bits: *first_width_bits }
                        }
                    }
                }
            }
        };

        match action {
            Action::Hit(tpl) => {
                let updated = slot
                    .as_mut()
                    .and_then(Rc::get_mut)
                    .is_some_and(|cur| cur.update_if_same_template(&tpl, text.as_bytes()));
                if !updated {
                    *slot = Some(Rc::new(tpl.instantiate(text)));
                }
                tpl.layout.clone()
            }
            Action::Skip | Action::SameAsFirst => {
                *slot = None;
                self.layout_live(text, style, scale)
            }
            Action::First => {
                *slot = None;
                let layout = self.layout_live(text, style, scale);
                let mut first_bytes = [0u8; MAX_TABULAR_LEN];
                let bytes = text.as_bytes();
                first_bytes[..bytes.len()].copy_from_slice(bytes);
                self.tabular.insert(
                    key,
                    EntryState::Observed {
                        first_width_bits: layout.full_width().to_bits(),
                        first_bytes,
                        len: bytes.len() as u8,
                    },
                );
                layout
            }
            Action::WidthCheck { first_width_bits } => {
                let layout = self.layout_live(text, style, scale);
                if layout.full_width().to_bits() != first_width_bits {
                    self.tabular.insert(key, EntryState::Ineligible);
                    *slot = None;
                    return layout;
                }
                match self.verify_tabular_mask(key.mask_bytes(), style, scale) {
                    Some(tpl) => {
                        self.tabular.insert(key, EntryState::Verified(tpl.clone()));
                        let tab = tpl.instantiate(text);
                        *slot = Some(Rc::new(tab));
                        tpl.layout.clone()
                    }
                    None => {
                        self.tabular.insert(key, EntryState::Ineligible);
                        *slot = None;
                        layout
                    }
                }
            }
        }
    }

    /// Shapes `text` for probing without polluting `self.layouts` or `self.shapes`.
    fn shape_probe(&mut self, text: &str, style: TextStyle, scale: f32) -> Rc<Layout<Ink>> {
        self.layout_keyed(
            None,
            text,
            &[],
            &[],
            0.0,
            &[],
            &[],
            &[],
            style,
            scale,
            None,
            Alignment::Start,
            false,
            false,
            Keep::Owned,
            parley::BaseDirection::Auto,
        )
    }

    /// Proves tabular-digit invariance for `mask` in `style` at `scale` directly from Parley's
    /// output and the font's OpenType tables. Returns `None` on any discrepancy.
    fn verify_tabular_mask(
        &mut self,
        mask: &[u8],
        style: TextStyle,
        scale: f32,
    ) -> Option<Rc<TabularTemplate>> {
        let len = mask.len();
        let mut digit_positions = Vec::new();
        let mut byte_to_digit_slot = [u8::MAX; MAX_TABULAR_LEN];
        for (i, &b) in mask.iter().enumerate() {
            if b.is_ascii_digit() {
                byte_to_digit_slot[i] = digit_positions.len() as u8;
                digit_positions.push(i as u8);
            }
        }
        if digit_positions.is_empty() {
            return None;
        }

        // 1. Shape all 10 uniform fills ('0'..='9') and verify bit-identical geometry.
        let mut buf = [0u8; MAX_TABULAR_LEN];
        buf[..len].copy_from_slice(mask);

        let template_str = std::str::from_utf8(&buf[..len]).ok()?;
        let template_layout = self.shape_probe(template_str, style, scale);
        let base = extract_snapshot(&template_layout, len)?;

        let mut digit_glyphs = vec![[0u32; 10]; digit_positions.len()];
        for (ds, &pos) in digit_positions.iter().enumerate() {
            digit_glyphs[ds][0] = base.glyphs[pos as usize].id;
        }

        for d in 1..10u8 {
            let ch = b'0' + d;
            for &pos in &digit_positions {
                buf[pos as usize] = ch;
            }
            let s = std::str::from_utf8(&buf[..len]).ok()?;
            let laid = self.shape_probe(s, style, scale);
            let snap = extract_snapshot(&laid, len)?;
            if snap.full_width_bits != base.full_width_bits
                || snap.width_bits != base.width_bits
                || snap.height_bits != base.height_bits
                || snap.line_metrics != base.line_metrics
                || snap.runs != base.runs
                || snap.clusters != base.clusters
            {
                return None;
            }
            for ((&a, &b), &ds) in base.glyphs.iter().zip(&snap.glyphs).zip(&byte_to_digit_slot[..len]) {
                if a.face != b.face
                    || a.rel.to_bits() != b.rel.to_bits()
                    || a.x.to_bits() != b.x.to_bits()
                    || a.y.to_bits() != b.y.to_bits()
                    || a.advance.to_bits() != b.advance.to_bits()
                    || a.byte != b.byte
                {
                    return None;
                }
                if ds == u8::MAX {
                    if a.id != b.id {
                        return None;
                    }
                } else {
                    digit_glyphs[ds as usize][d as usize] = b.id;
                }
            }
        }

        // 2. If any two digit slots are adjacent, shape the 102-byte order-2 De Bruijn sequence
        // covering all 100 ordered digit pairs (a, b) strictly interior to prove zero digit-pair
        // kerning, ligatures, or pairwise GPOS adjustments.
        let has_adjacent_digits = digit_positions.windows(2).any(|w| w[1] == w[0] + 1);
        if has_adjacent_digits {
            let first_adj_pos = digit_positions.windows(2).find(|w| w[1] == w[0] + 1)?[0] as usize;
            let ref_ds = byte_to_digit_slot[first_adj_pos] as usize;
            let ref_g = base.glyphs[first_adj_pos];
            let ref_run = &base.runs[ref_g.face as usize];
            let ref_digits = digit_glyphs[ref_ds];

            for w in digit_positions.windows(2) {
                if w[1] != w[0] + 1 {
                    continue;
                }
                let (p0, p1) = (w[0] as usize, w[1] as usize);
                let (g0, g1) = (base.glyphs[p0], base.glyphs[p1]);
                let (ds0, ds1) = (byte_to_digit_slot[p0] as usize, byte_to_digit_slot[p1] as usize);
                if g0.face != ref_g.face
                    || g1.face != ref_g.face
                    || digit_glyphs[ds0] != ref_digits
                    || digit_glyphs[ds1] != ref_digits
                    || g0.advance.to_bits() != ref_g.advance.to_bits()
                    || g0.x.to_bits() != ref_g.x.to_bits()
                    || g0.y.to_bits() != ref_g.y.to_bits()
                    || g1.x.to_bits() != ref_g.x.to_bits()
                    || g1.y.to_bits() != ref_g.y.to_bits()
                {
                    return None;
                }
                if p1 + 1 < len && g1.advance.to_bits() != ref_g.advance.to_bits() {
                    return None;
                }
            }

            let db_layout = self.shape_probe(DE_BRUIJN_DIGITS, style, scale);
            let db = extract_snapshot(&db_layout, DE_BRUIJN_DIGITS.len())?;
            if db.runs.len() != 1 {
                return None;
            }
            let db_run = &db.runs[0];
            if db_run.font_id != ref_run.font_id
                || db_run.font_index != ref_run.font_index
                || db_run.font_size_bits != ref_run.font_size_bits
                || db_run.synthesis_skew_bits != ref_run.synthesis_skew_bits
                || db_run.synthesis_embolden != ref_run.synthesis_embolden
                || db_run.coords != ref_run.coords
                || db_run.brush != ref_run.brush
            {
                return None;
            }
            let db_bytes = DE_BRUIJN_DIGITS.as_bytes();
            let last_idx = db_bytes.len() - 1;
            for (k, &b) in db_bytes.iter().enumerate() {
                let d = (b - b'0') as usize;
                let g = db.glyphs[k];
                if g.id != ref_digits[d]
                    || g.x.to_bits() != ref_g.x.to_bits()
                    || g.y.to_bits() != ref_g.y.to_bits()
                {
                    return None;
                }
                // All 100 ordered pairs (a, b) occur strictly inside indices 0..last_idx.
                if k < last_idx && g.advance.to_bits() != ref_g.advance.to_bits() {
                    return None;
                }
            }
        }

        // 3. Fail-closed OpenType table check for every run that contains at least one digit slot.
        for (ri, run) in base.runs.iter().enumerate() {
            let range = run.text_start..run.text_end;
            if !range.clone().any(|p| byte_to_digit_slot[p] != u8::MAX) {
                continue;
            }
            let mut run_digit_gids = Vec::new();
            let mut run_fixed_gids = Vec::new();
            for p in range.clone() {
                let ds = byte_to_digit_slot[p];
                if ds != u8::MAX {
                    run_digit_gids.push(digit_glyphs[ds as usize]);
                } else {
                    run_fixed_gids.push((p - range.start, base.glyphs[p].id));
                }
            }
            let face = &base.faces[ri].face;
            if !tables::verify_run_tables(
                face.font.data.as_ref(),
                face.font.index,
                &mask[range],
                style,
                &run_digit_gids,
                &run_fixed_gids,
            ) {
                return None;
            }
        }

        let slots: Vec<TabularSlot> = base
            .glyphs
            .into_iter()
            .enumerate()
            .map(|(i, g)| TabularSlot {
                face: g.face,
                glyph_id: g.id,
                rel: g.rel,
                x: g.x,
                y: g.y,
                byte: g.byte,
                digit_slot: byte_to_digit_slot[i],
            })
            .collect();

        let bytes = size_of::<TabularTemplate>()
            + 2048
            + len * 64
            + base.faces.iter().map(|f| size_of::<TabularFace>() + f.coords.len() * size_of::<i16>()).sum::<usize>()
            + slots.len() * size_of::<TabularSlot>()
            + digit_glyphs.len() * size_of::<[u32; 10]>()
            + digit_positions.len();

        Some(Rc::new(TabularTemplate {
            layout: template_layout,
            block_min: base.block_min,
            block_max: base.block_max,
            faces: base.faces,
            slots,
            digit_glyphs,
            digit_positions,
            bytes,
        }))
    }

    #[doc(hidden)]
    pub fn tabular_template_count(&self) -> usize {
        self.tabular.len()
    }

    #[doc(hidden)]
    pub fn tabular_template_bytes(&self) -> usize {
        self.tabular.bytes()
    }
}
