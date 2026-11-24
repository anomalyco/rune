//! Colour emoji in macOS's Apple Color Emoji, laid out as an emoji grid lays out each cell.
use super::*;

/// One of each kind of emoji sequence the font makes, by kind.
const CASES: &[(&str, &str)] = &[
    ("single", "\u{1F605}"),
    ("single", "\u{1FAE1}"),
    ("zwj", "\u{1F473}\u{200D}\u{2642}\u{FE0F}"),
    ("zwj", "\u{1F473}\u{200D}\u{2640}\u{FE0F}"),
    ("zwj", "\u{2764}\u{FE0F}\u{200D}\u{1F525}"),
    ("zwj", "\u{2764}\u{FE0F}\u{200D}\u{1FA79}"),
    ("zwj", "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}"),
    // Drawn as two half pictures, the right one a mark on the left one.
    ("zwj halves", "\u{1F469}\u{200D}\u{2764}\u{FE0F}\u{200D}\u{1F468}"),
    ("zwj halves", "\u{1FAF1}\u{1F3FB}\u{200D}\u{1FAF2}\u{1F3FC}"),
    ("skin tone", "\u{1F44B}\u{1F3FD}"),
    ("flag", "\u{1F3F3}\u{FE0F}\u{200D}\u{1F308}"),
    ("flag", "\u{1F1FA}\u{1F1F8}"),
    ("keycap", "1\u{FE0F}\u{20E3}"),
    // `flip` records: another picture mirrored.
    ("facing right", "\u{1F6B6}\u{200D}\u{27A1}\u{FE0F}"),
    ("facing right", "\u{1F3C3}\u{1F3FD}\u{200D}\u{27A1}\u{FE0F}"),
    // Unicode 17, in macOS 26.4 and later.
    ("newest", "\u{1FAEA}"),
    ("newest", "\u{1FAEF}"),
];

const EMOJI_FONT: &str = "/System/Library/Fonts/Apple Color Emoji.ttc";

#[test]
fn each_kind_of_emoji_draws_one_inked_colour_picture_in_one_advance() {
    let font = std::fs::read(EMOJI_FONT).expect("the system emoji font");
    let charmap = FontRef::from_index(&font, 0).expect("its first face").charmap();
    let mut ts = TextSystem::default();
    // An emoji grid's cell: the glyph at 78% of a 34 pt line, at 2x.
    let style = TextStyle::new(34.0 * 0.78, 400.0).lh(34.0);
    let em = style.size * 2.0;
    let mut failures = Vec::new();
    for &(kind, emoji) in CASES {
        if emoji.chars().any(|c| charmap.map(c) == 0) {
            eprintln!("skipped {kind} {emoji}: this macOS's emoji font hasn't got it");
            continue;
        }
        ts.begin_frame();
        let mut quads = Vec::new();
        ts.glyphs(emoji, style, 2.0, [0.0, 0.0], &mut quads);
        let (w, h) = ts.measure(emoji, style, 2.0);
        let (w, h) = (w * 2.0, h * 2.0);
        let mut fail = |why: String| failures.push(format!("{kind} {emoji}: {why}"));
        if quads.is_empty() || quads.iter().any(|q| q.color == 0) {
            fail(format!("{} glyphs, {} of them colour", quads.len(), quads.iter().filter(|q| q.color != 0).count()));
            continue;
        }
        if quads.iter().any(|q| ts.color_glyph(q.color).rgba().chunks(4).all(|p| p[3] == 0)) {
            fail("a picture with no ink".into());
        }
        if (w - em).abs() > 1.0 {
            fail(format!("{w} px wide, not one {em} px advance"));
        }
        if let Some(q) = quads.iter().find(|q| q.rect[0] < -0.5 || q.rect[1] < -0.5 || q.rect[0] + q.rect[2] > w + 0.5 || q.rect[1] + q.rect[3] > h + 0.5) {
            fail(format!("drawn at {:?}, past its {w} x {h} box", q.rect));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Every glyph with a picture in the font's `sbix` (a `png `, or a `dupe` or `flip` of one)
/// rasterizes in colour with ink.
#[test]
fn every_sbix_glyph_in_the_emoji_font_draws_in_colour() {
    let data = std::fs::read(EMOJI_FONT).expect("the system emoji font");
    let font = FontRef::from_index(&data, 0).expect("its first face");
    let sbix = font.table(swash::tag_from_bytes(b"sbix")).expect("an sbix table");
    let u32_at = |at: usize| u32::from_be_bytes(sbix[at..at + 4].try_into().unwrap()) as usize;
    let strikes: Vec<usize> = (0..u32_at(4)).map(|s| u32_at(8 + 4 * s)).collect();
    let has_picture = |g: usize| strikes.iter().any(|&s| u32_at(s + 8 + 4 * g) > u32_at(s + 4 + 4 * g) + 8);
    let mut cx = swash::scale::ScaleContext::new();
    let mut scaler = cx.builder(font).size(20.0).build();
    let (mut drawn, mut blank) = (0, Vec::new());
    for g in (0..font.metrics(&[]).glyph_count).filter(|&g| has_picture(g as usize)) {
        match crate::raster::color::rasterize_color(&mut scaler, font, g, 20.0) {
            Some((pixels, _)) if pixels.rgba().chunks(4).any(|p| p[3] != 0) => drawn += 1,
            _ => blank.push(g),
        }
    }
    assert!(blank.is_empty(), "{} of {} glyphs with a picture draw nothing: {:?}", blank.len(), drawn + blank.len(), &blank[..blank.len().min(20)]);
    assert!(drawn > 3000, "only {drawn} pictures");
}
