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
