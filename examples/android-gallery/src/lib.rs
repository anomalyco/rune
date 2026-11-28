//! Native Android entry point for `kit-gallery`, exercising touch taps, touch scrolling/flings,
//! long-press menus, and `rune-kit` components on Android through `RuneView` / `RuneActivity`.
//!
//! Build an APK with `scripts/android-run.sh --build-only examples/android-gallery`.

#![forbid(unsafe_code)]

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

    #[test]
    fn android_gallery_manifest_hides_action_bar_and_pads_safe_area() {
        let manifest = include_str!("../AndroidManifest.xml");
        assert!(
            manifest.contains(r#"android:theme="@android:style/Theme.DeviceDefault.NoActionBar""#),
            "android-gallery manifest must set a NoActionBar theme so no system title bar covers Rune",
        );

        let mut rt = kit_gallery::build_app().headless_runtime((390.0, 844.0), Env::new(true, 3.0));
        rt.set_safe_area([48.0, 0.0, 34.0, 0.0]);
        rt.frame(0.0);
        let scroll = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("index-scroll"))
            .expect("index-scroll present")
            .rect;
        assert_eq!(scroll, [0.0, 48.0, 390.0, 844.0 - 48.0 - 34.0]);
    }
}
