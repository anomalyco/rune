use super::*;
use rune::accesskit::{Action, ActionData, Role};

#[test]
fn scrub_bar_exposes_slider_a11y_and_seeks_on_drag_and_a11y_action() {
    let clip = Player::file(concat!(env!("CARGO_MANIFEST_DIR"), "/../../crates/rune/tests/fixtures/rgbw.mp4"));
    let viewer = Viewer { clip: clip.clone(), hidden: false, preferred_output: None, scrub_target: None, last_time: 0.0, last_seeking: false };
    let mut rt = rune::Runtime::new(viewer, view, (520.0, 360.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    // Run one more frame so the clip's duration from opening reflects in the slider's range.
    rt.frame(0.0);

    let tree = rt.accessibility_tree();
    let slider = tree.nodes.iter().find(|(_, n)| n.role() == Role::Slider).map(|(_, n)| n).expect("scrub slider in accessibility tree");
    assert_eq!(slider.label(), Some("Playback position"));
    assert!(slider.numeric_value().is_some());
    let bounds = slider.bounds().expect("scrub slider bounds");

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let w = (bounds.x1 - bounds.x0) as f32;
        let y = ((bounds.y0 + bounds.y1) * 0.5) as f32;
        // Drag across the scrub bar: fast-seeks during drag, exact seek on release.
        rt.pointer_down(bounds.x0 as f32 + w * 0.25, y);
        rt.frame(0.0);
        assert!(rt.state.scrub_target.is_some());
        rt.pointer_move(bounds.x0 as f32 + w * 0.75, y);
        rt.frame(0.0);
        assert!(rt.state.scrub_target.is_some());
        rt.pointer_up(bounds.x0 as f32 + w * 0.75, y);
        rt.frame(0.0);
        assert_eq!(rt.state.scrub_target, None);
        assert!((clip.time() - 0.75).abs() < 0.05, "{}", clip.time());

        // Assistive technology SetValue seeks cleanly too.
        assert!(rt.accessibility_action("scrub", Action::SetValue, Some(ActionData::NumericValue(0.4))));
        rt.frame(0.0);
        assert!((clip.time() - 0.4).abs() < 1e-6, "{}", clip.time());
    }
}
