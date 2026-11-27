use rune::render::KIND_BACKDROP_BLUR;
use rune::{Color, Cx, El, Env, PointerPress, Runtime, col, el, text};
use rune_kit::{
    Button, Combobox, ComboboxEvent, ComboboxOption, ComboboxState, HoverCard, Kit, Menu,
    MenuEntry, MenuItem, Placement, Popover, PopupBackdrop, PopupItem, PopupList, Select,
    context_menu, popover, root,
};

fn find<S>(rt: &Runtime<S>, key: &str) -> rune::runtime::ElementInfo {
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing element {key}"))
}

fn blur_instance_count<S>(rt: &Runtime<S>) -> usize {
    rt.instances.iter().filter(|i| i.params[1] == KIND_BACKDROP_BLUR).count()
}

fn settled<S: 'static>(state: S, view: fn(&S, &mut Cx<S>) -> El) -> Runtime<S> {
    let mut rt = Runtime::new(state, view, (900.0, 640.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.settle_all();
    rt.frame(0.0);
    rt
}

struct AllState {
    backdrop: Option<PopupBackdrop>,
    combo: ComboboxState,
}

fn all_state(backdrop: Option<PopupBackdrop>) -> AllState {
    let mut combo = ComboboxState::default();
    let _ = combo.step(ComboboxEvent::Toggle { current: None });
    AllState { backdrop, combo }
}

fn all_overlays_view(s: &AllState, _: &mut Cx<AllState>) -> El {
    let mut k = Kit::neutral(true);
    k.metrics.popup_backdrop = s.backdrop;
    let mut layer = Vec::new();

    let compact_menu = Menu::new("cm")
        .trigger(Button::new("Compact").build(&k))
        .item(
            MenuItem::new("Share")
                .key("cm-share")
                .submenu(vec![MenuEntry::item(MenuItem::new("Copy link").key("cm-copy"))], true),
        )
        .open(true)
        .build(&k, &mut layer);

    let standard_menu = Menu::new("sm")
        .standard()
        .trigger(Button::new("Standard").build(&k))
        .item(
            MenuItem::new("More")
                .key("sm-more")
                .submenu(vec![MenuEntry::item(MenuItem::new("Export").key("sm-export"))], true),
        )
        .open(true)
        .build(&k, &mut layer);

    let select = Select::new("sel", &["One", "Two"])
        .label("Choice")
        .current(Some("One"))
        .open(true)
        .build(&k, &mut layer);

    PopupList::<AllState>::new("pl", "pl-anchor")
        .items([PopupItem::new("a", "Alpha")])
        .open(true)
        .build(&k, &mut layer);

    let pop = Popover::new("pop")
        .trigger(Button::new("Popover").build(&k))
        .open(true)
        .child(text("Popover body"))
        .build(&k, &mut layer);

    let combo_opts = [ComboboxOption::new("m1", "Model 1")];
    let combo = Combobox::new("cb", &s.combo, &combo_opts, |s: &mut AllState| &mut s.combo)
        .label("Model")
        .build(&k, &mut layer);

    let card = HoverCard::new("hc")
        .trigger(Button::new("Hover").build(&k))
        .open(true)
        .child(text("Card body"))
        .build(&k, &mut layer);

    let custom_surface = popover::raised(
        &k,
        col().key("custom-raised").w(140.0).h(48.0).child(text("Custom")),
        k.radius.md,
    );
    layer.push(popover::anchored(&k, custom_surface, "custom-anchor", Placement::BottomStart, 4.0));

    root(
        col()
            .padding(16.0)
            .gap(8.0)
            .child(compact_menu)
            .child(standard_menu)
            .child(select)
            .child(el().key("pl-anchor").size(40.0, 20.0))
            .child(pop)
            .child(combo)
            .child(card)
            .child(el().key("custom-anchor").size(40.0, 20.0)),
        layer,
    )
}

#[test]
fn default_and_explicit_none_popup_backdrop_leave_surfaces_unchanged() {
    let k = Kit::neutral(true);
    assert_eq!(PopupBackdrop::kit(&k), PopupBackdrop::NONE);
    assert_eq!(PopupBackdrop::NONE, PopupBackdrop { sigma: 0.0, fill_alpha: 1.0, fill: None });

    let rt_default = settled(all_state(None), all_overlays_view);
    let rt_none = settled(all_state(Some(PopupBackdrop::NONE)), all_overlays_view);

    assert_eq!(blur_instance_count(&rt_default), 0, "default emits zero backdrop blur instances");
    assert_eq!(blur_instance_count(&rt_none), 0, "PopupBackdrop::NONE emits zero backdrop blur instances");

    for (key, expected_fill) in [
        ("cm-menu", k.c.layer_01),
        ("cm-share-sub-menu", k.c.layer_01),
        ("sm-menu", k.c.surface_raised),
        ("sm-more-sub-menu", k.c.surface_raised),
        ("sel-content", k.c.layer_01),
        ("pl-content", k.c.surface_raised),
        ("pop-content", k.c.surface_raised),
        ("cb-content", k.c.surface_raised),
        ("hc-card", k.c.surface_raised),
        ("custom-raised", k.c.surface_raised),
    ] {
        let def = find(&rt_default, key);
        let exp = find(&rt_none, key);
        assert_eq!(def.backdrop_blur, 0.0, "{key} default blur off");
        assert_eq!(exp.backdrop_blur, 0.0, "{key} explicit NONE blur off");
        assert_eq!(def.fill, expected_fill, "{key} default fill unchanged");
        assert_eq!(exp.fill, expected_fill, "{key} explicit NONE fill unchanged");
    }

    assert_eq!(
        rt_default.instances.len(),
        rt_none.instances.len(),
        "PopupBackdrop::NONE matches default paint instance count",
    );
}

#[test]
fn theme_popup_backdrop_frosted_and_tinted_apply_across_all_transient_surfaces() {
    let k = Kit::neutral(true);
    let rt = settled(all_state(Some(PopupBackdrop::frosted(20.0, 0.78))), all_overlays_view);

    let layer_tint = k.alpha(k.c.layer_01, k.c.layer_01.a * 0.78);
    let raised_tint = k.alpha(k.c.surface_raised, k.c.surface_raised.a * 0.78);

    // Every open transient surface blurs and tints its theme fill.
    for (key, expected_fill) in [
        ("cm-menu", layer_tint),
        ("cm-share-sub-menu", layer_tint),
        ("sm-menu", raised_tint),
        ("sm-more-sub-menu", raised_tint),
        ("sel-content", layer_tint),
        ("pl-content", raised_tint),
        ("pop-content", raised_tint),
        ("cb-content", raised_tint),
        ("hc-card", raised_tint),
        ("custom-raised", raised_tint),
    ] {
        let info = find(&rt, key);
        assert_eq!(info.backdrop_blur, 20.0, "{key} inherits theme backdrop_blur");
        assert_eq!(info.fill, expected_fill, "{key} tints theme fill by fill_alpha");
    }
    // Triggers stay unblurred.
    assert_eq!(find(&rt, "sel-trigger").backdrop_blur, 0.0);
    assert!(blur_instance_count(&rt) >= 10, "every frosted surface emits a BackdropBlur instance");

    // Explicit tint colour replaces every surface fill uniformly.
    let acrylic = Color { r: 0.14, g: 0.16, b: 0.20, a: 0.72 };
    let rt_tinted = settled(all_state(Some(PopupBackdrop::tinted(24.0, acrylic))), all_overlays_view);
    for key in [
        "cm-menu",
        "cm-share-sub-menu",
        "sm-menu",
        "sm-more-sub-menu",
        "sel-content",
        "pl-content",
        "pop-content",
        "cb-content",
        "hc-card",
        "custom-raised",
    ] {
        let info = find(&rt_tinted, key);
        assert_eq!(info.backdrop_blur, 24.0, "{key} tinted sigma");
        assert_eq!(info.fill, acrylic, "{key} tinted fill");
    }
}

#[test]
fn menu_backdrop_blur_and_surface_fill_propagate_to_nested_submenus_and_context_menus() {
    const TINT: Color = Color { r: 0.10, g: 0.12, b: 0.16, a: 0.76 };

    fn view(_: &(), _: &mut Cx<()>) -> El {
        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        let nested = MenuEntry::item(MenuItem::new("Leaf").key("leaf"));
        let mid = MenuEntry::item(MenuItem::new("Deep").key("deep").submenu(vec![nested], true));
        let top = MenuItem::new("Share").key("share").submenu(vec![mid], true);

        let ctx = context_menu("ctx", el().key("target").size(120.0, 40.0), Some(PointerPress { x: 40.0, y: 20.0 }))
            .backdrop_blur(18.0)
            .surface_fill(TINT)
            .item(top)
            .build(&k, &mut layer);

        let plain = Menu::new("plain")
            .trigger(Button::new("Plain").build(&k))
            .item(MenuItem::new("Solid"))
            .open(true)
            .build(&k, &mut layer);

        root(col().gap(12.0).child(ctx).child(plain), layer)
    }

    let rt = settled((), view);
    for key in ["ctx-menu", "share-sub-menu", "deep-sub-menu"] {
        let info = find(&rt, key);
        assert_eq!(info.backdrop_blur, 18.0, "{key} inherits Menu::backdrop_blur");
        assert_eq!(info.fill, TINT, "{key} inherits Menu::surface_fill");
    }
    let plain = find(&rt, "plain-menu");
    assert_eq!(plain.backdrop_blur, 0.0, "sibling menu stays unblurred");
    assert_eq!(plain.fill, Kit::neutral(true).c.layer_01);
    assert_eq!(blur_instance_count(&rt), 3, "only the frosted menu and its two open submenus emit blur");
}

#[test]
fn per_instance_builders_override_theme_popup_backdrop_including_opt_out() {
    const SOLID: Color = Color { r: 0.22, g: 0.22, b: 0.24, a: 1.0 };
    const CUSTOM_TINT: Color = Color { r: 0.12, g: 0.18, b: 0.28, a: 0.68 };

    fn view(s: &AllState, _: &mut Cx<AllState>) -> El {
        let mut k = Kit::neutral(true);
        k.metrics.popup_backdrop = Some(PopupBackdrop::frosted(20.0, 0.78));
        let mut layer = Vec::new();

        // Opt one menu completely out of theme blur and translucency (including its submenu).
        let solid_menu = Menu::new("solid")
            .trigger(Button::new("Solid").build(&k))
            .backdrop_blur(0.0)
            .surface_fill(SOLID)
            .item(MenuItem::new("Sub").key("solid-row").submenu(vec![MenuEntry::item(MenuItem::new("Leaf"))], true))
            .open(true)
            .build(&k, &mut layer);

        // Override only sigma on another menu; keep theme fill_alpha tint.
        let stronger_menu = Menu::new("strong")
            .trigger(Button::new("Strong").build(&k))
            .backdrop_blur(32.0)
            .item(MenuItem::new("Item"))
            .open(true)
            .build(&k, &mut layer);

        // Per-instance overrides on Select, PopupList, Popover, Combobox, HoverCard.
        let sel = Select::new("sel", &["A", "B"])
            .label("S")
            .open(true)
            .backdrop_blur(14.0)
            .surface_fill(CUSTOM_TINT)
            .build(&k, &mut layer);

        PopupList::<AllState>::new("pl", "pl-anchor")
            .items([PopupItem::new("a", "Alpha")])
            .open(true)
            .backdrop_blur(12.0)
            .surface_fill(CUSTOM_TINT)
            .build(&k, &mut layer);

        let pop = Popover::new("pop")
            .trigger(Button::new("Pop").build(&k))
            .open(true)
            .backdrop_blur(10.0)
            .surface_fill(CUSTOM_TINT)
            .child(text("Hi"))
            .build(&k, &mut layer);

        let combo_opts = [ComboboxOption::new("m1", "Model 1")];
        let combo = Combobox::new("cb", &s.combo, &combo_opts, |s: &mut AllState| &mut s.combo)
            .label("Model")
            .backdrop_blur(8.0)
            .surface_fill(CUSTOM_TINT)
            .build(&k, &mut layer);

        let card = HoverCard::new("hc")
            .trigger(Button::new("Card").build(&k))
            .open(true)
            .backdrop_blur(0.0)
            .surface_fill(SOLID)
            .child(text("Opaque"))
            .build(&k, &mut layer);

        root(
            col()
                .child(solid_menu)
                .child(stronger_menu)
                .child(sel)
                .child(el().key("pl-anchor").size(40.0, 20.0))
                .child(pop)
                .child(combo)
                .child(card),
            layer,
        )
    }

    let rt = settled(all_state(None), view);
    let k = Kit::neutral(true);

    assert_eq!(find(&rt, "solid-menu").backdrop_blur, 0.0);
    assert_eq!(find(&rt, "solid-menu").fill, SOLID);
    assert_eq!(find(&rt, "solid-row-sub-menu").backdrop_blur, 0.0);
    assert_eq!(find(&rt, "solid-row-sub-menu").fill, SOLID);

    assert_eq!(find(&rt, "strong-menu").backdrop_blur, 32.0);
    assert_eq!(find(&rt, "strong-menu").fill, k.alpha(k.c.layer_01, k.c.layer_01.a * 0.78));

    assert_eq!(find(&rt, "sel-content").backdrop_blur, 14.0);
    assert_eq!(find(&rt, "sel-content").fill, CUSTOM_TINT);

    assert_eq!(find(&rt, "pl-content").backdrop_blur, 12.0);
    assert_eq!(find(&rt, "pl-content").fill, CUSTOM_TINT);

    assert_eq!(find(&rt, "pop-content").backdrop_blur, 10.0);
    assert_eq!(find(&rt, "pop-content").fill, CUSTOM_TINT);

    assert_eq!(find(&rt, "cb-content").backdrop_blur, 8.0);
    assert_eq!(find(&rt, "cb-content").fill, CUSTOM_TINT);

    assert_eq!(find(&rt, "hc-card").backdrop_blur, 0.0);
    assert_eq!(find(&rt, "hc-card").fill, SOLID);
}

#[test]
fn reduced_transparency_disables_popup_backdrop_blur_and_restores_when_off() {
    let mut rt = settled(all_state(Some(PopupBackdrop::frosted(20.0, 0.78))), all_overlays_view);
    assert!(blur_instance_count(&rt) >= 10);
    assert_eq!(find(&rt, "cm-menu").backdrop_blur, 20.0);
    assert_eq!(find(&rt, "cm-share-sub-menu").backdrop_blur, 20.0);
    assert_eq!(find(&rt, "sel-content").backdrop_blur, 20.0);
    assert_eq!(find(&rt, "pl-content").backdrop_blur, 20.0);
    assert_eq!(find(&rt, "custom-raised").backdrop_blur, 20.0);

    rt.set_system_reduced_transparency(true);
    rt.frame(0.0);
    assert_eq!(blur_instance_count(&rt), 0, "Reduce Transparency skips all BackdropBlur records");
    for key in ["cm-menu", "cm-share-sub-menu", "sel-content", "pl-content", "pop-content", "custom-raised"] {
        assert_eq!(find(&rt, key).backdrop_blur, 0.0, "{key} reports 0 blur under Reduce Transparency");
    }

    rt.set_system_reduced_transparency(false);
    rt.frame(0.0);
    assert!(blur_instance_count(&rt) >= 10, "BackdropBlur resumes when Reduce Transparency turns off");
    assert_eq!(find(&rt, "cm-menu").backdrop_blur, 20.0);
    assert_eq!(find(&rt, "cm-share-sub-menu").backdrop_blur, 20.0);
}

#[test]
fn popup_backdrop_sanitizes_negative_and_non_finite_values() {
    for bad in [-12.0, -0.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let rt = settled(all_state(Some(PopupBackdrop { sigma: bad, fill_alpha: bad, fill: None })), all_overlays_view);
        assert_eq!(blur_instance_count(&rt), 0, "bad sigma={bad} clamps to 0");
        let menu = find(&rt, "cm-menu");
        assert_eq!(menu.backdrop_blur, 0.0);
        assert!(menu.fill.a.is_finite() && (0.0..=1.0).contains(&menu.fill.a), "bad alpha={bad} clamped: {:?}", menu.fill);
    }

    // Over-1.0 fill_alpha clamps to 1.0 (opaque theme fill unchanged).
    let rt_over = settled(all_state(Some(PopupBackdrop::frosted(16.0, 2.5))), all_overlays_view);
    assert_eq!(find(&rt_over, "cm-menu").backdrop_blur, 16.0);
    assert_eq!(find(&rt_over, "cm-menu").fill, Kit::neutral(true).c.layer_01);
}
