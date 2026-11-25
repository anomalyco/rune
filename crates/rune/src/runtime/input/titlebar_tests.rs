#[cfg(target_os = "windows")]
mod windows {
    use std::cell::Cell;

    use crate::el::el;
    use crate::runtime::Runtime;
    use crate::{Chrome, ChromeInsets, Cx, El, Env, Titlebar};

    #[derive(Default)]
    struct State {
        seen: Cell<ChromeInsets>,
    }

    fn view(state: &State, cx: &mut Cx<State>) -> El {
        state.seen.set(cx.chrome_insets());
        el().key("root").w_full().h_full()
    }

    fn caption_bounds<S: 'static>(rt: &Runtime<S>) -> [f32; 4] {
        let i = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some(crate::titlebar::BUTTONS_KEY))
            .expect("rune-caption node");
        rt.drawn_box(i)
    }

    #[test]
    fn headless_chrome_reserves_rtl_caption_space_on_the_left() {
        let width = 400.0;
        for scale in [1.0, 1.25] {
            let caption_w = crate::titlebar::Caption::new(36.0).width(scale);

            // LTR: caption sits on the right and reserves trailing space.
            let mut rt = Runtime::new(State::default(), view, (width, 200.0), Env::new(true, scale));
            rt.core_mut().headless_chrome_with(Chrome::DEFAULT, Some(Titlebar::new(36.0)), false, false);
            rt.frame(0.0);
            let expected_ltr = ChromeInsets::new(0.0, caption_w, 36.0);
            assert_eq!(rt.chrome_insets(), expected_ltr, "scale {scale} LTR runtime insets");
            assert_eq!(rt.state.seen.get(), expected_ltr, "scale {scale} LTR view insets");
            let bounds = caption_bounds(&rt);
            assert!((bounds[0] - (width - caption_w)).abs() < 1e-3, "scale {scale} LTR caption left: {bounds:?}");
            assert!((bounds[2] - width).abs() < 1e-3, "scale {scale} LTR caption right: {bounds:?}");

            // RTL: caption mirrors to the left and reserves leading space.
            let mut rt = Runtime::new(State::default(), view, (width, 200.0), Env::new(true, scale));
            rt.core_mut().headless_chrome_with(Chrome::DEFAULT, Some(Titlebar::new(36.0)), false, true);
            rt.frame(0.0);
            let expected_rtl = ChromeInsets::new(caption_w, 0.0, 36.0);
            assert_eq!(rt.chrome_insets(), expected_rtl, "scale {scale} RTL runtime insets");
            assert_eq!(rt.state.seen.get(), expected_rtl, "scale {scale} RTL view insets");
            let bounds = caption_bounds(&rt);
            assert!((bounds[0] - 0.0).abs() < 1e-3, "scale {scale} RTL caption left: {bounds:?}");
            assert!((bounds[2] - caption_w).abs() < 1e-3, "scale {scale} RTL caption right: {bounds:?}");

            // Borderless ignores the titlebar in RTL too.
            rt.core_mut().headless_chrome_with(Chrome::DEFAULT, Some(Titlebar::new(36.0)), true, true);
            assert_eq!(rt.chrome_insets(), ChromeInsets::default(), "scale {scale} RTL borderless");
        }
    }

    #[test]
    fn headless_chrome_caption_buttons_follow_set_labels() {
        let _guard = crate::labels::test_lock();
        crate::set_labels(|_| None);

        let mut rt = Runtime::new(State::default(), view, (400.0, 200.0), Env::new(true, 1.0));
        rt.core_mut().headless_chrome_with(Chrome::DEFAULT, Some(Titlebar::new(36.0)), false, false);
        rt.frame(0.0);
        let a11y = rt.accessibility_json();
        assert_eq!(super::a11y_name(&a11y, "rune-caption-minimize"), Some("Minimize"));
        assert_eq!(super::a11y_name(&a11y, "rune-caption-maximize"), Some("Maximize"));
        assert_eq!(super::a11y_name(&a11y, "rune-caption-close"), Some("Close"));

        crate::set_labels(|label| match label {
            crate::Label::Minimize => Some("Minimaliseren".into()),
            crate::Label::Maximize => Some("Maximaliseren".into()),
            crate::Label::Restore => Some("Herstellen".into()),
            crate::Label::Close => Some("Sluiten".into()),
            _ => None,
        });
        rt.update(|_, _| {});
        rt.frame(0.1);
        let a11y = rt.accessibility_json();
        assert_eq!(super::a11y_name(&a11y, "rune-caption-minimize"), Some("Minimaliseren"));
        assert_eq!(super::a11y_name(&a11y, "rune-caption-maximize"), Some("Maximaliseren"));
        assert_eq!(super::a11y_name(&a11y, "rune-caption-close"), Some("Sluiten"));
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::cell::Cell;

    use crate::el::el;
    use crate::runtime::Runtime;
    use crate::{Chrome, ChromeInsets, Cx, El, Env, Titlebar};

    #[derive(Default)]
    struct State {
        seen: Cell<ChromeInsets>,
        seen_inset: Cell<f32>,
    }

    fn view(state: &State, cx: &mut Cx<State>) -> El {
        state.seen.set(cx.chrome_insets());
        state.seen_inset.set(cx.titlebar_inset());
        el().key("root").w_full().h_full()
    }

    #[test]
    fn headless_chrome_reserves_rtl_traffic_light_space_on_the_right() {
        let titlebar = Titlebar::new(36.0);
        let custom_inset = titlebar.mac_chrome(Chrome::DEFAULT).titlebar_inset();
        let default_inset = Chrome::DEFAULT.titlebar_inset();

        // LTR custom titlebar: traffic lights sit on the left and reserve leading space.
        let mut rt = Runtime::new(State::default(), view, (400.0, 200.0), Env::new(true, 2.0));
        rt.core_mut().headless_chrome_with(Chrome::DEFAULT, Some(titlebar), false, false);
        rt.frame(0.0);
        let expected_ltr = ChromeInsets::new(custom_inset, 0.0, 36.0);
        assert_eq!(rt.chrome_insets(), expected_ltr, "LTR custom titlebar runtime insets");
        assert_eq!(rt.state.seen.get(), expected_ltr, "LTR custom titlebar view insets");
        assert_eq!(rt.state.seen_inset.get(), custom_inset, "LTR custom titlebar titlebar_inset");

        // RTL custom titlebar: traffic lights sit on the right and reserve trailing space.
        let mut rt = Runtime::new(State::default(), view, (400.0, 200.0), Env::new(true, 2.0));
        rt.core_mut().headless_chrome_with(Chrome::DEFAULT, Some(titlebar), false, true);
        rt.frame(0.0);
        let expected_rtl = ChromeInsets::new(0.0, custom_inset, 36.0);
        assert_eq!(rt.chrome_insets(), expected_rtl, "RTL custom titlebar runtime insets");
        assert_eq!(rt.state.seen.get(), expected_rtl, "RTL custom titlebar view insets");
        assert_eq!(rt.state.seen_inset.get(), custom_inset, "RTL custom titlebar titlebar_inset");

        // Full-size content without App::titlebar also moves the inset to trailing in RTL.
        let mut rt = Runtime::new(State::default(), view, (400.0, 200.0), Env::new(true, 2.0));
        rt.core_mut().headless_chrome_with(Chrome::DEFAULT, None, false, false);
        rt.frame(0.0);
        assert_eq!(rt.chrome_insets(), ChromeInsets::new(default_inset, 0.0, 0.0), "LTR default chrome insets");
        assert_eq!(rt.state.seen.get(), ChromeInsets::new(default_inset, 0.0, 0.0), "LTR default chrome view insets");

        rt.core_mut().headless_chrome_with(Chrome::DEFAULT, None, false, true);
        rt.frame(0.0);
        assert_eq!(rt.chrome_insets(), ChromeInsets::new(0.0, default_inset, 0.0), "RTL default chrome insets");
        assert_eq!(rt.state.seen.get(), ChromeInsets::new(0.0, default_inset, 0.0), "RTL default chrome view insets");
        assert_eq!(rt.state.seen_inset.get(), default_inset, "RTL default chrome titlebar_inset");

        // Borderless and standard titled windows reserve no traffic-light inset in RTL.
        rt.core_mut().headless_chrome_with(Chrome::DEFAULT, Some(titlebar), true, true);
        rt.frame(0.0);
        assert_eq!(rt.chrome_insets(), ChromeInsets::default(), "RTL borderless insets");
        assert_eq!(rt.state.seen_inset.get(), 0.0, "RTL borderless titlebar_inset");

        let standard = Chrome { full_size_content: false, traffic_lights: None, drag_regions: false };
        rt.core_mut().headless_chrome_with(standard, None, false, true);
        rt.frame(0.0);
        assert_eq!(rt.chrome_insets(), ChromeInsets::default(), "RTL standard titled insets");
        assert_eq!(rt.state.seen_inset.get(), 0.0, "RTL standard titled titlebar_inset");
    }
}

#[test]
fn headless_chrome_clears_traffic_lights_for_borderless_windows() {
    use std::cell::Cell;

    use crate::el::el;
    use crate::runtime::Runtime;
    use crate::{Chrome, ChromeInsets, Cx, El, Env, Titlebar};

    #[derive(Default)]
    struct State {
        insets: Cell<ChromeInsets>,
        inset: Cell<f32>,
    }

    fn view(state: &State, cx: &mut Cx<State>) -> El {
        state.insets.set(cx.chrome_insets());
        state.inset.set(cx.titlebar_inset());
        el().key("root").w_full().h_full()
    }

    let cases = [
        (Chrome::DEFAULT, None),
        (Chrome::DEFAULT, Some(Titlebar::new(36.0))),
        (Chrome { full_size_content: true, traffic_lights: Some((16.0, 17.0)), drag_regions: true }, Some(Titlebar::new(36.0))),
        (Chrome { full_size_content: false, traffic_lights: Some((16.0, 17.0)), drag_regions: false }, None),
    ];
    for (chrome, titlebar) in cases {
        for rtl in [false, true] {
            let mut rt = Runtime::new(State::default(), view, (400.0, 200.0), Env::new(true, 2.0));
            let resolved = rt.core_mut().headless_chrome_with(chrome, titlebar, true, rtl);
            rt.frame(0.0);
            assert_eq!(resolved, Chrome { full_size_content: false, traffic_lights: None, drag_regions: chrome.drag_regions });
            assert!(resolved.traffic_light_rects().is_none(), "borderless window must have no traffic lights");
            assert!(resolved.traffic_light_rects_in(400.0, rtl).is_none(), "borderless window must have no traffic lights (rtl={rtl})");
            assert_eq!(resolved.titlebar_inset(), 0.0);
            assert_eq!(rt.chrome_insets(), ChromeInsets::default());
            assert_eq!(rt.state.inset.get(), 0.0);
            assert_eq!(rt.state.insets.get(), ChromeInsets::default());
            assert!(rt.tree.nodes.iter().all(|n| n.key.as_deref() != Some(crate::titlebar::BUTTONS_KEY)));
        }
    }
}

#[test]
fn headless_chrome_current_tracks_dynamic_titlebar_and_ignores_borderless_or_native() {
    use crate::el::el;
    use crate::runtime::Runtime;
    use crate::{Chrome, Cx, El, Env, Titlebar};

    struct State { phase: usize }

    fn view(s: &State, cx: &mut Cx<State>) -> El {
        match s.phase {
            1 => cx.titlebar_height(72.0),
            2 => cx.set_titlebar(Titlebar::new(72.0).traffic_lights(30.0, 52.0)),
            _ => {}
        }
        el().key("root").w_full().h_full()
    }

    let mut rt = Runtime::new(State { phase: 0 }, view, (300.0, 100.0), Env::new(true, 1.0));
    let initial = rt.headless_chrome(Chrome::DEFAULT, Some(Titlebar::new(36.0)), false);
    assert_eq!(initial.traffic_lights, Some((16.0, 11.0)));
    assert_eq!(rt.core().headless_chrome_current(), initial);

    for (phase, origin) in [(0, (16.0, 11.0)), (1, (16.0, 29.0)), (2, (30.0, 52.0)), (3, (16.0, 11.0))] {
        rt.update(move |s, _| s.phase = phase);
        rt.frame(0.1);
        assert_eq!(rt.core().headless_chrome_current().traffic_lights, Some(origin), "phase {phase}");
    }

    // Borderless windows keep traffic lights cleared even when the view sets a titlebar.
    let mut borderless = Runtime::new(State { phase: 2 }, view, (300.0, 100.0), Env::new(true, 1.0));
    borderless.headless_chrome(Chrome::DEFAULT, Some(Titlebar::new(36.0)), true);
    borderless.frame(0.0);
    assert!(borderless.core().headless_chrome_current().traffic_light_rects().is_none());

    // Windows without App::titlebar keep their initial Chrome when the view calls Cx::set_titlebar.
    let mut native = Runtime::new(State { phase: 2 }, view, (300.0, 100.0), Env::new(true, 1.0));
    native.headless_chrome(Chrome::DEFAULT, None, false);
    native.frame(0.0);
    assert_eq!(native.core().headless_chrome_current(), Chrome::DEFAULT);
}

#[test]
fn headless_chrome_controls_false_hides_system_controls_and_zeroes_edge_insets() {
    use std::cell::Cell;

    use crate::el::el;
    use crate::runtime::Runtime;
    use crate::{Chrome, ChromeInsets, Cx, El, Env, Titlebar};

    struct State {
        phase: usize,
        insets: Cell<ChromeInsets>,
        inset: Cell<f32>,
    }

    fn view(state: &State, cx: &mut Cx<State>) -> El {
        match state.phase {
            1 => cx.set_titlebar(Titlebar::new(36.0).controls(false)),
            2 => cx.set_titlebar(Titlebar::new(44.0)),
            _ => {}
        }
        state.insets.set(cx.chrome_insets());
        state.inset.set(cx.titlebar_inset());
        el().key("root").w_full().h_full()
    }

    let expected_height = if cfg!(any(target_os = "macos", target_os = "windows")) { 36.0 } else { 0.0 };
    for rtl in [false, true] {
        let mut rt = Runtime::new(
            State { phase: 0, insets: Cell::default(), inset: Cell::default() },
            view,
            (400.0, 200.0),
            Env::new(true, 1.25),
        );
        let resolved = rt.core_mut().headless_chrome_with(
            Chrome { full_size_content: true, traffic_lights: Some((16.0, 17.0)), drag_regions: true },
            Some(Titlebar::new(36.0).controls(false)),
            false,
            rtl,
        );
        rt.frame(0.0);
        assert!(resolved.traffic_light_rects().is_none(), "controls(false) must hide headless traffic lights (rtl={rtl})");
        assert!(resolved.traffic_light_rects_in(400.0, rtl).is_none(), "controls(false) must hide headless traffic lights in rtl={rtl}");
        assert_eq!(resolved.titlebar_inset(), 0.0);
        assert_eq!(rt.state.inset.get(), 0.0);
        assert_eq!(rt.chrome_insets(), ChromeInsets::new(0.0, 0.0, expected_height), "rtl={rtl}");
        assert_eq!(rt.state.insets.get(), ChromeInsets::new(0.0, 0.0, expected_height), "rtl={rtl}");
        assert!(rt.tree.nodes.iter().all(|n| n.key.as_deref() != Some(crate::titlebar::BUTTONS_KEY)), "controls(false) must draw no caption buttons");
        if cfg!(target_os = "windows") {
            assert!(rt.tree.nodes.iter().any(|n| n.key.as_deref() == Some(crate::titlebar::FRAME_KEY)), "Windows keeps rune-frame so view paths stay stable");
        }
    }

    // Dynamic toggle via Cx::set_titlebar: controls(true) -> controls(false) -> controls(true).
    let mut rt = Runtime::new(
        State { phase: 0, insets: Cell::default(), inset: Cell::default() },
        view,
        (400.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.headless_chrome(Chrome::DEFAULT, Some(Titlebar::new(36.0)), false);
    rt.frame(0.0);
    assert!(rt.core().headless_chrome_current().traffic_light_rects().is_some());
    if cfg!(target_os = "windows") {
        assert!(rt.tree.nodes.iter().any(|n| n.key.as_deref() == Some(crate::titlebar::BUTTONS_KEY)));
    }

    let has_live_caption = |rt: &Runtime<State>| {
        rt.tree.nodes.iter().enumerate().any(|(i, n)| {
            n.key.as_deref() == Some(crate::titlebar::BUTTONS_KEY) && !rt.core().subtree_exiting(i)
        })
    };
    rt.update(|s, _| s.phase = 1);
    rt.frame(0.1);
    assert!(rt.core().headless_chrome_current().traffic_light_rects().is_none());
    assert_eq!(rt.chrome_insets(), ChromeInsets::new(0.0, 0.0, expected_height));
    assert!(!has_live_caption(&rt), "controls(false) must remove live caption buttons in the same commit");

    rt.update(|s, tx| { s.phase = 2; tx.instant(); });
    rt.frame(0.2);
    // Restoring controls updates caption at commit end and marks dirty for the next frame.
    rt.frame(0.25);
    assert!(rt.core().headless_chrome_current().traffic_light_rects().is_some());
    if cfg!(target_os = "windows") {
        assert!(has_live_caption(&rt));
        assert_eq!(rt.chrome_insets().height, 44.0);
        assert!(rt.chrome_insets().trailing > 0.0);
    }
}

fn a11y_find<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    if v["key"] == key { return Some(v); }
    v["children"].as_array()?.iter().find_map(|c| a11y_find(c, key))
}

fn a11y_name<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    a11y_find(v, key).and_then(|n| n["name"].as_str())
}

fn a11y_states(v: &serde_json::Value, key: &str) -> Vec<String> {
    a11y_find(v, key)
        .and_then(|n| n["states"].as_array())
        .map_or_else(Vec::new, |s| s.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
}

#[test]
fn caption_button_labels_follow_set_labels_across_states_and_edges() {
    use crate::el::el;
    use crate::runtime::Runtime;
    use crate::{Cx, El, Env, Label};

    let _guard = crate::labels::test_lock();
    crate::set_labels(|_| None);

    // Unset / None hook preserves the English defaults for all four caption labels.
    assert_eq!(crate::labels::label(Label::Minimize), "Minimize");
    assert_eq!(crate::labels::label(Label::Maximize), "Maximize");
    assert_eq!(crate::labels::label(Label::Restore), "Restore");
    assert_eq!(crate::labels::label(Label::Close), "Close");

    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().key("root").w_full().h_full()
    }

    let mut rt = Runtime::new((), view, (400.0, 200.0), Env::new(true, 1.0));
    let mut caption = crate::titlebar::Caption::new(36.0);
    rt.set_caption(Some(caption));
    rt.frame(0.0);

    let a11y = rt.accessibility_json();
    assert_eq!(a11y_name(&a11y, "rune-caption-minimize"), Some("Minimize"));
    assert_eq!(a11y_name(&a11y, "rune-caption-maximize"), Some("Maximize"));
    assert_eq!(a11y_name(&a11y, "rune-caption-close"), Some("Close"));
    assert_eq!(a11y_find(&a11y, "rune-caption-maximize").unwrap()["role"], "button");

    // Maximized state uses Restore by default.
    caption.maximized = true;
    rt.set_caption(Some(caption));
    rt.frame(0.1);
    assert_eq!(a11y_name(&rt.accessibility_json(), "rune-caption-maximize"), Some("Restore"));

    // Custom labels localize Minimize, Maximize, Restore, and Close in both JSON and AccessKit.
    crate::set_labels(|label| match label {
        Label::Minimize => Some("Minimaliseren".into()),
        Label::Maximize => Some("Maximaliseren".into()),
        Label::Restore => Some("Herstellen".into()),
        Label::Close => Some("Sluiten".into()),
        _ => None,
    });
    caption.maximized = false;
    rt.set_caption(Some(caption));
    rt.frame(0.2);
    let a11y = rt.accessibility_json();
    assert_eq!(a11y_name(&a11y, "rune-caption-minimize"), Some("Minimaliseren"));
    assert_eq!(a11y_name(&a11y, "rune-caption-maximize"), Some("Maximaliseren"));
    assert_eq!(a11y_name(&a11y, "rune-caption-close"), Some("Sluiten"));

    let ak = rt.accessibility_tree();
    let ak_button = |key: &str| {
        ak.nodes.iter().find_map(|(_, n)| (n.author_id() == Some(key)).then_some(n)).expect("caption node")
    };
    assert_eq!(ak_button("rune-caption-minimize").role(), accesskit::Role::Button);
    assert_eq!(ak_button("rune-caption-minimize").label(), Some("Minimaliseren"));
    assert_eq!(ak_button("rune-caption-maximize").label(), Some("Maximaliseren"));
    assert_eq!(ak_button("rune-caption-close").label(), Some("Sluiten"));

    // Switching to maximized updates the button to the localized Restore label.
    caption.maximized = true;
    rt.set_caption(Some(caption));
    rt.frame(0.3);
    assert_eq!(a11y_name(&rt.accessibility_json(), "rune-caption-maximize"), Some("Herstellen"));

    // Returning None for some variants falls back to English only for those variants.
    crate::set_labels(|label| match label {
        Label::Minimize => Some("Minimaliseren".into()),
        Label::Close => Some("Sluiten".into()),
        _ => None,
    });
    rt.update(|_, _| {});
    rt.frame(0.4);
    let a11y = rt.accessibility_json();
    assert_eq!(a11y_name(&a11y, "rune-caption-minimize"), Some("Minimaliseren"));
    assert_eq!(a11y_name(&a11y, "rune-caption-maximize"), Some("Restore"));
    assert_eq!(a11y_name(&a11y, "rune-caption-close"), Some("Sluiten"));

    caption.maximized = false;
    rt.set_caption(Some(caption));
    rt.frame(0.5);
    assert_eq!(a11y_name(&rt.accessibility_json(), "rune-caption-maximize"), Some("Maximize"));

    // Empty and whitespace-only translations fall back to English so buttons stay named in UIA.
    crate::set_labels(|label| match label {
        Label::Minimize | Label::Close => Some(String::new()),
        Label::Maximize | Label::Restore => Some("   	 ".into()),
        _ => None,
    });
    rt.update(|_, _| {});
    rt.frame(0.6);
    let a11y = rt.accessibility_json();
    assert_eq!(a11y_name(&a11y, "rune-caption-minimize"), Some("Minimize"));
    assert_eq!(a11y_name(&a11y, "rune-caption-maximize"), Some("Maximize"));
    assert_eq!(a11y_name(&a11y, "rune-caption-close"), Some("Close"));
    caption.maximized = true;
    rt.set_caption(Some(caption));
    rt.frame(0.7);
    assert_eq!(a11y_name(&rt.accessibility_json(), "rune-caption-maximize"), Some("Restore"));

    // Right-to-left mirrored caption with Arabic and long labels preserves exact accessible names.
    let long_close = "إغلاق ".repeat(40);
    let long_close_for_hook = long_close.clone();
    crate::set_labels(move |label| match label {
        Label::Minimize => Some("تصغير".into()),
        Label::Maximize => Some("تكبير".into()),
        Label::Restore => Some("استعادة".into()),
        Label::Close => Some(long_close_for_hook.clone()),
        _ => None,
    });
    caption.mirrored = true;
    caption.maximized = false;
    rt.set_caption(Some(caption));
    rt.frame(0.8);
    let a11y = rt.accessibility_json();
    assert_eq!(a11y_name(&a11y, "rune-caption-minimize"), Some("تصغير"));
    assert_eq!(a11y_name(&a11y, "rune-caption-maximize"), Some("تكبير"));
    assert_eq!(a11y_name(&a11y, "rune-caption-close"), Some(long_close.as_str()));

    // Disabled maximize button keeps its localized name and disabled state; Close-only omits min/max.
    caption.maximizable = false;
    rt.set_caption(Some(caption));
    rt.frame(0.9);
    let a11y = rt.accessibility_json();
    assert_eq!(a11y_name(&a11y, "rune-caption-maximize"), Some("تكبير"));
    assert!(a11y_states(&a11y, "rune-caption-maximize").iter().any(|s| s == "disabled"));

    caption.minimizable = false;
    rt.set_caption(Some(caption));
    rt.frame(1.0);
    let a11y = rt.accessibility_json();
    assert!(a11y_find(&a11y, "rune-caption-minimize").is_none());
    assert!(a11y_find(&a11y, "rune-caption-maximize").is_none());
    assert_eq!(a11y_name(&a11y, "rune-caption-close"), Some(long_close.as_str()));

    // Dynamic language switch on the next commit updates existing buttons without changing Caption.
    caption.minimizable = true;
    caption.maximizable = true;
    caption.mirrored = false;
    rt.set_caption(Some(caption));
    rt.frame(1.1);
    crate::set_labels(|label| match label {
        Label::Minimize => Some("Minimieren".into()),
        Label::Maximize => Some("Maximieren".into()),
        Label::Restore => Some("Wiederherstellen".into()),
        Label::Close => Some("Schließen".into()),
        _ => None,
    });
    rt.update(|_, _| {});
    rt.frame(1.2);
    let a11y = rt.accessibility_json();
    assert_eq!(a11y_name(&a11y, "rune-caption-minimize"), Some("Minimieren"));
    assert_eq!(a11y_name(&a11y, "rune-caption-maximize"), Some("Maximieren"));
    assert_eq!(a11y_name(&a11y, "rune-caption-close"), Some("Schließen"));
}

#[test]
fn full_screen_requests_latch_and_last_call_wins() {
    use crate::el::{FullScreenRequest, Tx, el};
    use crate::runtime::Runtime;
    use crate::{Cx, El, Env};

    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().key("root").w_full().h_full()
    }

    let mut rt = Runtime::new((), view, (400.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.take_full_screen_request(), None);
    assert!(!rt.take_toggle_maximized());

    // Explicit set_full_screen(true) and single-consume semantics.
    rt.update(|_, tx| tx.set_full_screen(true));
    assert_eq!(rt.take_full_screen_request(), Some(FullScreenRequest::Set(true)));
    assert_eq!(rt.take_full_screen_request(), None);

    // Explicit set_full_screen(false).
    rt.update(|_, tx| tx.set_full_screen(false));
    assert_eq!(rt.take_full_screen_request(), Some(FullScreenRequest::Set(false)));
    assert_eq!(rt.take_full_screen_request(), None);

    // Unchanged toggle_full_screen() latches Toggle.
    rt.update(|_, tx| tx.toggle_full_screen());
    assert_eq!(rt.take_full_screen_request(), Some(FullScreenRequest::Toggle));
    assert_eq!(rt.take_full_screen_request(), None);

    // Within one handler, last call wins.
    rt.update(|_, tx| {
        tx.toggle_full_screen();
        tx.set_full_screen(false);
    });
    assert_eq!(rt.take_full_screen_request(), Some(FullScreenRequest::Set(false)));

    rt.update(|_, tx| {
        tx.set_full_screen(true);
        tx.toggle_full_screen();
    });
    assert_eq!(rt.take_full_screen_request(), Some(FullScreenRequest::Toggle));

    // Across turns before the shell drains: a quiet update preserves an earlier request,
    // and a later request overwrites it; toggle_maximized stays independent.
    rt.update(|_, tx| {
        tx.set_full_screen(true);
        tx.toggle_maximized();
    });
    rt.update(|_, _| {});
    assert_eq!(rt.take_full_screen_request(), Some(FullScreenRequest::Set(true)));
    assert!(rt.take_toggle_maximized());
    assert!(!rt.take_toggle_maximized());

    rt.update(|_, tx| tx.set_full_screen(true));
    rt.update(|_, tx| tx.set_full_screen(false));
    assert_eq!(rt.take_full_screen_request(), Some(FullScreenRequest::Set(false)));

    // action::merge preserves an earlier request when the second Tx is empty, else last wins.
    let mut merged = Tx::default();
    merged.set_full_screen(true);
    crate::runtime::input::action::merge(&mut merged, Tx::default());
    assert_eq!(merged.full_screen, Some(FullScreenRequest::Set(true)));
    let mut override_tx = Tx::default();
    override_tx.toggle_full_screen();
    crate::runtime::input::action::merge(&mut merged, override_tx);
    assert_eq!(merged.full_screen, Some(FullScreenRequest::Toggle));
}

#[test]
fn deferred_titlebar_press_machine_holds_until_threshold_and_arms_double_click() {
    use super::{DRAG_THRESHOLD, Event, Pair, Request, TitlebarPress, step};
    use crate::runtime::TitlebarAction;

    let collect = |state: &mut TitlebarPress, event: Event| -> Vec<Request> {
        step(state, event).into_iter().collect()
    };

    // 1. Press in deferred mode enters Holding with no action; motion inside threshold stays Holding.
    let mut state = TitlebarPress::Rest;
    let origin = (100.0, 20.0);
    assert!(collect(&mut state, Event::Region { now: 1.0, point: origin, interval: 0.3, defer: true }).is_empty());
    assert_eq!(state, TitlebarPress::Holding(Pair { at: 1.0, point: origin }));
    for delta in [(0.0, 0.0), (7.9, 0.0), (-7.9, 7.9), (0.0, -7.9)] {
        let point = (origin.0 + delta.0, origin.1 + delta.1);
        assert!(collect(&mut state, Event::Move { point }).is_empty(), "inside threshold: {delta:?}");
        assert_eq!(state, TitlebarPress::Holding(Pair { at: 1.0, point: origin }));
    }

    // 2. Motion reaching DRAG_THRESHOLD (8 pt) on any axis emits Drag once and keeps Pair armed
    //    (Chromium's WindowEventFilterLinux rule: double-click is judged by press time & distance).
    for delta in [(DRAG_THRESHOLD, 0.0), (-DRAG_THRESHOLD, 0.0), (0.0, DRAG_THRESHOLD), (0.0, -DRAG_THRESHOLD)] {
        let mut s = TitlebarPress::Rest;
        assert!(collect(&mut s, Event::Region { now: 1.0, point: origin, interval: 0.3, defer: true }).is_empty());
        let moved = (origin.0 + delta.0, origin.1 + delta.1);
        assert_eq!(collect(&mut s, Event::Move { point: moved }), vec![Request::Action(TitlebarAction::Drag)], "at threshold: {delta:?}");
        assert_eq!(s, TitlebarPress::Armed(Pair { at: 1.0, point: origin }));
        // Further motion and release emit nothing.
        assert!(collect(&mut s, Event::Move { point: (moved.0 + 20.0, moved.1) }).is_empty());
        assert!(collect(&mut s, Event::Release).is_empty());
        // A second press after double_click_interval (a real drag) starts a new Holding.
        assert!(collect(&mut s, Event::Region { now: 1.5, point: origin, interval: 0.3, defer: true }).is_empty());
        assert_eq!(s, TitlebarPress::Holding(Pair { at: 1.5, point: origin }));
    }
    // Slow-hand double-click: first press crosses threshold (Drag), second press within interval
    // and CLICK_SLOP still emits DoubleClick.
    state = TitlebarPress::Rest;
    assert!(collect(&mut state, Event::Region { now: 1.0, point: origin, interval: 0.3, defer: true }).is_empty());
    assert_eq!(
        collect(&mut state, Event::Move { point: (origin.0 + DRAG_THRESHOLD, origin.1) }),
        vec![Request::Action(TitlebarAction::Drag)],
    );
    assert!(collect(&mut state, Event::Leave).is_empty());
    assert_eq!(
        collect(&mut state, Event::Region { now: 1.15, point: origin, interval: 0.3, defer: true }),
        vec![Request::Action(TitlebarAction::DoubleClick)],
    );
    assert_eq!(state, TitlebarPress::Rest);

    // 3. Press -> release -> press (no motion): zero Drag, DoubleClick on second press.
    state = TitlebarPress::Rest;
    assert!(collect(&mut state, Event::Region { now: 1.8, point: origin, interval: 0.3, defer: true }).is_empty());
    assert!(collect(&mut state, Event::Release).is_empty());
    assert_eq!(state, TitlebarPress::Armed(Pair { at: 1.8, point: origin }));
    assert_eq!(
        collect(&mut state, Event::Region { now: 1.9, point: origin, interval: 0.3, defer: true }),
        vec![Request::Action(TitlebarAction::DoubleClick)],
    );
    assert_eq!(state, TitlebarPress::Rest);

    // 4. Press -> small motion inside threshold -> release -> press: zero Drag, DoubleClick.
    state = TitlebarPress::Rest;
    assert!(collect(&mut state, Event::Region { now: 2.0, point: origin, interval: 0.3, defer: true }).is_empty());
    assert!(collect(&mut state, Event::Move { point: (origin.0 + 2.0, origin.1 - 1.0) }).is_empty());
    assert!(collect(&mut state, Event::Release).is_empty());
    assert_eq!(state, TitlebarPress::Armed(Pair { at: 2.0, point: origin }));
    assert_eq!(
        collect(&mut state, Event::Region { now: 2.2, point: (origin.0 + 3.0, origin.1), interval: 0.3, defer: true }),
        vec![Request::Action(TitlebarAction::DoubleClick)],
    );
    assert_eq!(state, TitlebarPress::Rest);
    // Motion after DoubleClick does not start a drag.
    assert!(collect(&mut state, Event::Move { point: (origin.0 + 40.0, origin.1) }).is_empty());

    // 5. Second press outside double-click interval starts a new Holding instead of DoubleClick.
    state = TitlebarPress::Rest;
    assert!(collect(&mut state, Event::Region { now: 3.0, point: origin, interval: 0.3, defer: true }).is_empty());
    assert!(collect(&mut state, Event::Release).is_empty());
    assert!(collect(&mut state, Event::Region { now: 3.4, point: origin, interval: 0.3, defer: true }).is_empty());
    assert_eq!(state, TitlebarPress::Holding(Pair { at: 3.4, point: origin }));

    // 6. Second press outside CLICK_SLOP (4 pt) starts a new Holding instead of DoubleClick.
    state = TitlebarPress::Rest;
    assert!(collect(&mut state, Event::Region { now: 4.0, point: origin, interval: 0.3, defer: true }).is_empty());
    assert!(collect(&mut state, Event::Release).is_empty());
    let far = (origin.0 + 5.0, origin.1);
    assert!(collect(&mut state, Event::Region { now: 4.1, point: far, interval: 0.3, defer: true }).is_empty());
    assert_eq!(state, TitlebarPress::Holding(Pair { at: 4.1, point: far }));

    // 7. Press -> leave -> enter (move) -> press with NO release in between:
    //    leave ends the held drag wait (so motion after re-enter never fires Drag) while keeping
    //    the pair armed so the second press within interval and slop emits DoubleClick.
    state = TitlebarPress::Rest;
    assert!(collect(&mut state, Event::Region { now: 6.0, point: origin, interval: 0.3, defer: true }).is_empty());
    assert!(collect(&mut state, Event::Leave).is_empty());
    assert_eq!(state, TitlebarPress::Armed(Pair { at: 6.0, point: origin }));
    assert!(collect(&mut state, Event::Move { point: (origin.0 + 30.0, origin.1) }).is_empty(), "leave ended drag wait");
    assert_eq!(
        collect(&mut state, Event::Region { now: 6.15, point: origin, interval: 0.3, defer: true }),
        vec![Request::Action(TitlebarAction::DoubleClick)],
    );
    assert_eq!(state, TitlebarPress::Rest);
    // Leave while already Armed preserves the armed pair too.
    assert!(collect(&mut state, Event::Region { now: 6.5, point: origin, interval: 0.3, defer: true }).is_empty());
    assert!(collect(&mut state, Event::Release).is_empty());
    assert!(collect(&mut state, Event::Leave).is_empty());
    assert_eq!(state, TitlebarPress::Armed(Pair { at: 6.5, point: origin }));

    // 8. Clear, SecondaryDown, Resize, and OtherPress each cancel Holding.
    for cancel_event in [
        Event::Clear,
        Event::SecondaryDown { trigger: super::WindowMenuTrigger::None },
        Event::OtherPress,
        Event::Empty { now: 7.05, point: origin, interval: 0.3, band: false },
    ] {
        let mut s = TitlebarPress::Rest;
        assert!(collect(&mut s, Event::Region { now: 7.0, point: origin, interval: 0.3, defer: true }).is_empty());
        assert!(collect(&mut s, cancel_event).is_empty());
        assert_eq!(s, TitlebarPress::Rest);
        assert!(collect(&mut s, Event::Move { point: (origin.0 + 30.0, origin.1) }).is_empty());
    }
    state = TitlebarPress::Rest;
    assert!(collect(&mut state, Event::Region { now: 7.5, point: origin, interval: 0.3, defer: true }).is_empty());
    assert!(collect(&mut state, Event::SecondaryDown { trigger: super::WindowMenuTrigger::OnRelease }).is_empty());
    assert_eq!(state, TitlebarPress::SecondaryOnRegion(None));
    assert!(collect(&mut state, Event::Move { point: (origin.0 + 30.0, origin.1) }).is_empty());
    state = TitlebarPress::Rest;
    assert!(collect(&mut state, Event::Region { now: 7.75, point: origin, interval: 0.3, defer: true }).is_empty());
    assert_eq!(
        collect(&mut state, Event::SecondaryDown { trigger: super::WindowMenuTrigger::OnPress }),
        vec![Request::Menu],
    );
    assert_eq!(state, TitlebarPress::Rest);
    assert!(collect(&mut state, Event::Move { point: (origin.0 + 30.0, origin.1) }).is_empty());
    state = TitlebarPress::Rest;
    assert!(collect(&mut state, Event::Region { now: 8.0, point: origin, interval: 0.3, defer: true }).is_empty());
    assert_eq!(
        collect(&mut state, Event::Resize { edge: crate::ResizeEdge::North }),
        vec![Request::Resize(crate::ResizeEdge::North)],
    );
    assert_eq!(state, TitlebarPress::Rest);

    // 9. Immediate mode (macOS / Windows) still emits Drag on press and DoubleClick on second press.
    state = TitlebarPress::Rest;
    assert_eq!(
        collect(&mut state, Event::Region { now: 9.0, point: origin, interval: 0.3, defer: false }),
        vec![Request::Action(TitlebarAction::Drag)],
    );
    assert_eq!(state, TitlebarPress::Armed(Pair { at: 9.0, point: origin }));
    assert_eq!(
        collect(&mut state, Event::Region { now: 9.1, point: origin, interval: 0.3, defer: false }),
        vec![Request::Action(TitlebarAction::DoubleClick)],
    );
    assert_eq!(state, TitlebarPress::Rest);
}

#[test]
fn deferred_window_drag_runtime_double_click_drag_leave_cancel_and_invariants() {
    use crate::el::{col, el, row};
    use crate::runtime::{Runtime, TitlebarAction};
    use crate::{Cx, El, Env};

    #[derive(Default)]
    struct State {
        button_clicks: u32,
        custom_doubles: u32,
    }

    fn view(_: &State, _: &mut Cx<State>) -> El {
        col().size(400.0, 200.0).child(
            row().key("bar").size(400.0, 40.0).window_drag()
                .child(el().key("btn").size(40.0, 40.0).on_press(|s: &mut State| s.button_clicks += 1))
                .child(
                    el().key("custom").size(80.0, 40.0).window_drag()
                        .on_double_press(|s: &mut State| s.custom_doubles += 1),
                ),
        )
    }

    // Platform default check: Linux/desktop Unix defers mouse drag; macOS and Windows stay immediate.
    let expected_default = cfg!(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")));
    assert_eq!(super::DEFER_MOUSE_DRAG, expected_default);

    // 1. Deferred mode (Linux): stationary double-click on bare .window_drag() emits DoubleClick with zero Drag.
    let mut rt = Runtime::new(State::default(), view, (400.0, 200.0), Env::new(true, 1.0));
    rt.frame.set_test_clock(crate::ManualClock::new(60.0));
    rt.double_click_interval = 0.3;
    rt.host.titlebar.defer_drag_override = Some(true);
    rt.frame(0.0);

    rt.pointer_move(250.0, 20.0);
    rt.pointer_down(250.0, 20.0);
    assert_eq!(rt.take_titlebar_action(), None, "deferred press must not start move at once");
    rt.pointer_move(253.0, 20.0);
    assert_eq!(rt.take_titlebar_action(), None, "drift inside 8 pt threshold stays a click");
    rt.pointer_up(253.0, 20.0);
    assert_eq!(rt.take_titlebar_action(), None);

    rt.frame.test_clock_mut().unwrap().seek(0.15);
    rt.pointer_down(252.0, 20.0);
    assert_eq!(rt.take_titlebar_action(), Some(TitlebarAction::DoubleClick));
    rt.pointer_move(280.0, 20.0);
    assert_eq!(rt.take_titlebar_action(), None, "moving after DoubleClick must not also emit Drag");
    rt.pointer_up(280.0, 20.0);

    // 2. Deferred mode drag: crossing 8 pt emits Drag once and observe_pointer reports a redraw.
    rt.frame.test_clock_mut().unwrap().seek(1.0);
    rt.pointer_move(250.0, 20.0);
    rt.pointer_down(250.0, 20.0);
    assert_eq!(rt.take_titlebar_action(), None);
    assert!(!rt.observe_pointer(257.0, 20.0), "still inside 8 pt threshold");
    assert_eq!(rt.take_titlebar_action(), None);
    assert!(rt.observe_pointer(258.0, 20.0), "crossing 8 pt threshold must wake host to take Drag");
    assert_eq!(rt.take_titlebar_action(), Some(TitlebarAction::Drag));
    rt.pointer_move(280.0, 20.0);
    assert_eq!(rt.take_titlebar_action(), None, "Drag is emitted once");
    rt.pointer_up(280.0, 20.0);

    // 3. Press -> leave -> enter -> press with NO release in between (compositor grab trace):
    //    leave ends the drag wait (so re-entering and moving does not drag), while keeping the
    //    armed pair so a second press within double-click time and slop emits DoubleClick.
    rt.frame.test_clock_mut().unwrap().seek(2.0);
    rt.pointer_move(250.0, 20.0);
    rt.pointer_down(250.0, 20.0);
    assert_eq!(rt.take_titlebar_action(), None);
    rt.pointer_leave();
    rt.pointer_move(280.0, 20.0);
    assert_eq!(rt.take_titlebar_action(), None, "leave while holding cancels pending drag");
    rt.frame.test_clock_mut().unwrap().seek(2.15);
    rt.pointer_move(250.0, 20.0);
    rt.pointer_down(250.0, 20.0);
    assert_eq!(rt.take_titlebar_action(), Some(TitlebarAction::DoubleClick), "press -> leave -> enter -> press double-clicks");
    rt.pointer_up(250.0, 20.0);

    // 3b. Press -> release -> press (zero motion) emits zero Drag and DoubleClick on second press.
    rt.frame.test_clock_mut().unwrap().seek(2.5);
    rt.pointer_move(250.0, 20.0);
    rt.pointer_down(250.0, 20.0);
    assert_eq!(rt.take_titlebar_action(), None);
    rt.pointer_up(250.0, 20.0);
    rt.frame.test_clock_mut().unwrap().seek(2.6);
    rt.pointer_down(250.0, 20.0);
    assert_eq!(rt.take_titlebar_action(), Some(TitlebarAction::DoubleClick));
    rt.pointer_up(250.0, 20.0);

    // 3c. Slow-hand drag past 8 pt followed quickly by a second press at the same local point
    //     still double-clicks (Chromium's WindowEventFilterLinux rule); a later press after the
    //     double-click interval does not.
    rt.frame.test_clock_mut().unwrap().seek(4.0);
    rt.pointer_move(250.0, 20.0);
    rt.pointer_down(250.0, 20.0);
    rt.pointer_move(258.0, 20.0);
    assert_eq!(rt.take_titlebar_action(), Some(TitlebarAction::Drag));
    rt.pointer_leave();
    rt.frame.test_clock_mut().unwrap().seek(4.15);
    rt.pointer_move(250.0, 20.0);
    rt.pointer_down(250.0, 20.0);
    assert_eq!(rt.take_titlebar_action(), Some(TitlebarAction::DoubleClick));
    rt.pointer_up(250.0, 20.0);

    // 4. Cancel (focus loss) between clicks cancels armed double-click.
    rt.frame.test_clock_mut().unwrap().seek(5.0);
    rt.pointer_move(250.0, 20.0);
    rt.pointer_down(250.0, 20.0);
    rt.pointer_up(250.0, 20.0);
    rt.cancel_pointer();
    rt.frame.test_clock_mut().unwrap().seek(5.1);
    rt.pointer_down(250.0, 20.0);
    assert_eq!(rt.take_titlebar_action(), None, "cancel clears armed double-click");
    rt.pointer_up(250.0, 20.0);

    // 5. Titlebar buttons and .window_drag() regions with their own on_double_press stay unchanged
    //    in both deferred (Linux) and immediate (macOS/Windows) modes.
    for defer in [true, false] {
        let mut rt = Runtime::new(State::default(), view, (400.0, 200.0), Env::new(true, 1.0));
        rt.frame.set_test_clock(crate::ManualClock::new(60.0));
        rt.double_click_interval = 0.3;
        rt.host.titlebar.defer_drag_override = Some(defer);
        rt.frame(0.0);

        // Button inside titlebar.
        rt.pointer_move(20.0, 20.0);
        rt.pointer_down(20.0, 20.0);
        assert_eq!(rt.take_titlebar_action(), None);
        rt.pointer_up(20.0, 20.0);
        rt.frame(0.016);
        assert_eq!(rt.state.button_clicks, 1, "defer={defer}: titlebar button clicks");

        // .window_drag() region with its own on_double_press handler.
        rt.pointer_move(80.0, 20.0);
        rt.pointer_down(80.0, 20.0);
        assert_eq!(rt.take_titlebar_action(), None);
        rt.pointer_up(80.0, 20.0);
        rt.frame.test_clock_mut().unwrap().seek(0.1);
        rt.pointer_down(80.0, 20.0);
        assert_eq!(rt.take_titlebar_action(), None);
        rt.pointer_up(80.0, 20.0);
        rt.frame(0.116);
        assert_eq!(rt.state.custom_doubles, 1, "defer={defer}: custom on_double_press fires");

        // Touch on bare .window_drag() keeps its own WindowDragPending / TOUCH_SLOP / long-press path.
        use crate::runtime::TouchPhase;
        rt.frame.test_clock_mut().unwrap().seek(1.0);
        rt.touch(1, TouchPhase::Started, 250.0, 20.0, 1.0);
        assert_eq!(rt.take_titlebar_action(), None);
        rt.touch(1, TouchPhase::Ended, 250.0, 20.0, 1.05);
        rt.frame.test_clock_mut().unwrap().seek(1.15);
        rt.touch(2, TouchPhase::Started, 250.0, 20.0, 1.15);
        assert_eq!(rt.take_titlebar_action(), Some(TitlebarAction::DoubleClick), "defer={defer}: touch double-tap");
        rt.touch(2, TouchPhase::Ended, 250.0, 20.0, 1.2);

        rt.frame.test_clock_mut().unwrap().seek(2.0);
        rt.touch(3, TouchPhase::Started, 250.0, 20.0, 2.0);
        rt.touch(3, TouchPhase::Moved, 260.0, 20.0, 2.02);
        assert_eq!(rt.take_titlebar_action(), Some(TitlebarAction::Drag), "defer={defer}: touch drag");
        rt.touch(3, TouchPhase::Ended, 260.0, 20.0, 2.05);
    }

    // 6. Windows caption hit-test remains HTCAPTION for bare drag regions and HTCLIENT for buttons.
    #[cfg(target_os = "windows")]
    {
        let mut rt = Runtime::new(State::default(), view, (400.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let hits = rt.core().window_drag_hits();
        assert!(super::drag_hit_at(&hits, 250.0, 20.0), "bare drag region stays HTCAPTION on Windows");
        assert!(!super::drag_hit_at(&hits, 20.0, 20.0), "titlebar button stays HTCLIENT on Windows");
        assert!(!super::drag_hit_at(&hits, 80.0, 20.0), "custom on_double_press region stays HTCLIENT on Windows");
    }
}

/// Rune's own frame on Linux (`titlebar::linux`): its buttons and resize edges are elements, so a
/// run without a window presses them as the pointer does, on every platform.
mod linux_frame {
    use crate::el::el;
    use crate::runtime::Runtime;
    use crate::titlebar::linux::{ButtonLayout, Sides, WindowState};
    use crate::titlebar::{BUTTONS_KEY, Caption, EDGES_KEY};
    use crate::{Cursor, Cx, El, Env, ResizeEdge};

    const FLOATING: WindowState = WindowState { maximized: false, full_screen: false, resizable: true, minimizable: true, maximizable: true, tiled: Sides::NONE };

    #[derive(Default)]
    struct State {
        presses: usize,
        modal: bool,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let sheet = s.modal.then(|| el().key("sheet").size(120.0, 80.0).modal(true, |s: &mut State| s.modal = false));
        el().key("root").w_full().h_full().on_press(|s: &mut State| s.presses += 1).children(sheet)
    }

    /// The frame's caption for a desktop `layout` in an app that reads left to right.
    fn caption(layout: &str, window: WindowState) -> Caption {
        let layout = ButtonLayout::parse(layout);
        Caption { mirrored: layout.leading, ..Caption::own_frame(44.0, layout, window) }
    }

    fn rig(caption: Caption) -> Runtime<State> {
        let mut rt = Runtime::new(State::default(), view, (400.0, 300.0), Env::new(true, 1.0));
        rt.set_caption(Some(caption));
        rt.frame(0.0);
        rt
    }

    fn drawn(rt: &Runtime<State>, key: &str) -> Option<[f32; 4]> {
        rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some(key)).map(|i| rt.drawn_box(i))
    }

    fn near(a: [f32; 4], b: [f32; 4]) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-3)
    }

    fn click(rt: &mut Runtime<State>, (x, y): (f32, f32), at: f64) {
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(at);
    }

    fn centre([x0, y0, x1, y1]: [f32; 4]) -> (f32, f32) {
        ((x0 + x1) / 2.0, (y0 + y1) / 2.0)
    }

    #[test]
    fn the_buttons_the_layout_names_sit_at_its_side_in_the_band() {
        // GNOME's default: close alone, a 24 pt circle in the band's middle, 12 pt from the edge.
        let close = caption("appmenu:close", FLOATING);
        let rt = rig(close);
        let bar = drawn(&rt, BUTTONS_KEY).expect("the buttons' row");
        assert!(near(bar, [400.0 - close.width(1.0), 0.0, 400.0, 44.0]), "{bar:?}");
        let button = drawn(&rt, "rune-caption-close").expect("close");
        assert!(near(button, [364.0, 10.0, 388.0, 34.0]), "{button:?}");
        assert!(drawn(&rt, "rune-caption-minimize").is_none() && drawn(&rt, "rune-caption-maximize").is_none());

        // All three at the left, close nearest the window's edge.
        let left = caption("close,minimize,maximize:appmenu", FLOATING);
        let rt = rig(left);
        let bar = drawn(&rt, BUTTONS_KEY).expect("the buttons' row");
        assert!(near(bar, [0.0, 0.0, left.width(1.0), 44.0]), "{bar:?}");
        let lefts: Vec<f32> = ["rune-caption-close", "rune-caption-minimize", "rune-caption-maximize"]
            .iter().map(|key| drawn(&rt, key).expect("a button")[0]).collect();
        assert!(near([lefts[0], lefts[1], lefts[2], 0.0], [12.0, 49.0, 86.0, 0.0]), "{lefts:?}");

        // Full screen draws none, and a layout that names none neither.
        let full = rig(caption("appmenu:close", WindowState { full_screen: true, ..FLOATING }));
        assert!(drawn(&full, BUTTONS_KEY).is_none() && drawn(&full, EDGES_KEY).is_none());
        assert!(drawn(&rig(caption("appmenu:", FLOATING)), BUTTONS_KEY).is_none());
    }

    #[test]
    fn the_band_inside_the_edges_resizes_the_window_and_the_rest_is_the_views() {
        use ResizeEdge::{East, North, NorthEast, NorthWest, South, SouthEast, SouthWest, West};
        let mut rt = rig(caption("appmenu:close", FLOATING));
        let cases = [
            ((200.0, 2.0), North, Cursor::ResizeNs),
            ((200.0, 297.0), South, Cursor::ResizeNs),
            ((2.0, 150.0), West, Cursor::ResizeEw),
            ((397.0, 150.0), East, Cursor::ResizeEw),
            ((2.0, 2.0), NorthWest, Cursor::ResizeNwse),
            // Along a side, within 16 pt of its corner.
            ((10.0, 2.0), NorthWest, Cursor::ResizeNwse),
            ((2.0, 10.0), NorthWest, Cursor::ResizeNwse),
            ((390.0, 3.0), NorthEast, Cursor::ResizeNesw),
            ((397.0, 12.0), NorthEast, Cursor::ResizeNesw),
            ((3.0, 290.0), SouthWest, Cursor::ResizeNesw),
            ((397.0, 297.0), SouthEast, Cursor::ResizeNwse),
        ];
        for ((x, y), edge, cursor) in cases {
            rt.pointer_move(x, y);
            assert_eq!(rt.cursor(), cursor, "at {x}, {y}");
            rt.pointer_down(x, y);
            assert_eq!(rt.take_resize_action(), Some(edge), "at {x}, {y}");
            rt.pointer_up(x, y);
        }
        rt.frame(0.1);
        assert_eq!(rt.state.presses, 0, "a press on the band never reaches the view");

        // Past the band's 5 pt, and 16 pt in from a corner, the view has the press.
        for (i, at) in [(200.0, 150.0), (200.0, 6.0), (20.0, 20.0)].into_iter().enumerate() {
            rt.pointer_down(at.0, at.1);
            assert_eq!(rt.take_resize_action(), None, "at {at:?}");
            rt.pointer_up(at.0, at.1);
            rt.frame(0.2 + i as f64 / 10.0);
            assert_eq!(rt.state.presses, i + 1, "at {at:?}");
        }

        // A maximized window, and one that can't be resized, have no band.
        for window in [WindowState { maximized: true, ..FLOATING }, WindowState { resizable: false, ..FLOATING }] {
            let mut rt = rig(caption("appmenu:close", window));
            assert!(drawn(&rt, EDGES_KEY).is_none(), "{window:?}");
            rt.pointer_down(200.0, 2.0);
            assert_eq!(rt.take_resize_action(), None, "{window:?}");
        }
    }

    #[test]
    fn a_window_tiled_to_the_left_half_resizes_from_its_free_side_only() {
        use ResizeEdge::East;
        // Tiled on its top, left and bottom: the right side is the one a neighbour shares.
        let tiled = Sides { right: false, ..Sides::ALL };
        let mut rt = rig(caption("appmenu:close", WindowState { tiled, ..FLOATING }));
        // The free side's band runs the window's whole height: its corners have no arms.
        for at in [(397.0, 150.0), (397.0, 2.0), (397.0, 297.0)] {
            rt.pointer_move(at.0, at.1);
            assert_eq!(rt.cursor(), Cursor::ResizeEw, "at {at:?}");
            rt.pointer_down(at.0, at.1);
            assert_eq!(rt.take_resize_action(), Some(East), "at {at:?}");
            rt.pointer_up(at.0, at.1);
        }
        // The tiled sides are the view's again.
        for (i, at) in [(200.0, 2.0), (2.0, 150.0), (200.0, 297.0), (2.0, 2.0)].into_iter().enumerate() {
            rt.pointer_down(at.0, at.1);
            assert_eq!(rt.take_resize_action(), None, "at {at:?}");
            rt.pointer_up(at.0, at.1);
            rt.frame(0.1 + i as f64 / 10.0);
            assert_eq!(rt.state.presses, i + 1, "at {at:?}");
        }
    }

    #[test]
    fn the_pointer_presses_the_buttons_and_a_modal_leaves_the_frame_in_reach() {
        let mut rt = rig(caption("appmenu:minimize,maximize,close", FLOATING));
        let maximize = centre(drawn(&rt, "rune-caption-maximize").expect("maximize"));
        click(&mut rt, maximize, 0.1);
        assert!(rt.take_toggle_maximized(), "a press on Maximize asks the window to maximize");
        assert_eq!(rt.state.presses, 0, "the view under the button saw nothing");

        // The frame is the window's, not the view's: a modal in the view blocks neither.
        rt.update(|s, _| s.modal = true);
        rt.frame(0.2);
        rt.pointer_down(200.0, 2.0);
        assert_eq!(rt.take_resize_action(), Some(ResizeEdge::North));
        rt.pointer_up(200.0, 2.0);
        click(&mut rt, maximize, 0.3);
        assert!(rt.take_toggle_maximized());
        assert!(rt.state.modal, "neither press was one outside the modal");
    }
}
