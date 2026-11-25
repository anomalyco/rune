//! The "Text/Emoji family" playground story: opt-in `Tx::set_emoji_family` /
//! `App::emoji_family` precedence with a runtime-registered COLR v0 demo font, system emoji
//! fallback for uncovered emoji, and monochrome text-symbol preservation.

use std::sync::OnceLock;

use rune::tok::{self, radius, type_};
use rune::{Color, Cx, El, Story, TextStyle, Tx, col, row, text};

const BASE_FONT: &[u8] = include_bytes!("../../../crates/rune/assets/IBMPlexMono-Regular.ttf");
const DEMO_FAMILY: &str = "Rune Demo Emoji";

#[derive(Default)]
pub(crate) struct EmojiFamilyDemo {
    pub(crate) use_custom: bool,
    pub(crate) applied: bool,
    pub(crate) registered: bool,
}

/// Looks up `cp` in a TTF's Format 4 `cmap` subtable, returning a nonzero glyph ID or `1`.
fn lookup_cmap_format4(cmap: &[u8], cp: u16) -> u16 {
    if cmap.len() < 4 {
        return 1;
    }
    let num_tables = u16::from_be_bytes([cmap[2], cmap[3]]) as usize;
    for i in 0..num_tables {
        let rec = 4 + i * 8;
        if rec + 8 > cmap.len() {
            break;
        }
        let off = u32::from_be_bytes([cmap[rec + 4], cmap[rec + 5], cmap[rec + 6], cmap[rec + 7]]) as usize;
        if off + 14 > cmap.len() || u16::from_be_bytes([cmap[off], cmap[off + 1]]) != 4 {
            continue;
        }
        let seg_count = (u16::from_be_bytes([cmap[off + 6], cmap[off + 7]]) / 2) as usize;
        let end_off = off + 14;
        let start_off = end_off + seg_count * 2 + 2;
        let delta_off = start_off + seg_count * 2;
        let range_off = delta_off + seg_count * 2;
        if range_off + seg_count * 2 > cmap.len() {
            continue;
        }
        for seg in 0..seg_count {
            let end = u16::from_be_bytes([cmap[end_off + seg * 2], cmap[end_off + seg * 2 + 1]]);
            let start = u16::from_be_bytes([cmap[start_off + seg * 2], cmap[start_off + seg * 2 + 1]]);
            if cp < start || cp > end {
                continue;
            }
            let delta = i16::from_be_bytes([cmap[delta_off + seg * 2], cmap[delta_off + seg * 2 + 1]]);
            let ro = u16::from_be_bytes([cmap[range_off + seg * 2], cmap[range_off + seg * 2 + 1]]);
            let gid = if ro == 0 {
                (cp as i32 + i32::from(delta)) as u16
            } else {
                let addr = range_off + seg * 2 + ro as usize + (cp - start) as usize * 2;
                if addr + 2 > cmap.len() {
                    return 1;
                }
                let raw = u16::from_be_bytes([cmap[addr], cmap[addr + 1]]);
                if raw == 0 { 0 } else { (raw as i32 + i32::from(delta)) as u16 }
            };
            if gid != 0 {
                return gid;
            }
        }
    }
    1
}

/// Builds a small valid COLR v0 + CPAL TTF from `BASE_FONT` covering `🚀`, `❤️`, and `🇧🇷`
/// (leaving `🎉` uncovered to demonstrate system emoji fallback).
fn demo_emoji_font() -> &'static [u8] {
    static FONT: OnceLock<Vec<u8>> = OnceLock::new();
    FONT.get_or_init(|| {
        let num = u16::from_be_bytes([BASE_FONT[4], BASE_FONT[5]]) as usize;
        let mut tables: Vec<([u8; 4], Vec<u8>)> = Vec::with_capacity(num + 2);
        for rec in BASE_FONT[12..12 + num * 16].as_chunks::<16>().0 {
            let tag: [u8; 4] = [rec[0], rec[1], rec[2], rec[3]];
            let off = u32::from_be_bytes([rec[8], rec[9], rec[10], rec[11]]) as usize;
            let len = u32::from_be_bytes([rec[12], rec[13], rec[14], rec[15]]) as usize;
            if off + len <= BASE_FONT.len() {
                tables.push((tag, BASE_FONT[off..off + len].to_vec()));
            }
        }

        let orig_cmap = tables.iter().find(|(t, _)| t == b"cmap").map(|(_, d)| d.as_slice()).unwrap_or(&[]);
        // Distinct coloured letter/symbol badges: rocket (blue), heart (coral), B (green), R (gold).
        let mappings: [(u32, u16, u16); 4] = [
            (0x2764, lookup_cmap_format4(orig_cmap, b'H' as u16), 1),  // ❤️
            (0x1F1E7, lookup_cmap_format4(orig_cmap, b'B' as u16), 2), // 🇧
            (0x1F1F7, lookup_cmap_format4(orig_cmap, b'R' as u16), 3), // 🇷
            (0x1F680, lookup_cmap_format4(orig_cmap, b'A' as u16), 0), // 🚀
        ];

        // Format 12 cmap (sorted by codepoint).
        let sub_len = (16 + 12 * mappings.len()) as u32;
        let mut cmap = Vec::with_capacity(12 + sub_len as usize);
        cmap.extend_from_slice(&0u16.to_be_bytes());
        cmap.extend_from_slice(&1u16.to_be_bytes());
        cmap.extend_from_slice(&3u16.to_be_bytes());
        cmap.extend_from_slice(&10u16.to_be_bytes());
        cmap.extend_from_slice(&12u32.to_be_bytes());
        cmap.extend_from_slice(&12u16.to_be_bytes());
        cmap.extend_from_slice(&0u16.to_be_bytes());
        cmap.extend_from_slice(&sub_len.to_be_bytes());
        cmap.extend_from_slice(&0u32.to_be_bytes());
        cmap.extend_from_slice(&(mappings.len() as u32).to_be_bytes());
        for &(cp, gid, _) in &mappings {
            cmap.extend_from_slice(&cp.to_be_bytes());
            cmap.extend_from_slice(&cp.to_be_bytes());
            cmap.extend_from_slice(&u32::from(gid).to_be_bytes());
        }

        // CPAL v0 with 4 BGRA colours: #009FFF, #EF3B44, #009C3B, #FFDF00.
        let palette: [[u8; 4]; 4] = [
            [0xFF, 0x9F, 0x00, 0xFF],
            [0x44, 0x3B, 0xEF, 0xFF],
            [0x3B, 0x9C, 0x00, 0xFF],
            [0x00, 0xDF, 0xFF, 0xFF],
        ];
        let mut cpal = Vec::with_capacity(14 + palette.len() * 4);
        cpal.extend_from_slice(&0u16.to_be_bytes()); // version
        cpal.extend_from_slice(&(palette.len() as u16).to_be_bytes()); // numPaletteEntries
        cpal.extend_from_slice(&1u16.to_be_bytes()); // numPalettes
        cpal.extend_from_slice(&(palette.len() as u16).to_be_bytes()); // numColorRecords
        cpal.extend_from_slice(&14u32.to_be_bytes()); // colorRecordsArrayOffset
        cpal.extend_from_slice(&0u16.to_be_bytes()); // colorRecordIndices[0]
        for bgra in palette {
            cpal.extend_from_slice(&bgra);
        }

        // COLR v0: base glyph records sorted ascending by glyph ID.
        let mut colr_entries: Vec<(u16, u16)> = mappings.iter().map(|&(_, gid, pal)| (gid, pal)).collect();
        colr_entries.sort_by_key(|&(gid, _)| gid);
        colr_entries.dedup_by_key(|entry| entry.0);
        let n = colr_entries.len() as u16;
        let base_off = 14u32;
        let layer_off = base_off + u32::from(n) * 6;
        let mut colr = Vec::with_capacity(layer_off as usize + colr_entries.len() * 4);
        colr.extend_from_slice(&0u16.to_be_bytes()); // version
        colr.extend_from_slice(&n.to_be_bytes()); // numBaseGlyphRecords
        colr.extend_from_slice(&base_off.to_be_bytes());
        colr.extend_from_slice(&layer_off.to_be_bytes());
        colr.extend_from_slice(&n.to_be_bytes()); // numLayerRecords
        for (i, &(gid, _)) in colr_entries.iter().enumerate() {
            colr.extend_from_slice(&gid.to_be_bytes());
            colr.extend_from_slice(&(i as u16).to_be_bytes());
            colr.extend_from_slice(&1u16.to_be_bytes());
        }
        for &(gid, pal) in &colr_entries {
            colr.extend_from_slice(&gid.to_be_bytes());
            colr.extend_from_slice(&pal.to_be_bytes());
        }

        // Format 0 name table for DEMO_FAMILY.
        let ps = DEMO_FAMILY.replace(' ', "");
        let entries: [(u16, &str); 5] = [
            (1, DEMO_FAMILY),
            (2, "Regular"),
            (4, DEMO_FAMILY),
            (6, &ps),
            (16, DEMO_FAMILY),
        ];
        let mut str_data = Vec::new();
        let mut records = Vec::new();
        for (name_id, val) in entries {
            let offset = str_data.len() as u16;
            for u in val.encode_utf16() {
                str_data.extend_from_slice(&u.to_be_bytes());
            }
            let len = (str_data.len() as u16) - offset;
            records.push((name_id, len, offset));
        }
        let storage_off = (6 + records.len() * 12) as u16;
        let mut name = Vec::with_capacity(storage_off as usize + str_data.len());
        name.extend_from_slice(&0u16.to_be_bytes());
        name.extend_from_slice(&(records.len() as u16).to_be_bytes());
        name.extend_from_slice(&storage_off.to_be_bytes());
        for (name_id, len, offset) in records {
            name.extend_from_slice(&3u16.to_be_bytes());
            name.extend_from_slice(&1u16.to_be_bytes());
            name.extend_from_slice(&0x0409u16.to_be_bytes());
            name.extend_from_slice(&name_id.to_be_bytes());
            name.extend_from_slice(&len.to_be_bytes());
            name.extend_from_slice(&offset.to_be_bytes());
        }
        name.extend_from_slice(&str_data);

        tables.retain(|(tag, _)| !matches!(tag, b"cmap" | b"name" | b"COLR" | b"CPAL"));
        tables.push((*b"cmap", cmap));
        tables.push((*b"name", name));
        tables.push((*b"COLR", colr));
        tables.push((*b"CPAL", cpal));
        tables.sort_by_key(|(tag, _)| *tag);

        let count = tables.len() as u16;
        let mut entry_sel = 0u16;
        while (1u16 << (entry_sel + 1)) <= count {
            entry_sel += 1;
        }
        let search_range = (1u16 << entry_sel) * 16;
        let range_shift = count * 16 - search_range;

        let mut out = Vec::new();
        out.extend_from_slice(&[0, 1, 0, 0]);
        out.extend_from_slice(&count.to_be_bytes());
        out.extend_from_slice(&search_range.to_be_bytes());
        out.extend_from_slice(&entry_sel.to_be_bytes());
        out.extend_from_slice(&range_shift.to_be_bytes());

        let mut offset = 12 + tables.len() * 16;
        for (tag, data) in &tables {
            out.extend_from_slice(tag);
            out.extend_from_slice(&0u32.to_be_bytes());
            out.extend_from_slice(&(offset as u32).to_be_bytes());
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            offset += (data.len() + 3) & !3;
        }
        for (_, data) in tables {
            out.extend_from_slice(&data);
            while out.len() % 4 != 0 {
                out.push(0);
            }
        }
        out
    })
}

fn poll(s: &mut EmojiFamilyDemo, _t: f64, tx: &mut Tx) -> bool {
    if s.applied == s.use_custom {
        return false;
    }
    s.applied = s.use_custom;
    if s.use_custom {
        if !s.registered {
            let _ = tx.add_font(demo_emoji_font());
            s.registered = true;
        }
        tx.set_emoji_family(Some(DEMO_FAMILY));
    } else {
        tx.set_emoji_family(None);
    }
    true
}

pub(crate) fn story() -> Story<EmojiFamilyDemo> {
    Story::new("Text/Emoji family", EmojiFamilyDemo::default, view)
        .poll(poll)
        .trigger('e', "Toggle custom COLR emoji family", |s, play| {
            s.use_custom = !s.use_custom;
            play.wake_after(std::time::Duration::ZERO);
        })
        .variant("System default", |_| {})
        .variant("Demo COLR emoji", |s| {
            s.use_custom = true;
        })
}

pub(crate) fn view(s: &EmojiFamilyDemo, cx: &mut Cx<EmojiFamilyDemo>) -> El {
    let t = tok::theme(cx.system_dark());
    let accent = tok::accent(0, cx.system_dark());
    let sample_style = TextStyle::new(20.0, 400.0);

    let mode_btn = |key: &'static str, label: &'static str, active: bool, custom: bool| {
        row()
            .key(key)
            .h(28.0)
            .px(12.0)
            .center()
            .radius(radius::SM)
            .fill(if active { accent } else { t.control_track })
            .hover(if active { accent } else { t.hover })
            .focusable(true)
            .role("button")
            .label(label)
            .checked(active)
            .on_press_tx(move |s: &mut EmojiFamilyDemo, tx: &mut Tx| {
                if custom {
                    if !s.registered {
                        let _ = tx.add_font(demo_emoji_font());
                        s.registered = true;
                    }
                    s.use_custom = true;
                    s.applied = true;
                    tx.set_emoji_family(Some(DEMO_FAMILY));
                } else {
                    s.use_custom = false;
                    s.applied = false;
                    tx.set_emoji_family(None);
                }
            })
            .child(text(label).font(type_::LABEL).color(if active { Color::WHITE } else { t.text }).a11y_hidden())
    };

    let active_desc = if s.use_custom {
        r#"Active: emoji_family = Some("Rune Demo Emoji") — 🚀 ❤️ 🇧🇷 use COLR demo font; 🎉 falls back to system emoji"#
    } else {
        "Active: emoji_family = None — all emoji use the platform's system emoji font"
    };

    col().w_full().h_full().center().padding(20.0).child(
        col()
            .w(460.0)
            .gap(12.0)
            .child(
                row()
                    .gap(8.0)
                    .center_y()
                    .child(mode_btn("emoji-system", "System default", !s.use_custom, false))
                    .child(mode_btn("emoji-custom", "Demo COLR emoji", s.use_custom, true)),
            )
            .child(text(active_desc).key("emoji-status").font(type_::LABEL).color(t.text_secondary).w_full().wrap())
            .child(
                col()
                    .key("emoji-samples")
                    .w_full()
                    .padding(14.0)
                    .gap(10.0)
                    .radius(radius::MD)
                    .fill(t.card)
                    .outline(1.0, 0.0, t.card_border)
                    .child(
                        col()
                            .gap(2.0)
                            .child(text("Covered by demo COLR font (default, U+FE0F, flag)").font(type_::LABEL).color(t.text_secondary))
                            .child(text("Launch 🚀  Heart ❤️  Brazil 🇧🇷").key("sample-covered").font(sample_style).color(t.text)),
                    )
                    .child(
                        col()
                            .gap(2.0)
                            .child(text("Uncovered emoji falls back to system emoji font").font(type_::LABEL).color(t.text_secondary))
                            .child(text("Party 🎉  Rainbow 🏳\u{FE0F}\u{200D}🌈").key("sample-fallback").font(sample_style).color(t.text)),
                    )
                    .child(
                        col()
                            .gap(2.0)
                            .child(text("Monochrome text symbols stay on the text font").font(type_::LABEL).color(t.text_secondary))
                            .child(text("Plain heart ❤  Text smile ☺\u{FE0E}").key("sample-mono").font(sample_style).color(t.text)),
                    ),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::text::TextSystem;
    use rune::{Env, Runtime};

    #[test]
    fn emoji_family_story_toggles_demo_colr_font_and_emits_color_quads() {
        let mut rt = Runtime::new(EmojiFamilyDemo::default(), view, (520.0, 360.0), Env::new(true, 1.0));
        rt.frame(0.0);
        assert_eq!(rt.emoji_family(), None);
        let a11y = rt.accessibility_json().to_string();
        assert!(a11y.contains("System default") && a11y.contains("Demo COLR emoji"));

        let (x, y) = rt.find("emoji-custom").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert!(rt.state.use_custom);
        assert_eq!(rt.emoji_family(), Some(DEMO_FAMILY));

        // Verify the synthetic COLR v0 + CPAL demo font rasterizes colour quads through TextSystem.
        let mut ts = TextSystem::default();
        assert!(ts.set_emoji_family(Some(DEMO_FAMILY)));
        let layout = ts.layout_rich("Launch 🚀 Heart ❤️ Brazil 🇧🇷", &[], TextStyle::new(20.0, 400.0), 2.0, None);
        ts.begin_frame();
        let mut quads = Vec::new();
        ts.glyphs_for_layout(&layout, 2.0, [0.0, 0.0], false, &mut quads);
        let color_count = quads.iter().filter(|q| q.color != 0).count();
        assert!(color_count >= 4, "demo COLR v0 font must emit colour quads for 🚀, ❤️, 🇧, 🇷: got {color_count}");

        let (sx, sy) = rt.find("emoji-system").unwrap();
        rt.pointer_down(sx, sy);
        rt.pointer_up(sx, sy);
        rt.frame(2.0 / 60.0);
        assert!(!rt.state.use_custom);
        assert_eq!(rt.emoji_family(), None);
    }
}
