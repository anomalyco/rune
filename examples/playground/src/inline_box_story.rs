//! The "Text/Inline boxes" playground story: `El::inline` with `InlineBox`, `InlineAlign`,
//! interactive chips inside wrapped prose, alignment modes, narrow-column whole-box wrapping,
//! and RTL placement.

use rune::tok::{self, radius, type_};
use rune::{Color, Cx, Direction, El, InlineAlign, InlineBox, Story, col, row, text};

pub(crate) struct InlineBoxDemo {
    pub(crate) align: InlineAlign,
    pub(crate) narrow: bool,
    pub(crate) rtl: bool,
    pub(crate) opens: usize,
    pub(crate) edits: usize,
}

impl Default for InlineBoxDemo {
    fn default() -> Self {
        Self {
            align: InlineAlign::TextCenter,
            narrow: false,
            rtl: false,
            opens: 0,
            edits: 0,
        }
    }
}

pub(crate) fn story() -> Story<InlineBoxDemo> {
    Story::new("Text/Inline boxes", InlineBoxDemo::default, view)
        .trigger('a', "Cycle alignment", |s, _| {
            s.align = match s.align {
                InlineAlign::TextCenter => InlineAlign::Baseline,
                InlineAlign::Baseline => InlineAlign::Middle,
                InlineAlign::Middle => InlineAlign::TextTop,
                InlineAlign::TextTop => InlineAlign::TextBottom,
                _ => InlineAlign::TextCenter,
            };
        })
        .trigger('w', "Toggle narrow wrap", |s, _| s.narrow = !s.narrow)
        .trigger('d', "Toggle LTR / RTL", |s, _| s.rtl = !s.rtl)
        .variant("Default", |_| {})
        .variant("Narrow wrap", |s| s.narrow = true)
        .variant("Baseline", |s| s.align = InlineAlign::Baseline)
        .variant("RTL", |s| s.rtl = true)
}

fn align_name(a: InlineAlign) -> &'static str {
    match a {
        InlineAlign::Baseline => "Baseline",
        InlineAlign::TextCenter => "TextCenter",
        InlineAlign::Middle => "Middle",
        InlineAlign::TextTop => "TextTop",
        InlineAlign::TextBottom => "TextBottom",
        _ => "Other",
    }
}

pub(crate) fn view(s: &InlineBoxDemo, cx: &mut Cx<InlineBoxDemo>) -> El {
    let t = tok::theme(cx.system_dark());
    let accent = tok::accent(0, cx.system_dark());
    let accent2 = tok::accent(4, cx.system_dark());
    let dir = if s.rtl { Direction::Rtl } else { Direction::Ltr };
    let card_w = if s.narrow { 220.0 } else { 420.0 };

    let code_slot = InlineBox::new(84.0, 20.0).align(s.align);
    let action_slot = InlineBox::new(56.0, 20.0).align(s.align);
    let badge_slot = InlineBox::new(28.0, 28.0).align(s.align);

    let code_chip = row()
        .key("inline-code-chip")
        .center()
        .radius(radius::SM)
        .fill(t.control_track)
        .hover(t.hover)
        .outline(1.0, 0.0, t.card_border)
        .role("button")
        .label("Open config.toml")
        .focusable(true)
        .on_press(|s: &mut InlineBoxDemo| s.opens += 1)
        .child(text("config.toml").font(type_::LABEL.mono()).color(t.text));

    let action_chip = row()
        .key("inline-edit-chip")
        .center()
        .radius(radius::SM)
        .fill(accent)
        .role("button")
        .label("Edit file")
        .focusable(true)
        .on_press(|s: &mut InlineBoxDemo| s.edits += 1)
        .child(text("Edit ->").font(type_::LABEL).color(Color::WHITE));

    let tall_badge = row()
        .key("inline-tall-badge")
        .center()
        .radius(14.0)
        .fill(accent2)
        .child(text("2x").font(type_::LABEL).color(Color::WHITE));

    let prose_el = if s.rtl {
        let rtl_body = "افتح ملف  ثم اضغط  للتعديل داخل السطر.";
        let at0 = "افتح ملف ".len();
        let at1 = "افتح ملف  ثم اضغط ".len();
        text(rtl_body)
            .key("inline-prose")
            .w_full()
            .dir(dir)
            .font(type_::BODY)
            .color(t.text)
            .wrap()
            .selectable(true)
            .inline(at0, code_slot, code_chip)
            .inline(at1, action_slot, action_chip)
    } else {
        let body = "Open  to inspect settings, click  to apply changes, and check  for taller-box line growth.";
        let at0 = "Open ".len();
        let at1 = "Open  to inspect settings, click ".len();
        let at2 = "Open  to inspect settings, click  to apply changes, and check ".len();
        text(body)
            .key("inline-prose")
            .w_full()
            .dir(dir)
            .font(type_::BODY)
            .color(t.text)
            .wrap()
            .selectable(true)
            .inline(at0, code_slot, code_chip)
            .inline(at1, action_slot, action_chip)
            .inline(at2, badge_slot, tall_badge)
    };

    let pill = |key: &'static str, label: &'static str, on: bool, f: fn(&mut InlineBoxDemo)| {
        row()
            .key(key)
            .h(26.0)
            .px(10.0)
            .center()
            .radius(radius::SM)
            .fill(if on { accent } else { t.control_track })
            .hover(if on { accent } else { t.hover })
            .child(text(label).font(type_::LABEL).color(if on { Color::WHITE } else { t.text }))
            .focusable(true)
            .role("button")
            .label(label)
            .on_press(move |s: &mut InlineBoxDemo| f(s))
    };

    col().w_full().h_full().center().padding(20.0).child(
        col().w(440.0).gap(12.0)
            .child(
                row().gap(6.0).center_y()
                    .child(pill("align-tc", "TextCenter", s.align == InlineAlign::TextCenter, |s| s.align = InlineAlign::TextCenter))
                    .child(pill("align-base", "Baseline", s.align == InlineAlign::Baseline, |s| s.align = InlineAlign::Baseline))
                    .child(pill("align-mid", "Middle", s.align == InlineAlign::Middle, |s| s.align = InlineAlign::Middle))
                    .child(pill("wrap-toggle", if s.narrow { "Narrow" } else { "Wide" }, s.narrow, |s| s.narrow = !s.narrow))
                    .child(pill("dir-toggle", if s.rtl { "RTL" } else { "LTR" }, s.rtl, |s| s.rtl = !s.rtl)),
            )
            .child(
                col().w(card_w).padding(14.0).gap(8.0).radius(radius::MD).fill(t.card).outline(1.0, 0.0, t.card_border)
                    .child(
                        text(format!(
                            "InlineAlign::{} · opens {} · edits {}",
                            align_name(s.align),
                            s.opens,
                            s.edits,
                        ))
                        .key("inline-status")
                        .font(type_::LABEL)
                        .color(t.text_secondary),
                    )
                    .child(prose_el),
            ),
    )
}
