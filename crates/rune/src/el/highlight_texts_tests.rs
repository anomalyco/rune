use crate::text::TextHighlight;
use crate::{Color, Link, Span, TextEdit, TextStyle, col, el, rich, text, text_input};

fn matches(query: &'static str) -> impl FnMut(&str) -> Vec<TextHighlight> {
    move |s: &str| s.match_indices(query).map(|(at, m)| TextHighlight::new(at..at + m.len(), Color::WHITE)).collect()
}

fn ranges(e: &crate::El) -> Vec<(u32, u32)> {
    e.text.as_ref().map_or_else(Vec::new, |t| t.cold.highlights.iter().map(|h| (h.start, h.end)).collect())
}

#[test]
fn highlights_matches_in_nested_text_and_rich_runs() {
    let page = col()
        .child(text("dark mode"))
        .child(el().child(rich("follow the dark setting, dark", vec![Span::new(0, 6).weight(600.0)]).link(Link::new(11..15, "dark"))))
        .child(text("light"))
        .highlight_texts(matches("dark"));
    assert_eq!(ranges(&page.children[0]), vec![(0, 4)]);
    assert_eq!(ranges(&page.children[1].children[0]), vec![(11, 15), (25, 29)]);
    assert!(ranges(&page.children[2]).is_empty());
}

#[test]
fn skips_fields_secure_text_and_ranges_that_do_not_fit() {
    let mut seen = Vec::new();
    let page = col()
        .child(text("secret dark").secure())
        .child(text_input(&TextEdit::new("dark field"), TextStyle::new(14.0, 400.0)))
        .child(text("é dark"))
        .highlight_texts(|s| {
            seen.push(s.to_owned());
            [TextHighlight::new(1..2, Color::WHITE), TextHighlight::new(3..7, Color::WHITE), TextHighlight::new(3..99, Color::WHITE), TextHighlight::new(4..4, Color::WHITE)]
        });
    assert_eq!(seen, vec!["é dark".to_owned()]);
    assert!(ranges(&page.children[0]).is_empty());
    assert!(ranges(&page.children[1]).is_empty());
    assert_eq!(ranges(&page.children[2]), vec![(3, 7)]);
}
