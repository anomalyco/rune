//! Button, IconButton, Switch, Checkbox and Radio.

use super::*;

// Button

pub(super) fn button_playground(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(Button::new("Button").build(k))
}

pub(super) fn button_variants(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    use ButtonVariant::*;
    flex(12.0)
        .child(Button::new("Neutral").variant(Neutral).build(k))
        .child(Button::new("Danger").variant(Danger).build(k))
        .child(Button::new("Warning").variant(Warning).build(k))
        .child(Button::new("Contrast").variant(Contrast).build(k))
        .child(Button::new("Submit").variant(Submit).build(k))
        .child(Button::new("Ghost").variant(Ghost).build(k))
        .child(Button::new("Ghost muted").variant(GhostMuted).icon(&ic::EDIT).build(k))
        .child(Button::new("Ghost faint").variant(GhostFaint).icon(&ic::EDIT).build(k))
        .child(Button::new("Loading").variant(Loading).build(k))
}

pub(super) fn button_sizes(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    flex(12.0)
        .child(Button::new("Mini").control_size(ControlSize::Mini).build(k))
        .child(Button::new("Small").control_size(ControlSize::Small).build(k))
        .child(Button::new("Regular").build(k))
        .child(Button::new("Large").control_size(ControlSize::Large).build(k))
        .child(Button::new("Extra large").control_size(ControlSize::ExtraLarge).build(k))
}

pub(super) fn button_icon(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    flex(12.0)
        .child(Button::new("Regular").icon(&ic::PLUS).build(k))
        .child(Button::new("Large").variant(ButtonVariant::Contrast).control_size(ControlSize::Large).icon(&ic::PLUS).build(k))
}

pub(super) fn button_shortcuts(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    use ButtonVariant::*;
    col()
        .gap(12.0)
        .child(
            flex(12.0)
                .child(Button::new("Save").shortcut("cmd+s").show_shortcut().build(k))
                .child(Button::new("End it now").variant(Contrast).shortcut("⏎").show_shortcut().build(k))
                .child(Button::new("Submit").variant(Submit).shortcut("cmd+enter").show_shortcut().build(k))
                .child(Button::new("End run").variant(Danger).shortcut("cmd+d").show_shortcut().build(k))
                .child(Button::new("Retry").variant(Warning).shortcut("cmd+r").show_shortcut().build(k)),
        )
        .child(
            flex(12.0)
                .child(Button::new("New").icon(&ic::PLUS).variant(Outline).shortcut("cmd+n").show_shortcut().build(k))
                .child(Button::new("Stay").variant(Ghost).shortcut("esc").show_shortcut().build(k))
                .child(Button::new("Edit").variant(GhostMuted).shortcut("E").show_shortcut().build(k))
                .child(Button::new("More").variant(GhostFaint).shortcut("M").show_shortcut().build(k))
                .child(Button::new("Disabled").shortcut("cmd+k").show_shortcut().disabled(true).build(k)),
        )
}

/// Focus options on buttons: `El::keyboard_focus_only()` (runs `on_press` on click without
/// taking focus, while Tab still visits it) and `El::focus_ring_radius(f32)` (explicit outer
/// focus-ring corner radius independent of the control's own radius).
pub(super) fn button_focus(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let last = s.picked("btn-focus", "None");
    let kbd_only = col()
        .gap(8.0)
        .items_start()
        .child(text("El::keyboard_focus_only()").font(k.ty.ui_medium).color(k.c.text_base))
        .child(
            row()
                .gap(8.0)
                .child(
                    Button::new("Toolbar action")
                        .variant(ButtonVariant::Contrast)
                        .on_press(|s: &mut G| { s.pick.insert("btn-focus".into(), "Toolbar action".into()); })
                        .build(k)
                        .key("kbd-only-btn")
                        .keyboard_focus_only(),
                )
                .child(
                    Button::new("Normal button")
                        .variant(ButtonVariant::Outline)
                        .on_press(|s: &mut G| { s.pick.insert("btn-focus".into(), "Normal button".into()); })
                        .build(k)
                        .key("normal-focus-btn"),
                ),
        )
        .child(
            text(format!("Last pressed: {last} — click Toolbar action without taking focus; Tab visits both."))
                .font(k.ty.small)
                .color(k.c.text_muted),
        );
    let ring_radii = col()
        .gap(8.0)
        .items_start()
        .child(text("El::focus_ring_radius(f32)").font(k.ty.ui_medium).color(k.c.text_base))
        .child(
            row()
                .gap(12.0)
                .child(
                    Button::new("Pill ring")
                        .variant(ButtonVariant::Contrast)
                        .build(k)
                        .key("pill-ring-btn")
                        .radius(999.0)
                        .focus_ring_radius(999.0),
                )
                .child(
                    Button::new("4pt box, 8pt ring")
                        .variant(ButtonVariant::Outline)
                        .build(k)
                        .key("row-ring-btn")
                        .radius(4.0)
                        .focus_ring(2.0, 2.0, k.c.border_focus)
                        .focus_ring_radius(8.0),
                )
                .child(
                    Button::new("Square ring (0pt)")
                        .variant(ButtonVariant::Outline)
                        .build(k)
                        .key("square-ring-btn")
                        .radius(6.0)
                        .focus_ring_radius(0.0),
                ),
        )
        .child(
            text("Tab through these buttons to compare pill, 8pt and square focus rings.")
                .font(k.ty.small)
                .color(k.c.text_muted),
        );
    col().gap(20.0).items_start().child(kbd_only).child(ring_radii)
}

pub(super) fn button_all_states(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    use ButtonVariant::*;
    let variants = [
        ("neutral", Neutral),
        ("danger", Danger),
        ("warning", Warning),
        ("contrast", Contrast),
        ("submit", Submit),
        ("ghost", Ghost),
        ("ghost-muted", GhostMuted),
        ("ghost-faint", GhostFaint),
        ("loading", Loading),
    ];
    let mut c = col().gap(12.0);
    for (name, v) in variants {
        let mut r = row().gap(8.0).flex_wrap();
        for (state, st) in STATES {
            r = r.child(Button::new(cap_first(state)).variant(v).state(st).build(k).key(format!("{name}-{state}")));
        }
        c = c.child(col().gap(8.0).child(caption(k, &title_case(name))).child(r));
    }
    c
}

// IconButton

pub(super) fn icon_button_playground(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(IconButton::new(&ic::PLUS).label("Add").build(k))
}

pub(super) fn icon_button_variants(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    use IconButtonVariant::*;
    flex(12.0)
        .child(IconButton::new(&ic::PLUS).label("Add").variant(Neutral).build(k))
        .child(IconButton::new(&ic::PLUS).label("Add").variant(Contrast).build(k))
        .child(IconButton::new(&ic::ARROW_UP).label("Send").variant(Submit).build(k))
        .child(IconButton::new(&ic::PLUS).label("Add").variant(Ghost).build(k))
}

pub(super) fn icon_button_sizes(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    flex(12.0)
        .child(IconButton::new(&ic::PLUS).label("Add").control_size(ControlSize::Mini).build(k))
        .child(IconButton::new(&ic::PLUS).label("Add").control_size(ControlSize::Small).build(k))
        .child(IconButton::new(&ic::PLUS).label("Add").build(k))
        .child(IconButton::new(&ic::PLUS).label("Add").control_size(ControlSize::Large).build(k))
        .child(IconButton::new(&ic::PLUS).label("Add").control_size(ControlSize::ExtraLarge).build(k))
}

pub(super) fn icon_button_all_states(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    use IconButtonVariant::*;
    let variants = [("neutral", Neutral), ("contrast", Contrast), ("submit", Submit), ("ghost", Ghost)];
    let mut c = col().gap(12.0);
    for (name, v) in variants {
        let mut r = row().gap(8.0).flex_wrap();
        for (state, st) in STATES {
            r = r.child(IconButton::new(&ic::PLUS).label("Add").variant(v).state(st).build(k).key(format!("{name}-{state}")));
        }
        c = c.child(col().gap(8.0).child(caption(k, &title_case(name))).child(r));
    }
    c
}

// Switch

/// `defaultChecked` items read as on until toggled.
fn on_by_default(s: &G, key: &str) -> bool {
    !s.is(key)
}

pub(super) fn switch_basic(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(Switch::new(on_by_default(s, "sw")).label("Enable notifications").on_toggle(flip("sw")).build(k))
}

pub(super) fn switch_appearances(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    col()
        .gap(12.0)
        .child(Switch::new(on_by_default(s, "std")).label("Standard").standard().on_toggle(flip("std")).build(k))
        .child(Switch::new(on_by_default(s, "cmp")).label("Compact").on_toggle(flip("cmp")).build(k))
}

pub(super) fn switch_states(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    col()
        .gap(12.0)
        .child(Switch::new(on_by_default(s, "a")).label("Enabled").on_toggle(flip("a")).build(k))
        .child(Switch::new(s.is("b")).label("Disabled").on_toggle(flip("b")).build(k))
        .child(Switch::new(false).label("Disabled switch").disabled(true).build(k))
}

/// Every control size off, on, hovered and pressed.
pub(super) fn switch_sizes(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let sizes = [("Mini", ControlSize::Mini), ("Small", ControlSize::Small), ("Regular", ControlSize::Regular),
        ("Large", ControlSize::Large), ("Extra large", ControlSize::ExtraLarge)];
    col().gap(12.0).children(sizes.into_iter().map(|(name, size)| {
        let key = format!("size-{name}");
        let sw = |on: bool, st: State| Switch::new(on).label(name).hide_label().control_size(size).state(st).build(k);
        row().gap(12.0).center_y()
            .child(caption(k, name).w(80.0))
            .child(Switch::new(s.is(&key)).control_size(size).label(name).hide_label().on_toggle(move |s: &mut G| s.flip(&key)).build(k))
            .child(sw(true, State::Rest))
            .child(sw(false, State::Hover))
            .child(sw(true, State::Pressed))
    }))
}

/// The compact switch as the web draws it (`Switch::web`): off and on, at rest, hovered,
/// keyboard-focused and disabled.
pub(super) fn switch_web(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let states = [("Rest", State::Rest), ("Hover", State::Hover), ("Focus", State::Focus), ("Disabled", State::Disabled)];
    col().gap(12.0)
        .child(row().gap(12.0).center_y().child(caption(k, "Live").w(80.0))
            .child(Switch::new(s.is("web")).web().label("Web").hide_label().on_toggle(flip("web")).build(k)))
        .children(states.into_iter().map(|(name, st)| {
            row().gap(16.0).center_y()
                .child(caption(k, name).w(80.0))
                .child(Switch::new(false).web().label(format!("{name} off")).hide_label().state(st).build(k))
                .child(Switch::new(true).web().label(format!("{name} on")).hide_label().state(st).build(k))
        }))
}

pub(super) fn switch_hidden_label(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(Switch::new(on_by_default(s, "sw")).label("Hidden label").hide_label().on_toggle(flip("sw")).build(k))
}

// Checkbox

pub(super) fn checkbox_basic(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(Checkbox::new(on_by_default(s, "cb")).label("Checkbox").on_toggle(flip("cb")).build(k))
}

pub(super) fn checkbox_states(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    col()
        .gap(12.0)
        .child(Checkbox::new(on_by_default(s, "a")).label("Checked").on_toggle(flip("a")).build(k))
        .child(Checkbox::new(s.is("b")).label("Unchecked").on_toggle(flip("b")).build(k))
        .child(Checkbox::new(false).label("Disabled").disabled(true).build(k))
        .child(
            Checkbox::new(s.is("d")).label("With description").description("Helper text").on_toggle(flip("d")).build(k),
        )
}

pub(super) fn checkbox_custom_icon(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(
        Checkbox::new(on_by_default(s, "cb")).label("Custom icon").icon(&ic::CHECK).on_toggle(flip("cb")).build(k),
    )
}

pub(super) fn checkbox_hidden_label(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    row().child(Checkbox::new(s.is("cb")).label("Hidden label").hide_label().on_toggle(flip("cb")).build(k))
}

// Radio

fn radio_group(
    s: &G,
    group: &'static str,
    default: &str,
    label: &str,
    items: &[(&'static str, &str, Option<&str>)],
) -> RadioGroup {
    let current = s.picked(group, default).to_owned();
    let mut g = RadioGroup::new().label(label);
    for (value, l, d) in items {
        let v = *value;
        let mut item = RadioItem::new(*l, current == v).on_select(move |s: &mut G| {
            s.pick.insert(group.to_owned(), v.to_owned());
        });
        if let Some(d) = d {
            item = item.description(*d);
        }
        g = g.item(item);
    }
    g
}

pub(super) fn radio_basic(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    radio_group(
        s,
        "freq",
        "daily",
        "Notification frequency",
        &[
            ("daily", "Daily", Some("Once per day at 9am.")),
            ("weekly", "Weekly", Some("Every Monday morning.")),
            ("never", "Never", Some("No notifications.")),
        ],
    )
    .build(k)
}

pub(super) fn radio_controlled(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let v = s.picked("ctl", "daily").to_owned();
    col()
        .gap(12.0)
        .child(
            radio_group(
                s,
                "ctl",
                "daily",
                "Controlled",
                &[("daily", "Daily", None), ("weekly", "Weekly", None), ("never", "Never", None)],
            )
            .build(k),
        )
        .child(text(format!("Value: {v}")).font(TextStyle::new(12.0, 440.0).lh(24.0)).color(k.c.text_faint))
}

pub(super) fn radio_states(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let disabled_item = RadioGroup::new()
        .label("Disabled item")
        .item(RadioItem::new("Enabled", s.picked("di", "a") == "a").on_select(|s: &mut G| {
            s.pick.insert("di".into(), "a".into());
        }))
        .item(RadioItem::new("Disabled", false).disabled(true));
    col()
        .gap(20.0)
        .child(
            radio_group(
                s,
                "def",
                "a",
                "Default",
                &[("a", "Option A", None), ("b", "Option B", Some("Has a description."))],
            )
            .build(k),
        )
        .child(
            RadioGroup::new()
                .label("Disabled group")
                .disabled(true)
                .item(RadioItem::new("Option A", true))
                .item(RadioItem::new("Option B", false))
                .build(k),
        )
        .child(disabled_item.build(k))
        .child(
            radio_group(s, "inv", "a", "Invalid", &[("a", "Option A", None), ("b", "Option B", None)])
                .description("Pick one option.")
                .invalid(true)
                .build(k),
        )
}

// Slider

pub(super) fn slider_basic(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let vol = s.value("slider-vol", 50.0);
    let thumb = s.value("slider-thumb", 240.0);
    let hidden = s.value("slider-hidden", 0.65);
    col()
        .gap(16.0)
        .child(
            row().gap(12.0).center_y()
                .child(
                    Slider::new("slider-vol", vol)
                        .range(0.0..=100.0)
                        .label("Volume")
                        .value_text(format!("{vol:.0}%"))
                        .on_change(|s: &mut G, v, _| s.set_value("slider-vol", v))
                        .build(k),
                )
                .child(caption(k, &format!("{vol:.0}%")).w(44.0)),
        )
        .child(
            row().gap(12.0).center_y()
                .child(
                    Slider::new("slider-thumb", thumb)
                        .range(160.0..=420.0)
                        .step(20.0)
                        .label("Thumbnail size")
                        .value_text(format!("{thumb:.0} px"))
                        .on_change(|s: &mut G, v, _| s.set_value("slider-thumb", v))
                        .build(k),
                )
                .child(caption(k, &format!("{thumb:.0} px")).w(52.0)),
        )
        .child(
            row().gap(12.0).center_y()
                .child(caption(k, "Hidden label").w(88.0))
                .child(
                    Slider::new("slider-hidden", hidden)
                        .label("Hidden label")
                        .hide_label()
                        .on_change(|s: &mut G, v, _| s.set_value("slider-hidden", v))
                        .build(k),
                ),
        )
}

pub(super) fn slider_sizes(s: &G, k: &Kit, _: &mut Vec<El>) -> El {
    let sizes = [
        ("Mini", ControlSize::Mini),
        ("Small", ControlSize::Small),
        ("Regular", ControlSize::Regular),
        ("Large", ControlSize::Large),
        ("Extra large", ControlSize::ExtraLarge),
    ];
    col().gap(12.0).children(sizes.into_iter().map(|(name, size)| {
        let key = format!("slider-size-{name}");
        let v = s.value(&key, 0.5);
        let key_cb = key.clone();
        row().gap(12.0).center_y()
            .child(caption(k, name).w(80.0))
            .child(
                Slider::new(key, v)
                    .control_size(size)
                    .label(name)
                    .hide_label()
                    .on_change(move |s: &mut G, v, _| s.set_value(&key_cb, v))
                    .build(k),
            )
    }))
}

pub(super) fn slider_states(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    col().gap(12.0).children(STATES.into_iter().map(|(name, st)| {
        row().gap(12.0).center_y()
            .child(caption(k, &cap_first(name)).w(80.0))
            .child(
                Slider::new(format!("slider-state-{name}"), 0.55)
                    .label(cap_first(name))
                    .hide_label()
                    .state(st)
                    .build(k),
            )
    }))
}
