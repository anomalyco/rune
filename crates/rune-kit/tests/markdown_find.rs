//! Tests for `rune_kit::markdown::find` and `Case`.
#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]
#![cfg(feature = "markdown")]

use std::ops::Range;

use rune::{Cx, El, Env, Runtime, ScrollSpec, col};
use rune_kit::markdown::{Case, Highlight, find};
use rune_kit::{Kit, Markdown, MarkdownStyle};

fn slice_all<'a>(src: &'a str, matches: &[Vec<Range<usize>>]) -> Vec<Vec<&'a str>> {
    matches.iter().map(|m| m.iter().map(|r| &src[r.clone()]).collect()).collect()
}

#[test]
fn matches_across_bold_italic_code_strike_and_links() {
    let src = "a **b**";
    let m = find(src, "a b", Case::Sensitive);
    assert_eq!(m, vec![vec![0..2, 4..5]]);
    assert_eq!(slice_all(src, &m), vec![vec!["a ", "b"]]);

    let rich = "Start *italic* and **bold** and `code` and ~~struck~~ and [linked words](https://example.org/path) end.";
    let m = find(
        rich,
        "Start italic and bold and code and struck and linked words end.",
        Case::Sensitive,
    );
    assert_eq!(
        slice_all(rich, &m),
        vec![vec![
            "Start ",
            "italic",
            " and ",
            "bold",
            " and ",
            "code",
            " and ",
            "struck",
            " and ",
            "linked words",
            " end.",
        ]]
    );

    // Nested inline formatting inside a link.
    let nested = "See [**bold *em* `code`**](https://example.org/u) now";
    let m = find(nested, "See bold em code now", Case::Sensitive);
    assert_eq!(slice_all(nested, &m), vec![vec!["See ", "bold ", "em", " ", "code", " now"]]);

    // Multiline inline code inside a quote and list item.
    let quoted = "> prefix `one\n> two` suffix\n\n- item `three\n  four` tail";
    let m1 = find(quoted, "prefix one two suffix", Case::Sensitive);
    assert_eq!(slice_all(quoted, &m1), vec![vec!["prefix ", "one\n", "two", " suffix"]]);
    let m2 = find(quoted, "item three four tail", Case::Sensitive);
    assert_eq!(slice_all(quoted, &m2), vec![vec!["item ", "three\n", "four", " tail"]]);
}

#[test]
fn link_and_image_urls_are_not_matched_while_visible_text_autolinks_and_alt_text_are() {
    let src = concat!(
        "Read [the guide](https://example.org/secret-url) or [ref text][secret-ref].\n\n",
        "Bare https://example.org/bare-link and <https://example.org/angle-link> work.\n\n",
        "Look at ![diagram alt text](https://example.org/hidden-image.png) and ![](https://example.org/empty-alt.png).\n\n",
        "[secret-ref]: https://example.org/ref-target\n",
    );

    for hidden in ["secret-url", "secret-ref", "ref-target", "hidden-image", "empty-alt", ".png"] {
        assert!(find(src, hidden, Case::Insensitive).is_empty(), "{hidden:?} must not match");
    }

    let guide = find(src, "Read the guide or ref text.", Case::Sensitive);
    assert_eq!(slice_all(src, &guide), vec![vec!["Read ", "the guide", " or ", "ref text", "."]]);

    let auto = find(
        src,
        "Bare https://example.org/bare-link and https://example.org/angle-link work.",
        Case::Sensitive,
    );
    assert_eq!(
        slice_all(src, &auto),
        vec![vec![
            "Bare https://example.org/bare-link and ",
            "https://example.org/angle-link",
            " work.",
        ]]
    );

    let img = find(src, "Look at diagram alt text and .", Case::Sensitive);
    assert_eq!(slice_all(src, &img), vec![vec!["Look at ", "diagram alt text", " and ", "."]]);
}

#[test]
fn matches_never_cross_block_boundaries_but_cross_soft_breaks_and_search_code_blocks() {
    let src = concat!(
        "# Heading One\n\n",
        "First paragraph\ncontinues on next line.\n\n",
        "Second paragraph.\n\n",
        "- item alpha\n",
        "- [x] item beta\n\n",
        "> quoted first\n>\n",
        "> quoted second\n\n",
        "| Head A | Head B |\n",
        "|---|---|\n",
        "| cell one | cell two |\n\n",
        "```rust\nfn compute() -> i32 {\n    42\n}\n```\n\n",
        "```diff\n-old_val\n+new_val\n```\n",
    );

    for across in [
        "Heading One First",
        "Heading One\nFirst",
        "line. Second",
        "line.\nSecond",
        "item alpha item beta",
        "item alpha\nitem beta",
        "[x] item beta",
        "quoted first quoted second",
        "Head A Head B",
        "Head B cell one",
        "cell one cell two",
        "rust",
        "diff",
        "```",
    ] {
        assert!(find(src, across, Case::Insensitive).is_empty(), "{across:?} must not match");
    }

    // Soft break inside a paragraph matches as a space and merges adjacent source ranges.
    let soft = find(src, "First paragraph continues", Case::Sensitive);
    assert_eq!(soft.len(), 1);
    assert_eq!(slice_all(src, &soft), vec![vec!["First paragraph\ncontinues"]]);

    // Soft break inside a blockquote skips the continuation `> ` marker.
    let bq = "> line one\n> line two";
    assert_eq!(slice_all(bq, &find(bq, "line one line two", Case::Sensitive)), vec![vec!["line one\n", "line two"]]);

    // Hard break matches as a newline.
    let hard = "line one  \nline two";
    assert_eq!(slice_all(hard, &find(hard, "line one\nline two", Case::Sensitive)), vec![vec!["line one  \nline two"]]);

    // Code block and diff block contents match in document order.
    let code_hit = find(src, "fn compute() -> i32 {\n    42", Case::Sensitive);
    assert_eq!(slice_all(src, &code_hit), vec![vec!["fn compute() -> i32 {\n    42"]]);
    let diff_hit = find(src, "-old_val\n+new_val", Case::Sensitive);
    assert_eq!(slice_all(src, &diff_hit), vec![vec!["-old_val\n+new_val"]]);

    // Pending trailing table header cells match individually without crossing cells.
    let pending = "Intro\n\n| alpha | **beta** |";
    assert!(find(pending, "alpha beta", Case::Sensitive).is_empty());
    assert_eq!(slice_all(pending, &find(pending, "beta", Case::Sensitive)), vec![vec!["beta"]]);
}

#[test]
fn entities_and_escapes_match_rendered_characters_and_merge_when_adjacent() {
    let src = "AT&amp;T &mdash; \\*literal\\* and &#x26; plus \\\\ end";
    assert!(find(src, "amp", Case::Insensitive).is_empty());
    assert!(find(src, "mdash", Case::Insensitive).is_empty());
    assert!(find(src, "\\*", Case::Insensitive).is_empty());

    // Entities merge with surrounding literal characters when adjacent in source.
    let att = find(src, "AT&T", Case::Sensitive);
    assert_eq!(att, vec![vec![0..8]]);
    assert_eq!(&src[att[0][0].clone()], "AT&amp;T");

    // Backslash escapes omit the escape `\` byte (just as inline markers are omitted).
    let star = find(src, "*literal*", Case::Sensitive);
    assert_eq!(slice_all(src, &star), vec![vec!["*literal", "*"]]);

    let whole = find(src, "AT&T — *literal* and & plus \\ end", Case::Sensitive);
    assert_eq!(
        slice_all(src, &whole),
        vec![vec!["AT&amp;T &mdash; ", "*literal", "* and &#x26; plus ", "\\ end"]]
    );

    // Escaped pipes in tables and inline code inside tables.
    let tbl = "| a\\|b | `c\\|d` |\n|---|---|\n| e\\|f | g |";
    assert_eq!(slice_all(tbl, &find(tbl, "a|b", Case::Sensitive)), vec![vec!["a", "|b"]]);
    assert_eq!(slice_all(tbl, &find(tbl, "c|d", Case::Sensitive)), vec![vec!["c\\|d"]]);
    assert_eq!(slice_all(tbl, &find(tbl, "e|f", Case::Sensitive)), vec![vec!["e", "|f"]]);
}

#[test]
fn multibyte_and_case_folding_keep_valid_source_byte_ranges() {
    // 'İ' (2 UTF-8 bytes) lowercases to "i\u{307}" (3 UTF-8 bytes).
    // 'ẞ' (3 UTF-8 bytes) lowercases to 'ß' (2 UTF-8 bytes).
    let src = "İstanbul İİ und STRAẞE sowie **Straße** führen zu Café 🦀 שלום";

    assert!(find(src, "café", Case::Sensitive).is_empty());
    let cafe = find(src, "CAFÉ", Case::Insensitive);
    assert_eq!(slice_all(src, &cafe), vec![vec!["Café"]]);

    let shalom = find(src, "🦀 שלום", Case::Insensitive);
    assert_eq!(slice_all(src, &shalom), vec![vec!["🦀 שלום"]]);

    let istanbul = find(src, "İSTANBUL", Case::Insensitive);
    assert_eq!(istanbul, vec![vec![0.."İstanbul".len()]]);
    assert_eq!(slice_all(src, &istanbul), vec![vec!["İstanbul"]]);

    let dotted = find(src, "i\u{307}", Case::Insensitive);
    assert_eq!(slice_all(src, &dotted), vec![vec!["İ"], vec!["İ"], vec!["İ"]]);

    // Plain ASCII "i" does not match half of 'İ', so byte ranges never split a codepoint.
    let plain_i = find(src, "i", Case::Insensitive);
    assert_eq!(slice_all(src, &plain_i), vec![vec!["i"]]); // only the 'i' in "sowie"

    // Both 3-byte 'ẞ' and 2-byte 'ß' fold to 'ß' and keep their own source byte lengths.
    let strasse = find(src, "straße", Case::Insensitive);
    assert_eq!(slice_all(src, &strasse), vec![vec!["STRAẞE"], vec!["Straße"]]);
    let strasse_upper = find(src, "STRAẞE", Case::Insensitive);
    assert_eq!(slice_all(src, &strasse_upper), vec![vec!["STRAẞE"], vec!["Straße"]]);

    // Greek final sigma 'ς' folds with 'σ' and 'Σ'.
    let greek = "ΟΣ and ος and οσ";
    assert_eq!(
        slice_all(greek, &find(greek, "ος", Case::Insensitive)),
        vec![vec!["ΟΣ"], vec!["ος"], vec!["οσ"]]
    );
    assert_eq!(
        slice_all(greek, &find(greek, "ΟΣ", Case::Insensitive)),
        vec![vec!["ΟΣ"], vec!["ος"], vec!["οσ"]]
    );
}

#[test]
fn adjacency_merging_and_non_overlapping_matches() {
    // Within one match, adjacent source segments merge; across formatting markers they stay split.
    let src = "a\r\nb &amp; **c** *d*";
    let m = find(src, "a b & c d", Case::Sensitive);
    assert_eq!(slice_all(src, &m), vec![vec!["a\r\nb &amp; ", "c", " ", "d"]]);

    // Separate matches never merge with each other and never overlap.
    let rep = "aaaa";
    let m = find(rep, "aa", Case::Sensitive);
    assert_eq!(m, vec![vec![0..2], vec![2..4]]);

    let overlap = "ababa";
    let m = find(overlap, "aba", Case::Sensitive);
    assert_eq!(m, vec![vec![0..3]]);
}

#[test]
fn empty_query_or_empty_source_returns_nothing() {
    assert!(find("Some **markdown** text", "", Case::Insensitive).is_empty());
    assert!(find("Some **markdown** text", "", Case::Sensitive).is_empty());
    assert!(find("", "markdown", Case::Insensitive).is_empty());
    assert!(find("", "", Case::Insensitive).is_empty());
}

#[test]
fn highlights_round_trip_paints_marks_and_scrolls_into_view() {
    let src = concat!(
        "# Search Header\n\n",
        "First **target phrase** in a paragraph with `target` code.\n\n",
        "- list *target phrase* item\n\n",
        "| Col |\n|---|\n| table **target** cell |\n\n",
        "```rust\nlet target_phrase = 1;\n```\n",
    );

    // 1. Direct round-trip: `Markdown::new(src).highlights(find(src, ...))`
    let direct_hits = find(src, "target phrase", Case::Insensitive);
    assert_eq!(direct_hits.len(), 2);
    assert_eq!(
        slice_all(src, &direct_hits),
        vec![vec!["target phrase"], vec!["target phrase"]]
    );

    // Cross-marker match: "First **target" and "list *target"
    let cross_hits = find(src, "target", Case::Insensitive);
    assert_eq!(cross_hits.len(), 5);

    struct App {
        src: String,
        query: String,
        active: usize,
    }

    fn app_view(s: &App, _: &mut Cx<App>) -> El {
        let k = Kit::neutral(true);
        let style = MarkdownStyle::from_kit(&k)
            .highlight_current_ring(Some((rune::color::hex("#f59e0b").unwrap(), 1.5)));
        let hits = find(&s.src, &s.query, Case::Insensitive);
        col()
            .key("scroll")
            .w(420.0)
            .h(120.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(
                Markdown::new(s.src.clone())
                    .style(style)
                    .highlights(
                        hits.iter()
                            .enumerate()
                            .map(|(i, m)| (m, Highlight::new().current_match(i == s.active))),
                    )
                    .build(&k),
            )
            .child(
                // Also verify passing `find(...)` directly without mapping.
                Markdown::new("Direct **match** test")
                    .highlights(Markdown::find("Direct **match** test", "Direct match", Case::Insensitive))
                    .build(&k)
                    .key("direct"),
            )
    }

    let mut rt = Runtime::new(
        App { src: src.to_owned(), query: "First target phrase".to_owned(), active: 0 },
        app_view,
        (420.0, 120.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    let marks: Vec<_> = rt
        .elements()
        .into_iter()
        .filter(|e| e.role == "mark")
        .map(|e| (e.key.unwrap_or_default(), e.label.unwrap_or_default()))
        .collect();
    // Both cross-marker matches ("First **target phrase**" and "Direct **match**") merge into one mark each.
    assert_eq!(
        marks,
        vec![
            ("md-mark-0".to_owned(), "First target phrase".to_owned()),
            ("md-mark-0".to_owned(), "Direct match".to_owned()),
        ]
    );

    // Switch query to "target" (5 matches across paragraph, inline code, list, table, and code block)
    // and scroll to the last match.
    rt.state.query = "target".to_owned();
    rt.state.active = 4;
    rt.invalidate();
    rt.frame(0.0);
    let labels: Vec<_> = rt
        .elements()
        .into_iter()
        .filter(|e| e.role == "mark" && !e.path.contains("direct"))
        .map(|e| (e.key.unwrap_or_default(), e.label.unwrap_or_default()))
        .collect();
    assert_eq!(
        labels,
        vec![
            ("md-mark-0".to_owned(), "target".to_owned()),
            ("md-mark-1".to_owned(), "target".to_owned()),
            ("md-mark-2".to_owned(), "target".to_owned()),
            ("md-mark-3".to_owned(), "target".to_owned()),
            ("md-mark-4".to_owned(), "target".to_owned()),
        ]
    );

    rt.update(|_, tx| tx.scroll_into_view(Markdown::highlight_key(4)));
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    let last_mark = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("md-mark-4")).unwrap();
    assert!(last_mark.rect[1] >= 0.0 && last_mark.rect[1] + last_mark.rect[3] <= 120.5, "{last_mark:?}");
}

#[test]
fn one_megabyte_document_searches_linearly() {
    let section = concat!(
        "## Section Heading\n\n",
        "Paragraph with **bold words**, *italic text*, `inline_code`, and [a link](https://example.org/ignored/needle).\n",
        "Continuation line after a soft break with İstanbul and Straße.\n\n",
        "- first list item\n",
        "- [x] second **needle** item\n\n",
        "```rust\nfn work(x: i32) -> i32 { x + 1 }\n```\n\n",
    );
    let repeats = (1_048_576 / section.len()) + 1;
    let doc = section.repeat(repeats);
    assert!(doc.len() >= 1_048_576);

    // "needle" appears once per section in rendered text (the link URL's `/needle` is ignored).
    let hits = find(&doc, "second needle item", Case::Insensitive);
    assert_eq!(hits.len(), repeats);
    assert_eq!(slice_all(&doc, &hits[..1]), vec![vec!["second ", "needle", " item"]]);
    assert_eq!(slice_all(&doc, &hits[repeats - 1..]), vec![vec!["second ", "needle", " item"]]);

    let sensitive = find(&doc, "Continuation line", Case::Sensitive);
    assert_eq!(sensitive.len(), repeats);
}

#[test]
fn frequent_matches_in_single_segment_are_linear() {
    let n = 200_000;
    let src = "a".repeat(n);
    let hits = find(&src, "a", Case::Sensitive);
    assert_eq!(hits.len(), n);
    assert_eq!(hits[0], vec![0..1]);
    assert_eq!(hits[n - 1], vec![n - 1..n]);
}

#[test]
fn rejected_unicode_mid_char_candidates_search_linearly() {
    let n = 200_000;
    let k = 2_000;
    let mut src = "İ".repeat(n);
    let query = format!("{}i", "i\u{307}".repeat(k));
    assert!(find(&src, &query, Case::Insensitive).is_empty());

    // Appending a real ASCII 'i' at the end lets the last k 'İ' chars + 'i' match on char boundaries.
    src.push('i');
    let hits = find(&src, &query, Case::Insensitive);
    assert_eq!(hits, vec![vec![(n - k) * "İ".len()..src.len()]]);
}

#[test]
fn unsorted_and_overlapping_multi_range_highlights_keep_all_ranges() {
    let mut rt = Runtime::new(
        "alpha beta gamma",
        |src, _| {
            let k = Kit::neutral(true);
            col()
                .w(400.0)
                .child(
                    Markdown::new(*src)
                        .highlights(vec![vec![11..16, 0..5], vec![2..5, 0..3]])
                        .build(&k),
                )
        },
        (400.0, 120.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    let marks: Vec<_> = rt
        .elements()
        .into_iter()
        .filter(|e| e.role == "mark")
        .map(|e| (e.key.unwrap_or_default(), e.label.unwrap_or_default()))
        .collect();
    assert_eq!(
        marks,
        vec![
            ("md-mark-0".to_owned(), "alpha".to_owned()),
            ("md-mark-0".to_owned(), "gamma".to_owned()),
            ("md-mark-1".to_owned(), "alpha".to_owned()),
        ]
    );
}

#[test]
fn highlighting_fragmented_match_is_linear() {
    // 100k bold tokens -> 199,999 source ranges in a single match and 200,000 SpanMap entries.
    let n = 100_000;
    let src = "**a** ".repeat(n);
    let q = "a ".repeat(n);
    let hits = find(&src, q.trim_end(), Case::Sensitive);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].len(), 2 * n - 1);
    let _el = Markdown::new(src).highlights(hits).render(&MarkdownStyle::neutral(true));

    // Also verify end-to-end mark merging in Runtime.
    let small_n = 500;
    let small_src = "**a** ".repeat(small_n);
    let small_q = "a ".repeat(small_n);
    let expected = small_q.trim_end().to_owned();
    let small_hits = find(&small_src, &expected, Case::Sensitive);
    let mut rt = Runtime::new(
        (small_src, small_hits),
        |(s, h), _| col().w(600.0).child(Markdown::new(s.as_str()).highlights(h).build(&Kit::neutral(true))),
        (600.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    let marks: Vec<_> = rt.elements().into_iter().filter(|e| e.role == "mark").collect();
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].label.as_deref(), Some(expected.as_str()));
}

