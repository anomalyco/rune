use super::super::cache::stream_cached;
use super::super::parser::parse_doc;
use super::super::*;

const JUICE: &str = "**More useful**\n1. **Run it live.** Real numbers.\n2. **Hands-on.** Press `e`.\n\n**More juice**\n4. **Hash ripple.** A changed hash scrambles.\n5. **Panels that carry over.** The panel stays put.\n8. **Motion with weight.** Panels spring in.\n\n**My pick:** 1, 2 and 4.";

fn texts(blocks: &[Block]) -> Vec<String> {
    blocks
        .iter()
        .map(|b| match b {
            Block::Para(i) => i.text.clone(),
            Block::List { items, .. } => format!("{}", items.len()),
            _ => String::new(),
        })
        .collect()
}

fn item_text(b: &Block, n: usize) -> &str {
    let Block::List { items, .. } = b else { panic!("not a list: {b:?}") };
    let Block::Para(i) = &items[n][0] else { panic!("item {n} isn't text") };
    &i.text
}

#[test]
fn numbered_lines_after_a_heading_line_are_a_list() {
    let blocks = parse(JUICE);
    assert_eq!(texts(&blocks), ["More useful", "2", "More juice", "3", "My pick: 1, 2 and 4."]);
    let Block::List { start, .. } = &blocks[3] else { unreachable!() };
    assert_eq!(*start, Some(4));
    assert_eq!(item_text(&blocks[3], 0), "Hash ripple. A changed hash scrambles.");
    assert_eq!(item_text(&blocks[3], 2), "Motion with weight. Panels spring in.");
    let Block::List { items, .. } = &blocks[3] else { unreachable!() };
    let Block::Para(first) = &items[0][0] else { unreachable!() };
    assert_eq!(&first.text[first.spans[0].0..first.spans[0].1], "Hash ripple.");
}

#[test]
fn item_text_maps_to_its_source() {
    let (_, _, _, _, _, maps) = parse_doc(JUICE, true);
    let BlockMap::List(items) = &maps[3] else { panic!("not a list map") };
    let BlockMap::Inline(map) = &items[0][0] else { panic!("not inline") };
    let (src, out) = map.0.first().unwrap();
    assert_eq!(out.start, 0);
    assert_eq!(&JUICE[src.clone()], "Hash ripple.");
}

#[test]
fn a_streamed_reply_reads_like_the_finished_one() {
    let mut last = None;
    for end in (1..=JUICE.len()).filter(|&i| JUICE.is_char_boundary(i)) {
        last = Some(stream_cached(&JUICE[..end]).0);
    }
    assert_eq!(*last.unwrap(), parse(JUICE));
}

#[test]
fn numbers_that_aren_t_list_markers_stay_text() {
    for src in ["Escaped\n4\\. not a list", "**Bold across\n4. lines**", "Bare\n4.", "Decimal\n4.5 points"] {
        let blocks = parse(src);
        assert!(matches!(blocks.as_slice(), [Block::Para(_)]), "{src:?} became {blocks:?}");
    }
}

#[test]
fn a_quoted_list_continues_too() {
    let blocks = parse("> Next\n> 3. three\n> 4. four");
    let [Block::Quote(inner)] = blocks.as_slice() else { panic!("{blocks:?}") };
    assert_eq!(texts(inner), ["Next", "2"]);
}
