//! Dense and roomy `Kit::metrics` themes and flat / bordered `Switch` chrome.

use super::*;

fn dense_kit(base: &Kit) -> Kit {
    let mut k = base.clone();
    k.radius = Radius { xs: 2.0, sm: 3.0, md: 4.0, lg: 6.0, xl: 8.0 };
    k.elevation.switch_off.clear();
    k.elevation.switch_on.clear();
    k.elevation.elements.clear();
    k.metrics.segmented = Some(SegmentedMetrics { height: 22.0, padding_x: 8.0, ..SegmentedMetrics::kit(&k) });
    k.metrics.disclosure_header_height = Some(24.0);
    let mut p = ProgressMetrics::kit(&k);
    p.height = 5.0;
    p.border_width = 0.0;
    k.metrics.progress = Some(p);
    k.metrics.divider_thickness = Some(0.5);
    k.metrics.keybind = Some(KeybindMetrics { height: 12.0, padding_x: 3.0, ..KeybindMetrics::kit(&k) });
    k.metrics.text_area_min_height = Some(52.0);
    k.metrics.standard_switch = Some(StandardSwitchMetrics {
        track_width: 24.0,
        track_height: 14.0,
        thumb: 12.0,
        ..StandardSwitchMetrics::kit(&k)
    });
    k.metrics.switch_look = Some(SwitchLook {
        track_border: 1.0,
        track_border_off: k.c.border_strong,
        track_border_on: rune::Color::TRANSPARENT,
        ..SwitchLook::flat(&k)
    });
    k
}

fn roomy_kit(base: &Kit) -> Kit {
    let mut k = base.clone();
    k.radius = Radius { xs: 4.0, sm: 6.0, md: 10.0, lg: 12.0, xl: 14.0 };
    k.elevation.switch_off.clear();
    k.elevation.switch_on.clear();
    k.elevation.elements.clear();
    k.metrics.segmented = Some(SegmentedMetrics { height: 36.0, padding_x: 16.0, ..SegmentedMetrics::kit(&k) });
    k.metrics.disclosure_header_height = Some(42.0);
    let mut p = ProgressMetrics::kit(&k);
    p.height = 12.0;
    p.border_width = 1.5;
    k.metrics.progress = Some(p);
    k.metrics.divider_thickness = Some(2.0);
    k.metrics.keybind = Some(KeybindMetrics { height: 20.0, padding_x: 7.0, ..KeybindMetrics::kit(&k) });
    k.metrics.text_area_min_height = Some(108.0);
    let mut std_sw = StandardSwitchMetrics::kit(&k);
    std_sw.track_width = 36.0;
    std_sw.track_height = 20.0;
    std_sw.thumb = 16.0;
    std_sw.inset = 2.0;
    k.metrics.standard_switch = Some(std_sw);
    k.metrics.switch_look = Some(SwitchLook {
        track_border: 1.5,
        track_border_off: k.c.border_strong,
        track_border_on: k.c.border_focus,
        ..SwitchLook::flat(&k)
    });
    k
}

fn sample_column(s: &G, title: &str, prefix: &'static str, k: &Kit) -> El {
    let seg_key = format!("{prefix}-seg");
    let seg_sel: usize = s.picked(&seg_key, "0").parse().unwrap_or(0);
    let sw_key = format!("{prefix}-sw");
    let std_key = format!("{prefix}-std");
    let col_key = format!("{prefix}-col");
    let acc_key = format!("{prefix}-acc");
    let seg_key_cb = seg_key.clone();
    let sw_key_cb = sw_key.clone();
    let std_key_cb = std_key.clone();
    let col_key_cb = col_key.clone();
    let acc_key_cb = acc_key.clone();

    col()
        .key(prefix)
        .w(210.0)
        .gap(10.0)
        .child(caption(k, title))
        .child(
            SegmentedControl::new(&["List", "Grid"], Some(seg_sel))
                .label(format!("{title} view"))
                .width(None)
                .on_pick(move |s: &mut G, i, _| {
                    s.pick.insert(seg_key_cb.clone(), i.to_string());
                })
                .build(k),
        )
        .child(
            row()
                .gap(12.0)
                .center_y()
                .child(
                    Switch::new(!s.is(&sw_key))
                        .label("Compact")
                        .on_toggle(move |s: &mut G| s.flip(&sw_key_cb))
                        .build(k),
                )
                .child(
                    Switch::new(!s.is(&std_key))
                        .standard()
                        .label("Std")
                        .on_toggle(move |s: &mut G| s.flip(&std_key_cb))
                        .build(k),
                ),
        )
        .child(Progress::new(Some(0.65)).label("Build").show_value().build(k))
        .child(Divider::horizontal().build(k))
        .child(row().gap(8.0).center_y().child(Keybind::new(&["⌘", "K"]).build(k)).child(Keybind::new(&["Shift", "P"]).build(k)))
        .child(
            Collapsible::new("Details", s.is(&col_key))
                .on_toggle(move |s: &mut G| s.flip(&col_key_cb))
                .child(text("Metrics scale this header.").font(k.ty.small).color(k.c.text_muted))
                .build(k),
        )
        .child(
            Accordion::new()
                .item(
                    AccordionItem::new("Section")
                        .expanded(s.is(&acc_key))
                        .on_toggle(move |s: &mut G| s.flip(&acc_key_cb))
                        .child(row().py(6.0).child(text("Accordion body.").font(k.ty.small).color(k.c.text_muted))),
                )
                .build(k),
        )
        .child(TextArea::new(&TextEdit::new("")).rows(2).width(None).placeholder("Notes").build(k))
}

/// Default, dense and roomy `Kit::metrics` themes side by side.
pub(super) fn metrics_themes(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let dense = dense_kit(k);
    let roomy = roomy_kit(k);
    row()
        .gap(20.0)
        .items_start()
        .child(sample_column(s, "Default", "def", k))
        .child(sample_column(s, "Dense + flat", "dense", &dense))
        .child(sample_column(s, "Roomy + flat", "roomy", &roomy))
}

/// Raised vs flat compact and web `Switch` controls, with and without a track border.
pub(super) fn switch_flat(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let mut flat_bordered = k.clone();
    flat_bordered.elevation.switch_off.clear();
    flat_bordered.elevation.switch_on.clear();
    flat_bordered.elevation.elements.clear();
    flat_bordered.metrics.switch_look = Some(SwitchLook {
        track_border: 1.0,
        track_border_off: k.c.border_strong,
        track_border_on: rune::Color::TRANSPARENT,
        ..SwitchLook::flat(k)
    });
    let mut both_bordered = k.clone();
    both_bordered.elevation.switch_off.clear();
    both_bordered.elevation.switch_on.clear();
    both_bordered.elevation.elements.clear();
    both_bordered.metrics.switch_look = Some(SwitchLook {
        track_border: 1.5,
        track_border_off: k.c.border_strong,
        track_border_on: k.c.border_focus,
        ..SwitchLook::flat(k)
    });

    let states = [("Rest", State::Rest), ("Hover", State::Hover), ("Pressed", State::Pressed), ("Focus", State::Focus), ("Disabled", State::Disabled)];
    let band = |name: &str, kit: &Kit, web: bool, per_instance_flat: bool| {
        let mut r = row().gap(12.0).center_y().child(caption(k, name).w(128.0));
        for (st_name, st) in states {
            for on in [false, true] {
                let mut sw = Switch::new(on)
                    .label(format!("{name} {st_name} {}", if on { "on" } else { "off" }))
                    .hide_label()
                    .state(st);
                if web {
                    sw = sw.web();
                }
                if per_instance_flat {
                    sw = sw.flat();
                }
                r = r.child(sw.build(kit));
            }
        }
        r
    };

    col()
        .gap(12.0)
        .child(
            row()
                .gap(16.0)
                .center_y()
                .child(Switch::new(!s.is("sf-raised")).label("Raised").on_toggle(flip("sf-raised")).build(k))
                .child(Switch::new(!s.is("sf-flat")).flat().label("Switch::flat()").on_toggle(flip("sf-flat")).build(k))
                .child(Switch::new(!s.is("sf-border")).label("Flat + off border").on_toggle(flip("sf-border")).build(&flat_bordered))
                .child(Switch::new(!s.is("sf-both")).label("Flat + both borders").on_toggle(flip("sf-both")).build(&both_bordered)),
        )
        .child(band("Raised compact", k, false, false))
        .child(band("Flat compact", k, false, true))
        .child(band("Flat + off border", &flat_bordered, false, false))
        .child(band("Flat + both borders", &both_bordered, false, false))
        .child(band("Flat web", &flat_bordered, true, false))
}
