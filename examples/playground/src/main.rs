//! A small preferences window with a playground (⌘⇧D): its scenes, a live page, a story with its
//! own state and timer, and an overlay, grouped by type, with triggers, variants (1–9) and a
//! side-column control. "Controls" nests groups inside it by path ("Controls/Switches/Toggle")
//! and comes first in the list. Every group starts folded but the open story's, and a click on
//! a group's header folds or unfolds it; / filters the list. "Every device" scrolls its stage,
//! and "Windows/Preferences" fills it edge to edge.
//!
//! `cargo run -p playground` shows the bar along the bottom; `-- --playground "Upload"` opens
//! the playground at that story, and `-- --snap out.png --playground` renders it headless;
//! `--variant "Long names"` beside `--playground "Many devices"` picks a variant.

#![forbid(unsafe_code)]

mod any_handlers;
mod any_press;
mod emoji_family_story;
mod mount;
mod pick;
mod scroll_anchor_story;
mod text_indent_story;

use std::time::Duration;

use rune::tok::{self, Theme, motion, radius, type_};
mod hover_press;
mod loops_press;
mod press_pointer;

use rune::{Align, App, AppSpec, Color, Cx, El, ElementInfo, Family, OverlaySpec, Play, Playground, ScreenEdge, ScrollSpec, Stage, Story, TextStyle, Tx, col, el, row, text};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    General,
    Sync,
    Devices,
}

const SECTIONS: [(Section, &str); 3] = [(Section::General, "General"), (Section::Sync, "Sync"), (Section::Devices, "Devices")];

struct Prefs {
    section: Section,
    launch_at_login: bool,
    sounds: bool,
    sync: bool,
    wifi_only: bool,
    devices: Vec<&'static str>,
}

fn init() -> Prefs {
    Prefs { section: Section::General, launch_at_login: true, sounds: false, sync: true, wifi_only: true, devices: vec!["Studio desk", "Travel laptop"] }
}

static SCENES: &[(&str, fn(&mut Prefs))] = &[
    ("Sections/General", |s| s.section = Section::General),
    ("Sections/Sync paused", |s| { s.section = Section::Sync; s.sync = false; }),
    ("Lists/Many devices", |s| {
        s.section = Section::Devices;
        s.devices.extend(["Kitchen tablet", "Spare phone", "Lab machine", "Old desktop"]);
    }),
];

fn view(s: &Prefs, cx: &mut Cx<Prefs>) -> El {
    let t = tok::theme(cx.system_dark());
    let accent = tok::accent(0, cx.system_dark());
    let nav = col().key("nav").w(176.0).h_full().pt(if cx.titlebar_inset() > 0.0 { 44.0 } else { 12.0 }).px(8.0).gap(2.0)
        .children(SECTIONS.iter().map(|&(section, name)| {
            let on = s.section == section;
            row().key(name).w_full().h(28.0).px(10.0).center_y().radius(radius::SM)
                .fill(if on { t.control_track } else { Color::TRANSPARENT })
                .hover(if on { t.control_track } else { t.hover })
                .child(text(name).font(type_::BODY).color(t.text))
                .role("tab").label(name)
                .on_press(move |s: &mut Prefs| s.section = section)
        }));
    let body = match s.section {
        Section::General => col().gap(8.0)
            .child(switch_row(t, accent, "launch", "Open at login", s.launch_at_login, |s| s.launch_at_login = !s.launch_at_login))
            .child(switch_row(t, accent, "sounds", "Play sounds", s.sounds, |s| s.sounds = !s.sounds)),
        Section::Sync => col().gap(8.0)
            .child(switch_row(t, accent, "sync", "Sync across devices", s.sync, |s| s.sync = !s.sync))
            .child(switch_row(t, accent, "wifi", "Only on Wi-Fi", s.wifi_only && s.sync, |s| s.wifi_only = !s.wifi_only).opacity(if s.sync { 1.0 } else { 0.5 }))
            .children((!s.sync).then(|| text("Changes stay on this device until sync is back on.").font(type_::BODY).color(t.text_secondary).px(4.0))),
        Section::Devices => devices(s, cx),
    };
    let title = SECTIONS.iter().find(|(section, _)| *section == s.section).map_or("", |(_, name)| name);
    row().w_full().h_full().fill(t.bg)
        .child(nav)
        .child(el().w(1.0).h_full().fill(t.separator))
        .child(col().key("content").grow(1.0).h_full().padding(24.0).gap(16.0)
            .child(text(title).font(type_::TITLE).color(t.text))
            .child(body))
}

/// The playground's live devices page: the list, drawn from the real state.
fn devices_page(s: &Prefs, cx: &mut Cx<Prefs>) -> El {
    col().w_full().h_full().padding(24.0).child(devices(s, cx))
}

/// The "Every device" story: more devices than the stage is tall, on a scrolling stage.
fn every_device() -> Prefs {
    Prefs { devices: EVERY_DEVICE.to_vec(), ..init() }
}

const EVERY_DEVICE: [&str; 24] = [
    "Studio desk", "Travel laptop", "Kitchen tablet", "Spare phone", "Lab machine", "Old desktop",
    "Living room TV", "Garage Pi", "Work phone", "Reading tablet", "Build server", "Test phone",
    "Backup drive", "Guest laptop", "Car display", "Watch", "Office printer", "Media box",
    "Shared iPad", "Loaner laptop", "Render node", "Home server", "Second monitor hub", "Field kit",
];

fn devices(s: &Prefs, cx: &mut Cx<Prefs>) -> El {
    let t = tok::theme(cx.system_dark());
    col().key("devices").w_full().radius(radius::MD).fill(t.card).outline(1.0, 0.0, t.card_border)
        .children(s.devices.iter().enumerate().map(|(i, name)| {
            let line = (i > 0).then(|| el().w_full().h(1.0).fill(t.separator));
            col().key(format!("device-{i}")).w_full()
                .children(line)
                .child(row().w_full().h(36.0).px(12.0).center_y().justify_between()
                    .child(text(*name).font(type_::BODY).color(t.text))
                    .child(text(if i == 0 { "This device" } else { "Synced" }).font(type_::LABEL).color(t.text_secondary)))
        }))
}

fn switch_row(t: Theme, accent: Color, key: &str, label: &'static str, on: bool, flip: fn(&mut Prefs)) -> El {
    switch(t, accent, key, label, on, flip)
}

/// A labelled switch on any state, `flip` turning it.
fn switch<S: 'static>(t: Theme, accent: Color, key: &str, label: &'static str, on: bool, flip: fn(&mut S)) -> El {
    let (w, h, k) = (34.0, 20.0, 16.0);
    let knob = el().size(k, k).absolute(if on { w - k - 2.0 } else { 2.0 }, 2.0).radius(k / 2.0).fill(t.knob).shadow(1.0, 3.0, t.knob_shadow);
    row().key(key.to_owned()).w_full().h(44.0).px(12.0).center_y().justify_between()
        .radius(radius::MD).fill(t.card).outline(1.0, 0.0, t.card_border)
        .child(text(label).font(type_::BODY).color(t.text))
        .child(el().size(w, h).radius(h / 2.0).fill(if on { accent } else { t.control_off }).motion(motion::SNAPPY).child(knob))
        .role("switch").label(label)
        .on_press(move |s: &mut S| flip(s))
}

/// The "Controls/Switches" stories' own state: one switch.
struct Switch {
    on: bool,
    dimmed: bool,
    label: &'static str,
}

fn switch_story(s: &Switch, cx: &mut Cx<Switch>) -> El {
    let t = tok::theme(cx.system_dark());
    let accent = tok::accent(0, cx.system_dark());
    col().w_full().h_full().center().child(
        col().w(300.0).child(switch(t, accent, "switch", s.label, s.on, |s: &mut Switch| s.on = !s.on).opacity(if s.dimmed { 0.5 } else { 1.0 })),
    )
}

/// The "Controls/Rows" stories: the devices list on its own.
fn device_rows(devices: &'static [&'static str]) -> impl Fn() -> Prefs {
    move || Prefs { devices: devices.to_vec(), ..init() }
}

/// The "Upload" story's own state: a pretend upload ticking along on a timer.
struct Upload {
    progress: u32,
    running: bool,
    last: f64,
}

fn upload_init() -> Upload {
    Upload { progress: 0, running: false, last: 0.0 }
}

/// One step every 0.1 s while running: `wake_after` asks for the next frame, whose poll steps.
fn upload_poll(u: &mut Upload, t: f64, tx: &mut Tx) -> bool {
    if !u.running { return false; }
    tx.wake_after(Duration::from_millis(100));
    if t - u.last < 0.1 { return false; }
    u.last = t;
    u.progress = (u.progress + 4).min(100);
    u.running = u.progress < 100;
    true
}

fn upload_view(u: &Upload, cx: &mut Cx<Upload>) -> El {
    let t = tok::theme(cx.system_dark());
    let accent = tok::accent(0, cx.system_dark());
    let label = match (u.running, u.progress) {
        (true, _) => "Pause",
        (false, 100) => "Again",
        (false, 0) => "Start",
        (false, _) => "Resume",
    };
    let track = 240.0;
    col().w_full().h_full().center().gap(12.0)
        .child(text(format!("Uploading backup · {}%", u.progress)).font(type_::BODY).color(t.text))
        .child(el().w(track).h(6.0).radius(3.0).fill(t.control_track)
            .child(el().w(track * u.progress as f32 / 100.0).h(6.0).radius(3.0).fill(accent).motion(motion::SNAPPY)))
        .child(row().key("upload-toggle").h(28.0).px(14.0).center().radius(radius::SM).fill(t.control_track).hover(t.hover)
            .child(text(label).font(type_::LABEL).color(t.text))
            .role("button").label(label)
            .on_press_tx(|u: &mut Upload, tx: &mut Tx| {
                if u.progress == 100 { u.progress = 0; }
                u.running = !u.running;
                tx.wake_after(Duration::ZERO);
            }))
}

/// The toast an app shows over the desktop when a sync finishes, previewed as a story.
struct Toast {
    message: &'static str,
    failed: bool,
}

fn toast_view(s: &Toast, cx: &mut Cx<Toast>) -> El {
    let t = tok::theme(cx.system_dark());
    el().w_full().h_full().center()
        .child(row().h(36.0).px(16.0).gap(8.0).center_y().radius(18.0).fill(t.card).outline(1.0, 0.0, t.card_border).shadow(4.0, 16.0, t.shadow_soft)
            .child(el().size(8.0, 8.0).radius(4.0).fill(tok::accent(if s.failed { 3 } else { 4 }, cx.system_dark())))
            .child(text(s.message).font(type_::BODY).color(t.text)))
}

static TOAST: OverlaySpec<Toast> = OverlaySpec {
    name: "Sync toast",
    size: (320.0, 64.0),
    edge: ScreenEdge::Top,
    margin: 12.0,
    align: Align::Center,
    inset: 0.0,
    interactive: false,
    view: toast_view,
};

/// The "Many devices" scene's own control in the side column: how many devices it lists.
fn device_count(s: &Prefs, cx: &mut Cx<Prefs>) -> El {
    let t = tok::theme(cx.system_dark());
    let step = |key: &str, label: &'static str, f: fn(&mut Prefs)| {
        row().key(key.to_owned()).size(24.0, 24.0).center().radius(radius::SM).hover(t.hover).press(t.press)
            .child(text(label).font(type_::BODY).color(t.text))
            .role("button").label(label)
            .on_press(move |s: &mut Prefs| f(s))
    };
    row().key("device-count").w_full().h(36.0).pl(12.0).pr(6.0).gap(2.0).center_y().radius(radius::LG).fill(t.card).outline(1.0, 0.0, t.card_border)
        .child(text("Devices").font(type_::BODY).color(t.text))
        .child(el().grow(1.0))
        .child(text(s.devices.len().to_string()).font(type_::BODY).color(t.text_secondary).pr(6.0))
        .child(step("fewer", "−", |s| { if s.devices.len() > 1 { s.devices.pop(); } }))
        .child(step("more", "+", |s| s.devices.push("New device")))
}

/// The "Layout/Display scale" story: a fixed-point surface whose `on_layout_tx` computes its
/// backing pixel size from `tx.scale()` and re-fires whenever display scale or zoom changes.
struct ScaleReadout {
    size_pt: [f32; 2],
    scale: f32,
    zoom: f32,
    reports: usize,
}

fn scale_readout_init() -> ScaleReadout {
    ScaleReadout { size_pt: [0.0, 0.0], scale: 1.0, zoom: 1.0, reports: 0 }
}

const ZOOM_STEPS: [f32; 4] = [1.0, 1.25, 1.5, 2.0];

fn scale_readout_view(s: &ScaleReadout, cx: &mut Cx<ScaleReadout>) -> El {
    cx.zoom(s.zoom);
    let t = tok::theme(cx.system_dark());
    let accent = tok::accent(0, cx.system_dark());
    let px_w = (s.size_pt[0] * s.scale).round() as u32;
    let px_h = (s.size_pt[1] * s.scale).round() as u32;
    let zoom_btn = |key: &'static str, label: &'static str, z: f32| {
        let on = (s.zoom - z).abs() < 1e-3;
        row().key(key).h(26.0).px(10.0).center().radius(radius::SM)
            .fill(if on { accent } else { t.control_track })
            .hover(if on { accent } else { t.hover })
            .child(text(label).font(type_::LABEL).color(if on { Color::WHITE } else { t.text }))
            .focusable(true).role("button").label(format!("Zoom {label}"))
            .on_press(move |s: &mut ScaleReadout| s.zoom = z)
    };
    col().w_full().h_full().center().gap(12.0)
        .child(
            col().key("pixel-surface").size(220.0, 120.0).center().gap(4.0)
                .radius(radius::MD).fill(t.card).outline(1.0, 0.0, t.card_border)
                .on_layout_tx(|s: &mut ScaleReadout, [_, _, w, h], tx| {
                    s.size_pt = [w, h];
                    s.scale = tx.scale();
                    s.reports += 1;
                })
                .child(text(format!("{px_w} × {px_h} px")).font(type_::TITLE).color(t.text))
                .child(text(format!(
                    "{:.0} × {:.0} pt @ {:.2}× · {} report{}",
                    s.size_pt[0], s.size_pt[1], s.scale, s.reports, if s.reports == 1 { "" } else { "s" }
                )).font(type_::LABEL).color(t.text_secondary)),
        )
        .child(
            row().gap(6.0).center_y()
                .child(zoom_btn("zoom-100", "100%", 1.0))
                .child(zoom_btn("zoom-125", "125%", 1.25))
                .child(zoom_btn("zoom-150", "150%", 1.5))
                .child(zoom_btn("zoom-200", "200%", 2.0)),
        )
}

const RUNTIME_FONT: &[u8] = include_bytes!("../../../crates/rune/assets/IBMPlexMono-Regular.ttf");
const RUNTIME_SAMPLE: &str = "Hamburgefonstiv 0123456789";

/// The "Text/Runtime font" story: registers bundled font bytes at runtime via `Tx::add_font`.
#[derive(Default)]
struct RuntimeFont {
    families: Vec<String>,
    active: bool,
}

fn runtime_font_view(s: &RuntimeFont, cx: &mut Cx<RuntimeFont>) -> El {
    let t = tok::theme(cx.system_dark());
    let sample_style = TextStyle::new(18.0, 400.0).family(if s.active { Family::Named("IBM Plex Mono") } else { Family::Sans });
    let (sample_w, _) = rune::text::measure(RUNTIME_SAMPLE, &[], sample_style, None);
    let status = if s.families.is_empty() {
        "Not registered yet — sample uses Inter".to_owned()
    } else {
        format!("Registered: {} · width {:.1} pt", s.families.join(", "), sample_w)
    };
    col().w_full().h_full().center().gap(14.0).padding(24.0)
        .child(
            col().key("sample-card").w(360.0).padding(16.0).gap(8.0).radius(radius::MD).fill(t.card).outline(1.0, 0.0, t.card_border)
                .child(row().child(text(RUNTIME_SAMPLE).key("sample-text").font(sample_style).color(t.text)))
                .child(text(status).key("sample-status").font(type_::LABEL).color(t.text_secondary)),
        )
        .child(
            row().gap(8.0)
                .child(
                    row().key("register-font").h(28.0).px(12.0).center().radius(radius::SM).fill(t.control_track).hover(t.hover)
                        .child(text("Register font bytes").font(type_::LABEL).color(t.text))
                        .role("button").label("Register font bytes").focusable(true)
                        .on_press_tx(|s: &mut RuntimeFont, tx: &mut Tx| {
                            s.families = tx.add_font(RUNTIME_FONT);
                            s.active = !s.families.is_empty();
                        }),
                )
                .child(
                    row().key("use-inter").h(28.0).px(12.0).center().radius(radius::SM).fill(t.control_track).hover(t.hover)
                        .child(text("Use Inter").font(type_::LABEL).color(t.text))
                        .role("button").label("Use Inter").focusable(true)
                        .on_press(|s: &mut RuntimeFont| s.active = false),
                ),
        )
}

/// R on the upload: start over, then pause part way and pick up again, on the logical clock.
fn upload_replay(u: &mut Upload, play: &mut Play<'_, Upload>) {
    u.running = true;
    play.wake_after(Duration::ZERO);
    play.after(0.6, |u, _| u.running = false);
    play.after(1.2, |u, play| {
        u.running = true;
        play.wake_after(Duration::ZERO);
    });
}

/// The "Layout/Fit & fractions" story: `w_fit`, `h_fit`, `self_auto`, and `h_frac`.
struct LayoutDemo {
    align: usize,
    split: f32,
}

fn layout_demo_init() -> LayoutDemo {
    LayoutDemo { align: 0, split: 0.4 }
}

fn layout_demo_view(s: &LayoutDemo, cx: &mut Cx<LayoutDemo>) -> El {
    let t = tok::theme(cx.system_dark());
    let accent = tok::accent(0, cx.system_dark());
    let accent2 = tok::accent(4, cx.system_dark());
    let pill = |key: &str, label: &str, bg: Color| {
        row().key(key.to_owned()).w_fit().h(28.0).px(12.0).center_y().radius(14.0).fill(bg)
            .child(text(label).font(type_::LABEL).color(Color::WHITE))
    };
    let align_label = match s.align % 4 {
        0 => "col() · default stretch",
        1 => "col().items_center()",
        2 => "col().items_end()",
        _ => "row().center_y()",
    };
    let fit_box = match s.align % 4 {
        0 => col().w_full().gap(8.0)
            .child(row().h(28.0).px(12.0).center_y().radius(radius::SM).fill(t.control_track)
                .child(text("Default child (stretches)").font(type_::LABEL).color(t.text_secondary)))
            .child(pill("fit-pill", "w_fit() pill hugs content", accent))
            .child(row().h(28.0).px(12.0).center_y().radius(radius::SM).fill(t.control_track).self_start().self_auto()
                .child(text("self_start().self_auto() restores stretch").font(type_::LABEL).color(t.text))),
        1 => col().w_full().items_center().gap(8.0)
            .child(pill("fit-pill", "w_fit() centred by col", accent))
            .child(pill("auto-pill", "self_start().self_auto() centred", accent2).self_start().self_auto()),
        2 => col().w_full().items_end().gap(8.0)
            .child(pill("fit-pill", "w_fit() aligned end", accent))
            .child(pill("auto-pill", "self_start().self_auto() aligned end", accent2).self_start().self_auto()),
        _ => row().w_full().h(72.0).center_y().gap(8.0)
            .child(pill("fit-pill", "w_fit() centred in row", accent))
            .child(col().h_fit().px(10.0).py(6.0).radius(radius::SM).fill(t.control_track)
                .child(text("h_fit() chip").font(type_::LABEL).color(t.text))),
    };
    let top_pct = (s.split * 100.0).round() as u32;
    let bot_pct = 100 - top_pct;
    let frac_box = row().w_full().h(140.0).gap(12.0)
        .child(col().grow(1.0).h_full().radius(radius::MD).fill(t.card).outline(1.0, 0.0, t.card_border).padding(6.0)
            .child(row().w_full().h_frac(s.split).px(10.0).center_y().radius(radius::SM).fill(accent)
                .child(text(format!("h_frac({:.2}) · {top_pct}%", s.split)).font(type_::LABEL).color(Color::WHITE)))
            .child(row().w_full().h_frac(1.0 - s.split).px(10.0).center_y().radius(radius::SM).fill(t.control_track)
                .child(text(format!("h_frac({:.2}) · {bot_pct}%", 1.0 - s.split)).font(type_::LABEL).color(t.text))))
        .child(row().w(140.0).h_full().padding(8.0).gap(6.0).radius(radius::MD).fill(t.card).outline(1.0, 0.0, t.card_border)
            .child(el().w(8.0).radius(4.0).fill(t.control_track))
            .child(col().h_fit().px(8.0).py(6.0).radius(radius::SM).fill(accent2)
                .child(text("h_fit()").font(type_::LABEL).color(Color::WHITE))));

    col().w_full().h_full().center().padding(20.0).child(
        col().w(420.0).gap(16.0)
            .child(col().w_full().gap(6.0)
                .child(text(format!("w_fit / self_auto — {align_label}")).font(type_::LABEL).color(t.text_secondary))
                .child(col().w_full().padding(12.0).radius(radius::MD).fill(t.card).outline(1.0, 0.0, t.card_border).child(fit_box)))
            .child(col().w_full().gap(6.0)
                .child(text("h_frac split & h_fit in stretching row").font(type_::LABEL).color(t.text_secondary))
                .child(frac_box)),
    )
}

/// One sampled element captured by `Tx::read_elements` in the "Layout/Element inspector" story.
#[derive(Clone)]
struct InspectedRow {
    key: String,
    fill: Color,
    border: Option<Color>,
    detail: String,
}

/// The "Layout/Element inspector" story: inspects the presented frame via `Tx::read_elements`
/// and highlights sample elements' fills, borders, descriptions, text metrics, and clips.
#[derive(Default)]
struct ElementInspector {
    total: usize,
    bordered: usize,
    clipped: usize,
    highlight: bool,
    pending_inspect: bool,
    rows: Vec<InspectedRow>,
}

fn inspector_poll(s: &mut ElementInspector, _t: f64, tx: &mut Tx) -> bool {
    if std::mem::take(&mut s.pending_inspect) {
        tx.read_elements::<ElementInspector>(|s, elements, _| s.capture(elements));
    }
    false
}

const INSPECT_KEYS: [&str; 4] = ["sample-pill", "sample-card", "sample-trunc", "sample-scroll"];

impl ElementInspector {
    fn capture(&mut self, elements: &[ElementInfo]) {
        self.total = elements.len();
        self.bordered = elements.iter().filter(|e| e.border_color.is_some()).count();
        self.clipped = elements.iter().filter(|e| e.clips).count();
        self.highlight = true;
        self.rows = INSPECT_KEYS.iter().filter_map(|&key| {
            let e = elements.iter().find(|e| e.key.as_deref() == Some(key))?;
            let mut parts = Vec::new();
            if let Some(b) = e.border_color { parts.push(format!("border {}", b.hex())); }
            if let Some(d) = &e.description { parts.push(format!("desc '{d}'")); }
            if let Some([w, h]) = e.text_size {
                parts.push(format!("text {w:.0}×{h:.0}{}", if e.text_cut { " (cut)" } else { "" }));
            }
            if e.inset != [0.0; 4] { parts.push(format!("inset {:?}", e.inset.map(|v| v.round() as i32))); }
            if e.clips { parts.push(format!("clips axes={:?}", e.scroll_axes)); }
            Some(InspectedRow {
                key: key.to_owned(),
                fill: e.fill,
                border: e.border_color,
                detail: if parts.is_empty() { format!("fill {}", e.fill.hex()) } else { parts.join(" · ") },
            })
        }).collect();
    }
}

fn inspector_view(s: &ElementInspector, cx: &mut Cx<ElementInspector>) -> El {
    let t = tok::theme(cx.system_dark());
    let accent = tok::accent(0, cx.system_dark());
    let green = tok::accent(4, cx.system_dark());
    let hi = |el: El| if s.highlight { el.outline(1.0, 2.0, accent) } else { el };

    let samples = row().w_full().gap(10.0).center_y()
        .child(hi(
            row().key("sample-pill").h(32.0).px(12.0).center().radius(16.0).fill(accent)
                .role("button").label("Deploy").description("Deploys the active build").focusable(true)
                .child(text("Deploy").font(type_::LABEL).color(Color::WHITE)),
        ))
        .child(hi(
            col().key("sample-card").size(104.0, 44.0).padding(8.0).center().radius(radius::SM)
                .fill(t.card).border(1.5, green).border_layout()
                .child(text("Bordered").font(type_::LABEL).color(t.text)),
        ))
        .child(hi(
            row().key("sample-trunc-wrap").w(96.0).h(32.0).px(8.0).center_y().radius(radius::SM).fill(t.card)
                .child(text("Long label that truncates cleanly").key("sample-trunc").font(type_::LABEL).color(t.text).truncate()),
        ))
        .child(hi(
            col().key("sample-scroll").size(96.0, 44.0).padding(6.0).gap(4.0).radius(radius::SM)
                .fill(t.card).scroll(ScrollSpec::default())
                .child(text("Row 1").font(type_::LABEL).color(t.text))
                .child(text("Row 2").font(type_::LABEL).color(t.text_secondary))
                .child(text("Row 3").font(type_::LABEL).color(t.text_secondary)),
        ));

    let summary = if s.rows.is_empty() {
        "Press Inspect frame to snapshot ElementInfo via Tx::read_elements".to_owned()
    } else {
        format!("{} elements · {} bordered · {} clipping", s.total, s.bordered, s.clipped)
    };

    let swatch = |c: Color| el().size(12.0, 12.0).radius(3.0).fill(c).border(1.0, t.card_border);
    let report = col().w_full().padding(12.0).gap(6.0).radius(radius::MD).fill(t.card).outline(1.0, 0.0, t.card_border)
        .child(text(summary).key("inspect-summary").font(type_::LABEL).color(t.text_secondary))
        .children(s.rows.iter().map(|r| {
            row().w_full().gap(8.0).center_y()
                .child(swatch(r.fill))
                .children(r.border.map(swatch))
                .child(text(r.key.clone()).font(type_::LABEL).color(t.text))
                .child(text(r.detail.clone()).font(type_::LABEL).color(t.text_secondary).truncate())
        }));

    col().w_full().h_full().center().padding(20.0).child(
        col().w(480.0).gap(14.0)
            .child(samples)
            .child(
                row().gap(8.0)
                    .child(
                        row().key("inspect-btn").h(28.0).px(12.0).center().radius(radius::SM).fill(t.control_track).hover(t.hover)
                            .role("button").label("Inspect frame").description("Reads presented ElementInfo list").focusable(true)
                            .child(text("Inspect frame").font(type_::LABEL).color(t.text))
                            .on_press_tx(|_: &mut ElementInspector, tx: &mut Tx| {
                                tx.read_elements::<ElementInspector>(|s, elements, _| s.capture(elements));
                            }),
                    )
                    .child(
                        row().key("clear-inspect").h(28.0).px(12.0).center().radius(radius::SM).fill(t.control_track).hover(t.hover)
                            .role("button").label("Clear").focusable(true)
                            .child(text("Clear").font(type_::LABEL).color(t.text))
                            .on_press(|s: &mut ElementInspector| *s = ElementInspector::default()),
                    ),
            )
            .child(report),
    )
}

/// The "Controls/Focus card" story: focusable cards styled via `El::on_focus_change`,
/// `El::on_focus_within_change`, `El::focus_border`, `Tx::focus_within`, and `Tx::focus_visible`.
#[derive(Default)]
struct FocusCard {
    card_a_focused: bool,
    card_b_focused: bool,
    group_within: bool,
    focus_visible: bool,
    last_within_group: bool,
    transitions: usize,
}

fn focus_card_view(s: &FocusCard, cx: &mut Cx<FocusCard>) -> El {
    let t = tok::theme(cx.system_dark());
    let accent = tok::accent(0, cx.system_dark());
    let card = |key: &'static str, title: &'static str, subtitle: &'static str, focused: bool, is_a: bool| {
        col().key(key).w(170.0).padding(12.0).gap(4.0).radius(radius::MD)
            .fill(if focused { t.control_track } else { t.card })
            .border(1.5, t.card_border)
            .focus_border(accent)
            .no_focus_ring()
            .focusable(true)
            .role("button")
            .label(title)
            .on_focus_change(move |s: &mut FocusCard, on, tx: &mut Tx| {
                if is_a { s.card_a_focused = on; } else { s.card_b_focused = on; }
                s.focus_visible = on && tx.focus_visible();
                s.last_within_group = tx.focus_within("focus-group");
                s.transitions += 1;
            })
            .child(text(title).font(type_::BODY).color(t.text))
            .child(text(subtitle).font(type_::LABEL).color(t.text_secondary))
            .child(text(if focused {
                if s.focus_visible { "Keyboard focus" } else { "Pointer focus" }
            } else {
                "Idle"
            }).font(type_::LABEL).color(if focused { accent } else { t.text_secondary }))
    };
    let status = format!(
        "group within: {} · focus_visible: {} · transitions: {}",
        s.group_within, s.focus_visible, s.transitions
    );
    col().w_full().h_full().center().padding(20.0).gap(12.0)
        .child(
            col().key("focus-group").padding(14.0).gap(10.0).radius(radius::LG)
                .fill(t.bg)
                .border(1.0, if s.group_within { accent } else { t.card_border })
                .on_focus_within_change(|s: &mut FocusCard, within, tx: &mut Tx| {
                    s.group_within = within;
                    s.last_within_group = tx.focus_within("focus-group");
                })
                .child(text(if s.group_within { "Card group · focus within" } else { "Card group · click or Tab to focus" })
                    .key("group-header").font(type_::LABEL).color(if s.group_within { accent } else { t.text_secondary }))
                .child(
                    row().gap(10.0)
                        .child(card("card-a", "Primary card", "Tab shows accent border", s.card_a_focused, true))
                        .child(card("card-b", "Secondary card", "Click keeps rest border", s.card_b_focused, false)),
                ),
        )
        .child(
            row().gap(10.0).center_y()
                .child(
                    row().key("blur-btn").h(26.0).px(10.0).center().radius(radius::SM).fill(t.control_track).hover(t.hover)
                        .role("button").label("Blur").focusable(true)
                        .on_press_tx(|_: &mut FocusCard, tx: &mut Tx| tx.blur())
                        .child(text("Blur").font(type_::LABEL).color(t.text)),
                )
                .child(text(status).key("focus-status").font(type_::LABEL).color(t.text_secondary)),
        )
}

fn main() {
    let spec = AppSpec { scenes: SCENES, ..AppSpec::new("Preferences", (640.0, 420.0), init, view) };
    App::new(spec)
        .playground(
            Playground::<Prefs>::new()
                .page("Lists/Live devices", devices_page)
                .trigger("Lists/Many devices", 'a', "Add device", |s, _| s.devices.push("New device"))
                .trigger("Lists/Many devices", 'x', "Remove the last", |s, _| { s.devices.pop(); })
                .side("Lists/Many devices", device_count)
                .variant("Lists/Many devices", "Six", |_| {})
                .variant("Lists/Many devices", "Just this one", |s| s.devices.truncate(1))
                .variant("Lists/Many devices", "Long names", |s| {
                    s.devices = vec!["Studio desk in the back office", "Travel laptop (borrowed for the conference)", "Kitchen tablet"];
                })
                .story(Story::new("Lists/Every device", every_device, devices).stage(Stage::Scroll))
                .story(scroll_anchor_story::anchor_story())
                .story(
                    Story::new("Layout/Display scale", scale_readout_init, scale_readout_view)
                        .trigger('z', "Cycle zoom", |s, _| {
                            let i = ZOOM_STEPS.iter().position(|z| (s.zoom - z).abs() < 1e-3).unwrap_or(0);
                            s.zoom = ZOOM_STEPS[(i + 1) % ZOOM_STEPS.len()];
                        })
                        .variant("100% zoom", |_| {})
                        .variant("150% zoom", |s| s.zoom = 1.5)
                        .variant("200% zoom", |s| s.zoom = 2.0),
                )
                .story(Story::new("Windows/Preferences", init, view).stage(Stage::Fill))
                .story(
                    Story::new("Progress/Upload", upload_init, upload_view)
                        .poll(upload_poll)
                        .trigger('r', "Run, pause, resume", upload_replay)
                        .trigger('f', "Finish", |u, _| { u.progress = 100; u.running = false; })
                        .variant("From the start", |_| {})
                        .variant("Half done", |u| u.progress = 48),
                )
                .story(
                    Story::overlay("Toasts/Sync toast", &TOAST, || Toast { message: "Synced 3 devices", failed: false })
                        .trigger('e', "Fail", |s, _| { s.message = "Sync failed, retrying"; s.failed = true; })
                        .variant("Synced", |_| {})
                        .variant("Failed", |s| { s.message = "Sync failed, retrying"; s.failed = true; }),
                )
                .story(
                    Story::new("Controls/Buttons/Hover & press", hover_press::HoverPressDemo::default, hover_press::view)
                        .trigger('p', "Toggle popup cover", |s, _| s.popup_open = !s.popup_open)
                        .trigger('t', "Toggle hover_passes_below", |s, _| s.pass_through = !s.pass_through)
                        .variant("Closed", |_| {})
                        .variant("Popup blocking", |s| { s.popup_open = true; s.pass_through = false; })
                        .variant("Pass-through cover", |s| { s.popup_open = true; s.pass_through = true; }),
                )
                .story(
                    Story::new("Controls/Popups/Passive press observer", any_press::AnyPressDemo::default, any_press::view)
                        .trigger('o', "Toggle popup", |s, _| s.popup_open = !s.popup_open)
                        .variant("Open", |_| {})
                        .variant("Closed", |s| s.popup_open = false),
                )
                .story(
                    Story::new("Controls/Buttons/Press cursor & bounds", press_pointer::PressPointerDemo::default, press_pointer::view)
                        .trigger('c', "Toggle press_cursor", |s, _| s.pointer_default = !s.pointer_default)
                        .variant("Pointer cursor on", |_| {})
                        .variant("Default cursor (off)", |s| s.pointer_default = false),
                )
                .story(
                    Story::new("Controls/Motion/Loops & press offset", loops_press::LoopsPressDemo::default, loops_press::view)
                        .trigger('r', "Replay 2× shimmer", |s, _| s.shimmer_gen = s.shimmer_gen.wrapping_add(1))
                        .trigger('p', "Toggle repeating pulse", |s, _| s.pulsing = !s.pulsing)
                        .variant("Active", |_| {})
                        .variant("Pulse paused", |s| s.pulsing = false),
                )
                .story(
                    Story::new("Controls/Switches/Toggle", || Switch { on: true, dimmed: false, label: "Sync across devices" }, switch_story)
                        .trigger('t', "Flip", |s, _| s.on = !s.on)
                        .variant("On", |_| {})
                        .variant("Off", |s| s.on = false)
                        .variant("Dimmed", |s| s.dimmed = true),
                )
                .story(Story::new("Controls/Switches/Long label", || Switch { on: false, dimmed: false, label: "Only sync over Wi-Fi when on battery" }, switch_story))
                .story(
                    Story::new("Controls/Focus card", FocusCard::default, focus_card_view)
                        .variant("Resting", |_| {})
                        .variant("Card focused", |s| { s.card_a_focused = true; s.group_within = true; s.focus_visible = true; }),
                )
                .story(
                    Story::new("Controls/State-independent handlers", any_handlers::AnyHandlersDemo::default, any_handlers::view)
                        .trigger('w', "Toggle strip width", |s, _| s.wide = !s.wide)
                        .trigger('s', "Toggle offscreen spacer", |s, _| s.top_spacer = !s.top_spacer)
                        .variant("Visible & compact", |_| {})
                        .variant("Wide strip", |s| s.wide = true)
                        .variant("Offscreen", |s| s.top_spacer = true),
                )
                .story(Story::new("Controls/Rows/Two devices", device_rows(&["Studio desk", "Travel laptop"]), devices))
                .story(Story::new("Controls/Rows/One device", device_rows(&["Studio desk"]), devices))
                .story(Story::new("Controls/Rows/Long names", device_rows(&["Studio desk in the back office", "Travel laptop (borrowed for the conference)"]), devices))
                .story(
                    Story::new("Layout/Fit & fractions", layout_demo_init, layout_demo_view)
                        .trigger('a', "Cycle alignment", |s, _| s.align = (s.align + 1) % 4)
                        .trigger('s', "Cycle split", |s, _| {
                            s.split = if s.split < 0.3 { 0.4 } else if s.split < 0.55 { 0.65 } else { 0.25 };
                        })
                        .variant("40 / 60 · stretch", |_| {})
                        .variant("25 / 75 · centred col", |s| { s.align = 1; s.split = 0.25; })
                        .variant("65 / 35 · row center_y", |s| { s.align = 3; s.split = 0.65; }),
                )
                .story(text_indent_story::story())
                .story(emoji_family_story::story())
                .story(
                    Story::new("Text/Runtime font", RuntimeFont::default, runtime_font_view)
                        .trigger('l', "Register font bytes", |s, play| {
                            s.families = rune::text::add_font(RUNTIME_FONT);
                            s.active = !s.families.is_empty();
                            play.wake_after(Duration::ZERO);
                        })
                        .trigger('r', "Use Inter", |s, _| s.active = false)
                        .variant("Before load", |_| {})
                        .variant("Loaded", |s| {
                            s.families = rune::text::add_font(RUNTIME_FONT);
                            s.active = !s.families.is_empty();
                        }),
                )
                .story(
                    Story::new("Layout/Element inspector", ElementInspector::default, inspector_view)
                        .poll(inspector_poll)
                        .trigger('i', "Inspect frame", |s, play| {
                            s.pending_inspect = true;
                            play.wake_after(Duration::ZERO);
                        })
                        .variant("Idle", |_| {})
                        .variant("Inspected", |s| s.pending_inspect = true),
                )
                .story(
                    Story::new("Lifecycle/On mount", mount::MountDemo::default, mount::view)
                        .trigger('t', "Toggle tooltip", |s, _| {
                            s.show_tip = !s.show_tip;
                            if !s.show_tip {
                                s.status.clear();
                            }
                        })
                        .trigger('r', "Rebuild view", |s, _| s.rebuilds += 1)
                        .variant("Hidden", |_| {})
                        .variant("Shown", |s| *s = mount::MountDemo::shown()),
                )
                .story(
                    Story::new("Layout/Pointer pick", pick::PointerPick::default, pick::view)
                        .poll(pick::poll)
                        .trigger('m', "Cycle pick mode", |s, _| s.mode = s.mode.next())
                        .variant("Interactive (row)", |s| *s = pick::PointerPick::with_probe(pick::PickMode::Interactive, "pick-row-1"))
                        .variant("All (passive layer)", |s| *s = pick::PointerPick::with_probe(pick::PickMode::All, "pick-row-1"))
                        .variant("Drop target", |s| *s = pick::PointerPick::with_probe(pick::PickMode::DropTargets, "pick-drop-zone"))
                        .variant("Tilted (local coords)", |s| *s = pick::PointerPick::with_probe(pick::PickMode::Interactive, "pick-rotated")),
                )
                .group_order(&["Controls"])
                .start_folded(true)
                .sizes(&[("Compact", (420.0, 300.0)), ("Wide", (900.0, 560.0))]),
        )
        .run();
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn runtime_font_story_registers_font_bytes_and_updates_sample_width() {
        let mut rt = Runtime::new(RuntimeFont::default(), runtime_font_view, (520.0, 300.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let width_of = |rt: &Runtime<RuntimeFont>, key: &str| {
            rt.elements().iter().find(|e| e.key.as_deref() == Some(key)).unwrap().target[2]
        };
        let before_w = width_of(&rt, "sample-text");
        assert!(rt.state.families.is_empty());

        let tree = rt.accessibility_json();
        let tree_text = tree.to_string();
        assert!(tree_text.contains("Register font bytes"), "button has accessible label");

        let (x, y) = rt.find("register-font").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);

        assert_eq!(rt.state.families, vec!["IBM Plex Mono"]);
        assert!(rt.state.active);
        let after_w = width_of(&rt, "sample-text");
        assert!((after_w - before_w).abs() > 5.0, "sample text width updated: {before_w} -> {after_w}");
    }

    #[test]
    fn element_inspector_story_reads_presented_elements_via_tx() {
        let mut rt = Runtime::new(ElementInspector::default(), inspector_view, (560.0, 320.0), Env::new(true, 1.0));
        rt.frame(0.0);
        assert!(rt.state.rows.is_empty());
        assert_eq!(rt.presented_focus_key(), None);

        let (x, y) = rt.find("inspect-btn").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);

        assert_eq!(rt.presented_focus_key(), Some("inspect-btn"));
        assert!(rt.state.highlight);
        assert!(rt.state.bordered >= 1, "bordered card should be counted");
        assert!(rt.state.clipped >= 1, "scroll box should be counted");
        assert_eq!(rt.state.rows.len(), INSPECT_KEYS.len());
        let card = rt.state.rows.iter().find(|r| r.key == "sample-card").unwrap();
        assert!(card.border.is_some());
        let pill = rt.state.rows.iter().find(|r| r.key == "sample-pill").unwrap();
        assert!(pill.detail.contains("Deploys the active build"));
        let trunc = rt.state.rows.iter().find(|r| r.key == "sample-trunc").unwrap();
        assert!(trunc.detail.contains("(cut)"));
    }

    #[test]
    fn focus_card_story_tracks_focus_change_within_and_modality() {
        use rune::{Key, KeyEvent, Mods};

        let mut rt = Runtime::new(FocusCard::default(), focus_card_view, (460.0, 260.0), Env::new(true, 1.0));
        rt.frame(0.0);
        assert!(!rt.state.card_a_focused && !rt.state.group_within && rt.state.transitions == 0);

        let tree = rt.accessibility_json().to_string();
        assert!(tree.contains("Primary card") && tree.contains("Secondary card"));

        // Tab focuses card-a with keyboard focus-visible modality.
        rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
        rt.frame(1.0 / 60.0);
        assert!(rt.state.card_a_focused && rt.state.group_within && rt.state.focus_visible && rt.state.last_within_group);

        // Clicking card-b moves focus with pointer modality (focus_visible == false) while group_within stays true.
        let (bx, by) = rt.find("card-b").unwrap();
        rt.pointer_down(bx, by);
        rt.pointer_up(bx, by);
        rt.frame(1.0 / 60.0);
        assert!(!rt.state.card_a_focused && rt.state.card_b_focused && rt.state.group_within && !rt.state.focus_visible);

        // Clicking Blur drops focus and clears group_within.
        let (rx, ry) = rt.find("blur-btn").unwrap();
        rt.pointer_down(rx, ry);
        rt.pointer_up(rx, ry);
        rt.frame(1.0 / 60.0);
        assert!(!rt.state.card_b_focused && !rt.state.group_within);
    }

    #[test]
    fn mount_story_reflects_on_mount_and_chained_badge_in_same_frame() {
        let mut rt = Runtime::new(mount::MountDemo::default(), mount::view, (460.0, 260.0), Env::new(true, 1.0));
        rt.frame(0.0);
        assert_eq!((rt.state.mounts, rt.state.child_mounts), (0, 0));

        // Clicking Show tooltip mounts the panel and chained badge in the very same frame.
        let (tx, ty) = rt.find("toggle-tip").unwrap();
        rt.pointer_down(tx, ty);
        rt.pointer_up(tx, ty);
        rt.frame(1.0 / 60.0);
        assert_eq!((rt.state.mounts, rt.state.child_mounts), (1, 1));
        let status = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("mount-tip-status")).and_then(|e| e.text);
        assert_eq!(status.as_deref(), Some("Mounted (#1) in same frame"));
        let badge = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("mount-chain-text")).and_then(|e| e.text);
        assert_eq!(badge.as_deref(), Some("chain #1"));
        assert!(rt.accessibility_json().to_string().contains("Mounted (#1) in same frame"));

        // Rebuilding the view does not re-fire on_mount.
        let (rx, ry) = rt.find("rebuild-tip").unwrap();
        rt.pointer_down(rx, ry);
        rt.pointer_up(rx, ry);
        rt.frame(1.0 / 60.0);
        assert_eq!((rt.state.rebuilds, rt.state.mounts, rt.state.child_mounts), (1, 1, 1));

        // Hiding and showing again re-fires both (even while the exit transition is still running).
        let (tx, ty) = rt.find("toggle-tip").unwrap();
        rt.pointer_down(tx, ty);
        rt.pointer_up(tx, ty);
        rt.frame(1.0 / 60.0);
        let (tx, ty) = rt.find("toggle-tip").unwrap();
        rt.pointer_down(tx, ty);
        rt.pointer_up(tx, ty);
        rt.frame(1.0 / 60.0);
        assert_eq!((rt.state.mounts, rt.state.child_mounts), (2, 2));
    }
}
