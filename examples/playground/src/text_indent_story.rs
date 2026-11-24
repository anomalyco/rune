//! The "Text/Indent & hanging" playground story: first-line `El::text_indent` for prose,
//! compact chat messages and bibliography entries with negative hanging indents, and RTL mirroring.

use rune::tok::{self, radius, type_};
use rune::{Color, Cx, Direction, El, Span, Story, col, rich, row, text};

pub(crate) struct TextIndentDemo {
    pub(crate) prose_indent: f32,
    pub(crate) rtl: bool,
}

impl Default for TextIndentDemo {
    fn default() -> Self {
        Self { prose_indent: 24.0, rtl: false }
    }
}

pub(crate) fn story() -> Story<TextIndentDemo> {
    Story::new("Text/Indent & hanging", TextIndentDemo::default, view)
        .trigger('i', "Cycle indent", |s, _| {
            s.prose_indent = if s.prose_indent < 20.0 { 24.0 } else if s.prose_indent < 36.0 { 40.0 } else { 0.0 };
        })
        .trigger('d', "Toggle LTR / RTL", |s, _| s.rtl = !s.rtl)
        .variant("Default", |_| {})
        .variant("Wide indent", |s| s.prose_indent = 40.0)
        .variant("RTL", |s| s.rtl = true)
}

pub(crate) fn view(s: &TextIndentDemo, cx: &mut Cx<TextIndentDemo>) -> El {
    let t = tok::theme(cx.system_dark());
    let accent = tok::accent(0, cx.system_dark());
    let dir = if s.rtl { Direction::Rtl } else { Direction::Ltr };

    let prose = if s.rtl {
        "هذه فقرة نصية تبدأ بمسافة بادئة في السطر الأول فقط، بينما تعود الأسطر الملتفة اللاحقة إلى الحافة اليمنى للحاوية كما في معيار CSS."
    } else {
        "First-line indentation shifts only the opening line inward from the inline-start edge, while subsequent wrapped lines align cleanly along the column margin."
    };

    let bib = if s.rtl {
        "المرجع الأول: يمتد السطر الأول إلى داخل الحاشية اليمنى باستخدام قيمة سالبة، وتصطف بقية الأسطر تحت عمود النص."
    } else {
        "Knuth, D. E., & Plass, M. F. (1981). Breaking paragraphs into lines. Software: Practice and Experience, 11(11), 1119–1184."
    };

    let chat_msg = "10:42 Ada: Shipped hanging text_indent so compact chat rows put timestamp + author on line 0 while wrapped lines align cleanly under the message column.";
    let chat_spans = vec![
        Span::new(0, 5).color(t.text_secondary).mono(),
        Span::new(6, 10).bold().color(accent),
    ];

    let mut prose_el = text(prose)
        .key("indent-prose")
        .w_full()
        .dir(dir)
        .font(type_::BODY)
        .color(t.text)
        .wrap()
        .selectable(true)
        .text_indent(s.prose_indent);
    if s.prose_indent == 0.0 {
        prose_el.set_text_indent(0.0);
    }

    let bib_el = text(bib)
    .pl(28.0)
    .key("indent-bib")
    .w_full()
    .dir(dir)
    .font(type_::BODY)
    .color(t.text)
    .wrap()
    .selectable(true)
    .text_indent(-28.0);

    let chat_el = rich(chat_msg, chat_spans)
        .key("indent-chat")
        .w_full()
        .pl(76.0)
        .font(type_::BODY)
        .color(t.text)
        .wrap()
        .selectable(true)
        .text_indent(-76.0);

    let pill = |key: &'static str, label: &'static str, on: bool, f: fn(&mut TextIndentDemo)| {
        row().key(key).h(26.0).px(10.0).center().radius(radius::SM)
            .fill(if on { accent } else { t.control_track })
            .hover(if on { accent } else { t.hover })
            .child(text(label).font(type_::LABEL).color(if on { Color::WHITE } else { t.text }))
            .focusable(true).role("button").label(label)
            .on_press(move |s: &mut TextIndentDemo| f(s))
    };

    col().w_full().h_full().center().padding(20.0).child(
        col().w(440.0).gap(12.0)
            .child(
                row().gap(6.0).center_y()
                    .child(pill("indent-0", "0 pt", s.prose_indent == 0.0, |s| s.prose_indent = 0.0))
                    .child(pill("indent-24", "24 pt", (s.prose_indent - 24.0).abs() < 0.1, |s| s.prose_indent = 24.0))
                    .child(pill("indent-40", "40 pt", (s.prose_indent - 40.0).abs() < 0.1, |s| s.prose_indent = 40.0))
                    .child(pill("indent-dir", if s.rtl { "RTL" } else { "LTR" }, s.rtl, |s| s.rtl = !s.rtl)),
            )
            .child(
                col().w_full().padding(12.0).gap(4.0).radius(radius::MD).fill(t.card).outline(1.0, 0.0, t.card_border)
                    .child(text(format!("First-line indent · text_indent({:.0})", s.prose_indent)).font(type_::LABEL).color(t.text_secondary))
                    .child(prose_el),
            )
            .child(
                col().w_full().padding(12.0).gap(4.0).radius(radius::MD).fill(t.card).outline(1.0, 0.0, t.card_border)
                    .child(text("Bibliography · pl(28).text_indent(-28)").font(type_::LABEL).color(t.text_secondary))
                    .child(bib_el),
            )
            .child(
                col().w_full().padding(12.0).gap(4.0).radius(radius::MD).fill(t.card).outline(1.0, 0.0, t.card_border)
                    .child(text("Compact chat · pl(76).text_indent(-76)").font(type_::LABEL).color(t.text_secondary))
                    .child(chat_el),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn text_indent_story_toggles_indent_and_rtl_and_exposes_controls() {
        let mut rt = Runtime::new(TextIndentDemo::default(), view, (520.0, 420.0), Env::new(true, 1.0));
        rt.frame(0.0);
        assert_eq!(rt.state.prose_indent, 24.0);
        let a11y = rt.accessibility_json().to_string();
        assert!(a11y.contains("Knuth, D. E.") && a11y.contains("40 pt"));

        let (x, y) = rt.find("indent-40").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.prose_indent, 40.0);

        let (dx, dy) = rt.find("indent-dir").unwrap();
        rt.pointer_down(dx, dy);
        rt.pointer_up(dx, dy);
        rt.frame(2.0 / 60.0);
        assert!(rt.state.rtl);
    }
}
