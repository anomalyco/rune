//! Browser WAI-ARIA mirror and focus-proxy fixture, selected only by `?aria-proof`.
use rune::{AppSpec, Cx, El, HasPopup, Orientation, col, row, text};
use rune::tok::{self, type_};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = "
export function ariaProof() { return new URLSearchParams(location.search).has('aria-proof'); }
")]
extern "C" {
    #[wasm_bindgen(js_name = ariaProof)]
    pub fn enabled() -> bool;
}

struct State {
    pending: bool,
    open: bool,
    presses: u32,
}

impl Default for State {
    fn default() -> Self {
        Self { pending: true, open: true, presses: 0 }
    }
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let t = tok::theme(cx.system_dark());
    let mut root = col().w_full().h_full().padding(16.0).gap(12.0).fill(t.bg)
        .child(
            row().key("pending-save").role("button").h(36.0).px(12.0).center_y().radius(8.0).fill(t.card)
                .focusable(true)
                .aria_disabled(s.pending)
                .on_press(|s: &mut State| s.presses += 1)
                .child(text(if s.pending { "Saving" } else { "Save" }).font(type_::LABEL).color(t.text)),
        )
        .child(
            row().key("toggle-pending").role("button").h(36.0).px(12.0).center_y().radius(8.0).fill(t.control_off)
                .focusable(true)
                .on_press(|s: &mut State| s.pending = !s.pending)
                .child(text("Toggle pending").font(type_::LABEL).color(t.text)),
        )
        .child(
            row().key("tabs").role("tablist").label("Sections").orientation(Orientation::Horizontal).gap(8.0)
                .child(
                    row().key("tab-a").role("tab").h(32.0).px(10.0).center_y().radius(6.0).fill(t.card)
                        .focusable(true).focus_selected(true)
                        .on_press(|_: &mut State| {})
                        .child(text("Alpha").font(type_::LABEL).color(t.text)),
                ),
        )
        .child(
            row().key("fruit-trigger").role("button").h(36.0).px(12.0).center_y().radius(8.0).fill(t.control_off)
                .focusable(true)
                .expanded(s.open)
                .has_popup(HasPopup::Listbox)
                .controls("fruit-list")
                .owns("fruit-list")
                .on_press(|s: &mut State| s.open = !s.open)
                .child(text("Fruit").font(type_::LABEL).color(t.text)),
        );
    if s.open {
        root = root.child(
            col().key("fruit-list").role("listbox").label("Fruit options").gap(4.0).padding(8.0).radius(8.0).fill(t.card)
                .multiselectable(true)
                .required(true)
                .orientation(Orientation::Vertical)
                .child(
                    row().key("opt-apple").role("option").h(32.0).px(8.0).center_y()
                        .focusable(true).focus_selected(true)
                        .on_press(|_: &mut State| {})
                        .child(text("Apple").font(type_::BODY).color(t.text)),
                ),
        );
    }
    root
}

pub fn run() {
    rune::run(AppSpec::new("ARIA proof", (390.0, 760.0), State::default, view));
}
