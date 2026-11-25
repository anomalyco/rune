//! Browser `aria-autocomplete` mirror and focus-proxy fixture, selected only by `?auto-complete-proof`.
use rune::{Align, AppSpec, AutoComplete, Cx, El, HasPopup, Side, TextEdit, Transition, col, el, row, text, text_input};
use rune::tok::{self, type_};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = "
export function autoCompleteProof() { return new URLSearchParams(location.search).has('auto-complete-proof'); }
")]
extern "C" {
    #[wasm_bindgen(js_name = autoCompleteProof)]
    pub fn enabled() -> bool;
}

struct State {
    fruit: TextEdit,
    search: TextEdit,
    open: bool,
    mode: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            fruit: TextEdit::new("Ap"),
            search: TextEdit::new(""),
            open: false,
            mode: 0,
        }
    }
}

impl State {
    fn auto_complete(&self) -> Option<AutoComplete> {
        match self.mode % 4 {
            0 => Some(AutoComplete::List),
            1 => Some(AutoComplete::Inline),
            2 => Some(AutoComplete::Both),
            _ => None,
        }
    }

    fn mode_label(&self) -> &'static str {
        match self.mode % 4 {
            0 => "list",
            1 => "inline",
            2 => "both",
            _ => "none",
        }
    }
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let t = tok::theme(cx.system_dark());
    let btn = |key: &'static str, label: &'static str| {
        row().key(key).role("button").h(32.0).px(12.0).center_y().radius(6.0).fill(t.control_off)
            .focusable(true)
            .child(text(label).font(type_::LABEL).color(t.text))
    };

    let mut combo = text_input(&s.fruit, type_::BODY)
        .single_line()
        .key("fruit-combo")
        .role("combobox")
        .label("Fruit")
        .placeholder("Type a fruit", t.text_secondary)
        .color(t.text)
        .size(220.0, 32.0)
        .px(10.0)
        .radius(6.0)
        .fill(t.card)
        .expanded(s.open)
        .has_popup(HasPopup::Listbox)
        .controls("fruit-list")
        .on_edit(|s: &mut State| &mut s.fruit);
    if let Some(mode) = s.auto_complete() {
        combo = combo.auto_complete(mode);
    }

    let search = text_input(&s.search, type_::BODY)
        .single_line()
        .key("search-input")
        .label("Search items")
        .placeholder("Filter items", t.text_secondary)
        .color(t.text)
        .size(220.0, 32.0)
        .px(10.0)
        .radius(6.0)
        .fill(t.card)
        .auto_complete(AutoComplete::List)
        .active_descendant("search-opt-1")
        .on_edit(|s: &mut State| &mut s.search);

    let mut root = col().w_full().h_full().padding(16.0).gap(10.0).fill(t.bg)
        .child(text(format!("Mode: {}", s.mode_label())).key("mode-text").font(type_::LABEL).color(t.text))
        .child(combo)
        .child(
            row().gap(8.0)
                .child(btn("toggle-popup", "Toggle popup").on_press(|s: &mut State| s.open = !s.open))
                .child(btn("cycle-mode", "Cycle mode").on_press(|s: &mut State| s.mode = (s.mode + 1) % 4))
                .child(btn("other-btn", "Other").on_press(|_: &mut State| {})),
        )
        .child(search)
        .child(
            col().key("search-list").role("listbox").label("Search results").gap(4.0).padding(6.0).radius(6.0).fill(t.card)
                .child(
                    el().key("search-opt-1").role("option").label("Apricot").size(200.0, 26.0)
                        .child(text("Apricot").font(type_::BODY).color(t.text)),
                ),
        );

    if s.open {
        root = root.child(
            col().key("fruit-list").role("listbox").label("Fruit").padding(6.0).gap(4.0).radius(8.0).fill(t.card)
                .transition(Transition::none())
                .anchored_fit("fruit-combo", Side::Bottom, Align::Start, 4.0)
                .modal(true, |s: &mut State| s.open = false)
                .child(
                    el().key("opt-apple").role("option").label("Apple").focusable(true).size(180.0, 28.0)
                        .on_press(|s: &mut State| {
                            s.fruit = TextEdit::new("Apple");
                            s.open = false;
                        })
                        .child(text("Apple").font(type_::BODY).color(t.text)),
                ),
        );
    }

    root
}

pub fn run() {
    rune::run(AppSpec::new("AutoComplete proof", (390.0, 640.0), State::default, view));
}
