//! `El::hover_motion_within` sets the hover curves kit controls' inner elements use.
use rune::{Anim, Color, Cx, Curve, Easing, El, Env, Runtime, col, el, row, text};
use rune_kit::{Kit, Popover, SegmentedControl, Switch, root};

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Default,
    /// Instant on the root.
    Within,
    /// A slow tween on the root; `own` keeps its own instant curves.
    SlowRoot,
    /// Instant on the root, a slow tween around the segmented control and popover.
    Nested,
}

fn slow() -> Anim {
    Curve::tween(0.5, Easing::EASE_OUT).into()
}

fn view(mode: &Mode, _: &mut Cx<Mode>) -> El {
    let k = Kit::neutral(true);
    let mut layer = Vec::new();
    let pop = Popover::new("pop").title("Details").trigger(text("Open")).open(true).build(&k, &mut layer);
    let mut seg = col()
        .gap(8.0)
        .child(SegmentedControl::new(&["One", "Two"], Some(0)).on_pick(|_: &mut Mode, _, _| {}).build(&k))
        .child(pop);
    if *mode == Mode::Nested {
        seg = seg.hover_motion_within(slow(), slow());
    }
    let switch = Switch::new(false).on_toggle(|_: &mut Mode| {}).build(&k);
    let own = k.interactive(el().key("own").size(60.0, 28.0)).hover_motion(Anim::INSTANT, Anim::INSTANT);
    let group = row()
        .key("group")
        .size(60.0, 28.0)
        .hover_group()
        .child(el().key("group-child").size(20.0, 20.0).fill(Color::WHITE).group_opacity(0.0, 1.0));
    layer.push(k.interactive(el().key("layer-chip").absolute(380.0, 20.0).size(60.0, 28.0)));
    let mut body = col().w_full().h_full().gap(12.0).child(seg).child(switch).child(own).child(group);
    body = match mode {
        Mode::Default => body,
        Mode::Within | Mode::Nested => body.hover_motion_within(Anim::INSTANT, Anim::INSTANT),
        Mode::SlowRoot => body.hover_motion_within(slow(), slow()),
    };
    root(body, layer)
}

fn hover(rt: &Runtime<Mode>, key: &str) -> f32 {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().hover
}

/// The hover of `key` on the first frame after the pointer leaves it.
fn after_leaving(mode: Mode, key: &str) -> f32 {
    let mut rt = Runtime::new(mode, view, (500.0, 600.0), Env::new(true, 1.0));
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    let (x, y) = rt.find(key).unwrap();
    rt.pointer_move(x, y);
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
    let hovered = hover(&rt, key);
    assert!(hovered > 0.9, "{key} hovers: {hovered}");
    rt.pointer_move(490.0, 590.0);
    rt.frame(1.0 / 120.0);
    hover(&rt, key)
}

#[test]
fn kit_controls_drop_hover_at_once_under_hover_motion_within() {
    for key in ["segment1", "control", "close", "layer-chip"] {
        assert_eq!(after_leaving(Mode::Within, key), 0.0, "{key}");
    }
}

#[test]
fn without_it_the_kit_controls_still_fade() {
    for key in ["segment1", "control", "close", "layer-chip"] {
        let h = after_leaving(Mode::Default, key);
        assert!(h > 0.0 && h < 1.0, "{key} fades: {h}");
    }
}

#[test]
fn an_elements_own_curves_beat_the_inherited_ones() {
    assert_eq!(after_leaving(Mode::SlowRoot, "own"), 0.0);
    assert!(after_leaving(Mode::SlowRoot, "segment1") > 0.5);
}

#[test]
fn the_nearest_hover_motion_within_wins_including_across_anchors() {
    assert!(after_leaving(Mode::Nested, "segment1") > 0.5);
    assert!(after_leaving(Mode::Nested, "close") > 0.5);
    assert_eq!(after_leaving(Mode::Nested, "control"), 0.0);
    assert_eq!(after_leaving(Mode::Nested, "layer-chip"), 0.0);
}

#[test]
fn instant_hover_motion_within_marks_group_opacity_swaps_as_instant_cuts() {
    let intent_for = |mode| {
        let mut rt = Runtime::new(mode, view, (500.0, 600.0), Env::new(true, 1.0));
        rt.frame(1.0 / 60.0);
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some("group-child")).unwrap().live_intent
    };
    assert_eq!(intent_for(Mode::Default) & (4 | 16), 0);
    assert_eq!(intent_for(Mode::Within) & (4 | 16), 4 | 16);
}
