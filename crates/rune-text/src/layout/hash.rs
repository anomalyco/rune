//! The shaped-layout caches' keys. A text is hashed once per commit into its shape key (see
//! [`crate::el::TextSpec`]), and each width it breaks at only mixes that key with the width, so
//! measuring and re-breaking a long paragraph doesn't rehash its bytes.
use std::hash::{Hash, Hasher};

use crate::{InlineBoxRun, PillChip, Span, SpanBoxRun};
use rune_core::tok::{OpticalSize, TextStyle};

/// A fast, non-cryptographic hasher: bytes go through `rustc_hash`'s string hash and each word
/// through a folded multiply, as foldhash and wyhash mix. Keys are 64 bits and never leave the
/// process, so it needs to avoid accidental collisions, not adversarial ones.
#[derive(Default)]
pub(crate) struct KeyHasher(u64);

const SEED: u64 = 0x243f_6a88_85a3_08d3;
const MUL: u64 = 0x9e37_79b9_7f4a_7c15;

impl Hasher for KeyHasher {
    fn write(&mut self, bytes: &[u8]) {
        let mut fx = rustc_hash::FxHasher::default();
        fx.write(bytes);
        self.write_u64(fx.finish() ^ bytes.len() as u64);
    }
    fn write_u8(&mut self, i: u8) { self.write_u64(i as u64) }
    fn write_u16(&mut self, i: u16) { self.write_u64(i as u64) }
    fn write_u32(&mut self, i: u32) { self.write_u64(i as u64) }
    fn write_usize(&mut self, i: usize) { self.write_u64(i as u64) }
    fn write_u64(&mut self, i: u64) {
        let full = ((self.0 ^ i ^ SEED) as u128).wrapping_mul(MUL as u128);
        self.0 = (full as u64) ^ ((full >> 64) as u64);
    }
    fn finish(&self) -> u64 { self.0 }
}

/// A key of `text` in `style` at `scale`, with `width` when given (see [`shape_key`]).
/// Colour-only spans resolve at glyph emission and do not enter the key.
pub fn text_hash(text: &str, spans: &[Span], style: TextStyle, scale: f32, width: Option<f32>) -> u64 {
    let mut h = KeyHasher::default();
    text.hash(&mut h);
    for s in super::span::layout_spans(spans) {
        s.hash_layout(&mut h);
    }
    (style.size.to_bits(), style.weight.to_bits(), style.family, style.line_height.to_bits(), style.tracking.to_bits())
        .hash(&mut h);
    (scale.to_bits(), width.map(f32::to_bits)).hash(&mut h);
    let optical = match style.optical_size {
        OpticalSize::Auto => (0, 0), OpticalSize::FontDefault => (1, 0), OpticalSize::Fixed(size) => (2, size.to_bits()),
    };
    (style.variations, style.features, optical, style.italic).hash(&mut h);
    h.finish()
}

/// What shaping depends on: the text, its spans, style and scale, overflow-wrap and pills.
#[doc(hidden)]
pub fn shape_key(text: &str, spans: &[Span], style: TextStyle, scale: f32, wrap_anywhere: bool, pills: &[PillChip]) -> u64 {
    let key = text_hash(text, spans, style, scale, None);
    if !wrap_anywhere && pills.is_empty() {
        return key;
    }
    let mut h = KeyHasher::default();
    (key, wrap_anywhere).hash(&mut h);
    for p in pills {
        hash_pill_layout(p, &mut h);
    }
    h.finish()
}

/// Hashes every layout-affecting field of a [`PillChip`] into `h`, shared by [`shape_key`] and
/// per-paragraph cache keying so both invalidate identically when a pill's geometry changes.
pub(crate) fn hash_pill_layout(p: &PillChip, h: &mut impl Hasher) {
    let nonneg_bits = |v: f32| (if v.is_finite() && v > 0.0 { v } else { 0.0 }).to_bits();
    let replace = |r: &crate::Replace| [r.width, r.ascent, r.descent].map(f32::to_bits);
    (
        p.start,
        p.end,
        p.has_lead_visual(),
        p.pad_x.map(nonneg_bits),
        p.margin_x.map(nonneg_bits),
        p.bare.as_ref().map(|b| (b.size.to_bits(), b.gap.to_bits(), b.replace.as_ref().map(replace))),
    )
        .hash(h);
}

/// Mixes layout-affecting [`SpanBoxRun`] horizontal padding into a [`shape_key`]. Returns `key`
/// unchanged when no box adds horizontal padding, keeping plain text keys identical.
#[doc(hidden)]
pub fn mix_span_boxes(key: u64, span_boxes: &[SpanBoxRun]) -> u64 {
    if !span_boxes.iter().any(|b| b.style.has_padding()) {
        return key;
    }
    let mut h = KeyHasher::default();
    (key, 0x7370_616e_626f_7801u64).hash(&mut h);
    for b in span_boxes {
        if b.style.has_padding() {
            (b.start, b.end, b.style.pad_left().to_bits(), b.style.pad_right().to_bits()).hash(&mut h);
        }
    }
    h.finish()
}

/// Mixes in-flow [`InlineBoxRun`] slots into a [`shape_key`]. Returns `key` unchanged when
/// `inline_boxes` is empty, keeping plain text keys bit-identical.
#[doc(hidden)]
pub fn mix_inline_boxes(key: u64, inline_boxes: &[InlineBoxRun]) -> u64 {
    if inline_boxes.is_empty() {
        return key;
    }
    let mut h = KeyHasher::default();
    (key, 0x696e_6c69_6e65_6278u64).hash(&mut h);
    for b in inline_boxes {
        (
            b.at,
            b.slot.width().to_bits(),
            b.slot.height().to_bits(),
            b.slot.alignment(),
            b.slot.baseline_offset().map(f32::to_bits),
        )
            .hash(&mut h);
    }
    h.finish()
}

/// [`shape_key`] for a paragraph laid out in `base`: left to right keeps the key, the others
/// salt it, as their bidi levels shape the same text differently.
#[doc(hidden)]
pub fn directed(key: u64, base: parley::BaseDirection) -> u64 {
    match base {
        parley::BaseDirection::Auto => key,
        parley::BaseDirection::Rtl => key ^ 0x6a09_e667_f3bc_c909,
        parley::BaseDirection::Ltr => key ^ 0xbb67_ae85_84ca_a73b,
    }
}

/// A shaping broken at `max_width`, aligned and block- or first-line-indented: the shape key
/// itself when the layout is unwrapped, start-aligned and unindented.
pub(crate) fn break_key(shape: u64, max_width: Option<f32>, align: parley::Alignment, blocks: Option<&[crate::Block]>, text_indent: f32) -> u64 {
    if max_width.is_none() && align == parley::Alignment::Start && blocks.is_none() && text_indent == 0.0 {
        return shape;
    }
    let indent_bits = if text_indent == 0.0 { 0u32 } else { text_indent.to_bits() };
    let mut h = KeyHasher::default();
    (shape, max_width.map(f32::to_bits), align as u8, blocks, indent_bits).hash(&mut h);
    h.finish()
}

/// A truncated or line-clamped cut of `shape` at `max_width`.
#[doc(hidden)]
pub fn elide_key(
    shape: u64,
    mode: u8,
    param: usize,
    max_width: f32,
    align: parley::Alignment,
    blocks: Option<&[crate::Block]>,
) -> u64 {
    let mut h = KeyHasher::default();
    (shape, mode, param, max_width.to_bits(), align as u8, blocks).hash(&mut h);
    h.finish()
}

/// A run's text hashed alone, so each style and scale only mixes it in (see [`super::Run`]).
pub(super) fn run_hash(text: &str) -> u64 {
    let mut h = KeyHasher::default();
    text.hash(&mut h);
    h.finish()
}

/// A run's key at one style and scale.
pub(super) fn run_key(run: u64, style: TextStyle, scale: f32) -> u64 {
    let mut h = KeyHasher::default();
    (run, text_hash("", &[], style, scale, None)).hash(&mut h);
    h.finish()
}
