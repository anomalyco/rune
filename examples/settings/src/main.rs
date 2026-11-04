//! A small Settings window rendered entirely by Rune.

use rune::tok::{self, Theme, motion, radius, space, type_};
#[cfg(feature = "frame-profile")]
#[global_allocator]
static ALLOC: procwatch::alloc::Counting = procwatch::alloc::Counting::system();
use rune::{Align, AppSpec, Color, Cx, Drag, El, Phase, Spring, col, el, row, text};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Appearance {
    Auto,
    Light,
    Dark,
}

struct Settings {
    appearance: Appearance,
    accent: usize,
    launch_at_login: bool,
    menu_bar: bool,
    sounds: bool,
    /// Slider position 0..1; animation speed is 2^(2v − 1), so the middle is 1×.
    speed: f32,
    reduce_motion: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            appearance: Appearance::Auto,
            accent: 0,
            launch_at_login: true,
            menu_bar: true,
            sounds: false,
            speed: 0.5,
            reduce_motion: false,
        }
    }
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

/// Everything a component needs to paint itself.
#[derive(Clone, Copy)]
struct Paint {
    t: Theme,
    accent: Color,
    hair: f32,
}

fn view(s: &Settings, cx: &mut Cx<Settings>) -> El {
    let dark = match s.appearance {
        Appearance::Auto => cx.system_dark(),
        Appearance::Light => false,
        Appearance::Dark => true,
    };
    cx.reduced_motion(s.reduce_motion);
    cx.time_scale(speed_of(s.speed));
    let p = Paint { t: tok::theme(dark), accent: tok::accent(s.accent, dark), hair: 1.0 / cx.env().scale };
    let t = p.t;

    col()
        .key("window")
        .role("window")
        .fill(t.bg)
        .child(
            row()
                .key("titlebar")
                .h(40.0)
                .w_full()
                .center()
                .child(text("Settings").key("title").font(type_::TITLE).color(t.text)),
        )
        .child(
            col()
                .key("content")
                .grow(1.0)
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
                        setting_row(p, "speed", "Animation speed", None, speed_control(p, s.speed)),
                        toggle_row(p, "reduce", "Reduce motion", s.reduce_motion, |s| {
                            s.reduce_motion = !s.reduce_motion
                        }),
                    ],
                )),
        )
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

const APPEARANCES: [(Appearance, &str); 3] =
    [(Appearance::Auto, "Auto"), (Appearance::Light, "Light"), (Appearance::Dark, "Dark")];

fn segmented(p: Paint, current: Appearance) -> El {
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
                    s.appearance = a;
                    tx.color_motion(motion::CALM);
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

fn speed_control(p: Paint, v: f32) -> El {
    let (w, h, k, bar) = (180.0, 22.0, 18.0, 4.0);
    let travel = w - k;
    let to_value = move |x: f32| ((x - k / 2.0) / travel).clamp(0.0, 1.0);
    row()
        .key("speed-control")
        .gap(space::S3)
        .center_y()
        .child(
            el().key("slider")
                .role("slider")
                .label("Animation speed")
                // The slider's position, in twentieths; it reads as the speed it gives.
                .range_value(f64::from(v), 0.0, 1.0)
                .range_step(0.05)
                .a11y_value(format!("{:.2}×", speed_of(v)))
                .on_step(|s: &mut Settings, steps, _| s.speed = (s.speed + steps as f32 * 0.05).clamp(0.0, 1.0))
                .size(w, h)
                .motion(motion::SNAPPY)
                .on_drag(move |s: &mut Settings, d: &mut Drag, tx| match d.phase {
                    Phase::Down => {
                        d.data = s.speed;
                        let knob = k / 2.0 + v * travel;
                        if (d.x - knob).abs() > k / 2.0 + 2.0 {
                            // Clicked the track: the knob animates there.
                            s.speed = to_value(d.x);
                        }
                    }
                    Phase::Move => {
                        let knob = k / 2.0 + d.data * travel;
                        let grab = if (d.start_x - knob).abs() <= k / 2.0 + 2.0 { d.start_x - knob } else { 0.0 };
                        s.speed = to_value(d.x - grab);
                        tx.instant();
                    }
                    Phase::Up => { if d.cancelled() { s.speed = d.data; } }
                })
                .child(
                    el().key("track")
                        .size(w, bar)
                        .absolute(0.0, (h - bar) / 2.0)
                        .radius(bar / 2.0)
                        .fill(p.t.control_off)
                        .a11y_hidden(),
                )
                .child(
                    el().key("fill")
                        .size(k / 2.0 + v * travel, bar)
                        .absolute(0.0, (h - bar) / 2.0)
                        .radius(bar / 2.0)
                        .fill(p.accent)
                        .a11y_hidden(),
                )
                .child(
                    el().key("knob")
                        .size(k, k)
                        .absolute(v * travel, (h - k) / 2.0)
                        .radius(k / 2.0)
                        .fill(p.t.knob)
                        .shadow(1.0, 3.0, p.t.knob_shadow)
                        .shadow(0.0, 0.5, p.t.edge)
                        .a11y_hidden(),
                ),
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

fn main() {
    rune::run(AppSpec::<Settings> {
        scenes: &[
            ("default", |_| {}),
            ("light", |s| s.appearance = Appearance::Light),
            ("dark", |s| s.appearance = Appearance::Dark),
            ("reduced-motion", |s| s.reduce_motion = true),
            ("toggles-off", |s| {
                s.launch_at_login = false;
                s.menu_bar = false;
                s.sounds = false;
            }),
            ("accent-pink", |s| s.accent = 2),
            ("slow", |s| s.speed = 0.0),
        ],
        ..AppSpec::new("Settings", (620.0, 520.0), Settings::default, view)
    });
}
