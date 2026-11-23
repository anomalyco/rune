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
