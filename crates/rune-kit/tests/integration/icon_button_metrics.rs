use rune::{Cx, El, Env, Runtime, col, el};
use rune_kit::shell::{PaneState, SidebarToggle};
use rune_kit::{
    BySize, ControlSize, IconButton, IconButtonMetrics, Kit, Radius, SplitView, State, Toolbar,
};

const SIZES: [ControlSize; 5] = [
    ControlSize::Mini,
    ControlSize::Small,
    ControlSize::Regular,
    ControlSize::Large,
    ControlSize::ExtraLarge,
];

fn find_key<'a>(els: &'a [rune::runtime::ElementInfo], key: &str) -> &'a rune::runtime::ElementInfo {
    els.iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("missing key {key}"))
}

fn find_child<'a>(
    els: &'a [rune::runtime::ElementInfo],
    root_key: &str,
    child_key: &str,
) -> &'a rune::runtime::ElementInfo {
    let prefix = format!("{}/", find_key(els, root_key).path);
    els.iter()
        .find(|e| e.key.as_deref() == Some(child_key) && e.path.starts_with(&prefix))
        .unwrap_or_else(|| panic!("missing {child_key} under {root_key}"))
}

#[test]
fn icon_button_metrics_defaults_preserve_built_in_values_and_follow_radius_scale() {
    fn view(kit: &Kit, _: &mut Cx<Kit>) -> El {
        let mut c = col().items_start().gap(4.0);
        for (i, s) in SIZES.into_iter().enumerate() {
            c = c.child(
                IconButton::new(kit.glyph.copy)
                    .control_size(s)
                    .label(format!("Action {i}"))
                    .build(kit)
                    .key(format!("ib-{i}")),
            );
        }
        c
    }

    for (name, base) in [
        ("neutral-light", Kit::neutral(false)),
        ("neutral-dark", Kit::neutral(true)),
        ("opencode-light", rune_theme_opencode::kit(false).clone()),
        ("opencode-dark", rune_theme_opencode::kit(true).clone()),
    ] {
        let table = IconButtonMetrics::kit(&base);
        let expected = [
            (ControlSize::Mini, 16.0, base.radius.sm, 12.0),
            (ControlSize::Small, 20.0, base.radius.sm, 16.0),
            (ControlSize::Regular, 24.0, base.radius.md, 16.0),
            (ControlSize::Large, 28.0, base.radius.md, 16.0),
            (ControlSize::ExtraLarge, 36.0, base.radius.lg, 18.0),
        ];
        for (size, side, radius, icon) in expected {
            assert_eq!(
                table.get(size),
                IconButtonMetrics { size: side, radius, icon },
                "{name}: default IconButtonMetrics for {size:?}",
            );
        }

        let mut rt_def = Runtime::new(base.clone(), view, (200.0, 300.0), Env::new(base.dark, 1.0));
        rt_def.frame(0.0);
        let mut explicit = base.clone();
        explicit.metrics.icon_button = Some(IconButtonMetrics::kit(&base));
        let mut rt_exp = Runtime::new(explicit, view, (200.0, 300.0), Env::new(base.dark, 1.0));
        rt_exp.frame(0.0);

        let els_def = rt_def.elements();
        for (i, (_, side, radius, icon)) in expected.into_iter().enumerate() {
            let key = format!("ib-{i}");
            let btn = find_key(&els_def, &key);
            let glyph = find_child(&els_def, &key, "icon");
            assert_eq!((btn.rect[2], btn.rect[3], btn.radius), (side, side, radius), "{name}: {key}");
            assert_eq!((glyph.rect[2], glyph.rect[3]), (icon, icon), "{name}: {key} glyph");
        }
        assert_eq!(rt_def.instances.len(), rt_exp.instances.len(), "{name}: paint instance count");
        for (a, b) in rt_def.instances.iter().zip(&rt_exp.instances) {
            assert_eq!((a.rect, a.radii, a.fill, a.border, a.params), (b.rect, b.radii, b.fill, b.border, b.params));
        }
    }

    // Custom Kit::radius flows into default IconButton corners without setting metrics.icon_button.
    let mut custom_r = Kit::neutral(true);
    custom_r.radius = Radius { xs: 1.0, sm: 3.0, md: 7.0, lg: 11.0, xl: 15.0 };
    let mut rt = Runtime::new(custom_r, view, (200.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let els = rt.elements();
    for (i, want_r) in [3.0, 3.0, 7.0, 7.0, 11.0].into_iter().enumerate() {
        assert_eq!(find_key(&els, &format!("ib-{i}")).radius, want_r, "radius index {i}");
    }
}

#[test]
fn icon_button_custom_metrics_drive_size_radius_glyph_hit_area_focus_and_a11y() {
    struct App {
        presses: usize,
    }

    fn custom_kit() -> Kit {
        let mut k = Kit::neutral(true);
        let m = |size, radius, icon| IconButtonMetrics { size, radius, icon };
        k.metrics.icon_button = Some(BySize {
            mini: m(22.0, 4.0, 10.0),
            small: m(26.0, 6.0, 14.0),
            regular: m(32.0, 8.0, 18.0),
            large: m(38.0, 10.0, 20.0),
            extra_large: m(44.0, 14.0, 24.0),
        });
        k
    }

    fn view(_: &App, _: &mut Cx<App>) -> El {
        let k = custom_kit();
        let mut c = col().padding(16.0).gap(12.0).items_start();
        for (i, s) in SIZES.into_iter().enumerate() {
            let mut b = IconButton::new(k.glyph.close)
                .control_size(s)
                .label(format!("Btn {i}"))
                .on_press(|s: &mut App| s.presses += 1);
            if s == ControlSize::Regular {
                b = b.state(State::Focus);
            }
            c = c.child(b.build(&k).key(format!("ib-{i}")));
        }
        c.child(
            IconButton::new(k.glyph.close)
                .label("Default size")
                .on_press(|s: &mut App| s.presses += 10)
                .build(&k)
                .key("ib-default"),
        )
    }

    let mut rt = Runtime::new(App { presses: 0 }, view, (300.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.settle_all();
    rt.frame(0.0);

    let els = rt.elements();
    let expected = [
        (22.0, 4.0, 10.0),
        (26.0, 6.0, 14.0),
        (32.0, 8.0, 18.0),
        (38.0, 10.0, 20.0),
        (44.0, 14.0, 24.0),
    ];
    for (i, (side, radius, icon)) in expected.into_iter().enumerate() {
        let key = format!("ib-{i}");
        let btn = find_key(&els, &key);
        let glyph = find_child(&els, &key, "icon");
        assert_eq!((btn.rect[2], btn.rect[3], btn.radius), (side, side, radius), "{key} box");
        assert_eq!((glyph.rect[2], glyph.rect[3]), (icon, icon), "{key} glyph");
        let btn_cx = btn.rect[0] + btn.rect[2] * 0.5;
        let btn_cy = btn.rect[1] + btn.rect[3] * 0.5;
        let glyph_cx = glyph.rect[0] + glyph.rect[2] * 0.5;
        let glyph_cy = glyph.rect[1] + glyph.rect[3] * 0.5;
        assert!((btn_cx - glyph_cx).abs() < 0.5, "{key}: glyph horizontally centered");
        assert!((btn_cy - glyph_cy).abs() < 0.5, "{key}: glyph vertically centered");
    }

    // Unsized IconButton uses Regular (32x32, radius 8, glyph 18).
    let def_btn = find_key(&els, "ib-default");
    assert_eq!((def_btn.rect[2], def_btn.rect[3], def_btn.radius), (32.0, 32.0, 8.0));
    let def_glyph = find_child(&els, "ib-default", "icon");
    assert_eq!((def_glyph.rect[2], def_glyph.rect[3]), (18.0, 18.0));

    // Focus ring on Regular (32x32, radius 8, width 2, offset 2 -> grow 4, outer rect 40x40).
    let reg = find_key(&els, "ib-2").rect;
    let has_focus_ring = rt.instances.iter().any(|i| {
        i.params[1] == rune::render::KIND_RECT
            && (i.params[0] - 2.0).abs() < 0.01
            && i.fill == [0.0; 4]
            && i.border[3] > 0.0
            && (i.rect[0] - (reg[0] - 4.0)).abs() < 0.5
            && (i.rect[1] - (reg[1] - 4.0)).abs() < 0.5
            && (i.rect[2] - (reg[2] + 8.0)).abs() < 0.5
            && (i.rect[3] - (reg[3] + 8.0)).abs() < 0.5
    });
    assert!(has_focus_ring, "focus ring follows themed 32x32 IconButton bounds");

    // Accessibility tree exposes the themed 32x32 bounds for the default button.
    let a11y = rt.accessibility_json();
    let a11y_str = a11y.to_string();
    assert!(a11y_str.contains("\"Default size\""), "accessible label preserved");
    let def_node = find_a11y_by_name(&a11y, "Default size").expect("a11y node for Default size");
    let bounds = def_node["bounds"].as_array().expect("a11y bounds array");
    let w = bounds[2].as_f64().unwrap() as f32;
    let h = bounds[3].as_f64().unwrap() as f32;
    assert_eq!((w, h), (32.0, 32.0), "a11y bounds follow themed IconButton size");

    // Hit target covers the expanded 32x32 area (point at +30,+30 is outside the old 24x24 box).
    let (hx, hy) = (def_btn.rect[0] + 30.0, def_btn.rect[1] + 30.0);
    rt.pointer_move(hx, hy);
    rt.pointer_down(hx, hy);
    rt.pointer_up(hx, hy);
    rt.frame(0.0);
    assert_eq!(rt.state.presses, 10, "click in outer edge of themed 32x32 button hits");

    // Idle frames do not commit or animate.
    rt.settle_all();
    rt.frame(0.0);
    let commits = rt.stats.commits;
    for _ in 0..5 {
        assert!(!rt.frame(1.0 / 60.0));
    }
    assert_eq!(rt.stats.commits, commits);
}

fn find_a11y_by_name<'a>(node: &'a serde_json::Value, name: &str) -> Option<&'a serde_json::Value> {
    if node.get("name").and_then(|v| v.as_str()) == Some(name) {
        return Some(node);
    }
    if let Some(children) = node.get("children").and_then(|v| v.as_array()) {
        for ch in children {
            if let Some(found) = find_a11y_by_name(ch, name) {
                return Some(found);
            }
        }
    }
    None
}

#[test]
fn split_view_sidebar_toggle_follows_icon_button_metrics() {
    struct App {
        sidebar: PaneState,
    }

    fn view(s: &App, _: &mut Cx<App>) -> El {
        let mut k = Kit::neutral(true);
        let mut table = IconButtonMetrics::kit(&k);
        table.regular = IconButtonMetrics { size: 32.0, radius: 8.0, icon: 18.0 };
        k.metrics.icon_button = Some(table);
        SplitView::new(el().key("main"))
            .sidebar(&s.sidebar, |s: &mut App| &mut s.sidebar, el().key("side-body"))
            .sidebar_toggle(SidebarToggle::new().size(ControlSize::Regular).unread(true))
            .toolbar(Toolbar::new().title("Inbox"))
            .reduced_motion(true)
            .build(&k)
    }

    let mut rt = Runtime::new(
        App { sidebar: PaneState::new(220.0).collapsed() },
        view,
        (600.0, 400.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    rt.settle_all();
    rt.frame(0.0);

    let els = rt.elements();
    let toggle = find_key(&els, "sidebar-toggle");
    let dot = find_key(&els, "sidebar-toggle-unread");
    assert_eq!((toggle.rect[2], toggle.rect[3], toggle.radius), (32.0, 32.0, 8.0));
    // Vertically centered in the 48pt toolbar: (48 - 32) / 2 = 8.
    assert!((toggle.rect[1] - 8.0).abs() < 0.5, "toggle vertically centered: {:?}", toggle.rect);
    // Unread dot sits at `side - 5.0` = 27.0 from toggle's left edge.
    assert!(
        ((dot.rect[0] - toggle.rect[0]) - 27.0).abs() < 0.5,
        "unread dot follows themed toggle width: toggle={:?}, dot={:?}",
        toggle.rect,
        dot.rect,
    );
}

#[test]
fn icon_button_metrics_clamp_negative_and_non_finite_values_to_zero() {
    fn view(bad: &f32, _: &mut Cx<f32>) -> El {
        let mut k = Kit::neutral(true);
        let b = IconButtonMetrics { size: *bad, radius: *bad, icon: *bad };
        k.metrics.icon_button = Some(BySize { mini: b, small: b, regular: b, large: b, extra_large: b });
        let mut k_rad = Kit::neutral(true);
        k_rad.radius = Radius { xs: *bad, sm: *bad, md: *bad, lg: *bad, xl: *bad };
        col()
            .child(IconButton::new(k.glyph.close).label("Bad metrics").build(&k).key("bad-m"))
            .child(IconButton::new(k_rad.glyph.close).label("Bad radius").build(&k_rad).key("bad-r"))
    }

    for bad in [-12.0, -0.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut rt = Runtime::new(bad, view, (200.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let els = rt.elements();
        let bm = find_key(&els, "bad-m");
        let bm_icon = find_child(&els, "bad-m", "icon");
        assert_eq!((bm.rect[2], bm.rect[3], bm.radius), (0.0, 0.0, 0.0), "bad={bad}");
        assert_eq!((bm_icon.rect[2], bm_icon.rect[3]), (0.0, 0.0), "bad={bad} icon");
        let br = find_key(&els, "bad-r");
        assert_eq!(br.radius, 0.0, "bad={bad} radius");
    }
}
