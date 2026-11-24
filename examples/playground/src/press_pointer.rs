//! The "Controls/Buttons/Press cursor & bounds" story: app-wide `Cx::press_cursor`,
//! `Tx::press_point` local/window press readout (mouse, double, middle, or `None` on keyboard),
//! and opt-in `El::hover_tracks_bounds` during a held press beside a default-retention container.

use rune::tok::{self, radius, type_};
use rune::{Color, Cursor, Cx, El, PointerEvent, TextEdit, Tx, col, el, row, text, text_input};

pub(crate) struct PressPointerDemo {
    pub(crate) pointer_default: bool,
    pub(crate) last_point: Option<PointerEvent>,
    pub(crate) presses: usize,
    pub(crate) card_inside: bool,
    pub(crate) pill_inside: bool,
    pub(crate) field: TextEdit,
}

impl Default for PressPointerDemo {
    fn default() -> Self {
        Self {
            pointer_default: true,
            last_point: None,
            presses: 0,
            card_inside: false,
            pill_inside: false,
            field: TextEdit::new("Keeps I-beam"),
        }
    }
}

impl PressPointerDemo {
    fn record(&mut self, point: Option<PointerEvent>, count: bool) {
        self.last_point = point;
        if count {
            self.presses += 1;
        }
    }
}

pub(crate) fn view(s: &PressPointerDemo, cx: &mut Cx<PressPointerDemo>) -> El {
    if s.pointer_default {
        cx.press_cursor(Cursor::Pointer);
    }
    let t = tok::theme(cx.system_dark());
    let accent = tok::accent(0, cx.system_dark());
    let accent_hover = rune::color::hex("#274DEA").unwrap_or(accent);
    let accent_press = rune::color::hex("#1A34A8").unwrap_or(accent);

    let toggle_cursor = row().key("toggle-cursor").h(28.0).px(12.0).center().radius(radius::SM)
        .fill(if s.pointer_default { accent } else { t.control_track })
        .hover_fill(if s.pointer_default { accent_hover } else { t.hover })
        .role("switch").label("App-wide press_cursor(Pointer)").checked(s.pointer_default).focusable(true)
        .on_press(|s: &mut PressPointerDemo| s.pointer_default = !s.pointer_default)
        .child(text("press_cursor").font(type_::LABEL).color(if s.pointer_default { Color::WHITE } else { t.text }));

    let grab_header = row().key("grab-header").w_full().h(40.0).px(12.0).gap(8.0).center_y().justify_between()
        .radius(radius::MD).fill(t.card).border(1.0, t.card_border)
        .cursor(Cursor::Grab).hit()
        .child(text("Grab header").font(type_::LABEL).color(t.text_secondary))
        .child(
            row().gap(6.0).center_y()
                .child(
                    row().key("header-action").h(26.0).px(10.0).center().radius(radius::SM)
                        .fill(t.control_track).hover_fill(t.hover)
                        .role("button").label("Header action").focusable(true)
                        .on_press_tx(|s: &mut PressPointerDemo, tx: &mut Tx| {
                            s.record(tx.press_point(), true);
                        })
                        .child(text("Action").font(type_::LABEL).color(t.text)),
                )
                .child(
                    row().key("explicit-cursor").h(26.0).px(10.0).center().radius(radius::SM)
                        .fill(t.control_track).hover_fill(t.hover)
                        .cursor(Cursor::Crosshair)
                        .role("button").label("Explicit crosshair").focusable(true)
                        .on_press_tx(|s: &mut PressPointerDemo, tx: &mut Tx| {
                            s.record(tx.press_point(), true);
                        })
                        .child(text("Crosshair").font(type_::LABEL).color(t.text)),
                )
                .child(
                    row().key("disabled-action").h(26.0).px(10.0).center().radius(radius::SM)
                        .fill(t.control_track).opacity(0.5)
                        .role("button").label("Disabled button").disabled(true).hit()
                        .child(text("Disabled").font(type_::LABEL).color(t.text_secondary)),
                ),
        );

    let indicator = |key: &'static str, on: bool, on_color: Color, off_color: Color| {
        el().key(key).size(8.0, 8.0).radius(4.0).fill(if on { on_color } else { off_color })
    };

    let bounds_card = col().key("bounds-pill").w_full().padding(12.0).gap(10.0)
        .radius(radius::MD).fill(t.card).border(1.0, if s.pill_inside { accent } else { t.card_border })
        .on_hover_change(|s: &mut PressPointerDemo, inside, _| s.pill_inside = inside)
        .child(
            row().w_full().justify_between().center_y()
                .child(text("Default container (holds hover through press)").font(type_::LABEL).color(t.text_secondary))
                .child(indicator("pill-state", s.pill_inside, accent, t.control_track)),
        )
        .child(
            row().key("bounds-btn").w_full().h(52.0).px(14.0).center_y().justify_between().radius(radius::SM)
                .fill(accent).hover_fill(accent_hover).press_fill(accent_press)
                .press_scale(0.97)
                .hover_tracks_bounds()
                .role("button").label("Press & drag outside bounds").focusable(true).press_on_return()
                .on_hover_change(|s: &mut PressPointerDemo, inside, _| s.card_inside = inside)
                .on_press_tx(|s: &mut PressPointerDemo, tx: &mut Tx| {
                    s.record(tx.press_point(), true);
                })
                .on_double_press_tx(|s: &mut PressPointerDemo, tx: &mut Tx| {
                    s.record(tx.press_point(), false);
                })
                .on_middle_press(|s: &mut PressPointerDemo, tx: &mut Tx| {
                    s.record(tx.press_point(), true);
                })
                .child(text("Press, hold, and drag outside").font(type_::BODY).color(Color::WHITE))
                .child(indicator("card-state", s.card_inside, Color::WHITE, Color::WHITE.alpha(0.25))),
        );

    col().w_full().h_full().center().padding(16.0).child(
        col().w_full().max_w(420.0).gap(10.0)
            .child(row().w_full().justify_between().center_y()
                .child(text("Cx::press_cursor · Tx::press_point · El::hover_tracks_bounds").font(type_::LABEL).color(t.text_secondary))
                .child(toggle_cursor))
            .child(grab_header)
            .child(bounds_card)
            .child(
                row().w_full().gap(8.0).center_y()
                    .child(text({
                        let p = s.last_point.unwrap_or_default();
                        format!("presses {} · local {:.0},{:.0} · win {:.0},{:.0}", s.presses, p.x, p.y, p.window_x, p.window_y)
                    }).key("press-readout").w(292.0).font(type_::LABEL).color(t.text).instant_changes())
                    .child(
                        row().w(120.0).h(28.0).px(8.0).center_y().radius(radius::SM).fill(t.card).border(1.0, t.card_border)
                            .child(text_input(&s.field, type_::LABEL).key("cursor-field").w_full().color(t.text).on_edit(|s: &mut PressPointerDemo| &mut s.field)),
                    ),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Key, KeyEvent, Runtime};

    #[test]
    fn press_pointer_story_exercises_cursor_point_bounds_and_a11y() {
        let mut rt = Runtime::new(PressPointerDemo::default(), view, (520.0, 320.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let tree = rt.accessibility_json().to_string();
        assert!(tree.contains("Press & drag outside bounds"));
        assert!(tree.contains("App-wide press_cursor(Pointer)"));
        assert!(tree.contains("Disabled button"));

        // press_cursor wins over Grab header, keeps explicit Crosshair, arrow on disabled, I-beam on field.
        for (key, want) in [
            ("header-action", Cursor::Pointer),
            ("explicit-cursor", Cursor::Crosshair),
            ("disabled-action", Cursor::Default),
            ("cursor-field", Cursor::Text),
        ] {
            let (x, y) = rt.find(key).unwrap();
            rt.pointer_move(x, y);
            assert_eq!(rt.cursor(), want, "cursor over {key}");
        }

        // Hold press on bounds-btn and drag outside: bounds_btn reports outside, pill stays inside.
        let (bx, by) = rt.find("bounds-btn").unwrap();
        rt.pointer_move(bx, by);
        rt.pointer_down(bx, by);
        rt.frame(1.0 / 60.0);
        assert!(rt.state.card_inside && rt.state.pill_inside);
        rt.pointer_move(4.0, 4.0);
        rt.frame(1.0 / 60.0);
        assert!(!rt.state.card_inside, "hover_tracks_bounds transitions outside while held");
        assert!(rt.state.pill_inside, "default ancestor retains hover while held");
        rt.pointer_move(bx, by);
        rt.pointer_up(bx, by);
        rt.frame(1.0 / 60.0);
        assert!(rt.state.card_inside);
        let pt = rt.state.last_point.expect("pointer press records press_point");
        assert!((pt.window_x - bx).abs() < 0.01 && (pt.window_y - by).abs() < 0.01);

        // Keyboard Space activation sets press_point to None.
        rt.update(|_, tx| tx.focus("bounds-btn", true));
        rt.frame(0.0);
        rt.key(KeyEvent { key: Key::Text(" ".into()), mods: Default::default() });
        rt.frame(0.0);
        assert_eq!(rt.state.last_point, None, "keyboard press reports press_point == None");
    }

    #[test]
    fn press_pointer_story_passes_motion_gate() {
        let rt = Runtime::new(PressPointerDemo::default(), view, (520.0, 320.0), Env::new(true, 1.0));
        let mut run = rune::motion_gate::Run::new(rt, 120.0);
        run.idle(0.1);
        run.act("hover bounds", |rt| { let (x, y) = rt.find("bounds-btn").unwrap(); rt.pointer_move(x, y); });
        run.play(0.15);
        run.act("press bounds", |rt| { let (x, y) = rt.find("bounds-btn").unwrap(); rt.pointer_down(x, y); });
        run.play(0.15);
        run.act("drag outside", |rt| { rt.pointer_move(4.0, 4.0); });
        run.play(0.15);
        run.act("drag back inside", |rt| { let (x, y) = rt.find("bounds-btn").unwrap(); rt.pointer_move(x, y); });
        run.play(0.15);
        run.act("release bounds", |rt| { let (x, y) = rt.find("bounds-btn").unwrap(); rt.pointer_up(x, y); });
        run.play(0.3);
        run.quiet();
        run.play(0.2);
        let findings = run.finish();
        assert!(findings.is_empty(), "unexpected motion_gate findings: {findings:?}");
    }
}
