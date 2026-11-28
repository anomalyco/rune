//! Native Android entry point for `kit-gallery`, exercising touch taps, touch scrolling/flings,
//! long-press menus, and `rune-kit` components on Android through `RuneView` / `RuneActivity`.
//!
//! Build an APK with `scripts/android-run.sh --build-only examples/android-gallery`.

#![cfg_attr(not(target_os = "android"), forbid(unsafe_code))]

pub fn start() {
    kit_gallery::build_app().run();
}

rune::android_main!(start);

#[cfg(test)]
mod tests {
    use rune::{Env, TouchPhase};

    #[test]
    fn android_gallery_runs_with_touch_controls_and_400ms_long_press() {
        let mut rt = kit_gallery::build_app().headless_runtime((390.0, 844.0), Env::new(true, 3.0));
        rt.set_touch_controls(true);
        rt.set_touch_long_press(0.4);
        rt.frame(0.0);
        assert!(rt.touch_controls());
        assert_eq!(rt.touch_long_press(), 0.4);

        // Touch drag across the gallery index scrolls smoothly without panicking.
        rt.touch(1, TouchPhase::Started, 195.0, 500.0, 0.0);
        for i in 1..=5 {
            rt.touch(1, TouchPhase::Moved, 195.0, 500.0 - 30.0 * i as f32, i as f64 * 0.01);
        }
        rt.touch(1, TouchPhase::Ended, 195.0, 350.0, 0.055);
        for _ in 0..4 {
            rt.frame(1.0 / 60.0);
        }
    }
}
