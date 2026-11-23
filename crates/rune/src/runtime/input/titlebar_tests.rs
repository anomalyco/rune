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
