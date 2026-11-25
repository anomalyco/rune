//! A small Settings window rendered entirely by Rune.

#![forbid(unsafe_code)]

use rune::tok::{self, Theme, motion, radius, space, type_};
#[cfg(feature = "frame-profile")]
#[global_allocator]
static ALLOC: procwatch::alloc::Counting = procwatch::alloc::Counting::system();
use rune::{Align, App, AppHandle, AppSpec, Color, ColorScheme, Cx, El, MacColorSpace, MacWindowOptions, MenuItem, ScrollSpec, Spring, Tx, col, el, row, text};

const DEFAULT_FONT_ID: &str = "__default__";

struct Settings {
    appearance: ColorScheme,
    app_handle: Option<AppHandle<Settings>>,
    accent: usize,
    launch_at_login: bool,
    menu_bar: bool,
    sounds: bool,
    /// Slider position 0..1; animation speed is 2^(2v − 1), so the middle is 1×.
    speed: f32,
    reduce_motion: bool,
    sans_font: Option<String>,
    mono_font: Option<String>,
    sans_picker: rune_kit::ComboboxState,
    mono_picker: rune_kit::ComboboxState,
    about_open: bool,
    quit_confirm: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            appearance: ColorScheme::System,
            app_handle: None,
            accent: 0,
            launch_at_login: true,
            menu_bar: true,
            sounds: false,
            speed: 0.5,
            reduce_motion: false,
            sans_font: None,
            mono_font: None,
            sans_picker: rune_kit::ComboboxState::default(),
            mono_picker: rune_kit::ComboboxState::default(),
            about_open: false,
            quit_confirm: false,
        }
    }
}

impl Settings {
    fn apply_fonts(&self, tx: &mut Tx) {
        let quote = |name: &str| format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""));
        let sans = match &self.sans_font {
            Some(name) => format!("{}, Inter", quote(name)),
            None => "Inter".into(),
        };
        let mono = match &self.mono_font {
            Some(name) => format!("{}, ui-monospace", quote(name)),
            None => "ui-monospace".into(),
        };
        tx.set_font_families(sans, mono);
    }
}

fn apply_color_scheme(s: &mut Settings, scheme: ColorScheme, tx: &mut Tx) {
    s.appearance = scheme;
    tx.set_color_scheme(scheme);
    if let Some(handle) = &s.app_handle {
        handle.set_color_scheme(scheme);
    }
    tx.color_motion(motion::CALM);
}

fn speed_of(v: f32) -> f32 {
    2f32.powf(v * 2.0 - 1.0)
}

#[test]
fn cancelling_the_speed_slider_restores_its_value_before_a_track_press() {
    let mut rt = rune::Runtime::new(Settings::default(), view, (620.0, 520.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    let (x, y) = rt.find("slider").unwrap();
    rt.pointer_down(x + 60.0, y);
    rt.pointer_move(x + 70.0, y);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.speed > 0.5);
    rt.key(rune::KeyEvent { key: rune::Key::Escape, mods: rune::Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.speed, 0.5);
    rt.pointer_up(x + 70.0, y);
    rt.frame(0.0);
    assert_eq!(rt.state.speed, 0.5);
}

#[test]
fn about_and_quit_confirm_sheets_open_and_dismiss_on_escape() {
    let mut rt = rune::Runtime::new(Settings::default(), view, (620.0, 520.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(rt.find("about-dialog").is_none());
    assert!(rt.find("quit-dialog").is_none());

    rt.update(|s, _| s.about_open = true);
    rt.frame(0.0);
    assert!(rt.find("about-dialog").is_some());
    rt.key(rune::KeyEvent { key: rune::Key::Escape, mods: rune::Mods::default() });
    rt.frame(0.0);
    assert!(!rt.state.about_open);

    rt.update(|s, _| s.quit_confirm = true);
    rt.frame(0.0);
    assert!(rt.find("quit-dialog").is_some());
    rt.key(rune::KeyEvent { key: rune::Key::Escape, mods: rune::Mods::default() });
    rt.frame(0.0);
    assert!(!rt.state.quit_confirm);
}

#[test]
fn font_picker_lists_installed_families_and_updates_slot() {
    let mut rt = rune::Runtime::new(Settings::default(), view, (620.0, 640.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(!rt.state.mono_picker.is_open());

    rt.update(|s, _| {
        s.mono_picker.step(rune_kit::ComboboxEvent::Toggle { current: Some(DEFAULT_FONT_ID) });
    });
    rt.frame(0.0);
    assert!(rt.state.mono_picker.is_open());

    if let Some(mono) = rune::text::installed_families().iter().find(|f| f.monospace()) {
        let name = mono.name().to_owned();
        rt.update(|s, tx| {
            s.mono_font = Some(name.clone());
            s.mono_picker.close();
            s.apply_fonts(tx);
        });
        rt.frame(0.0);
        assert_eq!(rt.state.mono_font.as_deref(), Some(mono.name()));
        assert!(!rt.state.mono_picker.is_open());
    }
}

#[test]
fn titlebar_reflects_window_maximized_and_full_screen_state() {
    let mut rt = rune::Runtime::new(Settings::default(), view, (620.0, 640.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    use rune::accesskit::Toggled;
    let node_state = |rt: &mut rune::Runtime<Settings>, key: &str| {
        let tree = rt.accessibility_tree();
        tree.nodes
            .iter()
            .find(|(_, node)| node.author_id() == Some(key))
            .map(|(_, node)| (node.label().map(str::to_owned), node.toggled()))
    };
    assert_eq!(node_state(&mut rt, "window-state"), Some((Some("Window state: Windowed".into()), None)));
    assert_eq!(node_state(&mut rt, "window-maximize"), Some((Some("Maximize".into()), Some(Toggled::False))));
    assert_eq!(node_state(&mut rt, "window-full-screen"), Some((Some("Full screen".into()), Some(Toggled::False))));

    rt.set_maximized(true);
    rt.frame(1.0);
    assert_eq!(node_state(&mut rt, "window-state"), Some((Some("Window state: Maximized".into()), None)));
    assert_eq!(node_state(&mut rt, "window-maximize"), Some((Some("Restore".into()), Some(Toggled::True))));

    rt.set_full_screen(true);
    rt.frame(2.0);
    assert_eq!(node_state(&mut rt, "window-state"), Some((Some("Window state: Full screen".into()), None)));
    assert_eq!(node_state(&mut rt, "window-full-screen"), Some((Some("Exit full screen".into()), Some(Toggled::True))));

    rt.set_maximized(false);
    rt.set_full_screen(false);
    rt.frame(3.0);
    assert_eq!(node_state(&mut rt, "window-state"), Some((Some("Window state: Windowed".into()), None)));
    assert_eq!(node_state(&mut rt, "window-maximize"), Some((Some("Maximize".into()), Some(Toggled::False))));
    assert_eq!(node_state(&mut rt, "window-full-screen"), Some((Some("Full screen".into()), Some(Toggled::False))));
}

/// Everything a component needs to paint itself.
#[derive(Clone, Copy)]
struct Paint {
    t: Theme,
    accent: Color,
    hair: f32,
}

fn view(s: &Settings, cx: &mut Cx<Settings>) -> El {
    let dark = match s.appearance {
        ColorScheme::System => cx.system_dark(),
        ColorScheme::Light => false,
        ColorScheme::Dark => true,
    };
    cx.reduced_motion(s.reduce_motion);
    cx.time_scale(speed_of(s.speed));
    let p = Paint { t: tok::theme(dark), accent: tok::accent(s.accent, dark), hair: 1.0 / cx.env().scale };
    let t = p.t;
    let mut k = rune_kit::Kit::neutral(dark);
    k.c.bg_accent = p.accent;
    k.c.border_focus = p.accent;
    let mut overlays = Vec::new();
    let sans_control = font_picker(
        &k,
        &mut overlays,
        "sans-font",
        "Interface font",
        "Default (Inter)",
        s.sans_font.as_deref(),
        &s.sans_picker,
        false,
        |s| &mut s.sans_picker,
        |s, chosen, tx| {
            s.sans_font = (chosen != DEFAULT_FONT_ID).then(|| chosen.to_owned());
            s.apply_fonts(tx);
        },
    );
    let mono_control = font_picker(
        &k,
        &mut overlays,
        "mono-font",
        "Monospace font",
        "Default (System)",
        s.mono_font.as_deref(),
        &s.mono_picker,
        true,
        |s| &mut s.mono_picker,
        |s, chosen, tx| {
            s.mono_font = (chosen != DEFAULT_FONT_ID).then(|| chosen.to_owned());
            s.apply_fonts(tx);
        },
    );

    let window = col()
        .key("window")
        .role("window")
        .fill(t.bg)
        .child(titlebar(p, cx.chrome_insets(), cx.is_maximized(), cx.is_full_screen()))
        .child(
            col()
                .key("content")
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec::default())
                .px(space::S6)
                .pt(space::S1)
                .pb(space::S6)
                .gap(space::S6)
                .clip_fade(0.0, space::S1, 0.0, space::S6)
                .child(section(
                    p,
                    "appearance",
                    "Appearance",
                    [
                        setting_row(p, "theme", "Theme", None, segmented(p, s.appearance)),
                        setting_row(p, "accent", "Accent colour", None, swatches(p, s.accent, dark)),
                    ],
                ))
                .child(section(
                    p,
                    "general",
                    "General",
                    [
                        toggle_row(p, "login", "Launch at login", s.launch_at_login, |s| {
                            s.launch_at_login = !s.launch_at_login
                        }),
                        toggle_row(p, "menubar", "Show in menu bar", s.menu_bar, |s| s.menu_bar = !s.menu_bar),
                        toggle_row(p, "sounds", "Play sounds", s.sounds, |s| s.sounds = !s.sounds),
                    ],
                ))
                .child(section(
                    p,
                    "motion",
                    "Motion",
                    [
                        setting_row(p, "speed", "Animation speed", None, speed_control(p, dark, s.speed)),
                        toggle_row(p, "reduce", "Reduce motion", s.reduce_motion, |s| {
                            s.reduce_motion = !s.reduce_motion
                        }),
                    ],
                ))
                .child(section(
                    p,
                    "fonts",
                    "Fonts",
                    [
                        setting_row(p, "sans-font-row", "Interface font", None, sans_control),
                        setting_row(p, "mono-font-row", "Monospace font", None, mono_control),
                    ],
                )),
        )
        .children(s.about_open.then(|| modal_sheet(
            p,
            "about-dialog",
            "About Settings",
            "Custom macOS app-menu About item (MenuItem::about), replacing AppKit's standard About panel.",
            row().key("actions").justify_end().child(sheet_button(p, "about-done", "Done", true, |s, _| s.about_open = false)),
            |s| s.about_open = false,
        )))
        .children(s.quit_confirm.then(|| modal_sheet(
            p,
            "quit-dialog",
            "Quit Settings?",
            "⌘Q is claimed by this app's Close Settings menu item; the built-in Quit item yields its shortcut.",
            row().key("actions").justify_end().gap(space::S2)
                .child(sheet_button(p, "quit-cancel", "Cancel", false, |s, _| s.quit_confirm = false))
                .child(sheet_button(p, "quit-confirm", "Quit", true, |_, tx| tx.quit())),
            |s| s.quit_confirm = false,
        )));

    rune_kit::root(window, overlays)
}

#[allow(clippy::too_many_arguments)]
fn font_picker(
    k: &rune_kit::Kit,
    overlays: &mut Vec<El>,
    key: &'static str,
    label: &'static str,
    default_label: &'static str,
    current: Option<&str>,
    state: &rune_kit::ComboboxState,
    mono_only: bool,
    access: fn(&mut Settings) -> &mut rune_kit::ComboboxState,
    choose: fn(&mut Settings, &str, &mut rune::Tx),
) -> El {
    let current_id = current.unwrap_or(DEFAULT_FONT_ID);
    // Avoid enumerating system fonts during first-frame launch; query lazily once opened.
    let options = if state.is_open() {
        let mut opts = Vec::new();
        opts.push(rune_kit::ComboboxOption::new(DEFAULT_FONT_ID, default_label));
        for fam in rune::text::installed_families() {
            if mono_only && !fam.monospace() {
                continue;
            }
            let mut opt = rune_kit::ComboboxOption::new(fam.name(), fam.name());
            let n = fam.weights().len();
            let weights = if n == 1 { "1 weight".to_owned() } else { format!("{n} weights") };
            opt.detail = if !mono_only && fam.monospace() {
                format!("Mono · {weights}")
            } else {
                weights
            };
            opts.push(opt);
        }
        opts
    } else {
        vec![rune_kit::ComboboxOption::new(current_id, current.unwrap_or(default_label))]
    };
    rune_kit::Combobox::new(key, state, &options, access)
        .label(label)
        .current(Some(current_id))
        .placeholder(default_label)
        .search_placeholder("Search installed fonts…")
        .width(220.0)
        .placement(rune_kit::Placement::TopEnd)
        .on_choose(choose)
        .build(k, overlays)
}

fn titlebar(p: Paint, insets: rune::ChromeInsets, maximized: bool, full_screen: bool) -> El {
    let status = if full_screen {
        "Full screen"
    } else if maximized {
        "Maximized"
    } else {
        "Windowed"
    };
    let max_label = if maximized { "Restore" } else { "Maximize" };
    let fs_label = if full_screen { "Exit full screen" } else { "Full screen" };
    let leading = insets.leading.max(space::S3);
    let trailing = insets.trailing.max(space::S3);
    let state_pill = el()
        .key("window-state")
        .role("status")
        .label(format!("Window state: {status}"))
        .h(22.0)
        .px(space::S2)
        .center()
        .radius(radius::SM)
        .fill(if full_screen || maximized { p.accent.alpha(0.16) } else { p.t.control_track })
        .child(
            text(status)
                .key("label")
                .font(type_::LABEL)
                .color(if full_screen || maximized { p.accent } else { p.t.text_secondary })
                .a11y_hidden(),
        );
    let max_button = titlebar_button(p, "window-maximize", max_label, maximized, |_, tx| {
        tx.toggle_maximized();
    });
    let fs_button = titlebar_button(
        p,
        "window-full-screen",
        fs_label,
        full_screen,
        if full_screen {
            |_, tx| tx.set_full_screen(false)
        } else {
            |_, tx| tx.set_full_screen(true)
        },
    );
    row()
        .key("titlebar")
        .h(40.0)
        .w_full()
        .pl(leading)
        .pr(trailing)
        .center_y()
        .justify_between()
        .window_drag()
        .child(row().key("titlebar-left").center_y().gap(space::S2).child(state_pill))
        .child(text("Settings").key("title").font(type_::TITLE).color(p.t.text))
        .child(
            row()
                .key("titlebar-actions")
                .center_y()
                .gap(space::S1)
                .child(max_button)
                .child(fs_button),
        )
}

fn titlebar_button(
    p: Paint,
    key: &'static str,
    label: &'static str,
    active: bool,
    action: fn(&mut Settings, &mut rune::Tx),
) -> El {
    el().key(key)
        .role("button")
        .label(label)
        .checked(active)
        .focusable(true)
        .h(24.0)
        .px(space::S2)
        .center()
        .radius(radius::SM)
        .fill(if active { p.accent.alpha(0.18) } else { p.t.control_track })
        .hover(p.t.hover)
        .press(p.t.press)
        .on_press_tx(action)
        .child(
            text(label)
                .key("label")
                .font(type_::LABEL)
                .color(if active { p.accent } else { p.t.text })
                .a11y_hidden(),
        )
}

fn modal_sheet(
    p: Paint,
    key: &'static str,
    title: &'static str,
    body: &'static str,
    actions: El,
    dismiss: fn(&mut Settings),
) -> El {
    el().key("backdrop")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .center()
        .fill(Color::BLACK.alpha(0.32))
        .child(
            col().key(key)
                .role("dialog")
                .label(title)
                .modal(true, dismiss)
                .w(360.0)
                .padding(space::S6)
                .gap(space::S3)
                .fill(p.t.card)
                .radius(radius::LG)
                .border(p.hair, p.t.card_border)
                .shadow(8.0, 28.0, p.t.shadow)
                .child(text(title).key("title").font(type_::TITLE).color(p.t.text))
                .child(text(body).key("body").w_full().wrap().font(type_::BODY).color(p.t.text_secondary))
                .child(actions),
        )
}

fn sheet_button(
    p: Paint,
    key: &'static str,
    label: &'static str,
    primary: bool,
    action: fn(&mut Settings, &mut rune::Tx),
) -> El {
    el().key(key)
        .role("button")
        .label(label)
        .focusable(true)
        .h(28.0)
        .px(space::S3)
        .center()
        .radius(radius::MD)
        .fill(if primary { p.accent } else { p.t.control_track })
        .hover(if primary { p.accent.alpha(0.88) } else { p.t.hover })
        .press(p.t.press)
        .on_press_tx(action)
        .child(text(label).key("label").font(type_::LABEL).color(if primary { Color::WHITE } else { p.t.text }).a11y_hidden())
}

fn section<const N: usize>(p: Paint, key: &str, title: &str, rows: [El; N]) -> El {
    let mut card = col()
        .key("card")
        .padding(space::S1)
        .fill(p.t.card)
        .radius(radius::LG)
        .border(p.hair, p.t.card_border)
        .shadow(1.0, 2.0, p.t.shadow)
        .shadow(8.0, 24.0, p.t.shadow_soft);
    for (i, r) in rows.into_iter().enumerate() {
        card = card.child(r);
        if i + 1 < N {
            card = card.child(el().key(format!("sep{i}")).h(p.hair).ml(space::S3).mr(space::S3).fill(p.t.separator).a11y_hidden());
        }
    }
    col()
        .key(key)
        .gap(space::S2)
        .child(text(title).key("header").font(type_::SECTION).color(p.t.text_secondary).ml(space::S3))
        .child(card)
}

fn setting_row(p: Paint, key: &str, label: &str, on_press: Option<fn(&mut Settings)>, control: El) -> El {
    let mut r = row()
        .key(key)
        .h(44.0)
        .px(space::S3)
        .center_y()
        .justify_between()
        .radius(radius::MD)
        .label(label)
        .child(text(label).key("label").font(type_::BODY).color(p.t.text).a11y_hidden())
        .child(control);
    if let Some(f) = on_press {
        r = r.hover(p.t.hover).press(p.t.press).focusable(true).on_press(f);
    }
    r
}

fn toggle_row(p: Paint, key: &str, label: &str, on: bool, flip: fn(&mut Settings)) -> El {
    setting_row(p, key, label, Some(flip), toggle(p, on).a11y_hidden()).role("switch").checked(on)
}

fn toggle(p: Paint, on: bool) -> El {
    let (w, h, k) = (38.0, 22.0, 18.0);
    let inset = (h - k) / 2.0;
    el().key("switch")
        .role("switch")
        .size(w, h)
        .radius(h / 2.0)
        .fill(if on { p.accent } else { p.t.control_off })
        .motion(motion::SNAPPY)
        .child(
            el().key("knob")
                .size(k, k)
                .absolute(if on { w - k - inset } else { inset }, inset)
                .radius(k / 2.0)
                .fill(p.t.knob)
                .shadow(1.0, 3.0, p.t.knob_shadow)
                .shadow(0.0, 0.5, p.t.edge),
        )
}

const APPEARANCES: [(ColorScheme, &str); 3] =
    [(ColorScheme::System, "System"), (ColorScheme::Light, "Light"), (ColorScheme::Dark, "Dark")];

fn segmented(p: Paint, current: ColorScheme) -> El {
    let (w, h, pad) = (205.0, 28.0, 2.0);
    let seg = (w - 2.0 * pad) / 3.0;
    let idx = APPEARANCES.iter().position(|(a, _)| *a == current).unwrap_or(0);
    let mut segs = row().key("segments");
    for (i, (a, name)) in APPEARANCES.into_iter().enumerate() {
        let selected = i == idx;
        segs = segs.child(
            el().key(name)
                .role("segment")
                .label(name)
                .checked(selected)
                .focusable(true)
                .size(seg, h - 2.0 * pad)
                .center()
                .radius(radius::SM)
                .hover(if selected { Color::TRANSPARENT } else { p.t.hover })
                .on_press_tx(move |s: &mut Settings, tx| {
                    apply_color_scheme(s, a, tx);
                })
                .child(
                    text(name)
                        .key("label")
                        .font(type_::LABEL)
                        .color(if selected { p.t.text } else { p.t.text_secondary })
                        .a11y_hidden(),
                ),
        );
    }
    el().key("segmented")
        .role("segmented")
        .size(w, h)
        .padding(pad)
        .radius(radius::MD)
        .fill(p.t.control_track)
        .motion(motion::SNAPPY)
        .child(
            el().key("indicator")
                .size(seg, h - 2.0 * pad)
                .absolute(pad + idx as f32 * seg, pad)
                .radius(radius::SM)
                .fill(p.t.indicator)
                .shadow(1.0, 3.0, p.t.shadow)
                .shadow(0.0, 0.5, p.t.edge)
                .lead(motion::LEAD, motion::DEFAULT)
                .a11y_hidden(),
        )
        .child(segs)
}

fn swatches(p: Paint, current: usize, dark: bool) -> El {
    let (d, gap, ring_w) = (18.0, 12.0, 2.0);
    let ring = d + 2.0 * (ring_w + 2.0);
    let mut r = row().key("swatches").role("radiogroup").label("Accent colour").gap(gap).motion(motion::SNAPPY);
    r = r.child(
        el().key("ring")
            .size(ring, ring)
            .absolute(current as f32 * (d + gap) - (ring - d) / 2.0, -(ring - d) / 2.0)
            .radius(ring / 2.0)
            .border(ring_w, p.accent)
            .lead(Spring::new(0.22, 0.0), Spring::new(0.28, 0.0))
            .a11y_hidden(),
    );
    for i in 0..6 {
        r = r.child(
            el().key(tok::ACCENT_NAMES[i])
                .role("radio")
                .label(tok::ACCENT_NAMES[i])
                .checked(i == current)
                .focusable(true)
                .size(d, d)
                .radius(d / 2.0)
                .fill(tok::accent(i, dark))
                .border(p.hair, Color::BLACK.alpha(if dark { 0.0 } else { 0.08 }))
                .on_press(move |s: &mut Settings| s.accent = i),
        );
    }
    r
}

fn speed_control(p: Paint, dark: bool, v: f32) -> El {
    let mut k = rune_kit::Kit::neutral(dark);
    k.c.bg_accent = p.accent;
    k.c.border_focus = p.accent;
    row()
        .key("speed-control")
        .gap(space::S3)
        .center_y()
        .child(
            rune_kit::Slider::new("slider", v)
                .range(0.0..=1.0)
                .label("Animation speed")
                .hide_label()
                .value_text(format!("{:.2}×", speed_of(v)))
                .width(180.0)
                .on_change(|s: &mut Settings, v, _| s.speed = v)
                .build(&k),
        )
        .child(
            text(format!("{:.2}×", speed_of(v)))
                .key("value")
                .w(40.0)
                .font(type_::MONO_CAPTION)
                .color(p.t.text_secondary)
                .align(Align::End)
                .a11y_hidden(),
        )
}

fn accent_title(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

fn native_menu_items() -> Vec<MenuItem<Settings>> {
    let appearance_items = APPEARANCES.into_iter().map(|(appearance, name)| {
        MenuItem::with_action(name, move |s: &mut Settings, tx| {
            apply_color_scheme(s, appearance, tx);
        })
        .id(format!("appearance-{}", name.to_ascii_lowercase()))
    });

    let accent_items = tok::ACCENT_NAMES.iter().enumerate().map(|(index, &name)| {
        MenuItem::with_action(accent_title(name), move |s: &mut Settings, _| {
            s.accent = index;
        })
        .id(format!("accent-{name}"))
    });

    let speed_presets: [(&str, &str, f32); 3] = [
        ("speed-slow", "0.5× (Slow)", 0.0),
        ("speed-normal", "1.0× (Normal)", 0.5),
        ("speed-fast", "2.0× (Fast)", 1.0),
    ];
    let speed_items = speed_presets.into_iter().map(|(id, title, value)| {
        MenuItem::with_action(title, move |s: &mut Settings, _| {
            s.speed = value;
        })
        .id(id)
    });

    let mut appearance_children: Vec<MenuItem<Settings>> = appearance_items.collect();
    appearance_children.push(MenuItem::separator());
    appearance_children.push(MenuItem::submenu("Accent Colour", accent_items).id("accent-submenu"));

    vec![
        MenuItem::about("About Settings", |s: &mut Settings, _| {
            s.quit_confirm = false;
            s.about_open = true;
        })
        .id("about"),
        MenuItem::new("Close Settings…", |s: &mut Settings, tx| {
            if s.quit_confirm {
                tx.quit();
            } else {
                s.about_open = false;
                s.quit_confirm = true;
            }
        })
        .id("close-settings")
        .key_equivalent("cmd+q".parse().expect("valid cmd+q shortcut")),
        MenuItem::submenu("Appearance", appearance_children)
            .id("appearance-submenu")
            .menu("View"),
        MenuItem::submenu("Animation Speed", speed_items)
            .id("speed-submenu")
            .menu("View"),
    ]
}

fn parse_mac_color_space(value: Option<&str>) -> MacColorSpace {
    if value.is_some_and(|v| v.trim().eq_ignore_ascii_case("unmanaged")) { MacColorSpace::Unmanaged } else { MacColorSpace::Srgb }
}

fn main() {
    let spec = AppSpec::<Settings> {
        scenes: &[
            ("default", |_| {}),
            ("light", |s| s.appearance = ColorScheme::Light),
            ("dark", |s| s.appearance = ColorScheme::Dark),
            ("reduced-motion", |s| s.reduce_motion = true),
            ("toggles-off", |s| {
                s.launch_at_login = false;
                s.menu_bar = false;
                s.sounds = false;
            }),
            ("accent-pink", |s| s.accent = 2),
            ("slow", |s| s.speed = 0.0),
            ("fonts", |s| {
                s.mono_picker.step(rune_kit::ComboboxEvent::Toggle { current: Some(DEFAULT_FONT_ID) });
            }),
            ("about", |s| s.about_open = true),
            ("quit-confirm", |s| s.quit_confirm = true),
        ],
        ..AppSpec::new("Settings", (620.0, 640.0), Settings::default, view)
    };
    let color_space = parse_mac_color_space(rune::env::var("SETTINGS_MAC_COLOR_SPACE").as_deref());
    let base = App::new(spec).color_scheme(ColorScheme::System)
        .macos_window(MacWindowOptions::default().color_space(color_space));
    let app = native_menu_items().into_iter().fold(base, App::menu_item);
    let handle = app.handle();
    app.init(move || Settings {
        app_handle: Some(handle),
        ..Settings::default()
    })
    .run();
}

#[test]
fn appearance_picker_switches_color_scheme_and_updates_accessibility_state() {
    let mut rt = rune::Runtime::new(Settings::default(), view, (620.0, 520.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.state.appearance, ColorScheme::System);

    for (label, expected) in [
        ("Light", ColorScheme::Light),
        ("Dark", ColorScheme::Dark),
        ("System", ColorScheme::System),
    ] {
        let (x, y) = rt.find(label).expect("appearance segment");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert_eq!(rt.state.appearance, expected);
        let tree = rt.accessibility_tree();
        let selected = tree.nodes.iter().find(|(_, n)| n.label() == Some(label)).expect("segment a11y node");
        assert_eq!(selected.1.toggled(), Some(rune::accesskit::Toggled::True));
    }
}

#[cfg(test)]
#[test]
fn mac_color_space_env_parser_defaults_to_srgb_and_accepts_unmanaged() {
    assert_eq!(parse_mac_color_space(None), MacColorSpace::Srgb);
    assert_eq!(parse_mac_color_space(Some("")), MacColorSpace::Srgb);
    assert_eq!(parse_mac_color_space(Some("srgb")), MacColorSpace::Srgb);
    assert_eq!(parse_mac_color_space(Some("unmanaged")), MacColorSpace::Unmanaged);
    assert_eq!(parse_mac_color_space(Some(" UNMANAGED ")), MacColorSpace::Unmanaged);
}
