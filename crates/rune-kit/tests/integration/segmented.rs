use rune::{Cx, El, Env, Runtime};
use rune_kit::{Kit, SegmentedControl, Select, SelectAppearance};

fn rect(rt: &Runtime<usize>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}")).rect
}

fn fitted(selected: &usize, _: &mut Cx<usize>) -> El {
    rune::row().child(SegmentedControl::new(&["A", "Much longer label", "日本語"], Some(*selected)).fit().build(&Kit::neutral(true)).key("seg"))
}

#[test]
fn fitted_segments_share_the_widest_labels_width_and_the_indicator_slides_between_them() {
    let mut rt = Runtime::new(0usize, fitted, (600.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let widths: Vec<f32> = (0..3).map(|i| rect(&rt, &format!("segment{i}"))[2]).collect();
    assert!(widths.iter().all(|w| (w - widths[0]).abs() < 0.01), "equal segments: {widths:?}");
    let label = rt.elements().into_iter().filter(|e| e.text.as_deref() == Some("Much longer label")).map(|e| e.rect[2]).next().unwrap();
    assert!(widths[1] >= label + 24.0 - 0.01, "the widest label fits with its padding: {widths:?} vs {label}");
    assert!((rect(&rt, "seg")[2] - widths.iter().sum::<f32>()).abs() < 0.01);
    assert_eq!(rect(&rt, "indicator")[0], rect(&rt, "segment0")[0]);
    rt.state = 2;
    rt.invalidate();
    for _ in 0..120 { rt.frame(1.0 / 60.0); }
    let (indicator, target) = (rect(&rt, "indicator"), rect(&rt, "segment2"));
    assert!((indicator[0] - target[0]).abs() < 0.01 && (indicator[2] - target[2]).abs() < 0.01);
}

fn pickable(selected: &usize, _: &mut Cx<usize>) -> El {
    rune::row().child(
        SegmentedControl::new(&["Left", "Either", "Right"], Some(*selected))
            .fit()
            .on_pick(|s: &mut usize, i, _: &mut rune::Tx| *s = i)
            .build(&Kit::neutral(true))
            .key("seg"),
    )
}

fn hover(rt: &Runtime<usize>, key: &str) -> f32 {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().hover
}

#[test]
fn an_unselected_segment_hovers_on_the_first_frame_and_fades_as_the_pointer_leaves() {
    let mut rt = Runtime::new(1usize, pickable, (600.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let (x, y) = rt.find("segment0").unwrap();
    rt.pointer_move(x, y);
    rt.frame(1.0 / 120.0);
    assert_eq!(hover(&rt, "segment0"), 1.0, "the first frame under the pointer is fully hovered");
    let (x, y) = rt.find("segment1").unwrap();
    rt.pointer_move(x, y);
    rt.frame(1.0 / 120.0);
    let leaving = hover(&rt, "segment0");
    assert!(leaving > 0.0 && leaving < 1.0, "leaving fades: {leaving}");
    assert_eq!(hover(&rt, "segment1"), 0.0, "the selected segment stays as it is");
    rt.frame(0.5);
    assert_eq!(hover(&rt, "segment0"), 0.0);
}

fn field(_: &usize, _: &mut Cx<usize>) -> El {
    rune::row().child(Select::new::<&str>("model", &[]).appearance(SelectAppearance::Field).width(200.0).current(Some("Parakeet")).open(true).trigger(&Kit::neutral(true)))
}

#[test]
fn a_select_trigger_is_the_closed_field_without_a_listbox() {
    let mut rt = Runtime::new(0usize, field, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rect(&rt, "model-trigger")[2], 200.0);
    assert!(rt.elements().iter().all(|e| e.role != "listbox"), "open turns the chevron but builds no listbox");
}

#[test]
fn segments_render_icons_trailing_elements_and_tooltips_with_shortcuts() {
    use rune::el;
    use rune_kit::{Badge, glyphs};

    fn view(selected: &usize, _: &mut Cx<usize>) -> El {
        let k = Kit::neutral(true);
        rune::col()
            .gap(8.0)
            .child(
                SegmentedControl::new(&["Comments", "Checks", ""], Some(*selected))
                    .icons(&[Some(&glyphs::CHECK), None, Some(&glyphs::STAR)])
                    .icon(1, &glyphs::SPARKLE)
                    .trailing(0, Badge::new("3").build(&k).key("count"))
                    .trailing(1, el().key("dot").size(6.0, 6.0).radius(3.0).fill(k.c.danger.fg))
                    .help(0, "Review comments")
                    .shortcut(0, "cmd+1")
                    .shortcut(1, "F2")
                    .help(2, "Starred")
                    .shortcut(2, "cmd+3")
                    .fit()
                    .on_pick(|s: &mut usize, i, _| *s = i)
                    .build(&k)
                    .key("rich"),
            )
            .child(
                SegmentedControl::new(&["Plain", "Default"], Some(0))
                    .build(&k)
                    .key("plain"),
            )
    }

    let mut rt = Runtime::new(0usize, view, (640.0, 160.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let els = rt.elements();
    let find = |suffix: &str| els.iter().find(|e| e.path.ends_with(suffix)).unwrap_or_else(|| panic!("no {suffix}"));
    let has = |suffix: &str| els.iter().any(|e| e.path.ends_with(suffix));

    // Icons rendered at 14pt on all three segments (via icons() and icon() override).
    for seg in ["rich/segment0/icon", "rich/segment1/icon", "rich/segment2/icon"] {
        let ic = find(seg);
        assert!((ic.rect[2] - 14.0).abs() < 0.01 && (ic.rect[3] - 14.0).abs() < 0.01, "{seg}: {:?}", ic.rect);
    }
    // Labels present for non-empty segments and omitted for icon-only segment.
    assert_eq!(find("rich/segment0/label").text.as_deref(), Some("Comments"));
    assert_eq!(find("rich/segment1/label").text.as_deref(), Some("Checks"));
    assert!(!has("rich/segment2/label"), "empty label omits text child for icon-only segment");
    // LTR child order inside segment 0: icon -> label -> trailing badge.
    let icon0 = find("rich/segment0/icon").rect;
    let label0 = find("rich/segment0/label").rect;
    let count0 = find("rich/segment0/count").rect;
    assert!(icon0[0] + icon0[2] < label0[0], "icon before label: {icon0:?} {label0:?}");
    assert!(label0[0] + label0[2] < count0[0], "label before trailing: {label0:?} {count0:?}");
    assert!((label0[0] - (icon0[0] + icon0[2]) - 6.0).abs() < 0.5, "6pt gap after icon");
    assert!((count0[0] - (label0[0] + label0[2]) - 6.0).abs() < 0.5, "6pt gap before trailing");
    assert!(has("rich/segment1/dot"), "trailing dot rendered on segment 1");

    // Tooltip text uses Button/IconButton compose_help formatting.
    let cmd1 = if cfg!(target_os = "macos") { "⌘1" } else { "Super+1" };
    let cmd3 = if cfg!(target_os = "macos") { "⌘3" } else { "Super+3" };
    assert_eq!(find("rich/segment0").help.as_deref(), Some(format!("Review comments  {cmd1}").as_str()));
    assert_eq!(find("rich/segment1").help.as_deref(), Some("Checks  F2"));
    assert_eq!(find("rich/segment2").help.as_deref(), Some(format!("Starred  {cmd3}").as_str()));

    // Default SegmentedControl without new builders stays unchanged.
    assert!(!has("plain/segment0/icon"));
    assert_eq!(find("plain/segment0").help, None);
    assert_eq!(find("plain/segment0").role, "radio");
    assert_eq!(find("plain").role, "radiogroup");
}

#[test]
fn fitted_segments_size_to_widest_icon_label_and_trailing_and_follow_rtl() {
    use rune::{Direction, el};
    use rune_kit::glyphs;

    fn view(rtl: &bool, _: &mut Cx<bool>) -> El {
        let k = Kit::neutral(true);
        rune::row()
            .dir(if *rtl { Direction::Rtl } else { Direction::Ltr })
            .child(
                SegmentedControl::new(&["A", "Comments", ""], Some(0))
                    .icons(&[None, Some(&glyphs::CHECK), Some(&glyphs::STAR)])
                    .trailing(1, el().key("badge").size(22.0, 16.0).fill(k.c.layer_02))
                    .fit()
                    .build(&k)
                    .key("seg"),
            )
    }

    let mut rt = Runtime::new(false, view, (640.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let els = rt.elements();
    let find = |suffix: &str| els.iter().find(|e| e.path.ends_with(suffix)).unwrap_or_else(|| panic!("no {suffix}"));
    let widths: Vec<f32> = (0..3).map(|i| find(&format!("seg/segment{i}")).rect[2]).collect();
    assert!(widths.iter().all(|w| (w - widths[0]).abs() < 0.01), "equal fit widths: {widths:?}");
    let icon1 = find("seg/segment1/icon").rect;
    let label1 = find("seg/segment1/label").rect;
    let badge1 = find("seg/segment1/badge").rect;
    let needed = 24.0 + icon1[2] + 6.0 + label1[2] + 6.0 + badge1[2];
    assert!(widths[1] >= needed - 0.5, "widest segment fits icon + label + trailing + gaps + padding: {widths:?} vs {needed}");

    // RTL flips inline order: trailing on the left, label in the middle, icon on the right.
    let mut rtl_rt = Runtime::new(true, view, (640.0, 100.0), Env::new(true, 1.0));
    rtl_rt.frame(0.0);
    let rtl_els = rtl_rt.elements();
    let rtl_find = |suffix: &str| rtl_els.iter().find(|e| e.path.ends_with(suffix)).unwrap_or_else(|| panic!("no {suffix}"));
    let r_icon = rtl_find("seg/segment1/icon").rect;
    let r_label = rtl_find("seg/segment1/label").rect;
    let r_badge = rtl_find("seg/segment1/badge").rect;
    assert!(r_badge[0] + r_badge[2] < r_label[0], "RTL: trailing before label: {r_badge:?} {r_label:?}");
    assert!(r_label[0] + r_label[2] < r_icon[0], "RTL: label before icon: {r_label:?} {r_icon:?}");
}

#[test]
fn segmented_control_handles_empty_out_of_bounds_disabled_and_huge_labels() {
    use rune::el;
    use rune_kit::{State, glyphs};

    #[derive(Default)]
    struct PickState {
        picked: usize,
        disabled_hits: usize,
    }

    fn view(s: &PickState, _: &mut Cx<PickState>) -> El {
        let k = Kit::neutral(true);
        let huge = "Very long segment label that must truncate cleanly inside a fixed control ".repeat(6);
        rune::col()
            .gap(8.0)
            .child(
                SegmentedControl::new::<&str>(&[], None)
                    .icons(&[Some(&glyphs::CHECK)])
                    .icon(0, &glyphs::STAR)
                    .trailing(0, el().size(6.0, 6.0))
                    .help(0, "Empty")
                    .shortcut(0, "F1")
                    .description(0, "Ignored")
                    .controls(0, "panel")
                    .as_tabs()
                    .build(&k)
                    .key("empty"),
            )
            .child(
                SegmentedControl::new(&[huge.as_str(), "Disabled"], Some(s.picked))
                    .width(Some(220.0))
                    .icons(&[Some(&glyphs::CHECK), Some(&glyphs::STAR), Some(&glyphs::SPARKLE)])
                    .icon(usize::MAX, &glyphs::STAR)
                    .trailing(0, el().key("dot").size(6.0, 6.0).fill(k.c.danger.fg))
                    .trailing(usize::MAX, el().size(6.0, 6.0))
                    .help(1, "Unavailable")
                    .help(usize::MAX, "Ignored")
                    .shortcut(usize::MAX, "F9")
                    .description(usize::MAX, "Ignored")
                    .controls(usize::MAX, "missing")
                    .segment_state(1, State::Disabled)
                    .on_pick(|s: &mut PickState, i, _| {
                        if i == 1 {
                            s.disabled_hits += 1;
                        }
                        s.picked = i;
                    })
                    .build(&k)
                    .key("bounded"),
            )
    }

    let mut rt = Runtime::new(PickState::default(), view, (400.0, 140.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let els = rt.elements();
    let find = |suffix: &str| els.iter().find(|e| e.path.ends_with(suffix)).unwrap_or_else(|| panic!("no {suffix}"));
    assert_eq!(find("empty").role, "tablist");
    let seg0 = find("bounded/segment0").rect;
    let lbl0 = find("bounded/segment0/label").rect;
    assert!(lbl0[2] <= seg0[2], "huge label truncates within fixed segment: {lbl0:?} in {seg0:?}");
    assert_eq!(find("bounded/segment1").help.as_deref(), Some("Unavailable"));

    // Clicking a disabled segment fires no pick handler.
    let seg1 = find("bounded/segment1").rect;
    let (x, y) = (seg1[0] + seg1[2] * 0.5, seg1[1] + seg1[3] * 0.5);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    assert_eq!(rt.state.disabled_hits, 0);
    assert_eq!(rt.state.picked, 0);
}
