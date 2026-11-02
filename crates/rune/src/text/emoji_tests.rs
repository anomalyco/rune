use super::*;
use crate::text::{Span, TextSystem};
use crate::tok::{Family, TextStyle};

#[test]
fn default_emoji_and_sequences_route_whole_graphemes() {
    for sample in [
        "😀",
        "🎉",
        "👨‍👩‍👧‍👦",
        "👩🏽‍💻",
        "👍🏽",
        "🇪🇬",
        "1️⃣",
        "1\u{20E3}",
        "#️⃣",
        "❤️",
        "©️",
        "☺️",
        "🏴\u{e0067}\u{e0062}\u{e0065}\u{e006e}\u{e0067}\u{e007f}",
    ] {
        assert_eq!(
            routes(sample),
            vec![(0..sample.len(), Route::Emoji)],
            "{sample}"
        );
    }
    assert_eq!(routes("a😀🎉b"), vec![(1..9, Route::Emoji)]);
}

#[test]
fn text_presentation_and_ordinary_symbols_keep_their_fonts() {
    for sample in ["© ☺ ❤ ☀ ✔ 1 # *", "😀︎", "☺︎", "❤︎", "a العربية", "⎋ ⌘ ⇧"]
    {
        assert!(routes(sample).is_empty(), "{sample}");
    }
    assert_eq!(routes("😀︎😀️"), vec![(7..14, Route::Emoji)]);
}

#[test]
fn color_emoji_win_over_an_explicit_outline_font_including_spans() {
    let mut ts = TextSystem::default();
    if crate::text::system_font_files(&["notocoloremoji", "applecoloremoji", "seguiemj"]).is_empty()
        || ts.font_cx.collection.family_id("DejaVu Sans").is_none()
    {
        return;
    }
    for sample in ["😀", "👨‍👩‍👧‍👦", "👩🏽‍💻", "👍🏽", "🇪🇬", "1️⃣", "#️⃣", "©️", "☺️"]
    {
        for spans in [vec![], vec![Span::new(0, sample.len()).mono().bold()]] {
            let layout = ts.layout_rich(
                sample,
                &spans,
                TextStyle::new(20.0, 400.0).family(Family::Named("DejaVu Sans")),
                1.0,
                None,
            );
            let mut quads = Vec::new();
            ts.glyphs_for_layout(&layout, 1.0, [0.0, 0.0], false, &mut quads);
            assert_eq!(
                quads.iter().filter(|q| q.color != 0).count(),
                1,
                "{sample}: {quads:?}"
            );
            assert!(quads.iter().all(|q| q.color != 0), "{sample}: {quads:?}");
        }
    }
    for sample in ["©", "☺", "😀︎", "❤︎"] {
        let layout = ts.layout_rich(sample, &[], TextStyle::new(20.0, 400.0), 1.0, None);
        let mut quads = Vec::new();
        ts.glyphs_for_layout(&layout, 1.0, [0.0, 0.0], false, &mut quads);
        assert!(!quads.is_empty(), "{sample}");
        assert!(quads.iter().all(|q| q.color == 0), "{sample}: {quads:?}");
    }
}

#[test]
fn missing_emoji_family_falls_back_to_outline_coverage() {
    let mut ts = TextSystem::default();
    if ts.font_cx.collection.family_id("DejaVu Sans").is_none() {
        return;
    }
    ts.emoji = vec!["Unavailable Emoji Font".into()];
    ts.symbol_families.retain(|name| !name.contains("Emoji"));
    let layout = ts.layout_rich("😀", &[], TextStyle::new(20.0, 400.0), 1.0, None);
    let mut quads = Vec::new();
    ts.glyphs_for_layout(&layout, 1.0, [0.0, 0.0], false, &mut quads);
    for line in layout.lines() {
        for item in line.items() {
            if let parley::PositionedLayoutItem::GlyphRun(run) = item {
                assert!(run.glyphs().all(|glyph| glyph.id != 0));
            }
        }
    }
    assert_eq!(quads.len(), 1);
    assert_eq!(quads[0].color, 0);
}
