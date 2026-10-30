#![cfg(feature = "markdown")]

use rune::{Color, Cx, El, Env, LinkPaint, Runtime, ScrollSpec, col};
use rune_kit::markdown::{Block, Highlight, Style, parse, parse_cached};
use rune_kit::{Kit, Markdown, MarkdownStyle};
use std::rc::Rc;

fn view(src: &String, _: &mut Cx<String>) -> El {
    col().w_full().child(Markdown::new(src.clone()).build(&Kit::neutral(true)))
}

fn laid_out(src: &str) -> Runtime<String> {
    let mut rt = Runtime::new(src.to_owned(), view, (480.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn by_key<S>(rt: &Runtime<S>, key: &str) -> rune::runtime::ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}"))
}

fn keys(rt: &Runtime<String>) -> Vec<String> {
    rt.elements().into_iter().filter_map(|e| e.key).collect()
}

const DOC: &str = "# Title\n\nSome `code`, *em*, **bold** and [a link](https://x.dev/a).\n\n#### Small\n\n- one\n- two\n\n3. three\n\n> quoted\n\n---\n\n```rust\nfn main() {}\n```\n";

#[test]
fn parses_each_block_type() {
    let b = parse(DOC);
    assert!(matches!(&b[0], Block::Heading(1, i) if i.text == "Title"));
    let Block::Para(p) = &b[1] else { panic!("{b:?}") };
    assert_eq!(p.text, "Some \u{202f}\u{202f}code\u{202f}\u{202f}, em, bold and a link.");
    let styles: Vec<Style> = p.spans.iter().map(|s| s.2).collect();
    assert_eq!(styles, [Style::CodePad, Style::Code, Style::CodePad, Style::Em, Style::Strong, Style::Link]);
    assert_eq!(p.links.len(), 1);
    let (a, z, url) = &p.links[0];
    assert_eq!((&p.text[*a..*z], url.as_str()), ("a link", "https://x.dev/a"));
    assert!(matches!(&b[2], Block::Heading(4, _)));
    assert!(matches!(&b[3], Block::List { start: None, items } if items.len() == 2));
    assert!(matches!(&b[4], Block::List { start: Some(3), items } if items.len() == 1));
    assert!(matches!(&b[5], Block::Quote(q) if matches!(&q[0], Block::Para(i) if i.text == "quoted")));
    assert!(matches!(&b[6], Block::Rule));
    assert!(matches!(&b[7], Block::Code(c) if c == "fn main() {}"));
}

#[test]
fn an_unclosed_fence_is_a_growing_code_block() {
    let b = parse("Look:\n\n```ts\nconst a = 1\n");
    assert!(matches!(&b[1], Block::Code(text) if text == "const a = 1"));
}

#[test]
fn renders_each_block_type_with_kit_tokens() {
    let k = Kit::neutral(true);
    let rt = laid_out(DOC);
    let title = by_key(&rt, "b0");
    assert_eq!(title.text.as_deref(), Some("Title"));
    assert_eq!(title.text_color, Some(k.c.text_base));
    assert_eq!(by_key(&rt, "b1").text_color, Some(k.c.text_base));
    assert_eq!(by_key(&rt, "b2").text_color, Some(k.c.text_muted), "level 4+ headings are muted");
    assert!(keys(&rt).iter().filter(|k| *k == "i0").count() == 2 && keys(&rt).iter().filter(|k| *k == "i1").count() == 1);
    let texts: Vec<_> = rt.elements().into_iter().filter_map(|e| e.text).collect();
    for want in ["•", "one", "two", "3.", "three", "quoted", "fn main() {}"] {
        assert!(texts.iter().any(|t| t == want), "{want} in {texts:?}");
    }
    assert_eq!(by_key(&rt, "b6").rect[3], 0.5, "a rule is a half-point line");
    let code = by_key(&rt, "b7");
    assert_eq!(code.fill, k.c.layer_01);
    assert!(title.rect[1] < by_key(&rt, "b1").rect[1] && by_key(&rt, "b1").rect[1] < code.rect[1]);
}

#[test]
fn style_builders_override_kit_tokens() {
    let red = rune::Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
    let k = Kit::neutral(true);
    fn styled(src: &String, _: &mut Cx<String>) -> El {
        let k = Kit::neutral(true);
        let red = rune::Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
        Markdown::new(src.clone()).style(MarkdownStyle::from_kit(&k).text(red).code_block(red, red)).build(&k)
    }
    let mut rt = Runtime::new(DOC.to_owned(), styled, (480.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(by_key(&rt, "b0").text_color, Some(red));
    assert_eq!(by_key(&rt, "b7").fill, red);
    assert_ne!(k.c.text_base, red);
}

#[test]
fn a_streaming_append_keeps_earlier_blocks_mounted() {
    let steps = ["# Plan\n\nFirst para", "# Plan\n\nFirst para is done.\n\n- a", "# Plan\n\nFirst para is done.\n\n- a\n- b\n\n```sh\nls", "# Plan\n\nFirst para is done.\n\n- a\n- b\n\n```sh\nls -la\n"];
    let parsed: Vec<_> = steps.iter().map(|s| parse(s)).collect();
    assert_eq!(parsed[2][0], parsed[3][0]);
    assert_eq!(parsed[1][0], parsed[3][0], "the heading block never changes while later text arrives");

    let mut rt = laid_out(steps[0]);
    let mut seen = keys(&rt);
    let head = by_key(&rt, "b0").path.clone();
    for step in &steps[1..] {
        rt.state = (*step).to_owned();
        rt.invalidate();
        rt.frame(1.0 / 60.0);
        let now = keys(&rt);
        for k in &seen {
            assert!(now.contains(k), "{k} survives {step:?}");
        }
        assert_eq!(by_key(&rt, "b0").path, head);
        seen = now;
    }
    assert!(seen.iter().any(|k| k == "b3"), "{seen:?}");
}

/// Every prefix of a nested document parses to the same blocks as before the parser's
/// container handling was shared (a hash of the debug output, captured on the old code).
#[test]
fn every_streaming_prefix_parses_like_before() {
    let doc = "# T\n\n> quote *em [link](https://x.dev) **bold\n>\n> - in `quote`\n>   1. deep\n\n- a\n  > b\n- c **d\n\n```\ncode\n";
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for end in (0..=doc.len()).filter(|&i| doc.is_char_boundary(i)) {
        for byte in format!("{:?}", parse(&doc[..end])).bytes() {
            h ^= u64::from(byte);
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    }
    assert_eq!(h, 11_559_275_952_329_967_701, "golden hash");
}

#[test]
fn identical_source_parses_once() {
    let a = parse_cached("some **cached** text");
    let b = parse_cached("some **cached** text");
    assert!(Rc::ptr_eq(&a, &b));
    let c = parse_cached("some other text");
    assert!(!Rc::ptr_eq(&a, &c));
}

struct Reader {
    src: String,
    handled: bool,
    opened: Vec<String>,
}

fn reader(s: &Reader, _: &mut Cx<Reader>) -> El {
    let md = Markdown::new(s.src.clone());
    let md = if s.handled { md.on_link(|s: &mut Reader, url| s.opened.push(url.to_owned())) } else { md };
    col().w_full().padding(10.0).child(md.build(&Kit::neutral(true)))
}

const LINKS: &str = "See [one](https://a.dev) and [two](https://b.dev).\n\n- an item with [three](https://c.dev)\n";

#[test]
fn links_press_through_on_link() {
    let mut rt = Runtime::new(Reader { src: LINKS.into(), handled: true, opened: Vec::new() }, reader, (480.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let find = |rt: &Runtime<Reader>, key: &str| {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}"))
    };
    let (one, two, three) = (find(&rt, "https://a.dev"), find(&rt, "https://b.dev"), find(&rt, "https://c.dev"));
    assert_eq!((one.role, one.label.as_deref()), ("link", Some("one")));
    assert_eq!(three.label.as_deref(), Some("three"));
    assert!(one.rect[0] + one.rect[2] < two.rect[0] && two.rect[1] < three.rect[1], "{one:?} {two:?} {three:?}");

    // A click on the link's text opens its destination; the plain words beside it don't.
    let centre = |e: &rune::runtime::ElementInfo| (e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
    let click = |rt: &mut Runtime<Reader>, (x, y): (f32, f32)| {
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    };
    rt.pointer_move(centre(&two).0, centre(&two).1);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.cursor(), rune::Cursor::Pointer);
    click(&mut rt, centre(&two));
    click(&mut rt, centre(&three));
    click(&mut rt, ((one.rect[0] + one.rect[2] + two.rect[0]) / 2.0, centre(&one).1));
    assert_eq!(rt.state.opened, ["https://b.dev", "https://c.dev"]);
    // `--script` taps by key through `find`, which resolves a link by its destination.
    let at = rt.find("https://c.dev").expect("a link is found by its destination");
    click(&mut rt, at);
    assert_eq!(rt.state.opened.pop().as_deref(), Some("https://c.dev"));

    // Links are Tab stops in reading order, opened with Enter.
    rt.key(rune::KeyEvent { key: rune::Key::Tab, mods: Default::default() });
    rt.frame(1.0 / 60.0);
    rt.key(rune::KeyEvent { key: rune::Key::Enter, mods: Default::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.opened, ["https://b.dev", "https://c.dev", "https://a.dev"]);

    // Without a handler, links are styled text: no stops, no hand.
    let mut rt = Runtime::new(Reader { src: LINKS.into(), handled: false, opened: Vec::new() }, reader, (480.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(rt.elements().iter().all(|e| !e.path.contains("#link-") && !e.focusable));
    rt.pointer_move(centre(&two).0, centre(&two).1);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.cursor(), rune::Cursor::Default);
}

const TABLE: &str = "| Name | Count | Note |\n|:-----|:-----:|-----:|\n| **a** | 1 | [x](https://x.dev) |\n| b |\n";

#[test]
fn tables_parse_alignment_inline_cells_and_ragged_rows() {
    let b = parse(TABLE);
    let Block::Table(t) = &b[0] else { panic!("{b:?}") };
    assert_eq!(t.align, [rune::Align::Start, rune::Align::Center, rune::Align::End]);
    let head: Vec<_> = t.head.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(head, ["Name", "Count", "Note"]);
    assert_eq!(t.rows[0][0].spans[0].2, Style::Strong);
    assert_eq!(t.rows[0][2].links[0].2, "https://x.dev");
    assert!(t.rows[1].len() <= 3 && t.rows[1][0].text == "b");
    assert!(!t.open);

    let rt = laid_out(TABLE);
    let (th, r0) = (by_key(&rt, "th"), by_key(&rt, "r0"));
    assert!(th.rect[1] < r0.rect[1]);
    let texts: Vec<_> = rt.elements().into_iter().filter_map(|e| e.text).collect();
    for want in ["Name", "Count", "Note", "a", "1", "x", "b"] {
        assert!(texts.iter().any(|t| t == want), "{want} in {texts:?}");
    }
}

#[test]
fn a_trailing_header_line_is_already_a_table() {
    for src in ["Intro\n\n| a | b |", "Intro\n\n| a | b |\n|:-", "Intro\n\n| a | b |\n|--|--:|"] {
        let b = parse(src);
        let Block::Table(t) = &b[1] else { panic!("{src:?}: {b:?}") };
        assert!(t.rows.is_empty() && t.head.len() == 2, "{src:?}");
    }
    let Block::Table(t) = &parse("| a | b |\n|--|--:|")[0] else { panic!() };
    assert_eq!(t.align[1], rune::Align::End);
    assert!(matches!(&parse("| just one |")[0], Block::Para(_)), "a single cell is a paragraph");
    assert!(matches!(&parse("a | b")[0], Block::Para(_)));
    // An escaped pipe stays in its cell, and the cell's markup still parses.
    let Block::Table(t) = &parse("| \\| **a** \\| b | c |")[0] else { panic!() };
    assert_eq!((t.head.len(), t.head[0].text.as_str()), (2, "| a | b"));
    assert_eq!(t.head[0].spans[0].2, Style::Strong);
}

/// The x of every cell `c{j}` in the header row, in order.
fn column_edges(rt: &Runtime<String>) -> Vec<f32> {
    rt.elements().into_iter().filter(|e| e.path.contains("th") && e.key.as_deref().is_some_and(|k| k.starts_with('c'))).map(|e| e.rect[0]).collect()
}

fn streaming(src: &String, _: &mut Cx<String>) -> El {
    col().w_full().child(Markdown::new(src.clone()).streaming(true).build(&Kit::neutral(true)))
}

/// A finished message with a word fade, as a transcript draws every row.
fn revealed(src: &String, _: &mut Cx<String>) -> El {
    col().w_full().child(Markdown::new(src.clone()).reveal(0.18).build(&Kit::neutral(true)))
}

#[test]
fn a_streaming_row_does_not_move_the_columns() {
    let base = "| Step | Result |\n|---|---|\n| one | ok |\n";
    let mut rt = Runtime::new(base.to_owned(), streaming, (480.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let before = column_edges(&rt);
    assert_eq!(before.len(), 2, "{:?}", keys(&rt));
    for partial in ["| a much longer first cell that is still arriving", "| a much longer first cell that is still arriving | and more"] {
        rt.state = format!("{base}{partial}");
        rt.invalidate();
        rt.frame(1.0 / 60.0);
        assert_eq!(column_edges(&rt), before, "{partial:?}");
    }
    rt.state = format!("{base}| a much longer first cell that is still arriving | and more |\n");
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    assert_ne!(column_edges(&rt), before, "a finished row sizes the columns");
}

/// Unless it streams the text is whole: a table that ends it, with no newline after its last
/// row, sizes its columns to that row too, word fade or not.
#[test]
fn a_table_ending_a_whole_note_sizes_to_its_last_row() {
    let src = "| Field | Compared |\n|---|---|\n| title | yes |\n| a much longer cell at the end | no |";
    assert!(matches!(&parse(src)[0], Block::Table(t) if t.open));
    let whole = column_edges(&laid_out(src));
    assert_eq!(whole, column_edges(&laid_out(&format!("{src}\n"))), "the trailing newline changes nothing");
    let mut rt = Runtime::new(src.to_owned(), revealed, (480.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(column_edges(&rt), whole, "a word fade alone doesn't mean the text is still arriving");
    let mut rt = Runtime::new(src.to_owned(), streaming, (480.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(column_edges(&rt)[1] < whole[1], "while streaming, the last row is still arriving");
}

#[test]
fn a_table_narrower_than_its_container_never_wraps_a_cell() {
    struct Doc {
        src: &'static str,
        family: rune::Family,
    }
    fn view(d: &Doc, _: &mut Cx<Doc>) -> El {
        let body = rune::TextStyle { family: d.family, line_height: 22.4, ..Kit::neutral(true).ty.body };
        col().w_full().child(Markdown::new(d.src).render(&MarkdownStyle::neutral(true).body_font(body)))
    }
    let src = "| Field | Compared |\n|---|---|\n| title | yes |\n| updatedAt | yes |\n| draft cursor | no |\n| 1,234.50 | 98% |\n";
    for family in [rune::Family::Sans, rune::Family::Named("System Font")] {
        for scale in [1.0, 2.0, 3.0] {
            let mut rt = Runtime::new(Doc { src, family }, view, (1100.0, 600.0), Env::new(true, scale));
            rt.frame(0.0);
            let all = rt.elements();
            let in_cell = |mut i: usize| loop {
                if all[i].key.as_deref().is_some_and(|k| k.len() > 1 && k.starts_with('c') && k[1..].bytes().all(|b| b.is_ascii_digit())) {
                    return true;
                }
                match all[i].parent {
                    Some(p) => i = p,
                    None => return false,
                }
            };
            let cells: Vec<_> = (0..all.len()).filter(|&i| all[i].text.is_some() && in_cell(i)).map(|i| &all[i]).collect();
            assert_eq!(cells.len(), 10, "{family:?} @{scale}");
            let line = cells[0].rect[3];
            for c in &cells {
                assert_eq!(c.rect[3], line, "{:?} wraps at {family:?} @{scale}: {:?}", c.text, c.rect);
            }
        }
    }
}

#[test]
fn table_options_drop_the_outline_dim_the_header_and_line_up_figures() {
    let [dim, rule] = [[0.5, 0.5, 0.5], [0.0, 0.0, 1.0]].map(|[r, g, b]| rune::Color { r, g, b, a: 1.0 });
    fn styled(src: &String, _: &mut Cx<String>) -> El {
        let [dim, rule] = [[0.5, 0.5, 0.5], [0.0, 0.0, 1.0]].map(|[r, g, b]| rune::Color { r, g, b, a: 1.0 });
        col().w_full().child(Markdown::new(src.clone()).render(&MarkdownStyle::neutral(true).table_outline(None).table_rules(rule).table_header_text(dim)))
    }
    let src = "| Name | Count |\n|---|--:|\n| a | 1111 |\n| b | 0000 |\n";
    let text_color = |rt: &Runtime<String>, t: &str| rt.elements().into_iter().find(|e| e.text.as_deref() == Some(t)).unwrap().text_color;
    let frame_radius = |rt: &Runtime<String>| {
        let all = rt.elements();
        let th = all.iter().position(|e| e.key.as_deref() == Some("th")).unwrap();
        all[all[th].parent.unwrap()].radius
    };
    let rt = laid_out(src);
    assert!(!keys(&rt).iter().any(|k| k == "s-end"), "the outline closes the last row by default");
    assert_eq!(frame_radius(&rt), 6.0);
    assert_eq!(text_color(&rt, "Name"), text_color(&rt, "a"));

    let mut rt = Runtime::new(src.to_owned(), styled, (480.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!((by_key(&rt, "s0").fill, by_key(&rt, "s-end").fill), (rule, rule));
    assert_eq!(frame_radius(&rt), 0.0, "square corners leave the last rule whole");
    assert_eq!(text_color(&rt, "Name"), Some(dim));
    assert_eq!(text_color(&rt, "a"), Some(Kit::neutral(true).c.text_base));

    // Tabular figures (on by default) give every digit one width and leave other text alone.
    for family in [rune::Family::Sans, rune::Family::Named("System Font")] {
        let plain = rune::TextStyle::new(14.0, 400.0).family(family);
        let tnum = plain.features("'tnum' 1");
        let w = |t: &str, f| rune::text::measure(t, &[], f, None).0;
        assert_eq!(w("1111", tnum), w("0000", tnum), "{family:?}");
        assert_ne!(w("1111", plain), w("0000", plain), "{family:?}'s default figures are proportional");
        assert_eq!(w("draft cursor, updatedAt", tnum), w("draft cursor, updatedAt", plain), "{family:?}");
    }
}

#[test]
fn table_header_takes_its_own_rule_fill_and_weight() {
    fn styled(src: &String, _: &mut Cx<String>) -> El {
        let [rule, head] = [[0.0, 0.0, 1.0], [1.0, 0.0, 0.0]].map(|[r, g, b]| rune::Color { r, g, b, a: 1.0 });
        let style = MarkdownStyle::neutral(true)
            .table_outline(None)
            .table_rules(rule)
            .table_header_rule(head)
            .table_header_fill(rune::Color::TRANSPARENT)
            .table_header_weight(500.0);
        col().w_full().child(Markdown::new(src.clone()).render(&style))
    }
    fn strong(src: &String, _: &mut Cx<String>) -> El {
        col().w_full().child(Markdown::new(src.clone()).render(&MarkdownStyle::neutral(true).table_outline(None)))
    }
    let [rule, head] = [[0.0, 0.0, 1.0], [1.0, 0.0, 0.0]].map(|[r, g, b]| rune::Color { r, g, b, a: 1.0 });
    let src = "| A header wider than the narrowest column | Count |\n|---|--:|\n| a | 1 |\n| b | 2 |\n";
    // The first column is as wide as its widest cell, the header.
    let width = |rt: &Runtime<String>| rt.elements().into_iter().find(|e| e.path.ends_with("th/c0")).unwrap().rect[2];
    let mut plain = Runtime::new(src.to_owned(), strong, (900.0, 400.0), Env::new(true, 1.0));
    plain.frame(0.0);
    let mut rt = Runtime::new(src.to_owned(), styled, (900.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!([by_key(&rt, "s0").fill, by_key(&rt, "s1").fill, by_key(&rt, "s-end").fill], [head, rule, rule]);
    assert_eq!(by_key(&rt, "th").fill, rune::Color::TRANSPARENT);
    assert!(width(&rt) < width(&plain), "a medium header is narrower than the strong default");

    let mut empty = Runtime::new("| Name |\n|---|\n".to_owned(), styled, (480.0, 400.0), Env::new(true, 1.0));
    empty.frame(0.0);
    assert_eq!(by_key(&empty, "s-end").fill, head, "with no rows the closing rule is the header's");
}

/// A web table's look: cells a step smaller than the body, padded 10 by 4, the first column
/// flush with the paragraph above.
#[test]
fn table_font_and_padding_set_the_cells_and_the_first_column_sits_flush() {
    fn styled(src: &String, _: &mut Cx<String>) -> El {
        let body = rune::TextStyle { size: 15.0, line_height: 24.0, ..Kit::neutral(true).ty.body };
        let style = MarkdownStyle::neutral(true)
            .body_font(body)
            .table_outline(None)
            .table_font(rune::TextStyle { size: 15.0 * 0.93, line_height: 20.0, ..body })
            .table_cell_padding(10.0, 4.0)
            .table_flush_left(true);
        col().w_full().child(Markdown::new(src.clone()).render(&style))
    }
    let src = "Intro\n\n| Name | Count |\n|---|---|\n| alpha | 1 |\n";
    let mut rt = Runtime::new(src.to_owned(), styled, (480.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let all = rt.elements();
    let text = |t: &str| all.iter().find(|e| e.text.as_deref() == Some(t)).unwrap_or_else(|| panic!("no {t}"));
    let cells: Vec<_> = all.iter().filter(|e| e.path.contains("th") && e.key.as_deref().is_some_and(|k| k.starts_with('c'))).collect();
    assert_eq!(text("Name").rect[0], text("Intro").rect[0], "the first column is flush with the paragraph");
    assert_eq!(text("Name").rect[0], cells[0].rect[0]);
    assert_eq!(text("Count").rect[0] - cells[1].rect[0], 10.0);
    assert_eq!(by_key(&rt, "th").rect[3], 20.0 + 2.0 * 4.0, "the table font's line and the cell padding");
    assert_eq!(by_key(&rt, "r0").rect[3], 28.0);
    // Columns hold their text on one line: the first is its text plus the right padding only.
    let name = rune::text::measure("Name", &[], rune::TextStyle { size: 15.0 * 0.93, line_height: 20.0, weight: 670.0, features: "'tnum' 1", ..Kit::neutral(true).ty.body }, None).0;
    let alpha = rune::text::measure("alpha", &[], rune::TextStyle { size: 15.0 * 0.93, line_height: 20.0, features: "'tnum' 1", ..Kit::neutral(true).ty.body }, None).0;
    assert_eq!(cells[0].rect[2], name.max(alpha) + 10.0);
}

#[test]
fn columns_too_wide_to_fit_stop_at_the_frame_with_or_without_an_outline() {
    fn view(outline: &bool, _: &mut Cx<bool>) -> El {
        let head: String = (0..10).map(|i| format!("| heading{i} ")).collect();
        let row: String = (0..10).map(|i| format!("| unbreakablecell{i} ")).collect();
        let src = format!("{head}|\n{}|\n{row}|\n", "|---".repeat(10));
        let style = MarkdownStyle::neutral(true);
        let style = if *outline { style } else { style.table_outline(None) };
        col().w(400.0).child(Markdown::new(src).render(&style))
    }
    for outline in [true, false] {
        let mut rt = Runtime::new(outline, view, (600.0, 400.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let all = rt.elements();
        let th = all.iter().position(|e| e.key.as_deref() == Some("th")).unwrap();
        let frame = all[all[th].parent.unwrap()].rect;
        let right = frame[0] + frame[2];
        let cells: Vec<_> = all.iter().filter(|e| e.key.as_deref().is_some_and(|k| k.len() == 2 && k.starts_with('c'))).collect();
        assert_eq!(cells.len(), 20, "outline {outline}");
        assert!(
            cells.iter().any(|c| c.rect[0] < right && c.rect[0] + c.rect[2] > right),
            "outline {outline}: a column straddles the frame's edge, {frame:?}"
        );
        for c in cells {
            let shown = c.visible[2] == 0.0 || c.visible[0] + c.visible[2] <= right + 0.01;
            assert!(shown, "outline {outline}: {:?} spills past {frame:?}", c.visible);
        }
    }
}

#[test]
fn diffs_tint_whole_lines_and_dim_hunk_headers() {
    let src = "```diff\n@@ -1,2 +1,2 @@\n keep\n-old\n+new\n```\n";
    assert!(matches!(&parse(src)[0], Block::Diff(d) if d.lines().count() == 4));
    assert!(matches!(&parse("```patch\n+a\n```")[0], Block::Diff(_)));
    fn styled(src: &String, _: &mut Cx<String>) -> El {
        let [add, remove, hunk] = [[0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]].map(|[r, g, b]| rune::Color { r, g, b, a: 1.0 });
        col().w_full().child(Markdown::new(src.clone()).render(&MarkdownStyle::neutral(false).diff(add, remove).diff_hunk(hunk)))
    }
    let mut rt = Runtime::new(src.to_owned(), styled, (480.0, 400.0), Env::new(false, 1.0));
    rt.frame(0.0);
    let fill = |k: &str| by_key(&rt, k).fill;
    let [add, remove, hunk] = [[0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]].map(|[r, g, b]| rune::Color { r, g, b, a: 1.0 });
    assert_eq!((fill("l2"), fill("l3")), (remove, add));
    assert_ne!(fill("l1"), add);
    let block = by_key(&rt, "b0");
    assert_eq!(by_key(&rt, "l3").rect[2], block.rect[2], "tints run edge to edge");
    let header = rt.elements().into_iter().find(|e| e.text.as_deref() == Some("@@ -1,2 +1,2 @@")).unwrap();
    assert_eq!(header.text_color, Some(hunk));
    let line_color = |rt: &Runtime<String>, t: &str| rt.elements().into_iter().find(|e| e.text.as_deref() == Some(t)).unwrap().text_color.unwrap();
    let text = Kit::neutral(false).c.text_base;
    assert_eq!((line_color(&rt, "+new"), line_color(&rt, "-old")), (text, text), "line text is untinted by default");

    fn tinted(src: &String, _: &mut Cx<String>) -> El {
        col().w_full().child(Markdown::new(src.clone()).render(&MarkdownStyle::neutral(false).diff_text_tint(0.5)))
    }
    let mut rt = Runtime::new(src.to_owned(), tinted, (480.0, 400.0), Env::new(false, 1.0));
    rt.frame(0.0);
    let (add, remove) = (line_color(&rt, "+new"), line_color(&rt, "-old"));
    assert!(add.g > add.r && remove.r > remove.g, "{add:?} {remove:?}");
    assert_eq!(line_color(&rt, " keep"), text);
}

#[test]
fn markdown_and_tooltips_render_without_a_kit() {
    fn plain(src: &String, _: &mut Cx<String>) -> El {
        let mut layer = Vec::new();
        let tip = rune_kit::Tooltip::new("t").trigger(rune::text("?")).label("Help").open(true).render(&rune_kit::TooltipStyle::neutral(true), &mut layer);
        rune_kit::root(col().w_full().child(Markdown::new(src.clone()).render(&MarkdownStyle::neutral(true))).child(tip), layer)
    }
    let mut rt = Runtime::new(DOC.to_owned(), plain, (480.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let k = Kit::neutral(true);
    assert_eq!(by_key(&rt, "b0").text_color, Some(k.c.text_base));
    assert_eq!(by_key(&rt, "b7").fill, k.c.layer_01, "neutral(dark) matches Kit::neutral(dark)");
    let tip = by_key(&rt, "t-tooltip");
    assert_eq!(tip.role, "tooltip");
    assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("Help")));
}

#[cfg(feature = "mermaid")]
#[test]
fn a_mermaid_block_draws_a_diagram_or_falls_back_to_its_source() {
    let rt = laid_out("```mermaid\ngraph TD\n  A[Start] --> B{Ok?}\n```\n");
    let k = keys(&rt);
    assert!(k.iter().any(|k| k == "diagram") && !k.iter().any(|k| k == "source"), "{k:?}");
    assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("Ok?")));

    for bad in ["pie title Pets\n  \"Dogs\": 3", "graph TD\nA[unclosed --> B", "sequenceDiagram\n  A->>"] {
        let rt = laid_out(&format!("```mermaid\n{bad}\n```\n"));
        let k = keys(&rt);
        assert!(k.iter().any(|k| k == "source") && !k.iter().any(|k| k == "hint") && !k.iter().any(|k| k == "diagram"), "{bad:?}: {k:?}");
    }

    // Until the fence closes, the source is a code block that grows in place.
    assert!(matches!(&parse("```mermaid\ngraph TD\nA-->B")[0], Block::Code(_)));
    assert!(matches!(&parse("```mermaid\ngraph TD\nA-->B\n```")[0], Block::Mermaid(_)));
    assert!(rune_kit::mermaid::supported("sequenceDiagram\nA->>B: hi"));
    assert!(rune_kit::mermaid::render("gantt\n", &MarkdownStyle::neutral(false)).is_none());
}

#[test]
fn scroll_wide_keeps_code_lines_whole_and_scrolls_them_sideways() {
    fn wide(src: &String, _: &mut Cx<String>) -> El {
        col().w_full().child(Markdown::new(src.clone()).scroll_wide(true).build(&Kit::neutral(true)))
    }
    let line = format!("let wide = [{}];", (0..60).map(|i| i.to_string()).collect::<Vec<_>>().join(", "));
    let src = format!("Intro\n\n```rust\n{line}\nshort();\n```\n");
    let code = |rt: &Runtime<String>| rt.elements().into_iter().find(|e| e.text.as_deref().is_some_and(|t| t.starts_with("let wide"))).expect("code");

    // Off (the default) the line wraps inside the column.
    let plain = laid_out(&src);
    assert!(code(&plain).rect[2] <= 480.0, "{:?}", code(&plain).rect);
    assert!(!keys(&plain).iter().any(|k| k == "scroll"));

    let mut rt = Runtime::new(src.clone(), wide, (480.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let before = code(&rt).rect;
    assert!(before[2] > 600.0 && before[3] < code(&plain).rect[3], "one long line, unwrapped: {before:?} vs {:?}", code(&plain).rect);
    let viewport = by_key(&rt, "scroll").rect;
    assert!((viewport[3] - (before[3] + 24.0)).abs() < 0.5, "the block fits the unwrapped lines: {viewport:?} {before:?}");
    let (x, y) = (240.0, before[1] + 8.0);
    rt.wheel_xy(x, y, -100.0, -5.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(code(&rt).rect[0], before[0] - 100.0);
    rt.wheel_xy(x, y, -100_000.0, 0.0);
    rt.frame(1.0 / 60.0);
    let end = code(&rt).rect;
    assert!((end[0] + end[2] + 12.0 - 480.0).abs() < 1.0, "the line's end and its padding reach the right edge: {end:?}");
}

#[test]
fn scroll_wide_leaves_a_narrow_table_at_its_own_width() {
    fn wide(src: &String, _: &mut Cx<String>) -> El {
        col().w_full().child(Markdown::new(src.clone()).scroll_wide(true).build(&Kit::neutral(true)))
    }
    let src = "| a | b |\n|---|---|\n| 1 | 2 |\n".to_owned();
    let plain = laid_out(&src);
    let mut rt = Runtime::new(src, wide, (480.0, 800.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let (rule, natural) = (by_key(&rt, "s0").rect, by_key(&plain, "s0").rect);
    assert!(natural[2] < 200.0, "{natural:?}");
    assert!((rule[2] - natural[2]).abs() < 0.5, "not stretched across the column: {rule:?} vs {natural:?}");
}

/// The ink extent `[left, right]` of the glyphs drawn inside `cell`, at scale 1.
fn ink(rt: &Runtime<String>, cell: &rune::runtime::ElementInfo) -> [f32; 2] {
    let r = cell.rect;
    let glyphs = rt.instances.iter().filter(|i| i.params[1] == rune::render::KIND_GLYPH).filter(|i| {
        let (x, y) = (i.rect[0] + i.rect[2] / 2.0, i.rect[1] + i.rect[3] / 2.0);
        x >= r[0] && x <= r[0] + r[2] && y >= r[1] && y <= r[1] + r[3]
    });
    glyphs.fold([f32::MAX, f32::MIN], |[l, rr], i| [l.min(i.rect[0]), rr.max(i.rect[0] + i.rect[2])])
}

/// Cell `c{j}` of the row keyed `row` (`th`, `r0`, ...).
fn cell(rt: &Runtime<String>, row: &str, j: usize) -> rune::runtime::ElementInfo {
    let all = rt.elements();
    let r = all.iter().position(|e| e.key.as_deref() == Some(row)).unwrap_or_else(|| panic!("no {row}"));
    let key = format!("c{j}");
    all.iter().find(|e| e.parent == Some(r) && e.key.as_deref() == Some(key.as_str())).cloned().unwrap_or_else(|| panic!("no {row}/{key}"))
}

#[test]
fn cells_sit_where_their_column_aligns_them() {
    let rt = laid_out("| Left column heading | Centred column heading | Right column heading |\n|:--|:-:|--:|\n| a | b | c |\n");
    for row in ["th", "r0"] {
        let [left, centre, right] = [0, 1, 2].map(|j| cell(&rt, row, j));
        let ([l0, _], [c0, c1], [_, r1]) = (ink(&rt, &left), ink(&rt, &centre), ink(&rt, &right));
        assert!((l0 - (left.rect[0] + 10.0)).abs() < 2.0, "{row}: start-aligned text starts at the padding, {l0} in {:?}", left.rect);
        let (before, after) = (c0 - centre.rect[0], centre.rect[0] + centre.rect[2] - c1);
        assert!((before - after).abs() < 2.0, "{row}: centred, {before} either side {after}");
        assert!((right.rect[0] + right.rect[2] - 10.0 - r1).abs() < 2.0, "{row}: end-aligned text ends at the padding, {r1} in {:?}", right.rect);
    }
}

const LONG_CELL: &str = "| Term | Meaning |\n|---|---|\n| cache | a sentence much too long for one line of a narrow column, so it wraps inside its cell |\n| hit | yes |\n";

#[test]
fn a_long_cell_wraps_inside_its_column_and_its_row_grows_to_fit() {
    fn narrow(src: &String, _: &mut Cx<String>) -> El {
        col().w_full().child(col().w(300.0).child(Markdown::new(src.clone()).build(&Kit::neutral(true))))
    }
    let mut rt = Runtime::new(LONG_CELL.to_owned(), narrow, (480.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let all = rt.elements();
    let text = |t: &str| all.iter().find(|e| e.text.as_deref().is_some_and(|x| x.starts_with(t))).unwrap_or_else(|| panic!("no {t}")).rect;
    let (term, long, hit) = (text("cache"), text("a sentence"), text("hit"));
    assert!(long[3] > 2.5 * term[3], "the long cell wraps onto several lines: {long:?} vs {term:?}");
    assert!(long[0] + long[2] <= 300.0, "and stays inside the table: {long:?}");
    assert_eq!(term[1], long[1], "cells in a row share their top");
    let (r0, r1) = (by_key(&rt, "r0").rect, by_key(&rt, "r1").rect);
    assert!(r0[3] >= long[3] + 12.0 - 0.5, "the row holds the wrapped text and its padding: {r0:?}");
    assert!(hit[1] > long[1] + long[3] && r1[1] >= r0[1] + r0[3], "the next row starts below it");
    let [_, ink_right] = ink(&rt, &cell(&rt, "r0", 1));
    assert!(ink_right <= 300.0, "no glyph past the frame: {ink_right}");
}

#[test]
fn scroll_wide_keeps_a_wide_tables_cells_on_one_line_and_scrolls_it_sideways() {
    fn wide(src: &String, _: &mut Cx<String>) -> El {
        col().w_full().child(col().w(300.0).child(Markdown::new(src.clone()).scroll_wide(true).build(&Kit::neutral(true))))
    }
    let mut rt = Runtime::new(LONG_CELL.to_owned(), wide, (480.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let text = |rt: &Runtime<String>, t: &str| rt.elements().into_iter().find(|e| e.text.as_deref().is_some_and(|x| x.starts_with(t))).unwrap().rect;
    let (term, long) = (text(&rt, "cache"), text(&rt, "a sentence"));
    assert_eq!(long[3], term[3], "one line: {long:?}");
    assert!(long[0] + long[2] > 400.0, "wider than the column: {long:?}");
    let all = rt.elements();
    let th = all.iter().position(|e| e.key.as_deref() == Some("th")).unwrap();
    let frame = all[th].parent.unwrap();
    let viewport = all[all[frame].parent.unwrap()].rect;
    assert_eq!(viewport[2], 300.0, "the viewport keeps to the column");
    rt.wheel_xy(150.0, long[1] + 4.0, -120.0, 0.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(text(&rt, "a sentence")[0], long[0] - 120.0, "the table scrolls sideways");
}

#[test]
fn table_like_draws_markdown_tables_with_the_data_tables_metrics() {
    fn shared(src: &String, _: &mut Cx<String>) -> El {
        let k = Kit::neutral(true);
        col().w_full().child(Markdown::new(src.clone()).render(&MarkdownStyle::from_kit(&k).table_like(&rune_kit::TableStyle::from_kit(&k))))
    }
    let k = Kit::neutral(true);
    let t = rune_kit::TableStyle::from_kit(&k);
    let mut rt = Runtime::new("| Name | Size |\n|---|--:|\n| a | 1 |\n".to_owned(), shared, (480.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let row = by_key(&rt, "r0").rect[3];
    assert!((row - t.row_height).abs() <= 0.5, "a one-line row is the data table's row height: {row}");
    assert_eq!(by_key(&rt, "s0").fill, t.rule);
    let header = rt.elements().into_iter().find(|e| e.text.as_deref() == Some("Name")).unwrap();
    assert_eq!(header.text_color, Some(t.header_text));
    assert_eq!(header.rect[0] - cell(&rt, "th", 0).rect[0], t.cell_pad);
}

struct Chips {
    src: String,
    tx_mode: bool,
    opened: Vec<String>,
}

const CHIP_BG: &str = "#ff0055";
const CHIP_HOVER: &str = "#00ff55";

fn chips(s: &Chips, _: &mut Cx<Chips>) -> El {
    let bg = rune::color::hex(CHIP_BG).unwrap();
    let hover = rune::color::hex(CHIP_HOVER).unwrap();
    let md = Markdown::new(s.src.clone()).link_style(move |url| {
        url.starts_with("session:").then(|| (rune::Span { color: Some(Color::WHITE), ..rune::Span::new(0, 0) }.bg(bg, 3.0), LinkPaint::new().bg(hover)))
    });
    let md = if s.tx_mode {
        md.on_link_tx(|s: &mut Chips, url, tx| {
            s.opened.push(url.to_owned());
            tx.copy(url);
        })
    } else {
        md.on_link(|s: &mut Chips, url| s.opened.push(url.to_owned()))
    };
    col().w_full().padding(10.0).child(md.build(&Kit::neutral(true)))
}

fn chip_rt(tx_mode: bool) -> Runtime<Chips> {
    let src = "Open [Title](session:abc) or [web](https://a.dev).".to_owned();
    let mut rt = Runtime::new(Chips { src, tx_mode, opened: Vec::new() }, chips, (480.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn fill_rects<S>(rt: &Runtime<S>, hex: &str) -> Vec<[f32; 4]> {
    let want = rune::color::hex(hex).unwrap().premul(1.0);
    rt.instances
        .iter()
        .filter(|i| i.params[1] == rune::render::KIND_RECT && i.fill.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3))
        .map(|i| i.rect)
        .collect()
}

fn has_fill(rt: &Runtime<Chips>, hex: &str) -> bool {
    !fill_rects(rt, hex).is_empty()
}

fn centre<S>(rt: &Runtime<S>, key: &str) -> (f32, f32) {
    let e = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}"));
    (e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0)
}

#[test]
fn on_link_tx_reaches_the_tx_and_on_link_still_works() {
    let mut rt = chip_rt(true);
    let (x, y) = centre(&rt, "https://a.dev");
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.opened, ["https://a.dev"]);
    assert_eq!(rt.copy_request.as_deref(), Some("https://a.dev"));

    let mut rt = chip_rt(false);
    let (x, y) = centre(&rt, "session:abc");
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.opened, ["session:abc"]);
    assert!(rt.copy_request.is_none());

    // Setting both on_link and on_link_tx keeps whichever was called last.
    fn tx_last(_: &Vec<String>, _: &mut Cx<Vec<String>>) -> El {
        Markdown::new("[go](session:1)")
            .on_link(|s: &mut Vec<String>, _| s.push("plain".into()))
            .on_link_tx(|s: &mut Vec<String>, url, tx| {
                s.push(format!("tx:{url}"));
                tx.copy(url);
            })
            .build(&Kit::neutral(true))
    }
    fn plain_last(_: &Vec<String>, _: &mut Cx<Vec<String>>) -> El {
        Markdown::new("[go](session:1)")
            .on_link_tx(|s: &mut Vec<String>, _, tx| {
                s.push("tx".into());
                tx.copy("tx");
            })
            .on_link(|s: &mut Vec<String>, url| s.push(format!("plain:{url}")))
            .build(&Kit::neutral(true))
    }
    let mut rt = Runtime::new(Vec::new(), tx_last, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let (x, y) = centre(&rt, "session:1");
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state, ["tx:session:1"]);
    assert_eq!(rt.copy_request.as_deref(), Some("session:1"));

    let mut rt = Runtime::new(Vec::new(), plain_last, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let (x, y) = centre(&rt, "session:1");
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state, ["plain:session:1"]);
    assert!(rt.copy_request.is_none());
}

#[test]
fn link_style_looks_only_at_the_urls_it_names_and_hover_matches_its_padded_rect() {
    let mut rt = chip_rt(false);
    let resting = fill_rects(&rt, CHIP_BG);
    assert_eq!(resting.len(), 1, "one resting chip rect: {resting:?}");
    let chip_el = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("session:abc")).unwrap();
    assert_eq!((chip_el.role, chip_el.label.as_deref()), ("link", Some("Title")));
    assert!((resting[0][2] - (chip_el.rect[2] + 6.0)).abs() < 1.5, "bg_pad 3 widens both sides: {:?} vs {:?}", resting[0], chip_el.rect);

    // The https link stays at the default look: hovering it paints no chip hover colour.
    let (x, y) = centre(&rt, "https://a.dev");
    rt.pointer_move(x, y);
    rt.frame(1.0 / 60.0);
    assert!(!has_fill(&rt, CHIP_HOVER));

    // Hovering the chip replaces the resting fill with the hover fill on the exact same rect.
    let (x, y) = centre(&rt, "session:abc");
    rt.pointer_move(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(fill_rects(&rt, CHIP_HOVER), resting, "hover bg keeps the resting pad and height");
    assert!(!has_fill(&rt, CHIP_BG), "resting bg does not bleed under hover bg");
}

#[test]
fn styled_links_keep_inner_styles_select_copy_and_stream_across_chunks() {
    fn view(src: &String, _: &mut Cx<String>) -> El {
        let bg = rune::color::hex(CHIP_BG).unwrap();
        let hover = rune::color::hex(CHIP_HOVER).unwrap();
        col().w(160.0).child(
            Markdown::new(src.clone())
                .reveal(0.2)
                .selectable(true)
                .link_style(move |url| {
                    url.starts_with("session:").then(|| {
                        (rune::Span::new(0, 0).color(Color::WHITE).weight(500.0).bg(bg, 3.0), LinkPaint::new().bg(hover))
                    })
                })
                .on_link(|_: &mut String, _| {})
                .build(&Kit::neutral(true)),
        )
    }

    // 1. Streaming a link split across chunks: once `)` arrives, the link gets its chip style.
    let mut rt = Runtime::new("Start [First **bold** `code`](ses".to_owned(), view, (160.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(fill_rects(&rt, CHIP_BG).is_empty(), "unclosed link syntax is still plain text");

    rt.state = "Start [First **bold** `code`](session:one)".to_owned();
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    assert_eq!(fill_rects(&rt, CHIP_BG).len(), 1, "completed link gets one merged chip rect");

    // 2. Appending a second styled link fades its glyphs in through word reveal and wraps cleanly.
    rt.state = "Start [First **bold** `code`](session:one) and *[Second](session:two)* on the next line.".to_owned();
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    let chips = fill_rects(&rt, CHIP_BG);
    assert_eq!(chips.len(), 2, "inner bold and code runs share one merged chip rect per link: {chips:?}");
    assert!(chips[0][1] + chips[0][3] < chips[1][1], "chips on adjacent lines do not overlap: {chips:?}");
    // Mid-reveal the newly arrived second link's glyphs are fading in (0 < alpha < 1).
    let fading = rt.instances.iter().any(|i| i.params[1] == rune::render::KIND_GLYPH && i.fill[3] > 0.01 && i.fill[3] < 0.99);
    assert!(fading, "reveal fades in styled link glyphs");
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    // After the reveal lands, all glyphs inside both styled links draw at full WHITE alpha 1.
    let white_glyphs = rt
        .instances
        .iter()
        .filter(|i| i.params[1] == rune::render::KIND_GLYPH && (i.fill[0] - 1.0).abs() < 1e-3 && (i.fill[1] - 1.0).abs() < 1e-3 && (i.fill[2] - 1.0).abs() < 1e-3 && (i.fill[3] - 1.0).abs() < 1e-3)
        .count();
    assert!(white_glyphs >= "FirstboldcodeSecond".len(), "both links' glyphs keep WHITE (even inside *em*): {white_glyphs}");

    // Selecting and copying gets the link text without markdown syntax or inline-code padding.
    rt.pointer_move(2.0, 6.0);
    rt.pointer_down(2.0, 6.0);
    rt.pointer_up(2.0, 6.0);
    rt.frame(1.0 / 60.0);
    let cmd = rune::Mods { cmd: true, ..Default::default() };
    rt.key(rune::KeyEvent { key: rune::Key::Command('a'), mods: cmd });
    rt.frame(1.0 / 60.0);
    rt.key(rune::KeyEvent { key: rune::Key::Command('c'), mods: cmd });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.copy_request.as_deref(), Some("Start First bold code and Second on the next line."));
}

// These fixtures intentionally have only blank-line-separated top-level blocks.
fn fixture_keys<'a>(src: &str, keys: &'a [&'static str]) -> Vec<(usize, &'a str)> {
    std::iter::once(0).chain(src.match_indices("\n\n").map(|(at, _)| at + 2)).zip(keys.iter().copied()).collect()
}

#[test]
fn caller_block_keys_insert_remove_edit_and_retarget_mid_flight() {
    type DocState = (String, Vec<&'static str>);
    fn view((src, keys): &DocState, _: &mut Cx<DocState>) -> El {
        col().w(320.0).child(Markdown::new(src.clone()).block_keys(fixture_keys(src, keys)).build(&Kit::neutral(true)))
    }

    let mut rt = Runtime::new(("Alpha\n\nBeta".to_owned(), vec!["p-a", "p-b"]), view, (320.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let y_a0 = by_key(&rt, "p-a").rect[1];
    let y_b0 = by_key(&rt, "p-b").rect[1];
    assert_eq!(y_a0, 0.0);
    assert!(y_b0 > 20.0);

    // 1. Insert a block at the top: old blocks keep their keys and spring down; new block fades in.
    rt.state = ("Intro\n\nAlpha\n\nBeta".to_owned(), vec!["p-intro", "p-a", "p-b"]);
    rt.invalidate();
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    let intro = by_key(&rt, "p-intro");
    let a_mid = by_key(&rt, "p-a");
    let b_mid = by_key(&rt, "p-b");
    assert!(intro.opacity > 0.0 && intro.opacity < 1.0, "inserted block enters on its fade: {}", intro.opacity);
    assert!(a_mid.moving && a_mid.rect[1] > y_a0 && a_mid.rect[1] < y_b0, "p-a springs from old y to new y: {a_mid:?}");
    assert!(b_mid.moving && b_mid.rect[1] > y_b0, "p-b springs down with p-a: {b_mid:?}");
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    let y_a_landed = by_key(&rt, "p-a").rect[1];
    let y_b_landed = by_key(&rt, "p-b").rect[1];
    assert!((by_key(&rt, "p-intro").opacity - 1.0).abs() < 1e-3);
    assert!((y_a_landed - y_b0).abs() < 0.5 && y_b_landed > y_a_landed);

    // 2. Remove the middle block: it stays mounted while exiting and the trailing sibling springs up.
    rt.state = ("Intro\n\nBeta".to_owned(), vec!["p-intro", "p-b"]);
    rt.invalidate();
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    let a_exit = by_key(&rt, "p-a");
    let b_reflow = by_key(&rt, "p-b");
    assert!(a_exit.exiting && a_exit.opacity > 0.0 && a_exit.opacity < 1.0, "removed block stays mounted while exiting: {a_exit:?}");
    assert!(b_reflow.moving && b_reflow.rect[1] < y_b_landed && b_reflow.rect[1] > y_a_landed, "p-b springs up: {b_reflow:?}");
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.elements().iter().all(|e| e.key.as_deref() != Some("p-a")), "exited block unmounts after transition");
    assert!((by_key(&rt, "p-b").rect[1] - y_a_landed).abs() < 0.5);

    // 3. Edit a block in place and retarget mid-flight: height and sibling y continue smoothly without jumping.
    let h0 = by_key(&rt, "p-intro").rect[3];
    let y0 = by_key(&rt, "p-b").rect[1];
    rt.state = ("Intro line one  \nline two  \nline three\n\nBeta".to_owned(), vec!["p-intro", "p-b"]);
    rt.invalidate();
    rt.frame(0.0);
    for _ in 0..3 {
        rt.frame(1.0 / 60.0);
    }
    let h_mid = by_key(&rt, "p-intro").rect[3];
    let y_mid = by_key(&rt, "p-b").rect[1];
    assert!(h_mid > h0 && y_mid > y0, "height and sibling y are mid-spring: {h_mid} > {h0}, {y_mid} > {y0}");
    assert!(h_mid < by_key(&rt, "p-intro").target[3] && y_mid < by_key(&rt, "p-b").target[1],
        "editing animates toward the larger height and sibling position, not straight to them");

    rt.state = ("Intro line one  \nline two  \nline three  \nline four  \nline five\n\nBeta".to_owned(), vec!["p-intro", "p-b"]);
    rt.invalidate();
    rt.frame(0.0);
    let h_retarget = by_key(&rt, "p-intro").rect[3];
    let y_retarget = by_key(&rt, "p-b").rect[1];
    assert!((h_retarget - h_mid).abs() < 0.5 && (y_retarget - y_mid).abs() < 0.5, "retarget commit does not jump: {h_retarget} vs {h_mid}, {y_retarget} vs {y_mid}");
    rt.frame(1.0 / 60.0);
    assert!(by_key(&rt, "p-intro").rect[3] > h_retarget && by_key(&rt, "p-b").rect[1] > y_retarget);
}

#[test]
fn block_keys_fallback_nested_and_selectable() {
    fn view(keys: &Vec<&'static str>, _: &mut Cx<Vec<&'static str>>) -> El {
        let src = "# Heading\n\n> Quoted one\n>\n> Quoted two\n\nParagraph three";
        col().w(320.0).child(Markdown::new(src).block_keys(fixture_keys(src, keys)).selectable(true).reveal(0.15).build(&Kit::neutral(true)))
    }

    // Fewer keys, empty key and duplicate key fall back to unused b{i} keys without panicking;
    // nested blocks inside the quote keep their own b0, b1 keys under the quote's caller key.
    let mut rt = Runtime::new(vec!["sec-head", "sec-quote"], view, (320.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let paths: Vec<_> = rt.elements().into_iter().map(|e| e.path).collect();
    assert!(paths.iter().any(|p| p.ends_with("/sec-head")), "{paths:?}");
    assert!(paths.iter().any(|p| p.contains("/sec-quote/") && p.ends_with("/b0")) && paths.iter().any(|p| p.contains("/sec-quote/") && p.ends_with("/b1")), "{paths:?}");
    assert!(paths.iter().any(|p| p.ends_with("/b2")), "missing 3rd key falls back to b2: {paths:?}");

    // Extra keys, empty keys and duplicates never collide or panic.
    rt.state = vec!["", "dup", "dup", "extra-1", "extra-2"];
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    let top_keys: Vec<_> = rt.elements().into_iter().filter(|e| !e.exiting && e.path.matches('/').count() == 2).filter_map(|e| e.key).collect();
    assert_eq!(top_keys, ["b0", "dup", "b2"]);

    // Selection across caller-keyed blocks copies the whole document in order.
    rt.pointer_move(4.0, 6.0);
    rt.pointer_down(4.0, 6.0);
    rt.pointer_up(4.0, 6.0);
    rt.frame(1.0 / 60.0);
    let cmd = rune::Mods { cmd: true, ..Default::default() };
    rt.key(rune::KeyEvent { key: rune::Key::Command('a'), mods: cmd });
    rt.frame(1.0 / 60.0);
    rt.key(rune::KeyEvent { key: rune::Key::Command('c'), mods: cmd });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.copy_request.as_deref(), Some("Heading\nQuoted one\nQuoted two\nParagraph three"));
}

#[test]
fn source_keys_do_not_shift_when_parsers_disagree() {
    fn view(src: &String, _: &mut Cx<String>) -> El {
        // The caller splits a loose list into two blocks and keeps an HTML block that the
        // renderer omits. Neither boundary must steal the later paragraph's durable ID.
        let keys = [(0, "list"), (src.find("- second").unwrap(), "second-item"),
            (src.find("<div>").unwrap(), "html"), (src.find("After").unwrap(), "after")];
        col().w(320.0).child(Markdown::new(src.clone()).block_keys(keys).reveal(0.15)
            .selectable(true).streaming(true).build(&Kit::neutral(true)))
    }
    let mut rt = Runtime::new("- first\n\n- second\n\n<div>ignored</div>\n\nAfter".to_owned(), view,
        (320.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(by_key(&rt, "after").text.as_deref(), Some("After"));
    assert!(rt.elements().iter().all(|e| !matches!(e.key.as_deref(), Some("second-item" | "html"))));
    // Appending a partial block takes the incremental parse path; absolute offsets stay valid.
    rt.state.push_str(" grows\n\npartial");
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    assert_eq!(by_key(&rt, "after").text.as_deref(), Some("After grows"));
    assert!(by_key(&rt, "after").revealing, "appended words reveal under the retained key");
    assert_eq!(by_key(&rt, "b2").text.as_deref(), Some("partial"));
}

#[test]
fn nested_tasks_report_exact_source_marker_offsets_and_tab_skips_read_only() {
    let src = "Préface\n\n- plain\n  - [ ] nested\n- [x] outer\n\nTail\n\n- [ ] last\n";
    let mut rt = Runtime::new(TaskDoc { src: src.into(), handled: true, ..Default::default() },
        task_view, (320.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // Space without focus does nothing. Tab walks document order, including nested tasks.
    rt.key(rune::KeyEvent { key: rune::Key::Text(" ".into()), mods: Default::default() });
    rt.frame(1.0 / 60.0);
    assert!(rt.state.toggled.is_empty());
    for (n, (marker, key, checked)) in [("[ ] nested", "b1", true), ("[x] outer", "b1", false), ("[ ] last", "b3", true)].into_iter().enumerate() {
        rt.key(rune::KeyEvent { key: rune::Key::Tab, mods: Default::default() });
        rt.frame(1.0 / 60.0);
        rt.key(rune::KeyEvent { key: rune::Key::Text(" ".into()), mods: Default::default() });
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.toggled.len(), n + 1);
        assert_eq!(rt.state.toggled[n], (key.to_owned(), src.find(marker).unwrap(), checked));
    }
    rt.state.read_only = true;
    rt.state.toggled.clear();
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    for _ in 0..4 {
        rt.key(rune::KeyEvent { key: rune::Key::Tab, mods: Default::default() });
        rt.key(rune::KeyEvent { key: rune::Key::Text(" ".into()), mods: Default::default() });
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.state.toggled.is_empty());
    assert!(rt.elements().iter().filter(|e| e.role == "checkbox").all(|e| !e.focusable));
}

#[test]
fn task_controls_match_checkbox_paint_in_both_themes() {
    fn view(&(dark, checked): &(bool, bool), _: &mut Cx<(bool, bool)>) -> El {
        let kit = Kit::neutral(dark);
        col().w(320.0)
            .child(rune_kit::Checkbox::new(checked).label("Task").build(&kit).key("standard"))
            .child(Markdown::new(if checked { "- [x] Task" } else { "- [ ] Task" }).build(&kit).key("markdown"))
    }
    for dark in [false, true] {
        for checked in [false, true] {
            let mut rt = Runtime::new((dark, checked), view, (320.0, 160.0), Env::new(dark, 1.0));
            rt.frame(0.0);
            let controls: Vec<_> = rt.elements().into_iter().filter(|e| e.key.as_deref() == Some("control")).collect();
            assert_eq!(controls.len(), 2);
            assert_eq!((controls[0].fill, controls[0].radius, controls[0].rect[2..].to_vec()),
                (controls[1].fill, controls[1].radius, controls[1].rect[2..].to_vec()));
            let marks: Vec<_> = rt.elements().into_iter().filter(|e| e.key.as_deref() == Some("mark")).collect();
            assert_eq!((marks[0].opacity, marks[0].text_color, marks[0].rect[2..].to_vec()),
                (marks[1].opacity, marks[1].text_color, marks[1].rect[2..].to_vec()));
        }
    }
}

#[derive(Default)]
struct TaskDoc {
    src: String,
    keys: Vec<&'static str>,
    handled: bool,
    read_only: bool,
    toggled: Vec<(String, usize, bool)>,
}

fn task_view(s: &TaskDoc, _: &mut Cx<TaskDoc>) -> El {
    let mut md = Markdown::new(s.src.clone());
    if !s.keys.is_empty() {
        md = md.block_keys(fixture_keys(&s.src, &s.keys));
    }
    if s.handled {
        md = md.on_task_tx(|s: &mut TaskDoc, key, item, checked, tx| {
            s.toggled.push((key.to_owned(), item, checked));
            tx.copy(format!("{key}:{item}:{checked}"));
        });
    }
    if s.read_only {
        md = md.tasks_read_only();
    }
    col().w(320.0).child(md.build(&Kit::neutral(true)))
}

#[test]
fn task_lists_render_checkboxes_press_space_and_respect_read_only() {
    let k = Kit::neutral(true);
    let mut rt = Runtime::new(
        TaskDoc {
            src: "Notes\n\n- [ ] a\n- [x] b\n".to_owned(),
            keys: vec!["intro", "checklist"],
            handled: true,
            read_only: false,
            toggled: Vec::new(),
        },
        task_view,
        (320.0, 300.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    let boxes: Vec<_> = rt.elements().into_iter().filter(|e| e.role == "checkbox").collect();
    assert_eq!(boxes.len(), 2, "two checkbox elements: {boxes:?}");
    assert_eq!((boxes[0].label.as_deref(), boxes[0].item_selected, boxes[0].focusable), (Some("a"), false, true));
    assert_eq!((boxes[1].label.as_deref(), boxes[1].item_selected, boxes[1].focusable), (Some("b"), true, true));

    let marks: Vec<_> = rt.elements().into_iter().filter(|e| e.key.as_deref() == Some("mark")).collect();
    assert_eq!((marks[0].opacity, marks[1].opacity), (0.0, 1.0));
    let controls: Vec<_> = rt.elements().into_iter().filter(|e| e.key.as_deref() == Some("control")).collect();
    assert_eq!((controls[0].fill, controls[1].fill), (Color::TRANSPARENT, k.c.surface_weak));

    let click = |rt: &mut Runtime<TaskDoc>, r: [f32; 4]| {
        let (x, y) = (r[0] + 12.0, r[1] + r[3] * 0.5);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    };

    // Each callback identifies the source marker, not an index among tasks or list items.
    click(&mut rt, boxes[0].rect);
    assert_eq!(rt.state.toggled, [("checklist".to_owned(), 9, true)]);
    assert_eq!(rt.copy_request.as_deref(), Some("checklist:9:true"));

    click(&mut rt, boxes[1].rect);
    assert_eq!(rt.state.toggled, [("checklist".to_owned(), 9, true), ("checklist".to_owned(), 17, false)]);
    assert_eq!(rt.copy_request.as_deref(), Some("checklist:17:false"));

    // Space on the focused task checkbox toggles it through the same handler.
    rt.key(rune::KeyEvent { key: rune::Key::Text(" ".into()), mods: Default::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.toggled.last(), Some(&("checklist".to_owned(), 17, false)));
    assert_eq!(rt.state.toggled.len(), 3, "Space fires once on the focused checkbox");
    rt.key(rune::KeyEvent { key: rune::Key::Enter, mods: Default::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.toggled.len(), 3, "checkboxes toggle with Space, not Enter");

    // Without caller block_keys, on_task receives the default b{index} key.
    rt.state.keys.clear();
    rt.state.toggled.clear();
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    click(&mut rt, boxes[0].rect);
    assert_eq!(rt.state.toggled, [("b1".to_owned(), 9, true)]);

    // `.tasks_read_only()` and missing `on_task` keep role/label/checked state but ignore presses and Space.
    for (handled, read_only) in [(true, true), (false, false)] {
        rt.state.handled = handled;
        rt.state.read_only = read_only;
        rt.state.toggled.clear();
        rt.invalidate();
        rt.frame(1.0 / 60.0);
        let ro_boxes: Vec<_> = rt.elements().into_iter().filter(|e| !e.exiting && e.role == "checkbox").collect();
        assert_eq!(ro_boxes.len(), 2);
        assert!(!ro_boxes[0].focusable && !ro_boxes[1].focusable);
        assert_eq!((ro_boxes[0].item_selected, ro_boxes[1].item_selected), (false, true));
        click(&mut rt, ro_boxes[0].rect);
        rt.key(rune::KeyEvent { key: rune::Key::Text(" ".into()), mods: Default::default() });
        rt.frame(1.0 / 60.0);
        assert!(rt.state.toggled.is_empty(), "read-only tasks must not fire: handled={handled} read_only={read_only}");
    }
}

#[test]
fn link_ring_frames_chosen_links_merges_inner_runs_and_stays_on_hover() {
    const CHIP_RING: &str = "#3b82f6";

    fn view(_: &(), _: &mut Cx<()>) -> El {
        let bg = rune::color::hex(CHIP_BG).unwrap();
        let hover = rune::color::hex(CHIP_HOVER).unwrap();
        let ring = rune::color::hex(CHIP_RING).unwrap();
        Markdown::new("See [Session **bold** `code`](session:abc) and [Docs](https://a.dev).")
            .link_style(move |url| {
                url.starts_with("session:").then(|| {
                    (rune::Span::new(0, 0).color(Color::WHITE).weight(500.0).bg(bg, 3.0), LinkPaint::new().bg(hover))
                })
            })
            .link_ring(move |url| url.starts_with("session:").then_some((ring, 1.0)))
            .on_link(|_: &mut (), _| {})
            .build(&Kit::neutral(true))
    }

    let mut rt = Runtime::new((), view, (320.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let want_ring = rune::color::hex(CHIP_RING).unwrap().premul(1.0);
    let ringed: Vec<_> = rt
        .instances
        .iter()
        .filter(|i| i.params[1] == rune::render::KIND_RECT && i.params[0] > 0.0)
        .copied()
        .collect();
    assert_eq!(ringed.len(), 1, "only the session: link gets a ring, merged across plain/bold/code runs: {ringed:?}");
    assert_eq!((ringed[0].radii, ringed[0].params[0], ringed[0].border), ([4.0; 4], 1.0, want_ring));
    assert_eq!(fill_rects(&rt, CHIP_BG), [ringed[0].rect]);

    // Hovering the https link adds no ring and leaves the session chip's ring alone.
    let (hx, hy) = centre(&rt, "https://a.dev");
    rt.pointer_move(hx, hy);
    rt.frame(1.0 / 60.0);
    assert_eq!(
        rt.instances.iter().filter(|i| i.params[1] == rune::render::KIND_RECT && i.params[0] > 0.0).count(),
        1
    );

    // Hovering the session link swaps its fill to CHIP_HOVER and keeps the ring.
    let (sx, sy) = centre(&rt, "session:abc");
    rt.pointer_move(sx, sy);
    rt.frame(1.0 / 60.0);
    assert_eq!(fill_rects(&rt, CHIP_HOVER), [ringed[0].rect]);
    let hovered: Vec<_> = rt
        .instances
        .iter()
        .filter(|i| i.params[1] == rune::render::KIND_RECT && i.params[0] > 0.0)
        .copied()
        .collect();
    assert_eq!(hovered.len(), 1);
    assert_eq!((hovered[0].rect, hovered[0].params[0], hovered[0].border), (ringed[0].rect, 1.0, want_ring));
}

#[test]
fn strikethrough_parses_and_paints_in_light_and_dark_themes() {
    let src = "Keep ~~struck~~ and ~~**bold** `code`~~ in\n\n- ~~item~~\n\n| Col |\n|---|\n| ~~cell~~ |\n";
    let b = parse(src);
    let Block::Para(p) = &b[0] else { panic!("{b:?}") };
    assert_eq!(p.text, "Keep struck and bold \u{202f}\u{202f}code\u{202f}\u{202f} in");
    let struck: Vec<&str> = p
        .spans
        .iter()
        .filter(|s| matches!(s.2, Style::Strike))
        .map(|&(a, z, _)| &p.text[a..z])
        .collect();
    assert_eq!(struck, ["struck", "bold \u{202f}\u{202f}code\u{202f}\u{202f}"]);

    fn view(&(dark, strike): &(bool, bool), _: &mut Cx<(bool, bool)>) -> El {
        let k = Kit::neutral(dark);
        let doc = if strike {
            "Keep ~~struck~~ and ~~**bold** `code`~~\n\n- ~~item~~\n\n| Col |\n|---|\n| ~~cell~~ |\n"
        } else {
            "Keep struck and **bold** `code`\n\n- item\n\n| Col |\n|---|\n| cell |\n"
        };
        col().w(360.0).child(Markdown::new(doc).build(&k))
    }

    for dark in [false, true] {
        let mut plain = Runtime::new((dark, false), view, (360.0, 300.0), Env::new(dark, 1.0));
        plain.frame(0.0);
        let mut struck_rt = Runtime::new((dark, true), view, (360.0, 300.0), Env::new(dark, 1.0));
        struck_rt.frame(0.0);

        let thin_rects = |rt: &Runtime<(bool, bool)>| -> Vec<[f32; 4]> {
            rt.instances
                .iter()
                .filter(|i| i.params[1] == rune::render::KIND_RECT && i.rect[3] >= 0.9 && i.rect[3] <= 2.5 && i.rect[2] > 12.0)
                .map(|i| i.rect)
                .collect()
        };
        let extra = thin_rects(&struck_rt).len().saturating_sub(thin_rects(&plain).len());
        assert!(extra >= 4, "dark={dark}: strike lines drawn for paragraph, list item and table cell ({extra} new lines)");
    }
}

#[test]
fn bare_urls_autolink_with_gfm_punctuation_rules_and_respect_opt_out() {
    let src = concat!(
        "Visit https://example.org/docs?x=1#top, http://example.org:8080/a; ",
        "and www.example.org/path/to_file! ",
        "Also (see https://en.wikipedia.org/wiki/Rust_(programming_language)) ",
        "and https://example.org/entity&nbsp; ",
        "but not `https://in.code` or [explicit](https://explicit.example/x).\n\n",
        "```\nhttps://in.fence.example\n```\n",
    );
    let b = parse(src);
    let Block::Para(p) = &b[0] else { panic!("{b:?}") };
    let links: Vec<(&str, &str)> = p.links.iter().map(|(a, z, u)| (&p.text[*a..*z], u.as_str())).collect();
    assert_eq!(
        links,
        [
            ("https://example.org/docs?x=1#top", "https://example.org/docs?x=1#top"),
            ("http://example.org:8080/a", "http://example.org:8080/a"),
            ("www.example.org/path/to_file", "http://www.example.org/path/to_file"),
            (
                "https://en.wikipedia.org/wiki/Rust_(programming_language)",
                "https://en.wikipedia.org/wiki/Rust_(programming_language)",
            ),
            ("https://example.org/entity", "https://example.org/entity"),
            ("explicit", "https://explicit.example/x"),
        ]
    );

    // Pressing a bare `www.` link sends its `http://` destination through `on_link`, and `.autolink(false)` leaves bare URLs plain.
    struct AutoState {
        autolink: bool,
        opened: Vec<String>,
    }
    fn auto_view(s: &AutoState, _: &mut Cx<AutoState>) -> El {
        col().w(480.0).child(
            Markdown::new("Try www.example.org/hello, or [named](https://named.example).")
                .autolink(s.autolink)
                .on_link(|s: &mut AutoState, url| s.opened.push(url.to_owned()))
                .build(&Kit::neutral(true)),
        )
    }
    let mut rt = Runtime::new(AutoState { autolink: true, opened: Vec::new() }, auto_view, (480.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let (x, y) = rt.find("http://www.example.org/hello").expect("bare www link is findable by normalized URL");
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.opened, ["http://www.example.org/hello"]);

    rt.state.autolink = false;
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    assert!(rt.find("http://www.example.org/hello").is_none(), "autolink(false) disables bare URL links");
    assert!(rt.find("https://named.example").is_some(), "explicit links remain active");
}

#[test]
fn highlights_map_source_bytes_across_formatting_blocks_and_syntax_without_reshaping() {
    let src = concat!(
        "# Heading Alpha\n\n",
        "Plain **bold token** and `inline_code` plus ~~struck_word~~ and [linked text](https://example.org/hidden).\n\n",
        "Second paragraph continues here.\n\n",
        "- list item one\n",
        "- [x] task needle done\n\n",
        "> quoted thoughts\n\n",
        "| Col A | Col B |\n",
        "|---|---|\n",
        "| cell_alpha | cell_beta |\n\n",
        "```rust\nfn compute(value: i32) -> i32 {\n    value + 1\n}\n```\n",
    );

    let range_of = |needle: &str| -> std::ops::Range<usize> {
        let start = src.find(needle).unwrap_or_else(|| panic!("no {needle:?}"));
        start..start + needle.len()
    };

    // Include:
    // 0: heading substring
    // 1: span across plain -> **bold token** -> plain ("Plain **bold")
    // 2: inside inline code ("inline_code")
    // 3: inside strikethrough ("struck")
    // 4: spanning across first and second paragraphs ("hidden).\n\nSecond")
    // 5: inside list item ("item one")
    // 6: inside task item ("needle")
    // 7: inside quote ("thoughts")
    // 8: inside table cell ("cell_beta")
    // 9: inside code block ("compute(value") - marked as current!
    // 10: syntax-only range ("**") - must produce no visible highlight
    // 11: link destination syntax ("(https://example.org/hidden)") - must produce no visible highlight
    let bold_stars = src.find("**bold").unwrap();
    let link_dest = src.find("(https://example.org/hidden)").unwrap();
    let highlights = vec![
        (range_of("Alpha"), Highlight::new()),
        (range_of("Plain **bold"), Highlight::new()),
        (range_of("inline_code"), Highlight::new()),
        (range_of("struck"), Highlight::new()),
        (range_of("linked text](https://example.org/hidden).\n\nSecond"), Highlight::new()),
        (range_of("item one"), Highlight::new()),
        (range_of("needle"), Highlight::new()),
        (range_of("thoughts"), Highlight::new()),
        (range_of("cell_beta"), Highlight::new()),
        (range_of("compute(value"), Highlight::current().key("active-match")),
        (bold_stars..bold_stars + 2, Highlight::new().key("syntax-only-stars")),
        (link_dest..link_dest + "(https://example.org/hidden)".len(), Highlight::new().key("syntax-only-url")),
    ];

    type HlState = (String, Vec<(std::ops::Range<usize>, Highlight)>);
    fn hl_view((src, hls): &HlState, _: &mut Cx<HlState>) -> El {
        let k = Kit::neutral(true);
        let style = MarkdownStyle::from_kit(&k)
            .highlight_bg(rune::color::hex("#eab30838").unwrap())
            .highlight_current_bg(rune::color::hex("#f59e0b66").unwrap())
            .highlight_current_ring(Some((rune::color::hex("#f59e0b").unwrap(), 1.5)));
        col().w(520.0).child(Markdown::new(src.clone()).style(style).highlights(hls).build(&k))
    }
    fn render_with(src: &str, hls: &[(std::ops::Range<usize>, Highlight)]) -> Runtime<HlState> {
        let mut rt = Runtime::new((src.to_owned(), hls.to_vec()), hl_view, (520.0, 900.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt
    }

    let plain_rt = render_with(src, &[]);
    let hl_rt = render_with(src, &highlights);

    // 1. Every glyph quad in `hl_rt` is in the exact same position and UV as in `plain_rt` (no text reshaping!).
    let glyphs = |rt: &Runtime<HlState>| -> Vec<([u32; 4], [u32; 4])> {
        rt.instances
            .iter()
            .filter(|i| i.params[1] == rune::render::KIND_GLYPH)
            .map(|i| (i.rect.map(f32::to_bits), i.uv.map(f32::to_bits)))
            .collect()
    };
    assert_eq!(glyphs(&plain_rt), glyphs(&hl_rt), "highlights must never reshape text or move glyphs");

    // 2. Check exposed mark elements and labels.
    let marks: Vec<_> = hl_rt
        .elements()
        .into_iter()
        .filter(|e| e.role == "mark")
        .map(|e| (e.key.unwrap_or_default(), e.label.unwrap_or_default()))
        .collect();
    assert!(marks.contains(&("md-mark-0".into(), "Alpha".into())), "{marks:?}");
    assert!(marks.contains(&("md-mark-1".into(), "Plain bold".into())), "{marks:?}");
    assert!(marks.contains(&("md-mark-2".into(), "inline_code".into())), "{marks:?}");
    assert!(marks.contains(&("md-mark-3".into(), "struck".into())), "{marks:?}");
    // Cross-block highlight (#4) produces two mark elements: "linked text." in para 1 and "Second" in para 2.
    let cross: Vec<&str> = marks.iter().filter(|(k, _)| k == "md-mark-4").map(|(_, l)| l.as_str()).collect();
    assert_eq!(cross, ["linked text.", "Second"]);
    assert!(marks.contains(&("md-mark-5".into(), "item one".into())), "{marks:?}");
    assert!(marks.contains(&("md-mark-6".into(), "needle".into())), "{marks:?}");
    assert!(marks.contains(&("md-mark-7".into(), "thoughts".into())), "{marks:?}");
    assert!(marks.contains(&("md-mark-8".into(), "cell_beta".into())), "{marks:?}");
    assert!(marks.contains(&("active-match".into(), "compute(value".into())), "{marks:?}");

    // Syntax-only ranges produce no mark elements at all.
    assert!(!marks.iter().any(|(k, _)| k.starts_with("syntax-only")), "syntax-only ranges must not highlight: {marks:?}");

    // 3. Current match draws its inset ring, while standard matches draw rounded bg rects without a ring.
    let want_ring = rune::color::hex("#f59e0b").unwrap().premul(1.0);
    let ringed: Vec<_> = hl_rt
        .instances
        .iter()
        .filter(|i| i.params[1] == rune::render::KIND_RECT && (i.params[0] - 1.5).abs() < 1e-3 && i.border == want_ring)
        .collect();
    assert_eq!(ringed.len(), 1, "current match gets its 1.5pt ring: {ringed:?}");
}

#[test]
fn scroll_into_view_scrolls_to_highlighted_match_in_tall_document_and_code_block() {
    let mut doc = String::from("# Searchable Document\n\n");
    for i in 0..25 {
        doc.push_str(&format!("Paragraph {i} with filler words to push later content below the fold.\n\n"));
    }
    doc.push_str("Paragraph with the first_target_match in body text.\n\n");
    doc.push_str("```rust\n");
    for i in 0..30 {
        doc.push_str(&format!("let line_{i} = {i};\n"));
    }
    doc.push_str("let deep_code_target = 42;\n```\n");

    let first_range = {
        let s = doc.find("first_target_match").unwrap();
        s..s + "first_target_match".len()
    };
    let code_range = {
        let s = doc.find("deep_code_target").unwrap();
        s..s + "deep_code_target".len()
    };

    struct SearchApp {
        doc: String,
        r1: std::ops::Range<usize>,
        r2: std::ops::Range<usize>,
    }

    fn search_view(s: &SearchApp, _: &mut Cx<SearchApp>) -> El {
        let hls = [
            (s.r1.clone(), Highlight::new()),
            (s.r2.clone(), Highlight::current().key("code-hit")),
        ];
        col()
            .key("doc-scroll")
            .w_full()
            .h(200.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(Markdown::new(s.doc.clone()).highlights(&hls).build(&Kit::neutral(true)))
    }

    let mut rt = Runtime::new(
        SearchApp { doc, r1: first_range, r2: code_range },
        search_view,
        (480.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    // Initially both targets are well below the 200pt viewport.
    let mark_y = |rt: &Runtime<SearchApp>, key: &str| -> f32 {
        by_key(rt, key).rect[1]
    };
    let y1_initial = mark_y(&rt, &Markdown::highlight_key(0));
    let y2_initial = mark_y(&rt, "code-hit");
    assert!(y1_initial > 400.0 && y2_initial > y1_initial + 300.0, "initially below fold: {y1_initial}, {y2_initial}");

    // Scroll to the first highlight using its default key `Markdown::highlight_key(0)`.
    rt.update(|_, tx| tx.scroll_into_view(Markdown::highlight_key(0)));
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    let m1 = by_key(&rt, &Markdown::highlight_key(0)).rect;
    assert!(m1[1] >= 0.0 && m1[1] + m1[3] <= 200.5, "first highlight scrolled into viewport: {m1:?}");

    // Scroll to the deep code-block highlight using its custom key `"code-hit"`.
    rt.update(|_, tx| tx.scroll_into_view("code-hit"));
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    let m2 = by_key(&rt, "code-hit").rect;
    assert!(m2[1] >= 0.0 && m2[1] + m2[3] <= 200.5, "deep code-block highlight scrolled into viewport: {m2:?}");
}

#[test]
fn review_highlight_mapping_edges_and_streaming() {
    type State = (String, Vec<(std::ops::Range<usize>, Highlight)>);
    fn view((src, marks): &State, _: &mut Cx<State>) -> El {
        col().w(480.0).child(Markdown::new(src.as_str()).highlights(marks).build(&Kit::neutral(true)))
    }
    for (src, needle, expected) in [
        ("before &amp; after", "amp", "&"),
        ("before \\* after", "*", "*"),
        ("before <b>after</b>", "after", "after"),
        ("one\r\ntwo", "two", "two"),
        ("one  \r\ntwo", "two", "two"),
        ("> - nested **needle**", "needle", "needle"),
        ("left שלום world right", "שלום world", "שלום world"),
        ("| a | b |\n|---|---|\n| a\\|needle | end |", "needle", "needle"),
        ("| a |\n|---|\n| `a\\|needle` |", "needle", "needle"),
        ("`a\r\nb needle`", "needle", "needle"),
        ("`a\r\nbb cc dd ee ff gg hh`", "bb", "bb"),
        ("` a\r\nbb cc dd ee ff gg hh `", "bb", "bb"),
        ("`a bb cc dd ee ff gg\r\nhh ii`", "bb", "bb"),
        ("`a\r\nbb cc dd ee ff gg hh`", "gg", "gg"),
        ("> `one\n> two three four five`", "three", "three"),
        ("- `one\n  two three four five`", "three", "three"),
        ("https://example.org/needle", "needle", "needle"),
        ("reference [^needle]\n\n[^needle]: text", "needle", "needle"),
    ] {
        let start = src.find(needle).unwrap();
        let mut rt = Runtime::new((src.into(), vec![(start..start + needle.len(), Highlight::new())]), view, (480.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let labels: Vec<_> = rt.elements().into_iter().filter(|e| e.role == "mark").filter_map(|e| e.label).collect();
        assert_eq!(labels, [expected], "source={src:?}");
    }
    let src = "αβ **needle**\n\nsecond https://example.org/path";
    let start = src.find("needle").unwrap();
    let marks = vec![(start..start + 6, Highlight::new()), (1..3, Highlight::current()), (usize::MAX - 1..usize::MAX, Highlight::new())];
    let mut rt = Runtime::new((String::new(), marks), view, (480.0, 200.0), Env::new(true, 1.0));
    for end in 0..=src.len() {
        if !src.is_char_boundary(end) { continue; }
        rt.state.0 = src[..end].into();
        rt.invalidate();
        rt.frame(0.0);
        for e in rt.elements().into_iter().filter(|e| e.role == "mark") {
            assert_ne!(e.key.as_deref(), Some("md-mark-2"));
        }
    }
    rt.state.1 = vec![(start..start + 6, Highlight::current().fg(rune::Color::WHITE))];
    rt.invalidate();
    rt.frame(0.0);
    assert_eq!(rt.stats.shaped, 0, "changing highlights including foreground must reuse the shape cache");
}

#[test]
fn review_marks_do_not_wrap_large_offsets_or_intercept_links() {
    struct State { opened: Vec<String> }
    fn view(_: &State, _: &mut Cx<State>) -> El {
        let src = "~~https://example.org/path~~ and שלום world";
        col().w(220.0).child(Markdown::new(src).selectable(true)
            .highlights([(10..24, Highlight::new()), (2..26, Highlight::current().key("hit"))])
            .on_link(|s: &mut State, url| s.opened.push(url.into())).build(&Kit::neutral(true)))
            .child(rune::text("plain").text_highlight(rune::TextHighlight::new(
                (u32::MAX as usize + 1)..(u32::MAX as usize + 5), rune::Color::WHITE).key("outside")))
    }
    let mut rt = Runtime::new(State { opened: Vec::new() }, view, (220.0, 180.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(rt.find("outside").is_none(), "large offsets must not wrap to the beginning of text");
    assert!(!rt.elements().iter().any(|e| e.key.as_deref() == Some("outside")));
    let (x, y) = rt.find("hit").unwrap();
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    assert_eq!(rt.state.opened, ["https://example.org/path"]);
}

#[test]
fn review_autolinks_skip_images_emails_and_unsafe_schemes() {
    let src = "HTTP://example.org/a, ![https://image.example/alt](image.png) [https://label.example](https://dest.example) <https://angle.example> `https://code.example` foo@bar.com javascript:alert(1) data:text/plain,test";
    let blocks = parse(src);
    let Block::Para(p) = &blocks[0] else { panic!("{blocks:?}") };
    assert_eq!(p.links.iter().map(|(_, _, u)| u.as_str()).collect::<Vec<_>>(),
        ["HTTP://example.org/a", "https://dest.example", "https://angle.example"]);
}

#[test]
fn review_two_thousand_matches_add_no_layout_nodes_or_shapes() {
    type State = (String, bool);
    fn view((src, marked): &State, _: &mut Cx<State>) -> El {
        let ranges = if *marked { src.match_indices("needle").map(|(i, _)| (i..i + 6, Highlight::new())).collect() } else { Vec::new() };
        col().w(480.0).h(200.0).scroll(ScrollSpec { stick_bottom: false })
            .child(Markdown::new(src.as_str()).highlights(ranges).build(&Kit::neutral(true)))
    }
    let src = "needle needle needle needle with some fictional filler words.\n\n".repeat(500);
    let mut rt = Runtime::new((src, false), view, (480.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let nodes = rt.stats.layout_nodes;
    rt.state.1 = true;
    rt.invalidate();
    rt.frame(0.0);
    assert_eq!(rt.stats.shaped, 0);
    assert_eq!(rt.stats.layout_nodes, nodes);
    assert_eq!(rt.elements().iter().filter(|e| e.role == "mark").count(), 2000);
    assert!(rt.find("md-mark-1999").is_some());
}
