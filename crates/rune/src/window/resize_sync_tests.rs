use super::ResizeSync;

#[test]
fn resize_sync_defaults_to_auto_and_auto_resolves_to_off() {
    assert_eq!(ResizeSync::default(), ResizeSync::Auto);
    assert!(!ResizeSync::Auto.is_enabled(), "Auto must resolve to off today");
    assert!(!ResizeSync::Off.is_enabled());
    assert!(ResizeSync::On.is_enabled());
}

#[test]
fn parse_env_recognizes_supported_values_and_rejects_unknowns() {
    for raw in ["1", " 1 ", "true", "TRUE", "True", "yes", "YES", "on", "ON", " on "] {
        assert_eq!(ResizeSync::parse_env(raw), Some(ResizeSync::On), "raw={raw:?}");
    }
    for raw in ["0", " 0 ", "false", "FALSE", "False", "no", "NO", "off", "OFF", " off "] {
        assert_eq!(ResizeSync::parse_env(raw), Some(ResizeSync::Off), "raw={raw:?}");
    }
    for raw in ["auto", "AUTO", "Auto", " auto "] {
        assert_eq!(ResizeSync::parse_env(raw), Some(ResizeSync::Auto), "raw={raw:?}");
    }
    for raw in ["", "   ", "2", "only", "sync", "flip", "discard"] {
        assert_eq!(ResizeSync::parse_env(raw), None, "raw={raw:?}");
    }
}

#[test]
fn env_override_precedence_and_kill_switch_over_builder() {
    for configured in [ResizeSync::Auto, ResizeSync::On, ResizeSync::Off] {
        assert_eq!(configured.resolve(None), configured);
        assert_eq!(configured.resolve(Some("")), configured);
        assert_eq!(configured.resolve(Some("unknown")), configured);
        for on in ["1", "true", "yes", "on"] {
            let resolved = configured.resolve(Some(on));
            assert_eq!(resolved, ResizeSync::On);
            assert!(resolved.is_enabled());
        }
        // Kill switch: 0/false/no/off disables even a window built with ResizeSync::On.
        for off in ["0", "false", "no", "off"] {
            let resolved = configured.resolve(Some(off));
            assert_eq!(resolved, ResizeSync::Off);
            assert!(!resolved.is_enabled());
        }
        let resolved = configured.resolve(Some("auto"));
        assert_eq!(resolved, ResizeSync::Auto);
        assert!(!resolved.is_enabled());
    }
}

#[test]
fn per_window_modes_resolve_independently_in_one_process() {
    let win_on = ResizeSync::On;
    let win_off = ResizeSync::Off;
    let win_auto = ResizeSync::Auto;

    // With no env override, each window keeps its own configured mode.
    assert!(win_on.resolve(None).is_enabled());
    assert!(!win_off.resolve(None).is_enabled());
    assert!(!win_auto.resolve(None).is_enabled());

    // Unrecognized env leaves each window's configured mode intact.
    assert!(win_on.resolve(Some("bogus")).is_enabled());
    assert!(!win_off.resolve(Some("bogus")).is_enabled());
    assert!(!win_auto.resolve(Some("bogus")).is_enabled());

    // Kill switch overrides every window, including On.
    assert!(!win_on.resolve(Some("0")).is_enabled());
    assert!(!win_off.resolve(Some("0")).is_enabled());
    assert!(!win_auto.resolve(Some("0")).is_enabled());

    // Env On enables every window, including Off and Auto.
    assert!(win_on.resolve(Some("1")).is_enabled());
    assert!(win_off.resolve(Some("1")).is_enabled());
    assert!(win_auto.resolve(Some("1")).is_enabled());
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "invoked by effective_reads_process_env_override_without_mutating_parent"]
fn effective_env_child() {
    let expected = match crate::env::var("RUNE_TEST_RESIZE_SYNC_CASE").as_deref() {
        Some("unset") | Some("bogus") => (ResizeSync::Auto, ResizeSync::On, ResizeSync::Off),
        Some("one") | Some("on") | Some("true") | Some("yes") => (ResizeSync::On, ResizeSync::On, ResizeSync::On),
        Some("zero") | Some("off") | Some("false") | Some("no") => (ResizeSync::Off, ResizeSync::Off, ResizeSync::Off),
        Some("auto") => (ResizeSync::Auto, ResizeSync::Auto, ResizeSync::Auto),
        other => panic!("unexpected test case: {other:?}"),
    };
    assert_eq!(ResizeSync::Auto.effective(), expected.0);
    assert_eq!(ResizeSync::On.effective(), expected.1);
    assert_eq!(ResizeSync::Off.effective(), expected.2);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn effective_reads_process_env_override_without_mutating_parent() {
    let exe = std::env::current_exe().expect("test binary path");
    for (case, env_value) in [
        ("unset", None),
        ("one", Some("1")),
        ("on", Some("on")),
        ("true", Some("TRUE")),
        ("yes", Some("yes")),
        ("zero", Some("0")),
        ("off", Some("off")),
        ("false", Some("false")),
        ("no", Some("NO")),
        ("auto", Some("auto")),
        ("bogus", Some("bogus")),
    ] {
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("--exact")
            .arg("window::resize_sync::tests::effective_env_child")
            .arg("--ignored")
            .arg("--nocapture")
            .env("RUNE_TEST_RESIZE_SYNC_CASE", case);
        match env_value {
            Some(v) => { cmd.env(ResizeSync::ENV_VAR, v); }
            None => { cmd.env_remove(ResizeSync::ENV_VAR); }
        }
        let output = cmd.output().expect("spawn resize_sync env child");
        assert!(
            output.status.success(),
            "case={case} env={env_value:?} failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}
