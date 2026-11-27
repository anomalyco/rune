use rune::{Color, Cx, El, Env, Runtime, TextEdit, TextStyle, col};
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
    assert_eq!(IconButtonMetrics::kit(&k).regular, IconButtonMetrics { size: 24.0, radius: k.radius.md, icon: 16.0 });
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

const RED: Color = Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };

fn switch_view(flat: &bool, cx: &mut Cx<bool>) -> El {
    let mut k = Kit::neutral(cx.system_dark());
    k.elevation.switch_on.clear();
    k.elevation.switch_off.clear();
    k.elevation.elements.clear();
    if *flat {
        k.metrics.switch_look =
            Some(SwitchLook { track_border: 1.0, track_border_off: RED, ..SwitchLook::flat(&k) });
    }
    col()
        .child(Switch::new(false).build(&k).key("off"))
        .child(Switch::new(true).build(&k).key("on"))
        .child(Switch::new(true).web().build(&k).key("web"))
}

/// `SwitchLook::flat` drops the track's sheen and the knob's gradient and rim, and the track
/// takes the kit's border.
#[test]
fn switch_look_turns_off_the_sheen_and_rim_and_borders_the_track() {
    for flat in [false, true] {
        let mut rt = Runtime::new(flat, switch_view, (200.0, 200.0), Env::new(false, 1.0));
        rt.frame(0.0);
        rt.settle_all();
        rt.frame(0.0);
        let gradients = rt.instances.iter().filter(|i| i.params[1] == rune::render::KIND_GRADIENT).count();
        let els = rt.elements();
        let under = |root: &str, key: &str| {
            let path = &els.iter().find(|e| e.key.as_deref() == Some(root)).unwrap().path;
            els.iter()
                .find(|e| e.key.as_deref() == Some(key) && e.path.strip_prefix(path.as_str()).is_some_and(|r| r.starts_with('/')))
                .unwrap_or_else(|| panic!("{key} in {root}"))
                .clone()
        };
        let k = Kit::neutral(false);
        if flat {
            assert_eq!(gradients, 0, "a flat switch paints no gradients");
            assert_eq!(under("off", "control").border_color, Some(RED));
            assert_eq!(under("on", "control").border_color, Some(k.c.bg_accent));
            assert_eq!(under("web", "control").border_color, Some(k.c.bg_accent));
            assert_eq!(under("on", "thumb").border_color, None, "no knob rim");
        } else {
            assert!(gradients >= 6, "the built-in switch paints its sheen and knob depth: {gradients}");
            assert_eq!(under("off", "control").border_color, None);
            assert!(under("on", "thumb").border_color.is_some(), "the knob's rim");
        }
    }
}

fn fnv_bytes(h: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *h ^= u64::from(b);
        *h = h.wrapping_mul(0x1000_0000_01b3);
    }
}

fn fnv_f32(h: &mut u64, v: f32) {
    let bits = if v == 0.0 { 0u32 } else { v.to_bits() };
    fnv_bytes(h, &bits.to_le_bytes());
}

fn fnv_color(h: &mut u64, c: Color) {
    for ch in [c.r, c.g, c.b, c.a] {
        fnv_f32(h, ch);
    }
}

fn frame_signature<S: 'static>(rt: &Runtime<S>) -> (u64, u64, usize, usize) {
    let mut lh: u64 = 0xcbf2_9ce4_8422_2325;
    let els = rt.elements();
    for e in &els {
        fnv_bytes(&mut lh, e.path.as_bytes());
        fnv_bytes(&mut lh, &[0]);
        if let Some(k) = &e.key {
            fnv_bytes(&mut lh, k.as_bytes());
        }
        fnv_bytes(&mut lh, &[0]);
        fnv_bytes(&mut lh, e.role.as_bytes());
        for v in e.rect { fnv_f32(&mut lh, v); }
        fnv_f32(&mut lh, e.radius);
        fnv_color(&mut lh, e.fill);
        match e.border_color {
            Some(c) => { fnv_bytes(&mut lh, &[1]); fnv_color(&mut lh, c); }
            None => fnv_bytes(&mut lh, &[0]),
        }
        for v in e.inset { fnv_f32(&mut lh, v); }
    }
    let mut ph: u64 = 0xcbf2_9ce4_8422_2325;
    for i in &rt.instances {
        for v in i.rect { fnv_f32(&mut ph, v); }
        for v in i.radii { fnv_f32(&mut ph, v); }
        for v in i.fill { fnv_f32(&mut ph, v); }
        for v in i.border { fnv_f32(&mut ph, v); }
        for v in i.params { fnv_f32(&mut ph, v); }
        for v in i.uv { fnv_f32(&mut ph, v); }
        for v in i.clip { fnv_f32(&mut ph, v); }
        for v in i.fade { fnv_bytes(&mut ph, &v.to_le_bytes()); }
    }
    (lh, ph, els.len(), rt.instances.len())
}

fn explicit_default_kit(base: &Kit) -> Kit {
    let mut k = base.clone();
    k.metrics.segmented = Some(SegmentedMetrics { height: 28.0, padding_x: 12.0, radius: 6.0, inset: 0.0, gap: 0.0 });
    k.metrics.disclosure_header_height = Some(32.0);
    k.metrics.progress = Some(ProgressMetrics { height: 8.0, border_width: 1.0 });
    k.metrics.divider_thickness = Some(1.0);
    k.metrics.keybind = Some(KeybindMetrics { height: 14.0, padding_x: 4.0, radius: 2.0 });
    k.metrics.text_area_min_height = Some(80.0);
    k.metrics.standard_switch = Some(StandardSwitchMetrics { track_width: 28.0, track_height: 16.0, thumb: 14.0, inset: 1.0 });
    k.metrics.switch_look = Some(SwitchLook {
        sheen: true,
        knob_depth: true,
        track_border: 0.0,
        track_border_off: base.c.border_base,
        track_border_on: base.c.bg_accent,
    });
    k
}

fn all_controls_scene(k: &Kit, edit: &TextEdit) -> El {
    use rune::{row, text};
    col()
        .gap(8.0)
        .w(360.0)
        .child(SegmentedControl::new(&["List", "Grid", "Board"], Some(1)).build(k).key("seg-fixed"))
        .child(SegmentedControl::new(&["One", "Two"], Some(0)).fit().build(k).key("seg-fit"))
        .child(SegmentedControl::new(&["A", "B"], Some(1)).width(None).build(k).key("seg-fill"))
        .child(Collapsible::new("Details", true).child(text("Body")).build(k).key("col-open"))
        .child(Collapsible::new("Closed", false).build(k).key("col-closed"))
        .child(
            Accordion::new()
                .item(AccordionItem::new("First").expanded(true).child(text("Body")))
                .item(AccordionItem::new("Second").expanded(false))
                .build(k)
                .key("acc"),
        )
        .child(Progress::new(Some(0.6)).label("Upload").show_value().build(k).key("prog"))
        .child(Divider::horizontal().build(k).key("div-h"))
        .child(row().h(20.0).child(Divider::vertical().build(k).key("div-v")))
        .child(Keybind::new(&["⌘", "Shift", "K"]).build(k).key("kb"))
        .child(TextArea::new(edit).rows(2).width(Some(240.0)).build(k).key("area-plain"))
        .child(TextArea::new(edit).rows(2).max_height(140.0).width(Some(240.0)).build(k).key("area-max"))
        .child(Switch::new(false).label("Off").build(k).key("sw-off"))
        .child(Switch::new(true).label("On").build(k).key("sw-on"))
        .child(Switch::new(true).web().label("Web").build(k).key("sw-web"))
        .child(Switch::new(false).standard().label("Std off").build(k).key("sw-std-off"))
        .child(Switch::new(true).standard().label("Std on").build(k).key("sw-std-on"))
        .child(Switch::new(true).standard().label("Std press").state(State::Pressed).build(k).key("sw-std-press"))
}

/// Default `Metrics::default()` and explicit `::kit(&k)` values produce bit-identical layout
/// rects and paint instances for every theme in the workspace (light and dark).
#[test]
fn default_metrics_match_explicit_kit_defaults_bit_for_bit() {
    struct StateKit {
        kit: Kit,
        edit: TextEdit,
    }
    fn view(s: &StateKit, _: &mut Cx<StateKit>) -> El {
        all_controls_scene(&s.kit, &s.edit)
    }
    for (name, base) in [
        ("neutral-light", Kit::neutral(false)),
        ("neutral-dark", Kit::neutral(true)),
        ("opencode-light", rune_theme_opencode::kit(false).clone()),
        ("opencode-dark", rune_theme_opencode::kit(true).clone()),
    ] {
        assert_eq!(SegmentedMetrics::kit(&base), SegmentedMetrics { height: 28.0, padding_x: 12.0, radius: 6.0, inset: 0.0, gap: 0.0 });
        assert_eq!(ProgressMetrics::kit(&base), ProgressMetrics { height: 8.0, border_width: 1.0 });
        assert_eq!(KeybindMetrics::kit(&base), KeybindMetrics { height: 14.0, padding_x: 4.0, radius: 2.0 });
        assert_eq!(StandardSwitchMetrics::kit(&base), StandardSwitchMetrics { track_width: 28.0, track_height: 16.0, thumb: 14.0, inset: 1.0 });
        assert_eq!(
            SwitchLook::kit(&base),
            SwitchLook { sheen: true, knob_depth: true, track_border: 0.0, track_border_off: base.c.border_base, track_border_on: base.c.bg_accent }
        );

        let mut rt_def = Runtime::new(StateKit { kit: base.clone(), edit: TextEdit::new("") }, view, (420.0, 900.0), Env::new(base.dark, 1.0));
        rt_def.frame(0.0);
        rt_def.settle_all();
        rt_def.frame(0.0);

        // Pin literal rendered dimensions on the default scene so changing X::kit(&k) cannot
        // silently move both sides together.
        let els = rt_def.elements();
        let find = |suffix: &str| els.iter().find(|e| e.path.ends_with(suffix)).unwrap_or_else(|| panic!("{name}: no {suffix}"));
        assert_eq!((find("/seg-fixed").rect[3], find("/seg-fixed").radius), (28.0, 6.0), "{name}: SegmentedControl");
        assert_eq!(find("/seg-fixed/segment0").inset, [12.0, 0.0, 12.0, 0.0], "{name}: SegmentedControl padding_x");
        assert_eq!((find("/col-open/trigger").rect[3], find("/col-open/trigger").radius), (32.0, 4.0), "{name}: Collapsible");
        assert_eq!(find("/acc/item0/trigger").rect[3], 32.0, "{name}: Accordion");
        assert_eq!(find("/prog/track").rect[3], 8.0, "{name}: Progress height");
        assert!(find("/prog/track").border_color.is_some(), "{name}: Progress 1pt rim");
        assert_eq!(find("/div-h").rect[3], 1.0, "{name}: horizontal Divider");
        assert_eq!(find("/div-v").rect[2], 1.0, "{name}: vertical Divider");
        assert_eq!((find("/kb/key0").rect[3], find("/kb/key0").radius), (14.0, 2.0), "{name}: Keybind");
        assert_eq!(find("/kb/key0").inset, [4.0, 0.0, 4.0, 0.0], "{name}: Keybind padding_x");
        assert_eq!(find("/area-plain").rect[3], 80.0, "{name}: TextArea min height");
        assert_eq!((find("/sw-std-on/control").rect[2], find("/sw-std-on/control").rect[3]), (28.0, 16.0), "{name}: standard Switch track");
        let std_track = find("/sw-std-on/control").rect;
        let std_thumb = find("/sw-std-on/control/thumb").rect;
        assert_eq!((std_thumb[2], std_thumb[3]), (14.0, 14.0), "{name}: standard Switch thumb");
        assert_eq!((std_thumb[0] - std_track[0], std_thumb[1] - std_track[1]), (13.0, 1.0), "{name}: standard Switch inset");

        let mut rt_exp = Runtime::new(StateKit { kit: explicit_default_kit(&base), edit: TextEdit::new("") }, view, (420.0, 900.0), Env::new(base.dark, 1.0));
        rt_exp.frame(0.0);
        rt_exp.settle_all();
        rt_exp.frame(0.0);

        assert_eq!(frame_signature(&rt_def), frame_signature(&rt_exp), "{name}: default vs literal metrics drifted");
    }
}

/// Changing `k.radius.{xs,sm,md}` flows automatically into Keybind, Collapsible trigger and
/// SegmentedControl (and its focus ring) without setting individual metrics overrides.
#[test]
fn radius_scale_flows_into_segmented_collapsible_and_keybind() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let mut k = Kit::neutral(true);
        k.radius.xs = 5.0;
        k.radius.sm = 7.0;
        k.radius.md = 10.0;
        col()
            .gap(12.0)
            .padding(16.0)
            .w(320.0)
            .child(SegmentedControl::new(&["List", "Grid"], Some(0)).segment_state(0, State::Focus).build(&k).key("seg"))
            .child(Collapsible::new("Details", false).build(&k).key("col"))
            .child(Keybind::new(&["⌘", "K"]).build(&k).key("kb"))
    }
    let mut rt = Runtime::new((), view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let els = rt.elements();
    let find = |suffix: &str| els.iter().find(|e| e.path.ends_with(suffix)).unwrap_or_else(|| panic!("no {suffix}"));
    assert_eq!(find("/seg").radius, 10.0);
    assert_eq!(find("/seg/indicator").radius, 10.0);
    assert_eq!(find("/seg/segment0").radius, 10.0);
    assert_eq!(find("/col/trigger").radius, 7.0);
    assert_eq!(find("/kb/key0").radius, 5.0);
    assert_eq!(find("/kb/key1").radius, 5.0);

    // Focused segment draws its 2pt outline at offset -2pt (grow = 0, so rect and radius match seg0).
    let seg0 = find("/seg/segment0").rect;
    let has_ring = rt.instances.iter().any(|i| {
        i.params[1] == rune::render::KIND_RECT
            && (i.params[0] - 2.0).abs() < 0.01
            && i.fill == [0.0; 4]
            && i.border[3] > 0.0
            && (i.rect[0] - seg0[0]).abs() < 0.5
            && (i.rect[3] - seg0[3]).abs() < 0.5
            && (i.radii[0] - 10.0).abs() < 0.5
    });
    assert!(has_ring, "focus ring should follow k.radius.md = 10.0");
}

#[test]
fn dense_and_roomy_metrics_drive_geometry_hit_areas_focus_rings_and_centering() {
    use rune::{Key, row, text};
    struct App {
        picked: usize,
        col_open: bool,
        acc_open: bool,
        edit: TextEdit,
        roomy: bool,
    }
    fn kit_for(roomy: bool) -> Kit {
        let mut k = Kit::neutral(true);
        if roomy {
            k.metrics.segmented = Some(SegmentedMetrics { height: 44.0, padding_x: 20.0, radius: 12.0, inset: 0.0, gap: 0.0 });
            k.metrics.disclosure_header_height = Some(44.0);
            let mut p = ProgressMetrics::kit(&k);
            p.height = 14.0;
            p.border_width = 2.0;
            k.metrics.progress = Some(p);
            k.metrics.divider_thickness = Some(3.0);
            k.metrics.keybind = Some(KeybindMetrics { height: 22.0, padding_x: 8.0, radius: 6.0 });
            k.metrics.text_area_min_height = Some(120.0);
            k.metrics.standard_switch = Some(StandardSwitchMetrics { track_width: 36.0, track_height: 20.0, thumb: 16.0, inset: 2.0 });
        } else {
            k.metrics.segmented = Some(SegmentedMetrics { height: 22.0, padding_x: 6.0, radius: 3.0, inset: 0.0, gap: 0.0 });
            k.metrics.disclosure_header_height = Some(24.0);
            let mut p = ProgressMetrics::kit(&k);
            p.height = 4.0;
            p.border_width = 0.0;
            k.metrics.progress = Some(p);
            k.metrics.divider_thickness = Some(0.5);
            k.metrics.keybind = Some(KeybindMetrics { height: 12.0, padding_x: 2.0, radius: 1.0 });
            k.metrics.text_area_min_height = Some(40.0);
            k.metrics.standard_switch = Some(StandardSwitchMetrics { track_width: 18.0, track_height: 16.0, thumb: 14.0, inset: 1.0 });
        }
        k
    }
    fn view(s: &App, _: &mut Cx<App>) -> El {
        let k = kit_for(s.roomy);
        col()
            .padding(16.0)
            .gap(10.0)
            .w(360.0)
            .child(
                SegmentedControl::new(&["List", "Grid"], Some(s.picked))
                    .fit()
                    .segment_state(0, State::Focus)
                    .on_pick(|s: &mut App, i, _| s.picked = i)
                    .build(&k)
                    .key("seg"),
            )
            .child(Collapsible::new("Details", s.col_open).on_toggle(|s: &mut App| s.col_open = !s.col_open).child(text("Body")).build(&k).key("col"))
            .child(
                Accordion::new()
                    .item(AccordionItem::new("First").expanded(s.acc_open).on_toggle(|s: &mut App| s.acc_open = !s.acc_open).child(text("Body")))
                    .build(&k)
                    .key("acc"),
            )
            .child(Progress::new(Some(0.5)).build(&k).key("prog"))
            .child(Divider::horizontal().build(&k).key("div-h"))
            .child(row().h(24.0).child(Divider::vertical().build(&k).key("div-v")))
            .child(Keybind::new(&["K", "Enter"]).build(&k).key("kb"))
            .child(TextArea::new(&s.edit).rows(1).width(Some(240.0)).build(&k).key("area-plain"))
            .child(TextArea::new(&s.edit).rows(1).max_height(200.0).width(Some(240.0)).build(&k).key("area-max"))
            .child(Switch::new(false).standard().label("Std off").build(&k).key("std-off"))
            .child(Switch::new(true).standard().label("Std on").build(&k).key("std-on"))
            .child(Switch::new(false).standard().label("Std off press").state(State::Pressed).build(&k).key("std-off-p"))
            .child(Switch::new(true).standard().label("Std on press").state(State::Pressed).build(&k).key("std-on-p"))
    }

    // Roomy theme: geometry, centering, hit areas over full height, focus ring and keyboard nav.
    let mut rt = Runtime::new(
        App { picked: 0, col_open: false, acc_open: false, edit: TextEdit::new(""), roomy: true },
        view,
        (420.0, 900.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    let els = rt.elements();
    let find = |suffix: &str| els.iter().find(|e| e.path.ends_with(suffix)).unwrap_or_else(|| panic!("no {suffix}"));

    let seg = find("/seg");
    let seg0 = find("/seg/segment0");
    let seg1 = find("/seg/segment1");
    let seg0_label = find("/seg/segment0/label");
    assert_eq!((seg.rect[3], seg.radius), (44.0, 12.0));
    assert_eq!((seg0.rect[3], seg0.radius), (44.0, 12.0));
    assert!(seg0.rect[2] >= seg0_label.rect[2] + 40.0 - 0.5, "20pt side padding on each side");
    let seg_cy = seg0.rect[1] + seg0.rect[3] * 0.5;
    let lbl_cy = seg0_label.rect[1] + seg0_label.rect[3] * 0.5;
    assert!((seg_cy - lbl_cy).abs() < 1.0, "segment label vertically centered: {seg_cy} vs {lbl_cy}");

    // Focus ring (width 2, offset -2 => grow 0) matches the 12pt radius and 44pt height.
    let focus_ring = rt.instances.iter().any(|i| {
        i.params[1] == rune::render::KIND_RECT
            && (i.params[0] - 2.0).abs() < 0.01
            && i.fill == [0.0; 4]
            && i.border[3] > 0.0
            && (i.rect[3] - 44.0).abs() < 0.5
            && (i.radii[0] - 12.0).abs() < 0.5
    });
    assert!(focus_ring, "focus ring radius follows SegmentedMetrics::radius");

    // Collapsible & Accordion headers: 44pt high, vertically centered titles.
    let col_trig = find("/col/trigger");
    let col_title = els.iter().find(|e| e.path.contains("/col/trigger/") && e.key.as_deref() == Some("title")).unwrap();
    assert_eq!(col_trig.rect[3], 44.0);
    assert!(((col_trig.rect[1] + 22.0) - (col_title.rect[1] + col_title.rect[3] * 0.5)).abs() < 1.0);
    let acc_trig = find("/acc/item0/trigger");
    let acc_title = find("/acc/item0/trigger/title");
    assert_eq!(acc_trig.rect[3], 44.0);
    assert!(((acc_trig.rect[1] + 22.0) - (acc_title.rect[1] + acc_title.rect[3] * 0.5)).abs() < 1.0);

    // Progress: 14pt high with 2pt border.
    let prog_track = find("/prog/track");
    assert_eq!(prog_track.rect[3], 14.0);
    assert!(prog_track.border_color.is_some());
    assert!(rt.instances.iter().any(|i| {
        i.params[1] == rune::render::KIND_RECT
            && (i.params[0] - 2.0).abs() < 0.01
            && (i.rect[3] - 14.0).abs() < 0.5
            && i.border[3] > 0.0
    }));

    // Divider: 3pt thick horizontal and vertical.
    assert_eq!(find("/div-h").rect[3], 3.0);
    assert_eq!(find("/div-v").rect[2], 3.0);

    // Keybind: 22pt high, min_w 22pt, radius 6pt, 8pt sides.
    let kb0 = find("/kb/key0");
    let kb1 = find("/kb/key1");
    assert_eq!((kb0.rect[3], kb0.radius), (22.0, 6.0));
    assert!(kb0.rect[2] >= 22.0 - 0.01);
    assert_eq!((kb1.rect[3], kb1.radius), (22.0, 6.0));

    // TextArea min height: 120pt for both plain and max_height branches.
    assert_eq!(find("/area-plain").rect[3], 120.0);
    assert_eq!(find("/area-max").rect[3], 120.0);

    // Standard Switch: 36x20 track, 16pt thumb, 2pt inset.
    let std_on_track = find("/std-on/control");
    let std_on_thumb = find("/std-on/control/thumb");
    assert_eq!((std_on_track.rect[2], std_on_track.rect[3], std_on_track.radius), (36.0, 20.0, 10.0));
    assert_eq!((std_on_thumb.rect[2], std_on_thumb.rect[3], std_on_thumb.radius), (16.0, 16.0, 8.0));
    assert_eq!(std_on_thumb.rect[0] - std_on_track.rect[0], 18.0);
    assert_eq!(std_on_thumb.rect[1] - std_on_track.rect[1], 2.0);
    let std_off_track = find("/std-off/control");
    let std_off_thumb = find("/std-off/control/thumb");
    assert_eq!((std_off_thumb.rect[2], std_off_thumb.rect[3], std_off_thumb.radius), (18.0, 18.0, 9.0));
    assert_eq!(std_off_thumb.rect[0] - std_off_track.rect[0], 1.0);
    assert_eq!(std_off_thumb.rect[1] - std_off_track.rect[1], 1.0);

    // Hit area test: clicking near the bottom edge (y + 41pt) of the 44pt segment, collapsible,
    // and accordion triggers hits them.
    let (s1x, s1y) = (seg1.rect[0] + seg1.rect[2] * 0.5, seg1.rect[1] + 41.0);
    rt.pointer_move(s1x, s1y);
    rt.pointer_down(s1x, s1y);
    rt.pointer_up(s1x, s1y);
    rt.frame(0.0);
    assert_eq!(rt.state.picked, 1, "clicking bottom of 44pt segment must pick it");

    // Keyboard navigation on SegmentedControl with custom metrics.
    rt.key(Key::Left.into());
    rt.frame(0.0);
    assert_eq!(rt.state.picked, 0, "Left arrow moves choice back to segment 0");

    let (cx, cy) = (col_trig.rect[0] + 20.0, col_trig.rect[1] + 41.0);
    rt.pointer_move(cx, cy);
    rt.pointer_down(cx, cy);
    rt.pointer_up(cx, cy);
    rt.frame(0.0);
    assert!(rt.state.col_open, "clicking bottom of 44pt collapsible trigger must toggle it");

    // Re-read accordion rect after collapsible expanded.
    let acc_r = rt.elements().into_iter().find(|e| e.path.ends_with("/acc/item0/trigger")).unwrap().rect;
    let (ax, ay) = (acc_r[0] + 20.0, acc_r[1] + 41.0);
    rt.pointer_move(ax, ay);
    rt.pointer_down(ax, ay);
    rt.pointer_up(ax, ay);
    rt.frame(0.0);
    assert!(rt.state.acc_open, "clicking bottom of 44pt accordion trigger must toggle it");

    // Dense theme: borderless Progress, tight standard switch pressed stretch clamping, compact TextArea.
    let mut rt_d = Runtime::new(
        App { picked: 0, col_open: false, acc_open: false, edit: TextEdit::new(""), roomy: false },
        view,
        (420.0, 900.0),
        Env::new(true, 1.0),
    );
    rt_d.frame(0.0);
    let els_d = rt_d.elements();
    let find_d = |suffix: &str| els_d.iter().find(|e| e.path.ends_with(suffix)).unwrap_or_else(|| panic!("no {suffix}"));
    assert_eq!((find_d("/seg").rect[3], find_d("/seg").radius), (22.0, 3.0));
    assert_eq!(find_d("/col/trigger").rect[3], 24.0);
    assert_eq!(find_d("/acc/item0/trigger").rect[3], 24.0);
    let acc_d_trig = find_d("/acc/item0/trigger").rect;
    let acc_d_title = find_d("/acc/item0/trigger/title").rect;
    assert!(((acc_d_trig[1] + acc_d_trig[3] * 0.5) - (acc_d_title[1] + acc_d_title[3] * 0.5)).abs() < 1.0);
    assert_eq!(find_d("/prog/track").rect[3], 4.0);
    assert_eq!(find_d("/prog/track").border_color, None, "border_width 0.0 omits Progress border");
    assert_eq!(find_d("/div-h").rect[3], 0.5);
    assert_eq!(find_d("/div-v").rect[2], 0.5);
    assert_eq!((find_d("/kb/key0").rect[3], find_d("/kb/key0").radius), (12.0, 1.0));
    assert_eq!(find_d("/area-plain").rect[3], 40.0);
    assert_eq!(find_d("/area-max").rect[3], 40.0);

    // Tight 18pt standard switch track: pressed thumb stretch clamps inside track bounds.
    for prefix in ["/std-off-p/control", "/std-on-p/control"] {
        let track = find_d(prefix).rect;
        let thumb = find_d(&format!("{prefix}/thumb")).rect;
        assert!(thumb[0] >= track[0] - 0.01, "{prefix}: thumb left {} < track left {}", thumb[0], track[0]);
        assert!(
            thumb[0] + thumb[2] <= track[0] + track[2] + 0.01,
            "{prefix}: thumb right {} > track right {}",
            thumb[0] + thumb[2],
            track[0] + track[2],
        );
    }
}

/// #2363 completion: `Switch::flat()` and `SwitchLook::flat` paint plain fills with no
/// gradients or knob rim; `Switch::flat()` also omits track/knob shadows on raised kits;
/// transparent track border colour omits the border; default raised look stays unchanged.
#[test]
fn flat_switch_omits_sheen_rim_and_shadows_and_handles_transparent_track_border() {
    use rune::render::{KIND_GRADIENT, KIND_INSET, KIND_RECT, KIND_SHADOW};

    struct SwCfg {
        kit: Kit,
        checked: bool,
        web: bool,
        instance_flat: bool,
    }
    fn view(s: &SwCfg, _: &mut Cx<SwCfg>) -> El {
        let mut sw = Switch::new(s.checked).label("Toggle").hide_label();
        if s.web {
            sw = sw.web();
        }
        if s.instance_flat {
            sw = sw.flat();
        }
        col().padding(20.0).child(sw.build(&s.kit).key("sw"))
    }
    let render = |kit: Kit, checked: bool, web: bool, instance_flat: bool| {
        let mut rt = Runtime::new(SwCfg { kit, checked, web, instance_flat }, view, (160.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.settle_all();
        rt.frame(0.0);
        rt
    };

    let base = rune_theme_opencode::kit(true).clone();

    // Default raised switch still paints gradients, knob rim and shadows.
    for web in [false, true] {
        let rt = render(base.clone(), true, web, false);
        assert!(rt.instances.iter().any(|i| i.params[1] == KIND_GRADIENT), "default web={web} draws gradients");
        assert!(rt.instances.iter().any(|i| i.params[1] == KIND_SHADOW || i.params[1] == KIND_INSET), "default web={web} draws shadows");
        assert!(rt.instances.iter().any(|i| i.params[0] > 0.0 && i.border[3] > 0.0), "default web={web} draws knob rim");
    }

    // Flat via theme SwitchLook (with elevation cleared) AND via per-instance Switch::flat() on
    // the raised kit: 0 gradients, 0 shadows, 0 borders, exactly 2 rects (track + knob).
    let mut flat_kit = base.clone();
    flat_kit.elevation.switch_off.clear();
    flat_kit.elevation.switch_on.clear();
    flat_kit.elevation.elements.clear();
    flat_kit.metrics.switch_look = Some(SwitchLook::flat(&base));
    for (label, kit, inst_flat) in [("theme-flat", flat_kit.clone(), false), ("instance-flat", base.clone(), true)] {
        for web in [false, true] {
            for checked in [false, true] {
                let rt = render(kit.clone(), checked, web, inst_flat);
                assert!(!rt.instances.iter().any(|i| i.params[1] == KIND_GRADIENT), "{label} web={web} checked={checked}: no gradients");
                assert!(!rt.instances.iter().any(|i| i.params[1] == KIND_SHADOW || i.params[1] == KIND_INSET), "{label} web={web} checked={checked}: no shadows");
                let rects: Vec<_> = rt.instances.iter().filter(|i| i.params[1] == KIND_RECT).collect();
                assert_eq!(rects.len(), 2, "{label} web={web} checked={checked}: exactly track + knob fills");
                assert!(rects.iter().all(|i| i.params[0] == 0.0), "{label} web={web} checked={checked}: no rim or border");
            }
        }
    }

    // Track border: off colour drawn when off, transparent on colour omits the border when on.
    let green = Color { r: 0.0, g: 1.0, b: 0.0, a: 1.0 };
    let mut bordered_off_only = flat_kit;
    bordered_off_only.metrics.switch_look = Some(SwitchLook {
        track_border: 2.0,
        track_border_off: RED,
        track_border_on: Color::TRANSPARENT,
        ..SwitchLook::flat(&base)
    });
    let rt_off = render(bordered_off_only.clone(), false, false, false);
    let off_ctrl = rt_off.elements().into_iter().find(|e| e.path.ends_with("/control")).unwrap();
    assert_eq!(off_ctrl.border_color, Some(RED));
    let off_rects: Vec<_> = rt_off.instances.iter().filter(|i| i.params[1] == KIND_RECT).collect();
    assert_eq!(off_rects.len(), 2);
    assert!((off_rects[0].params[0] - 2.0).abs() < 0.01, "off track border width is 2pt");
    assert!((off_rects[0].border[0] - 1.0).abs() < 0.02 && off_rects[0].border[3] > 0.99);
    assert_eq!(off_rects[1].params[0], 0.0, "flat knob still has no rim");

    let rt_on = render(bordered_off_only, true, false, false);
    let on_ctrl = rt_on.elements().into_iter().find(|e| e.path.ends_with("/control")).unwrap();
    assert_eq!(on_ctrl.border_color, None, "transparent track_border_on omits border");
    let on_rects: Vec<_> = rt_on.instances.iter().filter(|i| i.params[1] == KIND_RECT).collect();
    assert_eq!(on_rects.len(), 2);
    assert_eq!(on_rects[0].params[0], 0.0, "transparent track_border_on emits no border");

    // Non-flat switch can also take a track border while keeping its sheen and shadows.
    let mut raised_bordered = base.clone();
    raised_bordered.metrics.switch_look = Some(SwitchLook {
        track_border: 1.0,
        track_border_off: RED,
        track_border_on: green,
        ..SwitchLook::kit(&base)
    });
    let rt_rb = render(raised_bordered, true, false, false);
    let rb_ctrl = rt_rb.elements().into_iter().find(|e| e.path.ends_with("/control")).unwrap();
    assert_eq!(rb_ctrl.border_color, Some(green));
    assert!(rt_rb.instances.iter().any(|i| i.params[1] == KIND_GRADIENT));
}

/// Flat compact Switch animates its knob spring and press squeeze with the exact same per-frame
/// geometry as the default raised Switch, and toggles by pointer and keyboard.
#[test]
fn flat_switch_preserves_toggle_and_press_motion_and_keyboard() {
    use rune::Key;
    struct App {
        on: bool,
        pressed: bool,
        flat: bool,
    }
    fn view(s: &App, _: &mut Cx<App>) -> El {
        let mut sw = Switch::new(s.on).label("Sync").on_toggle(|s: &mut App| s.on = !s.on);
        if s.pressed {
            sw = sw.state(State::Pressed);
        }
        if s.flat {
            sw = sw.flat();
        }
        col().padding(20.0).child(sw.build(rune_theme_opencode::kit(true)).key("sw"))
    }
    let mut rt_raised = Runtime::new(App { on: false, pressed: false, flat: false }, view, (200.0, 100.0), Env::new(true, 1.0));
    let mut rt_flat = Runtime::new(App { on: false, pressed: false, flat: true }, view, (200.0, 100.0), Env::new(true, 1.0));
    rt_raised.frame(0.0);
    rt_flat.frame(0.0);

    // Press squeeze: identical thumb rect.
    rt_raised.update(|s, _| s.pressed = true);
    rt_flat.update(|s, _| s.pressed = true);
    for _ in 0..10 {
        rt_raised.frame(1.0 / 120.0);
        rt_flat.frame(1.0 / 120.0);
        let tr = rt_raised.elements().into_iter().find(|e| e.path.ends_with("/thumb")).unwrap();
        let tf = rt_flat.elements().into_iter().find(|e| e.path.ends_with("/thumb")).unwrap();
        assert_eq!(tr.rect, tf.rect, "pressed squeeze geometry diverged");
        assert_eq!(tr.moving, tf.moving);
    }

    // Toggle off -> on: identical spring trajectory across 40 frames at 120Hz.
    rt_raised.update(|s, _| { s.pressed = false; s.on = true; });
    rt_flat.update(|s, _| { s.pressed = false; s.on = true; });
    let mut saw_moving = false;
    for frame in 0..40 {
        rt_raised.frame(1.0 / 120.0);
        rt_flat.frame(1.0 / 120.0);
        let tr = rt_raised.elements().into_iter().find(|e| e.path.ends_with("/thumb")).unwrap();
        let tf = rt_flat.elements().into_iter().find(|e| e.path.ends_with("/thumb")).unwrap();
        saw_moving |= tf.moving;
        assert_eq!(tr.rect, tf.rect, "toggle frame {frame}: knob rect diverged");
        assert_eq!(tr.moving, tf.moving, "toggle frame {frame}: moving state diverged");
    }
    assert!(saw_moving, "knob should animate on toggle");

    // Keyboard toggle on flat Switch.
    rt_flat.settle_all();
    rt_flat.key(Key::Tab.into());
    rt_flat.frame(0.0);
    rt_flat.key(Key::Text(" ".into()).into());
    rt_flat.frame(0.0);
    assert!(!rt_flat.state.on, "Space toggles focused flat Switch");
}

/// Flat compact Switch knobs keep exact device-pixel vertical centering at fractional scales.
#[test]
fn flat_switch_knobs_sit_centred_on_device_pixels() {
    use rune::render::KIND_RECT;
    const SIZES: [ControlSize; 5] =
        [ControlSize::Mini, ControlSize::Small, ControlSize::Regular, ControlSize::Large, ControlSize::ExtraLarge];
    fn view(_: &(), cx: &mut Cx<()>) -> El {
        let mut k = Kit::neutral(cx.system_dark());
        k.elevation.switch_off.clear();
        k.elevation.switch_on.clear();
        k.elevation.elements.clear();
        k.metrics.switch_look = Some(SwitchLook { track_border: 1.0, ..SwitchLook::flat(&k) });
        col().padding(10.09375).gap(1.1).children(SIZES.into_iter().flat_map(|size| {
            [false, true].map(|on| Switch::new(on).label("Switch").hide_label().control_size(size).build(&k))
        }))
    }
    for scale in [1.2, 1.25, 1.5, 1.75] {
        let mut rt = Runtime::new((), view, (120.0, 240.0), Env::new(true, scale));
        rt.frame(0.0);
        let els = rt.elements();
        let drawn = |r: [f32; 4]| {
            let want = [r[0] * scale, r[1] * scale, (r[0] + r[2]) * scale, (r[1] + r[3]) * scale];
            rt.instances.iter()
                .filter(|i| i.params[1] == KIND_RECT)
                .map(|i| [i.rect[0], i.rect[1], i.rect[0] + i.rect[2], i.rect[1] + i.rect[3]])
                .find(|e| (0..4).all(|k| (e[k] - want[k]).abs() <= 1.01))
                .unwrap_or_else(|| panic!("{scale}x: nothing drawn at {want:?}"))
        };
        let thumbs: Vec<_> = els.iter().filter(|e| e.path.ends_with("/thumb")).collect();
        assert_eq!(thumbs.len(), SIZES.len() * 2);
        for thumb in thumbs {
            let track = els.iter().find(|e| thumb.path.strip_suffix("/thumb") == Some(e.path.as_str())).unwrap();
            let (t, k) = (drawn(track.rect), drawn(thumb.rect));
            assert!(t.iter().chain(&k).all(|v| v.fract() == 0.0));
            assert_eq!(k[1] - t[1], t[3] - k[3], "{scale}x {}: track {t:?}, knob {k:?}", thumb.path);
        }
    }
}

/// Negative, NaN and infinite values in every new metric and SwitchLook::track_border clamp
/// cleanly to 0 without panicking.
#[test]
fn negative_and_non_finite_metrics_clamp_to_zero_without_panicking() {
    struct Bad {
        val: f32,
        edit: TextEdit,
    }
    fn view(s: &Bad, _: &mut Cx<Bad>) -> El {
        let v = s.val;
        let mut k = Kit::neutral(true);
        k.metrics.segmented = Some(SegmentedMetrics { height: v, padding_x: v, radius: v, inset: v, gap: v });
        k.metrics.disclosure_header_height = Some(v);
        k.metrics.progress = Some(ProgressMetrics { height: v, border_width: v });
        k.metrics.divider_thickness = Some(v);
        k.metrics.keybind = Some(KeybindMetrics { height: v, padding_x: v, radius: v });
        k.metrics.text_area_min_height = Some(v);
        k.metrics.standard_switch = Some(StandardSwitchMetrics { track_width: v, track_height: v, thumb: v, inset: v });
        k.metrics.switch_look = Some(SwitchLook { track_border: v, ..SwitchLook::flat(&k) });
        let mut k_rad = Kit::neutral(true);
        k_rad.radius.xs = v;
        k_rad.radius.sm = v;
        k_rad.radius.md = v;
        col()
            .child(all_controls_scene(&k, &s.edit))
            .child(SegmentedControl::new(&["A", "B"], Some(0)).build(&k_rad).key("bad-rad-seg"))
            .child(Collapsible::new("C", false).build(&k_rad).key("bad-rad-col"))
            .child(Keybind::new(&["K"]).build(&k_rad).key("bad-rad-kb"))
    }
    for bad in [-10.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut rt = Runtime::new(Bad { val: bad, edit: TextEdit::new("") }, view, (400.0, 800.0), Env::new(true, 1.0));
        rt.frame(0.0);
        for e in rt.elements() {
            assert!(e.rect.iter().all(|x| x.is_finite()), "bad={bad}: non-finite rect {:?} on {}", e.rect, e.path);
            assert!(e.rect[2] >= 0.0 && e.rect[3] >= 0.0, "bad={bad}: negative size {:?} on {}", e.rect, e.path);
            assert!(e.radius.is_finite() && e.radius >= 0.0, "bad={bad}: bad radius {} on {}", e.radius, e.path);
        }
        let prog_track = rt.elements().into_iter().find(|e| e.path.ends_with("/prog/track")).unwrap();
        assert_eq!(prog_track.border_color, None, "bad={bad}: Progress border omitted");
        let sw_ctrl = rt.elements().into_iter().find(|e| e.path.ends_with("/sw-off/control")).unwrap();
        assert_eq!(sw_ctrl.border_color, None, "bad={bad}: SwitchLook border omitted");
    }
}

/// Custom metrics and flat switches do not schedule extra commits or animation frames when idle,
/// and keep their accessibility roles, names and states intact.
#[test]
fn custom_metrics_stay_idle_at_rest_and_keep_accessibility() {
    struct Idle {
        edit: TextEdit,
    }
    fn view(s: &Idle, _: &mut Cx<Idle>) -> El {
        let mut k = Kit::neutral(true);
        k.radius.xs = 3.0;
        k.radius.sm = 5.0;
        k.radius.md = 8.0;
        k.metrics.segmented = Some(SegmentedMetrics { height: 34.0, padding_x: 14.0, ..SegmentedMetrics::kit(&k) });
        k.metrics.disclosure_header_height = Some(38.0);
        k.metrics.progress = Some(ProgressMetrics { height: 10.0, border_width: 0.0 });
        k.metrics.divider_thickness = Some(2.0);
        k.metrics.keybind = Some(KeybindMetrics { height: 18.0, ..KeybindMetrics::kit(&k) });
        k.metrics.text_area_min_height = Some(96.0);
        k.metrics.standard_switch = Some(StandardSwitchMetrics { track_width: 32.0, track_height: 18.0, thumb: 14.0, inset: 2.0 });
        k.metrics.switch_look = Some(SwitchLook { track_border: 1.0, ..SwitchLook::flat(&k) });
        all_controls_scene(&k, &s.edit)
    }
    let mut rt = Runtime::new(Idle { edit: TextEdit::new("") }, view, (420.0, 900.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.settle_all();
    rt.frame(0.0);
    let commits_before = rt.stats.commits;
    for _ in 0..10 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(rt.stats.commits, commits_before, "idle themed controls must not re-commit");
    assert!(!rt.animating, "idle themed controls must not keep animating");

    // Verify accessibility tree stays intact under custom metrics.
    let tree = rt.accessibility_json();
    let json = tree.to_string();
    assert!(json.contains("\"radio-group\""), "SegmentedControl keeps radio-group role");
    assert!(json.contains("\"switch\""), "flat and standard Switches keep switch role");
    assert!(json.contains("\"progress-indicator\""), "Progress keeps progress-indicator role");
    assert!(json.contains("\"expanded\""), "Collapsible/Accordion keep expanded state");
}
