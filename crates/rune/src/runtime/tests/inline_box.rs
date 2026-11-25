//! Headless Runtime tests for `El::inline` (`InlineBox` / `InlineAlign`): exact placement,
//! whole-box wrapping, line-height growth, nested text, relayout resize fast path, memo reuse,
//! pointer hit testing (including after scroll), keyboard focus reading order, selection/copy
//! skipping the box, `Tx::link_rect` beside an inline box, truncation/line-clamp hiding, RTL,
//! AccessKit bounds, web a11y mirror, PaintKeep & `text_quads` isolation, and zero idle wakes.

use super::*;
use crate::color::Color;
use crate::el::{col, el, rich, row, text, text_input};
use crate::{
    Direction, Env, InlineAlign, InlineBox, Key, KeyEvent, Link, Mods, ScrollSpec, Span, TextCase,
    TextEdit, TextStyle, memo,
};

const CHIP_BG: Color = Color { r: 0.16, g: 0.42, b: 0.85, a: 1.0 };

fn body_style() -> TextStyle {
    TextStyle::new(14.0, 400.0).lh(20.0)
}

fn find_rect<S: 'static>(rt: &Runtime<S>, key: &str) -> Option<[f32; 4]> {
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key))
        .map(|e| e.rect)
}

fn key_ev(key: Key, mods: Mods) -> KeyEvent {
    KeyEvent { key, mods }
}

fn settle<S: 'static>(rt: &mut Runtime<S>) {
    for _ in 0..30 {
        if !rt.frame(1.0 / 60.0) {
            return;
        }
    }
}

#[test]
fn placement_wrapping_line_growth_and_nested_text() {
    struct State {
        width: f32,
        tall: bool,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let h = if s.tall { 36.0 } else { 18.0 };
        let slot = InlineBox::new(96.0, h).align(InlineAlign::TextCenter);
        let badge = row()
            .key("badge")
            .fill(CHIP_BG)
            .radius(4.0)
            .center()
            .child(text("config.toml").key("badge-label").font(TextStyle::new(11.0, 500.0).mono()));
        col().w_full().padding(16.0).child(
            text("Open  and edit it.")
                .key("line")
                .w(s.width)
                .font(body_style())
                .wrap()
                .inline(5, slot, badge),
        )
    }

    // Wide single line: badge sits between "Open " and " and edit it." at exact (96, 18) size.
    let mut rt = Runtime::new(State { width: 320.0, tall: false }, view, (400.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let line_r = find_rect(&rt, "line").expect("line rect");
    let badge_r = find_rect(&rt, "badge").expect("badge rect");
    let label_r = find_rect(&rt, "badge-label").expect("nested text inside badge");
    assert!((badge_r[2] - 96.0).abs() < 0.1 && (badge_r[3] - 18.0).abs() < 0.1, "badge size: {badge_r:?}");
    assert!(badge_r[0] > line_r[0] + 25.0, "badge sits after 'Open ': {badge_r:?} vs {line_r:?}");
    assert!(badge_r[1] >= line_r[1] - 1.0 && badge_r[1] + badge_r[3] <= line_r[1] + line_r[3] + 1.0);
    // Nested text inside the inline child laid out and placed inside the badge.
    assert!(label_r[2] > 20.0 && label_r[3] > 8.0, "nested text laid out: {label_r:?}");
    assert!(
        label_r[0] >= badge_r[0] - 1.0 && label_r[0] + label_r[2] <= badge_r[0] + badge_r[2] + 1.0,
        "nested label fits inside badge: label={label_r:?} badge={badge_r:?}",
    );
    // Zero idle wakes at rest.
    settle(&mut rt);
    assert!(!rt.frame(1.0 / 60.0), "idle window with inline box must not keep asking for frames");
    assert!(rt.wake_deadline().is_none(), "idle window with inline box must schedule no wake");

    // Narrow width: box wraps whole onto line 1 (never splits).
    let open_w = crate::text::measure("Open ", &[], body_style(), None).0;
    rt.state.width = open_w + 48.0;
    rt.commit.dirty = true;
    settle(&mut rt);
    let line_wrap = find_rect(&rt, "line").unwrap();
    let badge_wrap = find_rect(&rt, "badge").unwrap();
    assert!(badge_wrap[1] > badge_r[1] + 10.0, "badge wrapped whole onto next line: {badge_wrap:?} vs {badge_r:?}");
    assert!((badge_wrap[0] - line_wrap[0]).abs() < 1.5, "wrapped badge starts at line left: {badge_wrap:?} vs {line_wrap:?}");
    assert!((badge_wrap[2] - 96.0).abs() < 0.1, "wrapped badge keeps full atomic width");

    // Taller box grows line height.
    rt.state.width = 320.0;
    rt.state.tall = true;
    rt.commit.dirty = true;
    settle(&mut rt);
    let line_tall = find_rect(&rt, "line").unwrap();
    assert!(line_tall[3] > line_r[3] + 10.0, "taller inline box grows line height: {line_tall:?} vs {line_r:?}");
}

#[test]
fn relayout_resize_fast_path_and_memo_reuse_update_inline_children() {
    use std::cell::Cell;
    use std::rc::Rc;

    struct State {
        builds: Rc<Cell<usize>>,
        version: u64,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        s.builds.set(s.builds.get() + 1);
        let ver = s.version;
        col().w_full().padding(12.0).child(memo("inline-memo", ver, move || {
            let slot = InlineBox::new(90.0, 18.0).align(InlineAlign::TextCenter);
            let label = format!("v{ver}");
            let chip = row().key("m-chip").fill(CHIP_BG).center().child(text(label).key("m-text"));
            text("Prefix  suffix").key("m-line").w_full().font(body_style()).wrap().inline(7, slot, chip)
        }))
    }

    let builds = Rc::new(Cell::new(0));
    let mut rt = Runtime::new(State { builds: builds.clone(), version: 1 }, view, (320.0, 160.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let wide_chip = find_rect(&rt, "m-chip").expect("wide chip");
    let wide_builds = builds.get();

    // Window resize hits Relayout fast path (no view rebuild) and moves inline child onto line 1.
    rt.set_viewport((120.0, 160.0), 1.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(builds.get(), wide_builds, "resize took Relayout fast path without rebuilding view");
    let narrow_chip = find_rect(&rt, "m-chip").expect("narrow chip");
    assert!(narrow_chip[1] > wide_chip[1] + 10.0, "relayout wrapped inline child: {narrow_chip:?} vs {wide_chip:?}");

    // Another commit with same memo key reuses memoized text + inline child cleanly.
    rt.commit.dirty = true;
    rt.frame(1.0 / 60.0);
    let reused_chip = find_rect(&rt, "m-chip").expect("reused chip");
    assert_eq!(reused_chip, narrow_chip);

    // Changing memo key rebuilds with updated child.
    rt.state.version = 2;
    rt.commit.dirty = true;
    rt.frame(1.0 / 60.0);
    let updated_text = rt.core().tree.nodes.iter().find(|n| n.key.as_deref() == Some("m-text")).unwrap();
    assert_eq!(updated_text.text.as_deref().unwrap().content, "v2");
}

#[test]
fn pointer_press_hover_and_scroll_hit_inline_child() {
    #[derive(Default)]
    struct State {
        presses: usize,
        hovered: bool,
    }

    fn view(_: &State, _: &mut Cx<State>) -> El {
        let slot = InlineBox::new(60.0, 20.0).align(InlineAlign::TextCenter);
        let btn = row()
            .key("inline-btn")
            .fill(CHIP_BG)
            .center()
            .role("button")
            .label("Edit")
            .focusable(true)
            .on_hover_change(|s: &mut State, inside, _| s.hovered = inside)
            .on_press(|s: &mut State| s.presses += 1)
            .child(text("Edit").font(TextStyle::new(12.0, 500.0)));
        col()
            .key("scroller")
            .w(280.0)
            .h(120.0)
            .scroll(ScrollSpec::default())
            .child(el().h(80.0).w_full())
            .child(
                text("Open  now")
                    .key("scrolled-line")
                    .w_full()
                    .font(body_style())
                    .wrap()
                    .inline(5, slot, btn),
            )
            .child(el().h(260.0).w_full())
    }

    let mut rt = Runtime::new(State::default(), view, (320.0, 160.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let r0 = find_rect(&rt, "inline-btn").expect("button rect before scroll");
    let (cx, cy) = (r0[0] + r0[2] * 0.5, r0[1] + r0[3] * 0.5);

    // Hover and click before scroll.
    rt.pointer_move(cx, cy);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.hovered, "hover hits inline child before scroll");
    rt.pointer_down(cx, cy);
    rt.pointer_up(cx, cy);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.presses, 1, "press hits inline child before scroll");

    // Scroll down by 40 pt: button moves up by 40 pt on the same frame and hits at new coords.
    rt.wheel(140.0, 60.0, -40.0);
    settle(&mut rt);
    let r1 = find_rect(&rt, "inline-btn").expect("button rect after scroll");
    assert!(r1[1] < r0[1] - 10.0, "inline child scrolled with text: {r1:?} vs {r0:?}");
    let (cx1, cy1) = (r1[0] + r1[2] * 0.5, r1[1] + r1[3] * 0.5);
    rt.pointer_down(cx1, cy1);
    rt.pointer_up(cx1, cy1);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.presses, 2, "press hits inline child at scrolled coordinates");
}

#[test]
fn focus_order_respects_reading_order_and_skips_elided_children() {
    struct State {
        width: f32,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let slot = InlineBox::new(40.0, 18.0).align(InlineAlign::TextCenter);
        let first = el().key("first-btn").role("button").label("First").focusable(true);
        let second = el().key("second-btn").role("button").label("Second").focusable(true);
        // Deliberately attach second (at byte 14) BEFORE first (at byte 6) to verify reading-order sort.
        col().w_full().padding(8.0).child(
            text("Alpha  middle  omega")
                .key("line")
                .w(s.width)
                .font(body_style())
                .truncate()
                .inline(14, slot, second)
                .inline(6, slot, first),
        )
    }

    let mut rt = Runtime::new(State { width: 320.0 }, view, (360.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // First Tab lands on "first-btn" (byte 6), second Tab lands on "second-btn" (byte 14).
    rt.key(key_ev(Key::Tab, Mods::default()));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.focused_key().as_deref(), Some("first-btn"), "first Tab hits byte-6 button");
    rt.key(key_ev(Key::Tab, Mods::default()));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.focused_key().as_deref(), Some("second-btn"), "second Tab hits byte-14 button");

    // Narrow to truncate off the second button: Tab must skip the elided button.
    rt.state.width = 110.0;
    rt.commit.dirty = true;
    rt.frame(1.0 / 60.0);
    let r_second = find_rect(&rt, "second-btn").unwrap();
    assert_eq!(r_second, [0.0; 4], "elided button has zero drawn rect");
    rt.key(key_ev(Key::Tab, Mods::default()));
    rt.frame(1.0 / 60.0);
    assert_ne!(rt.focused_key().as_deref(), Some("second-btn"), "Tab must skip elided inline button");
}

#[test]
fn selection_copy_skips_box_and_link_rect_stays_accurate() {
    #[derive(Default)]
    struct State {
        link_rects: Vec<(String, Option<[f32; 4]>)>,
    }

    fn view(_: &State, _: &mut Cx<State>) -> El {
        let slot = InlineBox::new(50.0, 18.0).align(InlineAlign::TextCenter);
        let badge = el().key("badge").fill(CHIP_BG);
        col().w_full().padding(16.0).child(
            text("Open  and read docs")
                .key("sel-line")
                .font(body_style())
                .selectable(true)
                .link(Link::new(15..19, "docs"))
                .on_link(|s: &mut State, id, tx| s.link_rects.push((id.to_owned(), tx.link_rect())))
                .inline(5, slot, badge),
        )
    }

    let mut rt = Runtime::new(State::default(), view, (360.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let line_r = find_rect(&rt, "sel-line").unwrap();
    let badge_r = find_rect(&rt, "badge").unwrap();

    // Drag-select across the whole line (crossing over the inline box) and copy: box is skipped.
    let y = line_r[1] + line_r[3] * 0.5;
    rt.pointer_down(line_r[0] + 1.0, y);
    rt.pointer_move(line_r[0] + line_r[2] - 1.0, y);
    rt.pointer_up(line_r[0] + line_r[2] - 1.0, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.selected_text().as_deref(), Some("Open  and read docs"));

    // Click the "docs" link after the inline box: Tx::link_rect returns its exact rect past the box.
    let node_idx = rt.core().tree.nodes.iter().position(|n| n.key.as_deref() == Some("sel-line")).unwrap();
    let expected = rt.core().link_first_rect(node_idx, 0).expect("link rect exists");
    assert!(expected[0] > badge_r[0] + badge_r[2], "link sits past the inline box: {expected:?} vs {badge_r:?}");
    let (lx, ly) = (expected[0] + expected[2] * 0.5, expected[1] + expected[3] * 0.5);
    rt.pointer_down(lx, ly);
    rt.pointer_up(lx, ly);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.link_rects.len(), 1);
    assert_eq!(rt.state.link_rects[0].0, "docs");
    assert_eq!(rt.state.link_rects[0].1, Some(expected));
}

#[test]
fn truncation_and_max_lines_hide_elided_children_from_paint_hit_a11y_and_web_mirror() {
    struct State {
        width: f32,
        presses: usize,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let slot = InlineBox::new(48.0, 18.0).align(InlineAlign::TextCenter);
        let btn = row()
            .key("tail-btn")
            .fill(CHIP_BG)
            .role("button")
            .label("Tail Action")
            .focusable(true)
            .on_press(|s: &mut State| s.presses += 1)
            .child(text("Go").key("tail-go"));
        col().w_full().padding(12.0).child(
            text("A long introductory sentence before the trailing button ")
                .key("clamped")
                .w(s.width)
                .font(body_style())
                .wrap()
                .max_lines(1)
                .inline(56, slot, btn),
        )
    }

    let mut rt = Runtime::new(State { width: 160.0, presses: 0 }, view, (360.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Elided child has zero rect, emits no painted CHIP_BG rect, is absent from AccessKit and web mirror.
    assert_eq!(find_rect(&rt, "tail-btn").unwrap(), [0.0; 4]);
    let painted_chip = rt.instances.iter().any(|q| {
        q.params[1] == crate::render::KIND_RECT
            && (q.fill[0] - CHIP_BG.r).abs() < 0.03
            && (q.fill[2] - CHIP_BG.b).abs() < 0.03
    });
    assert!(!painted_chip, "elided inline child must not paint");

    let a11y_str = rt.accessibility_json().to_string();
    assert!(!a11y_str.contains("Tail Action"), "elided inline child omitted from AccessKit: {a11y_str}");
    let web_items = rt.core().web_a11y_items();
    assert!(!web_items.iter().any(|it| it.text.contains("Tail Action")), "elided inline child omitted from web mirror");

    // Clicking at (0, 0) or inside the text box does not trigger the elided button.
    rt.pointer_down(14.0, 14.0);
    rt.pointer_up(14.0, 14.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.presses, 0);

    // Widening restores the inline child across paint, AccessKit, web mirror, and hit testing.
    rt.state.width = 480.0;
    rt.commit.dirty = true;
    rt.frame(1.0 / 60.0);
    let shown_r = find_rect(&rt, "tail-btn").unwrap();
    assert!((shown_r[2] - 48.0).abs() < 0.1 && (shown_r[3] - 18.0).abs() < 0.1, "restored child rect: {shown_r:?}");
    assert!(rt.accessibility_json().to_string().contains("Tail Action"), "restored child present in AccessKit");
    assert!(
        rt.core().web_a11y_items().iter().any(|it| it.text.contains("Tail Action") && it.rect[2] > 40.0),
        "restored child present in web mirror with bounds",
    );
}

#[test]
fn rtl_placement_accesskit_bounds_and_web_mirror_reading_order() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let slot = InlineBox::new(36.0, 18.0).align(InlineAlign::TextCenter);
        let badge = row()
            .key("rtl-badge")
            .fill(CHIP_BG)
            .role("button")
            .label("RTL Chip")
            .focusable(true)
            .child(text("42"));
        let rtl_text = "שלום  עולם";
        let mid = "שלום ".len();
        col().w(260.0).padding(12.0).child(
            text(rtl_text)
                .key("rtl-line")
                .w_full()
                .dir(Direction::Rtl)
                .font(body_style())
                .wrap()
                .inline(mid, slot, badge),
        )
    }

    let mut rt = Runtime::new((), view, (300.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let line_r = find_rect(&rt, "rtl-line").unwrap();
    let badge_r = find_rect(&rt, "rtl-badge").unwrap();
    assert!((badge_r[2] - 36.0).abs() < 0.1 && (badge_r[3] - 18.0).abs() < 0.1);
    assert!(
        badge_r[0] > line_r[0] + line_r[2] * 0.4 && badge_r[0] + badge_r[2] <= line_r[0] + line_r[2] + 1.0,
        "RTL inline child sits on the right half: badge={badge_r:?}, line={line_r:?}",
    );

    // AccessKit reports the inline child with bounds matching its drawn rect.
    let tree = rt.accessibility_tree();
    let ak_badge = tree
        .nodes
        .iter()
        .find(|(_, n)| n.label() == Some("RTL Chip"))
        .expect("RTL Chip in AccessKit")
        .1
        .bounds()
        .expect("RTL Chip has AccessKit bounds");
    assert!((ak_badge.x0 as f32 - badge_r[0]).abs() <= 1.0 && (ak_badge.y0 as f32 - badge_r[1]).abs() <= 1.0);

    // Web a11y mirror reports both the parent text and the inline child button with bounds.
    let web_items = rt.core().web_a11y_items();
    let text_pos = web_items.iter().position(|it| it.kind == "text").expect("parent text in web mirror");
    let btn_pos = web_items.iter().position(|it| it.text == "RTL Chip").expect("inline button in web mirror");
    assert!(btn_pos > text_pos, "inline child follows text in web mirror order");
    let wb = web_items[btn_pos].rect;
    assert!((wb[0] - badge_r[0]).abs() <= 1.0 && (wb[2] - badge_r[2]).abs() <= 1.0);
}

#[test]
fn paint_keep_text_quads_and_noop_edge_cases() {
    // 1. No-op on text_input, non-text El, and out-of-range / non-char-boundary offsets.
    let edit = TextEdit::new("hello");
    let field = text_input(&edit, body_style()).inline(2, InlineBox::new(20.0, 16.0), el().key("ignored-field"));
    assert!(field.children.is_empty());
    assert!(field.text.as_ref().unwrap().cold.inline_boxes.is_empty());

    let plain_box = el().inline(0, InlineBox::new(20.0, 16.0), el().key("ignored-box"));
    assert!(plain_box.children.is_empty());

    let utf8 = text("café")
        .inline(4, InlineBox::new(20.0, 16.0), el()) // mid-char inside 'é'
        .inline(99, InlineBox::new(20.0, 16.0), el()); // out of bounds
    assert!(utf8.children.is_empty());
    assert!(utf8.text.as_ref().unwrap().cold.inline_boxes.is_empty());

    // 2. PaintKeep zero mismatches across steady frames & text_quads never copies from a layout without the box.
    struct Toggle {
        with_box: bool,
    }
    fn toggle_view(s: &Toggle, _: &mut Cx<Toggle>) -> El {
        let mut t = rich("Open  file", vec![Span::new(0, 4).bold()])
            .key("t")
            .font(body_style())
            .text_case(TextCase::Uppercase)
            .isolate(0..4, Direction::Ltr)
            .dir_by_paragraph();
        if s.with_box {
            t = t.inline(5, InlineBox::new(40.0, 18.0).align(InlineAlign::TextCenter), el().key("b").fill(CHIP_BG));
        }
        row().padding(12.0).child(t)
    }

    let mut rt = Runtime::new(Toggle { with_box: false }, toggle_view, (300.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let plain_w = find_rect(&rt, "t").unwrap()[2];
    let plain_glyphs: Vec<[f32; 4]> = rt.instances.iter().filter(|q| q.params[1] == crate::render::KIND_GLYPH).map(|q| q.rect).collect();

    // Flip to with_box = true on the same NodeId: text_quads MUST NOT copy the old unboxed glyph quads.
    rt.state.with_box = true;
    rt.commit.dirty = true;
    settle(&mut rt);
    let boxed_w = find_rect(&rt, "t").unwrap()[2];
    let boxed_glyphs: Vec<[f32; 4]> = rt.instances.iter().filter(|q| q.params[1] == crate::render::KIND_GLYPH).map(|q| q.rect).collect();
    assert!(boxed_w > plain_w + 35.0, "inline box widened text: {plain_w} -> {boxed_w}");
    assert_ne!(boxed_glyphs, plain_glyphs, "glyph quads shifted for inline box instead of copying stale text_quads");

    // Steady frame triggers PaintKeep verification in debug builds: must have 0 mismatches.
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.core().paint.keep_check_mismatches(), 0, "PaintKeep copied inline-box subtree with zero mismatches");
}
