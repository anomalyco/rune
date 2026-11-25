//! Showcase of `Kit::metrics` overlay tokens and `Menu` spacing/separator controls in
//! Default, Dense, and Roomy themes.

use super::*;

fn themed_kit(base: &Kit, preset: &str) -> Kit {
    let mut k = base.clone();
    match preset {
        "dense" => {
            k.radius = Radius { xs: 2.0, sm: 3.0, md: 4.0, lg: 6.0, xl: 8.0 };
            k.metrics.dialog = Some(DialogMetrics { padding: 12.0, ..DialogMetrics::kit(&k) });
            k.metrics.popup_row = Some(PopupRowMetrics { height: 22.0, ..PopupRowMetrics::kit(&k) });
            let mut sm = SelectMetrics::kit(&k);
            sm.chevron = 14.0;
            sm.chevron_box = 16.0;
            k.metrics.select = Some(sm);
            let mut pm = PopoverMetrics::kit(&k);
            pm.min_width = 160.0;
            pm.max_width = 240.0;
            k.metrics.popover = Some(pm);
            k.metrics.menu = Some(MenuMetrics {
                standard_padding: 2.0,
                standard_separator_above: 2.0,
                standard_separator_below: 2.0,
                standard_inner_border: 0.0,
                ..MenuMetrics::kit(&k)
            });
        }
        "roomy" => {
            k.radius = Radius { xs: 4.0, sm: 8.0, md: 12.0, lg: 16.0, xl: 20.0 };
            k.metrics.dialog = Some(DialogMetrics {
                padding: 24.0,
                title: Some(TextStyle::new(18.0, 600.0).lh(24.0)),
                ..DialogMetrics::kit(&k)
            });
            k.metrics.popup_row = Some(PopupRowMetrics { height: 36.0, ..PopupRowMetrics::kit(&k) });
            let mut sm = SelectMetrics::kit(&k);
            sm.chevron = 18.0;
            sm.chevron_box = 24.0;
            k.metrics.select = Some(sm);
            let mut pm = PopoverMetrics::kit(&k);
            pm.min_width = 240.0;
            pm.max_width = 380.0;
            k.metrics.popover = Some(pm);
            k.metrics.menu = Some(MenuMetrics {
                padding: 8.0,
                standard_padding: 10.0,
                separator_above: 6.0,
                separator_below: 6.0,
                standard_separator_above: 8.0,
                standard_separator_below: 8.0,
                ..MenuMetrics::kit(&k)
            });
        }
        _ => {}
    }
    k
}

pub(super) fn overlays_metrics(s: &G, base: &Kit, layer: &mut Vec<El>) -> El {
    let preset = s.picked("overlay-metrics-preset", "dense");
    let k = themed_kit(base, preset);
    let idx = match preset {
        "default" => 0,
        "roomy" => 2,
        _ => 1,
    };
    let switcher = SegmentedControl::new(&["Default", "Dense", "Roomy"], Some(idx))
        .on_pick(|s: &mut G, i, _| {
            let v = match i {
                0 => "default",
                2 => "roomy",
                _ => "dense",
            };
            s.pick.insert("overlay-metrics-preset".into(), v.into());
        })
        .build(base);

    let prepend = s.is("om-prepend");
    let menu_open = is_open(s, "om-menu");
    let mut menu = Menu::new("om-menu")
        .label("Themed actions")
        .trigger(
            Button::new("Open Menu")
                .variant(ButtonVariant::Outline)
                .expanded(menu_open)
                .on_press(toggle("om-menu"))
                .build(&k),
        )
        .open(menu_open)
        .on_close(close);
    if prepend {
        menu = menu.item(MenuItem::new("Pinned item").key("om-pinned").on_select(flip("om-prepend")));
    }
    let menu_el = menu
        .item(MenuItem::new("Add").key("om-add").on_select(close))
        .separator_keyed("om-sep")
        .item(MenuItem::new("Arrange").key("om-arrange").on_select(close))
        .item(MenuItem::new("Delete").key("om-delete").danger(true).on_select(close))
        .build(&k, layer);

    let sel_choice = s.picked("om-select", "Medium");
    let select_el = Select::new("om-select", &["Compact", "Medium", "Spacious", "Extra wide", "Custom", "Auto", "System", "Fixed"])
        .label("Density")
        .appearance(SelectAppearance::Field)
        .width(200.0)
        .current(Some(sel_choice))
        .open(is_open(s, "om-select"))
        .on_toggle(toggle("om-select"))
        .on_close(close)
        .on_choose(|s: &mut G, v: &str, _| {
            s.pick.insert("om-select".into(), v.into());
            s.open = None;
        })
        .build(&k, layer);

    let pop_el = Popover::new("om-popover")
        .trigger(
            Button::new("Open Popover")
                .variant(ButtonVariant::Outline)
                .on_press(toggle("om-popover"))
                .build(&k),
        )
        .title("Popover limits")
        .description("Width clamps from Kit::metrics.popover.")
        .child(overlay_text(&k, "Content sized inside min_width..max_width."))
        .open(is_open(s, "om-popover"))
        .on_close(close)
        .build(&k, layer);

    let dialog_btn = Button::new("Open Dialog")
        .variant(ButtonVariant::Outline)
        .on_press(toggle("om-dialog"))
        .build(&k);
    Dialog::new("om-dialog", "Themed dialog")
        .description("Radius, header padding, and title style come from Kit::metrics.dialog.")
        .fit()
        .open(is_open(s, "om-dialog"))
        .on_close(close)
        .child(
            row()
                .w_full()
                .padding(16.0)
                .justify_end()
                .child(Button::new("Done").on_press(close).build(&k)),
        )
        .build(&k, layer);

    let list = PopupList::<G>::new("om-list", "")
        .label("Popup rows")
        .max_height(150.0)
        .items([
            PopupItem::new("menu", "Compact menu rows").description("PopupRowMetrics"),
            PopupItem::new("select", "Select listbox rows").description("Scales max height"),
            PopupItem::new("popup", "PopupList rows").description("Shared virtual height"),
        ])
        .active(Some(0))
        .build_list(&k);

    col()
        .gap(12.0)
        .items_start()
        .child(
            row()
                .gap(12.0)
                .center_y()
                .child(switcher)
                .child(
                    Button::new(if prepend { "Remove top item" } else { "Prepend menu item" })
                        .variant(ButtonVariant::Ghost)
                        .control_size(ControlSize::Small)
                        .on_press(flip("om-prepend"))
                        .build(base),
                ),
        )
        .child(row().gap(10.0).center_y().child(menu_el).child(select_el).child(pop_el).child(dialog_btn))
        .child(
            col()
                .w(320.0)
                .padding(4.0)
                .radius(k.radius.md)
                .fill(k.c.layer_01)
                .border(1.0, k.c.border_weak)
                .child(list),
        )
}
