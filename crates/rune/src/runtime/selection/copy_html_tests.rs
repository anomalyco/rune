use super::*;
use crate::el::TextSpec;
use crate::{Color, Cx, El, Env, Link, Runtime, Span, col, rich};

fn color_runs_spec(lines: usize) -> TextSpec {
    let mut spec = TextSpec::new("AB\n".repeat(lines));
    spec.spans = (0..lines)
        .flat_map(|i| [
            Span::new(3 * i, 3 * i + 1).color(Color::BLACK),
            Span::new(3 * i + 1, 3 * i + 2).color(Color::WHITE),
        ])
        .collect();
    spec
}

#[test]
fn color_only_spans_do_not_rescan_spans_at_every_boundary() {
    let lines = 16_384;
    let spec = color_runs_spec(lines);
    let span_count = spec.spans.len();
    let (html, visits) = count_span_visits(|| inline_html(&spec, 0, spec.content.len()));
    assert_eq!(html, "AB<br>".repeat(lines));
    assert_eq!(
        visits, span_count,
        "paint-only spans should be visited once and skipped, not rescanned (got {visits} for {span_count} spans)",
    );
}

#[test]
fn overlapping_markup_spans_match_existing_html_byte_for_byte() {
    // Bytes: "0123456789" (0..10), "<&>\"\n" (10..15), "xy" (15..17), "é" (17..19), "あ" (19..22), "z" (22..23).
    let content = "0123456789<&>\"\nxyéあz".to_owned();
    let len = content.len();
    let mut spec = TextSpec::new(content);
    spec.spans = vec![
        // Overlapping bold (0..8), italic (4..12), mono (6..15), and a second bold (10..19).
        Span::new(0, 8).bold(),
        Span::new(4, 12).italic(),
        Span::new(6, 15).mono(),
        Span::new(10, 19).weight(700.0),
        // Non-bold weight below 600 and paint-only styling emit no markup.
        Span::new(0, len).weight(500.0),
        // Span covering multi-byte UTF-8 characters.
        Span::new(17, len).italic(),
    ];
    spec.cold.strikes = vec![(2, 10, None), (15, len as u32, Some(Color::BLACK))];
    // Non-URL link at index 0 shadows URL link at index 1 on their overlap (0..4).
    spec.cold.links = vec![
        Link::new(0..4, "chip-id"),
        Link::new(0..10, "https://example.com/?a=1&b=2"),
        Link::new(10..15, "mailto:hi@example.com"),
    ];

    let expected = concat!(
        "<strong>01</strong>",
        "<strong><s>23</s></strong>",
        "<a href=\"https://example.com/?a=1&amp;b=2\">",
        "<strong><em><s>45</s></em></strong>",
        "<strong><em><s><code>67</code></s></em></strong>",
        "<em><s><code>89</code></s></em>",
        "</a>",
        "<a href=\"mailto:hi@example.com\">",
        "<strong><em><code>&lt;&amp;</code></em></strong>",
        "<strong><code>&gt;&quot;<br></code></strong>",
        "</a>",
        "<strong><s>xy</s></strong>",
        "<strong><em><s>é</s></em></strong>",
        "<em><s>あz</s></em>",
    );
    let actual = inline_html(&spec, 0, len);
    assert_eq!(actual, expected);

    // Partial selection inside overlapping spans preserves outer active marks.
    let partial = inline_html(&spec, 5, 13);
    let expected_partial = concat!(
        "<a href=\"https://example.com/?a=1&amp;b=2\">",
        "<strong><em><s>5</s></em></strong>",
        "<strong><em><s><code>67</code></s></em></strong>",
        "<em><s><code>89</code></s></em>",
        "</a>",
        "<a href=\"mailto:hi@example.com\">",
        "<strong><em><code>&lt;&amp;</code></em></strong>",
        "<strong><code>&gt;</code></strong>",
        "</a>",
    );
    assert_eq!(partial, expected_partial);

    // Mid-character endpoint (byte 18 inside 2-byte 'é') matches original is_char_boundary semantics.
    spec.spans[3].end = 18;
    let expected_mid_char = concat!(
        "<strong>01</strong>",
        "<strong><s>23</s></strong>",
        "<a href=\"https://example.com/?a=1&amp;b=2\">",
        "<strong><em><s>45</s></em></strong>",
        "<strong><em><s><code>67</code></s></em></strong>",
        "<em><s><code>89</code></s></em>",
        "</a>",
        "<a href=\"mailto:hi@example.com\">",
        "<strong><em><code>&lt;&amp;</code></em></strong>",
        "<strong><code>&gt;&quot;<br></code></strong>",
        "</a>",
        "<strong><s>xy</s></strong>",
        "<em><s>éあz</s></em>",
    );
    assert_eq!(inline_html(&spec, 0, len), expected_mid_char);
}

#[test]
fn many_overlapping_markup_spans_sweep_linearly() {
    let n = 512;
    let content = "a".repeat(n + 4);
    let mut spec = TextSpec::new(content);
    for i in 0..n {
        let sp = match i % 3 {
            0 => Span::new(i, i + 4).bold(),
            1 => Span::new(i, i + 4).italic(),
            _ => Span::new(i, i + 4).mono(),
        };
        spec.spans.push(sp);
    }
    let span_count = spec.spans.len();
    let (html, visits) = count_span_visits(|| inline_html(&spec, 0, spec.content.len()));

    assert!(
        html.starts_with(
            "<strong>a</strong><strong><em>a</em></strong><strong><em><code>a</code></em></strong><strong><em><code>a</code></em></strong>",
        ),
        "unexpected prefix: {}",
        &html[..html.len().min(160)],
    );
    assert!(
        visits <= 3 * span_count,
        "overlapping markup spans should sweep linearly, not quadratically (got {visits} for {span_count} spans)",
    );
}

#[test]
fn base_style_redacted_uncopied_and_non_char_boundary_spans() {
    // Heading base weight (>= 600) suppresses <strong>; italic base suppresses <em>.
    let mut heading = TextSpec::new("Hello world".to_owned());
    heading.style.weight = 700.0;
    heading.style.italic = true;
    heading.spans = vec![
        Span::new(0, 5).bold(),
        Span::new(0, 11).italic(),
        Span::new(6, 11).mono(),
    ];
    let (html, visits) = count_span_visits(|| inline_html(&heading, 0, heading.content.len()));
    assert_eq!(html, "Hello <code>world</code>");
    assert!(visits <= 3 * heading.spans.len());

    // Paint-only spans inside a bold span add no extra boundaries.
    let mut painted_bold = TextSpec::new("abcdef".to_owned());
    painted_bold.spans = vec![
        Span::new(0, 6).bold(),
        Span::new(0, 2).color(Color::BLACK),
        Span::new(2, 4).bg(Color::WHITE, 2.0),
        Span::new(4, 6).underline(Color::BLACK).size(16.0),
    ];
    let (html, visits) = count_span_visits(|| inline_html(&painted_bold, 0, painted_bold.content.len()));
    assert_eq!(html, "<strong>abcdef</strong>");
    assert!(visits <= painted_bold.spans.len() + 2);

    // Redacted text emits nothing.
    let mut secret = TextSpec::new("secret".to_owned());
    secret.redacted = true;
    secret.spans = vec![Span::new(0, 6).bold()];
    assert_eq!(inline_html(&secret, 0, 6), "");

    // Uncopied slice inside a link does not split or close the link prematurely.
    let mut uncopied = TextSpec::new("ab_cd".to_owned());
    uncopied.cold.uncopied = vec![(2, 3)];
    uncopied.cold.links = vec![Link::new(0..5, "https://example.com")];
    uncopied.spans = vec![Span::new(0, 2).bold(), Span::new(3, 5).bold()];
    assert_eq!(
        inline_html(&uncopied, 0, 5),
        "<a href=\"https://example.com\"><strong>ab</strong><strong>cd</strong></a>",
    );

    // Non-char-boundary span endpoints never panic or underflow.
    let mut utf8 = TextSpec::new("éé".to_owned());
    utf8.spans = vec![Span::new(1, 3).bold(), Span::new(1, 4).italic()];
    assert_eq!(inline_html(&utf8, 0, utf8.content.len()), "éé");
}

#[test]
fn runtime_copy_html_matches_selectable_rich_text() {
    let lines = 64;
    let spans: Vec<Span> = (0..lines)
        .flat_map(|i| [
            Span::new(3 * i, 3 * i + 1).color(Color::BLACK),
            Span::new(3 * i + 1, 3 * i + 2).color(Color::WHITE),
        ])
        .collect();
    let span_count = spans.len();
    let content = "AB\n".repeat(lines);
    let state = (content, spans);

    fn view(s: &(String, Vec<Span>), _: &mut Cx<(String, Vec<Span>)>) -> El {
        col().size(200.0, 200.0).child(
            rich(s.0.clone(), s.1.clone())
                .key("doc")
                .by_line()
                .selectable(true),
        )
    }

    let mut rt = Runtime::new(state, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|_, tx| tx.select_all_text("doc"));
    rt.frame(0.0);

    let (html, visits) = count_span_visits(|| rt.core().copy_html());
    assert_eq!(html.as_deref(), Some(format!("<p>{}</p>", "AB<br>".repeat(lines)).as_str()));
    assert_eq!(visits, span_count);
}
