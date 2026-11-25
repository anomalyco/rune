use super::*;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use rune::accesskit::{Action, ActionData};
use rune::accesskit::Role;
use std::sync::Arc;

fn test_viewer(clip: Player) -> Viewer {
    let source = clip.source().clone();
    Viewer {
        source,
        clip,
        waker: None,
        filmstrip: None,
        hover_thumb: None,
        hidden: false,
        preferred_output: None,
        scrub_target: None,
        last_time: 0.0,
        last_presented: None,
        last_seeking: false,
        last_thumbs_done: 0,
    }
}

#[test]
fn scrub_bar_exposes_slider_a11y_and_seeks_on_drag_and_a11y_action() {
    let clip = Player::file(concat!(env!("CARGO_MANIFEST_DIR"), "/../../crates/rune/tests/fixtures/rgbw.mp4"));
    let viewer = test_viewer(clip.clone());
    let mut rt = rune::Runtime::new(viewer, view, (520.0, 440.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    // Run one more frame so the clip's duration from opening reflects in the slider's range.
    rt.frame(0.0);

    let tree = rt.accessibility_tree();
    let slider = tree.nodes.iter().find(|(_, n)| n.role() == Role::Slider).map(|(_, n)| n).expect("scrub slider in accessibility tree");
    assert_eq!(slider.label(), Some("Playback position"));
    assert!(slider.numeric_value().is_some());
    let bounds = slider.bounds().expect("scrub slider bounds");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = bounds;

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

#[test]
fn frame_step_buttons_keys_and_readout_work_and_stay_accessible() {
    let clip = Player::file(concat!(env!("CARGO_MANIFEST_DIR"), "/../../crates/rune/tests/fixtures/rgbw.mp4"));
    let viewer = test_viewer(clip.clone());
    let mut rt = rune::Runtime::new(viewer, view, (520.0, 440.0), rune::Env::new(true, 1.0));
    rt.poll = Some(poll);
    rt.frame(0.0);
    rt.frame(0.0);

    let tree = rt.accessibility_tree();
    let labels: Vec<_> = tree.nodes.iter().filter(|(_, n)| n.role() == Role::Button).filter_map(|(_, n)| n.label()).collect();
    assert!(labels.contains(&"Previous frame"), "missing Previous frame button: {labels:?}");
    assert!(labels.contains(&"Next frame"), "missing Next frame button: {labels:?}");

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        assert!((clip.frame_duration().unwrap() - 0.25).abs() < 1e-3, "{:?}", clip.frame_duration());
        assert!(clip.presented_time().unwrap().abs() < 1e-3, "{:?}", clip.presented_time());

        // Assistive technology Click on "Next frame" steps from frame 0 to frame 1.
        assert!(rt.accessibility_action("step-forward", Action::Click, None));
        rt.frame(0.0);
        assert!((clip.presented_time().unwrap() - 0.25).abs() < 1e-3, "{:?}", clip.presented_time());
        // Once the scrub bar settles, the idle paused window requests no frames.
        rt.frame(1.0);
        assert!(!rt.frame(1.1), "idle paused window requests no frames after stepping");

        // Readout shows frame index, presented timestamp, and fps in the accessibility tree.
        let tree = rt.accessibility_tree();
        let has_readout = tree.nodes.iter().any(|(_, n)| {
            n.role() == Role::Label && n.value().is_some_and(|v| v.contains("Frame 1") && v.contains("4.00 fps"))
        });
        assert!(has_readout, "frame readout missing from accessibility tree");

        // Right arrow while paused steps forward to frame 2; Left steps back to frame 1.
        let mut tx = Tx::default();
        let right = KeyEvent { key: Key::Right, mods: rune::Mods::default() };
        let left = KeyEvent { key: Key::Left, mods: rune::Mods::default() };
        assert!(on_key(&mut rt.state, &right, &mut tx));
        rt.frame(0.0);
        assert!((clip.presented_time().unwrap() - 0.5).abs() < 1e-3, "{:?}", clip.presented_time());

        assert!(on_key(&mut rt.state, &left, &mut tx));
        rt.frame(0.0);
        assert!((clip.presented_time().unwrap() - 0.25).abs() < 1e-3, "{:?}", clip.presented_time());

        // Assistive technology Click on "Previous frame" steps back to frame 0.
        assert!(rt.accessibility_action("step-back", Action::Click, None));
        rt.frame(0.0);
        assert!(clip.presented_time().unwrap().abs() < 1e-3, "{:?}", clip.presented_time());
    }
}

#[test]
fn hardware_media_keys_toggle_playback_only_when_opted_in() {
    let clip = Player::file(concat!(env!("CARGO_MANIFEST_DIR"), "/../../crates/rune/tests/fixtures/rgbw.mp4"));
    clip.set_muted(true);
    let viewer = test_viewer(clip.clone());
    let mut rt = rune::Runtime::new(viewer, view, (520.0, 440.0), rune::Env::new(true, 1.0));
    rt.on_key = Some(on_key);
    rt.frame(0.0);
    rt.frame(0.0);

    // Off by default: media keys do not reach on_key.
    assert!(!rt.media_keys());
    assert!(!clip.is_playing());
    rt.key(KeyEvent::from(Key::Media(MediaKey::PlayPause)));
    rt.frame(0.0);
    assert!(!clip.is_playing());

    // Opted in (matching App::media_keys): PlayPause/Pause/Play/Stop control playback.
    rt.set_media_keys(true);
    rt.key(KeyEvent::from(Key::Media(MediaKey::PlayPause)));
    rt.frame(0.0);
    assert!(clip.is_playing());
    rt.key(KeyEvent::from(Key::Media(MediaKey::Pause)));
    rt.frame(0.0);
    assert!(!clip.is_playing());
    rt.key(KeyEvent::from(Key::Media(MediaKey::Play)));
    rt.frame(0.0);
    assert!(clip.is_playing());
    rt.key(KeyEvent::from(Key::Media(MediaKey::Stop)));
    rt.frame(0.0);
    assert!(!clip.is_playing());
}

#[test]
fn filmstrip_exposes_accessible_tiles_hover_preview_and_click_to_seek() {
    let clip = Player::file(concat!(env!("CARGO_MANIFEST_DIR"), "/../../crates/rune/tests/fixtures/rgbw.mp4"));
    let mut viewer = test_viewer(clip.clone());
    viewer.waker = Some(Arc::new(|| {}));
    let mut rt = rune::Runtime::new(viewer, view, (520.0, 440.0), rune::Env::new(true, 1.0));
    rt.poll = Some(poll);
    rt.frame(0.0);
    rt.frame(0.0);

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let strip = rt.state.filmstrip.clone().expect("filmstrip started once duration loaded");
        strip.wait();
        rt.frame(0.0);
        assert_eq!(strip.len(), FILMSTRIP_COUNT);
        assert!(strip.is_done());
        assert!(strip.frames().iter().all(|f| f.is_ready()));

        let tree = rt.accessibility_tree();
        let has_group = tree.nodes.iter().any(|(_, n)| n.role() == Role::Group && n.label() == Some("Video filmstrip"));
        assert!(has_group, "filmstrip group present in accessibility tree");
        let seek_buttons = tree.nodes.iter().filter(|(_, n)| {
            n.role() == Role::Button && n.label().is_some_and(|l| l.starts_with("Seek to "))
        }).count();
        assert_eq!(seek_buttons, FILMSTRIP_COUNT);

        // Hovering the scrub bar shows the thumbnail preview.
        let slider = tree.nodes.iter().find(|(_, n)| n.role() == Role::Slider).map(|(_, n)| n).unwrap();
        let b = slider.bounds().unwrap();
        rt.pointer_move(((b.x0 + b.x1) * 0.5) as f32, ((b.y0 + b.y1) * 0.5) as f32);
        rt.frame(0.0);
        assert!(rt.state.hover_thumb.is_some());
        let tree = rt.accessibility_tree();
        let has_preview = tree.nodes.iter().any(|(_, n)| n.role() == Role::Group && n.label() == Some("Thumbnail preview"));
        assert!(has_preview, "hover thumbnail preview appears in accessibility tree");

        // Clicking a filmstrip tile seeks the clip.
        assert!(rt.accessibility_action("thumb-4", Action::Click, None));
        rt.frame(0.0);
        assert!((clip.time() - strip.frame(4).unwrap().requested_time()).abs() < 1e-6);

        // Settled filmstrip + paused clip requests no idle frames.
        rt.frame(1.0);
        assert!(!rt.frame(1.1), "settled filmstrip stays idle");
    }
}
