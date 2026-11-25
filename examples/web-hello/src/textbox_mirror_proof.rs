//! Browser accessibility mirror proof for unfocused and focused textboxes (`?textbox-mirror-proof`).
use rune::{App, AppSpec, Color, Cx, El, TextEdit, col, el, text, text_input, tok};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = "
export function textboxMirrorProofEnabled() { return new URLSearchParams(location.search).has('textbox-mirror-proof'); }
export function textboxMirrorProofReport(name, reason, secretLen, activeKey) {
    window.__textboxMirrorProof = { name, reason, secretLen, activeKey, polls: (window.__textboxMirrorProof?.polls ?? 0) + 1 };
}
")]
extern "C" {
    #[wasm_bindgen(js_name = textboxMirrorProofEnabled)]
    pub fn enabled() -> bool;
    #[wasm_bindgen(js_name = textboxMirrorProofReport)]
    fn report(name: &str, reason: &str, secret_len: usize, active_key: &str);
}

struct State {
    name: TextEdit,
    reason: TextEdit,
    secret: TextEdit,
    query: TextEdit,
    readonly: TextEdit,
    disabled: TextEdit,
    invalid: TextEdit,
}

impl Default for State {
    fn default() -> Self {
        Self {
            name: TextEdit::new("Ada"),
            reason: TextEdit::new("Line 1\nLine 2"),
            secret: TextEdit::new("hunter2secret").secure(),
            query: TextEdit::new(""),
            readonly: TextEdit::new("fixed-token"),
            disabled: TextEdit::new("locked-value"),
            invalid: TextEdit::new("bad@"),
        }
    }
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let ph_color = Color::WHITE;
    let field = |edit: &TextEdit| text_input(edit, tok::type_::BODY).w(320.0).h(40.0);
    col().role("main").w_full().h_full().padding(16.0).gap(8.0)
        .child(
            field(&s.name).key("name").single_line().label("Name")
                .placeholder("Your name", ph_color).required(true)
                .on_edit(|s: &mut State| &mut s.name),
        )
        .child(
            field(&s.reason).key("reason").h(72.0).label("Reason")
                .placeholder("Explain why", ph_color)
                .on_edit(|s: &mut State| &mut s.reason),
        )
        .child(
            field(&s.secret).key("secret").single_line().label("Password")
                .placeholder("Enter password", ph_color).required(true)
                .on_edit(|s: &mut State| &mut s.secret),
        )
        .child(
            field(&s.query).key("query").single_line()
                .placeholder("Search query", ph_color)
                .on_edit(|s: &mut State| &mut s.query),
        )
        .child(
            field(&s.readonly).key("readonly").single_line().label("Token")
                .read_only(true)
                .on_edit(|s: &mut State| &mut s.readonly),
        )
        .child(
            field(&s.disabled).key("disabled").single_line().label("Locked")
                .disabled(true)
                .on_edit(|s: &mut State| &mut s.disabled),
        )
        .child(
            field(&s.invalid).key("invalid").single_line().label("Email")
                .invalid(true)
                .on_edit(|s: &mut State| &mut s.invalid),
        )
        .child(
            el().key("blur-btn").role("button").focusable(true).w(160.0).h(36.0)
                .child(text("Done"))
                .on_press(|_: &mut State| {}),
        )
}

pub fn run() {
    let mut spec = AppSpec::new("Textbox mirror proof", (390.0, 760.0), State::default, view);
    spec.poll = Some(|s, _, _| {
        let active = if s.name.focused { "name" }
            else if s.reason.focused { "reason" }
            else if s.secret.focused { "secret" }
            else if s.query.focused { "query" }
            else if s.readonly.focused { "readonly" }
            else if s.invalid.focused { "invalid" }
            else { "" };
        report(&s.name.text, &s.reason.text, s.secret.text.chars().count(), active);
        false
    });
    App::new(spec).run();
}
