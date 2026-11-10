use super::*;

/// Every inline's text (and code block's), in order, a newline between them.
fn shown(blocks: &[Block]) -> String {
    fn walk(blocks: &[Block], out: &mut Vec<String>) {
        for b in blocks {
            match b {
                Block::Para(i) | Block::Heading(_, i) => out.push(i.text.clone()),
                Block::List { items, .. } => items.iter().for_each(|i| walk(i, out)),
                Block::Quote(inner) => walk(inner, out),
                Block::Table(t) => {
                    out.extend(t.head.iter().map(|i| i.text.clone()));
                    // A row still arriving is padded with empty cells.
                    for row in &t.rows {
                        let n = row.iter().rposition(|i| !i.text.is_empty()).map_or(0, |n| n + 1);
                        out.extend(row[..n].iter().map(|i| i.text.clone()));
                    }
                }
                Block::Code(c) | Block::Diff(c) | Block::Mermaid(c) => out.push(c.clone()),
                Block::Image { alt, .. } => out.push(alt.clone()),
                Block::Rule => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(blocks, &mut out);
    out.join("\n")
}

/// The text under `style` in the first inline.
fn styled(blocks: &[Block], style: Style) -> Vec<String> {
    let Some(Block::Para(i) | Block::Heading(_, i)) = blocks.first() else { return Vec::new() };
    i.spans.iter().filter(|s| s.2 == style).map(|s| i.text[s.0..s.1].to_owned()).collect()
}

fn stream(src: &str) -> Vec<Block> {
    parse_streaming(src, true).0
}

fn prefixes(doc: &str) -> impl Iterator<Item = &str> {
    (1..=doc.len()).filter(|&i| doc.is_char_boundary(i)).map(|i| &doc[..i])
}

/// Streamed a byte at a time, each string's text is the start of the whole text: no marker
/// shows and then goes, and no word changes.
#[test]
fn an_open_span_never_shows_its_markers() {
    for doc in [
        "…and **Settings › Appearance**…",
        "Open **Settings › Appearance** to change it.",
        "**Note:** run `cargo test` and *then* ~~not~~ this.",
        "Mix **bold _and em_** end, an __under__ and `` a`b `` here.",
        "# Heading with **bold**\n\nSee https://example.com/a_b_c and 2 * 3 = 6, snake_case_name.",
        "- item **one**\n- item `two`\n\n> quoted *em*",
        "| a | **b** |\n|---|---|\n| `c` | *d* |",
        "See [the **docs**](https://example.com/a_b \"t\") and ![a chart](x.png), then *[more](y)*.",
        "- [ ] todo\n- [x] done",
    ] {
        // An image becomes its own block when its URL arrives; compare the characters,
        // not the whitespace around that block.
        let visible = |s: String| if doc.contains("![") { s.split_whitespace().collect::<String>() } else { s };
        let whole = visible(shown(&parse(doc)));
        for src in prefixes(doc) {
            let text = visible(shown(&stream(src)));
            assert!(whole.starts_with(&text), "{src:?} shows {text:?}, not the start of {whole:?}");
        }
    }
}

#[test]
fn an_open_span_draws_as_it_will_end() {
    assert_eq!(styled(&stream("…and **Settings ›"), Style::Strong), ["Settings ›"]);
    assert_eq!(styled(&stream("…and **Settings › Appearance*"), Style::Strong), ["Settings › Appearance"], "a closer half typed");
    assert_eq!(styled(&stream("an _important"), Style::Em), ["important"]);
    assert_eq!(styled(&stream("was ~~old"), Style::Strike), ["old"]);
    assert_eq!(styled(&stream("**bold _and em"), Style::Em), ["and em"]);
    assert_eq!(styled(&stream("**bold _and em"), Style::Strong), ["bold and em"]);
    let blocks = stream("Run `cargo te");
    assert_eq!(styled(&blocks, Style::Code), ["cargo te"]);
    assert_eq!(shown(&blocks), format!("Run {}cargo te", super::super::parser::OPEN), "no padding after a span still arriving");
    let Block::Table(t) = &stream("| a | b |\n|---|---|\n| c | **d")[0] else { panic!() };
    assert_eq!(t.rows[0][1].spans.iter().map(|s| s.2).collect::<Vec<_>>(), [Style::Strong], "in a cell still arriving");
}

#[test]
fn a_link_still_arriving_draws_as_a_link() {
    for src in ["see [the do", "see [the docs]", "see [the docs](", "see [the docs](https://ex", "see [the docs](https://example.com \"Ti"] {
        let blocks = stream(src);
        assert_eq!(shown(&blocks), if src.ends_with("do") { "see the do" } else { "see the docs" }, "{src:?}");
        assert_eq!(styled(&blocks, Style::Link), [if src.ends_with("do") { "the do" } else { "the docs" }], "{src:?}");
    }
    assert_eq!(styled(&stream("[**bold"), Style::Strong), ["bold"], "an open span inside its text");
    assert_eq!(shown(&stream("see ![a ch")), "see a ch", "an image shows its alt text");
    // A `[` that isn't a link's shows as written once that's known, and a word's own never hides.
    assert_eq!(shown(&stream("cite [1] here")), "cite [1] here");
    assert_eq!(shown(&stream("xs[0")), "xs[0");
}

#[test]
fn markers_at_the_very_end_wait_for_the_next_character() {
    assert_eq!(shown(&stream("…and **")), "…and");
    assert_eq!(shown(&stream("…and *")), "…and");
    assert_eq!(shown(&stream("use `")), "use");
    assert_eq!(shown(&stream("**`")), "");
    assert_eq!(shown(&stream("a **b** _")), "a b");
    // Those that can't open stay: a word's own `_` or `*`, and `*` between spaces.
    assert_eq!(shown(&stream("snake_")), "snake_");
    assert_eq!(shown(&stream("2*")), "2*");
    assert_eq!(shown(&stream("2 * ")), "2 *");
    assert_eq!(shown(&stream("2 * 3")), "2 * 3");
    assert_eq!(shown(&stream("takes ~5 min")), "takes ~5 min", "one `~` isn't a strike");
}

#[test]
fn a_lone_star_becomes_a_bullet_without_showing() {
    assert_eq!(shown(&stream("Steps:\n\n*")), "Steps:");
    assert!(matches!(&stream("Steps:\n\n* one")[1], Block::List { .. }));
    assert_eq!(styled(&stream("* one"), Style::Em), Vec::<String>::new());
}

#[test]
fn a_finished_paragraph_keeps_its_markers() {
    for src in ["a **b\n\nc", "a **b\n\n", "| x | **y |\n|---|---|\n| z |", "```\na **b\n", "# a **b\n"] {
        assert_eq!(stream(src), parse(src), "{src:?}");
    }
}

#[test]
fn a_header_line_waits_until_it_is_a_table() {
    for src in ["| File", "| File |", "| File |\n", "| File |\n|", "Intro\n\n| File", "Intro\n| File | Change |"] {
        let blocks = stream(src);
        assert!(blocks.iter().all(|b| !shown(std::slice::from_ref(b)).contains("File")), "{src:?} draws {blocks:?}");
    }
    assert!(matches!(&stream("| File | Change")[0], Block::Table(t) if t.head.len() == 2), "two cells draw as the header");
    assert!(matches!(&stream("| File |\n|---|\n")[0], Block::Table(_)), "a one-column table once its delimiter row ends");
    assert!(matches!(&stream("| a | *")[0], Block::Table(t) if t.head.len() == 2), "a marker left out doesn't undo the header");
    assert_eq!(shown(&stream("| a | *")), "a\n", "and its cell waits, empty");
    // Shown as a paragraph once it's known not to be a table.
    assert!(matches!(&stream("| File\nis a word")[0], Block::Para(_)));
    assert!(matches!(&stream("| File\n| Change")[0], Block::Para(_)));
    assert!(matches!(&stream("| File\n\n")[0], Block::Para(_)));
    assert!(matches!(&stream("| File |\n| Change |\n")[0], Block::Para(i) if i.text.contains("Change")));
    // A row of a table, or a line of code, isn't held.
    assert!(matches!(&stream("| a | b |\n|---|---|\n| c")[0], Block::Table(t) if t.rows.len() == 1));
    assert!(matches!(&stream("```\n| File")[0], Block::Code(c) if c == "| File"));
}

#[test]
fn a_streamed_table_comes_in_as_a_table() {
    let doc = "| File | Change |\n|---|---|\n| a.rs | x |";
    for src in prefixes(doc) {
        let blocks = stream(src);
        assert!(blocks.iter().all(|b| matches!(b, Block::Table(_))), "{src:?} draws {blocks:?}");
    }
}

/// A stream's string parses the same grown from its last block as whole.
#[test]
fn a_stream_grows_its_parse_like_a_whole_one() {
    let doc = "Intro with **bold\nacross lines** and `code`.\n\n| File | Change |\n|---|---|\n| a.rs | **x** |\n\n- an _item\n- more\n\nlast *words";
    for src in prefixes(doc) {
        let (blocks, sources, tasks, starts, maps) = stream_cached(src);
        let (want, want_sources, want_tasks, _, want_starts, want_maps) = parse_streaming(src, true);
        assert_eq!(*blocks, want, "blocks of {src:?}");
        assert_eq!((&*sources, &*tasks, &*starts, &*maps), (&want_sources[..], &want_tasks[..], &want_starts[..], &want_maps[..]), "{src:?}");
    }
}
