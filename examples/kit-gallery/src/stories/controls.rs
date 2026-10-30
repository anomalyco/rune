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
    row().child(IconButton::new(&ic::PLUS).build(k))
}

pub(super) fn icon_button_variants(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    use IconButtonVariant::*;
    flex(12.0)
        .child(IconButton::new(&ic::PLUS).variant(Neutral).build(k))
        .child(IconButton::new(&ic::PLUS).variant(Contrast).build(k))
        .child(IconButton::new(&ic::ARROW_UP).variant(Submit).build(k))
        .child(IconButton::new(&ic::PLUS).variant(Ghost).build(k))
}

pub(super) fn icon_button_sizes(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    flex(12.0)
        .child(IconButton::new(&ic::PLUS).control_size(ControlSize::Mini).build(k))
        .child(IconButton::new(&ic::PLUS).control_size(ControlSize::Small).build(k))
        .child(IconButton::new(&ic::PLUS).build(k))
        .child(IconButton::new(&ic::PLUS).control_size(ControlSize::Large).build(k))
        .child(IconButton::new(&ic::PLUS).control_size(ControlSize::ExtraLarge).build(k))
}

pub(super) fn icon_button_all_states(_: &G, k: &Kit, _: &mut Vec<El>) -> El {
    use IconButtonVariant::*;
    let variants = [("neutral", Neutral), ("contrast", Contrast), ("submit", Submit), ("ghost", Ghost)];
    let mut c = col().gap(12.0);
    for (name, v) in variants {
        let mut r = row().gap(8.0).flex_wrap();
        for (state, st) in STATES {
            r = r.child(IconButton::new(&ic::PLUS).variant(v).state(st).build(k).key(format!("{name}-{state}")));
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
        let sw = |on: bool, st: State| Switch::new(on).control_size(size).state(st).build(k);
        row().gap(12.0).center_y()
            .child(caption(k, name).w(80.0))
            .child(Switch::new(s.is(&key)).control_size(size).label(name).hide_label().on_toggle(move |s: &mut G| s.flip(&key)).build(k))
            .child(sw(true, State::Rest))
            .child(sw(false, State::Hover))
            .child(sw(true, State::Pressed))
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
