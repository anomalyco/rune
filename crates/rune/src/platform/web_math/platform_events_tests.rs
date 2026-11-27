use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use serde_json::json;
use crate::{App, AppSpec, Cx, El, Env, Runtime, el};
use crate::app::SharedCore;
use super::*;

#[derive(Default)]
struct State {
    views: Arc<AtomicUsize>,
    focus_events: Vec<bool>,
    visibility_events: Vec<bool>,
}

fn test_runtime() -> (Runtime<State>, Arc<SharedCore>, Arc<AtomicUsize>) {
    let views = Arc::new(AtomicUsize::new(0));
    let counter = views.clone();
    let spec = AppSpec::new(
        "web-platform-events",
        (390.0, 844.0),
        State::default,
        |s: &State, _: &mut Cx<State>| -> El {
            s.views.fetch_add(1, Ordering::Relaxed);
            el().size(100.0, 40.0)
        },
    );
    let app = App::new(spec)
        .init(move || State { views: counter.clone(), ..Default::default() })
        .on_focus(|s, focused, _| s.focus_events.push(focused))
        .on_visibility(|s, visible, _| s.visibility_events.push(visible));
    let (rt, shared, _, _, _) = super::super::start_web_runtime(
        app,
        (390.0, 844.0),
        Env::default(),
        Arc::new(|| {}),
        Arc::new(|| {}),
    );
    shared.core.set_visible(true);
    (rt, shared.core.clone(), views)
}

#[test]
fn motion_contrast_languages_and_focus_short_circuit_unchanged_values() {
    let (mut rt, _, _) = test_runtime();

    // Defaults: reduced_motion = false, contrast = None.
    assert!(!apply_motion(rt.core_mut(), &json!({ "kind": "motion", "reduced": false })));
    assert!(apply_motion(rt.core_mut(), &json!({ "kind": "motion", "reduced": true })));
    assert!(!apply_motion(rt.core_mut(), &json!({ "kind": "motion", "reduced": true })));
    assert!(apply_motion(rt.core_mut(), &json!({ "kind": "motion", "reduced": false })));

    let colors = json!([
        [0, 0, 0], [255, 255, 255], [0, 120, 215], [255, 255, 255],
        [128, 128, 128], [32, 32, 32], [255, 255, 255], [80, 160, 255]
    ]);
    assert!(!apply_contrast(rt.core_mut(), &json!({ "kind": "contrast", "colors": null })));
    assert!(apply_contrast(rt.core_mut(), &json!({ "kind": "contrast", "colors": colors })));
    assert!(!apply_contrast(rt.core_mut(), &json!({ "kind": "contrast", "colors": colors })));
    assert!(apply_contrast(rt.core_mut(), &json!({ "kind": "contrast", "colors": null })));
    assert!(!apply_contrast(rt.core_mut(), &json!({ "kind": "contrast", "colors": null })));

    assert!(apply_languages(rt.core_mut(), &json!({ "kind": "languages", "languages": ["fr-FR", "en-US"] })));
    assert!(!apply_languages(rt.core_mut(), &json!({ "kind": "languages", "languages": [" fr-FR ", "", "en-US", "fr-FR"] })));
    assert!(apply_languages(rt.core_mut(), &json!({ "kind": "languages", "languages": ["de-DE"] })));

    // Focus transitions None -> Some(true) once, then no-ops on duplicate true, then flips to false.
    {
        let (core, mut app) = rt.split_mut();
        assert_eq!(
            apply_focus(core, &mut app, &json!({ "kind": "focus", "focused": true })),
            FocusChange { focused: true, changed: true }
        );
        assert_eq!(
            apply_focus(core, &mut app, &json!({ "kind": "focus", "focused": true })),
            FocusChange { focused: true, changed: false }
        );
        assert_eq!(
            apply_focus(core, &mut app, &json!({ "kind": "focus", "focused": false })),
            FocusChange { focused: false, changed: true }
        );
        assert_eq!(
            apply_focus(core, &mut app, &json!({ "kind": "focus", "focused": false })),
            FocusChange { focused: false, changed: false }
        );
    }
    assert_eq!(rt.state.focus_events, vec![true, false]);
}

#[test]
fn visibility_short_circuits_unchanged_state_and_preserves_transitions() {
    let (mut rt, shared, _) = test_runtime();
    rt.frame(0.0);

    // Initial focus = true, visible = true.
    {
        let (core, mut app) = rt.split_mut();
        assert_eq!(
            apply_focus(core, &mut app, &json!({ "kind": "focus", "focused": true })),
            FocusChange { focused: true, changed: true }
        );
        assert_eq!(
            apply_visibility(core, &mut app, &shared, &json!({ "kind": "visibility", "visible": true, "focused": true })),
            VisibilityAction::NoOp
        );
        // Unfocused -> visible+focused with same visibility still triggers focus gain redraw.
        assert_eq!(
            apply_focus(core, &mut app, &json!({ "kind": "focus", "focused": false })),
            FocusChange { focused: false, changed: true }
        );
        assert_eq!(
            apply_visibility(core, &mut app, &shared, &json!({ "kind": "visibility", "visible": true, "focused": true })),
            VisibilityAction::Redraw
        );
        // Repeating visible+focused now no-ops.
        assert_eq!(
            apply_visibility(core, &mut app, &shared, &json!({ "kind": "visibility", "visible": true, "focused": true })),
            VisibilityAction::NoOp
        );
        // Visible -> hidden returns Hide; duplicate hidden returns NoOp.
        assert_eq!(
            apply_visibility(core, &mut app, &shared, &json!({ "kind": "visibility", "visible": false, "focused": false })),
            VisibilityAction::Hide
        );
        assert!(!shared.is_visible());
        assert_eq!(
            apply_visibility(core, &mut app, &shared, &json!({ "kind": "visibility", "visible": false, "focused": false })),
            VisibilityAction::NoOp
        );
        // Hidden -> visible returns Show and updates focus when focused.
        assert_eq!(
            apply_visibility(core, &mut app, &shared, &json!({ "kind": "visibility", "visible": true, "focused": true })),
            VisibilityAction::Show
        );
        assert!(shared.is_visible());
    }
    assert_eq!(rt.state.visibility_events, vec![false, true]);
}

#[test]
fn apply_init_decides_commit_noop_or_hidden_across_startup_states() {
    let (mut rt, shared, views) = test_runtime();
    let init = json!({
        "kind": "init",
        "h": 844,
        "insets": [0, 0, 0, 0],
        "visual": [0, 844, 844],
        "reduced": true,
        "hover": false,
        "coarse": true,
        "colors": [
            [0, 0, 0], [255, 255, 255], [0, 120, 215], [255, 255, 255],
            [128, 128, 128], [32, 32, 32], [255, 255, 255], [80, 160, 255]
        ],
        "focused": true,
        "visible": true
    });

    assert_eq!(parse_primary_pointer(&init), PrimaryPointer::Touch);
    assert_eq!(parse_visual_viewport(&init["visual"]), Some([0.0, 844.0, 844.0]));
    let outcome = {
        let (core, mut app) = rt.split_mut();
        apply_init(core, &mut app, &shared, &init, true, true)
    };
    assert_eq!(
        outcome,
        InitOutcome {
            action: InitAction::CommitFrame,
            focus: FocusChange { focused: true, changed: true },
            clear_presented: false,
        }
    );
    assert!(rt.core().host.system_reduced_motion());
    assert!(rt.core().system_contrast().is_some());
    assert_eq!(rt.state.focus_events, vec![true]);

    rt.frame(0.0);
    assert_eq!(views.load(Ordering::Relaxed), 1);

    // Repeating the same init snapshot with unchanged viewport and pointer returns NoOp.
    let repeat = {
        let (core, mut app) = rt.split_mut();
        apply_init(core, &mut app, &shared, &init, false, false)
    };
    assert_eq!(
        repeat,
        InitOutcome {
            action: InitAction::NoOp,
            focus: FocusChange { focused: true, changed: false },
            clear_presented: false,
        }
    );

    // Viewport-only or pointer-only changes still request CommitFrame.
    let viewport_only = {
        let (core, mut app) = rt.split_mut();
        apply_init(core, &mut app, &shared, &init, true, false)
    };
    assert_eq!(viewport_only.action, InitAction::CommitFrame);
    let pointer_only = {
        let (core, mut app) = rt.split_mut();
        apply_init(core, &mut app, &shared, &init, false, true)
    };
    assert_eq!(pointer_only.action, InitAction::CommitFrame);

    // Hidden startup returns CommitHidden, clears presented cache, and does not fire on_visibility before frame 0.
    let (mut hidden_rt, hidden_shared, hidden_views) = test_runtime();
    let hidden_init = json!({
        "kind": "init",
        "h": 844,
        "insets": [0, 0, 0, 0],
        "visual": [0, 844, 844],
        "reduced": false,
        "hover": true,
        "coarse": false,
        "colors": null,
        "focused": false,
        "visible": false
    });
    assert_eq!(parse_primary_pointer(&hidden_init), PrimaryPointer::Hover);
    let hidden_outcome = {
        let (core, mut app) = hidden_rt.split_mut();
        apply_init(core, &mut app, &hidden_shared, &hidden_init, true, false)
    };
    assert_eq!(
        hidden_outcome,
        InitOutcome {
            action: InitAction::CommitHidden,
            focus: FocusChange { focused: false, changed: true },
            clear_presented: true,
        }
    );
    assert!(!hidden_shared.is_visible());
    assert!(hidden_rt.state.visibility_events.is_empty(), "hidden startup before frame 0 must not fire on_visibility");
    hidden_rt.frame(0.0);
    assert_eq!(hidden_views.load(Ordering::Relaxed), 1);

    // Becoming visible from a hidden state via init returns CommitFrame and clears presented cache.
    let show_init = json!({
        "kind": "init",
        "reduced": false,
        "colors": null,
        "focused": false,
        "visible": true
    });
    let show_outcome = {
        let (core, mut app) = hidden_rt.split_mut();
        apply_init(core, &mut app, &hidden_shared, &show_init, false, false)
    };
    assert_eq!(
        show_outcome,
        InitOutcome {
            action: InitAction::CommitFrame,
            focus: FocusChange { focused: false, changed: false },
            clear_presented: true,
        }
    );
    assert!(hidden_shared.is_visible());
}
