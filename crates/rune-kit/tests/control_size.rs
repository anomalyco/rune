#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]
#![allow(deprecated)]

use rune::{Cx, El, Env, Runtime, col};
use rune_kit::*;

const SIZES: [ControlSize; 5] =
    [ControlSize::Mini, ControlSize::Small, ControlSize::Regular, ControlSize::Large, ControlSize::ExtraLarge];

fn view(_: &(), cx: &mut Cx<()>) -> El {
    let k = Kit::neutral(cx.system_dark());
    let mut c = col().items_start();
    for (i, s) in SIZES.into_iter().enumerate() {
        c = c.child(Button::new("Button").control_size(s).build(&k).key(format!("button-{i}")));
        c = c.child(IconButton::new(k.glyph.copy).control_size(s).build(&k).key(format!("icon-{i}")));
    }
    c.child(Button::new("Button").build(&k).key("default"))
        .child(IconButton::new(k.glyph.copy).build(&k).key("icon-default"))
        .children([ButtonSize::Small, ButtonSize::Normal, ButtonSize::Large].into_iter().enumerate().map(|(i, s)| {
            col().child(Button::new("Button").size(s).build(&k).key(format!("legacy-{i}")))
                .child(IconButton::new(k.glyph.copy).size(s).build(&k).key(format!("legacy-icon-{i}")))
        }))
}

#[test]
fn control_sizes_match_appkit_push_buttons_and_old_sizes_keep_their_metrics() {
    let mut rt = Runtime::new((), view, (400.0, 900.0), Env { system_dark: false, scale: 2.0 });
    rt.frame(0.0);
    let els = rt.elements();
    let size = |key: &str| {
        let e = els.iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{key}"));
        (e.rect[2], e.rect[3])
    };
    for (i, h) in [16.0, 20.0, 24.0, 28.0, 36.0].into_iter().enumerate() {
        assert_eq!(size(&format!("button-{i}")).1, h, "push button {i}");
        assert_eq!(size(&format!("icon-{i}")), (h, h), "icon button {i}");
    }
    assert_eq!(size("default").1, 24.0, "a button is Regular unless sized");
    assert_eq!(size("icon-default"), (24.0, 24.0));
    for (i, (h, side)) in [(24.0, 20.0), (28.0, 24.0), (32.0, 28.0)].into_iter().enumerate() {
        assert_eq!(size(&format!("legacy-{i}")).1, h, "ButtonSize keeps its height");
        assert_eq!(size(&format!("legacy-icon-{i}")), (side, side));
    }
    // Wider padding as the size grows, as AppKit's: 8, 10, 12, 14 and 18 points a side.
    let widths: Vec<f32> = (0..5).map(|i| size(&format!("button-{i}")).0).collect();
    assert!(widths.windows(2).all(|w| w[0] < w[1]), "{widths:?}");
}

fn switches(_: &(), cx: &mut Cx<()>) -> El {
    let k = Kit::neutral(cx.system_dark());
    let mut c = col().items_start();
    for (i, s) in SIZES.into_iter().enumerate() {
        for (j, (on, st)) in [(false, State::Rest), (true, State::Rest), (false, State::Pressed)].into_iter().enumerate() {
            c = c.child(Switch::new(on).control_size(s).state(st).build(&k).key(format!("switch-{i}-{j}")));
        }
    }
    c
}

#[test]
fn switches_have_appkit_capsule_tracks_and_pill_knobs() {
    let mut rt = Runtime::new((), switches, (400.0, 900.0), Env { system_dark: false, scale: 2.0 });
    rt.frame(0.0);
    rt.settle_all();
    rt.frame(0.0);
    let els = rt.elements();
    let parts = |key: &str| {
        let i = els.iter().position(|e| e.key.as_deref() == Some(key)).unwrap();
        let under = |k: &str| els.iter().find(|e| e.key.as_deref() == Some(k) && e.path.starts_with(&els[i].path)).unwrap();
        (under("control").clone(), under("thumb").clone())
    };
    let measured = [(36.0, 16.0, 21.0, 14.0, 1.0), (44.0, 20.0, 26.0, 16.0, 2.0), (54.0, 24.0, 32.0, 20.0, 2.0),
        (64.0, 28.0, 38.0, 24.0, 2.0), (80.0, 36.0, 48.0, 30.0, 3.0)];
    for (i, (tw, th, kw, kh, inset)) in measured.into_iter().enumerate() {
        let (track, off) = parts(&format!("switch-{i}-0"));
        assert_eq!((track.rect[2], track.rect[3], track.radius), (tw, th, th / 2.0), "track {i}");
        assert_eq!((off.rect[2], off.rect[3], off.radius), (kw, kh, kh / 2.0), "knob {i}");
        assert_eq!((off.rect[0] - track.rect[0], off.rect[1] - track.rect[1]), (inset, inset), "inset {i}");
        let (track, on) = parts(&format!("switch-{i}-1"));
        assert_eq!(track.rect[0] + tw - (on.rect[0] + on.rect[2]), inset, "on sits at the far end {i}");
        let (_, pressed) = parts(&format!("switch-{i}-2"));
        assert!(pressed.rect[2] > kw && pressed.rect[3] == kh, "a pressed knob stretches {i}");
    }
}
