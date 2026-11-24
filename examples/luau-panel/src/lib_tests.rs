use rune::{Env, Runtime};

use super::*;

#[test]
fn props_showcase_scene_renders_without_errors_and_rests_idle() {
    let script = Script::new("panel", sources());
    assert!(script.error().is_none(), "start: {:?}", script.error());
    script.query("setTab", [rune_luau::Data::from("props")]).expect("switch to props tab");
    let mut rt = Runtime::new(State::new(script), view, (720.0, 640.0), Env::new(true, 1.0));
    rt.poll = Some(|s, now, tx| s.script.poll(now, tx));
    rt.frame(0.0);
    assert!(rt.state.script.log().is_empty(), "unexpected logs: {:?}", rt.state.script.log());
    for key in ["props-grid", "clamped-copy", "tilted-tile", "clipped-box", "raised-badge", "demo-tablist", "flying-card", "pct-fill", "role-slider", "card-scroll-motion", "reduced-motion-readout", "scroll-readout", "scroll-progress", "scroll-demo"] {
        assert!(rt.presented(key).is_some(), "showcase missing {key}");
    }
    // Pressing the flying card triggers a matched handover.
    let (x, y) = rt.find("flying-card").expect("flying-card");
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.016);
    assert!(!rt.matched_handovers().is_empty(), "flying-card press should trigger matched handover");
    for _ in 0..180 {
        if !rt.frame(1.0 / 60.0) {
            break;
        }
    }
    // Once settled, idle frames ask for nothing and never enter the VM.
    let calls_before = rt.state.script.metrics().vm_calls;
    for _ in 0..30 {
        assert!(!rt.frame(1.0 / 60.0), "settled showcase must not ask for idle frames");
    }
    assert_eq!(rt.state.script.metrics().vm_calls, calls_before, "idle showcase must not enter VM");
}

#[test]
fn default_list_view_renders_without_errors_and_rests_idle() {
    let script = Script::new("panel", sources());
    assert!(script.error().is_none(), "start: {:?}", script.error());
    let mut rt = Runtime::new(State::new(script), view, (720.0, 640.0), Env::new(true, 1.0));
    rt.poll = Some(|s, now, tx| s.script.poll(now, tx));
    rt.frame(0.0);
    for _ in 0..10 {
        if !rt.frame(1.0 / 60.0) {
            break;
        }
    }
    assert!(rt.state.script.log().is_empty(), "unexpected logs: {:?}", rt.state.script.log());
    assert!(rt.find("pulse").is_some() && rt.find("ping").is_some() && rt.find("tab-toggle").is_some());
    let calls_before = rt.state.script.metrics().vm_calls;
    for _ in 0..30 {
        assert!(!rt.frame(1.0 / 60.0), "idle default panel must not ask for frames");
    }
    assert_eq!(rt.state.script.metrics().vm_calls, calls_before, "idle default panel must not enter VM");
}

#[test]
fn props_showcase_scroll_indicator_and_reduced_motion_readout_update_and_settle() {
    let script = Script::new("panel", sources());
    assert!(script.error().is_none(), "start: {:?}", script.error());
    script.query("setTab", [rune_luau::Data::from("props")]).expect("switch to props tab");
    let mut rt = Runtime::new(State::new(script), view, (720.0, 640.0), Env::new(true, 1.0));
    rt.poll = Some(|s, now, tx| s.script.poll(now, tx));
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.script.log().is_empty(), "unexpected logs: {:?}", rt.state.script.log());

    // Initial reducedMotion readout is off.
    assert!(
        rt.elements().iter().any(|e| e.key.as_deref() == Some("reduced-motion-readout") && e.label.as_deref() == Some("reducedMotion off")),
        "expected reducedMotion off readout"
    );

    // Scrolling the showcase scroll-demo updates the indicator and settles idle.
    rt.update(|_, tx| {
        tx.scroll_to_y("scroll-demo", 40.0);
    });
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    let readout = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("scroll-readout")).expect("scroll-readout");
    assert!(readout.text.as_deref().is_some_and(|t| t.starts_with("40 / ")), "expected scroll readout at y=40, got {:?}", readout.text);

    // Toggling reduced motion updates the readout immediately.
    rt.set_system_reduced_motion(true);
    rt.frame(1.0 / 60.0);
    assert!(
        rt.elements().iter().any(|e| e.key.as_deref() == Some("reduced-motion-readout") && e.label.as_deref() == Some("reducedMotion on")),
        "expected reducedMotion on readout after toggle"
    );

    for _ in 0..60 {
        if !rt.frame(1.0 / 60.0) {
            break;
        }
    }
    let calls_before = rt.state.script.metrics().vm_calls;
    for _ in 0..30 {
        assert!(!rt.frame(1.0 / 60.0), "settled showcase must not ask for frames");
    }
    assert_eq!(rt.state.script.metrics().vm_calls, calls_before, "settled showcase must not enter VM");
}
