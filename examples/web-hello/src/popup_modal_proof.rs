//! Browser popup-modal accessibility mirror fixture, selected only by `?popup-modal-proof`.
use rune::{Align, AppSpec, Cx, El, HasPopup, Side, Transition, col, el, row, text};
use rune::tok::{self, type_};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = "
export function popupModalProof() { return new URLSearchParams(location.search).has('popup-modal-proof'); }
")]
extern "C" {
    #[wasm_bindgen(js_name = popupModalProof)]
    pub fn enabled() -> bool;
}

#[derive(Default)]
struct State {
    menu: bool,
    dialog: bool,
    list: bool,
    anchored_dialog: bool,
    menu_clicks: u32,
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let t = tok::theme(cx.system_dark());
    let btn = |key: &'static str, label: &'static str| {
        row().key(key).role("button").h(32.0).px(12.0).center_y().radius(6.0).fill(t.control_off)
            .focusable(true)
            .child(text(label).font(type_::LABEL).color(t.text))
    };
    let mut root = col().w_full().h_full().padding(16.0).gap(10.0).fill(t.bg)
        .child(
            btn("menu-button", "Edit")
                .expanded(s.menu)
                .has_popup(HasPopup::Menu)
                .controls("edit-menu")
                .on_press(|s: &mut State| {
                    s.menu_clicks += 1;
                    s.menu = !s.menu;
                }),
        )
        .child(btn("other-button", "Other").on_press(|_: &mut State| {}))
        .child(
            btn("open-dialog", "Open dialog")
                .on_press(|s: &mut State| {
                    s.dialog = true;
                    s.list = true;
                }),
        )
        .child(
            btn("confirm-trigger", "Confirm")
                .expanded(s.anchored_dialog)
                .has_popup(HasPopup::Dialog)
                .controls("confirm-dialog")
                .on_press(|s: &mut State| s.anchored_dialog = !s.anchored_dialog),
        );

    if s.menu {
        root = root.child(
            col().key("edit-menu").role("menu").label("Edit").padding(6.0).gap(4.0).radius(8.0).fill(t.card)
                .transition(Transition::none())
                .anchored_fit("menu-button", Side::Bottom, Align::Start, 4.0)
                .modal(true, |s: &mut State| s.menu = false)
                .child(
                    el().key("mi-copy").role("menuitem").label("Copy").focusable(true).size(120.0, 28.0)
                        .on_press(|s: &mut State| s.menu = false)
                        .child(text("Copy").font(type_::BODY).color(t.text)),
                ),
        );
    }

    if s.dialog {
        let fruit = row().key("fruit-trigger").role("combobox").label("Fruit").h(32.0).px(12.0).center_y()
            .radius(6.0).fill(t.control_off).focusable(true)
            .expanded(s.list)
            .has_popup(HasPopup::Listbox)
            .controls("fruit-list")
            .on_press(|s: &mut State| s.list = !s.list)
            .child(text("Fruit").font(type_::LABEL).color(t.text));
        root = root.child(
            col().key("settings-dialog").role("dialog").label("Settings").padding(12.0).gap(8.0).radius(8.0).fill(t.card)
                .transition(Transition::none())
                .absolute(40.0, 140.0)
                .modal(true, |s: &mut State| {
                    s.list = false;
                    s.dialog = false;
                })
                .child(fruit)
                .child(btn("save-btn", "Save").on_press(|_: &mut State| {})),
        );
        if s.list {
            root = root.child(
                col().key("fruit-list").role("listbox").label("Fruit").padding(6.0).gap(4.0).radius(8.0).fill(t.card)
                    .transition(Transition::none())
                    .anchored_fit("fruit-trigger", Side::Bottom, Align::Start, 4.0)
                    .modal(true, |s: &mut State| s.list = false)
                    .child(
                        el().key("opt-apple").role("option").label("Apple").focusable(true).size(120.0, 28.0)
                            .on_press(|s: &mut State| s.list = false)
                            .child(text("Apple").font(type_::BODY).color(t.text)),
                    ),
            );
        }
    }

    if s.anchored_dialog {
        root = root.child(
            col().key("confirm-dialog").role("dialog").label("Sure?").padding(12.0).gap(8.0).radius(8.0).fill(t.card)
                .transition(Transition::none())
                .anchored_fit("confirm-trigger", Side::Bottom, Align::Start, 4.0)
                .modal(true, |s: &mut State| s.anchored_dialog = false)
                .child(btn("confirm-ok", "OK").on_press(|s: &mut State| s.anchored_dialog = false)),
        );
    }

    root
}

pub fn run() {
    rune::run(AppSpec::new("Popup modal proof", (390.0, 640.0), State::default, view));
}
