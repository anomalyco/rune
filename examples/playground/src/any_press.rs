//! The "Controls/Popups/Passive press observer" story: a non-focus-taking popup dismissed via
//! `El::on_any_press` without consuming clicks or moving focus, beside a live
//! `PointerEvent::pressing` readout over buttons and a text input.

use rune::tok::{self, radius, type_};
use rune::{Color, Cx, El, PointerButton, PointerEvent, TextEdit, col, el, row, text, text_input};

const POPUP_W: f32 = 220.0;
const POPUP_H: f32 = 96.0;

pub(crate) struct AnyPressDemo {
    pub(crate) popup_open: bool,
    pub(crate) clicks: u32,
    pub(crate) preserved_clicks: u32,
    pub(crate) last_observed: Option<(PointerButton, PointerEvent)>,
    pub(crate) pressing_now: bool,
    pub(crate) field: TextEdit,
}

impl Default for AnyPressDemo {
    fn default() -> Self {
        Self {
            popup_open: true,
            clicks: 0,
            preserved_clicks: 0,
            last_observed: None,
            pressing_now: false,
            field: TextEdit::new("Focus stays here"),
        }
    }
}

fn button_name(button: PointerButton) -> &'static str {
    match button {
        PointerButton::Primary => "Primary",
        PointerButton::Secondary => "Secondary",
        PointerButton::Middle => "Middle",
        PointerButton::Back => "Back",
        PointerButton::Forward => "Forward",
        _ => "Other",
    }
}

pub(crate) fn view(s: &AnyPressDemo, cx: &mut Cx<AnyPressDemo>) -> El {
    let t = tok::theme(cx.system_dark());
    let accent = tok::accent(0, cx.system_dark());

    let observed_label = match s.last_observed {
        Some((btn, p)) => format!(
            "Last observed: {} at local ({:.0}, {:.0}) · window ({:.0}, {:.0})",
            button_name(btn), p.x, p.y, p.window_x, p.window_y,
        ),
        None => "Press any mouse button anywhere to observe it".to_owned(),
    };

    let trigger = row().key("toggle-popup").h(30.0).px(12.0).center().radius(radius::SM)
        .fill(if s.popup_open { accent } else { t.control_track })
        .hover(if s.popup_open { accent } else { t.hover })
        .role("button").label("Toggle popup").focusable(true)
        .on_press(|s: &mut AnyPressDemo| s.popup_open = !s.popup_open)
        .child(text(if s.popup_open { "Popup open" } else { "Open popup" })
            .font(type_::LABEL).color(if s.popup_open { Color::WHITE } else { t.text }));

    let action_btn = row().key("count-clicks").h(30.0).px(12.0).center().radius(radius::SM)
        .fill(t.control_track).hover(t.hover)
        .role("button").label("Count clicks").focusable(true)
        .on_press(|s: &mut AnyPressDemo| s.clicks += 1)
        .child(text(format!("Clicks: {}", s.clicks)).font(type_::LABEL).color(t.text));

    let keep_focus_btn = row().key("keep-focus").h(30.0).px(12.0).center().radius(radius::SM)
        .fill(t.control_track).hover(t.hover)
        .role("button").label("Keep focus").preserve_focus_on_press(true)
        .on_press(|s: &mut AnyPressDemo| s.preserved_clicks += 1)
        .child(text(format!("Keep focus: {}", s.preserved_clicks)).font(type_::LABEL).color(t.text));

    let field = row().w_full().h(32.0).px(10.0).center_y()
        .radius(radius::SM).fill(t.card).border(1.0, t.card_border)
        .child(
            text_input(&s.field, type_::BODY)
                .key("focus-field")
                .w_full().color(t.text)
                .label("Sample input")
                .on_edit(|s: &mut AnyPressDemo| &mut s.field),
        );

    let pressing_pill = row().key("pressing-state").h(24.0).px(10.0).center_y().gap(6.0)
        .radius(12.0).fill(t.control_track)
        .child(el().size(8.0, 8.0).radius(4.0).fill(if s.pressing_now { accent } else { t.text_secondary }))
        .child(text(if s.pressing_now { "PointerEvent::pressing = true" } else { "PointerEvent::pressing = false" })
            .font(type_::LABEL).color(t.text));

    let trigger_rect = cx.window_layout("toggle-popup");
    let popup = s.popup_open.then(|| {
        col().key("passive-popup")
            .absolute(20.0, 12.0)
            .size(POPUP_W, POPUP_H)
            .padding(12.0).gap(6.0)
            .radius(radius::MD).fill(t.card).outline(1.0, 0.0, accent)
            .shadow(4.0, 16.0, t.shadow_soft)
            .on_any_press(move |s: &mut AnyPressDemo, btn, p, _| {
                s.last_observed = Some((btn, p));
                let in_popup = p.x >= 0.0 && p.y >= 0.0 && p.x <= POPUP_W && p.y <= POPUP_H;
                let in_trigger = trigger_rect.is_some_and(|[x, y, w, h]| {
                    p.window_x >= x && p.window_y >= y && p.window_x <= x + w && p.window_y <= y + h
                });
                if !in_popup && !in_trigger {
                    s.popup_open = false;
                }
            })
            .child(text("Non-focus-taking popup").font(type_::BODY).color(t.text))
            .child(text("Press outside with any button: closes without swallowing the click.")
                .font(type_::LABEL).color(t.text_secondary))
    });

    col().key("any-press-stage").w_full().h_full().center().padding(20.0)
        .on_pointer_move_local(|s: &mut AnyPressDemo, p, _| {
            s.pressing_now = p.pressing;
        })
        .child(
            col().w(420.0).gap(12.0).padding(16.0).radius(radius::MD)
                .fill(t.card).outline(1.0, 0.0, t.card_border)
                .child(row().gap(8.0).center_y().child(trigger).child(action_btn).child(keep_focus_btn))
                .child(field)
                .child(row().w_full().justify_between().center_y().child(pressing_pill))
                .child(text(observed_label).key("observed-status").font(type_::LABEL).color(t.text_secondary)),
        )
        .children(popup)
}
