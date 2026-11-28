use accesskit::{Node as AkNode, Role};

use crate::color::Color;
use crate::el::{col, rich, text};
use crate::text::{Link, Span, TextHighlight};
use crate::{Cx, El, Env, Icon, InlineIcon, InlineImage, Key, KeyEvent, Mods, Pixels, Runtime, TextStyle};

const HL_BG: Color = Color { r: 1.0, g: 0.9, b: 0.2, a: 0.5 };
const LINK_BLUE: Color = Color { r: 0.1, g: 0.4, b: 0.9, a: 1.0 };
const GLOBE: Icon = Icon { name: "globe", view: 16.0, body: "" };

fn px(w: u32, h: u32) -> Pixels {
    let p = Pixels::new();
    p.set(w, h, vec![255; (w * h * 4) as usize]);
    p
}

#[test]
fn span_struct_literal_remains_unchanged_and_compatible() {
    let s = Span {
        start: 0,
        end: 4,
        weight: Some(600.0),
        mono: false,
        size: None,
        color: Some(LINK_BLUE),
        bg: None,
        bg_pad: 0.0,
        underline: None,
        italic: true,
    };
    assert_eq!((s.start, s.end, s.italic), (0, 4, true));
}

#[test]
fn multiple_inline_images_work_inside_styled_spans_and_clickable_links_with_highlights() {
    const BODY: &str = "see ￼ and ￼ end";
    struct S {
        clicked: Vec<String>,
        highlight_image_only: bool,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let first = BODY.find('￼').unwrap();
        let first_end = first + '￼'.len_utf8();
        let second = BODY.rfind('￼').unwrap();
        let second_end = second + '￼'.len_utf8();
        let hl = if s.highlight_image_only {
            vec![TextHighlight::new(first..first_end, HL_BG)]
        } else {
            vec![TextHighlight::new(0..BODY.len(), HL_BG)]
        };
        let t = rich(
            BODY,
            vec![
                Span::new(0, BODY.len()).italic(),
                Span::new(first, second_end).bold().color(LINK_BLUE),
            ],
        )
        .key("t")
        .font(TextStyle::new(14.0, 400.0).lh(20.0))
        .inline_image(first..first_end, InlineImage::new(px(40, 32), 20.5, 16.25, 4.75).rise(2.5))
        .inline_image(second..second_end, InlineImage::new(px(30, 24), 15.25, 10.5, 2.5).label("link formula"))
        .link(Link::new(second..second_end, "img-link"))
        .on_link(|s: &mut S, id, _| {
            s.clicked.push(id.to_owned());
        })
        .text_highlights(hl);
        col().key("root").padding(10.0).child(t)
    }

    let mut rt = Runtime::new(
        S { clicked: Vec::new(), highlight_image_only: true },
        view,
        (400.0, 120.0),
        Env::new(true, 2.0),
    );
    rt.frame(0.0);

    // Two inline image quads painted, plus an image-only highlight rect matching the first image's box.
    let img_quads: Vec<[f32; 4]> = rt
        .instances
        .iter()
        .filter(|q| q.params[1] == crate::render::KIND_IMAGE)
        .map(|q| q.rect)
        .collect();
    assert_eq!(img_quads.len(), 2, "both inline images painted: {img_quads:?}");

    let hl_quads: Vec<[f32; 4]> = rt
        .instances
        .iter()
        .filter(|q| q.params[1] == crate::render::KIND_RECT && q.fill[3] > 0.1)
        .map(|q| q.rect)
        .collect();
    assert_eq!(hl_quads.len(), 1, "image-only highlight painted: {hl_quads:?}");
    // Highlight rect covers the first image's width (20.5 * 2 = 41 px) and height (21 * 2 = 42 px).
    assert!(hl_quads[0][2] >= 40.0, "image-only highlight width covers image: {:?}", hl_quads[0]);
    assert!(hl_quads[0][3] >= 40.0, "image-only highlight height covers tall image: {:?}", hl_quads[0]);

    // Link wrapping the second inline image speaks the image's accessible label.
    rt.a11y_activate("Test");
    let update = rt.a11y_update().expect("initial a11y tree");
    let link_node = update
        .nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.role() == Role::Link)
        .expect("inline link a11y node present");
    assert_eq!(link_node.label(), Some("link formula"));

    // Clicking directly on the second inline image fires its enclosing link.
    let second_center_pt = (
        (img_quads[1][0] + img_quads[1][2] * 0.5) / 2.0,
        (img_quads[1][1] + img_quads[1][3] * 0.5) / 2.0,
    );
    rt.pointer_move(second_center_pt.0, second_center_pt.1);
    rt.pointer_down(second_center_pt.0, second_center_pt.1);
    rt.pointer_up(second_center_pt.0, second_center_pt.1);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.clicked, ["img-link"], "clicking inline image inside link activates link");

    // Switching to a full-line highlight covers the entire line + tall image without a gap.
    rt.update(|s, _| s.highlight_image_only = false);
    rt.frame(2.0 / 60.0);
    let full_hl: Vec<[f32; 4]> = rt
        .instances
        .iter()
        .filter(|q| q.params[1] == crate::render::KIND_RECT && q.fill[3] > 0.1)
        .map(|q| q.rect)
        .collect();
    assert_eq!(full_hl.len(), 1, "full-line highlight merges across both images without gaps: {full_hl:?}");
    assert!(full_hl[0][3] >= 40.0, "full-line highlight covers tall raised image vertically: {:?}", full_hl[0]);
}

#[test]
fn inline_image_a11y_label_substitutes_text_runs_name_from_content_and_fingerprint_without_sibling_image() {
    const BODY: &str = "solve ￼ now";
    struct S {
        formula: String,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let start = BODY.find('￼').unwrap();
        let end = start + '￼'.len_utf8();
        let sel_text = text(BODY)
            .key("selectable")
            .font(TextStyle::new(14.0, 400.0).lh(20.0))
            .selectable(true)
            .inline_image(start..end, InlineImage::new(px(40, 20), 28.0, 10.0, 3.0).label(s.formula.clone()));
        let btn = col()
            .key("btn")
            .role("tab")
            .on_press(|_: &mut S| {})
            .child(
                text(BODY)
                    .font(TextStyle::new(14.0, 400.0).lh(20.0))
                    .inline_image(start..end, InlineImage::new(px(40, 20), 28.0, 10.0, 3.0).label(s.formula.clone())),
            );
        let icon_text = text("visit site")
            .key("with-icon")
            .font(TextStyle::new(14.0, 400.0).lh(20.0))
            .inline_icon(6, InlineIcon::glyph(&GLOBE).label("external link"));
        col().key("root").padding(8.0).child(sel_text).child(btn).child(icon_text)
    }

    let mut rt = Runtime::new(
        S { formula: "α² + β²".to_owned() },
        view,
        (500.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    rt.a11y_activate("Test");
    let update = rt.a11y_update().expect("initial a11y tree");
    let nodes: Vec<&AkNode> = update.nodes.iter().map(|(_, n)| n).collect();

    // 1. Selectable text node has substituted value and TextRun with valid lengths/positions/widths/word_starts.
    let sel_node = nodes
        .iter()
        .find(|n| n.author_id() == Some("selectable"))
        .expect("selectable text node present");
    assert_eq!(sel_node.value(), Some("solve α² + β² now"));

    let run_id = sel_node.children().first().copied().expect("TextRun child present");
    let run_node = update
        .nodes
        .iter()
        .find_map(|(id, n)| (*id == run_id).then_some(n))
        .expect("TextRun node in update");
    assert_eq!(run_node.role(), Role::TextRun);
    let run_val = run_node.value().expect("TextRun has value");
    assert_eq!(run_val, "solve α² + β² now");

    let lengths = run_node.character_lengths();
    let positions = run_node.character_positions().expect("character_positions");
    let widths = run_node.character_widths().expect("character_widths");
    let word_starts = run_node.word_starts();

    assert_eq!(positions.len(), lengths.len());
    assert_eq!(widths.len(), lengths.len());
    let mut byte_acc = 0usize;
    for &len in lengths {
        byte_acc += usize::from(len);
        assert!(run_val.is_char_boundary(byte_acc), "cumulative length {byte_acc} is char boundary");
    }
    assert_eq!(byte_acc, run_val.len(), "character_lengths sums to spoken UTF-8 length");
    for w in positions.windows(2) {
        assert!(w[1] >= w[0] - 1e-3, "positions non-decreasing: {positions:?}");
    }
    // The 7 chars of "α² + β²" (indices 6..13) span the inline image's 28 pt box width.
    let label_w: f32 = widths[6..13].iter().sum();
    assert!((label_w - 28.0).abs() < 0.5, "substituted label chars cover image width: {label_w}");
    assert!(!word_starts.is_empty(), "word_starts populated for spoken text");
    for &ws in word_starts {
        assert!((ws as usize) < lengths.len(), "word_start {ws} in bounds of {}", lengths.len());
    }

    // 2. No sibling Role::Image node for replaced inline images (avoid double-speak),
    //    while InlineIcon::label still emits its sibling Role::Image node.
    let image_labels: Vec<Option<&str>> = nodes
        .iter()
        .filter(|n| n.role() == Role::Image)
        .map(|n| n.label())
        .collect();
    assert_eq!(image_labels, vec![Some("external link")], "only InlineIcon emits Role::Image");

    // 3. Name-from-content on button substitutes the inline image label.
    let btn_node = nodes
        .iter()
        .find(|n| n.author_id() == Some("btn"))
        .expect("button node present");
    assert_eq!(btn_node.label(), Some("solve α² + β² now"));

    // 4. Web a11y mirror clean_slice also substitutes the inline image label.
    let sel_tree_node = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("selectable")).unwrap();
    let spec = sel_tree_node.text.as_deref().unwrap();
    assert_eq!(crate::runtime::web_a11y::clean_slice(spec, 0, spec.content.len()), "solve α² + β² now");

    // 5. Selection & copy still keep the replaced bytes (U+FFFC), not the a11y label.
    rt.update(|_, tx| tx.select_all_text("selectable"));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.selected_text().as_deref(), Some(BODY));
    rt.key(KeyEvent { key: Key::Command('c'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(2.0 / 60.0);
    assert_eq!(rt.copy_request.as_ref().map(|w| w.content().text()), Some(BODY));

    // 6. Changing only the inline image's label updates both TextRun value and button name-from-content
    //    (proving a11y_print and a11y_content_prints hash the label).
    let _ = rt.a11y_update();
    rt.update(|s, _| s.formula = "E = mc^2".to_owned());
    rt.frame(3.0 / 60.0);
    let diff = rt.a11y_update().expect("label change invalidates a11y tree");
    assert!(
        diff.nodes.iter().any(|(_, n)| n.author_id() == Some("btn") && n.label() == Some("solve E = mc^2 now")),
        "button name-from-content updated when only inline image label changed",
    );
    assert!(
        diff.nodes.iter().any(|(_, n)| n.role() == Role::TextRun && n.value() == Some("solve E = mc^2 now")),
        "TextRun updated when only inline image label changed",
    );
}
