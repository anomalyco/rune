use std::rc::Rc;

use parley::{Layout, PositionedLayoutItem};
use rune_core::color::Color;
use rune_core::tok::{Family, TextStyle};

use super::*;
use crate::layout::cursor::{cursor_from_point, cursor_geometry, range_boxes};
use crate::{GlyphQuad, Ink, TextSystem};

const INTER_REGULAR: &[u8] = include_bytes!("../../../../rune/assets/Inter-Regular.ttf");
const INTER_MEDIUM: &[u8] = include_bytes!("../../../../rune/assets/Inter-Medium.ttf");
const INTER_SEMIBOLD: &[u8] = include_bytes!("../../../../rune/assets/Inter-SemiBold.ttf");
const IBM_PLEX_MONO: &[u8] = include_bytes!("../../../../rune/assets/IBMPlexMono-Regular.ttf");

fn text_with_test_fonts() -> TextSystem {
    TextSystem::new(&[INTER_REGULAR, INTER_MEDIUM, INTER_SEMIBOLD, IBM_PLEX_MONO], ("Inter", "IBM Plex Mono"))
}

#[derive(Debug, PartialEq, Eq)]
struct QuadBits {
    rect: [u32; 4],
    uv: [u32; 2],
    ink: u32,
    byte: u32,
    color: u32,
    has_outline: bool,
    lcd: bool,
}

fn quad_bits(qs: &[GlyphQuad]) -> Vec<QuadBits> {
    qs.iter()
        .map(|q| QuadBits {
            rect: q.rect.map(f32::to_bits),
            uv: q.uv.map(f32::to_bits),
            ink: q.ink,
            byte: q.byte,
            color: q.color,
            has_outline: q.outline.is_some(),
            lcd: q.lcd,
        })
        .collect()
}

fn layout_glyph_records(layout: &Layout<Ink>) -> Vec<(u64, u32, u32, u32, u32, u32)> {
    let mut out = Vec::new();
    for line in layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(gr) = item else { continue };
            let font = gr.run().font();
            let mut rel = gr.offset();
            for g in gr.glyphs() {
                out.push((
                    font.data.id(),
                    font.index,
                    g.id,
                    rel.to_bits(),
                    g.x.to_bits(),
                    g.y.to_bits(),
                ));
                rel += g.advance;
            }
        }
    }
    out
}

fn tabular_glyph_records(tab: &TabularGlyphs) -> Vec<(u64, u32, u32, u32, u32, u32)> {
    let tpl = &tab.template;
    tpl.slots
        .iter()
        .map(|slot| {
            let f = &tpl.faces[slot.face as usize].face;
            let id = if slot.digit_slot == u8::MAX {
                slot.glyph_id
            } else {
                let ds = slot.digit_slot as usize;
                tpl.digit_glyphs[ds][tab.digits[ds] as usize]
            };
            (f.font.data.id(), f.font.index, id, slot.rel.to_bits(), slot.x.to_bits(), slot.y.to_bits())
        })
        .collect()
}

fn bbox_bits(b: parley::BoundingBox) -> [u64; 4] {
    [b.x0.to_bits(), b.y0.to_bits(), b.x1.to_bits(), b.y1.to_bits()]
}

/// Strict differential oracle comparing `layout_live_tabular` + `glyphs_for_tabular` against
/// a fresh full `shape_probe` + `glyphs_in_band`. Fails immediately on the first bit difference
/// in glyph IDs, positions, quads, line box, caret rects, hit-testing, or selection rects.
fn assert_bit_identical(
    text: &mut TextSystem,
    value: &str,
    style: TextStyle,
    scale: f32,
    expect_fast: Option<bool>,
    ctx: &str,
) {
    let (fast_layout, fast_tab) = text.layout_live_tabular(value, style, scale);
    if let Some(want) = expect_fast {
        assert_eq!(
            fast_tab.is_some(),
            want,
            "{ctx}: value={value:?} fast_path expected={want}, got={}",
            fast_tab.is_some()
        );
    }
    let ref_layout = text.shape_probe(value, style, scale);

    // 1. Layout and line box metrics.
    assert_eq!(fast_layout.len(), ref_layout.len(), "{ctx}: line count for {value:?}");
    assert_eq!(fast_layout.is_rtl(), ref_layout.is_rtl(), "{ctx}: is_rtl for {value:?}");
    assert_eq!(fast_layout.full_width().to_bits(), ref_layout.full_width().to_bits(), "{ctx}: full_width for {value:?}");
    assert_eq!(fast_layout.width().to_bits(), ref_layout.width().to_bits(), "{ctx}: width for {value:?}");
    assert_eq!(fast_layout.height().to_bits(), ref_layout.height().to_bits(), "{ctx}: height for {value:?}");
    for li in 0..ref_layout.len() {
        let fa = fast_layout.get(li).unwrap();
        let fb = ref_layout.get(li).unwrap();
        assert_eq!(fa.text_range(), fb.text_range(), "{ctx}: line {li} range for {value:?}");
        assert_eq!(line_metrics_bits(fa.metrics()), line_metrics_bits(fb.metrics()), "{ctx}: line {li} metrics for {value:?}");
    }

    // 2. Direct glyph ID and pen-position parity.
    if let Some(tab) = &fast_tab {
        assert_eq!(
            tabular_glyph_records(tab),
            layout_glyph_records(&ref_layout),
            "{ctx}: raw glyph IDs/positions mismatch for {value:?}"
        );
    }

    // 3. Bit-identical GlyphQuad output across origins, zoom, bytes, glide, and clip bands.
    text.begin_frame();
    text.set_glyph_inks(Color::WHITE, &[]);
    let cases = [
        ([13.25f32, 27.5], 1.0f32, false, [0.0f32, 0.0], f32::NEG_INFINITY..f32::INFINITY),
        ([13.25, 27.5], 1.0, true, [0.0, 0.0], f32::NEG_INFINITY..f32::INFINITY),
        ([4.5, 9.75], 1.5, true, [0.0, 0.0], f32::NEG_INFINITY..f32::INFINITY),
        ([10.0, 20.0], 1.0, false, [0.25, -0.25], f32::NEG_INFINITY..f32::INFINITY),
        ([10.0, 20.0], 1.0, false, [0.0, 0.0], 1000.0..2000.0),
        ([10.0, 20.0], 1.0, false, [0.0, 0.0], -500.0..-100.0),
    ];
    let mut q_fast = Vec::new();
    let mut q_ref = Vec::new();
    for (origin, k, bytes, glide, band) in cases {
        q_fast.clear();
        q_ref.clear();
        text.set_glide(glide);
        if let Some(tab) = &fast_tab {
            text.glyphs_for_tabular(tab, origin, k, bytes, band.clone(), &mut q_fast);
        } else {
            text.glyphs_in_band(&fast_layout, origin, k, bytes, band.clone(), &mut q_fast);
        }
        text.glyphs_in_band(&ref_layout, origin, k, bytes, band.clone(), &mut q_ref);
        assert_eq!(
            quad_bits(&q_fast),
            quad_bits(&q_ref),
            "{ctx}: GlyphQuad mismatch for {value:?} origin={origin:?} k={k} bytes={bytes} glide={glide:?} band={band:?}"
        );
    }
    text.set_glide([0.0, 0.0]);

    // 4. Caret geometry at every char boundary (both affinities).
    for b in 0..=value.len() {
        if !value.is_char_boundary(b) {
            continue;
        }
        for aff in [parley::layout::Affinity::Downstream, parley::layout::Affinity::Upstream] {
            let cf = parley::editing::Cursor::from_byte_index(&fast_layout, b, aff);
            let cr = parley::editing::Cursor::from_byte_index(&ref_layout, b, aff);
            assert_eq!(
                bbox_bits(cursor_geometry(&cf, &fast_layout, 1.5 * scale)),
                bbox_bits(cursor_geometry(&cr, &ref_layout, 1.5 * scale)),
                "{ctx}: caret rect mismatch at byte {b} ({aff:?}) for {value:?}"
            );
        }
    }

    // 5. Hit testing across the full line width.
    let fw = ref_layout.full_width();
    let mid_y = ref_layout.height() * 0.5;
    for step in 0..=16 {
        let x = -4.0 + (fw + 8.0) * (step as f32 / 16.0);
        let hf = cursor_from_point(&fast_layout, x, mid_y);
        let hr = cursor_from_point(&ref_layout, x, mid_y);
        assert_eq!(hf.index(), hr.index(), "{ctx}: hit-test index at x={x} for {value:?}");
        assert_eq!(hf.affinity(), hr.affinity(), "{ctx}: hit-test affinity at x={x} for {value:?}");
    }

    // 6. Selection rectangles across byte subranges.
    let bounds: Vec<usize> = value.char_indices().map(|(i, _)| i).chain([value.len()]).collect();
    for (ai, &a) in bounds.iter().enumerate() {
        for &b in bounds.iter().skip(ai + 1) {
            let rf: Vec<_> = range_boxes(&fast_layout, a, b, true).into_iter().map(|(bb, li)| (bbox_bits(bb), li)).collect();
            let rr: Vec<_> = range_boxes(&ref_layout, a, b, true).into_iter().map(|(bb, li)| (bbox_bits(bb), li)).collect();
            assert_eq!(rf, rr, "{ctx}: selection range_boxes({a}..{b}) mismatch for {value:?}");
        }
    }
}

#[test]
fn de_bruijn_sequence_covers_all_100_ordered_digit_pairs_interior() {
    let bytes = DE_BRUIJN_DIGITS.as_bytes();
    assert_eq!(bytes.len(), 102);
    assert!(bytes.iter().all(u8::is_ascii_digit));
    // Every ordered pair (a, b) must appear strictly inside 0..101 (before final byte 101).
    let mut seen = [[false; 10]; 10];
    for w in bytes[..bytes.len() - 1].windows(2) {
        seen[(w[0] - b'0') as usize][(w[1] - b'0') as usize] = true;
    }
    for (a, row) in seen.iter().enumerate() {
        for (b, &hit) in row.iter().enumerate() {
            assert!(hit, "missing interior digit pair {a}{b}");
        }
    }
}

#[test]
fn differential_sweep_inter_tnum_all_digits_positions_and_formats() {
    let mut text = text_with_test_fonts();
    let styles = [
        TextStyle::new(10.0, 400.0).features("'tnum' 1"),
        TextStyle::new(12.0, 400.0).features("'tnum' 1"),
        TextStyle::new(13.5, 500.0).features("'tnum' 1"),
        TextStyle::new(16.0, 600.0).features("'tnum' 1"),
        TextStyle::new(14.0, 400.0).features("'tnum' 1").tracking(0.5),
    ];
    let scales = [1.0f32, 1.25, 2.0];
    let masks = [
        "0",
        "00",
        "000.00 bar",
        "-000.00 bar",
        "+0,000.00 ms",
        "$0,000.00",
        "00:00:00",
        "(000.0%)",
    ];

    for style in styles {
        for scale in scales {
            for mask in masks {
                // First string observes the mask; second string triggers verification and hits.
                let first = mask.to_string();
                let second: String = mask.chars().map(|c| if c.is_ascii_digit() { '1' } else { c }).collect();
                assert_bit_identical(&mut text, &first, style, scale, Some(false), "inter first");
                assert_bit_identical(&mut text, &second, style, scale, Some(true), "inter second");

                // Sweep all 10 digits in EVERY digit position, asserting 0 shapes on the fast path.
                let digit_pos: Vec<usize> = mask.bytes().enumerate().filter(|(_, b)| b.is_ascii_digit()).map(|(i, _)| i).collect();
                for &pos in &digit_pos {
                    for d in b'0'..=b'9' {
                        let mut buf = mask.as_bytes().to_vec();
                        // Fill other slots with varying digits so mixed combinations are tested.
                        for (k, &p) in digit_pos.iter().enumerate() {
                            buf[p] = b'0' + ((k as u8 + (d - b'0') * 3) % 10);
                        }
                        buf[pos] = d;
                        let val = std::str::from_utf8(&buf).unwrap();
                        let shaped_before = text.shaped;
                        let (_, tab) = text.layout_live_tabular(val, style, scale);
                        assert!(tab.is_some(), "expected verified fast path for {val:?}");
                        assert_eq!(text.shaped, shaped_before, "verified fast path must shape 0 times for {val:?}");
                        assert_bit_identical(&mut text, val, style, scale, Some(true), "inter sweep");
                    }
                }
            }
        }
    }

    // Exhaustive all-100 ordered pairs 00..=99 across plain, signed-unit, and colon-calt masks.
    let style = TextStyle::new(12.0, 400.0).features("'tnum' 1");
    for a in 0..10u8 {
        for b in 0..10u8 {
            let plain = format!("{a}{b}");
            let unit = format!("-{a}{b}.{b}{a} bar");
            let clock = format!("{a}{b}:{b}{a}");
            assert_bit_identical(&mut text, &plain, style, 2.0, Some(true), "inter 100 pairs plain");
            assert_bit_identical(&mut text, &unit, style, 2.0, None, "inter 100 pairs unit");
            assert_bit_identical(&mut text, &clock, style, 2.0, None, "inter 100 pairs clock");
        }
    }
}

#[test]
fn differential_sweep_ibm_plex_mono_with_and_without_tnum() {
    let mut text = text_with_test_fonts();
    let styles = [
        TextStyle::new(12.0, 400.0).family(Family::Mono),
        TextStyle::new(14.0, 400.0).family(Family::Mono).features("'tnum' 1"),
        TextStyle::new(12.0, 400.0).family(Family::Mono).tracking(0.4),
    ];
    for style in styles {
        for scale in [1.0f32, 2.0] {
            let mask = "-00,000.00 kPa";
            let second = "-12,345.67 kPa";
            assert_bit_identical(&mut text, mask, style, scale, Some(false), "mono first");
            assert_bit_identical(&mut text, second, style, scale, Some(true), "mono second");

            let shaped_before = text.shaped;
            for a in 0..10u8 {
                for b in 0..10u8 {
                    let val = format!("-{a}{b},{b}{a}{a}.{a}{b} kPa");
                    let (_, tab) = text.layout_live_tabular(&val, style, scale);
                    assert!(tab.is_some());
                }
            }
            assert_eq!(text.shaped, shaped_before, "mono fast path shapes 0 times across all 100 pairs");

            for d in 0..10u8 {
                let val = format!("-{d}9,8{d}6.{d}1 kPa");
                assert_bit_identical(&mut text, &val, style, scale, Some(true), "mono verify");
            }
        }
    }
}

#[test]
fn proportional_digits_without_tnum_mark_ineligible_with_zero_probes() {
    let mut text = text_with_test_fonts();
    let style = TextStyle::new(13.0, 400.0); // Inter default: proportional figures
    let s0 = text.shaped;
    let (_, t1) = text.layout_live_tabular("111.11 bar", style, 2.0);
    assert!(t1.is_none());
    assert_eq!(text.shaped - s0, 1, "first observation shapes once");

    // Different width on second string marks Ineligible immediately with zero probe shapes.
    let s1 = text.shaped;
    let (_, t2) = text.layout_live_tabular("888.88 bar", style, 2.0);
    assert!(t2.is_none());
    assert_eq!(text.shaped - s1, 1, "width mismatch skips all probes");

    // Subsequent values stay on full shaping and match bit-for-bit.
    for v in ["000.00 bar", "123.45 bar", "987.65 bar", "456.78 bar"] {
        assert_bit_identical(&mut text, v, style, 2.0, Some(false), "proportional");
    }
}

#[test]
fn ligature_and_contextual_features_fall_back_and_match_full_reshape() {
    let mut text = text_with_test_fonts();
    // 'frac' 1 converts digits around '/' into numerators/denominators/fractions.
    let frac = TextStyle::new(14.0, 400.0).features("'tnum' 1, 'frac' 1");
    for v in ["0/0", "1/1", "1/2", "1/4", "3/4", "7/8", "9/9"] {
        assert_bit_identical(&mut text, v, frac, 2.0, Some(false), "inter frac");
    }
    // 'dlig' 1 enables discretionary ligatures on Inter.
    let dlig = TextStyle::new(14.0, 400.0).features("'tnum' 1, 'dlig' 1");
    for v in ["0/0", "1/1", "1/2", "3/4", "2/3"] {
        assert_bit_identical(&mut text, v, dlig, 2.0, None, "inter dlig");
    }
    // 'ordn' 1 enables ordinal contextual rules.
    let ordn = TextStyle::new(14.0, 400.0).features("'tnum' 1, 'ordn' 1");
    for v in ["0st", "1st", "2st", "3st", "1nd", "2nd"] {
        assert_bit_identical(&mut text, v, ordn, 2.0, None, "inter ordn");
    }
}

#[test]
fn rtl_non_ascii_control_and_overlong_strings_fall_back() {
    let mut text = text_with_test_fonts();
    let style = TextStyle::new(14.0, 400.0).features("'tnum' 1");
    let long65 = "0".repeat(65);
    let cases = [
        "الضغط 123.45 بار",
        "الضغط 987.65 بار",
        "123.45 بار",
        "שלום 00:42 עולם",
        "שלום 12:34 עולם",
        "12\n34",
        "12\t34",
        "12\r34",
        "bar",
        "",
        long65.as_str(),
    ];
    for v in cases {
        assert_bit_identical(&mut text, v, style, 2.0, Some(false), "fallback edge");
    }
}

#[test]
fn cache_bound_and_invalidation_on_font_scale_and_drop_shaped() {
    let mut text = text_with_test_fonts();
    let style = TextStyle::new(12.0, 400.0).features("'tnum' 1");
    let layouts_before = text.cached_layout_count();

    // Exceed MAX_TABULAR_TEMPLATES (256) with 300 distinct masks.
    for i in 0..300u32 {
        let prefix = format!("M{:02}{:02}", (b'A' + (i / 26) as u8) as char, (b'a' + (i % 26) as u8) as char);
        let a = format!("{prefix} 00");
        let b = format!("{prefix} 12");
        let _ = text.layout_live_tabular(&a, style, 1.0);
        let _ = text.layout_live_tabular(&b, style, 1.0);
        assert!(text.tabular_template_count() <= MAX_TABULAR_TEMPLATES);
        assert!(text.tabular_template_bytes() <= MAX_TABULAR_BYTES);
    }
    assert_eq!(text.tabular_template_count(), MAX_TABULAR_TEMPLATES);
    assert_eq!(text.cached_layout_count(), layouts_before, "tabular probes/hits must not grow layout cache");

    // In-place update reuses the unique Rc<TabularGlyphs> and bumps value_id strictly.
    let mut slot = None;
    let _ = text.layout_live_tabular_into("00.00 bar", style, 2.0, &mut slot);
    let _ = text.layout_live_tabular_into("12.34 bar", style, 2.0, &mut slot);
    let ptr1 = Rc::as_ptr(slot.as_ref().unwrap());
    let id1 = slot.as_ref().unwrap().value_id();
    let _ = text.layout_live_tabular_into("56.78 bar", style, 2.0, &mut slot);
    let ptr2 = Rc::as_ptr(slot.as_ref().unwrap());
    let id2 = slot.as_ref().unwrap().value_id();
    assert_eq!(ptr1, ptr2, "unique Rc<TabularGlyphs> updated in place with zero allocation");
    assert!(id2 > id1, "value_id strictly increases on every digit update");

    // drop_shaped() (and set_families) clears the tabular cache.
    assert!(text.tabular_template_count() > 0);
    text.drop_shaped();
    assert_eq!(text.tabular_template_count(), 0);
    assert_eq!(text.tabular_template_bytes(), 0);
}

/// Builds an in-memory modified copy of a TrueType font with one table replaced or added,
/// recalculating table offsets and checksums so `skrifa` and `harfrust` load it cleanly.
fn patch_sfnt_table(base: &[u8], tag: [u8; 4], payload: &[u8]) -> &'static [u8] {
    let u16_be = |b: &[u8], o: usize| u16::from_be_bytes([b[o], b[o + 1]]);
    let u32_be = |b: &[u8], o: usize| u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let sfnt_ver = u32_be(base, 0);
    let n = u16_be(base, 4) as usize;
    let mut tables: Vec<([u8; 4], Vec<u8>)> = Vec::with_capacity(n + 1);
    let mut replaced = false;
    for i in 0..n {
        let off = 12 + i * 16;
        let t: [u8; 4] = base[off..off + 4].try_into().unwrap();
        let t_off = u32_be(base, off + 8) as usize;
        let t_len = u32_be(base, off + 12) as usize;
        if t == tag {
            tables.push((t, payload.to_vec()));
            replaced = true;
        } else if t != *b"DSIG" {
            tables.push((t, base[t_off..t_off + t_len].to_vec()));
        }
    }
    if !replaced {
        tables.push((tag, payload.to_vec()));
    }
    tables.sort_by_key(|(t, _)| *t);
    let num = tables.len() as u16;
    let mut entry_selector = 0u16;
    let mut search_range = 16u16;
    while search_range * 2 <= num * 16 {
        search_range *= 2;
        entry_selector += 1;
    }
    let range_shift = num * 16 - search_range;
    let mut out = Vec::new();
    out.extend_from_slice(&sfnt_ver.to_be_bytes());
    out.extend_from_slice(&num.to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&range_shift.to_be_bytes());
    let header_len = 12 + tables.len() * 16;
    let mut data_off = (header_len + 3) & !3;
    let mut offsets = Vec::with_capacity(tables.len());
    for (t, data) in &tables {
        let mut sum = 0u32;
        for chunk in data.chunks(4) {
            let mut b = [0u8; 4];
            b[..chunk.len()].copy_from_slice(chunk);
            sum = sum.wrapping_add(u32::from_be_bytes(b));
        }
        out.extend_from_slice(t);
        out.extend_from_slice(&sum.to_be_bytes());
        out.extend_from_slice(&(data_off as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        offsets.push(data_off);
        data_off += (data.len() + 3) & !3;
    }
    for ((_, data), off) in tables.iter().zip(offsets) {
        out.resize(off, 0);
        out.extend_from_slice(data);
    }
    out.resize((out.len() + 3) & !3, 0);
    Box::leak(out.into_boxed_slice())
}

fn gid_of(font_bytes: &[u8], ch: char) -> u16 {
    use skrifa::MetadataProvider;
    skrifa::FontRef::new(font_bytes).unwrap().charmap().map(ch).unwrap().to_u32() as u16
}

/// Builds a minimal GSUB v1.0 table with one `liga` feature (Type 4 LigatureSubst) turning
/// `seq[0] + seq[1..]` into `target`.
fn build_single_ligature_gsub(seq: &[u16], target: u16) -> Vec<u8> {
    let mut b = Vec::new();
    let push_u16 = |b: &mut Vec<u8>, v: u16| b.extend_from_slice(&v.to_be_bytes());
    // Header (10 bytes): version 1.0, scriptList=10, featureList=30, lookupList=44
    push_u16(&mut b, 1); push_u16(&mut b, 0);
    push_u16(&mut b, 10); push_u16(&mut b, 30); push_u16(&mut b, 44);
    // ScriptList at 10 (20 bytes): 1 script 'DFLT' -> offset 8 (abs 18)
    push_u16(&mut b, 1);
    b.extend_from_slice(b"DFLT"); push_u16(&mut b, 8);
    // Script at 18 (4 bytes): defaultLangSys=4 (abs 22), langSysCount=0
    push_u16(&mut b, 4); push_u16(&mut b, 0);
    // LangSys at 22 (8 bytes): lookupOrder=0, reqFeature=0xffff, featureCount=1, idx=0
    push_u16(&mut b, 0); push_u16(&mut b, 0xffff); push_u16(&mut b, 1); push_u16(&mut b, 0);
    assert_eq!(b.len(), 30);
    // FeatureList at 30 (14 bytes): 1 feature 'liga' -> offset 8 (abs 38)
    push_u16(&mut b, 1);
    b.extend_from_slice(b"liga"); push_u16(&mut b, 8);
    // Feature at 38 (6 bytes): params=0, lookupCount=1, lookupIdx=0
    push_u16(&mut b, 0); push_u16(&mut b, 1); push_u16(&mut b, 0);
    assert_eq!(b.len(), 44);
    // LookupList at 44: 1 lookup -> offset 4 (abs 48)
    push_u16(&mut b, 1); push_u16(&mut b, 4);
    // Lookup at 48: type=4 (Ligature), flag=0, subtableCount=1, subtableOff=8 (abs 56)
    push_u16(&mut b, 4); push_u16(&mut b, 0); push_u16(&mut b, 1); push_u16(&mut b, 8);
    // LigatureSubstFormat1 at 56: format=1, covOff=8 (abs 64), ligSetCount=1, ligSetOff=14 (abs 70)
    push_u16(&mut b, 1); push_u16(&mut b, 8); push_u16(&mut b, 1); push_u16(&mut b, 14);
    // CoverageFormat1 at 64 (6 bytes): format=1, count=1, glyph=seq[0]
    push_u16(&mut b, 1); push_u16(&mut b, 1); push_u16(&mut b, seq[0]);
    // LigatureSet at 70 (4 bytes): count=1, ligOff=4 (abs 74)
    push_u16(&mut b, 1); push_u16(&mut b, 4);
    // Ligature at 74: ligGlyph=target, compCount=seq.len(), components=seq[1..]
    push_u16(&mut b, target); push_u16(&mut b, seq.len() as u16);
    for &c in &seq[1..] { push_u16(&mut b, c); }
    b
}

/// Builds a minimal Format-0 `kern` table kerning a single pair `(left, right)` by `value`.
fn build_single_pair_kern(left: u16, right: u16, value: i16) -> Vec<u8> {
    let mut b = Vec::new();
    let push_u16 = |b: &mut Vec<u8>, v: u16| b.extend_from_slice(&v.to_be_bytes());
    // kern header: version=0, nTables=1
    push_u16(&mut b, 0); push_u16(&mut b, 1);
    // subtable header: version=0, length=20, coverage=1 (horizontal format 0)
    push_u16(&mut b, 0); push_u16(&mut b, 20); push_u16(&mut b, 1);
    // format 0: nPairs=1, searchRange=6, entrySelector=0, rangeShift=0
    push_u16(&mut b, 1); push_u16(&mut b, 6); push_u16(&mut b, 0); push_u16(&mut b, 0);
    push_u16(&mut b, left); push_u16(&mut b, right); b.extend_from_slice(&value.to_be_bytes());
    b
}

#[test]
fn adversarial_fonts_with_hidden_pair_kern_triple_ligature_and_separator_ligature_are_rejected() {
    let g1 = gid_of(IBM_PLEX_MONO, '1');
    let g2 = gid_of(IBM_PLEX_MONO, '2');
    let g3 = gid_of(IBM_PLEX_MONO, '3');
    let g7 = gid_of(IBM_PLEX_MONO, '7');
    let gcolon = gid_of(IBM_PLEX_MONO, ':');
    let gx = gid_of(IBM_PLEX_MONO, 'X');

    // (a) Hidden pair kerning on ('1', '7') only: invisible to all 10 uniform fills 00..99,
    //     caught by the order-2 De Bruijn probe!
    let kern_font = patch_sfnt_table(IBM_PLEX_MONO, *b"kern", &build_single_pair_kern(g1, g7, -200));
    let mut ts_kern = TextSystem::new(&[kern_font], ("IBM Plex Mono", "IBM Plex Mono"));
    let style = TextStyle::new(14.0, 400.0);
    assert_bit_identical(&mut ts_kern, "00", style, 2.0, Some(false), "kern first");
    assert_bit_identical(&mut ts_kern, "22", style, 2.0, Some(false), "kern caught by De Bruijn");
    assert_bit_identical(&mut ts_kern, "17", style, 2.0, Some(false), "kern pair 17 exact");

    // (b) Hidden 3-digit ligature "123" -> 'X': invisible to ALL 10 uniform fills AND to
    //     order-2 De Bruijn (which contains no "123"), caught by OpenType GSUB verifier!
    assert!(!DE_BRUIJN_DIGITS.contains("123"));
    let lig3_font = patch_sfnt_table(IBM_PLEX_MONO, *b"GSUB", &build_single_ligature_gsub(&[g1, g2, g3], gx));
    let mut ts_lig3 = TextSystem::new(&[lig3_font], ("IBM Plex Mono", "IBM Plex Mono"));
    assert_bit_identical(&mut ts_lig3, "000", style, 2.0, Some(false), "lig3 first");
    assert_bit_identical(&mut ts_lig3, "444", style, 2.0, Some(false), "lig3 caught by GSUB check");
    assert_bit_identical(&mut ts_lig3, "123", style, 2.0, Some(false), "lig3 123 exact");

    // (c) Hidden mixed-digit separator ligature "1:2" -> 'X': invisible to ALL 10 uniform fills
    //     "0:0".."9:9" and has no adjacent digits at all, caught by OpenType GSUB verifier!
    let lig_sep_font = patch_sfnt_table(IBM_PLEX_MONO, *b"GSUB", &build_single_ligature_gsub(&[g1, gcolon, g2], gx));
    let mut ts_sep = TextSystem::new(&[lig_sep_font], ("IBM Plex Mono", "IBM Plex Mono"));
    assert_bit_identical(&mut ts_sep, "0:0", style, 2.0, Some(false), "sep lig first");
    assert_bit_identical(&mut ts_sep, "3:3", style, 2.0, Some(false), "sep lig caught by GSUB check");
    assert_bit_identical(&mut ts_sep, "1:2", style, 2.0, Some(false), "sep lig 1:2 exact");
}

#[test]
#[ignore = "timing; run with --profile perf -- --ignored --nocapture"]
fn tabular_perf_bench() {
    fn reading(i: usize, k: u64) -> String {
        let v = 100.0 + 80.0 * ((i as f64) * 0.37 + (k as f64) * 0.113).sin();
        format!("{v:06.2} bar")
    }
    fn median(mut v: Vec<f64>) -> f64 {
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    }
    fn run_before(values: &[Vec<String>], style: TextStyle, scale: f32) -> (u32, f64, f64) {
        let mut ts = text_with_test_fonts();
        let mut quads: Vec<GlyphQuad> = Vec::with_capacity(16);
        ts.begin_frame();
        ts.set_glyph_inks(Color::WHITE, &[]);
        for s in &values[0] {
            let l = ts.layout_live(s, style, scale);
            quads.clear();
            ts.glyphs_in_band(&l, [10.0, 20.0], 1.0, false, f32::NEG_INFINITY..f32::INFINITY, &mut quads);
        }
        let shaped_before = ts.shaped;
        let mut shape_ns = 0.0f64;
        let mut total_ns = 0.0f64;
        let mut sink = 0usize;
        for tick in &values[1..] {
            ts.begin_frame();
            ts.set_glyph_inks(Color::WHITE, &[]);
            let t0 = rune_core::time::now();
            let mut layouts = Vec::with_capacity(tick.len());
            for s in tick {
                layouts.push(ts.layout_live(s, style, scale));
            }
            let t1 = rune_core::time::now();
            for l in &layouts {
                quads.clear();
                ts.glyphs_in_band(l, [10.0, 20.0], 1.0, false, f32::NEG_INFINITY..f32::INFINITY, &mut quads);
                sink += quads.len();
            }
            let t2 = rune_core::time::now();
            shape_ns += (t1 - t0).as_nanos() as f64;
            total_ns += (t2 - t0).as_nanos() as f64;
        }
        std::hint::black_box(sink);
        let updates = (values.len() - 1) * values[0].len();
        (ts.shaped - shaped_before, shape_ns / updates as f64, total_ns / updates as f64)
    }
    fn run_after(values: &[Vec<String>], style: TextStyle, scale: f32) -> (u32, f64, f64) {
        let mut ts = text_with_test_fonts();
        let mut quads: Vec<GlyphQuad> = Vec::with_capacity(16);
        let mut slots: Vec<Option<Rc<TabularGlyphs>>> = vec![None; values[0].len()];
        ts.begin_frame();
        ts.set_glyph_inks(Color::WHITE, &[]);
        for (s, slot) in values[0].iter().zip(&mut slots) {
            let l = ts.layout_live_tabular_into(s, style, scale, slot);
            quads.clear();
            if let Some(tab) = slot.as_deref() {
                ts.glyphs_for_tabular(tab, [10.0, 20.0], 1.0, false, f32::NEG_INFINITY..f32::INFINITY, &mut quads);
            } else {
                ts.glyphs_in_band(&l, [10.0, 20.0], 1.0, false, f32::NEG_INFINITY..f32::INFINITY, &mut quads);
            }
        }
        let shaped_before = ts.shaped;
        let mut shape_ns = 0.0f64;
        let mut total_ns = 0.0f64;
        let mut sink = 0usize;
        for tick in &values[1..] {
            ts.begin_frame();
            ts.set_glyph_inks(Color::WHITE, &[]);
            let t0 = rune_core::time::now();
            for (s, slot) in tick.iter().zip(&mut slots) {
                let _ = ts.layout_live_tabular_into(s, style, scale, slot);
            }
            let t1 = rune_core::time::now();
            for slot in &slots {
                quads.clear();
                let tab = slot.as_deref().expect("verified tabular");
                ts.glyphs_for_tabular(tab, [10.0, 20.0], 1.0, false, f32::NEG_INFINITY..f32::INFINITY, &mut quads);
                sink += quads.len();
            }
            let t2 = rune_core::time::now();
            shape_ns += (t1 - t0).as_nanos() as f64;
            total_ns += (t2 - t0).as_nanos() as f64;
        }
        std::hint::black_box(sink);
        let updates = (values.len() - 1) * values[0].len();
        (ts.shaped - shaped_before, shape_ns / updates as f64, total_ns / updates as f64)
    }

    const N: usize = 500;
    const TICKS: usize = 61;
    const PAIRS: usize = 9;
    let values: Vec<Vec<String>> = (0..TICKS)
        .map(|k| (0..N).map(|i| reading(i, k as u64)).collect())
        .collect();

    for (label, style) in [
        ("Inter 12pt 'tnum' 1", TextStyle::new(12.0, 400.0).features("'tnum' 1")),
        ("IBM Plex Mono 12pt", TextStyle::new(12.0, 400.0).family(Family::Mono)),
    ] {
        let (mut bs_v, mut bt_v, mut as_v, mut at_v) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let (mut bc_last, mut ac_last) = (0u32, 0u32);
        for pair in 0..PAIRS {
            if pair % 2 == 0 {
                let (bc, bs, bt) = run_before(&values, style, 2.0);
                let (ac, as_, at) = run_after(&values, style, 2.0);
                bc_last = bc; ac_last = ac;
                bs_v.push(bs); bt_v.push(bt); as_v.push(as_); at_v.push(at);
            } else {
                let (ac, as_, at) = run_after(&values, style, 2.0);
                let (bc, bs, bt) = run_before(&values, style, 2.0);
                bc_last = bc; ac_last = ac;
                bs_v.push(bs); bt_v.push(bt); as_v.push(as_); at_v.push(at);
            }
        }
        let minmax = |v: &[f64]| (
            v.iter().copied().fold(f64::INFINITY, f64::min),
            v.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        );
        let (bsm, asm) = (median(bs_v.clone()), median(as_v.clone()));
        let (btm, atm) = (median(bt_v.clone()), median(at_v.clone()));
        println!("=== {label} (N={N}, {} ticks, {PAIRS} paired alternating runs, scale=2.0) ===", TICKS - 1);
        println!("  shaped/tick: before={} after={}", bc_last / (TICKS as u32 - 1), ac_last / (TICKS as u32 - 1));
        println!("  shape-only ns/val: before median={bsm:.1} (min/max={:.1}..{:.1}) after median={asm:.1} (min/max={:.1}..{:.1}) ({:.1}x)",
            minmax(&bs_v).0, minmax(&bs_v).1, minmax(&as_v).0, minmax(&as_v).1, bsm / asm);
        println!("  shape+emit ns/val: before median={btm:.1} (min/max={:.1}..{:.1}) after median={atm:.1} (min/max={:.1}..{:.1}) ({:.1}x)",
            minmax(&bt_v).0, minmax(&bt_v).1, minmax(&at_v).0, minmax(&at_v).1, btm / atm);
        println!("  500-cell tick ms:  before shape={:.3} total={:.3} | after shape={:.3} total={:.3}",
            bsm * 500.0 / 1e6, btm * 500.0 / 1e6, asm * 500.0 / 1e6, atm * 500.0 / 1e6);
    }
}
