//! A small chat view with safe-area insets, text input and clipboard copy, built for both
//! desktop and `wasm32` browsers (`scripts/web-build.sh web-hello`).

#![forbid(unsafe_code)]

use rune::{AppSpec, Cx, El, ScrollSpec, TextEdit, Tx, col, el, row, text, text_input};
use rune::tok::{self, type_};
#[cfg(target_arch = "wasm32")]
mod a11y_proof;
#[cfg(target_arch = "wasm32")]
mod aria_proof;
#[cfg(target_arch = "wasm32")]
mod auto_complete_proof;
#[cfg(target_arch = "wasm32")]
mod image_proof;
#[cfg(target_arch = "wasm32")]
mod input_proof;
#[cfg(target_arch = "wasm32")]
mod popup_modal_proof;
#[cfg(target_arch = "wasm32")]
mod rtl_proof;

struct State {
    messages: Vec<String>,
    draft: TextEdit,
}

impl Default for State {
    fn default() -> Self {
        Self {
            messages: (1..=24).map(|n| format!("Message {n}: Hello from Rune." )).collect(),
            draft: TextEdit::new(""),
        }
    }
}

fn send(state: &mut State, tx: &mut Tx) {
    if state.draft.text.trim().is_empty() { return; }
    state.messages.push(state.draft.text.clone());
    state.draft.clear();
    tx.scroll_to_bottom("messages");
}

fn view(state: &State, cx: &mut Cx<State>) -> El {
    let dark = cx.system_dark();
    let t = tok::theme(dark);
    let [top, right, bottom, left] = cx.safe_area();
    let mut messages = col().w_full().gap(12.0).py(16.0);
    for (i, message) in state.messages.iter().enumerate() {
        messages = messages.child(text(message.clone()).key(format!("message-{i}"))
            .font(type_::BODY).color(t.text).w_full().padding(16.0).radius(12.0).fill(t.card));
    }
    let composer = text_input(&state.draft, type_::BODY).key("composer").label("Message")
        .placeholder("Message", t.text_secondary).color(t.text)
        .grow(1.0).h(48.0).padding(12.0).radius(12.0).fill(t.card)
        .on_edit(|s: &mut State| &mut s.draft).on_submit(send);
    let send_button = el().key("send").role("button").disabled(state.draft.text.trim().is_empty())
        .h(48.0).px(16.0).center().radius(12.0).fill(t.control_off)
        .child(text("Send").font(type_::LABEL).color(t.text)).on_press_tx(send);
    let copy = el().key("copy").role("button").h(44.0).px(8.0).center()
        .child(text("Copy last").font(type_::LABEL).color(t.text_secondary)).on_press_tx(|s: &mut State, tx| {
            if let Some(message) = s.messages.last() { tx.copy(message.clone()); }
        });
    let title = text("Messages").font(type_::TITLE).color(t.text).grow(1.0);
    #[cfg(target_arch = "wasm32")]
    let title = title.on_press(|_: &mut State| { rune::web::open_url("https://www.rust-lang.org/").ok(); });
    col().w_full().h_full().pt(16.0 + top).pr(16.0 + right).pb(16.0 + bottom).pl(16.0 + left).gap(8.0).fill(t.bg)
        .child(row().key("header").role("toolbar").label("Conversation actions").w_full().center_y().child(title).child(copy))
        .child(col().key("messages").w_full().grow(1.0).shrink()
            .scroll(ScrollSpec { stick_bottom: true }).scroll_edge_fade(16.0).child(messages))
        .child(row().w_full().gap(8.0).child(composer).child(send_button))
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(inline_js = "
export function recordFrameDrawn(messages, elements) {
    const prev = window.__runeFramesDrawn;
    window.__runeFramesDrawn = {
        count: (prev ? prev.count : 0) + 1,
        messages,
        elements,
    };
}
")]
extern "C" {
    #[wasm_bindgen(js_name = recordFrameDrawn)]
    fn record_frame_drawn(messages: usize, elements: usize);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen::prelude::wasm_bindgen(start))]
pub fn start() {
    #[cfg(target_arch = "wasm32")]
    if a11y_proof::enabled() { a11y_proof::run(); return; }
    #[cfg(target_arch = "wasm32")]
    if aria_proof::enabled() { aria_proof::run(); return; }
    #[cfg(target_arch = "wasm32")]
    if auto_complete_proof::enabled() { auto_complete_proof::run(); return; }
    #[cfg(target_arch = "wasm32")]
    if popup_modal_proof::enabled() { popup_modal_proof::run(); return; }
    #[cfg(target_arch = "wasm32")]
    if rtl_proof::enabled() { rtl_proof::run(); return; }
    #[cfg(target_arch = "wasm32")]
    if input_proof::enabled() { input_proof::run(); return; }
    #[cfg(target_arch = "wasm32")]
    if image_proof::enabled() { image_proof::run(); return; }
    #[cfg(target_arch = "wasm32")]
    if image_proof::scaled_enabled() { image_proof::run_scaled(); return; }
    let app = rune::App::new(AppSpec::new("Messages", (390.0, 760.0), State::default, view));
    #[cfg(target_arch = "wasm32")]
    let app = app.on_frame_drawn(|rt| {
        record_frame_drawn(rt.state.messages.len(), rt.elements().len());
    });
    app.run();
}
