use rune::render::{Kind, unpack_fade};
use rune::{Cx, Direction, El, Env, PointerPress, Runtime, col, el};
use rune_kit::shell::{TabItem, TabOrientation};
use rune_kit::{Button, Keybind, Kit, Menu, MenuEntry, MenuItem, TabStrip, context_menu, root};

fn settle<S: 'static>(rt: &mut Runtime<S>) {
    rt.frame(0.0);
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
}

fn glyphs_in<S>(rt: &Runtime<S>, rect: [f32; 4]) -> Vec<([f32; 4], [f32; 4])> {
    let [rx, ry, rw, rh] = rect;
    let mut out: Vec<([f32; 4], [f32; 4])> = rt
        .instances
        .iter()
        .filter(|inst| inst.params[1] == Kind::Glyph.as_f32())
        .filter(|inst| {
            let cx = inst.rect[0] + inst.rect[2] * 0.5;
            let cy = inst.rect[1] + inst.rect[3] * 0.5;
            cx >= rx - 1.0 && cx <= rx + rw + 1.0 && cy >= ry - 1.0 && cy <= ry + rh + 1.0
        })
        .map(|inst| (inst.rect, unpack_fade(inst.fade).0))
        .collect();
    out.sort_by(|a, b| a.0[0].total_cmp(&b.0[0]));
    out
}

#[test]
fn rtl_context_menu_opens_at_physical_pointer_not_mirrored() {
    #[derive(Default)]
    struct S { point: Option<PointerPress> }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let k = Kit::neutral(true);
        let area = el().key("area").w_full().h_full().on_secondary_press(|s: &mut S, p, _| s.point = Some(p));
        let mut layer = Vec::new();
        let body = context_menu("ctx", area, s.point)
            .item(MenuItem::new("Copy"))
            .on_close(|s: &mut S| s.point = None)
            .build(&k, &mut layer);
        root(body, layer).dir(Direction::Rtl)
    }
    let mut rt = Runtime::new(S::default(), view, (600.0, 300.0), Env::new(true, 1.0));
    settle(&mut rt);
    // Right-click near the right side at physical x=500.
    rt.secondary_down(500.0, 80.0);
    rt.secondary_up(500.0, 80.0);
    settle(&mut rt);
    let point = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("ctx-point")).expect("point proxy");
    assert!((point.rect[0] - 500.0).abs() < 1.0, "point proxy must stay unmirrored at x=500: {:?}", point.rect);
    let menu = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("ctx-menu")).expect("menu mounted");
    let right = menu.rect[0] + menu.rect[2];
    assert!((right - 500.0).abs() < 1.5, "RTL context menu opens leftward from x=500, got {:?}", menu.rect);
}

#[test]
fn menu_and_submenu_grow_past_min_width_for_long_items_and_clamp_in_narrow_window() {
    struct S;
    fn view(_: &S, _: &mut Cx<S>) -> El {
        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        let sub_item = MenuItem::new("A very long submenu option label that exceeds 160 points easily");
        let body = Menu::new("wide")
            .trigger(el().key("trig").size(40.0, 24.0))
            .item(MenuItem::new("Short").shortcut("Ctrl+S"))
            .item(MenuItem::new("A very long top-level menu item label that exceeds 160 points").shortcut("Ctrl+Shift+P"))
            .item(MenuItem::new("More").submenu(vec![MenuEntry::item(sub_item)], true))
            .open(true)
            .build(&k, &mut layer);
        root(col().padding(16.0).child(body), layer)
    }

    let mut rt = Runtime::new(S, view, (1200.0, 400.0), Env::new(true, 1.0));
    settle(&mut rt);
    let menu = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("wide-menu")).unwrap();
    assert!(menu.rect[2] > 260.0, "menu should grow past 160pt min_width to fit long item, got {}", menu.rect[2]);
    let long_label = rt.elements().into_iter().find(|e| e.text.as_deref() == Some("A very long top-level menu item label that exceeds 160 points")).unwrap();
    assert!(!long_label.text_cut, "long menu item label must not truncate when window has room");

    let sub = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("wide-item2-sub-menu")).unwrap();
    assert!(sub.rect[2] > 240.0, "submenu should grow past 160pt min_width to fit long item, got {}", sub.rect[2]);
    let sub_label = rt.elements().into_iter().find(|e| e.text.as_deref() == Some("A very long submenu option label that exceeds 160 points easily")).unwrap();
    assert!(!sub_label.text_cut, "long submenu item label must not truncate when window has room: menu={:?} sub={:?} label={:?} text_size={:?}", menu.rect, sub.rect, sub_label.rect, sub_label.text_size);

    // Inside a narrow 220pt window, the menu still shrinks and clamps inside bounds.
    let mut narrow = Runtime::new(S, view, (220.0, 300.0), Env::new(true, 1.0));
    settle(&mut narrow);
    let narrow_menu = narrow.elements().into_iter().find(|e| e.key.as_deref() == Some("wide-menu")).unwrap();
    assert!(narrow_menu.rect[0] >= 0.0 && narrow_menu.rect[0] + narrow_menu.rect[2] <= 220.5, "narrow menu clamps: {:?}", narrow_menu.rect);
}

#[test]
fn shortcuts_read_left_to_right_in_rtl_menus_buttons_and_keybinds() {
    fn view(rtl: &bool, _: &mut Cx<bool>) -> El {
        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        let menu = Menu::new("sc")
            .trigger(el().key("trig").size(40.0, 24.0))
            .item(MenuItem::new("Zoom In").key("eq").shortcut("Ctrl+="))
            .item(MenuItem::new("Back").key("arrow").shortcut("Alt+←"))
            .open(true)
            .build(&k, &mut layer);
        let btn = Button::new("Zoom").shortcut("Ctrl+=").show_shortcut().build(&k).key("btn");
        let kb = Keybind::new(&["Ctrl", "Shift", "S"]).build(&k).key("kb");
        let body = col().padding(16.0).gap(8.0).child(menu).child(btn).child(kb);
        let r = root(body, layer);
        if *rtl { r.dir(Direction::Rtl) } else { r }
    }

    let mut ltr = Runtime::new(false, view, (500.0, 300.0), Env::new(true, 1.0));
    let mut rtl = Runtime::new(true, view, (500.0, 300.0), Env::new(true, 1.0));
    settle(&mut ltr);
    settle(&mut rtl);

    // Compare relative glyph positions & sizes inside both menu shortcuts and button shortcut.
    for (item_key, label) in [("eq", "Ctrl+="), ("arrow", "Alt+←")] {
        let ltr_el = ltr.elements().into_iter().find(|e| e.key.as_deref() == Some("shortcut") && e.path.contains(item_key)).unwrap();
        let rtl_el = rtl.elements().into_iter().find(|e| e.key.as_deref() == Some("shortcut") && e.path.contains(item_key)).unwrap();
        let lg = glyphs_in(&ltr, ltr_el.rect);
        let rg = glyphs_in(&rtl, rtl_el.rect);
        assert!(!lg.is_empty(), "{label} should emit glyphs");
        assert_eq!(lg.len(), rg.len(), "{label} glyph count differs in RTL");
        let l0 = lg[0].0[0];
        let r0 = rg[0].0[0];
        for (i, ((lr, _), (rr, _))) in lg.iter().zip(rg.iter()).enumerate() {
            assert!(((lr[0] - l0) - (rr[0] - r0)).abs() <= 1.1 && (lr[2] - rr[2]).abs() <= 1.1 && (lr[3] - rr[3]).abs() <= 1.1,
                "{label} glyph {i} reversed in RTL: LTR={lr:?} RTL={rr:?}\n  all LTR={lg:?}\n  all RTL={rg:?}");
        }
    }

    // Button inline shortcut also stays LTR.
    let ltr_btn_sc = ltr.elements().into_iter().find(|e| e.key.as_deref() == Some("shortcut") && e.path.contains("/btn/")).unwrap();
    let rtl_btn_sc = rtl.elements().into_iter().find(|e| e.key.as_deref() == Some("shortcut") && e.path.contains("/btn/")).unwrap();
    let lg = glyphs_in(&ltr, ltr_btn_sc.rect);
    let rg = glyphs_in(&rtl, rtl_btn_sc.rect);
    assert_eq!(lg.len(), rg.len());
    let (l0, r0) = (lg[0].0[0], rg[0].0[0]);
    for ((lr, _), (rr, _)) in lg.iter().zip(rg.iter()) {
        assert!(((lr[0] - l0) - (rr[0] - r0)).abs() <= 1.1 && (lr[3] - rr[3]).abs() <= 1.1);
    }

    // Keybind keeps keycap order left-to-right in RTL (key0 < key1 < key2).
    let k0 = rtl.elements().into_iter().find(|e| e.path.ends_with("/kb/key0")).unwrap();
    let k1 = rtl.elements().into_iter().find(|e| e.path.ends_with("/kb/key1")).unwrap();
    let k2 = rtl.elements().into_iter().find(|e| e.path.ends_with("/kb/key2")).unwrap();
    assert!(k0.rect[0] < k1.rect[0] && k1.rect[0] < k2.rect[0], "Keybind keycaps must stay LTR in RTL: {:?}, {:?}, {:?}", k0.rect, k1.rect, k2.rect);
}

#[test]
fn rtl_tab_strip_fades_latin_titles_at_right_and_arabic_titles_at_left() {
    struct S { active: String, rtl: bool }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let k = Kit::neutral(true);
        let items = vec![
            TabItem::new("latin", "README-long-latin-title-that-reaches-the-fade.md"),
            TabItem::new("arabic", "مرحبا بالعالم في شريط التبويبات الطويل جدا"),
        ];
        let strip = TabStrip::new("tabs", TabOrientation::Horizontal)
            .tabs(items)
            .viewport_width(Some(360.0))
            .active(Some(s.active.as_str()))
            .on_close(|_: &mut S, _, _| {})
            .build(&k);
        let root = col().w(360.0).h(40.0).child(strip);
        if s.rtl { root.dir(Direction::Rtl) } else { root }
    }

    for rtl in [false, true] {
        // Active Latin tab: fades at physical RIGHT edge (widths[2] < 0), NEVER left (widths[0] == 0).
        let mut rt = Runtime::new(S { active: "latin".into(), rtl }, view, (360.0, 40.0), Env::new(true, 1.0));
        settle(&mut rt);
        let label = rt.elements().into_iter().find(|e| e.text.as_deref() == Some("README-long-latin-title-that-reaches-the-fade.md")).unwrap();
        let gs = glyphs_in(&rt, label.rect);
        assert!(!gs.is_empty(), "Latin title should emit glyphs (rtl={rtl})");
        let fade = gs[0].1;
        assert_eq!(fade[0], 0.0, "Latin title must NOT fade on left edge (rtl={rtl}): {fade:?}");
        assert!(fade[2] < -1.0, "Latin title must fade linearly on right edge (rtl={rtl}): {fade:?}");

        // Active Arabic tab: fades at physical LEFT edge (widths[0] < 0), NEVER right (widths[2] == 0).
        let mut rt = Runtime::new(S { active: "arabic".into(), rtl }, view, (360.0, 40.0), Env::new(true, 1.0));
        settle(&mut rt);
        let label = rt.elements().into_iter().find(|e| e.text.as_deref() == Some("مرحبا بالعالم في شريط التبويبات الطويل جدا")).unwrap();
        let gs = glyphs_in(&rt, label.rect);
        assert!(!gs.is_empty(), "Arabic title should emit glyphs (rtl={rtl})");
        let fade = gs[0].1;
        assert!(fade[0] < -1.0, "Arabic title must fade linearly on left edge (rtl={rtl}): {fade:?}");
        assert_eq!(fade[2], 0.0, "Arabic title must NOT fade on right edge (rtl={rtl}): {fade:?}");

        // Hovered inactive Latin tab in RTL: hovered fading row also fades on RIGHT, never left.
        if rtl {
            let mut rt = Runtime::new(S { active: "arabic".into(), rtl: true }, view, (360.0, 40.0), Env::new(true, 1.0));
            settle(&mut rt);
            let (tx, ty) = rt.find("t-latin").unwrap();
            rt.pointer_move(tx, ty);
            settle(&mut rt);
            let hovered = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("hovered") && e.path.contains("t-latin")).unwrap();
            let gs = glyphs_in(&rt, hovered.rect);
            assert!(!gs.is_empty(), "Hovered Latin title should emit glyphs");
            let fade = gs.last().unwrap().1;
            assert_eq!(fade[0], 0.0, "Hovered Latin title in RTL must NOT fade on left edge: {fade:?}");
            assert!(fade[2] < -1.0, "Hovered Latin title in RTL must fade linearly on right edge: {fade:?}");
        }
    }
}
