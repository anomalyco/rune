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

use std::time::Duration;

use rune::tok::{self, Theme, motion, radius, type_};
use rune::{Align, App, AppSpec, Color, Cx, El, OverlaySpec, Play, Playground, ScreenEdge, Stage, Story, Tx, col, el, row, text};

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
                    Story::new("Controls/Switches/Toggle", || Switch { on: true, dimmed: false, label: "Sync across devices" }, switch_story)
                        .trigger('t', "Flip", |s, _| s.on = !s.on)
                        .variant("On", |_| {})
                        .variant("Off", |s| s.on = false)
                        .variant("Dimmed", |s| s.dimmed = true),
                )
                .story(Story::new("Controls/Switches/Long label", || Switch { on: false, dimmed: false, label: "Only sync over Wi-Fi when on battery" }, switch_story))
                .story(Story::new("Controls/Rows/Two devices", device_rows(&["Studio desk", "Travel laptop"]), devices))
                .story(Story::new("Controls/Rows/One device", device_rows(&["Studio desk"]), devices))
                .story(Story::new("Controls/Rows/Long names", device_rows(&["Studio desk in the back office", "Travel laptop (borrowed for the conference)"]), devices))
                .group_order(&["Controls"])
                .start_folded(true)
                .sizes(&[("Compact", (420.0, 300.0)), ("Wide", (900.0, 560.0))]),
        )
        .run();
}
