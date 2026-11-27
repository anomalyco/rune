use super::*;
use parley::layout::Affinity;
use rune_core::color::Color;

fn input<'a>(content: &'a str, blocks: &'a [Block], style: TextStyle, dir: TextDir) -> TextInput<'a> {
    TextInput {
        content,
        spans: &[],
        blocks,
        pills: &[],
        span_boxes: &[],
        inline_boxes: &[],
        text_indent: 0.0,
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

const INTER: &[u8] = include_bytes!("../../../rune/assets/Inter-Regular.ttf");
const PLEX: &[u8] = include_bytes!("../../../rune/assets/IBMPlexMono-Regular.ttf");

/// A self-contained `TextSystem` with bundled sans and fixed-pitch monospace faces, so
/// `by_line` deferral tests do not depend on host system monospace fonts.
fn mono_text_system() -> TextSystem {
    TextSystem::new_local(&[INTER, PLEX], ("Inter", "IBM Plex Mono"))
}

fn rich_input<'a>(
    content: &'a str,
    spans: &'a [Span],
    style: TextStyle,
    by_line: bool,
    dir: TextDir,
) -> TextInput<'a> {
    TextInput {
        content,
        spans,
        blocks: &[],
        pills: &[],
        span_boxes: &[],
        inline_boxes: &[],
        text_indent: 0.0,
        style,
        shape_key: None,
        wrap_anywhere: false,
        pretty: false,
        redacted: false,
        by_line,
        by_paragraph: true,
        dir,
    }
}

fn as_paragraphs(tl: &TextLayout) -> &Rc<ParagraphLayout> {
    match tl {
        TextLayout::Paragraphs(p) => p,
        TextLayout::Single(_) => panic!("expected ParagraphLayout"),
    }
}

fn assert_waiting_runs_span_free(pl: &ParagraphLayout, spans: &[Span]) {
    let layout_spans: Vec<&Span> = span::layout_spans(spans).collect();
    for p in &pl.paragraphs {
        if !p.shaped {
            assert!(
                !layout_spans.iter().any(|s| (s.start as usize) < p.end && p.start < s.end as usize && {
                    // Any overlap with a non-newline byte inside the waiting run is forbidden.
                    let lo = (s.start as usize).max(p.start);
                    let hi = (s.end as usize).min(p.end);
                    pl.lazy.as_ref().unwrap().content.as_bytes()[lo..hi].iter().any(|&b| b != b'\n' && b != b'\r')
                }),
                "waiting paragraph {}..{} overlaps a layout span",
                p.start,
                p.end,
            );
        }
    }
}

#[test]
fn by_line_defers_plain_ascii_paragraphs_when_head_has_underline() {
    let head = "A".repeat(1500);
    let tail: String = (0..1000).map(|i| format!("line {i}\n")).collect();
    let content = format!("{head}\n{tail}");
    let spans = [Span::new(0, head.len()).underline(Color::BLACK)];
    let style = TextStyle::new(13.0, 400.0).mono();
    let scale = 2.0;
    let max_w = Some(320.0 * scale);

    let mut plain_ts = mono_text_system();
    let plain_tl = plain_ts.layout_text(&rich_input(&content, &[], style, true, TextDir::Auto), false, scale, max_w, Alignment::Start, None);
    let plain_pl = as_paragraphs(&plain_tl);
    assert!(plain_pl.waiting(), "unspanned baseline should defer plain interior lines");
    let baseline_shaped = plain_ts.shaped;

    let mut spanned_ts = mono_text_system();
    let spanned_tl = spanned_ts.layout_text(&rich_input(&content, &spans, style, true, TextDir::Auto), false, scale, max_w, Alignment::Start, None);
    let spanned_pl = as_paragraphs(&spanned_tl);
    assert!(spanned_pl.waiting(), "decorated head line must not disable deferral for plain lines below it");
    assert_eq!(spanned_ts.shaped, baseline_shaped, "initial shaped count matches unspanned baseline");
    assert_waiting_runs_span_free(spanned_pl, &spans);
    assert!(spanned_pl.paragraphs[0].shaped, "decorated head paragraph shapes eagerly");
    assert_eq!(spanned_pl.height, plain_pl.height, "total height matches unspanned layout");

    // Filling every waiting paragraph moves no line heights and matches the baseline line geometry.
    let (filled, moved) = spanned_ts
        .fill_paragraphs(spanned_pl, std::time::Duration::MAX, (0.0, f32::INFINITY), f32::INFINITY)
        .expect("waiting paragraphs fill");
    assert!(!moved, "filling span-free waiting runs must not shift heights");
    assert!(!filled.waiting());
    assert_eq!(filled.height, plain_pl.height);
    assert_eq!(filled.paragraphs.len(), 1002);

    // Head underline decorations match laying out the underlined head paragraph alone.
    let mut head_decos = Vec::new();
    spanned_ts.decorations(&filled.paragraphs[0].layout, &spans, scale, [0.0, 0.0], &mut head_decos);
    let mut solo_ts = mono_text_system();
    let solo_layout = solo_ts.layout_rich(&head, &spans, style, scale, max_w);
    let mut solo_decos = Vec::new();
    solo_ts.decorations(&solo_layout, &spans, scale, [0.0, 0.0], &mut solo_decos);
    assert!(!head_decos.is_empty(), "underlined head emits underline decorations");
    assert_eq!(head_decos.len(), solo_decos.len());
    for (a, b) in head_decos.iter().zip(&solo_decos) {
        assert_eq!(a.rect, b.rect);
    }
}

#[test]
fn interior_disjoint_overlapping_multiline_and_input_spans_keep_waiting_runs_span_free() {
    let lines: Vec<String> = (0..400).map(|i| format!("line {i:03} plain ascii")).collect();
    let content = lines.join("\n");
    let mut starts = Vec::with_capacity(lines.len());
    let mut at = 0;
    for l in &lines {
        starts.push((at, at + l.len()));
        at += l.len() + 1;
    }
    let style = TextStyle::new(13.0, 400.0).mono();
    let scale = 2.0;
    let max_w = Some(480.0 * scale);

    // 1. Disjoint spans in the interior: underline on 120, tall 22pt bold on 200, multi-line
    //    span crossing 280..=282, empty span on 150, colour-only span on 160, newline-only
    //    span between 170 and 171.
    let disjoint = vec![
        Span::new(starts[120].0, starts[120].1).underline(Color::BLACK),
        Span::new(starts[150].0, starts[150].0).underline(Color::BLACK),
        Span::new(starts[160].0, starts[160].1).color(Color::WHITE),
        Span::new(starts[170].1, starts[170].1 + 1).underline(Color::BLACK),
        Span::new(starts[200].0, starts[200].1).bold().size(22.0),
        Span::new(starts[280].0 + 2, starts[282].1 - 2).italic(),
    ];
    let mut ts = mono_text_system();
    let tl = ts.layout_text(&rich_input(&content, &disjoint, style, true, TextDir::Auto), false, scale, max_w, Alignment::Start, None);
    let pl = as_paragraphs(&tl);
    assert!(pl.waiting(), "interior disjoint spans keep plain runs waiting");
    assert_waiting_runs_span_free(pl, &disjoint);
    for idx in [120usize, 200, 280, 281, 282] {
        let (s, e) = starts[idx];
        let p = pl.paragraphs.iter().find(|p| p.start == s && p.end == e).unwrap_or_else(|| panic!("line {idx} should be its own eager paragraph"));
        assert!(p.shaped, "spanned line {idx} must shape eagerly");
    }
    // Colour-only line 160 stays inside a waiting run rather than forcing eager shaping.
    let (s160, e160) = starts[160];
    assert!(pl.paragraphs.iter().any(|p| !p.shaped && p.start <= s160 && e160 <= p.end), "colour-only interior line remains deferred");
    let expected_height = pl.height;
    let (filled, moved) = ts
        .fill_paragraphs(pl, std::time::Duration::MAX, (0.0, f32::INFINITY), f32::INFINITY)
        .expect("fill");
    assert!(!moved, "filling around a 22pt spanned line must not shift heights");
    assert_eq!(filled.height, expected_height);
    assert!(filled.paragraphs[200].height > filled.paragraphs[199].height, "22pt line keeps its taller height");

    // 2. Overlapping and unsorted layout spans (layout_disjoint == false).
    let overlapping = vec![
        Span::new(starts[250].0, starts[250].1).bold(),
        Span::new(starts[140].0, starts[140].1).underline(Color::BLACK),
        Span::new(starts[140].0 + 2, starts[140].1 - 2).italic(),
    ];
    let mut ts_ov = mono_text_system();
    let tl_ov = ts_ov.layout_text(&rich_input(&content, &overlapping, style, true, TextDir::Auto), false, scale, max_w, Alignment::Start, None);
    let pl_ov = as_paragraphs(&tl_ov);
    assert!(pl_ov.waiting(), "unsorted/overlapping layout spans still defer plain runs");
    assert_waiting_runs_span_free(pl_ov, &overlapping);
    for idx in [140usize, 250] {
        let (s, e) = starts[idx];
        let p = pl_ov.paragraphs.iter().find(|p| p.start == s && p.end == e).expect("eager paragraph");
        assert!(p.shaped);
    }
    let (_, moved_ov) = ts_ov
        .fill_paragraphs(pl_ov, std::time::Duration::MAX, (0.0, f32::INFINITY), f32::INFINITY)
        .expect("fill overlapping");
    assert!(!moved_ov);

    // 3. Multi-line input (by_line: false) with a layout span also defers plain ASCII runs.
    let mut ts_in = mono_text_system();
    let tl_in = ts_in.layout_text(&rich_input(&content, &disjoint, style, false, TextDir::Plaintext(Some(Flow::Ltr))), true, scale, max_w, Alignment::Start, None);
    let pl_in = as_paragraphs(&tl_in);
    assert!(pl_in.waiting(), "by_paragraph input with layout spans still defers plain runs");
    assert_waiting_runs_span_free(pl_in, &disjoint);
    let (_, moved_in) = ts_in
        .fill_paragraphs(pl_in, std::time::Duration::MAX, (0.0, f32::INFINITY), f32::INFINITY)
        .expect("fill input");
    assert!(!moved_in);

    // 4. Rebreaking at a narrower width keeps span-free invariant and exact height.
    let narrower = Some(200.0 * scale);
    let rebroken_tl = ts.layout_text(&rich_input(&content, &disjoint, style, true, TextDir::Auto), false, scale, narrower, Alignment::Start, Some(&tl));
    let rebroken_pl = as_paragraphs(&rebroken_tl);
    assert!(rebroken_pl.waiting());
    assert_waiting_runs_span_free(rebroken_pl, &disjoint);
    let (_, moved_rebreak) = ts
        .fill_paragraphs(rebroken_pl, std::time::Duration::MAX, (0.0, f32::INFINITY), f32::INFINITY)
        .expect("fill rebroken");
    assert!(!moved_rebreak);
}

#[test]
fn rtl_blocks_and_pills_preserve_existing_deferral_limits() {
    let content: String = (0..200).map(|i| format!("line {i:03} plain\n")).collect();
    let spans = [Span::new(0, 8).underline(Color::BLACK)];
    let style = TextStyle::new(13.0, 400.0).mono();
    let scale = 2.0;
    let max_w = Some(400.0 * scale);

    // Fixed RTL by_line stays eager (not deferred).
    let mut ts = mono_text_system();
    let rtl = ts.layout_text(&rich_input(&content, &spans, style, true, TextDir::Fixed(Flow::Rtl)), false, scale, max_w, Alignment::Start, None);
    assert!(!as_paragraphs(&rtl).waiting(), "RTL by_line must not defer");

    // Non-Paragraph blocks disable deferral.
    let blocks = [Block::Quote];
    let mut with_block = rich_input(&content, &spans, style, true, TextDir::Auto);
    with_block.blocks = &blocks;
    let blk_tl = ts.layout_text(&with_block, false, scale, max_w, Alignment::Start, None);
    assert!(!as_paragraphs(&blk_tl).waiting(), "non-Paragraph blocks must not defer");

    // Pills disable deferral.
    let pills = [PillChip { start: 0, end: 4, icon: None, image: None, bg: None, fg: None, ring: None, radius: None, pad_x: None, margin_x: None, bare: None }];
    let mut with_pill = rich_input(&content, &spans, style, true, TextDir::Auto);
    with_pill.pills = &pills;
    let pill_tl = ts.layout_text(&with_pill, false, scale, max_w, Alignment::Start, None);
    assert!(!as_paragraphs(&pill_tl).waiting(), "pills must not defer");

    // An RTL line inside an LTR document shapes eagerly while surrounding plain lines defer.
    let mut mixed_lines: Vec<String> = (0..200).map(|i| format!("line {i:03} plain")).collect();
    mixed_lines[100] = "שלום עולם".to_string();
    let mixed = mixed_lines.join("\n");
    let mixed_tl = ts.layout_text(&rich_input(&mixed, &spans, style, true, TextDir::Plaintext(Some(Flow::Ltr))), false, scale, max_w, Alignment::Start, None);
    let mixed_pl = as_paragraphs(&mixed_tl);
    assert!(mixed_pl.waiting(), "plain ASCII lines around an RTL line still defer");
    assert_waiting_runs_span_free(mixed_pl, &spans);
    let (_, moved) = ts
        .fill_paragraphs(mixed_pl, std::time::Duration::MAX, (0.0, f32::INFINITY), f32::INFINITY)
        .expect("fill mixed");
    assert!(!moved);

    // When the mono slot resolves to a proportional face (e.g. a host with no monospace font
    // installed and none bundled), `by_line` intentionally disables deferral and shapes eagerly.
    let mut prop_ts = TextSystem::new_local(&[INTER], ("Inter", "Inter"));
    let prop_tl = prop_ts.layout_text(&rich_input(&content, &[], style, true, TextDir::Auto), false, scale, max_w, Alignment::Start, None);
    assert!(!as_paragraphs(&prop_tl).waiting(), "proportional fallback in mono slot must not defer by_line");
}

#[test]
fn layout_text_and_fill_paragraphs_ignore_prev_from_older_shaped_epoch() {
    let _guard = crate::with_families("Inter", "IBM Plex Mono");
    let style = TextStyle::new(14.0, 400.0);
    let scale = 2.0;
    let max_w = Some(600.0 * scale);
    let sample = "iiiiiiiiii 0123456789\niiiiiiiiii 0123456789";

    let mut ts = TextSystem::new_local(&[INTER, PLEX], ("Inter", "IBM Plex Mono"));
    let old_tl = ts.layout_text(&rich_input(sample, &[], style, true, TextDir::Auto), false, scale, max_w, Alignment::Start, None);
    let old_w = old_tl.view().full_width();

    // Switching families drops shaped caches and advances shaped_epoch: passing the previous
    // ParagraphLayout must re-shape in the new font instead of reusing old-font paragraphs.
    assert!(ts.set_families("IBM Plex Mono", "Inter"));
    let switched_tl = ts.layout_text(&rich_input(sample, &[], style, true, TextDir::Auto), false, scale, max_w, Alignment::Start, Some(&old_tl));
    let mut fresh_ts = TextSystem::new_local(&[INTER, PLEX], ("IBM Plex Mono", "Inter"));
    let fresh_tl = fresh_ts.layout_text(&rich_input(sample, &[], style, true, TextDir::Auto), false, scale, max_w, Alignment::Start, None);
    assert!(
        (switched_tl.view().full_width() - fresh_tl.view().full_width()).abs() < 0.01,
        "switched layout must match fresh launch: {} vs {}",
        switched_tl.view().full_width(),
        fresh_tl.view().full_width(),
    );
    assert!(
        (switched_tl.view().full_width() - old_w).abs() > 10.0,
        "switched width must differ from old font: {old_w} -> {}",
        switched_tl.view().full_width(),
    );

    // A deferred ParagraphLayout from an older shaped_epoch must not be filled with new-font lines.
    let long: String = (0..200).map(|i| format!("line {i:03} plain\n")).collect();
    let mono_style = TextStyle::new(13.0, 400.0).mono();
    let mut lazy_ts = mono_text_system();
    let lazy_tl = lazy_ts.layout_text(&rich_input(&long, &[], mono_style, true, TextDir::Auto), false, scale, Some(400.0 * scale), Alignment::Start, None);
    let lazy_pl = as_paragraphs(&lazy_tl);
    assert!(lazy_pl.waiting());
    assert!(lazy_ts.set_families("IBM Plex Mono", "Inter"));
    assert!(
        lazy_ts.fill_paragraphs(lazy_pl, std::time::Duration::MAX, (0.0, f32::INFINITY), f32::INFINITY).is_none(),
        "fill_paragraphs must ignore a ParagraphLayout from an older shaped_epoch",
    );
}

fn build_markdown_like_doc(lines: &[String]) -> (String, Vec<Span>) {
    let mut content = String::new();
    let mut spans = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let start = content.len();
        content.push_str(line);
        let end = content.len();
        if line.starts_with("# ") {
            spans.push(Span::new(start, end).size(20.0).bold());
            // Overlapping inline italic span inside heading (tests layout_disjoint == false).
            if end - start > 6 {
                spans.push(Span::new(start + 2, start + 6).italic());
            }
        } else if line.starts_with("**") && end - start > 8 {
            spans.push(Span::new(start, start + 4).bold());
            spans.push(Span::new(start + 5, end).italic());
        } else if line.starts_with("__") && end > start {
            spans.push(Span::new(start, end).underline(Color::BLACK));
        }
        if i + 1 < lines.len() {
            content.push('\n');
        }
    }
    (content, spans)
}

#[test]
fn spanned_multiline_edits_reuse_unchanged_paragraphs() {
    let style = TextStyle::new(14.0, 400.0);
    let scale = 2.0;
    let max_w = Some(600.0 * scale);
    let mut lines: Vec<String> = (0..220)
        .map(|i| {
            if i % 25 == 0 {
                format!("# Heading {i:03} section title")
            } else if i % 7 == 0 {
                format!("**Bold** and italic line {i:03} in a markdown document")
            } else if i % 11 == 0 {
                format!("__Underlined__ line {i:03} in a markdown document")
            } else {
                format!("Paragraph {i:03} with plain ASCII text in a markdown document")
            }
        })
        .collect();

    let (content, spans) = build_markdown_like_doc(&lines);
    let mut ts = mono_text_system();

    // Initial load still defers plain ASCII interior paragraphs (#1900).
    let tl0 = ts.layout_text(&rich_input(&content, &spans, style, false, TextDir::Plaintext(Some(Flow::Ltr))), true, scale, max_w, Alignment::Start, None);
    let pl0 = as_paragraphs(&tl0);
    assert!(pl0.waiting(), "initial spanned load should keep plain interior lines deferred");
    assert_waiting_runs_span_free(pl0, &spans);

    // Editing at the end while waiting runs still exist keeps those waiting runs and shapes only the edited line.
    let mut lines_while_waiting = lines.clone();
    lines_while_waiting[219].push('!');
    let (cw, sw) = build_markdown_like_doc(&lines_while_waiting);
    let shaped_before = ts.shaped;
    let tlw = ts.layout_text(&rich_input(&cw, &sw, style, false, TextDir::Plaintext(Some(Flow::Ltr))), true, scale, max_w, Alignment::Start, Some(&tl0));
    let plw = as_paragraphs(&tlw);
    assert!(plw.waiting(), "edit at end while waiting runs exist must keep interior runs waiting");
    assert!(ts.shaped - shaped_before <= 2, "edit while waiting should shape only edited line, got {}", ts.shaped - shaped_before);

    // Fill all waiting paragraphs, matching a settled editor after initial load.
    let (filled, moved) = ts
        .fill_paragraphs(pl0, std::time::Duration::MAX, (0.0, f32::INFINITY), f32::INFINITY)
        .expect("fill all");
    assert!(!moved);
    assert!(!filled.waiting());
    let mut cur_tl = TextLayout::Paragraphs(filled);

    // Edit at start, middle (plain + spanned), end, same-length replacement, newline insert/delete.
    let edits: Vec<(&str, Box<dyn Fn(&mut Vec<String>)>)> = vec![
        ("start", Box::new(|l| l[0].push('x'))),
        ("middle_plain", Box::new(|l| l[111].push('y'))),
        ("middle_spanned", Box::new(|l| l[100].push('z'))),
        ("end", Box::new(|l| l[219].push('w'))),
        ("same_len_replace", Box::new(|l| {
            let last = if l[111].ends_with('A') { 'B' } else { 'A' };
            l[111].pop();
            l[111].push(last);
        })),
        ("insert_newline", Box::new(|l| l.insert(112, "inserted plain line".to_string()))),
        ("delete_newline", Box::new(|l| { l.remove(112); })),
    ];

    for (label, apply) in edits {
        apply(&mut lines);
        let (c, s) = build_markdown_like_doc(&lines);
        let shaped_before = ts.shaped;
        let next_tl = ts.layout_text(&rich_input(&c, &s, style, false, TextDir::Plaintext(Some(Flow::Ltr))), true, scale, max_w, Alignment::Start, Some(&cur_tl));
        let next_pl = as_paragraphs(&next_tl);
        let shaped_delta = ts.shaped - shaped_before;
        assert!(
            shaped_delta <= 2,
            "{label}: expected at most 2 reshaped paragraphs, got {shaped_delta}",
        );
        assert!(
            !next_pl.waiting(),
            "{label}: editing an already-shaped spanned document must not re-defer paragraphs",
        );
        assert_eq!(next_pl.paragraphs.len(), lines.len(), "{label}: paragraph count");

        // Fresh layout from scratch must agree on total height and paragraph boundaries.
        let mut fresh_ts = mono_text_system();
        let fresh_tl = fresh_ts.layout_text(&rich_input(&c, &s, style, false, TextDir::Plaintext(Some(Flow::Ltr))), true, scale, max_w, Alignment::Start, None);
        let fresh_pl = as_paragraphs(&fresh_tl);
        assert!(
            (next_pl.height - fresh_pl.height).abs() < 0.01,
            "{label}: height mismatch: incremental {} vs fresh {}",
            next_pl.height,
            fresh_pl.height,
        );
        cur_tl = next_tl;
    }

    // Span-only edit on one line (text unchanged): shapes only that paragraph.
    let (c, mut s) = build_markdown_like_doc(&lines);
    let line55_start = c.find("Paragraph 057").unwrap();
    let line55_end = line55_start + "Paragraph 057".len();
    s.push(Span::new(line55_start, line55_end).size(24.0).bold());
    let prev_pl = as_paragraphs(&cur_tl).clone();
    let shaped_before = ts.shaped;
    let span_tl = ts.layout_text(&rich_input(&c, &s, style, false, TextDir::Plaintext(Some(Flow::Ltr))), true, scale, max_w, Alignment::Start, Some(&cur_tl));
    let span_pl = as_paragraphs(&span_tl);
    assert_eq!(ts.shaped - shaped_before, 1, "adding a span on one line must shape only that paragraph");
    assert!(!span_pl.waiting());
    assert!(span_pl.paragraphs[57].height > prev_pl.paragraphs[57].height, "24pt span must take effect on line 57");
    assert!(Rc::ptr_eq(&span_pl.paragraphs[0].layout, &prev_pl.paragraphs[0].layout), "line 0 layout reused");
    assert!(Rc::ptr_eq(&span_pl.paragraphs[219].layout, &prev_pl.paragraphs[219].layout), "line 219 layout reused");

    // Multi-line span edit: editing outside keeps all other paragraphs; editing inside updates only covered lines.
    let mut multi_spans = s.clone();
    let m_start = c.find("Paragraph 080").unwrap();
    let m_end = c.find("Paragraph 082").unwrap() + "Paragraph 082".len();
    multi_spans.push(Span::new(m_start, m_end).italic());
    let multi_tl = ts.layout_text(&rich_input(&c, &multi_spans, style, false, TextDir::Plaintext(Some(Flow::Ltr))), true, scale, max_w, Alignment::Start, Some(&span_tl));
    let multi_pl = as_paragraphs(&multi_tl);
    assert!(!multi_pl.waiting());
    // Changing only underline color (no layout-attribute change) shapes nothing.
    let mut recolored = multi_spans.clone();
    for sp in &mut recolored {
        if sp.underline.is_some() {
            sp.underline = Some(Color::WHITE);
        }
    }
    let shaped_before = ts.shaped;
    let recolored_tl = ts.layout_text(&rich_input(&c, &recolored, style, false, TextDir::Plaintext(Some(Flow::Ltr))), true, scale, max_w, Alignment::Start, Some(&multi_tl));
    assert_eq!(ts.shaped - shaped_before, 0, "recolouring underline span must not reshape any paragraph");
    assert!(Rc::ptr_eq(&as_paragraphs(&recolored_tl).paragraphs[57].layout, &multi_pl.paragraphs[57].layout));

    // Pasting > LAZY_MIN lines into the middle of a settled spanned document keeps surrounding
    // paragraphs and defers the newly pasted plain interior.
    let mut pasted_lines = lines.clone();
    let paste_block: Vec<String> = (0..120).map(|i| format!("pasted line {i:03} plain ascii")).collect();
    pasted_lines.splice(112..112, paste_block);
    let (pc, ps) = build_markdown_like_doc(&pasted_lines);
    let pasted_tl = ts.layout_text(&rich_input(&pc, &ps, style, false, TextDir::Plaintext(Some(Flow::Ltr))), true, scale, max_w, Alignment::Start, Some(&cur_tl));
    let pasted_pl = as_paragraphs(&pasted_tl);
    assert!(pasted_pl.waiting(), "large paste into spanned doc should defer its plain ASCII interior");
    assert_waiting_runs_span_free(pasted_pl, &ps);
    assert!(Rc::ptr_eq(&pasted_pl.paragraphs[0].layout, &prev_pl.paragraphs[0].layout), "head preserved across large paste");
    assert!(Rc::ptr_eq(&pasted_pl.paragraphs.last().unwrap().layout, &prev_pl.paragraphs.last().unwrap().layout), "tail preserved across large paste");
}

#[test]
fn by_line_spanned_streaming_append_reuses_shaped_and_waiting_paragraphs() {
    let head = "A".repeat(1500);
    let tail: String = (0..300).map(|i| format!("line {i:03} plain ascii\n")).collect();
    let mut content = format!("{head}\n{tail}");
    let spans = [Span::new(0, head.len()).underline(Color::BLACK)];
    let style = TextStyle::new(13.0, 400.0).mono();
    let scale = 2.0;
    let max_w = Some(320.0 * scale);

    let mut ts = mono_text_system();
    let tl0 = ts.layout_text(&rich_input(&content, &spans, style, true, TextDir::Auto), false, scale, max_w, Alignment::Start, None);
    let pl0 = as_paragraphs(&tl0);
    assert!(pl0.waiting());

    // Fill all waiting paragraphs, then append a line (simulating a scrolled/settled streaming log).
    let (filled, moved) = ts
        .fill_paragraphs(pl0, std::time::Duration::MAX, (0.0, f32::INFINITY), f32::INFINITY)
        .expect("fill");
    assert!(!moved);
    let filled_tl = TextLayout::Paragraphs(filled.clone());

    content.push_str("line 300 appended\n");
    let shaped_before = ts.shaped;
    let tl1 = ts.layout_text(&rich_input(&content, &spans, style, true, TextDir::Auto), false, scale, max_w, Alignment::Start, Some(&filled_tl));
    let pl1 = as_paragraphs(&tl1);
    assert!(!pl1.waiting(), "appending one line must not re-defer already-shaped lines");
    assert!(ts.shaped - shaped_before <= 2, "streaming append must shape only the new line, got {}", ts.shaped - shaped_before);
    assert!(Rc::ptr_eq(&pl1.paragraphs[100].layout, &filled.paragraphs[100].layout), "interior line 100 reused");
}

#[test]
fn paragraphs_invalidate_when_pill_switches_to_or_resizes_replaced_image() {
    let content = "first line\noi :gato: tudo";
    let start = content.find(":gato:").unwrap() as u32;
    let end = start + ":gato:".len() as u32;
    let style = TextStyle::new(14.0, 400.0);
    let scale = 1.0;

    let mut ts = TextSystem::default();
    let chip = [PillChip {
        start,
        end,
        icon: None,
        image: None,
        bg: None,
        fg: None,
        ring: None,
        radius: None,
        pad_x: None,
        margin_x: None,
        bare: None,
    }];
    let replaced = |w: f32, asc: f32, desc: f32| [PillChip {
        start,
        end,
        icon: None,
        image: None,
        bg: None,
        fg: None,
        ring: None,
        radius: None,
        pad_x: None,
        margin_x: None,
        bare: Some(Box::new(BareIcon {
            size: 0.0,
            gap: 0.0,
            radius: 0.0,
            image: None,
            replace: Some(Replace { width: w, ascent: asc, descent: desc, bleed: 0.0 }),
            label: None,
            help: None,
        })),
    }];

    fn input_with<'a>(content: &'a str, style: TextStyle, pills: &'a [PillChip]) -> TextInput<'a> {
        let mut inp = rich_input(content, &[], style, true, TextDir::Auto);
        inp.pills = pills;
        inp
    }

    let tl_chip = ts.layout_text(&input_with(content, style, &chip), false, scale, Some(300.0), Alignment::Start, None);
    let pl_chip = as_paragraphs(&tl_chip);

    let r_small = replaced(18.0, 14.0, 4.0);
    let tl_small = ts.layout_text(&input_with(content, style, &r_small), false, scale, Some(300.0), Alignment::Start, Some(&tl_chip));
    let pl_small = as_paragraphs(&tl_small);
    assert!(Rc::ptr_eq(&pl_small.paragraphs[0].layout, &pl_chip.paragraphs[0].layout), "unchanged first paragraph reused");
    assert!(!Rc::ptr_eq(&pl_small.paragraphs[1].layout, &pl_chip.paragraphs[1].layout), "pill paragraph must re-shape when switching to replaced image");

    let r_big = replaced(40.0, 26.0, 8.0);
    let tl_big = ts.layout_text(&input_with(content, style, &r_big), false, scale, Some(300.0), Alignment::Start, Some(&tl_small));
    let pl_big = as_paragraphs(&tl_big);
    assert!(!Rc::ptr_eq(&pl_big.paragraphs[1].layout, &pl_small.paragraphs[1].layout), "pill paragraph must re-shape when replaced image size changes");
    assert!(pl_big.paragraphs[1].height > pl_small.paragraphs[1].height + 4.0, "taller replaced image grows paragraph height");
}
