//! Markdown tables on a card: `MarkdownStyle::table_frame`, `table_body`, `table_header_pad`,
//! `table_full_width`, `table_margin` and the `table_card` preset.
#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]
#![cfg(feature = "markdown")]

use rune::runtime::ElementInfo;
use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, col};
use rune_kit::{Kit, Markdown, MarkdownStyle};

const SRC: &str = "Before.\n\n| | West | Centre |\n|---|---|---|\n| North | Lettuce ×2 | Tomato ×2 |\n| Middle | Wheat ×1 | Feed box |\n| South | Ranch | Kitchen |\n\nAfter.\n";

struct Doc {
    src: String,
    style: fn(&Kit) -> MarkdownStyle,
    width: f32,
    streaming: bool,
    scroll_wide: bool,
}

fn view(d: &Doc, _: &mut Cx<Doc>) -> El {
    let k = Kit::neutral(true);
    let md = Markdown::new(d.src.clone()).streaming(d.streaming).scroll_wide(d.scroll_wide).selectable(true).copy_source(true);
    col().w_full().child(col().w(d.width).child(md.render(&(d.style)(&k)).key("note")))
}

fn card(k: &Kit) -> MarkdownStyle {
    MarkdownStyle::from_kit(k).table_card(k)
}

fn plain(k: &Kit) -> MarkdownStyle {
    MarkdownStyle::from_kit(k)
}

fn run(src: &str, style: fn(&Kit) -> MarkdownStyle, width: f32) -> Runtime<Doc> {
    run_with(Doc { src: src.to_owned(), style, width, streaming: false, scroll_wide: false })
}

fn run_with(d: Doc) -> Runtime<Doc> {
    let mut rt = Runtime::new(d, view, (900.0, 800.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt
}

fn by_key(rt: &Runtime<Doc>, key: &str) -> ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}"))
}

fn has(rt: &Runtime<Doc>, key: &str) -> bool {
    rt.elements().iter().any(|e| e.key.as_deref() == Some(key))
}

fn parent(rt: &Runtime<Doc>, key: &str) -> ElementInfo {
    let all = rt.elements();
    all[by_key(rt, key).parent.unwrap()].clone()
}

fn text(rt: &Runtime<Doc>, t: &str) -> ElementInfo {
    rt.elements().into_iter().find(|e| e.text.as_deref() == Some(t)).unwrap_or_else(|| panic!("no {t}"))
}

/// Cell `c{j}` of the row keyed `row`.
fn cell(rt: &Runtime<Doc>, row: &str, j: usize) -> ElementInfo {
    let all = rt.elements();
    let r = all.iter().position(|e| e.key.as_deref() == Some(row)).unwrap();
    let key = format!("c{j}");
    all.into_iter().find(|e| e.parent == Some(r) && e.key.as_deref() == Some(key.as_str())).unwrap()
}

#[test]
fn the_body_rows_sit_on_a_raised_card_inset_in_the_frame() {
    let k = Kit::neutral(true);
    let rt = run(SRC, card, 600.0);
    let frame = parent(&rt, "th");
    let (th, tb, tbc) = (by_key(&rt, "th"), by_key(&rt, "tb"), by_key(&rt, "tbc"));
    assert_eq!((frame.radius, frame.fill), (6.0, k.c.layer_01));
    assert_eq!((tb.radius, tb.fill), (4.0, k.c.bg_base));
    assert_eq!(parent(&rt, "tb").path, frame.path, "the card is the frame's");
    assert_eq!(parent(&rt, "tbc").path, tb.path);
    assert_eq!(th.fill, rune::Color::TRANSPARENT, "the header sits on the frame");
    // 2 points of frame around the header and the card.
    assert_eq!(th.rect[0] - frame.rect[0], 2.0);
    assert_eq!(th.rect[1] - frame.rect[1], 2.0);
    assert_eq!(tb.rect[1], th.rect[1] + th.rect[3], "no rule between the header and the card");
    assert_eq!(frame.rect[0] + frame.rect[2] - (tb.rect[0] + tb.rect[2]), 2.0);
    assert_eq!(frame.rect[1] + frame.rect[3] - (tb.rect[1] + tb.rect[3]), 2.0);
    assert_eq!(tbc.rect, tb.rect);
    let raised: Vec<_> = k.shadow_layers(&k.elevation.raised).collect();
    assert_eq!(rt.presented_pose("tb").unwrap().shadows(), &raised[..], "the card is raised");
    // Rules between body rows only: none under the header, none after the last row.
    assert!(!has(&rt, "s0") && !has(&rt, "s-end") && !has(&rt, "s3"));
    for (i, key) in ["s1", "s2"].into_iter().enumerate() {
        let rule = by_key(&rt, key);
        assert_eq!(parent(&rt, key).path, tbc.path);
        assert_eq!((rule.fill, rule.rect[3]), (k.c.border_muted, 0.5));
        let above = by_key(&rt, &format!("r{i}")).rect;
        assert_eq!(rule.rect[1], above[1] + above[3], "{key} sits under r{i}");
    }
    let last = by_key(&rt, "r2").rect;
    assert_eq!(last[1] + last[3], tb.rect[1] + tb.rect[3], "the last row ends the card");
}

#[test]
fn the_card_preset_sets_its_cells_header_and_padding() {
    let k = Kit::neutral(true);
    let rt = run(SRC, card, 600.0);
    let (west, north) = (text(&rt, "West"), text(&rt, "North"));
    assert_eq!(west.text_color, Some(k.c.text_faint), "a faint header");
    assert_eq!(north.text_color, Some(k.c.text_base));
    assert_eq!(by_key(&rt, "th").rect[3], 16.0 + 2.0 * 8.0, "the header's 16-point line padded 8");
    assert_eq!(by_key(&rt, "r0").rect[3], 16.0 + 2.0 * 10.0, "body rows padded 10");
    assert_eq!(west.rect[0] - cell(&rt, "th", 1).rect[0], 12.0);
    assert_eq!(north.rect[0] - cell(&rt, "r0", 0).rect[0], 12.0);
    assert_eq!(text(&rt, "Lettuce ×2").rect[0], west.rect[0], "header and body columns line up");
}

#[test]
fn header_padding_can_differ_from_the_cells() {
    fn style(k: &Kit) -> MarkdownStyle {
        MarkdownStyle::from_kit(k).table_cell_padding(6.0, 4.0).table_header_pad(20.0, 9.0)
    }
    let rt = run("| Head | B |\n|---|---|\n| x | y |\n", style, 600.0);
    assert_eq!(text(&rt, "Head").rect[0] - cell(&rt, "th", 0).rect[0], 20.0);
    assert_eq!(text(&rt, "x").rect[0] - cell(&rt, "r0", 0).rect[0], 6.0);
    let head = text(&rt, "Head");
    assert_eq!(cell(&rt, "th", 0).rect[2], head.rect[2] + 40.0, "the column holds the header and its padding");
    assert_eq!(by_key(&rt, "th").rect[3] - head.rect[3], 18.0);
    assert_eq!(by_key(&rt, "r0").rect[3] - text(&rt, "x").rect[3], 8.0);
}

#[test]
fn full_width_fills_the_text_and_shares_the_room_in_proportion() {
    fn wide(k: &Kit) -> MarkdownStyle {
        MarkdownStyle::from_kit(k).table_full_width(true)
    }
    let src = "| a | bbbbbbbbbbbb |\n|---|---|\n| 1 | 2 |\n";
    let (natural, full) = (run(src, plain, 600.0), run(src, wide, 600.0));
    let frame = parent(&full, "th");
    assert_eq!(frame.rect[2], 600.0, "the frame fills the text's width");
    assert!(parent(&natural, "th").rect[2] < 300.0);
    let [n0, n1] = [0, 1].map(|j| cell(&natural, "th", j).rect[2]);
    let [f0, f1] = [0, 1].map(|j| cell(&full, "th", j).rect[2]);
    assert!((f0 + f1 - (600.0 - 1.0)).abs() < 0.5, "the columns fill it: {f0} + {f1}");
    assert!((f0 / f1 - n0 / n1).abs() < 0.01, "in proportion: {f0}/{f1} vs {n0}/{n1}");
    assert_eq!(by_key(&full, "s0").rect[2], 599.0, "rules stretch with it");

    let card = run(SRC, card, 600.0);
    assert_eq!(parent(&card, "th").rect[2], 600.0);
    assert_eq!(by_key(&card, "tb").rect[2], 596.0, "the card stretches with the frame");
}

#[test]
fn full_width_scrolling_fills_the_viewport_and_scrolls_a_wide_table_at_its_natural_width() {
    let long = "| Term | Meaning |\n|---|---|\n| cache | a sentence much too long for one line of a narrow column, so it wraps |\n";
    let narrow = run_with(Doc { src: "| a | b |\n|---|---|\n| 1 | 2 |\n".into(), style: card, width: 400.0, streaming: false, scroll_wide: true });
    assert_eq!(parent(&narrow, "th").rect[2], 400.0);
    let rt = run_with(Doc { src: long.into(), style: card, width: 300.0, streaming: false, scroll_wide: true });
    let (term, meaning) = (text(&rt, "cache"), rt.elements().into_iter().find(|e| e.text.as_deref().is_some_and(|t| t.starts_with("a sentence"))).unwrap());
    assert_eq!(meaning.rect[3], term.rect[3], "one line: {meaning:?}");
    let frame = parent(&rt, "th");
    assert!(frame.rect[2] > 300.0, "the frame runs past the viewport");
    assert_eq!(rt.elements()[frame.parent.unwrap()].rect[2], 300.0, "the viewport keeps to the text");
}

#[test]
fn table_margin_sets_the_space_around_a_table_only() {
    fn spaced(k: &Kit) -> MarkdownStyle {
        MarkdownStyle::from_kit(k).table_margin(30.0, 40.0)
    }
    let gaps = |rt: &Runtime<Doc>| {
        let [before, table, after] = ["b0", "b1", "b2"].map(|k| by_key(rt, k).rect);
        (table[1] - (before[1] + before[3]), after[1] - (table[1] + table[3]))
    };
    assert_eq!(gaps(&run(SRC, plain, 600.0)), (12.0, 16.0), "today's spacing by default");
    assert_eq!(gaps(&run(SRC, spaced, 600.0)), (30.0, 40.0));
    assert_eq!(gaps(&run(SRC, card, 600.0)), (16.0, 16.0));
    let para = "One.\n\nTwo.\n";
    let [a, b] = [plain, spaced].map(|s| {
        let rt = run(para, s, 600.0);
        by_key(&rt, "b1").rect[1] - by_key(&rt, "b0").rect[1]
    });
    assert_eq!(a, b, "other blocks keep their spacing");
}

#[test]
fn defaults_keep_the_outlined_table() {
    let k = Kit::neutral(true);
    let rt = run(SRC, plain, 600.0);
    assert!(!has(&rt, "tb") && !has(&rt, "tbc"));
    assert!(has(&rt, "s0") && has(&rt, "s2") && !has(&rt, "s-end"));
    let frame = parent(&rt, "th");
    assert_eq!((frame.radius, frame.fill), (6.0, rune::Color::TRANSPARENT));
    assert_eq!(by_key(&rt, "th").fill, k.c.layer_01);
    assert_eq!(by_key(&rt, "th").rect[0] - frame.rect[0], 0.5);
    assert!(frame.rect[2] < 600.0, "as wide as its content");
}

#[test]
fn a_frame_without_a_card_fills_the_outline_and_pads_it() {
    fn framed(k: &Kit) -> MarkdownStyle {
        MarkdownStyle::from_kit(k).table_frame(k.c.layer_02, k.c.border_base, 3.0)
    }
    let k = Kit::neutral(true);
    let rt = run(SRC, framed, 600.0);
    let frame = parent(&rt, "th");
    assert_eq!((frame.radius, frame.fill), (6.0, k.c.layer_02));
    assert_eq!(by_key(&rt, "th").rect[0] - frame.rect[0], 3.0);
    assert!(has(&rt, "s0") && !has(&rt, "tb"), "the rows keep their rules");
}

#[test]
fn a_header_alone_draws_no_card() {
    let rt = run("| a | b |\n|---|---|\n", card, 600.0);
    assert!(has(&rt, "th") && !has(&rt, "tb"));
    let (frame, th) = (parent(&rt, "th"), by_key(&rt, "th"));
    assert_eq!(frame.rect[3], th.rect[3] + 4.0);
}

#[test]
fn a_streaming_last_row_gets_a_rule_above_and_none_below() {
    let src = "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | a row still arri";
    let rt = run_with(Doc { src: src.into(), style: card, width: 600.0, streaming: true, scroll_wide: false });
    assert!(has(&rt, "r1") && has(&rt, "s1") && !has(&rt, "s2") && !has(&rt, "s0"));
    let (r1, tb) = (by_key(&rt, "r1").rect, by_key(&rt, "tb").rect);
    assert_eq!(r1[1] + r1[3], tb[1] + tb[3]);
}

#[test]
fn narrow_columns_stop_at_the_card() {
    let src = "| Heading one | Heading two | Heading three |\n|---|---|---|\n| supercalifragilistic | expialidocious | antidisestablishmentarianism |\n";
    let rt = run(src, card, 220.0);
    let (frame, tb) = (parent(&rt, "th"), by_key(&rt, "tb"));
    assert_eq!(frame.rect[2], 220.0);
    assert_eq!(tb.rect[2], 216.0);
    let last = cell(&rt, "r0", 2).rect;
    assert!(last[0] + last[2] > tb.rect[0] + tb.rect[2], "the columns at their minimum overflow, clipped by the card: {last:?}");
    let head = cell(&rt, "th", 2).rect;
    assert_eq!((head[0], head[2]), (last[0], last[2]), "the header's columns match the body's");
}

fn copy(rt: &mut Runtime<Doc>, clipboard: &rune::clipboard::MemoryClipboard, f: impl FnOnce(&mut rune::Tx)) -> String {
    rt.update(|_, tx| f(tx));
    rt.frame(1.0 / 60.0);
    rt.key(KeyEvent { key: Key::Command('c'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(1.0 / 60.0);
    clipboard.text().map(|t| t.to_string()).unwrap_or_default()
}

#[test]
fn copying_a_card_table_takes_its_source_and_a_cell_as_drawn() {
    let mut rt = run(SRC, card, 600.0);
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    let table = &SRC[9..SRC.len() - 9];
    assert_eq!(copy(&mut rt, &clipboard, |tx| tx.select_all_text("note/b1")), table.trim_end());
    let lettuce = text(&rt, "Lettuce ×2").path;
    assert_eq!(copy(&mut rt, &clipboard, |tx| tx.select_text(lettuce, 0..7)), "Lettuce");
}

#[test]
fn a_streaming_last_row_does_not_move_the_columns_of_a_scrolling_full_width_table() {
    let src = |last: &str| format!("| Term | Meaning |\n|---|---|\n| cache | a short line |\n| tail | {last}");
    let edges = |last: &str| {
        let rt = run_with(Doc { src: src(last), style: card, width: 600.0, streaming: true, scroll_wide: true });
        let frame = parent(&rt, "th").rect;
        (0..2).map(|j| cell(&rt, "th", j).rect).chain([[frame[0], frame[1], frame[2], 0.0]]).collect::<Vec<_>>()
    };
    let short = edges("a row still");
    assert_eq!(short, edges(&format!("a row still arriving {}", "x".repeat(400))));
    assert_eq!(short[2][2], 600.0, "the frame still fills the viewport");
}

#[test]
fn the_later_of_outline_and_frame_wins() {
    let k = Kit::neutral(true);
    let inset = |rt: &Runtime<Doc>| by_key(rt, "th").rect[0] - parent(rt, "th").rect[0];
    fn outlined(k: &Kit) -> MarkdownStyle {
        MarkdownStyle::from_kit(k).table_card(k).table_outline(Some(k.c.border_base))
    }
    fn tabled(k: &Kit) -> MarkdownStyle {
        MarkdownStyle::from_kit(k).table_card(k).table(k.c.border_base, k.c.layer_02)
    }
    fn liked(k: &Kit) -> MarkdownStyle {
        MarkdownStyle::from_kit(k).table_card(k).table_like(&rune_kit::TableStyle::from_kit(k))
    }
    assert_eq!(inset(&run(SRC, card, 600.0)), 2.0);
    for style in [outlined as fn(&Kit) -> MarkdownStyle, tabled] {
        let rt = run(SRC, style, 600.0);
        assert_eq!(inset(&rt), 0.5, "the outline replaces the frame");
        assert_ne!(parent(&rt, "th").fill, k.c.layer_01);
    }
    let rt = run(SRC, liked, 600.0);
    assert!(!has(&rt, "tb"), "table_like drops the card");
    assert_eq!(inset(&rt), 0.5);
    let (th, c0) = (by_key(&rt, "th"), cell(&rt, "th", 0));
    assert_eq!(th.rect[3], c0.rect[3], "and the header's own padding");
    assert_eq!(by_key(&rt, "r0").rect[3], cell(&rt, "r0", 0).rect[3]);
}

#[test]
fn setters_take_non_finite_points_as_zero() {
    fn wild(k: &Kit) -> MarkdownStyle {
        MarkdownStyle::from_kit(k)
            .table_card(k)
            .table_frame(k.c.layer_01, k.c.border_muted, f32::INFINITY)
            .table_body(k.c.bg_base, f32::NAN, &[])
            .table_header_pad(f32::INFINITY, f32::NAN)
            .table_cell_padding(f32::INFINITY, f32::NEG_INFINITY)
            .table_min_column(f32::INFINITY)
            .table_margin(f32::INFINITY, f32::NAN)
    }
    let rt = run(SRC, wild, 600.0);
    for key in ["th", "tb", "r0"] {
        assert!(by_key(&rt, key).rect.iter().all(|v| v.is_finite()), "{key}");
    }
}

#[test]
fn a_header_alone_is_sized_by_its_own_padding() {
    fn padded(body: f32) -> impl Fn(&Kit) -> MarkdownStyle {
        move |k| MarkdownStyle::from_kit(k).table_header_pad(6.0, 6.0).table_cell_padding(body, 6.0)
    }
    fn wide(k: &Kit) -> MarkdownStyle {
        padded(30.0)(k)
    }
    fn same(k: &Kit) -> MarkdownStyle {
        padded(6.0)(k)
    }
    let src = "| a | b |\n|---|---|\n";
    let width = |style| cell(&run(src, style, 600.0), "th", 0).rect[2];
    assert_eq!(width(wide), width(same));
}

#[test]
fn an_outlined_table_header_rounds_inside_the_outline() {
    const HEAD: rune::Color = rune::Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
    fn styled(src: &String, _: &mut Cx<String>) -> El {
        col().w_full().child(Markdown::new(src.clone()).render(&MarkdownStyle::neutral(true).table_header_fill(HEAD)))
    }
    for (src, bottom) in [("| A | B |\n|---|---|\n| a | b |\n", 0.0), ("| A | B |\n|---|---|\n", 5.5)] {
        let mut rt = Runtime::new(src.to_owned(), styled, (480.0, 400.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let fill = rt.instances.iter().find(|i| i.fill == [HEAD.r, HEAD.g, HEAD.b, HEAD.a]).expect("the header's fill");
        assert_eq!(rune::render::unpack_clip_radii(fill.fade[3]), [5.5, 5.5, bottom, bottom], "{src:?}");
    }
}
