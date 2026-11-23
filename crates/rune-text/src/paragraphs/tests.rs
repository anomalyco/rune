use super::*;
use parley::layout::Affinity;
use rune_core::color::Color;

fn input<'a>(content: &'a str, blocks: &'a [Block], style: TextStyle, dir: TextDir) -> TextInput<'a> {
    TextInput {
        content,
        spans: &[],
        blocks,
        pills: &[],
        style,
        shape_key: None,
        wrap_anywhere: false,
        pretty: false,
        redacted: false,
        by_line: false,
        by_paragraph: true,
        dir,
    }
}

#[test]
fn caret_on_empty_indented_block_matches_filled_block_start() {
    let mut ts = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0);
    let scale = 2.0;
    let max_w = 400.0;
    let caret_w = 1.5 * scale;

    for blk in [
        Block::Paragraph,
        Block::Bullet(0),
        Block::Bullet(2),
        Block::Numbered(0),
        Block::Numbered(1),
        Block::Quote,
        Block::Code,
    ] {
        let blocks = [blk, blk];
        let empty = ts.layout_text(&input("one
", &blocks, style, TextDir::Fixed(Flow::Ltr)), true, scale, Some(max_w), Alignment::Start, None);
        let filled = ts.layout_text(&input("one
x", &blocks, style, TextDir::Fixed(Flow::Ltr)), true, scale, Some(max_w), Alignment::Start, None);
        let empty_x = empty.view().caret(4, Affinity::Downstream, caret_w).x0 as f32;
        let filled_x = filled.view().caret(4, Affinity::Downstream, caret_w).x0 as f32;
        let indent = (blk.indent_pt(style.size) * scale).round();
        assert_eq!(empty_x, filled_x, "LTR {blk:?}: empty caret x should match filled start");
        assert_eq!(empty_x, indent, "LTR {blk:?}: empty caret x should equal block indent");
        if blk == Block::Paragraph {
            assert_eq!(empty_x, 0.0);
        } else {
            assert!(empty_x > 0.0);
        }

        // RTL: right-edge caret respects code block right padding.
        let rtl_empty = ts.layout_text(&input("א
", &blocks, style, TextDir::Fixed(Flow::Rtl)), true, scale, Some(max_w), Alignment::Start, None);
        let at = "א
".len();
        let rtl_x = rtl_empty.view().caret(at, Affinity::Downstream, caret_w).x0 as f32;
        let right_pad = if blk == Block::Code { indent } else { 0.0 };
        assert_eq!(rtl_x, (max_w - right_pad - caret_w).max(indent), "RTL {blk:?}");
    }

    // Each paragraph uses its own block indent rather than the previous paragraph's.
    let to_list = ts.layout_text(&input("one
", &[Block::Paragraph, Block::Numbered(0)], style, TextDir::Fixed(Flow::Ltr)), true, scale, Some(max_w), Alignment::Start, None);
    let to_para = ts.layout_text(&input("one
", &[Block::Numbered(0), Block::Paragraph], style, TextDir::Fixed(Flow::Ltr)), true, scale, Some(max_w), Alignment::Start, None);
    assert_eq!(to_list.view().caret(4, Affinity::Downstream, caret_w).x0 as f32, (Block::Numbered(0).indent_pt(style.size) * scale).round());
    assert_eq!(to_para.view().caret(4, Affinity::Downstream, caret_w).x0 as f32, 0.0);
}

#[test]
fn empty_indented_block_move_line_selection_and_highlight_use_indent() {
    let mut ts = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0);
    let scale = 2.0;
    let max_w = 400.0;

    // Moving up from an empty nested list item keeps its indented x as the column goal.
    let nested = Block::Bullet(2);
    let indent = (nested.indent_pt(style.size) * scale).round();
    let text = "hello world across the top line
";
    let laid = ts.layout_text(&input(text, &[Block::Paragraph, nested], style, TextDir::Fixed(Flow::Ltr)), true, scale, Some(max_w), Alignment::Start, None);
    let (up_idx, goal_x) = laid.view().move_line(text.len(), false, None).expect("move up");
    assert_eq!(goal_x, indent);
    assert!(up_idx > 0, "moving up from indented empty item should land inside the paragraph above, not at byte 0");

    // Selecting across a blank list item starts its newline box after the indent.
    let num = Block::Numbered(0);
    let num_indent = (num.indent_pt(style.size) * scale).round();
    let multi = "a

b";
    let list_laid = ts.layout_text(&input(multi, &[num, num, num], style, TextDir::Fixed(Flow::Ltr)), true, scale, Some(max_w), Alignment::Start, None);
    let boxes = list_laid.view().range_boxes(0, multi.len(), false);
    assert_eq!(boxes.len(), 3, "one box per paragraph");
    assert_eq!(boxes[1].0.x0 as f32, num_indent, "empty list item selection starts after indent");

    // Highlight decorations across a blank list item also start after the indent.
    let mut decos = Vec::new();
    let hl = TextHighlight {
        start: 0,
        end: multi.len() as u32,
        bg: Color::WHITE,
        fg: None,
        ring: None,
        pad: 0.0,
        radius: 0.0,
        vpad: 0.0,
        key: None,
    };
    ts.highlight_decorations_flow(list_laid.view(), &[hl], multi.len(), scale, [0.0, 0.0], f32::NEG_INFINITY..f32::INFINITY, &mut decos);
    assert_eq!(decos.len(), 3, "one highlight box per paragraph");
    assert_eq!(decos[1].rect[0], num_indent, "empty list item highlight starts after indent");
}
