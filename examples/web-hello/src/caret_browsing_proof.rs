//! Browser `El::caret_browsing(true)` focus-proxy and DOM Selection mirror fixture, selected only
//! by `?caret-browsing-proof`.
use rune::{AppSpec, Cx, El, col, text};
use rune::tok::{self, type_};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = "
export function caretBrowsingProof() { return new URLSearchParams(location.search).has('caret-browsing-proof'); }
")]
extern "C" {
    #[wasm_bindgen(js_name = caretBrowsingProof)]
    pub fn enabled() -> bool;
}

fn view(_: &(), cx: &mut Cx<()>) -> El {
    let t = tok::theme(cx.system_dark());
    col()
        .w_full()
        .h_full()
        .padding(16.0)
        .gap(12.0)
        .fill(t.bg)
        .child(
            col()
                .key("doc")
                .label("Caret browsing proof")
                .w(320.0)
                .gap(6.0)
                .selection_group(true)
                .caret_browsing(true)
                .child(text("Alpha beta").key("p1").font(type_::BODY).color(t.text).selectable(true))
                .child(text("Gamma delta").key("p2").font(type_::BODY).color(t.text).selectable(true)),
        )
}

pub fn run() {
    rune::App::new(AppSpec::new("Caret Browsing Proof", (390.0, 320.0), || (), view)).run();
}
