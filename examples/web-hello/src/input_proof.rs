//! Browser input regression fixture, selected only by ?input-proof (and ?input-proof&audit).
use std::{cell::Cell, rc::Rc};

use rune::{
    App, AppHandle, AppSpec, Cx, Drag, El, Key, Phase, PhysicalInput, ScrollPos, TextEdit, col, el,
    row, text, text_input, tok,
};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = "
export function enabled() { return new URLSearchParams(location.search).has('input-proof'); }
export function auditEnabled() { return new URLSearchParams(location.search).has('audit'); }
export function keyboardResizeEnabled() { return new URLSearchParams(location.search).has('keyboard-resize'); }
export function mediaKeysEnabled() { return new URLSearchParams(location.search).has('media-keys'); }
export function queryZoom() {
    const z = parseFloat(new URLSearchParams(location.search).get('zoom') ?? '1');
    return Number.isFinite(z) && z > 0 ? z : 1;
}
export function report(plain, display, single, secretLength, submits, shortcuts, visible, windowVisible) {
    window.__inputProof = { plain, display, single, secretLength, submits, shortcuts, visible, windowVisible,
        polls: (window.__inputProof?.polls ?? 0) + 1 };
}
export function reportAudit(json) {
    const prev = window.__auditProof?.polls ?? 0;
    const data = JSON.parse(json);
    data.polls = prev + 1;
    window.__auditProof = data;
}
export function reportScroll(x, y) {
    window.__auditScroll = { x, y };
}
export function reportKeyboardResize(secretLength, bottom, dismisses, shiftedClicks, secretFocused, bottomFocused, secretBlurs, bottomBlurs) {
    window.__keyboardResizeProof = { secretLength, bottom, dismisses, shiftedClicks, secretFocused, bottomFocused, secretBlurs, bottomBlurs,
        polls: (window.__keyboardResizeProof?.polls ?? 0) + 1 };
}
")]
extern "C" {
    pub fn enabled() -> bool;
    #[wasm_bindgen(js_name = auditEnabled)]
    fn audit_enabled() -> bool;
    #[wasm_bindgen(js_name = keyboardResizeEnabled)]
    fn keyboard_resize_enabled() -> bool;
    #[wasm_bindgen(js_name = mediaKeysEnabled)]
    fn media_keys_enabled() -> bool;
    #[wasm_bindgen(js_name = queryZoom)]
    fn query_zoom() -> f32;
    fn report(plain: &str, display: &str, single: &str, secret_length: usize, submits: u32, shortcuts: u32, visible: bool, window_visible: bool);
    #[wasm_bindgen(js_name = reportAudit)]
    fn report_audit(json: &str);
    #[wasm_bindgen(js_name = reportScroll)]
    fn report_scroll(x: f32, y: f32);
    #[wasm_bindgen(js_name = reportKeyboardResize)]
    fn report_keyboard_resize(secret_length: usize, bottom: &str, dismisses: u32, shifted_clicks: u32, secret_focused: bool, bottom_focused: bool, secret_blurs: u32, bottom_blurs: u32);
}

struct State {
    plain: TextEdit,
    single: TextEdit,
    secret: TextEdit,
    submits: u32,
    shortcuts: u32,
    handle: Option<AppHandle<State>>,
}

impl Default for State {
    fn default() -> Self {
        Self { plain: TextEdit::default(), single: TextEdit::default(), secret: TextEdit::default().secure(),
            submits: 0, shortcuts: 0, handle: None }
    }
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let field = |edit: &TextEdit| text_input(edit, tok::type_::BODY).w_full().h(48.0);
    col().w_full().h_full().padding(16.0).gap(8.0)
        .child(field(&s.plain).key("plain").label("Plain").on_edit(|s: &mut State| &mut s.plain)
            .on_submit(|s: &mut State, _| s.submits += 1))
        .child(field(&s.single).key("single").label("Single line").single_line().on_edit(|s: &mut State| &mut s.single)
            .on_submit(|s: &mut State, _| s.submits += 1))
        .child(field(&s.secret).key("secret").label("Secret").single_line().on_edit(|s: &mut State| &mut s.secret)
            .on_submit(|s: &mut State, _| s.submits += 1))
        .child(el().key("button").focusable(true).w_full().h(48.0).child(text("Button"))
            .on_press(|s: &mut State| s.submits += 1))
}

struct Audit {
    zoom: f32,
    clicks: Vec<String>,
    secondary: u32,
    middle: u32,
    wheels: Vec<String>,
    pinches: Vec<f32>,
    scroll: Rc<Cell<(f32, f32)>>,
    keys: Vec<String>,
    key_ups: Vec<String>,
    physical_mods: Vec<String>,
    focus_lost: u32,
    drags: Vec<String>,
    hovered: bool,
    edit: TextEdit,
    handle: Option<AppHandle<Audit>>,
}

impl Default for Audit {
    fn default() -> Self {
        Self {
            zoom: query_zoom(),
            clicks: Vec::new(),
            secondary: 0,
            middle: 0,
            wheels: Vec::new(),
            pinches: Vec::new(),
            scroll: Rc::new(Cell::new((0.0, 0.0))),
            keys: Vec::new(),
            key_ups: Vec::new(),
            physical_mods: Vec::new(),
            focus_lost: 0,
            drags: Vec::new(),
            hovered: false,
            edit: TextEdit::default(),
            handle: None,
        }
    }
}

fn mods_json(m: rune::Mods) -> String {
    format!(r#"{{"shift":{},"ctrl":{},"alt":{},"meta":{}}}"#, m.shift, m.ctrl, m.alt, m.logo)
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn publish_audit(s: &Audit) {
    let (sx, sy) = s.scroll.get();
    let pinches: Vec<String> = s.pinches.iter().map(|d| format!("{d:.4}")).collect();
    let key_ups: Vec<String> = s.key_ups.iter().map(|k| json_str(k)).collect();
    let preedit = s.edit.preedit().map(|(p, _)| p).unwrap_or("");
    let json = format!(
        r#"{{"zoom":{:.2},"clicks":[{}],"secondary":{},"middle":{},"wheels":[{}],"pinches":[{}],"scrollX":{:.2},"scrollY":{:.2},"keys":[{}],"keyUps":[{}],"physicalMods":[{}],"focusLost":{},"drags":[{}],"hovered":{},"plain":{},"preedit":{},"visible":{}}}"#,
        s.zoom,
        s.clicks.join(","),
        s.secondary,
        s.middle,
        s.wheels.join(","),
        pinches.join(","),
        sx,
        sy,
        s.keys.join(","),
        key_ups.join(","),
        s.physical_mods.join(","),
        s.focus_lost,
        s.drags.join(","),
        s.hovered,
        json_str(&s.edit.text),
        json_str(preedit),
        s.handle.as_ref().is_some_and(AppHandle::is_visible),
    );
    report_audit(&json);
}

fn audit_view(s: &Audit, cx: &mut Cx<Audit>) -> El {
    cx.zoom(s.zoom);
    publish_audit(s);
    let scroll = s.scroll.clone();
    col().w_full().h_full()
        .on_key_up(|s: &mut Audit, ev, _| { s.key_ups.push(format!("{:?}", ev.key)); true })
        .child(
            row().h(60.0)
                .child(el().key("left").size(120.0, 60.0).on_press_tx(|s: &mut Audit, tx| {
                    s.clicks.push(format!(r#"{{"target":"left","mods":{}}}"#, mods_json(tx.mods())));
                }))
                .child(el().key("right").size(120.0, 60.0).on_press_tx(|s: &mut Audit, tx| {
                    s.clicks.push(format!(r#"{{"target":"right","mods":{}}}"#, mods_json(tx.mods())));
                })),
        )
        .child(
            row().h(60.0)
                .child(
                    el().key("drag").size(120.0, 60.0).focusable(true)
                        .on_hover_change(|s: &mut Audit, h, _| s.hovered = h)
                        .on_drag(|s: &mut Audit, d: &mut Drag, _| {
                            let phase = match d.phase { Phase::Down => "down", Phase::Move => "move", Phase::Up => "up" };
                            s.drags.push(format!(r#"{{"phase":"{phase}","cancelled":{}}}"#, d.cancelled()));
                        })
                        .on_secondary_press(|s: &mut Audit, _, _| s.secondary += 1)
                        .on_middle_press(|s: &mut Audit, _| s.middle += 1)
                        .on_physical_input(|s: &mut Audit, ev, _| match ev {
                            PhysicalInput::Modifiers(m) => { s.physical_mods.push(mods_json(*m)); true }
                            PhysicalInput::FocusLost => { s.focus_lost += 1; true }
                            PhysicalInput::Key { .. } => false,
                        }),
                )
                .child(
                    el().key("wheel").size(120.0, 60.0).hit()
                        .on_wheel(|s: &mut Audit, ev, _| {
                            s.wheels.push(format!(
                                r#"{{"dx":{:.2},"dy":{:.2},"precise":{},"mods":{}}}"#,
                                ev.dx, ev.dy, ev.precise, mods_json(ev.mods)
                            ));
                            true
                        })
                        .on_magnify(|s: &mut Audit, ev, _| { s.pinches.push(ev.delta); true }),
                ),
        )
        .child(
            el().key("scroller").size(240.0, 100.0).scroll_xy()
                .on_scroll_offset(move |pos: ScrollPos| {
                    scroll.set((pos.offset[0], pos.offset[1]));
                    report_scroll(pos.offset[0], pos.offset[1]);
                })
                .child(el().size(1200.0, 1200.0)),
        )
        .child(
            text_input(&s.edit, tok::type_::BODY).key("field").label("Field").size(240.0, 48.0)
                .on_edit(|s: &mut Audit| &mut s.edit),
        )
}

fn run_audit() {
    let mut spec = AppSpec::new("Input audit", (450.0, 600.0), Audit::default, audit_view);
    spec.poll = Some(|s, _, _| {
        publish_audit(s);
        false
    });
    spec.on_key = Some(|s, ev, tx| {
        let phys = tx.physical_key().map(|p| json_str(&format!("{p:?}"))).unwrap_or_else(|| "null".into());
        let base = tx.key_base().map(|c| json_str(&c.to_string())).unwrap_or_else(|| "null".into());
        s.keys.push(format!(
            r#"{{"key":{},"physical":{},"base":{},"repeat":{},"mods":{}}}"#,
            json_str(&format!("{:?}", ev.key)),
            phys,
            base,
            tx.key_repeat(),
            mods_json(ev.mods),
        ));
        true
    });
    let app = if media_keys_enabled() { App::new(spec).media_keys() } else { App::new(spec) };
    let handle = app.handle();
    let state_handle = handle.clone();
    handle.update(move |s, _| s.handle = Some(state_handle));
    app.run();
}

struct KeyboardResizeState {
    secret: TextEdit,
    bottom: TextEdit,
    read_only: TextEdit,
    dismisses: u32,
    shifted_clicks: u32,
    secret_blurs: u32,
    bottom_blurs: u32,
}

impl Default for KeyboardResizeState {
    fn default() -> Self {
        Self {
            secret: TextEdit::default().secure(),
            bottom: TextEdit::default(),
            read_only: TextEdit::new("Read only"),
            dismisses: 0,
            shifted_clicks: 0,
            secret_blurs: 0,
            bottom_blurs: 0,
        }
    }
}

fn keyboard_resize_view(s: &KeyboardResizeState, _: &mut Cx<KeyboardResizeState>) -> El {
    report_keyboard_resize(
        s.secret.text.chars().count(),
        &s.bottom.text,
        s.dismisses,
        s.shifted_clicks,
        s.secret.focused,
        s.bottom.focused,
        s.secret_blurs,
        s.bottom_blurs,
    );
    col().w_full().h_full()
        .child(
            row().w_full().h(56.0).padding(8.0).gap(8.0)
                .child(el().key("dismiss").focusable(true).size(110.0, 40.0).child(text("Dismiss"))
                    .on_press(|s: &mut KeyboardResizeState| s.dismisses += 1))
                .child(text_input(&s.read_only, tok::type_::BODY).key("readonly").label("Read only")
                    .size(220.0, 40.0).read_only(true)),
        )
        .child(
            col().w_full().grow(1.0).min_h(0.0).padding(16.0)
                .child(el().w_full().h(180.0))
                .child(text_input(&s.secret, tok::type_::BODY).key("secret").label("Secret")
                    .single_line().w_full().h(48.0).shrink().min_h(0.0)
                    .on_edit(|s: &mut KeyboardResizeState| &mut s.secret)
                    .on_focus_change(|s: &mut KeyboardResizeState, focused, _| {
                        if !focused { s.secret_blurs += 1; }
                    }))
                .child(el().w_full().grow(1.0).min_h(0.0))
                .child(el().key("shifted-btn").focusable(true).w_full().h(48.0).child(text("Shifted action"))
                    .on_press(|s: &mut KeyboardResizeState| s.shifted_clicks += 1)),
        )
        .child(
            col().w_full().h(200.0).padding(16.0)
                .child(text_input(&s.bottom, tok::type_::BODY).key("bottom").label("Bottom")
                    .single_line().w_full().h(48.0)
                    .on_edit(|s: &mut KeyboardResizeState| &mut s.bottom)
                    .on_focus_change(|s: &mut KeyboardResizeState, focused, _| {
                        if !focused { s.bottom_blurs += 1; }
                    })),
        )
}

fn run_keyboard_resize() {
    let mut spec = AppSpec::new("Keyboard resize proof", (390.0, 699.0), KeyboardResizeState::default, keyboard_resize_view);
    spec.poll = Some(|s, _, _| {
        report_keyboard_resize(
            s.secret.text.chars().count(),
            &s.bottom.text,
            s.dismisses,
            s.shifted_clicks,
            s.secret.focused,
            s.bottom.focused,
            s.secret_blurs,
            s.bottom_blurs,
        );
        false
    });
    App::new(spec).run();
}

pub fn run() {
    if audit_enabled() {
        run_audit();
        return;
    }
    if keyboard_resize_enabled() {
        run_keyboard_resize();
        return;
    }
    let mut spec = AppSpec::new("Input proof", (390.0, 760.0), State::default, view);
    spec.poll = Some(|s, _, tx| {
        report(&s.plain.text, &s.plain.display().0, &s.single.text, s.secret.text.chars().count(),
            s.submits, s.shortcuts, s.handle.as_ref().is_some_and(AppHandle::is_visible), tx.window_visible());
        false
    });
    spec.on_key = Some(|s, event, _| {
        if event.key == Key::Command('k') { s.shortcuts += 1; true } else { false }
    });
    let app = App::new(spec);
    let handle = app.handle();
    let state_handle = handle.clone();
    handle.update(move |s, _| s.handle = Some(state_handle));
    handle.show();
    handle.set_position((0, 0)); // Must warn, never cover the canvas with an error page.
    app.run();
}
