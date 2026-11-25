use rune::{Cx, El, Env, Runtime, TextEdit, TextStyle, col};
use rune_kit::*;

/// A shadcn/Tailwind-sized kit: 24/28/32/36/40 buttons, a 32 × 18 switch and 32 point fields.
fn tailwind(base: &Kit) -> Kit {
    let mut k = base.clone();
    let b = |height, padding_x| ButtonMetrics { height, padding_x, radius: 8.0, icon: 16.0 };
    k.metrics.button =
        Some(BySize { mini: b(24.0, 8.0), small: b(28.0, 10.0), regular: b(32.0, 10.0), large: b(36.0, 12.0), extra_large: b(40.0, 16.0) });
    let s = SwitchMetrics { track_width: 32.0, track_height: 18.0, knob_width: 14.0, knob_height: 14.0, inset: 2.0 };
    k.metrics.switch = Some(BySize { mini: s, small: s, regular: s, large: s, extra_large: s });
    k.metrics.field = Some(FieldMetrics { height: 32.0, padding_x: 11.0, radius: 10.0, ..FieldMetrics::kit(base) });
    k
}

#[test]
fn metrics_default_to_the_built_in_ones() {
    let k = Kit::neutral(true);
    assert_eq!(k.metrics, Metrics::default());
    assert_eq!(ButtonMetrics::kit(&k).regular, ButtonMetrics { height: 24.0, padding_x: 12.0, radius: k.radius.md, icon: 16.0 });
    assert_eq!(SwitchMetrics::NS_SWITCH.get(ControlSize::Mini).track_width, 36.0);
    assert_eq!(FieldMetrics::kit(&k).height, 28.0);
    assert_eq!(TextAreaMetrics::kit(&k).padding, 8.0);
    assert_eq!(CheckboxColors::kit(&k).fill, k.c.surface_weak);
}

fn view(s: &TextEdit, cx: &mut Cx<TextEdit>) -> El {
    let base = Kit::neutral(cx.system_dark());
    let k = tailwind(&base);
    col()
        .items_start()
        .child(Button::new("Save").build(&base).key("button-default"))
        .child(Button::new("Save").build(&k).key("button"))
        .child(Button::new("Save").control_size(ControlSize::Mini).build(&k).key("button-mini"))
        .child(Switch::new(false).build(&k).key("switch"))
        .child(TextInput::new(s).width(Some(200.0)).build(&base).key("field-default"))
        .child(TextInput::new(s).width(Some(200.0)).build(&k).key("field"))
}

#[test]
fn controls_take_their_metrics_from_the_kit() {
    let mut rt = Runtime::new(TextEdit::new(""), view, (400.0, 600.0), Env { system_dark: false, scale: 2.0 });
    rt.frame(0.0);
    rt.settle_all();
    rt.frame(0.0);
    let els = rt.elements();
    let find = |key: &str| els.iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{key}"));
    let under = |root: &str, key: &str| {
        let path = &find(root).path;
        els.iter()
            .find(|e| {
                e.key.as_deref() == Some(key)
                    && e.path.strip_prefix(path.as_str()).is_some_and(|rest| rest.starts_with('/'))
            })
            .unwrap_or_else(|| panic!("{key} in {root}"))
    };
    assert_eq!(find("button-default").rect[3], 24.0);
    assert_eq!(find("button").rect[3], 32.0);
    assert_eq!(find("button-mini").rect[3], 24.0);
    let (track, knob) = (under("switch", "control"), under("switch", "thumb"));
    assert_eq!((track.rect[2], track.rect[3], track.radius), (32.0, 18.0, 9.0));
    assert_eq!((knob.rect[2], knob.rect[3]), (14.0, 14.0));
    assert_eq!(find("field-default").rect[3], 28.0);
    assert_eq!(find("field").rect[3], 32.0);
    // The text starts `padding_x` in: 8 by default, 11 here.
    let inset = |root: &str| under(root, "input").rect[0] - find(root).rect[0];
    assert_eq!(inset("field-default"), 8.0);
    assert_eq!(inset("field"), 11.0);
}

fn area_view(s: &TextEdit, cx: &mut Cx<TextEdit>) -> El {
    let base = Kit::neutral(cx.system_dark());
    let mut k = base.clone();
    k.metrics.text_area = Some(TextAreaMetrics { style: TextStyle::new(14.0, 400.0).lh(20.0), padding: 12.0 });
    col()
        .items_start()
        .child(TextArea::new(s).rows(3).width(Some(240.0)).build(&base).key("area-default"))
        .child(TextArea::new(s).rows(3).width(Some(240.0)).build(&k).key("area"))
        .child(TextArea::new(s).rows(5).width(Some(240.0)).build(&k).key("area-5"))
}

#[test]
fn text_area_takes_its_type_and_padding_from_the_kit() {
    let mut rt = Runtime::new(TextEdit::new(""), area_view, (400.0, 600.0), Env { system_dark: false, scale: 2.0 });
    rt.frame(0.0);
    let els = rt.elements();
    let find = |key: &str| els.iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{key}"));
    let input = |root: &str| {
        let path = &find(root).path;
        els.iter()
            .find(|e| {
                e.key.as_deref() == Some("input")
                    && e.path.strip_prefix(path.as_str()).is_some_and(|rest| rest.starts_with('/'))
            })
            .unwrap_or_else(|| panic!("input in {root}"))
    };
    let inset = |root: &str| (input(root).rect[0] - find(root).rect[0], input(root).rect[1] - find(root).rect[1]);
    assert_eq!(inset("area-default"), (8.0, 8.0));
    assert_eq!(inset("area"), (12.0, 12.0));
    let style = input("area").text_style.as_ref().expect("the input's resolved style");
    assert_eq!((style.size, style.line_height), (14.0, 20.0));
    // `rows * line + 2 * padding`, at least 80: 3 rows give 84, 5 rows 124.
    assert_eq!(find("area-default").rect[3], 80.0);
    assert_eq!(find("area").rect[3], 3.0 * 20.0 + 2.0 * 12.0);
    assert_eq!(find("area-5").rect[3], 5.0 * 20.0 + 2.0 * 12.0);
}

fn focus_view(s: &TextEdit, cx: &mut Cx<TextEdit>) -> El {
    let base = Kit::neutral(cx.system_dark());
    let mut k = base.clone();
    k.metrics.field = Some(FieldMetrics { focus_width: 3.0, focus_offset: 0.0, ..FieldMetrics::kit(&base) });
    col()
        .gap(40.0)
        .padding(40.0)
        .items_start()
        .child(TextInput::new(s).width(Some(200.0)).state(State::Focus).build(&base).key("ring-default"))
        .child(TextInput::new(s).width(Some(200.0)).state(State::Focus).build(&k).key("ring"))
}

/// The focus outline's rect grows `width + offset` past the box on each side.
#[test]
fn field_focus_outline_follows_the_kit() {
    let mut rt = Runtime::new(TextEdit::new(""), focus_view, (400.0, 400.0), Env::new(false, 1.0));
    rt.frame(0.0);
    rt.settle_all();
    rt.frame(0.0);
    let els = rt.elements();
    let rect = |key: &str| els.iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{key}")).rect;
    let ring = |r: [f32; 4], width: f32, grow: f32| {
        rt.instances.iter().any(|i| {
            i.params[1] == rune::render::KIND_RECT
                && (i.params[0] - width).abs() < 0.01
                && i.fill == [0.0; 4]
                && i.border[3] > 0.0
                && (i.rect[0] - (r[0] - grow)).abs() < 0.5
                && (i.rect[1] - (r[1] - grow)).abs() < 0.5
                && (i.rect[2] - (r[2] + 2.0 * grow)).abs() < 0.5
                && (i.rect[3] - (r[3] + 2.0 * grow)).abs() < 0.5
        })
    };
    assert!(ring(rect("ring-default"), 1.0, 0.0), "the built-in ring sits just inside the box");
    assert!(ring(rect("ring"), 3.0, 3.0), "a 3 point halo outside the box");
    assert!(!ring(rect("ring"), 1.0, 0.0), "no built-in ring on the kit's field");
}
