//! Browser RTL direction, language, and locale proof, selected only by `?rtl-proof`: exercises
//! `Cx::locale()`, `Cx::system_languages()`, `rune::text::set_language`, root `[data-rune-a11y]`
//! `dir` and `lang` attributes, per-element `dir` overrides on direction flips, and logical
//! `Align::Start`/`Align::End` text alignment.

use rune::tok::{self, type_};
use rune::{Align, AppSpec, Cx, Direction, El, col, el, row, text};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = "
export function rtlProofEnabled() {
    return new URLSearchParams(location.search).has('rtl-proof');
}
")]
extern "C" {
    #[wasm_bindgen(js_name = rtlProofEnabled)]
    pub fn enabled() -> bool;
}

struct State {
    rtl: bool,
    lang: Option<&'static str>,
}

impl Default for State {
    fn default() -> Self {
        rune::text::set_language(Some("ar-EG"));
        Self { rtl: true, lang: Some("ar-EG") }
    }
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let t = tok::theme(false);
    let root_dir = if s.rtl { Direction::Rtl } else { Direction::Ltr };
    let locale = cx.locale().unwrap_or("none").to_owned();
    let langs = cx.system_languages().join(", ");

    let controls = row()
        .key("controls")
        .role("toolbar")
        .label("Direction controls")
        .w_full()
        .gap(8.0)
        .child(
            el()
                .key("toggle-dir")
                .role("button")
                .label("Toggle direction")
                .focusable(true)
                .h(32.0)
                .px(12.0)
                .center()
                .radius(6.0)
                .fill(t.control_off)
                .child(text("Toggle direction").font(type_::LABEL).color(t.text))
                .on_press(|s: &mut State| {
                    s.rtl = !s.rtl;
                    s.lang = Some(if s.rtl { "ar-EG" } else { "en-US" });
                    rune::text::set_language(s.lang);
                }),
        )
        .child(
            el()
                .key("clear-lang")
                .role("button")
                .label("Clear language")
                .focusable(true)
                .h(32.0)
                .px(12.0)
                .center()
                .radius(6.0)
                .fill(t.control_off)
                .child(text("Clear language").font(type_::LABEL).color(t.text))
                .on_press(|s: &mut State| {
                    s.lang = None;
                    rune::text::set_language(None);
                }),
        );

    let ltr_island = col()
        .key("ltr-wrapper")
        .dir(Direction::Ltr)
        .w_full()
        .padding(8.0)
        .radius(8.0)
        .fill(t.card)
        .gap(4.0)
        .child(
            text("README.md")
                .key("ltr-file")
                .align(Align::Start)
                .font(type_::BODY)
                .color(t.text),
        )
        .child(
            row()
                .key("ltr-bar")
                .role("toolbar")
                .label("File actions")
                .gap(8.0)
                .child(
                    el()
                        .key("open-btn")
                        .role("button")
                        .label("Open")
                        .h(28.0)
                        .px(10.0)
                        .center()
                        .radius(6.0)
                        .fill(t.control_off)
                        .child(text("Open").font(type_::LABEL).color(t.text))
                        .on_press(|_: &mut State| {}),
                )
                .child(
                    text("إرسال")
                        .key("inner-rtl")
                        .dir(Direction::Rtl)
                        .font(type_::BODY)
                        .color(t.text),
                ),
        );

    col()
        .dir(root_dir)
        .w_full()
        .h_full()
        .padding(16.0)
        .gap(10.0)
        .fill(t.bg)
        .child(
            text("واجهة عربية")
                .key("rtl-heading")
                .role("heading")
                .level(1)
                .align(Align::Start)
                .font(type_::TITLE)
                .color(t.text),
        )
        .child(
            text("افتح README.md الآن")
                .key("rtl-start")
                .align(Align::Start)
                .font(type_::BODY)
                .color(t.text),
        )
        .child(
            text("محاذاة النهاية")
                .key("rtl-end")
                .align(Align::End)
                .font(type_::BODY)
                .color(t.text_secondary),
        )
        .child(
            text("Auto Latin text")
                .key("auto-latin")
                .dir(Direction::Auto)
                .font(type_::BODY)
                .color(t.text),
        )
        .child(ltr_island)
        .child(
            text(format!("locale:{locale} | langs:{langs}"))
                .key("locale-readout")
                .dir(Direction::Ltr)
                .font(type_::BODY)
                .color(t.text_secondary),
        )
        .child(controls)
}

pub fn run() {
    rune::run(AppSpec::new("Rune Web RTL Proof", (440.0, 520.0), State::default, view));
}
