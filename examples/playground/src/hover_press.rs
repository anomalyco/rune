//! The "Controls/Buttons/Hover & press" story: declarative replacement hover/press fill and
//! border colours (`hover_fill`, `press_fill`, `hover_border`, `press_border`) and an overlying
//! `.hit()` popup cover blocking hover underneath.

use rune::tok::{self, radius, type_};
use rune::{Color, Curve, Cx, Easing, El, Transition, col, row, text};

#[derive(Default)]
pub(crate) struct HoverPressDemo {
    pub(crate) popup_open: bool,
    pub(crate) pass_through: bool,
    pub(crate) hover_hits: usize,
    pub(crate) presses: usize,
}

pub(crate) fn view(s: &HoverPressDemo, cx: &mut Cx<HoverPressDemo>) -> El {
    let t = tok::theme(cx.system_dark());
    let accent = tok::accent(0, cx.system_dark());
    let accent_hover = rune::color::hex("#274DEA").unwrap();
    let accent_press = rune::color::hex("#1A34A8").unwrap();
    let grad_top = rune::color::hex("#4F46E5").unwrap();
    let grad_bot = rune::color::hex("#0EA5E9").unwrap();
    let press_down = Curve::tween(0.08, Easing::EASE_OUT);
    let press_up = Curve::tween(0.16, Easing::EASE_OUT);

    let solid_btn = row().key("solid-btn").h(34.0).px(14.0).center().radius(radius::SM)
        .fill(accent).hover_fill(accent_hover).press_fill(accent_press)
        .border(1.5, accent_hover).hover_border(accent_press).press_border(Color::WHITE)
        .press_motion(press_down, press_up)
        .role("button").label("Accent replacement").focusable(true)
        .on_press(|s: &mut HoverPressDemo| s.presses += 1)
        .child(text("Accent").font(type_::LABEL).color(Color::WHITE));

    let quiet_btn = row().key("quiet-btn").h(34.0).px(14.0).center().radius(radius::SM)
        .fill(Color::TRANSPARENT).hover_fill(t.control_track).press_fill(t.hover)
        .border(1.0, Color::TRANSPARENT).hover_border(t.card_border).press_border(accent)
        .press_motion(press_down, press_up)
        .role("button").label("Quiet replacement").focusable(true)
        .on_press(|s: &mut HoverPressDemo| s.presses += 1)
        .child(text("Quiet").font(type_::LABEL).color(t.text));

    let grad_btn = row().key("grad-btn").h(34.0).px(14.0).center().radius(radius::SM)
        .fill(grad_top).gradient(grad_top, grad_bot)
        .hover_fill(accent_hover).press_fill(Color::TRANSPARENT)
        .border(1.0, t.card_border).hover_border(Color::WHITE).press_border(accent)
        .press_motion(press_down, press_up)
        .role("button").label("Gradient to flat").focusable(true)
        .on_press(|s: &mut HoverPressDemo| s.presses += 1)
        .child(text("Gradient → flat").font(type_::LABEL).color(Color::WHITE));

    let under_row = |key: &'static str, label: &'static str| {
        row().key(key).w_full().h(34.0).px(12.0).center_y().justify_between().radius(radius::SM)
            .fill(t.card).hover_fill(t.control_track).press_fill(t.hover)
            .border(1.0, t.card_border).hover_border(accent)
            .focus_highlight(accent.alpha(0.18))
            .role("button").label(label).focusable(true)
            .on_hover(|s: &mut HoverPressDemo| s.hover_hits += 1)
            .on_press(|s: &mut HoverPressDemo| s.presses += 1)
            .child(text(label).font(type_::BODY).color(t.text))
            .child(text("hover + focus_highlight").font(type_::LABEL).color(t.text_secondary))
    };

    let mut list = col().key("under-list").w_full().gap(6.0)
        .child(under_row("under-row-0", "Row beneath popup"))
        .child(under_row("under-row-1", "Uncovered row below"));
    if s.popup_open {
        let mut cover = col().key("popup-cover").absolute(8.0, -4.0).w(220.0).h(44.0).px(12.0).center_y()
            .radius(radius::MD).fill(t.card.alpha(0.95)).border(1.0, accent).shadow(4.0, 12.0, t.shadow_soft)
            .transition(Transition::opacity()).hit()
            .child(text(if s.pass_through { "Pass-through cover" } else { "Popup cover (.hit() blocks hover)" }).font(type_::LABEL).color(t.text));
        if s.pass_through {
            cover = cover.hover_passes_below();
        }
        list = list.child(cover);
    }

    let toggle_btn = row().key("toggle-popup").h(28.0).px(12.0).center().radius(radius::SM)
        .fill(if s.popup_open { accent } else { t.control_track })
        .hover_fill(if s.popup_open { accent_hover } else { t.hover })
        .role("button").label("Toggle popup cover").focusable(true)
        .on_press(|s: &mut HoverPressDemo| s.popup_open = !s.popup_open)
        .child(text("Popup cover").font(type_::LABEL).color(if s.popup_open { Color::WHITE } else { t.text }));

    let pass_btn = row().key("toggle-pass").h(28.0).px(12.0).center().radius(radius::SM)
        .fill(if s.pass_through { accent } else { t.control_track })
        .hover_fill(if s.pass_through { accent_hover } else { t.hover })
        .role("button").label("Toggle hover_passes_below").focusable(true)
        .on_press(|s: &mut HoverPressDemo| s.pass_through = !s.pass_through)
        .child(text("Pass-through").font(type_::LABEL).color(if s.pass_through { Color::WHITE } else { t.text }));

    col().w_full().h_full().center().padding(16.0).child(
        col().w_full().max_w(380.0).gap(12.0)
            .child(text("Declarative hover_fill / press_fill / hover_border / press_border").font(type_::LABEL).color(t.text_secondary))
            .child(row().gap(8.0).child(solid_btn).child(quiet_btn).child(grad_btn))
            .child(row().gap(8.0).center_y().child(toggle_btn).child(pass_btn)
                .child(text(format!("hovers {} · presses {}", s.hover_hits, s.presses)).w(140.0).font(type_::LABEL).color(t.text_secondary).instant_changes()))
            .child(list),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn hover_press_story_exposes_accessible_buttons_and_popup_blocks_hover() {
        let mut rt = Runtime::new(HoverPressDemo::default(), view, (520.0, 320.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let tree = rt.accessibility_json().to_string();
        assert!(tree.contains("Accent replacement"));
        assert!(tree.contains("Quiet replacement"));
        assert!(tree.contains("Toggle popup cover"));

        let (rx, ry) = rt.find("under-row-0").unwrap();
        rt.pointer_move(rx, ry);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.hover_hits, 1);

        let (tx, ty) = rt.find("toggle-popup").unwrap();
        rt.pointer_move(tx, ty);
        rt.pointer_down(tx, ty);
        rt.pointer_up(tx, ty);
        rt.frame(1.0 / 60.0);
        assert!(rt.state.popup_open);
        let hits_before = rt.state.hover_hits;
        rt.pointer_move(rx, ry);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.hover_hits, hits_before, "popup cover blocks on_hover underneath");
    }

    #[test]
    fn hover_press_story_passes_motion_gate() {
        let rt = Runtime::new(HoverPressDemo::default(), view, (520.0, 320.0), Env::new(true, 1.0));
        let mut run = rune::motion_gate::Run::new(rt, 120.0);
        run.idle(0.1);
        run.act("hover solid", |rt| { let (x, y) = rt.find("solid-btn").unwrap(); rt.pointer_move(x, y); });
        run.play(0.2);
        run.act("press solid", |rt| { let (x, y) = rt.find("solid-btn").unwrap(); rt.pointer_down(x, y); });
        run.play(0.2);
        run.act("release solid", |rt| { let (x, y) = rt.find("solid-btn").unwrap(); rt.pointer_up(x, y); });
        run.play(0.3);
        run.act("hover quiet", |rt| { let (x, y) = rt.find("quiet-btn").unwrap(); rt.pointer_move(x, y); });
        run.play(0.2);
        run.act("hover grad", |rt| { let (x, y) = rt.find("grad-btn").unwrap(); rt.pointer_move(x, y); });
        run.play(0.2);
        run.act("open popup down", |rt| { let (x, y) = rt.find("toggle-popup").unwrap(); rt.pointer_move(x, y); rt.pointer_down(x, y); });
        run.play(0.05);
        run.act("open popup up", |rt| { let (x, y) = rt.find("toggle-popup").unwrap(); rt.pointer_up(x, y); });
        run.play(0.35);
        run.act("hover cover", |rt| { let (x, y) = rt.find("popup-cover").unwrap(); rt.pointer_move(x, y); });
        run.play(0.2);
        run.quiet();
        run.play(0.2);
        let findings = run.finish();
        assert!(findings.is_empty(), "unexpected motion_gate findings: {findings:?}\n{}", rune::motion_gate::dump(&run.rec, "solid-btn", 60, 75));
    }
}
