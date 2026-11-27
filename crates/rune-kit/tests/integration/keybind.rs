use rune::{Color, Cx, El, Env, Runtime, TextStyle, col};
use rune_kit::{Keybind, KeybindMetrics, KeybindVariant, Kit};

fn texts_of(view: fn(&(), &mut Cx<()>) -> El) -> Vec<String> {
    let mut rt = Runtime::new((), view, (400.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.elements().into_iter().filter_map(|e| e.text).collect()
}

#[test]
fn default_uppercases_reversible_single_characters() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        col()
            .child(Keybind::new(&["g"]).build(&k))
            .child(Keybind::new(&["Shift", "g"]).build(&k))
            .child(Keybind::new(&["Ctrl", "k"]).build(&k))
            .child(Keybind::new(&["Space", "Esc"]).build(&k))
            .child(Keybind::new(&["é", "α", "ı", "ß"]).build(&k))
    }
    assert_eq!(
        texts_of(view),
        vec!["G", "Shift", "G", "Ctrl", "K", "Space", "Esc", "É", "Α", "ı", "ß"],
    );
}

#[test]
fn keep_case_preserves_authored_single_character_case() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        col()
            .child(Keybind::new(&["g"]).keep_case().build(&k))
            .child(Keybind::new(&["G"]).keep_case().build(&k))
            .child(Keybind::new(&["g", "g"]).keep_case().build(&k))
            .child(Keybind::new(&["Shift", "G"]).keep_case().build(&k))
            .child(Keybind::new(&["Shift", "g"]).keep_case().build(&k))
            .child(Keybind::new(&["é", "α", "ı", "ß"]).keep_case().build(&k))
            .child(Keybind::new(&["ج", "א"]).keep_case().build(&k))
    }
    assert_eq!(
        texts_of(view),
        vec!["g", "G", "g", "g", "Shift", "G", "Shift", "g", "é", "α", "ı", "ß", "ج", "א"],
    );
}

#[test]
fn keep_case_handles_edges_and_builder_order() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        let empty: [&str; 0] = [];
        let many: Vec<String> = (0..50).map(|i| if i % 2 == 0 { "g".into() } else { "G".into() }).collect();
        col()
            .child(Keybind::new(&empty).build(&k).key("empty-default"))
            .child(Keybind::new(&empty).keep_case().build(&k).key("empty-keep"))
            .child(Keybind::new(&[""]).keep_case().build(&k).key("blank"))
            .child(Keybind::new(&["⌘", "⇧", "/", "?", "0"]).keep_case().build(&k).key("symbols"))
            .child(Keybind::new(&["g"]).variant(KeybindVariant::Ghost).keep_case().build(&k).key("ghost-a"))
            .child(Keybind::new(&["g"]).keep_case().variant(KeybindVariant::Ghost).build(&k).key("ghost-b"))
            .child(Keybind::new(&["g"]).keep_case().keep_case().build(&k).key("idempotent"))
            .child(Keybind::new(&many).keep_case().build(&k).key("many"))
    }
    let mut rt = Runtime::new((), view, (800.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let els = rt.elements();
    let texts_under = |prefix: &str| -> Vec<String> {
        els.iter().filter(|e| e.path.contains(prefix)).filter_map(|e| e.text.clone()).collect()
    };
    assert!(texts_under("empty-default/").is_empty());
    assert!(texts_under("empty-keep/").is_empty());
    assert_eq!(texts_under("blank/"), vec![""]);
    assert_eq!(texts_under("symbols/"), vec!["⌘", "⇧", "/", "?", "0"]);
    assert_eq!(texts_under("ghost-a/"), vec!["g"]);
    assert_eq!(texts_under("ghost-b/"), vec!["g"]);
    assert_eq!(texts_under("idempotent/"), vec!["g"]);

    // Ghost styling matches regardless of builder call order.
    let ghost_a = els.iter().find(|e| e.path.ends_with("ghost-a/key0")).unwrap();
    let ghost_b = els.iter().find(|e| e.path.ends_with("ghost-b/key0")).unwrap();
    assert_eq!(ghost_a.fill, ghost_b.fill);
    assert_eq!(ghost_a.fill, rune::Color::TRANSPARENT);

    let many_texts = texts_under("many/");
    assert_eq!(many_texts.len(), 50);
    assert_eq!(many_texts[0], "g");
    assert_eq!(many_texts[1], "G");
    assert_eq!(many_texts[48], "g");
    assert_eq!(many_texts[49], "G");
}

#[test]
fn keep_case_preserves_accessibility_group_and_hides_keycaps() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        let empty: [&str; 0] = [];
        col()
            .child(Keybind::new(&["g"]).build(&k).key("default-lower"))
            .child(Keybind::new(&["g"]).keep_case().build(&k).key("keep-lower"))
            .child(Keybind::new(&["Shift", "G"]).keep_case().build(&k).key("keep-shift-upper"))
            .child(Keybind::new(&["Shift", "g"]).keep_case().build(&k).key("keep-shift-lower"))
            .child(Keybind::new(&empty).keep_case().build(&k).key("keep-empty"))
    }
    let mut rt = Runtime::new((), view, (400.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let tree = rt.accessibility_json();
    let find = |key: &str| {
        tree["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["key"] == key)
            .unwrap_or_else(|| panic!("no a11y node {key}: {tree}"))
    };

    for (key, expected_name) in [
        ("default-lower", "g"),
        ("keep-lower", "g"),
        ("keep-shift-upper", "Shift G"),
        ("keep-shift-lower", "Shift g"),
    ] {
        let n = find(key);
        assert_eq!(n["role"], "group", "{key}: {n}");
        assert_eq!(n["name"], expected_name, "{key}: {n}");
        assert!(n["children"].is_null(), "keycaps stay hidden from a11y tree for {key}: {n}");
    }
    let empty = find("keep-empty");
    assert_eq!(empty["role"], "group");
    assert!(empty["children"].is_null());
}

#[test]
fn keybind_metrics_apply_border_and_label_style_with_unchanged_defaults() {
    const RIM: Color = Color { r: 0.9, g: 0.25, b: 0.15, a: 1.0 };
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let base = Kit::neutral(true);

        let mut default_border = base.clone();
        default_border.metrics.keybind = Some(KeybindMetrics { border_width: 1.0, ..KeybindMetrics::kit(&base) });

        let mut custom = base.clone();
        custom.metrics.keybind = Some(KeybindMetrics {
            height: 22.0,
            padding_x: 6.0,
            radius: 4.0,
            border_width: 2.0,
            border_color: RIM,
            label: TextStyle::new(14.0, 600.0).lh(18.0).italic(),
        });

        let mut transparent_rim = base.clone();
        transparent_rim.metrics.keybind = Some(KeybindMetrics {
            border_width: 2.0,
            border_color: Color::TRANSPARENT,
            ..KeybindMetrics::kit(&base)
        });

        let mut custom_ty = base.clone();
        custom_ty.ty.small_medium = TextStyle::new(13.0, 700.0).lh(16.0);

        col()
            .gap(8.0)
            .child(Keybind::new(&["⌘", "K"]).build(&base).key("default"))
            .child(Keybind::new(&["⌘", "K"]).build(&default_border).key("default-rim"))
            .child(Keybind::new(&["⌘", "K"]).build(&custom).key("custom-neutral"))
            .child(Keybind::new(&["⌘", "K"]).variant(KeybindVariant::Ghost).build(&custom).key("custom-ghost"))
            .child(Keybind::new(&["K"]).build(&transparent_rim).key("transparent-rim"))
            .child(Keybind::new(&["K"]).build(&custom_ty).key("custom-ty"))
    }

    let mut rt = Runtime::new((), view, (400.0, 320.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let els = rt.elements();
    let find = |suffix: &str| {
        els.iter()
            .find(|e| e.path.ends_with(suffix))
            .unwrap_or_else(|| panic!("no element ending with {suffix}"))
    };
    let text_in = |parent_suffix: &str| {
        let parent_path = &find(parent_suffix).path;
        els.iter()
            .find(|e| e.text.is_some() && e.path.starts_with(parent_path) && e.path != *parent_path)
            .unwrap_or_else(|| panic!("no text element inside {parent_suffix}"))
    };
    let base = Kit::neutral(true);

    // Built-in default: no border, 11pt small_medium on an 11pt line.
    let def0 = find("/default/key0");
    assert_eq!(def0.border_color, None);
    let def_ts = text_in("/default/key0").text_style.as_ref().unwrap();
    assert_eq!((def_ts.size, def_ts.weight, def_ts.line_height, def_ts.italic), (11.0, 500.0, 11.0, false));

    // Setting only border_width uses k.c.border_base from KeybindMetrics::kit(&k).
    let def_rim0 = find("/default-rim/key0");
    let def_rim1 = find("/default-rim/key1");
    assert_eq!(def_rim0.border_color, Some(base.c.border_base));
    assert_eq!(def_rim1.border_color, Some(base.c.border_base));
    assert!(rt.instances.iter().any(|i| {
        i.params[1] == rune::render::KIND_RECT
            && (i.params[0] - 1.0).abs() < 0.01
            && (i.rect[0] - def_rim0.rect[0]).abs() < 0.5
            && (i.rect[1] - def_rim0.rect[1]).abs() < 0.5
            && i.border[3] > 0.0
    }));

    // Custom border width/color and label style apply to Neutral keycaps.
    let cn0 = find("/custom-neutral/key0");
    let cn1 = find("/custom-neutral/key1");
    assert_eq!((cn0.rect[3], cn0.radius, cn0.border_color), (22.0, 4.0, Some(RIM)));
    assert_eq!((cn1.rect[3], cn1.radius, cn1.border_color), (22.0, 4.0, Some(RIM)));
    assert!(rt.instances.iter().any(|i| {
        i.params[1] == rune::render::KIND_RECT
            && (i.params[0] - 2.0).abs() < 0.01
            && (i.rect[0] - cn0.rect[0]).abs() < 0.5
            && (i.rect[1] - cn0.rect[1]).abs() < 0.5
            && i.border[3] > 0.0
    }));
    let cn_ts = text_in("/custom-neutral/key0").text_style.as_ref().unwrap();
    assert_eq!((cn_ts.size, cn_ts.weight, cn_ts.line_height, cn_ts.italic), (14.0, 600.0, 18.0, true));

    // Ghost stays unbordered, while taking the custom height and label text style.
    let cg0 = find("/custom-ghost/key0");
    assert_eq!((cg0.rect[3], cg0.fill, cg0.border_color), (22.0, Color::TRANSPARENT, None));
    let cg_ts = text_in("/custom-ghost/key0").text_style.as_ref().unwrap();
    assert_eq!((cg_ts.size, cg_ts.weight, cg_ts.line_height, cg_ts.italic), (14.0, 600.0, 18.0, true));

    // Transparent border_color omits the border even with positive border_width.
    assert_eq!(find("/transparent-rim/key0").border_color, None);

    // Mutating k.ty.small_medium without setting k.metrics.keybind flows into Keybind (keeping lh 11).
    let ty_ts = text_in("/custom-ty/key0").text_style.as_ref().unwrap();
    assert_eq!((ty_ts.size, ty_ts.weight, ty_ts.line_height), (13.0, 700.0, 11.0));
}
