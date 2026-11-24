#![forbid(unsafe_code)]

//! Cross-target JSON fetch example: `spawn_local` + `web_sys` `fetch` + `AppHandle::update_if` on
//! web, and `rune::Latest` on native. Draws nothing when idle.

use rune::tok::{self, type_};
use rune::{App, AppSpec, Cx, El, ScrollSpec, TextEdit, col, el, row, text, text_input};
#[cfg(target_arch = "wasm32")]
use rune::AppHandle;
#[cfg(not(target_arch = "wasm32"))]
use rune::{Latest, Tx};

enum Status { Idle, Loading, Ready(String), Error(String) }

impl Status {
    fn from_result(res: Result<String, String>) -> Self {
        match res { Ok(body) => Self::Ready(body), Err(err) => Self::Error(err) }
    }
}

struct State {
    target: TextEdit,
    status: Status,
    #[cfg(target_arch = "wasm32")]
    handle: AppHandle<State>,
    #[cfg(target_arch = "wasm32")]
    seq: u64,
    #[cfg(not(target_arch = "wasm32"))]
    latest: Latest<(bool, Result<String, String>)>,
}

fn format_json(raw: &str) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_str(raw).map_err(|e| format!("Invalid JSON: {e}"))?;
    serde_json::to_string_pretty(&value).map_err(|e| format!("Format error: {e}"))
}

#[cfg(target_arch = "wasm32")]
async fn fetch_json(url: &str) -> Result<String, String> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    let window = web_sys::window().ok_or_else(|| "No browser window".to_string())?;
    let headers = web_sys::Headers::new().map_err(|e| format!("{e:?}"))?;
    headers.set("Accept", "application/json").map_err(|e| format!("{e:?}"))?;
    let init = web_sys::RequestInit::new();
    init.set_method("GET");
    init.set_headers_headers(&headers);
    let promise: js_sys::Promise = window.fetch_with_str_and_init(url, &init);
    let resp: web_sys::Response = JsFuture::from(promise).await
        .map_err(|e| format!("Request failed: {e:?}"))?
        .dyn_into().map_err(|_| "Unexpected fetch response".to_string())?;
    if !resp.ok() { return Err(format!("HTTP {}", resp.status())); }
    let text_promise = resp.text().map_err(|e| format!("Read failed: {e:?}"))?;
    let body = JsFuture::from(text_promise).await
        .map_err(|e| format!("Body failed: {e:?}"))?
        .as_string().ok_or_else(|| "Non-string response body".to_string())?;
    format_json(&body)
}

#[cfg(target_arch = "wasm32")]
fn fetch_target(state: &mut State, initial: bool) {
    let url = state.target.text.trim().to_owned();
    if url.is_empty() { return; }
    state.seq = state.seq.wrapping_add(1);
    let (id, handle) = (state.seq, state.handle.clone());
    state.status = Status::Loading;
    wasm_bindgen_futures::spawn_local(async move {
        let res = fetch_json(&url).await;
        handle.update_if(move |s, tx| {
            if s.seq != id { return false; }
            s.status = Status::from_result(res);
            if initial { tx.instant(); }
            true
        });
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn fetch_target(state: &mut State, initial: bool) {
    let path = state.target.text.trim().to_owned();
    if path.is_empty() { return; }
    state.status = Status::Loading;
    state.latest.request(path, move |target| {
        // Drop in any blocking HTTP client here (for example `ureq::get(&target)` or
        // `reqwest::blocking::get(&target)`): `Latest` runs this closure on its worker pool.
        let res = std::fs::read_to_string(&target)
            .map_err(|e| format!("Read {target}: {e}"))
            .and_then(|raw| format_json(&raw));
        (initial, res)
    });
}

fn view(state: &State, cx: &mut Cx<State>) -> El {
    let t = tok::theme(cx.system_dark());
    let [top, right, bottom, left] = cx.safe_area();
    let field = text_input(&state.target, type_::BODY).key("target").label("JSON URL or path")
        .placeholder("JSON URL or path", t.text_secondary).color(t.text)
        .grow(1.0).h(44.0).padding(12.0).radius(10.0).fill(t.card)
        .on_edit(|s: &mut State| &mut s.target)
        .on_submit(|s: &mut State, _| fetch_target(s, false));
    let button = el().key("fetch").h(44.0).px(16.0).center().radius(10.0).fill(t.control_off)
        .child(text("Fetch").font(type_::LABEL).color(t.text))
        .on_press(|s: &mut State| fetch_target(s, false));
    let (label, body) = match &state.status {
        Status::Idle => ("Idle", String::new()),
        Status::Loading => ("Loading…", String::new()),
        Status::Ready(json) => ("Response", json.clone()),
        Status::Error(err) => ("Error", err.clone()),
    };
    col().w_full().h_full().pt(16.0 + top).pr(16.0 + right).pb(16.0 + bottom).pl(16.0 + left).gap(12.0).fill(t.bg)
        .child(text("Fetch JSON").font(type_::TITLE).color(t.text))
        .child(row().w_full().gap(8.0).child(field).child(button))
        .child(text(label).font(type_::LABEL).color(t.text_secondary))
        .child(col().key("output").w_full().grow(1.0).shrink().padding(12.0).radius(10.0).fill(t.card)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(text(body).font(type_::BODY.mono()).color(t.text).w_full()))
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen::prelude::wasm_bindgen(start))]
pub fn start() {
    #[allow(unused_mut)]
    let mut spec = AppSpec::new("Fetch", (480.0, 560.0), || unreachable!(), view);
    #[cfg(not(target_arch = "wasm32"))]
    {
        spec.setup = Some(|s: &mut State, waker| s.latest.set_waker(waker));
        spec.poll = Some(|s: &mut State, _, tx: &mut Tx| {
            let Some((initial, res)) = s.latest.take() else { return false };
            s.status = Status::from_result(res);
            if initial { tx.instant(); }
            true
        });
    }
    let app = App::new(spec);
    let handle = app.handle();
    #[cfg(target_arch = "wasm32")]
    let init_handle = handle.clone();
    app.init(move || State {
        target: TextEdit::new("data.json"),
        status: Status::Idle,
        #[cfg(target_arch = "wasm32")]
        handle: init_handle,
        #[cfg(target_arch = "wasm32")]
        seq: 0,
        #[cfg(not(target_arch = "wasm32"))]
        latest: Latest::default(),
    })
    .on_ready(move |_| handle.update(|s, tx| { tx.instant(); fetch_target(s, true); }))
    .run();
}
