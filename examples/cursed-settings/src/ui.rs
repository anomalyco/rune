//! The settings window, in OpenCode's settings design language: a sidebar of pages, then
//! titled sections of rows with a control on the right. Nothing here knows about effects
//! except the marks that tell the compositor which element is which.

use rune::tok::motion;
use rune::{Align, Color, Cx, Drag, El, Phase, Shape, Spring, TextStyle, Transition, Tx, canvas, col, el, icon, row, spacer, text};
use rune_kit::{Badge, BadgeVariant, Button, ButtonVariant, ControlSize, Kit, SegmentedControl, Select, Switch, TextInput, Toast, ToastRegion};
use rune_theme_opencode::icons as oc;

use crate::{APPEARANCES, Cursed, HAPTICS, PAGES, Page, icons};

pub const SIDEBAR_W: f32 = 232.0;
const NAV_PITCH: f32 = 30.0;
const NAV_H: f32 = 28.0;

// ---------------------------------------------------------------- marks

/// A mark: lighting material and effect node. Bit 31 keeps every explicit mark non-zero (0 inherits).
pub fn mk(material: u8, node: u16) -> u32 {
    (1 << 31) | ((material as u32) << 16) | node as u32
}

pub fn mark_node(m: u32) -> u16 {
    (m & 0xFFFF) as u16
}

pub fn mark_material(m: u32) -> u8 {
    ((m >> 16) & 0xFF) as u8
}

/// How a rect stands on the UI plane when the lights are on.
pub mod mat {
    /// Flat: no occluder.
    pub const NONE: u8 = 0;
    pub const CARD: u8 = 1;
    pub const CONTROL: u8 = 2;
    pub const SEL: u8 = 3;
}

/// Effect anchors.
pub mod node {
    pub const NAV: u16 = 1;
    pub const SEARCH: u16 = 2;
    pub const ALLOWED: u16 = 10;
    pub const IP: u16 = 11;
    pub const PERIMETER: u16 = 20;
    pub const RELOAD: u16 = 30;
    pub const CACHE: u16 = 31;
    pub const TOAST: u16 = 0xFFFE;
    pub const CURSOR: u16 = 0xFFFF;
}

// ---------------------------------------------------------------- type

const TITLE: TextStyle = TextStyle::new(15.0, 640.0).lh(22.0);
const SECTION: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0);

/// A press that counts as a tap (haptic feedback follows taps).
fn tap(f: impl Fn(&mut Cursed) + 'static) -> impl Fn(&mut Cursed) + 'static {
    move |s: &mut Cursed| {
        f(s);
        s.taps += 1;
    }
}

pub fn dark_theme(s: &Cursed, system_dark: bool) -> bool {
    match s.p.appearance {
        0 => false,
        _ => system_dark,
    }
}

pub fn view(s: &Cursed, cx: &mut Cx<Cursed>) -> El {
    cx.reduced_motion(s.p.reduce_motion);
    let k = rune_theme_opencode::kit(dark_theme(s, cx.system_dark()));
    let hair = 1.0 / cx.env().scale;
    let c = &k.c;
    let mut overlays = Vec::new();
    if s.toast_until.is_some() {
        let toast = Toast::new("restored", "Defaults restored")
            .description("The fire department has been notified.")
            .icon(k, &oc::CIRCLE_CHECK)
            .build(k);
        overlays.push(ToastRegion::new().toast(toast).build(k).mark(mk(mat::NONE, node::TOAST)));
    }
    if s.show_cursor
        && let Some(p) = s.cursor
    {
        overlays.push(pointer(p));
    }
    let base = row()
        .key("window")
        .fill(c.bg_base)
        .child(sidebar(s, k))
        .child(el().key("sidebar-edge").w(hair).h_full().fill(c.border_base))
        .child(content(s, k, hair));
    rune_kit::root(base, overlays)
}

// ---------------------------------------------------------------- sidebar

fn sidebar(s: &Cursed, k: &Kit) -> El {
    let c = &k.c;
    let w = SIDEBAR_W - 24.0;
    let idx = PAGES.iter().position(|p| *p == s.page).unwrap_or(0);
    let mut nav = el()
        .key("nav")
        .w(w)
        .h(PAGES.len() as f32 * NAV_PITCH)
        .mark(mk(mat::NONE, node::NAV))
        .child(
            el().key("nav-sel")
                .absolute(0.0, idx as f32 * NAV_PITCH)
                .size(w, NAV_H)
                .radius(6.0)
                .fill(c.layer_03)
                .lead(motion::LEAD, motion::DEFAULT)
                .mark(mk(mat::SEL, node::NAV)),
        );
    for (i, p) in PAGES.into_iter().enumerate() {
        let selected = i == idx;
        let glyph = match p {
            Page::General => &oc::SETTINGS_GEAR,
            Page::Appearance => &icons::SUN,
            Page::Network => &oc::GLOBE,
            Page::Security => &icons::SHIELD,
            Page::Accessibility => &icons::PERSON,
            Page::Developer => &oc::CONSOLE,
        };
        nav = nav.child(
            row()
                .key(format!("nav-{}", p.id()))
                .role("tab")
                .label(p.title())
                .absolute(0.0, i as f32 * NAV_PITCH)
                .size(w, NAV_H)
                .px(8.0)
                .gap(8.0)
                .center_y()
                .radius(6.0)
                .hover(if selected { Color::TRANSPARENT } else { c.hover })
                .on_press(tap(move |s: &mut Cursed| s.page = p))
                .child(icon(glyph, 16.0).key("icon").color(if selected { c.icon_base } else { c.icon_muted }))
                .child(text(p.title()).key("label").font(k.ty.ui).color(if selected { c.text_base } else { c.text_muted })),
        );
    }
    col()
        .key("sidebar")
        .w(SIDEBAR_W)
        .h_full()
        .fill(c.page)
        .px(12.0)
        .pt(20.0)
        .pb(16.0)
        .gap(16.0)
        .child(
            TextInput::new(&s.search)
                .placeholder("Search")
                .leading_icon(&oc::MAGNIFYING_GLASS)
                .width(Some(w))
                .on_edit(|s: &mut Cursed| &mut s.search)
                .build(k)
                .key("search")
                .mark(mk(mat::CONTROL, node::SEARCH)),
        )
        .child(nav)
        .child(spacer())
        .child(
            Button::new("Restore Defaults")
                .variant(ButtonVariant::Neutral)
                .on_press(tap(|s: &mut Cursed| s.restore()))
                .build(k)
                .key("restore")
                .mark(mk(mat::CONTROL, 0)),
        )
}

// ---------------------------------------------------------------- content

fn content(s: &Cursed, k: &Kit, hair: f32) -> El {
    let page = match s.page {
        Page::General => general(s, k, hair),
        Page::Appearance => appearance(s, k, hair),
        Page::Network => network(s, k, hair),
        Page::Security => security(s, k, hair),
        Page::Accessibility => accessibility(s, k, hair),
        Page::Developer => developer(s, k, hair),
    };
    col().key("content").grow(1.0).h_full().fill(k.c.bg_base).child(
        page.key(format!("page-{}", s.page.id()))
            .w_full()
            .px(40.0)
            .pt(32.0)
            .gap(24.0)
            .transition(Transition::asymmetric(Transition::opacity(), Transition::identity()))
            .opacity_motion(Spring::new(0.18, 0.0)),
    )
}

fn header(k: &Kit, title: &str, desc: &str) -> El {
    col()
        .key("header")
        .gap(2.0)
        .child(text(title).key("title").font(TITLE).color(k.c.text_base))
        .child(text(desc).key("desc").font(k.ty.ui).color(k.c.text_muted))
}

/// A titled list of rows with hairlines between them. `node` anchors effects to the card.
fn section(k: &Kit, hair: f32, key: &str, title: &str, trailing: Option<El>, node: u16, rows: Vec<El>) -> El {
    let c = &k.c;
    let mut inner = col().key("rows").w_full().mark(mk(mat::NONE, node));
    let n = rows.len();
    for (i, r) in rows.into_iter().enumerate() {
        inner = inner.child(r);
        if i + 1 < n {
            inner = inner.child(el().key(format!("sep{i}")).h(hair).w_full().fill(c.border_base));
        }
    }
    let mut head = row().key("head").w_full().h(24.0).center_y().justify_between().child(text(title).key("title").font(SECTION).color(c.text_base));
    if let Some(t) = trailing {
        head = head.child(t);
    }
    col().key(key).w_full().gap(10.0).child(head).child(
        col()
            .key(format!("{key}-card"))
            .w_full()
            .radius(8.0)
            .fill(c.layer_01)
            .border(0.5, c.border_muted)
            .px(16.0)
            .mark(mk(mat::CARD, node))
            .child(inner),
    )
}

fn setting(k: &Kit, key: &str, title: &str, desc: &str, control: El) -> El {
    row()
        .key(key)
        .w_full()
        .py(12.0)
        .gap(16.0)
        .center_y()
        .child(
            col()
                .key("copy")
                .grow(1.0)
                .shrink()
                .gap(1.0)
                .child(text(title).key("title").font(k.ty.ui_medium).color(k.c.text_base))
                .child(text(desc).key("desc").font(k.ty.ui).color(k.c.text_muted)),
        )
        .child(control)
}

fn switch(k: &Kit, key: &str, on: bool, label: &str, flip: impl Fn(&mut Cursed) + 'static) -> El {
    Switch::new(on).label(label).hide_label().on_toggle(tap(flip)).build(k).key(key).mark(mk(mat::CONTROL, 0))
}

fn toggle_row(k: &Kit, key: &str, title: &str, desc: &str, on: bool, flip: impl Fn(&mut Cursed) + 'static) -> El {
    setting(k, key, title, desc, switch(k, &format!("{key}-switch"), on, title, flip))
}

/// A plain row: a name with a trailing value.
fn item(k: &Kit, key: &str, lead: El, name: &str, trailing: El) -> El {
    row()
        .key(key)
        .w_full()
        .h(44.0)
        .gap(10.0)
        .center_y()
        .child(lead)
        .child(text(name).key("name").font(k.ty.ui_medium).color(k.c.text_base).grow(1.0))
        .child(trailing)
}

fn app_tile(key: &str, letter: &str, color: Color) -> El {
    row()
        .key(key)
        .size(22.0, 22.0)
        .radius(5.5)
        .fill(color)
        .center()
        .child(text(letter).key("letter").font(TextStyle::new(11.0, 640.0)).color(Color::WHITE))
}

fn muted(k: &Kit, key: &str, s: &str) -> El {
    text(s).key(key).font(k.ty.ui).color(k.c.text_muted)
}

fn hx(h: u32) -> Color {
    rune::color::hex(&format!("#{h:06x}")).unwrap_or(Color::BLACK)
}

// ---------------------------------------------------------------- pages

fn general(s: &Cursed, k: &Kit, hair: f32) -> El {
    let p = &s.p;
    let mut layer = Vec::new();
    col()
        .child(header(k, "General", "Startup, search and updates."))
        .child(section(
            k,
            hair,
            "startup",
            "Startup",
            None,
            0,
            vec![
                toggle_row(k, "login", "Launch at login", "Open OpenCode when you log in.", p.launch_at_login, |s| {
                    s.p.launch_at_login = !s.p.launch_at_login
                }),
                toggle_row(k, "fast", "Lightning-fast startup", "Skip the splash screen and open instantly.", p.fast_startup, |s| {
                    s.p.fast_startup = !s.p.fast_startup
                }),
            ],
        ))
        .child(section(
            k,
            hair,
            "search",
            "Search",
            None,
            0,
            vec![
                toggle_row(k, "spotlight", "Show in Spotlight", "Let Spotlight find your sessions and settings.", p.spotlight, |s| {
                    s.p.spotlight = !s.p.spotlight
                }),
                setting(
                    k,
                    "language",
                    "Language",
                    "Used for menus and messages.",
                    Select::new("lang", &["English"]).label("Language").current(Some("English")).build(k, &mut layer),
                ),
            ],
        ))
        .child(section(
            k,
            hair,
            "updates",
            "Updates",
            None,
            0,
            vec![toggle_row(k, "updates", "Automatic updates", "Download and install updates in the background.", p.auto_update, |s| {
                s.p.auto_update = !s.p.auto_update
            })],
        ))
}

fn appearance(s: &Cursed, k: &Kit, hair: f32) -> El {
    let p = &s.p;
    col()
        .child(header(k, "Appearance", "How OpenCode looks."))
        .child(section(
            k,
            hair,
            "theme",
            "Theme",
            None,
            0,
            vec![
                setting(
                    k,
                    "mode",
                    "Appearance",
                    "Match the system, or pick one.",
                    SegmentedControl::new(&APPEARANCES, Some(p.appearance))
                        .width(Some(204.0))
                        .on_pick(|s: &mut Cursed, i: usize, tx: &mut Tx| {
                            s.p.appearance = i;
                            s.taps += 1;
                            tx.color_motion(Spring::new(0.4, 0.0));
                        })
                        .build(k)
                        .key("mode-control")
                        .mark(mk(mat::CONTROL, 0)),
                ),
                setting(k, "brightness", "Brightness", "How bright the window is.", slider(k, p.brightness)),
            ],
        ))
        .child(section(
            k,
            hair,
            "text",
            "Text",
            None,
            0,
            vec![
                setting(k, "size", "Text size", "Used across the app.", Select::new("size", &["13 pt"]).label("Text size").current(Some("13 pt")).build(k, &mut Vec::new())),
                setting(k, "font", "Code font", "Used in diffs and the terminal.", Select::new("font", &["IBM Plex Mono"]).label("Code font").current(Some("IBM Plex Mono")).build(k, &mut Vec::new())),
            ],
        ))
}

fn network(s: &Cursed, k: &Kit, hair: f32) -> El {
    let p = &s.p;
    let allow = |key: &str, letter: &str, col_: u32, name: &str| {
        item(k, key, app_tile("tile", letter, hx(col_)), name, muted(k, "state", "Allow incoming connections"))
    };
    col()
        .child(header(k, "Network", "Firewall and addresses."))
        .child(section(
            k,
            hair,
            "firewall",
            "Firewall",
            None,
            0,
            vec![toggle_row(k, "firewall", "Firewall", "Block incoming connections from unknown apps.", p.firewall, |s| {
                s.p.firewall = !s.p.firewall
            })],
        ))
        .child(section(
            k,
            hair,
            "allowed",
            "Allowed apps",
            None,
            crate::ui::node::ALLOWED,
            vec![
                allow("opencode", "O", 0x1d1d1f, "OpenCode"),
                allow("rune", "R", 0x6d4aff, "Rune"),
                allow("display", "S", 0x2f80ed, "Studio Display Updater"),
            ],
        ))
        .child(section(
            k,
            hair,
            "address",
            "Address",
            None,
            0,
            vec![
                toggle_row(k, "static", "Static IP", "Use a fixed address instead of DHCP.", p.static_ip, |s| s.p.static_ip = !s.p.static_ip),
                setting(
                    k,
                    "ip",
                    "IP address",
                    "This Mac's address on the local network.",
                    TextInput::new(&s.ip)
                        .label("IP address")
                        .width(Some(168.0))
                        .disabled(!p.static_ip)
                        .on_edit(|s: &mut Cursed| &mut s.ip)
                        .build(k)
                        .key("ip-field")
                        .mark(mk(mat::CONTROL, node::IP)),
                ),
            ],
        ))
}

fn security(s: &Cursed, k: &Kit, hair: f32) -> El {
    let p = &s.p;
    let door = |key: &str, name: &str, state: &str| {
        item(
            k,
            key,
            el().key("dot").size(8.0, 8.0).radius(4.0).fill(k.c.success.icon).ml(7.0).mr(7.0),
            name,
            Badge::new(state).variant(BadgeVariant::Subtle).build(k),
        )
    };
    col()
        .child(header(k, "Security", "Locks, passwords and the perimeter."))
        .child(section(
            k,
            hair,
            "protection",
            "Protection",
            None,
            0,
            vec![
                toggle_row(k, "fence", "Electric fence", "Protect the perimeter of your network.", p.fence, |s| s.p.fence = !s.p.fence),
                toggle_row(k, "sleep", "Require password after sleep", "Ask for your password when the display wakes.", p.lock_on_sleep, |s| {
                    s.p.lock_on_sleep = !s.p.lock_on_sleep
                }),
            ],
        ))
        .child(section(
            k,
            hair,
            "perimeter",
            "Perimeter",
            None,
            node::PERIMETER,
            vec![
                door("front", "Front door", "Locked"),
                door("back", "Back door", "Locked"),
                door("ssh", "Port 22", "Closed"),
                door("server", "Port 4096", "OpenCode only"),
            ],
        ))
}

fn accessibility(s: &Cursed, k: &Kit, hair: f32) -> El {
    let p = &s.p;
    col()
        .child(header(k, "Accessibility", "Display, motion and feedback."))
        .child(section(
            k,
            hair,
            "display",
            "Display",
            None,
            0,
            vec![
                toggle_row(k, "contrast", "Increase contrast", "Make edges and text stand out more.", p.contrast, |s| s.p.contrast = !s.p.contrast),
                toggle_row(k, "transparency", "Reduce transparency", "Make glass surfaces solid.", p.reduce_transparency, |s| {
                    s.p.reduce_transparency = !s.p.reduce_transparency
                }),
            ],
        ))
        .child(section(
            k,
            hair,
            "motion",
            "Motion",
            None,
            0,
            vec![toggle_row(k, "reduce", "Reduce motion", "Calmer, shorter animations.", p.reduce_motion, |s| {
                s.p.reduce_motion = !s.p.reduce_motion
            })],
        ))
        .child(section(
            k,
            hair,
            "feedback",
            "Feedback",
            None,
            0,
            vec![setting(
                k,
                "haptics",
                "Haptic feedback",
                "How strongly clicks are felt.",
                SegmentedControl::new(&HAPTICS, Some(p.haptics))
                    .width(Some(204.0))
                    .on_pick(|s: &mut Cursed, i: usize, _: &mut Tx| {
                        s.p.haptics = i;
                        s.taps += 1;
                    })
                    .build(k)
                    .key("haptics-control")
                    .mark(mk(mat::CONTROL, 0)),
            )],
        ))
}

fn developer(s: &Cursed, k: &Kit, hair: f32) -> El {
    let p = &s.p;
    let c = &k.c;
    let mut reload = Button::new("Reload now").on_press(tap(|_: &mut Cursed| {})).build(k).key("reload-btn").mark(mk(mat::CONTROL, node::RELOAD));
    if p.hot_reload {
        reload = reload.fill(hx(0xd8401a)).motion(Spring::new(1.6, 0.0));
    } else {
        reload = reload.motion(Spring::new(2.0, 0.0));
    }
    let size = |key: &str, name: &str, v: &str| item(k, key, icon(&oc::FOLDER, 16.0).key("icon").color(c.icon_muted).ml(3.0).mr(3.0), name, muted(k, "size", v));
    let rows = if p.flushed {
        vec![row().key("empty").w_full().h(44.0).center_y().child(muted(k, "label", "Nothing cached."))]
    } else {
        vec![
            size("shaders", "Shader cache", "1.2 GB"),
            size("thumbs", "Thumbnails", "640 MB"),
            size("snapshots", "Session snapshots", "512 MB"),
        ]
    };
    let flush = Button::new("Flush cache")
        .control_size(ControlSize::Regular)
        .disabled(p.flushed || s.flush_t0.is_some())
        .on_press(tap(|s: &mut Cursed| s.flush_requested = true))
        .build(k)
        .key("flush-btn")
        .mark(mk(mat::CONTROL, 0));
    col()
        .child(header(k, "Developer", "Builds, reloads and caches."))
        .child(section(
            k,
            hair,
            "reloading",
            "Reloading",
            None,
            0,
            vec![
                toggle_row(k, "hot", "Hot reload", "Reload the app when files change.", p.hot_reload, |s| s.p.hot_reload = !s.p.hot_reload),
                setting(k, "reload", "Reload renderer", "Rebuild shaders and reload the window.", reload),
            ],
        ))
        .child(section(
            k,
            hair,
            "cache",
            if p.flushed { "Cache · Zero KB" } else { "Cache · 2.4 GB" },
            Some(flush),
            node::CACHE,
            rows,
        ))
}

// ---------------------------------------------------------------- controls

fn slider(k: &Kit, v: f32) -> El {
    let c = &k.c;
    let (w, h, kd, bar) = (168.0, 20.0, 16.0, 4.0);
    let travel = w - kd;
    let to_value = move |x: f32| ((x - kd / 2.0) / travel).clamp(0.0, 1.0);
    row()
        .key("brightness-control")
        .gap(10.0)
        .center_y()
        .child(icon(&icons::SUN, 14.0).key("dim").color(c.icon_faint))
        .child(
            el().key("brightness-slider")
                .role("slider")
                .label("Brightness")
                .size(w, h)
                .motion(motion::SNAPPY)
                .on_drag(move |s: &mut Cursed, d: &mut Drag, tx: &mut Tx| {
                    let knob = kd / 2.0 + s.p.brightness * travel;
                    match d.phase {
                        Phase::Down => {
                            d.data = s.p.brightness;
                            if (d.x - knob).abs() > kd / 2.0 + 3.0 {
                                s.p.brightness = to_value(d.x);
                            }
                        }
                        Phase::Move => {
                            let knob = kd / 2.0 + d.data * travel;
                            let grab = if (d.start_x - knob).abs() <= kd / 2.0 + 3.0 { d.start_x - knob } else { 0.0 };
                            s.p.brightness = to_value(d.x - grab);
                            tx.instant();
                        }
                        Phase::Up => { if d.cancelled() { s.p.brightness = d.data; } }
                    }
                })
                .child(el().key("track").size(w, bar).absolute(0.0, (h - bar) / 2.0).radius(bar / 2.0).fill(c.layer_04))
                .child(el().key("fill").size(kd / 2.0 + v * travel, bar).absolute(0.0, (h - bar) / 2.0).radius(bar / 2.0).fill(c.icon_base))
                .child(
                    el().key("knob")
                        .size(kd, kd)
                        .absolute(v * travel, (h - kd) / 2.0)
                        .radius(kd / 2.0)
                        .fill(c.knob_on)
                        .border(0.5, c.border_strong)
                        .shadow(1.0, 3.0, k.dark_a(0.18))
                        .mark(mk(mat::CONTROL, 0)),
                ),
        )
        .child(icon(&icons::SUN, 18.0).key("bright").color(c.icon_muted))
        .child(text(format!("{:.0}%", v * 100.0)).key("value").w(36.0).font(k.ty.ui).color(c.text_muted).align(Align::End))
}

/// The demo pointer: a black arrow with a white keyline, tip at `p`.
fn pointer(p: [f32; 2]) -> El {
    let arrow = [[0.0, 0.0], [0.0, 16.5], [4.1, 12.7], [6.9, 19.2], [9.4, 18.1], [6.7, 11.8], [12.2, 11.8]];
    let mut ring = arrow.to_vec();
    ring.push(arrow[0]);
    canvas(Shape::new().fill(arrow.to_vec(), Color::BLACK).stroke(ring, 1.3, Color::WHITE))
        .key("demo-pointer")
        .size(14.0, 21.0)
        .absolute(p[0], p[1])
        .mark(mk(mat::NONE, node::CURSOR))
}
