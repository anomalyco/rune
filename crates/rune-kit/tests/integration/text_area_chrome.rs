//! `TextArea::plain()`, `TextArea::trailing(El)`, and `TextArea::scroll_bar(bool)` /
//! `TextArea::scroll_bar_styled(ScrollBarStyle)`.

use rune::runtime::ElementInfo;
use rune::{Color, Cx, Direction, El, Env, Ime, Key, KeyEvent, Mods, Runtime, ScrollBars, TextEdit, col, row};
use rune_kit::{Button, ControlSize, Kit, ScrollBarStyle, State, TextArea};

fn find<S>(rt: &Runtime<S>, key: &str) -> ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("missing key {key}"))
}

fn find_path<S>(rt: &Runtime<S>, suffix: &str) -> ElementInfo {
    rt.elements()
        .into_iter()
        .find(|e| e.path.ends_with(suffix))
        .unwrap_or_else(|| panic!("missing path ending with {suffix}"))
}

fn has_path<S>(rt: &Runtime<S>, suffix: &str) -> bool {
    rt.elements().into_iter().any(|e| e.path.ends_with(suffix))
}

fn settle<S>(rt: &mut Runtime<S>) {
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
}

fn key<S>(rt: &mut Runtime<S>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    rt.frame(1.0 / 60.0);
}

fn click<S>(rt: &mut Runtime<S>, x: f32, y: f32) {
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

/// Counts box-sized paint instances (fill, border, shadow, outline) whose centre falls inside
/// `key`'s rect and whose width spans the box, ignoring opacity layer markers.
fn box_ink<S>(rt: &Runtime<S>, key: &str) -> usize {
    let r = find(rt, key).rect;
    rt.instances
        .iter()
        .filter(|i| {
            let kind = i.params[1];
            if kind == rune::render::KIND_LAYER || kind == rune::render::KIND_LAYER_END {
                return false;
            }
            let (cx, cy) = (i.rect[0] + i.rect[2] * 0.5, i.rect[1] + i.rect[3] * 0.5);
            cx >= r[0] && cx <= r[0] + r[2] && cy >= r[1] && cy <= r[1] + r[3] && i.rect[2] >= r[2] - 0.5
        })
        .count()
}

#[test]
fn plain_omits_chrome_keeps_padding_auto_grow_and_press_focus() {
    struct S {
        boxed: TextEdit,
        plain: TextEdit,
        composer: TextEdit,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let k = Kit::neutral(true);
        col()
            .w_full()
            .padding(16.0)
            .gap(12.0)
            .child(
                TextArea::new(&s.boxed)
                    .placeholder("Boxed")
                    .auto_grow(1, 3)
                    .on_edit(|s: &mut S| &mut s.boxed)
                    .build(&k)
                    .key("boxed"),
            )
            .child(
                TextArea::new(&s.plain)
                    .plain()
                    .placeholder("Plain")
                    .auto_grow(1, 3)
                    .on_edit(|s: &mut S| &mut s.plain)
                    .build(&k)
                    .key("plain"),
            )
            .child(
                col()
                    .key("composer-box")
                    .role("textbox")
                    .w(280.0)
                    .padding(6.0)
                    .child(
                        TextArea::new(&s.composer)
                            .plain()
                            .width(None)
                            .auto_grow(1, 3)
                            .placeholder("Message…")
                            .on_edit(|s: &mut S| &mut s.composer)
                            .build(&k)
                            .key("composer"),
                    ),
            )
    }

    let mut rt = Runtime::new(
        S { boxed: TextEdit::new("one"), plain: TextEdit::new("one"), composer: TextEdit::new("") },
        view,
        (400.0, 420.0),
        Env::new(true, 1.0),
    );
    settle(&mut rt);

    let line = 17.5;
    let boxed = find(&rt, "boxed");
    let plain = find(&rt, "plain");
    let plain_in = find_path(&rt, "plain/scroll/input");
    assert_eq!(plain.role, "textbox");
    assert_eq!(plain.fill.a, 0.0, "plain TextArea has no fill");
    assert!(boxed.fill.a > 0.0, "default boxed TextArea keeps its fill");
    assert_eq!(box_ink(&rt, "plain"), 0, "unfocused plain TextArea draws no box, border or shadow");
    assert!(box_ink(&rt, "boxed") > 0, "default boxed TextArea draws its box");
    assert!((plain_in.rect[0] - plain.rect[0] - 8.0).abs() < 0.01, "8 pt left padding preserved");
    assert!((plain_in.rect[1] - plain.rect[1] - 8.0).abs() < 0.01, "8 pt top padding preserved");
    assert!((plain.rect[3] - (line + 16.0)).abs() < 0.05, "one row plus 16 pt padding: {:?}", plain.rect);
    assert!((boxed.rect[3] - plain.rect[3]).abs() < 0.05, "plain matches boxed dimensions");

    // Hovering a plain TextArea draws no hover tint.
    rt.pointer_move(plain.rect[0] + 40.0, plain.rect[1] + plain.rect[3] * 0.5);
    rt.frame(1.0 / 60.0);
    assert_eq!(box_ink(&rt, "plain"), 0, "hovering plain TextArea draws no tint");

    // Clicking padding focuses the plain TextArea without drawing a focus outline, and auto_grow works.
    click(&mut rt, plain.rect[0] + 2.0, plain.rect[1] + 2.0);
    assert!(rt.state.plain.focused, "padding press focuses plain TextArea");
    assert_eq!(box_ink(&rt, "plain"), 0, "focused plain TextArea draws no focus outline");
    for n in 2..=5 {
        key(&mut rt, Key::Enter, Mods { shift: true, ..Mods::default() });
        rt.ime(Ime::Commit(format!("line {n}")));
        rt.frame(1.0 / 60.0);
    }
    settle(&mut rt);
    let plain_grown = find(&rt, "plain");
    assert!(
        (plain_grown.rect[3] - (3.0 * line + 16.0)).abs() < 0.05,
        "plain auto_grow caps at 3 rows: {:?}",
        plain_grown.rect
    );

    // Pressing the outer composer container's padding focuses the plain TextArea inside it.
    let outer = find(&rt, "composer-box");
    click(&mut rt, outer.rect[0] + 2.0, outer.rect[1] + 2.0);
    assert!(rt.state.composer.focused, "outer role(\"textbox\") padding focuses inner plain TextArea");
}

#[test]
fn plain_invalid_disabled_and_forced_colors() {
    struct S {
        valid: TextEdit,
        invalid: TextEdit,
        disabled: TextEdit,
        focused: TextEdit,
        forced_state: TextEdit,
        forced: Option<Color>,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut k = Kit::neutral(true);
        k.invalid_text = false;
        k.forced = s.forced;
        col()
            .padding(12.0)
            .gap(12.0)
            .child(TextArea::new(&s.valid).label("Valid").plain().on_edit(|s: &mut S| &mut s.valid).build(&k).key("valid"))
            .child(
                TextArea::new(&s.invalid)
                    .label("Invalid")
                    .plain()
                    .invalid(true)
                    .state(State::Hover)
                    .on_edit(|s: &mut S| &mut s.invalid)
                    .build(&k)
                    .key("invalid"),
            )
            .child(
                TextArea::new(&s.disabled)
                    .label("Disabled")
                    .plain()
                    .invalid(true)
                    .disabled(true)
                    .on_edit(|s: &mut S| &mut s.disabled)
                    .build(&k)
                    .key("disabled"),
            )
            .child(
                TextArea::new(&s.focused)
                    .label("Focused")
                    .plain()
                    .on_edit(|s: &mut S| &mut s.focused)
                    .build(&k)
                    .key("focused"),
            )
            .child(
                TextArea::new(&s.forced_state)
                    .label("Forced state")
                    .plain()
                    .state(State::Focus)
                    .on_edit(|s: &mut S| &mut s.forced_state)
                    .build(&k)
                    .key("forced-state"),
            )
    }

    let mut focused = TextEdit::new("active");
    focused.focused = true;
    let disabled = TextEdit::new("off");
    let mut rt = Runtime::new(
        S {
            valid: TextEdit::new("ok"),
            invalid: TextEdit::new("bad"),
            disabled,
            focused,
            forced_state: TextEdit::new("state"),
            forced: None,
        },
        view,
        (360.0, 560.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    let k = Kit::neutral(true);

    assert_eq!(find_path(&rt, "valid/input").text_color, Some(k.c.text_base));
    assert_eq!(
        find_path(&rt, "invalid/input").text_color,
        Some(k.c.danger.fg),
        "invalid plain TextArea uses danger ink even when k.invalid_text is false"
    );
    assert_eq!(box_ink(&rt, "invalid"), 0, "invalid plain TextArea draws no border or outline");
    assert_eq!(
        find_path(&rt, "disabled/input").text_color,
        Some(k.c.text_base),
        "disabled plain TextArea does not show danger ink"
    );

    // Disabled plain TextArea dims via k.disabled_dim (opacity 0.5).
    let dis_r = find_path(&rt, "disabled/input").rect;
    let dis_glyph = rt
        .instances
        .iter()
        .find(|i| {
            i.params[1] == rune::render::KIND_GLYPH
                && i.rect[0] >= dis_r[0]
                && i.rect[0] < dis_r[0] + dis_r[2]
                && i.rect[1] >= dis_r[1]
                && i.rect[1] < dis_r[1] + dis_r[3]
        })
        .expect("disabled glyph");
    let mut opacity = vec![1.0];
    for i in rt.instances.iter().take_while(|i| !std::ptr::eq(*i, dis_glyph)) {
        if i.params[1] == rune::render::KIND_LAYER {
            opacity.push(opacity.last().unwrap() * i.fill[3]);
        }
        if i.params[1] == rune::render::KIND_LAYER_END {
            opacity.pop();
        }
    }
    assert!((dis_glyph.fill[3] * opacity.last().unwrap() - 0.5).abs() < 0.01, "disabled plain dims to 0.5");

    // In forced-colors mode, focused plain TextArea paints the 2 pt focus outline, while
    // unfocused and disabled ones do not.
    let highlight = Color { r: 0.2, g: 0.8, b: 1.0, a: 1.0 };
    rt.update(|s, tx| {
        s.forced = Some(highlight);
        tx.instant();
    });
    rt.frame(0.0);
    assert_eq!(box_ink(&rt, "valid"), 0, "unfocused plain has no forced-colors outline");
    assert_eq!(box_ink(&rt, "disabled"), 0, "disabled plain has no forced-colors outline");
    assert!(rt.state.focused.focused, "focused editor stays focused");
    let has_highlight = |key: &str| {
        let r = find(&rt, key).rect;
        rt.instances.iter().any(|i| {
            let (cx, cy) = (i.rect[0] + i.rect[2] * 0.5, i.rect[1] + i.rect[3] * 0.5);
            cx >= r[0]
                && cx <= r[0] + r[2]
                && cy >= r[1]
                && cy <= r[1] + r[3]
                && i.border == [highlight.r, highlight.g, highlight.b, highlight.a]
        })
    };
    assert!(has_highlight("focused"), "focused plain draws forced-colors outline in highlight");
    assert!(has_highlight("forced-state"), "State::Focus plain draws forced-colors outline in highlight");
    assert!(!has_highlight("valid") && !has_highlight("disabled"));
}

#[test]
fn scroll_bar_on_auto_grow_and_max_height_shows_thumb_scrolls_and_keeps_focus() {
    struct S {
        grow: TextEdit,
        capped: TextEdit,
        off_default: TextEdit,
        off_explicit: TextEdit,
        no_scroll: TextEdit,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let k = Kit::neutral(true);
        col()
            .padding(12.0)
            .gap(10.0)
            .child(
                TextArea::new(&s.grow)
                    .auto_grow(1, 3)
                    .scroll_bar(true)
                    .on_edit(|s: &mut S| &mut s.grow)
                    .build(&k)
                    .key("grow"),
            )
            .child(
                TextArea::new(&s.capped)
                    .max_height(80.0)
                    .scroll_bar(true)
                    .on_edit(|s: &mut S| &mut s.capped)
                    .build(&k)
                    .key("capped"),
            )
            .child(
                TextArea::new(&s.off_default)
                    .auto_grow(1, 3)
                    .on_edit(|s: &mut S| &mut s.off_default)
                    .build(&k)
                    .key("off-default"),
            )
            .child(
                TextArea::new(&s.off_explicit)
                    .auto_grow(1, 3)
                    .scroll_bar_styled(ScrollBarStyle::from_kit(&k))
                    .scroll_bar(false)
                    .on_edit(|s: &mut S| &mut s.off_explicit)
                    .build(&k)
                    .key("off-explicit"),
            )
            .child(
                TextArea::new(&s.no_scroll)
                    .rows(2)
                    .scroll_bar(true)
                    .on_edit(|s: &mut S| &mut s.no_scroll)
                    .build(&k)
                    .key("no-scroll"),
            )
    }

    let many = (1..=10).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
    let mut grow = TextEdit::new("line 1");
    grow.focused = true;
    grow.set_caret(2);
    let mut rt = Runtime::new(
        S {
            grow,
            capped: TextEdit::new(&many),
            off_default: TextEdit::new(&many),
            off_explicit: TextEdit::new(&many),
            no_scroll: TextEdit::new(&many),
        },
        view,
        (360.0, 540.0),
        Env::new(true, 1.0),
    );
    rt.set_system_scroll_bars(ScrollBars::Always);
    settle(&mut rt);

    // Default, explicit scroll_bar(false), and non-scrolling TextArea mount no scroll bar.
    assert!(!has_path(&rt, "off-default/scroll-scroll-area/scroll-bar"));
    assert!(!has_path(&rt, "off-explicit/scroll-scroll-area/scroll-bar"));
    assert!(!has_path(&rt, "no-scroll/scroll-scroll-area/scroll-bar"));

    // auto_grow with 1 line does not overflow yet, so thumb has zero height.
    let line = 17.5;
    let one_h = find(&rt, "grow").rect[3];
    assert!((one_h - (line + 16.0)).abs() < 0.05, "auto_grow + scroll_bar starts at 1 row: {one_h}");
    assert_eq!(find_path(&rt, "grow/scroll-scroll-area/scroll-bar/thumb-slot/thumb").rect[3], 0.0);

    // max_height with 10 lines overflows immediately and shows its thumb.
    let capped_thumb = find_path(&rt, "capped/scroll-scroll-area/scroll-bar/thumb-slot/thumb");
    let capped_bar = find_path(&rt, "capped/scroll-scroll-area/scroll-bar");
    assert!(capped_thumb.rect[3] > 0.0, "max_height shows thumb when overflowing: {:?}", capped_thumb.rect);
    assert!((capped_bar.opacity - 1.0).abs() < 0.05, "bar visible under ScrollBars::Always: {}", capped_bar.opacity);

    // Dragging the max_height thumb scrolls its inner editor.
    let before_capped_y = find_path(&rt, "capped/scroll-scroll-area/scroll/input").rect[1];
    let cx = capped_thumb.rect[0] + capped_thumb.rect[2] * 0.5;
    let cy = capped_thumb.rect[1] + capped_thumb.rect[3] * 0.5;
    rt.pointer_move(cx, cy);
    rt.pointer_down(cx, cy);
    rt.pointer_move(cx, cy + 40.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_up(cx, cy + 40.0);
    rt.frame(1.0 / 60.0);
    let after_capped_y = find_path(&rt, "capped/scroll-scroll-area/scroll/input").rect[1];
    assert!(after_capped_y < before_capped_y - 10.0, "dragging max_height thumb scrolls content");

    // Re-focus grow and overflow it past 3 rows: auto_grow still grows, caps at 3 rows, and shows the thumb.
    rt.update(|s, tx| {
        s.grow = TextEdit::new(&many);
        s.grow.focused = true;
        s.grow.set_caret(3);
        tx.instant();
    });
    settle(&mut rt);
    let grown_h = find(&rt, "grow").rect[3];
    assert!((grown_h - (3.0 * line + 16.0)).abs() < 0.05, "auto_grow + scroll_bar caps at 3 rows: {grown_h}");
    let grow_thumb = find_path(&rt, "grow/scroll-scroll-area/scroll-bar/thumb-slot/thumb");
    let grow_bar = find_path(&rt, "grow/scroll-scroll-area/scroll-bar");
    assert!(grow_thumb.rect[3] > 0.0, "auto_grow shows thumb when overflowing: {:?}", grow_thumb.rect);
    assert!((grow_bar.opacity - 1.0).abs() < 0.05);

    // Dragging the auto_grow thumb scrolls without blurring the editor or moving its caret.
    let before_grow_y = find_path(&rt, "grow/scroll-scroll-area/scroll/input").rect[1];
    let gx = grow_thumb.rect[0] + grow_thumb.rect[2] * 0.5;
    let gy = grow_thumb.rect[1] + grow_thumb.rect[3] * 0.5;
    rt.pointer_move(gx, gy);
    rt.pointer_down(gx, gy);
    rt.pointer_move(gx, gy + 30.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_up(gx, gy + 30.0);
    rt.frame(1.0 / 60.0);
    let after_grow_y = find_path(&rt, "grow/scroll-scroll-area/scroll/input").rect[1];
    assert!(after_grow_y < before_grow_y - 10.0, "dragging auto_grow thumb scrolls content");
    assert!(rt.state.grow.focused, "dragging scroll bar thumb must not blur focused TextArea");
    assert_eq!(rt.state.grow.range(), (3, 3), "dragging scroll bar thumb must not move caret");
}

#[test]
fn scroll_bar_styled_custom_style_independent_siblings_and_rtl() {
    struct S {
        left: TextEdit,
        right: TextEdit,
        rtl: TextEdit,
    }
    const CUSTOM_THUMB: Color = Color { r: 0.9, g: 0.25, b: 0.15, a: 1.0 };

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let k = Kit::neutral(true);
        let custom = ScrollBarStyle::from_kit(&k).always_visible().thumb(CUSTOM_THUMB).width(8.0);
        col()
            .padding(12.0)
            .gap(12.0)
            .child(
                row()
                    .gap(12.0)
                    .child(
                        TextArea::new(&s.left)
                            .width(Some(180.0))
                            .auto_grow(1, 3)
                            .scroll_bar_styled(custom)
                            .on_edit(|s: &mut S| &mut s.left)
                            .build(&k)
                            .key("left"),
                    )
                    .child(
                        TextArea::new(&s.right)
                            .width(Some(180.0))
                            .auto_grow(1, 3)
                            .scroll_bar_styled(custom)
                            .on_edit(|s: &mut S| &mut s.right)
                            .build(&k)
                            .key("right"),
                    ),
            )
            .child(
                col().dir(Direction::Rtl).child(
                    TextArea::new(&s.rtl)
                        .width(Some(200.0))
                        .auto_grow(1, 3)
                        .scroll_bar_styled(custom)
                        .on_edit(|s: &mut S| &mut s.rtl)
                        .build(&k)
                        .key("rtl"),
                ),
            )
    }

    let many = (1..=12).map(|i| format!("row {i}")).collect::<Vec<_>>().join("\n");
    let mut rt = Runtime::new(
        S { left: TextEdit::new(&many), right: TextEdit::new(&many), rtl: TextEdit::new(&many) },
        view,
        (420.0, 260.0),
        Env::new(true, 1.0),
    );
    // Even under WhenScrolling, custom .always_visible() keeps the bar shown.
    rt.set_system_scroll_bars(ScrollBars::WhenScrolling);
    settle(&mut rt);

    let left_bar = find_path(&rt, "left/scroll-scroll-area/scroll-bar");
    let left_thumb = find_path(&rt, "left/scroll-scroll-area/scroll-bar/thumb-slot/thumb");
    let right_thumb = find_path(&rt, "right/scroll-scroll-area/scroll-bar/thumb-slot/thumb");
    assert!((left_bar.opacity - 1.0).abs() < 0.05, "always_visible custom style stays visible under WhenScrolling");
    assert!((left_thumb.rect[2] - 8.0).abs() < 0.1, "custom width 8.0 applied: {:?}", left_thumb.rect);
    assert_eq!(left_thumb.fill, CUSTOM_THUMB, "custom thumb colour applied");

    // Dragging left thumb scrolls only left; right thumb stays put.
    let lx = left_thumb.rect[0] + left_thumb.rect[2] * 0.5;
    let ly = left_thumb.rect[1] + left_thumb.rect[3] * 0.5;
    rt.pointer_move(lx, ly);
    rt.pointer_down(lx, ly);
    rt.pointer_move(lx, ly + 35.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_up(lx, ly + 35.0);
    rt.frame(1.0 / 60.0);
    let left_after = find_path(&rt, "left/scroll-scroll-area/scroll-bar/thumb-slot/thumb");
    let right_after = find_path(&rt, "right/scroll-scroll-area/scroll-bar/thumb-slot/thumb");
    assert!(left_after.rect[1] > left_thumb.rect[1] + 5.0, "left scrolled");
    assert!((right_after.rect[1] - right_thumb.rect[1]).abs() < 0.5, "right stayed put");

    // RTL puts the scroll bar on the viewport's left edge.
    let rtl_scroll = find_path(&rt, "rtl/scroll-scroll-area/scroll");
    let rtl_bar = find_path(&rt, "rtl/scroll-scroll-area/scroll-bar");
    assert!(
        (rtl_bar.rect[0] - rtl_scroll.rect[0]).abs() < 4.0,
        "RTL scroll bar sits at left edge of viewport: bar={:?} scroll={:?}",
        rtl_bar.rect,
        rtl_scroll.rect
    );
}

#[test]
fn trailing_reserves_width_wraps_scrolls_and_focuses_independently() {
    struct S {
        commit: TextEdit,
        one_line: TextEdit,
        capped: TextEdit,
        bare: TextEdit,
        commits: usize,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let k = Kit::neutral(true);
        col()
            .w(180.0)
            .gap(12.0)
            .child(
                TextArea::new(&s.commit)
                    .label("Commit message")
                    .width(None)
                    .auto_grow(1, 3)
                    .trailing(
                        Button::new("Commit")
                            .control_size(ControlSize::Small)
                            .on_press(|s: &mut S| s.commits += 1)
                            .build(&k)
                            .key("commit-btn"),
                    )
                    .on_edit(|s: &mut S| &mut s.commit)
                    .build(&k)
                    .key("commit-area"),
            )
            .child(
                TextArea::new(&s.one_line)
                    .label("Summary")
                    .width(None)
                    .auto_grow(1, 1)
                    .trailing(Button::new("Go").control_size(ControlSize::Small).build(&k).key("go-btn"))
                    .on_edit(|s: &mut S| &mut s.one_line)
                    .build(&k)
                    .key("one-line-area"),
            )
            .child(
                TextArea::new(&s.capped)
                    .label("Notes")
                    .width(None)
                    .max_height(84.0)
                    .trailing(Button::new("Save").control_size(ControlSize::Small).build(&k).key("save-btn"))
                    .on_edit(|s: &mut S| &mut s.capped)
                    .build(&k)
                    .key("capped-area"),
            )
            .child(
                TextArea::new(&s.bare)
                    .label("Bare")
                    .width(None)
                    .auto_grow(1, 3)
                    .on_edit(|s: &mut S| &mut s.bare)
                    .build(&k)
                    .key("bare-area"),
            )
    }

    let prose = "This commit message is intentionally long prose repeated many times to verify wrapping and scrolling inside a narrow 180 pt panel. ".repeat(7);
    assert!(prose.len() > 880);
    let mut rt = Runtime::new(
        S {
            commit: TextEdit::new(&prose),
            one_line: TextEdit::new(&prose),
            capped: TextEdit::new(&prose),
            bare: TextEdit::new(&prose),
            commits: 0,
        },
        view,
        (240.0, 400.0),
        Env::new(true, 1.0),
    );
    settle(&mut rt);

    // Default TextArea without trailing has no trailing node and uses full inner width.
    assert!(!has_path(&rt, "bare-area/trailing"));
    let bare = find(&rt, "bare-area");
    let bare_scroll = find_path(&rt, "bare-area/scroll");
    assert!((bare_scroll.rect[2] - (bare.rect[2] - 16.0)).abs() < 0.5, "default TextArea layout unchanged");

    // Trailing button reserves width; viewport wraps clear of it and bounds height.
    let area = find(&rt, "commit-area");
    let scroll = find_path(&rt, "commit-area/scroll");
    let input = find_path(&rt, "commit-area/scroll/input");
    let trailing = find_path(&rt, "commit-area/trailing");
    let btn = find(&rt, "commit-btn");
    let line = 17.5;
    assert!(
        scroll.rect[0] + scroll.rect[2] <= trailing.rect[0] - 7.5,
        "scroll viewport clears trailing button by 8 pt gap: scroll={:?} trailing={:?}",
        scroll.rect,
        trailing.rect
    );
    assert!(
        input.rect[2] <= scroll.rect[2] + 0.5,
        "input wraps inside viewport width: input={:?} scroll={:?}",
        input.rect,
        scroll.rect
    );
    assert!((area.rect[3] - (3.0 * line + 16.0)).abs() < 0.5, "height bounded at 3 rows: {:?}", area.rect);
    assert!(input.rect[3] > scroll.rect[3] + 50.0, "prose overflows and scrolls inside viewport");
    assert!(
        ((trailing.rect[1] + trailing.rect[3]) - (area.rect[1] + area.rect[3] - 8.0)).abs() < 0.5,
        "trailing anchors to bottom-end: trailing={:?} area={:?}",
        trailing.rect,
        area.rect
    );

    // auto_grow(1, 1) and max_height(84.0) both keep height bounded too.
    let one = find(&rt, "one-line-area");
    assert!(one.rect[3] < 60.0, "auto_grow(1, 1) with trailing stays compact: {:?}", one.rect);
    let capped = find(&rt, "capped-area");
    assert!((capped.rect[3] - 84.0).abs() < 1.0, "max_height(84) with trailing stays bounded: {:?}", capped.rect);

    // Clicking padding focuses the editor; Tab moves focus to trailing Commit button; Space activates it;
    // Shift+Tab returns focus to the editor.
    click(&mut rt, area.rect[0] + 2.0, area.rect[1] + 2.0);
    assert!(rt.state.commit.focused, "padding press focuses editor");
    key(&mut rt, Key::Tab, Mods::default());
    assert!(!rt.state.commit.focused, "Tab leaves editor for trailing button");
    key(&mut rt, Key::Text(" ".into()), Mods::default());
    assert_eq!(rt.state.commits, 1, "Space activates focused trailing button");
    key(&mut rt, Key::Tab, Mods { shift: true, ..Mods::default() });
    assert!(rt.state.commit.focused, "Shift+Tab returns focus to editor");

    // Clicking the trailing button directly also activates it.
    click(&mut rt, btn.rect[0] + btn.rect[2] * 0.5, btn.rect[1] + btn.rect[3] * 0.5);
    assert_eq!(rt.state.commits, 2, "clicking trailing button activates it");
}

#[test]
fn trailing_combines_with_plain_scroll_bar_and_rtl() {
    struct S {
        ltr: TextEdit,
        rtl: TextEdit,
        empty: TextEdit,
        sends: usize,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let k = Kit::neutral(true);
        col()
            .w(280.0)
            .gap(12.0)
            .child(
                TextArea::new(&s.ltr)
                    .plain()
                    .width(None)
                    .auto_grow(1, 3)
                    .scroll_bar(true)
                    .trailing(
                        Button::new("Send")
                            .control_size(ControlSize::Small)
                            .on_press(|s: &mut S| s.sends += 1)
                            .build(&k)
                            .key("send-ltr"),
                    )
                    .on_edit(|s: &mut S| &mut s.ltr)
                    .build(&k)
                    .key("ltr"),
            )
            .child(
                col().w_full().dir(Direction::Rtl).child(
                    TextArea::new(&s.rtl)
                        .plain()
                        .width(None)
                        .auto_grow(1, 3)
                        .scroll_bar(true)
                        .trailing(Button::new("إرسال").control_size(ControlSize::Small).build(&k).key("send-rtl"))
                        .on_edit(|s: &mut S| &mut s.rtl)
                        .build(&k)
                        .key("rtl"),
                ),
            )
            .child(
                TextArea::new(&s.empty)
                    .plain()
                    .width(None)
                    .auto_grow(1, 3)
                    .scroll_bar(true)
                    .trailing(Button::new("Send").control_size(ControlSize::Small).disabled(true).build(&k).key("send-off"))
                    .on_edit(|s: &mut S| &mut s.empty)
                    .build(&k)
                    .key("empty"),
            )
    }

    let many = (1..=8).map(|i| format!("message line {i}")).collect::<Vec<_>>().join("\n");
    let mut rt = Runtime::new(
        S { ltr: TextEdit::new(&many), rtl: TextEdit::new(&many), empty: TextEdit::new(""), sends: 0 },
        view,
        (320.0, 320.0),
        Env::new(true, 1.0),
    );
    rt.set_system_scroll_bars(ScrollBars::Always);
    settle(&mut rt);

    // LTR: plain omits chrome, scroll bar stays inside viewport clear of trailing button.
    assert_eq!(box_ink(&rt, "ltr"), 0, "plain + trailing + scroll_bar draws no outer box chrome");
    let ltr_scroll = find_path(&rt, "ltr/scroll-scroll-area/scroll");
    let ltr_bar = find_path(&rt, "ltr/scroll-scroll-area/scroll-bar");
    let ltr_trailing = find_path(&rt, "ltr/trailing");
    assert!(
        ltr_bar.rect[0] + ltr_bar.rect[2] <= ltr_trailing.rect[0] - 7.5,
        "LTR scroll bar stays clear of trailing button: bar={:?} trailing={:?}",
        ltr_bar.rect,
        ltr_trailing.rect
    );
    assert!(
        ltr_bar.rect[0] + ltr_bar.rect[2] <= ltr_scroll.rect[0] + ltr_scroll.rect[2] + 1.0,
        "LTR scroll bar sits at viewport right edge"
    );

    // RTL: trailing sits on the left end and scroll bar sits on viewport's left edge.
    let rtl_scroll = find_path(&rt, "rtl/scroll-scroll-area/scroll");
    let rtl_bar = find_path(&rt, "rtl/scroll-scroll-area/scroll-bar");
    let rtl_trailing = find_path(&rt, "rtl/trailing");
    assert!(
        rtl_trailing.rect[0] + rtl_trailing.rect[2] <= rtl_scroll.rect[0] - 7.5,
        "RTL trailing sits at left inline-end clear of scroll viewport: trailing={:?} scroll={:?}",
        rtl_trailing.rect,
        rtl_scroll.rect
    );
    assert!(
        (rtl_bar.rect[0] - rtl_scroll.rect[0]).abs() < 4.0,
        "RTL scroll bar sits at viewport left edge: bar={:?} scroll={:?}",
        rtl_bar.rect,
        rtl_scroll.rect
    );

    // Empty edge case: no thumb shown when content does not overflow.
    let empty_thumb = find_path(&rt, "empty/scroll-scroll-area/scroll-bar/thumb-slot/thumb");
    assert_eq!(empty_thumb.rect[3], 0.0, "empty TextArea has no scroll bar thumb");
}
