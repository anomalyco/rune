//! Tests for opt-in `TextSystem::set_emoji_family` / `emoji_family`, precedence with
//! selected active-stack colour fonts, lazy system emoji loading, normalization, and `measure`.

use super::tests::{glyph_signature, make_test_font, register_test_font};
use super::*;

/// Builds a synthetic test font whose glyphs all have advance width `advance_units` (in font
/// units, where IBM Plex Mono has UPM 1000) so switching emoji families visibly changes
/// `measure` widths too.
fn make_sized_test_font(family: &str, codepoints: &[char], is_color: bool, advance_units: u16) -> Vec<u8> {
    let base = make_test_font(family, codepoints, is_color);
    let num = u16::from_be_bytes([base[4], base[5]]) as usize;
    let mut tables: Vec<([u8; 4], Vec<u8>)> = Vec::with_capacity(num);
    for rec in base[12..12 + num * 16].as_chunks::<16>().0 {
        let tag: [u8; 4] = [rec[0], rec[1], rec[2], rec[3]];
        let off = u32::from_be_bytes([rec[8], rec[9], rec[10], rec[11]]) as usize;
        let len = u32::from_be_bytes([rec[12], rec[13], rec[14], rec[15]]) as usize;
        tables.push((tag, base[off..off + len].to_vec()));
    }
    let hhea_metrics = tables
        .iter()
        .find(|(t, _)| t == b"hhea")
        .map(|(_, d)| u16::from_be_bytes([d[34], d[35]]) as usize)
        .unwrap_or(1);
    if let Some((_, hmtx)) = tables.iter_mut().find(|(t, _)| t == b"hmtx") {
        for i in 0..hhea_metrics {
            let at = i * 4;
            if at + 2 <= hmtx.len() {
                hmtx[at..at + 2].copy_from_slice(&advance_units.to_be_bytes());
            }
        }
    }
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
}

/// Default `emoji_family() == None` produces byte-identical glyph IDs, font families, colour
/// counts, and widths compared to eager fallback across all text categories, including RTL +
/// emoji, and never loads system emoji on non-emoji text.
#[test]
fn default_emoji_family_none_preserves_glyphs_fonts_and_lazy_loading() {
    let style = TextStyle::new(16.0, 400.0);
    let samples = [
        "Item - next threshold >= 95%",
        "Item · next… threshold ≥ 95%",
        "Hello 世界 日本語 한국어",
        "Launch 🚀",
        "Fly ✈️",
        "Heart ❤️",
        "Keycap 1\u{FE0F}\u{20E3}",
        "Flag 🏳\u{FE0F}\u{200D}🌈 Dev 👩🏽\u{200D}💻",
        "Brazil 🇧🇷",
        "مرحبا 🚀 🇧🇷",
        "Heart ❤",
        "Smile ☺\u{FE0E}",
        "Copy ⌘K",
        "Let 𝐀𝐁𝐂 𝑥𝑦𝑧 ℝℂ",
        "Missing \u{0378} end",
    ];

    let mut plain = TextSystem::new_local(&[], ("Inter", "ui-monospace"));
    assert_eq!(plain.emoji_family(), None);
    let epoch_before = plain.shaped_epoch;
    // Clearing when already None is a no-op.
    assert!(!plain.set_emoji_family(None));
    assert!(!plain.set_emoji_family(Some("")));
    assert!(!plain.set_emoji_family(Some("   ")));
    assert_eq!(plain.shaped_epoch, epoch_before);

    // Covered non-emoji text (ASCII, punctuation, monochrome heart) keeps system emoji unloaded.
    let _ = glyph_signature(&mut plain, "Hello world 0123 · … ≥ ❤", &[], style);
    assert!(plain.emoji.is_none(), "covered non-emoji text must not load system emoji");

    let mut eager = TextSystem::new_local(&[], ("Inter", "ui-monospace"));
    eager.load_symbol_fonts();
    let _ = eager.layout_rich("ℝ", &[], style, 2.0, None);
    for sample in samples {
        let mut fresh = TextSystem::new_local(&[], ("Inter", "ui-monospace"));
        assert_eq!(fresh.emoji_family(), None);
        let got = glyph_signature(&mut fresh, sample, &[], style);
        let want = glyph_signature(&mut eager, sample, &[], style);
        assert_eq!(got, want, "default emoji_family(None) diverged for {sample:?}");
    }
}

/// `set_emoji_family` puts the opt-in family ahead of system emoji across `Family::Sans`,
/// `Family::Mono`, `Family::Named`, inline mono spans, and RTL runs, falls back to system emoji
/// for uncovered emoji, and keeps plain text symbols on monochrome text fonts.
#[test]
fn set_emoji_family_precedes_system_emoji_across_slots_and_preserves_text_symbols() {
    let app_emoji = make_test_font(
        "App Emoji",
        &['\u{2764}', '\u{1F680}', '\u{1F1E7}', '\u{1F1F7}', '\u{1F3F3}', '\u{1F308}', '\u{20E3}'],
        true,
    );
    let sys_emoji = make_test_font(
        "Sys Emoji",
        &['\u{2764}', '\u{1F680}', '\u{1F1E7}', '\u{1F1F7}', '\u{1F3F3}', '\u{1F308}', '\u{1F389}'],
        true,
    );
    let style = TextStyle::new(16.0, 400.0);

    let mut ts = TextSystem::new_local(&[], ("Inter", "ui-monospace"));
    register_test_font(&mut ts, app_emoji);
    register_test_font(&mut ts, sys_emoji);
    ts.emoji = Some("Sys Emoji");
    ts.push_symbol_family("Sys Emoji", true);
    ts.fallbacks.symbols = true;
    ts.fallbacks.asked.insert('\u{FE0F}');
    ts.apply_emoji_family(Some("App Emoji"));
    assert_eq!(ts.emoji_family(), Some("App Emoji"));

    // Sans slot does not mention "App Emoji", yet every Route::Emoji sample shapes from it.
    for (sample, label) in [
        ("Launch 🚀", "default presentation"),
        ("Heart ❤️", "explicit U+FE0F"),
        ("Brazil 🇧🇷", "regional indicator flag"),
        ("Flag 🏳\u{FE0F}\u{200D}🌈", "ZWJ sequence"),
        ("مرحبا 🚀 🇧🇷", "RTL with emoji and flag"),
    ] {
        let (glyphs, _, _) = glyph_signature(&mut ts, sample, &[], style);
        assert!(
            glyphs.iter().any(|(fam, gid)| fam == "App Emoji" && *gid != 0),
            "{label} ({sample:?}) must shape with App Emoji: {glyphs:?}"
        );
        assert!(
            !glyphs.iter().any(|(fam, _)| fam == "Sys Emoji"),
            "{label} ({sample:?}) must not be overridden by Sys Emoji: {glyphs:?}"
        );
    }

    // Mono slot, Family::Named, and inline mono spans all follow preferred_emoji too.
    let (mono_glyphs, _, _) = glyph_signature(&mut ts, "Launch 🚀", &[], style.mono());
    assert!(mono_glyphs.iter().any(|(fam, gid)| fam == "App Emoji" && *gid != 0), "Family::Mono: {mono_glyphs:?}");
    assert!(!mono_glyphs.iter().any(|(fam, _)| fam == "Sys Emoji"), "Family::Mono: {mono_glyphs:?}");

    let (named_glyphs, _, _) = glyph_signature(&mut ts, "Brazil 🇧🇷", &[], style.family(Family::Named("Inter")));
    assert!(named_glyphs.iter().any(|(fam, gid)| fam == "App Emoji" && *gid != 0), "Family::Named: {named_glyphs:?}");
    assert!(!named_glyphs.iter().any(|(fam, _)| fam == "Sys Emoji"), "Family::Named: {named_glyphs:?}");

    let text = "a 🚀 b";
    let span = [Span::new(0, text.len()).mono()];
    let (span_glyphs, _, _) = glyph_signature(&mut ts, text, &span, style);
    assert!(span_glyphs.iter().any(|(fam, gid)| fam == "App Emoji" && *gid != 0), "mono span: {span_glyphs:?}");
    assert!(!span_glyphs.iter().any(|(fam, _)| fam == "Sys Emoji"), "mono span: {span_glyphs:?}");

    // Uncovered emoji falls back to Sys Emoji.
    let (party, _, _) = glyph_signature(&mut ts, "Party 🎉", &[], style);
    assert!(party.iter().any(|(fam, gid)| fam == "Sys Emoji" && *gid != 0), "uncovered emoji must fall back: {party:?}");

    // Plain text symbols without U+FE0F stay on monochrome text faces (App Emoji is excluded).
    let (plain_heart, _, _) = glyph_signature(&mut ts, "Heart ❤", &[], style);
    assert!(
        plain_heart.iter().all(|(fam, gid)| fam == "Inter" && *gid != 0),
        "plain text heart must stay on Inter: {plain_heart:?}"
    );
    let (text_smile, _, _) = glyph_signature(&mut ts, "Smile ☺\u{FE0E}", &[], style);
    assert!(
        !text_smile.iter().any(|(fam, _)| fam == "App Emoji"),
        "U+FE0E text presentation must not use preferred_emoji: {text_smile:?}"
    );

    // Clearing restores Sys Emoji.
    ts.apply_emoji_family(None);
    assert_eq!(ts.emoji_family(), None);
    let (restored, _, _) = glyph_signature(&mut ts, "Launch 🚀", &[], style);
    assert!(restored.iter().any(|(fam, gid)| fam == "Sys Emoji" && *gid != 0), "cleared emoji_family must restore Sys Emoji: {restored:?}");
    assert!(!restored.iter().any(|(fam, _)| fam == "App Emoji"));
}

/// Explicit `emoji_family` takes precedence over a colour font named in the active family stack
/// (PR #2050), while emoji uncovered by `emoji_family` fall back to the active-stack colour font
/// first (still without loading system emoji), and then to the system emoji font.
#[test]
fn explicit_emoji_family_precedes_selected_active_stack_color_font() {
    let explicit = make_test_font("Explicit Emoji", &['\u{1F680}', '\u{1F1E7}', '\u{1F1F7}'], true);
    let stack_color = make_test_font("Stack Color", &['\u{1F680}', '\u{1F1E7}', '\u{1F1F7}', '\u{2764}'], true);
    let sys_emoji = make_test_font("Sys Emoji", &['\u{1F680}', '\u{2764}', '\u{1F389}'], true);
    let style = TextStyle::new(16.0, 400.0);

    let mut ts = TextSystem::new_local(&[], ("Inter", "ui-monospace"));
    register_test_font(&mut ts, explicit);
    register_test_font(&mut ts, stack_color);
    register_test_font(&mut ts, sys_emoji);
    ts.apply_families(r#"Inter, "Stack Color""#, "ui-monospace");
    ts.apply_emoji_family(Some("Explicit Emoji"));

    // 1. Covered by Explicit Emoji -> Explicit Emoji wins over Stack Color and never loads system emoji.
    for sample in ["Launch 🚀", "Brazil 🇧🇷"] {
        let (glyphs, _, _) = glyph_signature(&mut ts, sample, &[], style);
        assert!(glyphs.iter().any(|(fam, gid)| fam == "Explicit Emoji" && *gid != 0), "{sample:?}: {glyphs:?}");
        assert!(!glyphs.iter().any(|(fam, _)| fam == "Stack Color"), "{sample:?}: {glyphs:?}");
        assert!(ts.emoji.is_none(), "covered {sample:?} must not load system emoji");
    }

    // Also wins when Stack Color is named via Family::Named.
    let (named, _, _) = glyph_signature(&mut ts, "Launch 🚀", &[], style.family(Family::Named("Stack Color")));
    assert!(named.iter().any(|(fam, gid)| fam == "Explicit Emoji" && *gid != 0), "Family::Named: {named:?}");
    assert!(!named.iter().any(|(fam, _)| fam == "Stack Color"), "Family::Named: {named:?}");

    // 2. Absent from Explicit Emoji, present in Stack Color -> falls back to Stack Color, still no system emoji load.
    let (heart, _, _) = glyph_signature(&mut ts, "Heart ❤️", &[], style);
    assert!(heart.iter().any(|(fam, gid)| fam == "Stack Color" && *gid != 0), "fallback to Stack Color: {heart:?}");
    assert!(ts.emoji.is_none(), "Stack Color fallback must still keep system emoji unloaded");

    // 3. Absent from both -> falls back to system emoji.
    ts.emoji = Some("Sys Emoji");
    ts.push_symbol_family("Sys Emoji", true);
    ts.fallbacks.asked.insert('\u{FE0F}');
    let (party, _, _) = glyph_signature(&mut ts, "Party 🎉", &[], style);
    assert!(party.iter().any(|(fam, gid)| fam == "Sys Emoji" && *gid != 0), "fallback to Sys Emoji: {party:?}");
}

/// Preserves #2008 lazy loading: setting `emoji_family` never loads system emoji on ASCII/non-emoji
/// text, and when `preferred_emoji` covers every emoji in the text, system `self.emoji` stays
/// unloaded until an uncovered emoji appears.
#[test]
fn preferred_emoji_preserves_lazy_system_emoji_loading() {
    let app_emoji = make_test_font("App Emoji", &['\u{2764}', '\u{1F680}', '\u{1F1E7}', '\u{1F1F7}'], true);
    let style = TextStyle::new(16.0, 400.0);

    let mut ts = TextSystem::new_local(&[], ("Inter", "ui-monospace"));
    register_test_font(&mut ts, app_emoji);
    ts.apply_emoji_family(Some("App Emoji"));
    assert!(ts.emoji.is_none(), "setting emoji_family must not load system emoji");
    assert!(ts.font_files.is_empty(), "setting emoji_family must not open system font files");

    // ASCII and plain text symbols open no system font files and leave ts.emoji None.
    let _ = glyph_signature(&mut ts, "Hello 0123 ❤", &[], style);
    assert!(ts.emoji.is_none());
    assert!(ts.font_files.is_empty());

    // Covered emoji shapes from App Emoji without loading system emoji.
    for sample in ["Launch 🚀", "Heart ❤️", "Brazil 🇧🇷"] {
        let (glyphs, _, _) = glyph_signature(&mut ts, sample, &[], style);
        assert!(ts.emoji.is_none(), "covered {sample:?} must not load system emoji: {:?}", ts.emoji);
        assert!(ts.fallbacks.asked.is_empty(), "covered {sample:?} must not ask fallback: {:?}", ts.fallbacks.asked);
        assert!(glyphs.iter().any(|(fam, gid)| fam == "App Emoji" && *gid != 0), "{sample:?}: {glyphs:?}");
    }

    // Uncovered emoji triggers lazy system emoji lookup.
    let _ = glyph_signature(&mut ts, "Party 🎉", &[], style);
    assert!(ts.fallbacks.asked.contains(&'\u{FE0F}'), "uncovered emoji must trigger system emoji lookup");
}

/// Names with digits, spaces, quotes, CSS fallback lists, and missing names all resolve cleanly.
#[test]
fn emoji_family_parsing_digits_quotes_lists_and_missing_fallback() {
    let digits = make_test_font("Twemoji 17", &['\u{1F680}'], true);
    let backup = make_test_font("Backup Emoji", &['\u{1F389}'], true);
    let sys = make_test_font("Sys Emoji", &['\u{1F680}', '\u{1F389}', '\u{2764}'], true);
    let style = TextStyle::new(16.0, 400.0);

    let mut ts = TextSystem::new_local(&[], ("Inter", "ui-monospace"));
    register_test_font(&mut ts, digits);
    register_test_font(&mut ts, backup);
    register_test_font(&mut ts, sys);
    ts.emoji = Some("Sys Emoji");
    ts.push_symbol_family("Sys Emoji", true);
    ts.fallbacks.asked.insert('\u{FE0F}');

    // Unquoted name with space and digits.
    ts.apply_emoji_family(Some("  Twemoji 17  "));
    assert_eq!(ts.emoji_family(), Some("Twemoji 17"));
    let (g, _, _) = glyph_signature(&mut ts, "🚀", &[], style);
    assert!(g.iter().any(|(f, id)| f == "Twemoji 17" && *id != 0), "digits name: {g:?}");

    // Quoted single name.
    ts.apply_emoji_family(Some(r#""Twemoji 17""#));
    let (g, _, _) = glyph_signature(&mut ts, "🚀", &[], style);
    assert!(g.iter().any(|(f, id)| f == "Twemoji 17" && *id != 0), "quoted name: {g:?}");

    // Comma-separated CSS fallback list tries families in order, then falls back to Sys Emoji.
    ts.apply_emoji_family(Some(r#""Twemoji 17", "Backup Emoji""#));
    let (rocket, _, _) = glyph_signature(&mut ts, "🚀", &[], style);
    assert!(rocket.iter().any(|(f, id)| f == "Twemoji 17" && *id != 0), "list first: {rocket:?}");
    let (party, _, _) = glyph_signature(&mut ts, "🎉", &[], style);
    assert!(party.iter().any(|(f, id)| f == "Backup Emoji" && *id != 0), "list second: {party:?}");
    let (heart, _, _) = glyph_signature(&mut ts, "❤️", &[], style);
    assert!(heart.iter().any(|(f, id)| f == "Sys Emoji" && *id != 0), "list fallback: {heart:?}");

    // Missing family falls back gracefully to Sys Emoji without panicking.
    ts.apply_emoji_family(Some("Rune Missing Emoji Family"));
    let (missing, _, _) = glyph_signature(&mut ts, "🚀", &[], style);
    assert!(missing.iter().any(|(f, id)| f == "Sys Emoji" && *id != 0), "missing fallback: {missing:?}");
}

/// `TextSystem::set_emoji_family` updates `font_epoch()` and `measure::measure`, is a no-op when
/// unchanged, and stays isolated inside `with_families`.
#[test]
fn set_emoji_family_syncs_font_epoch_and_measure() {
    let narrow = make_sized_test_font("Narrow Emoji Measure", &['\u{1F680}'], true, 500);
    let wide = make_sized_test_font("Wide Emoji Measure", &['\u{1F680}'], true, 2000);
    assert_eq!(crate::add_font(narrow), vec!["Narrow Emoji Measure"]);
    assert_eq!(crate::add_font(wide), vec!["Wide Emoji Measure"]);

    let style = TextStyle::new(16.0, 400.0);
    let _guard = crate::with_families("Inter", "ui-monospace");
    let mut ts = TextSystem::new(&[], ("Inter", "ui-monospace"));

    let ep0 = crate::font_epoch();
    assert!(ts.set_emoji_family(Some("Narrow Emoji Measure")));
    let ep1 = crate::font_epoch();
    assert_ne!(ep0, ep1, "changing emoji_family must advance font_epoch");
    assert!(!ts.set_emoji_family(Some("Narrow Emoji Measure")));
    assert_eq!(crate::font_epoch(), ep1, "repeating same emoji_family must not advance font_epoch");

    let narrow_w = crate::measure("🚀", &[], style, None).0;
    assert!(ts.set_emoji_family(Some("Wide Emoji Measure")));
    let ep2 = crate::font_epoch();
    assert_ne!(ep1, ep2);
    let wide_w = crate::measure("🚀", &[], style, None).0;
    assert!(
        wide_w > narrow_w * 2.0,
        "measure must reflect switched emoji_family widths: narrow={narrow_w}, wide={wide_w}"
    );

    assert!(ts.set_emoji_family(Some("")));
    assert_eq!(ts.emoji_family(), None);
    let ep3 = crate::font_epoch();
    assert_ne!(ep2, ep3);
}
