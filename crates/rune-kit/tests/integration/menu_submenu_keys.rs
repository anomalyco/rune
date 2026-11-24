use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, col, el, row};
use rune_kit::{Button, Kit, Menu, MenuEntry, MenuItem, Placement, root};

fn find<S>(rt: &Runtime<S>, key: &str) -> Option<rune::runtime::ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key))
}

#[derive(Default)]
struct SubmenuState {
    placement: Option<Placement>,
    min_width: Option<f32>,
    align_right: bool,
    menu_placement: Option<Placement>,
}

fn submenu_view(s: &SubmenuState, _: &mut Cx<SubmenuState>) -> El {
    let k = Kit::neutral(true);
    let mut layer = Vec::new();
    let versions = vec![
        MenuEntry::item(MenuItem::new("HTTP/1.1").checked(true)),
        MenuEntry::item(MenuItem::new("HTTP/2")),
        MenuEntry::item(MenuItem::new("HTTP/3")),
    ];
    let mut sub_item = MenuItem::new("HTTP version").shortcut("HTTP/1.1").submenu(versions, true);
    if let Some(p) = s.placement {
        sub_item = sub_item.submenu_placement(p);
    }
    if let Some(w) = s.min_width {
        sub_item = sub_item.submenu_min_width(w);
    }
    let mut menu = Menu::new("versions")
        .trigger(Button::new("HTTP version").build(&k))
        .item(sub_item)
        .open(true);
    if let Some(p) = s.menu_placement {
        menu = menu.placement(p);
    }
    let trigger = menu.build(&k, &mut layer);
    let bar = if s.align_right {
        row().w_full().child(el().grow(1.0)).child(trigger)
    } else {
        row().w_full().pl(240.0).child(trigger)
    };
    root(col().w_full().h_full().padding(16.0).child(bar), layer)
}

fn mount_submenu(state: SubmenuState, size: (f32, f32)) -> Runtime<SubmenuState> {
    let mut rt = Runtime::new(state, submenu_view, size, Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    rt
}

#[test]
fn default_submenu_opens_right_at_160_pt_floor() {
    let rt = mount_submenu(SubmenuState::default(), (900.0, 480.0));
    let item = find(&rt, "versions-item0").expect("submenu item");
    let sub = find(&rt, "versions-item0-sub-menu").expect("submenu surface");
    assert!(
        (sub.target[0] - (item.target[0] + item.target[2])).abs() < 0.5,
        "default submenu opens at item right edge: sub {:?} vs item {:?}",
        sub.target,
        item.target,
    );
    assert!(
        (sub.target[1] - item.target[1]).abs() < 0.5,
        "default submenu top-aligns with item: sub {:?} vs item {:?}",
        sub.target,
        item.target,
    );
    assert!(
        sub.target[2] >= 160.0 - 0.5,
        "default submenu keeps 160 pt min_width, got {}",
        sub.target[2],
    );
}

#[test]
fn submenu_placement_left_start_and_min_width_configure_position_and_width() {
    let rt = mount_submenu(
        SubmenuState {
            placement: Some(Placement::LeftStart),
            min_width: Some(0.0),
            align_right: true,
            menu_placement: Some(Placement::BottomEnd),
        },
        (1200.0, 600.0),
    );
    let item = find(&rt, "versions-item0").expect("submenu item");
    let sub = find(&rt, "versions-item0-sub-menu").expect("left-placed submenu surface");
    assert!(
        ((sub.target[0] + sub.target[2]) - item.target[0]).abs() < 0.5,
        "LeftStart submenu right edge meets item left edge: sub {:?} vs item {:?}",
        sub.target,
        item.target,
    );
    assert!(
        (sub.target[1] - item.target[1]).abs() < 0.5,
        "LeftStart submenu top-aligns with item: sub {:?} vs item {:?}",
        sub.target,
        item.target,
    );
    assert!(
        sub.target[2] > 40.0 && sub.target[2] < 160.0,
        "submenu_min_width(0.0) sizes narrow items to content below 160 pt, got {}",
        sub.target[2],
    );

    // Explicit wider min_width expands the submenu surface.
    let wide = mount_submenu(
        SubmenuState {
            placement: Some(Placement::LeftStart),
            min_width: Some(240.0),
            align_right: true,
            menu_placement: Some(Placement::BottomEnd),
        },
        (1200.0, 600.0),
    );
    let wide_sub = find(&wide, "versions-item0-sub-menu").expect("wide submenu");
    assert!(
        wide_sub.target[2] >= 240.0 - 0.5,
        "submenu_min_width(240.0) enforces 240 pt floor, got {}",
        wide_sub.target[2],
    );

    // Negative and NaN min_width clamp cleanly to 0.0 without panicking.
    for bad in [-20.0, f32::NAN, f32::INFINITY] {
        let rt_bad = mount_submenu(
            SubmenuState {
                placement: Some(Placement::LeftStart),
                min_width: Some(bad),
                align_right: true,
                menu_placement: Some(Placement::BottomEnd),
            },
            (1200.0, 600.0),
        );
        let sub_bad = find(&rt_bad, "versions-item0-sub-menu").expect("clamped submenu");
        assert!(
            sub_bad.target[2] > 40.0 && sub_bad.target[2] < 160.0,
            "non-finite or negative min_width ({bad}) clamps to 0.0: got {}",
            sub_bad.target[2],
        );
    }

    // LeftEnd bottom-aligns the submenu with the item row's bottom edge.
    let end = mount_submenu(
        SubmenuState {
            placement: Some(Placement::LeftEnd),
            min_width: Some(0.0),
            align_right: true,
            menu_placement: Some(Placement::BottomEnd),
        },
        (1200.0, 600.0),
    );
    let end_item = find(&end, "versions-item0").unwrap();
    let end_sub = find(&end, "versions-item0-sub-menu").unwrap();
    assert!(
        ((end_sub.target[0] + end_sub.target[2]) - end_item.target[0]).abs() < 0.5,
        "LeftEnd places submenu to the left: {:?} vs {:?}",
        end_sub.target,
        end_item.target,
    );
    // Top of submenu stays inside the window (8 pt margin); check vertical anchor when room exists.
    assert!(end_sub.target[1] <= end_item.target[1] + 0.5);
}

#[derive(Default)]
struct KeyFocusState {
    prepend: usize,
    sub_open: bool,
    chosen: Vec<&'static str>,
}

fn key_focus_view(s: &KeyFocusState, _: &mut Cx<KeyFocusState>) -> El {
    let k = Kit::neutral(true);
    let mut layer = Vec::new();
    let mut menu = Menu::new("file")
        .label("File actions")
        .trigger(Button::new("File").build(&k))
        .open(true);
    for i in 0..s.prepend {
        menu = menu.item(
            MenuItem::new(format!("Inserted {i}"))
                .key(format!("inserted-{i}"))
                .on_select(|s: &mut KeyFocusState| s.chosen.push("inserted")),
        );
    }
    menu = menu
        .item(
            MenuItem::new("New")
                .key("new")
                .on_select(|s: &mut KeyFocusState| s.chosen.push("new")),
        )
        .item(
            MenuItem::new("Save")
                .key("save")
                .on_select(|s: &mut KeyFocusState| s.chosen.push("save")),
        )
        .item(
            MenuItem::new("Export")
                .key("export")
                .submenu(
                    vec![MenuEntry::item(MenuItem::new("PDF").key("export-pdf"))],
                    s.sub_open,
                )
                .submenu_placement(Placement::RightStart),
        )
        .item(MenuItem::new("Fallback").key("   "));
    let trigger = menu.build(&k, &mut layer);
    root(col().w_full().h_full().padding(16.0).child(trigger), layer)
}

#[test]
fn custom_item_key_preserves_focus_across_prepended_rows_and_anchors_submenu() {
    let mut rt = Runtime::new(
        KeyFocusState { prepend: 0, sub_open: true, chosen: Vec::new() },
        key_focus_view,
        (800.0, 480.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);

    // Move focus to "save" via explicit key.
    rt.update(|_, tx| tx.focus("save", true));
    rt.frame(0.0);
    assert!(find(&rt, "save").unwrap().focused, "save starts focused");

    // Blank key falls back to positional "{menu_key}-item{i}" (index 3 before prepend).
    assert!(find(&rt, "file-item3").is_some(), "blank key falls back to positional key");

    // Submenu anchors to the explicit item key "export" -> "export-sub-menu" and its keyed child.
    let export_item = find(&rt, "export").expect("keyed export row");
    let sub_menu = find(&rt, "export-sub-menu").expect("submenu keyed from explicit item key");
    assert!(find(&rt, "export-pdf").is_some(), "submenu item keeps its own explicit key");
    assert!((sub_menu.target[0] - (export_item.target[0] + export_item.target[2])).abs() < 0.5);

    // Insert two rows above "save": focus must remain on "save", not shift to an earlier row.
    rt.update(|s, _| s.prepend = 2);
    rt.frame(0.0);
    assert!(find(&rt, "save").unwrap().focused, "focus stays on keyed save row after rows inserted above");
    assert!(!find(&rt, "new").unwrap().focused);
    assert!(!find(&rt, "inserted-0").unwrap().focused);

    // Pressing Enter activates "save", confirming keyboard action still targets the preserved item.
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.chosen, ["save"]);
}

#[test]
fn menu_item_a11y_label_separates_accessible_name_from_drawn_text_and_drives_typeahead() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        let trigger = Menu::new("crumbs")
            .label("Breadcrumb path")
            .trigger(Button::new("Path").build(&k))
            .item(MenuItem::new("Alpha").key("alpha"))
            .item(
                MenuItem::new("…/appearance_panel.rs")
                    .key("panel")
                    .a11y_label("crates/rune-kit/appearance_panel.rs"),
            )
            .item(MenuItem::new("Fallback visible").key("blank-a11y").a11y_label("   "))
            .open(true)
            .build(&k, &mut layer);
        root(col().w_full().h_full().padding(16.0).child(trigger), layer)
    }

    let mut rt = Runtime::new((), view, (640.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);

    let menu = find(&rt, "crumbs-menu").expect("menu surface");
    assert_eq!(menu.label.as_deref(), Some("Breadcrumb path"));

    let panel = find(&rt, "panel").expect("panel item");
    assert_eq!(panel.label.as_deref(), Some("crates/rune-kit/appearance_panel.rs"));
    let prefix = format!("{}/", panel.path);
    let drawn = rt
        .elements()
        .into_iter()
        .find(|e| e.path.starts_with(&prefix) && e.key.as_deref() == Some("label"))
        .expect("drawn label text element");
    assert_eq!(drawn.text.as_deref(), Some("…/appearance_panel.rs"));

    // Blank a11y_label falls back to the drawn text.
    assert_eq!(find(&rt, "blank-a11y").unwrap().label.as_deref(), Some("Fallback visible"));

    // Typeahead matches the accessible name ("crates/...") rather than the elided "…/" prefix.
    rt.update(|_, tx| tx.focus("alpha", true));
    rt.frame(0.0);
    assert!(find(&rt, "alpha").unwrap().focused);
    rt.key(KeyEvent { key: Key::Text("c".into()), mods: Mods::default() });
    rt.frame(0.0);
    assert!(find(&rt, "panel").unwrap().focused, "typeahead finds item by a11y_label");
}

#[test]
fn small_window_submenu_clamps_both_axes_scrolls_and_keeps_a11y() {
    use rune::PointerPress;
    use rune_kit::context_menu;

    fn view(rows: &usize, _: &mut Cx<usize>) -> El {
        let k = Kit::neutral(true);
        let sub = (0..*rows)
            .map(|i| MenuEntry::item(MenuItem::new(format!("Application {i} with a longer title"))))
            .collect();
        let mut layer = Vec::new();
        let trigger = context_menu("ctx", el(), Some(PointerPress { x: 130.0, y: 60.0 }))
            .item(MenuItem::new("Open"))
            .item(MenuItem::new("Open With").submenu(sub, true))
            .item(MenuItem::new("Copy"))
            .build(&k, &mut layer);
        root(col().w_full().h_full().fill(k.c.bg_base).child(trigger), layer)
    }

    // 8-row submenu in a 420x400 window stays inside [8..412, 8..392] without clipping right.
    let mut short = Runtime::new(8usize, view, (420.0, 400.0), Env::new(true, 1.0));
    short.frame(0.0);
    short.frame(1.0 / 60.0);
    let sub8 = find(&short, "ctx-item1-sub-menu").expect("8-row submenu");
    assert!(
        sub8.target[0] >= 8.0 && sub8.target[0] + sub8.target[2] <= 412.0 + 0.5,
        "8-row submenu fits horizontally inside 420pt window: {:?}",
        sub8.target,
    );
    assert!(
        sub8.target[1] >= 8.0 && sub8.target[1] + sub8.target[3] <= 392.0 + 0.5,
        "8-row submenu fits vertically inside 400pt window: {:?}",
        sub8.target,
    );

    // 20-row submenu in a 420x400 window clamps height to 384pt, stays inside [8..412, 8..392], and scrolls.
    let mut long = Runtime::new(20usize, view, (420.0, 400.0), Env::new(true, 1.0));
    long.frame(0.0);
    long.frame(1.0 / 60.0);
    let sub20 = find(&long, "ctx-item1-sub-menu").expect("20-row submenu");
    assert!(
        sub20.target[0] >= 8.0 && sub20.target[0] + sub20.target[2] <= 412.0 + 0.5,
        "20-row submenu fits horizontally inside 420pt window: {:?}",
        sub20.target,
    );
    assert!(
        sub20.target[1] >= 8.0 && sub20.target[1] + sub20.target[3] <= 392.0 + 0.5,
        "20-row submenu fits vertically inside 400pt window: {:?}",
        sub20.target,
    );
    assert!(sub20.target[3] <= 384.5, "20-row submenu clamps to viewport.h - 16: {:?}", sub20.target);

    // Accessibility tree keeps menu role, accessible name, and expanded trigger state.
    let a11y = long.accessibility_json().to_string();
    assert!(a11y.contains("Open With"), "submenu keeps accessible name in a11y tree: {a11y}");
    assert_eq!(sub20.role, "menu");
    assert_eq!(sub20.label.as_deref(), Some("Open With"));

    // Wheel over the 20-row submenu scrolls its rows upward.
    let first_before = find(&long, "ctx-item1-sub-item0").expect("first submenu item").rect[1];
    let (cx, cy) = (sub20.target[0] + sub20.target[2] * 0.5, sub20.target[1] + sub20.target[3] * 0.5);
    long.wheel(cx, cy, -120.0);
    for _ in 0..8 {
        long.frame(1.0 / 60.0);
    }
    let first_after = find(&long, "ctx-item1-sub-item0").expect("first submenu item").rect[1];
    assert!(
        first_after < first_before - 10.0,
        "20-row submenu scrolls vertically on wheel: before={first_before}, after={first_after}",
    );
}
