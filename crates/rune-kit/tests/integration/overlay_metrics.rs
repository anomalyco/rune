use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, TextStyle, col, el, text};
use rune_kit::{
    Button, Combobox, ComboboxEvent, ComboboxOption, ComboboxState, Dialog, DialogMetrics, Kit, Menu,
    MenuEntry, MenuItem, MenuMetrics, Metrics, Popover, PopoverMetrics, PopupItem, PopupList,
    PopupRowMetrics, Radius, Select, SelectAppearance, SelectMetrics, root,
};

fn find<S>(rt: &Runtime<S>, key: &str) -> Option<rune::runtime::ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key))
}

fn find_under<S>(rt: &Runtime<S>, root_key: &str, key: &str) -> Option<rune::runtime::ElementInfo> {
    let prefix = format!("{}/", find(rt, root_key)?.path);
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key) && e.path.starts_with(&prefix))
}

fn settled<S: 'static>(state: S, view: fn(&S, &mut Cx<S>) -> El, size: (f32, f32)) -> Runtime<S> {
    let mut rt = Runtime::new(state, view, size, Env::new(true, 1.0));
    rt.frame(0.0);
    rt.settle_all();
    rt.frame(0.0);
    rt
}

#[test]
fn overlay_metrics_defaults_follow_radius_scale_and_sanitize_bad_floats() {
    let mut k = Kit::neutral(true);
    assert_eq!(k.metrics, Metrics::default());
    assert_eq!(DialogMetrics::kit(&k), DialogMetrics { radius: 6.0, padding: 16.0, title: None });
    assert_eq!(PopupRowMetrics::kit(&k), PopupRowMetrics { height: 28.0, radius: 4.0 });
    assert_eq!(SelectMetrics::kit(&k), SelectMetrics { chevron: 16.0, chevron_box: 20.0 });
    assert_eq!(PopoverMetrics::kit(&k), PopoverMetrics { min_width: 200.0, max_width: 320.0 });
    assert_eq!(
        MenuMetrics::kit(&k),
        MenuMetrics {
            padding: 2.0,
            standard_padding: 4.0,
            separator_above: 2.0,
            separator_below: 2.0,
            standard_separator_above: 4.0,
            standard_separator_below: 4.0,
            standard_inner_border: 1.0,
        },
    );

    k.radius = Radius { xs: 1.0, sm: 2.0, md: 10.0, lg: 12.0, xl: 14.0 };
    assert_eq!(DialogMetrics::kit(&k).radius, 10.0);
    assert_eq!(PopupRowMetrics::kit(&k).radius, 2.0);

    // Every new metric struct and builder must clamp negative/NaN/infinite values to >= 0 without panicking.
    for bad in [-12.0, -0.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        fn bad_view(bad: &f32, _: &mut Cx<f32>) -> El {
            let mut k = Kit::neutral(true);
            k.radius = Radius { xs: *bad, sm: *bad, md: *bad, lg: *bad, xl: *bad };
            k.metrics.dialog = Some(DialogMetrics { radius: *bad, padding: *bad, title: None });
            k.metrics.popup_row = Some(PopupRowMetrics { height: *bad, radius: *bad });
            k.metrics.select = Some(SelectMetrics { chevron: *bad, chevron_box: *bad });
            k.metrics.popover = Some(PopoverMetrics { min_width: *bad, max_width: *bad });
            k.metrics.menu = Some(MenuMetrics {
                padding: *bad,
                standard_padding: *bad,
                separator_above: *bad,
                separator_below: *bad,
                standard_separator_above: *bad,
                standard_separator_below: *bad,
                standard_inner_border: *bad,
            });
            let mut layer = Vec::new();
            Dialog::new("d", "Title").open(true).fit().build(&k, &mut layer);
            let m = Menu::new("m")
                .standard()
                .padding(*bad)
                .separator_spacing(*bad, *bad)
                .inner_border(*bad)
                .trigger(Button::new("M").build(&k))
                .item(MenuItem::new("One").key("one"))
                .separator_keyed("   ")
                .item(MenuItem::new("Two").key("two"))
                .open(true)
                .build(&k, &mut layer);
            let sel = Select::new("s", &["A", "B"])
                .label("S")
                .appearance(SelectAppearance::Field)
                .current(Some("A"))
                .open(true)
                .build(&k, &mut layer);
            let pop = Popover::new("p")
                .trigger(Button::new("P").build(&k))
                .open(true)
                .child(text("Hi"))
                .build(&k, &mut layer);
            PopupList::<f32>::new("pl", "anchor")
                .row_radius(*bad)
                .items([PopupItem::new("a", "Alpha")])
                .open(true)
                .build(&k, &mut layer);
            root(col().child(el().key("anchor").size(40.0, 20.0)).child(m).child(sel).child(pop), layer)
        }
        let rt = settled(bad, bad_view, (640.0, 480.0));
        assert_eq!(find(&rt, "d-dialog").unwrap().radius, 0.0);
        let menu = find(&rt, "m-menu").unwrap();
        assert_eq!(menu.radius, 0.0);
        assert!(menu.border_color.is_none(), "bad={bad} must clamp inner_border to 0 (no border)");
        let rows = find_under(&rt, "m-menu", "rows").unwrap();
        assert_eq!(rows.inset, [0.0, 0.0, 0.0, 0.0], "bad={bad} must clamp padding to 0");
        assert!(find(&rt, "sep1").is_some(), "blank separator_keyed falls back to positional sep1");
        assert_eq!(find(&rt, "s-content").unwrap().radius, 0.0);
        assert!(find(&rt, "p-content").unwrap().rect[2].is_finite());
        assert!(find(&rt, "pl-content").is_some());
    }
}

#[test]
fn radius_scale_flows_into_overlay_defaults() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let mut k = Kit::neutral(true);
        k.radius = Radius { xs: 1.0, sm: 2.0, md: 11.0, lg: 14.0, xl: 16.0 };
        let mut layer = Vec::new();
        Dialog::new("dlg", "Dialog").open(true).fit().build(&k, &mut layer);
        let menu = Menu::new("menu")
            .trigger(Button::new("Menu").build(&k))
            .item(MenuItem::new("Item"))
            .open(true)
            .build(&k, &mut layer);
        let select = Select::new("sel", &["One", "Two"])
            .label("Select")
            .open(true)
            .build(&k, &mut layer);
        let list = PopupList::<()>::new("pl", "")
            .items([PopupItem::new("a", "Alpha")])
            .build_list(&k);
        root(col().gap(8.0).child(menu).child(select).child(list), layer)
    }

    let rt = settled((), view, (800.0, 600.0));
    assert_eq!(find(&rt, "dlg-dialog").unwrap().radius, 11.0);
    assert_eq!(find(&rt, "menu-menu").unwrap().radius, 11.0);
    assert_eq!(find(&rt, "menu-item0").unwrap().radius, 2.0);
    assert_eq!(find(&rt, "sel-content").unwrap().radius, 11.0);
    assert_eq!(find(&rt, "option0").unwrap().radius, 2.0);
    assert_eq!(find(&rt, "pl-option-a").unwrap().radius, 2.0);
}

#[test]
fn dialog_metrics_apply_radius_padding_title_and_keep_viewport_margin() {
    struct State {
        with_desc: bool,
        custom_title: bool,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut k = Kit::neutral(true);
        k.metrics.dialog = Some(DialogMetrics {
            radius: 14.0,
            padding: 10.0,
            title: s.custom_title.then(|| TextStyle::new(20.0, 600.0).lh(26.0)),
        });
        let mut layer = Vec::new();
        let mut dlg = Dialog::new("dlg", "Confirm").open(true).fit();
        if s.with_desc {
            dlg = dlg.description("Details");
        }
        dlg.child(el().w_full().h(400.0)).build(&k, &mut layer);
        root(el().w_full().h_full(), layer)
    }

    let rt = settled(State { with_desc: true, custom_title: true }, view, (360.0, 260.0));
    let panel = find(&rt, "dlg-dialog").unwrap();
    let header = find_under(&rt, "dlg-dialog", "header").unwrap();
    let title = rt.elements().into_iter().find(|e| e.text.as_deref() == Some("Confirm")).unwrap();
    assert_eq!(panel.radius, 14.0);
    assert_eq!(header.inset, [10.0, 10.0, 10.0, 10.0]);
    let ts = title.text_style.unwrap();
    assert_eq!((ts.size, ts.line_height), (20.0, 26.0));
    assert!((panel.rect[3] - (260.0 - 32.0)).abs() < 0.5, "16 pt viewport edge margin preserved: {:?}", panel.rect);

    let with_desc = settled(State { with_desc: true, custom_title: false }, view, (500.0, 400.0));
    let ts_desc = with_desc.elements().into_iter().find(|e| e.text.as_deref() == Some("Confirm")).unwrap().text_style.unwrap();
    assert_eq!((ts_desc.size, ts_desc.line_height), (15.0, 20.0));

    let no_desc = settled(State { with_desc: false, custom_title: false }, view, (500.0, 400.0));
    let ts_plain = no_desc.elements().into_iter().find(|e| e.text.as_deref() == Some("Confirm")).unwrap().text_style.unwrap();
    assert_eq!((ts_plain.size, ts_plain.line_height), (13.0, 16.0));
}

#[test]
fn compact_menu_follows_popup_row_metrics_for_hit_area_highlight_and_keys() {
    struct State {
        open: bool,
        row_h: f32,
        row_r: f32,
        picked: Vec<&'static str>,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut k = Kit::neutral(true);
        k.metrics.popup_row = Some(PopupRowMetrics { height: s.row_h, radius: s.row_r });
        let mut layer = Vec::new();
        let trigger = Menu::new("m")
            .trigger(Button::new("Open").on_press(|s: &mut State| s.open = true).build(&k))
            .group_label("Actions")
            .item(MenuItem::new("First").key("first").on_select(|s: &mut State| s.picked.push("first")))
            .item(MenuItem::new("Second").key("second").on_select(|s: &mut State| s.picked.push("second")))
            .item(MenuItem::new("Third").key("third").on_select(|s: &mut State| s.picked.push("third")))
            .open(s.open)
            .on_close(|s: &mut State| s.open = false)
            .build(&k, &mut layer);
        root(col().padding(20.0).child(trigger), layer)
    }

    for (h, r) in [(20.0, 2.0), (38.0, 9.0)] {
        let mut rt = settled(State { open: false, row_h: h, row_r: r, picked: Vec::new() }, view, (480.0, 480.0));
        rt.update(|s, _| s.open = true);
        rt.frame(1.0 / 60.0);
        let label = find(&rt, "label0").unwrap();
        let first = find(&rt, "first").unwrap();
        let second = find(&rt, "second").unwrap();
        assert_eq!(label.rect[3], h);
        assert_eq!((first.rect[3], first.radius), (h, r));
        assert_eq!((second.rect[3], second.radius), (h, r));
        assert!((second.rect[1] - (first.rect[1] + h)).abs() < 0.5);

        rt.key(KeyEvent { key: Key::Down, mods: Mods::default() });
        rt.frame(1.0 / 60.0);
        rt.key(KeyEvent { key: Key::Down, mods: Mods::default() });
        rt.frame(1.0 / 60.0);
        rt.key(KeyEvent { key: Key::Down, mods: Mods::default() });
        rt.frame(1.0 / 60.0);
        let third = find(&rt, "third").unwrap();
        assert!(third.focused, "third row focused at row_h={h}");
        assert_eq!(third.radius, r);
        rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.picked, ["third"]);

        let (cx, cy) = (second.rect[0] + second.rect[2] * 0.5, second.rect[1] + second.rect[3] * 0.5);
        rt.pointer_down(cx, cy);
        rt.pointer_up(cx, cy);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.picked, ["third", "second"]);
    }
}

#[test]
fn select_follows_popup_row_and_chevron_metrics_including_scroll_into_view() {
    struct State {
        open: bool,
        row_h: f32,
        row_r: f32,
        chevron: f32,
        chevron_box: f32,
        current: String,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut k = Kit::neutral(true);
        k.metrics.popup_row = Some(PopupRowMetrics { height: s.row_h, radius: s.row_r });
        k.metrics.select = Some(SelectMetrics { chevron: s.chevron, chevron_box: s.chevron_box });
        let mut layer = Vec::new();
        let options: Vec<String> = (0..20).map(|i| format!("Option {i:02}")).collect();
        let field = Select::new("long", &options)
            .label("Long list")
            .appearance(SelectAppearance::Field)
            .current(Some(&s.current))
            .open(s.open)
            .on_toggle(|s: &mut State| s.open = !s.open)
            .on_close(|s: &mut State| s.open = false)
            .on_choose(|s: &mut State, v, _| { s.current = v.into(); s.open = false; })
            .build(&k, &mut layer);
        let inline = Select::new("inl", &["Alpha", "Beta"]).label("Inline").current(Some("Alpha")).trigger(&k);
        root(col().padding(24.0).gap(12.0).child(field).child(inline), layer)
    }

    for (h, r, chev, chev_box) in [(20.0, 2.0, 12.0, 16.0), (40.0, 8.0, 18.0, 24.0)] {
        let mut rt = settled(
            State { open: false, row_h: h, row_r: r, chevron: chev, chevron_box: chev_box, current: "Option 00".into() },
            view,
            (600.0, 500.0),
        );
        rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
        rt.frame(1.0 / 60.0);
        rt.key(KeyEvent { key: Key::Down, mods: Mods::default() });
        rt.frame(1.0 / 60.0);

        let field_chev = find_under(&rt, "long-trigger", "chevron").unwrap();
        assert_eq!((field_chev.rect[2], field_chev.rect[3]), (chev, chev));
        let box_path = field_chev.path.rsplit_once('/').unwrap().0;
        let box_el = rt.elements().into_iter().find(|e| e.path == box_path).unwrap();
        assert_eq!((box_el.rect[2], box_el.rect[3]), (chev_box, chev_box));
        let inline_chev = find_under(&rt, "inl-trigger", "chevron").unwrap();
        assert_eq!((inline_chev.rect[2], inline_chev.rect[3]), (chev, chev));

        let opt0 = find(&rt, "option0").unwrap();
        let listbox = find(&rt, "long-content").unwrap();
        let expected_max_h = 4.0 + 6.0 * (h + 2.0) + (h - 6.0).max(0.0);
        assert_eq!((opt0.rect[3], opt0.radius), (h, r));
        assert!(
            (listbox.rect[3] - expected_max_h).abs() < 0.5,
            "listbox height {} != expected {expected_max_h} for row_h={h}",
            listbox.rect[3],
        );

        for _ in 0..15 {
            rt.key(KeyEvent { key: Key::Down, mods: Mods::default() });
            rt.frame(1.0 / 60.0);
        }
        rt.settle_all();
        rt.frame(0.0);
        let opt15 = find(&rt, "option15").unwrap();
        let lb = find(&rt, "long-content").unwrap();
        assert!(opt15.focused, "option15 focused");
        assert!(
            opt15.rect[1] >= lb.rect[1] - 0.5 && opt15.rect[1] + opt15.rect[3] <= lb.rect[1] + lb.rect[3] + 0.5,
            "option15 {:?} scrolled into listbox {:?}",
            opt15.rect,
            lb.rect,
        );
        rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.current, "Option 15");
    }
}

#[test]
fn popup_list_and_combobox_share_one_virtual_row_height_and_row_radius_override() {
    struct State {
        row_h: f32,
        row_r: f32,
        override_r: Option<f32>,
        count: usize,
        active: Option<usize>,
        chosen: Vec<String>,
        combo: ComboboxState,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut k = Kit::neutral(true);
        k.metrics.popup_row = Some(PopupRowMetrics { height: s.row_h, radius: s.row_r });
        let mut layer = Vec::new();
        let items: Vec<_> = (0..s.count).map(|i| PopupItem::new(format!("id-{i}"), format!("Item {i:02}"))).collect();
        let mut pl = PopupList::<State>::new("pl", "anchor")
            .open(true)
            .max_height(180.0)
            .items(items)
            .active(s.active)
            .on_choose(|s, id, _| s.chosen.push(id.into()));
        if let Some(r) = s.override_r {
            pl = pl.row_radius(r);
        }
        pl.build(&k, &mut layer);

        let opts: Vec<_> = (0..25).map(|i| ComboboxOption::new(format!("c-{i}"), format!("Choice {i:02}"))).collect();
        let cb = Combobox::new("cb", &s.combo, &opts, |s: &mut State| &mut s.combo).label("Choices").build(&k, &mut layer);
        root(col().padding(20.0).gap(12.0).child(el().key("anchor").size(100.0, 24.0)).child(cb), layer)
    }

    let mut rt = settled(
        State { row_h: 22.0, row_r: 3.0, override_r: None, count: 40, active: Some(0), chosen: Vec::new(), combo: ComboboxState::default() },
        view,
        (600.0, 500.0),
    );
    let r0 = find(&rt, "pl-option-id-0").unwrap();
    let vp = find(&rt, "viewport").unwrap();
    assert_eq!((r0.rect[3], r0.radius), (22.0, 3.0));
    assert_eq!(vp.rect[3], 180.0);

    let r1 = find(&rt, "pl-option-id-1").unwrap();
    rt.pointer_down(r1.rect[0] + 10.0, r1.rect[1] + r1.rect[3] * 0.5);
    rt.pointer_up(r1.rect[0] + 10.0, r1.rect[1] + r1.rect[3] * 0.5);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.chosen, ["id-1"]);

    rt.update(|s, _| s.active = Some(35));
    rt.frame(1.0 / 60.0);
    rt.settle_all();
    rt.frame(0.0);
    let r35 = find(&rt, "pl-option-id-35").expect("virtualized row 35 mounted and revealed");
    let vp = find(&rt, "viewport").unwrap();
    assert_eq!(r35.rect[3], 22.0);
    assert!(
        r35.rect[1] >= vp.rect[1] - 0.5 && r35.rect[1] + r35.rect[3] <= vp.rect[1] + vp.rect[3] + 0.5,
        "row 35 {:?} inside viewport {:?}",
        r35.rect,
        vp.rect,
    );

    rt.update(|s, _| { s.count = 3; s.active = Some(0); });
    rt.frame(1.0 / 60.0);
    assert_eq!(find(&rt, "viewport").unwrap().rect[3], 66.0);

    rt.update(|s, _| { s.count = 0; s.active = None; });
    rt.frame(1.0 / 60.0);
    assert_eq!(find(&rt, "empty").unwrap().rect[3], 22.0);

    rt.update(|s, _| { s.count = 2; s.active = Some(0); s.override_r = Some(9.0); });
    rt.frame(1.0 / 60.0);
    assert_eq!(find(&rt, "pl-option-id-0").unwrap().radius, 9.0);

    rt.update(|s, _| {
        let _ = s.combo.step(ComboboxEvent::Toggle { current: None });
    });
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    let cb0 = find(&rt, "cb-option-c-0").unwrap();
    assert_eq!((cb0.rect[3], cb0.radius), (22.0, 3.0));
}

#[test]
fn popover_width_limits_respect_content_and_clamp_inversions() {
    struct State {
        min_w: f32,
        max_w: f32,
        child_w: f32,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut k = Kit::neutral(true);
        k.metrics.popover = Some(PopoverMetrics { min_width: s.min_w, max_width: s.max_w });
        let mut layer = Vec::new();
        let trigger = Popover::new("pop")
            .trigger(Button::new("Open").build(&k))
            .padding(10.0)
            .open(true)
            .child(el().size(s.child_w, 20.0))
            .build(&k, &mut layer);
        root(col().padding(20.0).child(trigger), layer)
    }

    let narrow = settled(State { min_w: 120.0, max_w: 240.0, child_w: 40.0 }, view, (600.0, 400.0));
    assert_eq!(find(&narrow, "pop-content").unwrap().rect[2], 120.0);

    let mid = settled(State { min_w: 120.0, max_w: 240.0, child_w: 160.0 }, view, (600.0, 400.0));
    assert_eq!(find(&mid, "pop-content").unwrap().rect[2], 180.0);

    let wide = settled(State { min_w: 120.0, max_w: 240.0, child_w: 400.0 }, view, (600.0, 400.0));
    assert_eq!(find(&wide, "pop-content").unwrap().rect[2], 240.0);

    let inv = settled(State { min_w: 260.0, max_w: 180.0, child_w: 40.0 }, view, (600.0, 400.0));
    assert_eq!(find(&inv, "pop-content").unwrap().rect[2], 260.0);
}

#[test]
fn menu_metrics_builders_padding_zero_border_zero_submenu_inheritance_and_clamping() {
    struct State {
        standard: bool,
        pad_override: Option<f32>,
        sep_override: Option<(f32, f32)>,
        border_override: Option<f32>,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut k = Kit::neutral(true);
        k.metrics.menu = Some(MenuMetrics {
            padding: 6.0,
            standard_padding: 10.0,
            separator_above: 3.0,
            separator_below: 7.0,
            standard_separator_above: 5.0,
            standard_separator_below: 9.0,
            standard_inner_border: 0.0,
        });
        let mut layer = Vec::new();
        let sub = vec![
            MenuEntry::item(MenuItem::new("Sub A").key("sub-a")),
            MenuEntry::Separator,
            MenuEntry::item(MenuItem::new("Sub B").key("sub-b")),
        ];
        let mut menu = Menu::new("m")
            .trigger(Button::new("Menu").build(&k))
            .item(MenuItem::new("Top").key("top"))
            .separator_keyed("main-sep")
            .item(MenuItem::new("More").key("more").submenu(sub, true))
            .open(true);
        if s.standard {
            menu = menu.standard();
        }
        if let Some(p) = s.pad_override {
            menu = menu.padding(p);
        }
        if let Some((a, b)) = s.sep_override {
            menu = menu.separator_spacing(a, b);
        }
        if let Some(bw) = s.border_override {
            menu = menu.inner_border(bw);
        }
        let trigger = menu.build(&k, &mut layer);
        root(col().padding(20.0).child(trigger), layer)
    }

    // 1. Kit::metrics.menu on compact menu: pad=6, sep above=3/below=7, separator flush with rows box.
    let compact = settled(
        State { standard: false, pad_override: None, sep_override: None, border_override: None },
        view,
        (700.0, 500.0),
    );
    let rows = find_under(&compact, "m-menu", "rows").unwrap();
    let top = find(&compact, "top").unwrap();
    let sep = find(&compact, "main-sep").unwrap();
    let more = find(&compact, "more").unwrap();
    assert_eq!(rows.inset, [6.0, 6.0, 6.0, 6.0]);
    assert_eq!(sep.rect[0], rows.rect[0], "separator stays flush left under any padding");
    assert_eq!(sep.rect[2], rows.rect[2], "separator spans full rows width");
    assert!((sep.rect[1] - (top.rect[1] + top.rect[3]) - 3.0).abs() < 0.5, "3 pt above separator");
    assert!((more.rect[1] - (sep.rect[1] + sep.rect[3]) - 7.0).abs() < 0.5, "7 pt below separator");
    assert!(find(&compact, "m-menu").unwrap().border_color.is_none());

    // 2. padding(0.0): zero inset and separator flush with rows box.
    let zero_pad = settled(
        State { standard: false, pad_override: Some(0.0), sep_override: Some((4.0, 4.0)), border_override: None },
        view,
        (700.0, 500.0),
    );
    let zp_rows = find_under(&zero_pad, "m-menu", "rows").unwrap();
    let zp_top = find(&zero_pad, "top").unwrap();
    let zp_sep = find(&zero_pad, "main-sep").unwrap();
    assert_eq!(zp_rows.inset, [0.0, 0.0, 0.0, 0.0]);
    assert_eq!(zp_top.rect[0], zp_rows.rect[0]);
    assert_eq!((zp_sep.rect[0], zp_sep.rect[2]), (zp_rows.rect[0], zp_rows.rect[2]));

    // 3. inner_border(0.0) omits border with zero gap and zero 1-pt border inset on the frame.
    let std_no_border = settled(
        State { standard: true, pad_override: None, sep_override: None, border_override: None },
        view,
        (700.0, 500.0),
    );
    let std_with_border = settled(
        State { standard: true, pad_override: None, sep_override: None, border_override: Some(1.0) },
        view,
        (700.0, 500.0),
    );
    let frame0 = find(&std_no_border, "m-menu").unwrap();
    let rows0 = find_under(&std_no_border, "m-menu", "rows").unwrap();
    let frame1 = find(&std_with_border, "m-menu").unwrap();
    let rows1 = find_under(&std_with_border, "m-menu", "rows").unwrap();
    assert!(frame0.border_color.is_none(), "standard_inner_border 0.0 omits border");
    assert_eq!(frame0.inset, [0.0, 0.0, 0.0, 0.0], "inner_border(0) leaves zero border inset on frame");
    assert_eq!(rows0.rect[0], frame0.rect[0], "no horizontal gap or 1-pt shift when inner_border is 0");
    assert_eq!(rows0.rect[1], frame0.rect[1], "no vertical gap or 1-pt shift when inner_border is 0");
    assert!(frame1.border_color.is_some());
    assert_eq!(rows0.rect, rows1.rect, "inner_border(0) leaves no gap and no 1-pt shift of rows");
    assert_eq!(frame0.rect, frame1.rect, "frame size stays identical when inner_border toggles 1 -> 0");

    // 4. Per-menu builders override Kit::metrics.menu AND propagate to open submenus.
    let overridden = settled(
        State { standard: true, pad_override: Some(12.0), sep_override: Some((4.0, 10.0)), border_override: Some(2.0) },
        view,
        (700.0, 500.0),
    );
    assert!(find(&overridden, "m-menu").unwrap().border_color.is_some());
    assert!(find(&overridden, "more-sub-menu").unwrap().border_color.is_some(), "inner_border propagates to submenu");
    let sub_rows = find_under(&overridden, "more-sub-menu", "rows").unwrap();
    let sub_a = find(&overridden, "sub-a").unwrap();
    let sub_sep = find_under(&overridden, "more-sub-menu", "sep1").unwrap();
    let sub_b = find(&overridden, "sub-b").unwrap();
    assert_eq!(sub_rows.inset, [12.0, 12.0, 12.0, 12.0], "padding propagates to submenu");
    assert_eq!((sub_sep.rect[0], sub_sep.rect[2]), (sub_rows.rect[0], sub_rows.rect[2]));
    assert!((sub_sep.rect[1] - (sub_a.rect[1] + sub_a.rect[3]) - 4.0).abs() < 0.5, "above spacing propagates");
    assert!((sub_b.rect[1] - (sub_sep.rect[1] + sub_sep.rect[3]) - 10.0).abs() < 0.5, "below spacing propagates");

    // 5. Point-anchored menu still clamps cleanly inside a small window under roomy padding.
    fn clamped_view(_: &(), _: &mut Cx<()>) -> El {
        let mut k = Kit::neutral(true);
        k.metrics.menu = Some(MenuMetrics {
            padding: 12.0,
            separator_above: 8.0,
            separator_below: 8.0,
            ..MenuMetrics::kit(&k)
        });
        let mut layer = Vec::new();
        let mut menu = Menu::new("pm").at(260.0, 200.0).open(true);
        for i in 0..10 {
            if i == 4 {
                menu = menu.separator_keyed("mid");
            }
            menu = menu.item(MenuItem::new(format!("Action {i}")));
        }
        let _ = menu.build(&k, &mut layer);
        root(el().w_full().h_full(), layer)
    }
    let rt = settled((), clamped_view, (300.0, 240.0));
    let frame = find(&rt, "pm-menu").unwrap();
    assert!(frame.rect[0] >= 0.0 && frame.rect[0] + frame.rect[2] <= 300.5, "x clamped: {:?}", frame.rect);
    assert!(frame.rect[1] >= 0.0 && frame.rect[1] + frame.rect[3] <= 240.5, "y clamped: {:?}", frame.rect);
}

#[test]
fn keyed_separator_identity_blank_fallback_and_duplicate_keys() {
    struct State {
        prepend: bool,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        let mut menu = Menu::new("edit").trigger(Button::new("Edit").build(&k)).open(true);
        if s.prepend {
            menu = menu.item(MenuItem::new("Inserted").key("inserted"));
        }
        let trigger = menu
            .item(MenuItem::new("Add").key("add"))
            .separator_keyed("after-add")
            .item(MenuItem::new("Arrange").key("arrange"))
            .separator()
            .item(MenuItem::new("Clean").key("clean"))
            .separator_keyed("")
            .item(MenuItem::new("Delete").key("delete"))
            .build(&k, &mut layer);
        root(col().padding(20.0).child(trigger), layer)
    }

    let mut rt = settled(State { prepend: false }, view, (400.0, 400.0));
    let before = find(&rt, "after-add").expect("keyed separator present");
    assert_eq!(before.opacity, 1.0);
    assert!(find(&rt, "sep3").is_some(), "un-keyed separator keeps positional sep3");
    assert!(find(&rt, "sep5").is_some(), "blank separator_keyed falls back to positional sep5");

    rt.update(|s, _| s.prepend = true);
    rt.frame(1.0 / 60.0);
    let after = find(&rt, "after-add").expect("keyed separator kept its identity");
    assert_eq!(after.path, before.path, "same element path");
    assert_eq!(after.opacity, 1.0, "no entrance fade replayed");
    assert!(!after.exiting, "not exiting/recreated");
    assert!(
        rt.elements().iter().all(|e| !(e.key.as_deref() == Some("after-add") && e.exiting)),
        "no old separator left fading out",
    );
    assert!(find(&rt, "sep4").is_some(), "un-keyed separator moved to positional sep4 as before");
    assert!(find(&rt, "sep6").is_some(), "blank separator_keyed moved to positional sep6");

    // Two separators given the same key do not panic or silently drop either: Rune renames the
    // second sibling to `dup#dup1` and renders both.
    fn dup_view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        let trigger = Menu::new("dup-menu")
            .trigger(Button::new("M").build(&k))
            .item(MenuItem::new("A").key("a"))
            .separator_keyed("dup")
            .item(MenuItem::new("B").key("b"))
            .separator_keyed("dup")
            .item(MenuItem::new("C").key("c"))
            .open(true)
            .build(&k, &mut layer);
        root(col().padding(20.0).child(trigger), layer)
    }
    let dup_rt = settled((), dup_view, (400.0, 400.0));
    let first_dup = find(&dup_rt, "dup").expect("first duplicate-keyed separator present");
    let second_dup = find(&dup_rt, "dup#dup1").expect("second duplicate-keyed separator renamed by Rune, not dropped");
    assert!(second_dup.rect[1] > first_dup.rect[1], "both separators lay out distinctly");
}

#[test]
fn idle_frames_with_custom_overlay_metrics_do_not_commit_or_request_frames() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let mut k = Kit::neutral(true);
        k.metrics.dialog = Some(DialogMetrics { padding: 12.0, ..DialogMetrics::kit(&k) });
        k.metrics.popup_row = Some(PopupRowMetrics { height: 24.0, ..PopupRowMetrics::kit(&k) });
        k.metrics.select = Some(SelectMetrics { chevron: 14.0, ..SelectMetrics::kit(&k) });
        k.metrics.popover = Some(PopoverMetrics { min_width: 160.0, ..PopoverMetrics::kit(&k) });
        k.metrics.menu = Some(MenuMetrics { padding: 4.0, ..MenuMetrics::kit(&k) });
        let mut layer = Vec::new();
        let m = Menu::new("m")
            .trigger(Button::new("M").build(&k))
            .item(MenuItem::new("A"))
            .separator_keyed("s")
            .item(MenuItem::new("B"))
            .open(true)
            .build(&k, &mut layer);
        root(col().padding(20.0).child(m), layer)
    }

    let mut rt = settled((), view, (480.0, 360.0));
    let commits_before = rt.stats.commits;
    for _ in 0..5 {
        assert!(!rt.frame(1.0 / 60.0), "settled menu with custom metrics must not animate on idle frames");
    }
    assert_eq!(rt.stats.commits, commits_before, "idle frames must not commit");
    assert!(rt.wake_deadline().is_none(), "settled menu schedules no idle wake");
}
