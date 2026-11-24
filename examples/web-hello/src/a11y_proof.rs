//! Browser accessibility mirror fixture, selected only by `?a11y-proof`: exercises container
//! roles (`toolbar`, `menu`, `menubar`, `tablist`/`tabpanel`, `tree`, `grid`/`gridcell`, `form`,
//! `search`), `aria-disabled` and `aria-busy` states, nested row actions, range controls
//! (`slider`, `spinbutton`), and the canvas `aria-activedescendant` focus proxy.

use rune::tok::{self, type_};
use rune::{AppSpec, Cx, El, col, el, row, text};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = "
export function a11yProofEnabled() {
    return new URLSearchParams(location.search).has('a11y-proof');
}
")]
extern "C" {
    #[wasm_bindgen(js_name = a11yProofEnabled)]
    pub fn enabled() -> bool;
}

struct State {
    busy: bool,
    locked: bool,
    tab: usize,
    volume: f64,
    retries: f64,
    previews: usize,
}

impl Default for State {
    fn default() -> Self {
        Self { busy: true, locked: true, tab: 0, volume: 40.0, retries: 3.0, previews: 0 }
    }
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    cx.web_focus();
    let t = tok::theme(false);

    let toolbar = row()
        .key("toolbar")
        .role("toolbar")
        .label("Editor actions")
        .w_full()
        .gap(8.0)
        .child(
            el()
                .key("toggle-states")
                .role("button")
                .label("Toggle states")
                .focusable(true)
                .h(32.0)
                .px(10.0)
                .center()
                .radius(6.0)
                .fill(t.control_off)
                .child(text("Toggle states").font(type_::LABEL).color(t.text))
                .on_press(|s: &mut State| {
                    s.busy = !s.busy;
                    s.locked = !s.locked;
                    s.volume = if s.volume < 60.0 { 75.0 } else { 40.0 };
                }),
        )
        .child(
            el()
                .key("disabled-btn")
                .role("button")
                .label("Disabled button")
                .disabled(s.locked)
                .h(32.0)
                .px(10.0)
                .center()
                .radius(6.0)
                .fill(t.card)
                .child(text("Disabled button").font(type_::LABEL).color(t.text_secondary)),
        )
        .child(
            el()
                .key("focus-slider")
                .role("button")
                .label("Focus volume")
                .focusable(true)
                .h(32.0)
                .px(10.0)
                .center()
                .radius(6.0)
                .fill(t.control_off)
                .child(text("Focus volume").font(type_::LABEL).color(t.text))
                .on_press_tx(|_: &mut State, tx| tx.focus("volume", true)),
        );

    let menu = col()
        .key("file-menu")
        .role("menu")
        .label("File menu")
        .w_full()
        .gap(4.0)
        .padding(8.0)
        .radius(8.0)
        .fill(t.card)
        .child(
            row()
                .key("mi-open")
                .role("menuitem")
                .label("Open")
                .focusable(true)
                .h(26.0)
                .px(8.0)
                .center_y()
                .child(text("Open").font(type_::BODY).color(t.text))
                .on_press(|s: &mut State| s.previews += 1),
        )
        .child(
            row()
                .key("mi-disabled")
                .role("menuitem")
                .label("Disabled item")
                .disabled(s.locked)
                .h(26.0)
                .px(8.0)
                .center_y()
                .child(text("Disabled item").font(type_::BODY).color(t.text_secondary)),
        );

    let tabs = col()
        .key("tabs-section")
        .w_full()
        .gap(4.0)
        .child(
            row()
                .key("tablist")
                .role("tablist")
                .label("Views")
                .focus_group()
                .gap(6.0)
                .child(
                    el()
                        .key("tab-day")
                        .role("tab")
                        .label("Day")
                        .focusable(true)
                        .focus_selected(s.tab == 0)
                        .h(28.0)
                        .px(10.0)
                        .center()
                        .radius(6.0)
                        .fill(t.card)
                        .child(text("Day").font(type_::LABEL).color(t.text))
                        .on_press(|s: &mut State| s.tab = 0),
                )
                .child(
                    el()
                        .key("tab-week")
                        .role("tab")
                        .label("Week")
                        .focus_selected(s.tab == 1)
                        .disabled(true)
                        .h(28.0)
                        .px(10.0)
                        .center()
                        .radius(6.0)
                        .fill(t.card)
                        .child(text("Week").font(type_::LABEL).color(t.text_secondary)),
                ),
        )
        .child(
            col()
                .key("tabpanel")
                .role("tabpanel")
                .label("Day")
                .w_full()
                .padding(8.0)
                .radius(8.0)
                .fill(t.card)
                .child(text("Monday").key("panel-day").font(type_::BODY).color(t.text)),
        );

    let tree = col()
        .key("files-tree")
        .role("tree")
        .label("Files")
        .busy(s.busy)
        .w_full()
        .gap(4.0)
        .padding(8.0)
        .radius(8.0)
        .fill(t.card)
        .child(
            row()
                .key("tree-src")
                .role("treeitem")
                .label("src")
                .level(1)
                .expanded(true)
                .busy(s.busy)
                .focusable(true)
                .h(26.0)
                .px(8.0)
                .center_y()
                .child(text("src").font(type_::BODY).color(t.text))
                .on_press(|_: &mut State| {}),
        )
        .child(
            row()
                .key("tree-main")
                .role("treeitem")
                .label("main.rs")
                .level(2)
                .focus_selected(true)
                .focusable(true)
                .h(28.0)
                .px(8.0)
                .gap(8.0)
                .center_y()
                .on_press(|_: &mut State| {})
                .child(text("main.rs").font(type_::BODY).color(t.text).grow(1.0))
                .child(
                    el()
                        .key("preview-btn")
                        .role("button")
                        .label("Preview")
                        .focusable(true)
                        .h(24.0)
                        .px(8.0)
                        .center()
                        .radius(4.0)
                        .fill(t.control_off)
                        .child(text("Preview").font(type_::LABEL).color(t.text))
                        .on_press(|s: &mut State| s.previews += 1),
                ),
        );

    let grid = col()
        .key("metrics-grid")
        .role("grid")
        .label("Metrics")
        .w_full()
        .gap(4.0)
        .padding(8.0)
        .radius(8.0)
        .fill(t.card)
        .child(
            row()
                .key("grid-header")
                .role("row")
                .w_full()
                .gap(12.0)
                .child(row().key("col-metric").role("columnheader").label("Metric").h(22.0).child(text("Metric").font(type_::LABEL).color(t.text)))
                .child(row().key("col-val").role("columnheader").label("Value").h(22.0).child(text("Value").font(type_::LABEL).color(t.text))),
        )
        .child(
            row()
                .key("grid-row-0")
                .role("row")
                .w_full()
                .gap(12.0)
                .child(row().key("cell-name").role("gridcell").h(22.0).child(text("Latency").font(type_::BODY).color(t.text)))
                .child(row().key("cell-val").role("gridcell").h(22.0).child(text("12 ms").font(type_::BODY).color(t.text))),
        );

    let form = col()
        .key("settings-form")
        .role("form")
        .label("Audio settings")
        .w_full()
        .gap(8.0)
        .padding(8.0)
        .radius(8.0)
        .fill(t.card)
        .child(
            row()
                .key("search-box")
                .role("search")
                .label("Filter audio")
                .h(24.0)
                .child(text("Filter controls").font(type_::BODY).color(t.text_secondary)),
        )
        .child(
            el()
                .key("volume")
                .role("slider")
                .label("Volume")
                .range_value(s.volume, 0.0, 100.0)
                .a11y_value(format!("{}%", s.volume.round()))
                .busy(s.busy)
                .focusable(true)
                .size(200.0, 24.0)
                .radius(12.0)
                .fill(t.control_off),
        )
        .child(
            el()
                .key("retries")
                .role("spinbutton")
                .label("Retries")
                .range_value(s.retries, 1.0, 10.0)
                .a11y_value(format!("{} retries", s.retries.round()))
                .disabled(s.locked)
                .size(140.0, 24.0)
                .radius(6.0)
                .fill(t.control_off),
        );

    col()
        .w_full()
        .h_full()
        .padding(12.0)
        .gap(8.0)
        .fill(t.bg)
        .child(toolbar)
        .child(menu)
        .child(tabs)
        .child(tree)
        .child(grid)
        .child(form)
}

pub fn run() {
    rune::run(AppSpec::new("Rune Web A11y Proof", (440.0, 720.0), State::default, view));
}
