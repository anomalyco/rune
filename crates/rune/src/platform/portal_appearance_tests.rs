use super::*;

#[test]
fn unanswered_portal_stays_none() {
    let slot = AtomicU8::new(UNKNOWN);
    assert_eq!(stored_dark(slot.load(Ordering::Relaxed)), None);
}

#[test]
fn portal_color_scheme_resolves_only_dark_as_dark() {
    assert_eq!(stored_dark(encode_scheme(1)), Some(true));
    assert_eq!(stored_dark(encode_scheme(2)), Some(false));
    assert_eq!(stored_dark(encode_scheme(0)), Some(false));
    assert_eq!(stored_dark(encode_scheme(7)), Some(false));
}

#[test]
fn switching_between_dark_and_no_preference_requests_redraw() {
    let slot = AtomicU8::new(UNKNOWN);
    // First answer arriving as 0 (no preference / GNOME Default) transitions out of UNKNOWN.
    assert!(record_scheme(&slot, 0));
    assert_eq!(stored_dark(slot.load(Ordering::Relaxed)), Some(false));
    // Explicit light keeps the same resolved appearance and triggers no redundant redraw.
    assert!(!record_scheme(&slot, 2));
    assert_eq!(stored_dark(slot.load(Ordering::Relaxed)), Some(false));
    // Switching to dark updates and requests a redraw.
    assert!(record_scheme(&slot, 1));
    assert_eq!(stored_dark(slot.load(Ordering::Relaxed)), Some(true));
    // Switching back from dark to 0 (no preference) leaves dark mode and requests a redraw.
    assert!(record_scheme(&slot, 0));
    assert_eq!(stored_dark(slot.load(Ordering::Relaxed)), Some(false));
    assert!(!record_scheme(&slot, 0));
}

#[test]
fn portal_reduced_motion_follows_spec_and_tracks_live_changes() {
    let slot = AtomicU8::new(0);
    assert!(!has_portal_motion(slot.load(Ordering::Relaxed)));
    assert!(!resolved_reduced_motion(slot.load(Ordering::Relaxed)));

    // 0 (no preference) marks the portal as authoritative without enabling reduced motion.
    assert!(!record_reduced_motion(&slot, 0));
    assert!(has_portal_motion(slot.load(Ordering::Relaxed)));
    assert!(!resolved_reduced_motion(slot.load(Ordering::Relaxed)));

    // 1 enables reduced motion and requests a redraw; repeating 1 requests none.
    assert!(record_reduced_motion(&slot, 1));
    assert!(resolved_reduced_motion(slot.load(Ordering::Relaxed)));
    assert!(!record_reduced_motion(&slot, 1));

    // Unknown values are treated as 0 (no preference) per org.freedesktop.portal.Settings.
    assert!(record_reduced_motion(&slot, 7));
    assert!(!resolved_reduced_motion(slot.load(Ordering::Relaxed)));
    assert!(!record_reduced_motion(&slot, 0));
}

#[test]
fn gnome_portal_enable_animations_drives_reduced_motion_and_combines_with_appearance_key() {
    let slot = AtomicU8::new(0);

    // Desktops with xdg-desktop-portal-gtk or older GNOME expose enable-animations only.
    assert!(record_enable_animations(&slot, false));
    assert!(has_portal_motion(slot.load(Ordering::Relaxed)));
    assert!(resolved_reduced_motion(slot.load(Ordering::Relaxed)));
    assert!(!record_enable_animations(&slot, false));

    // On GNOME 49+, reduced-motion=0 (no preference) does not override enable-animations=false.
    assert!(!record_reduced_motion(&slot, 0));
    assert!(resolved_reduced_motion(slot.load(Ordering::Relaxed)));

    // Re-enabling animations clears reduced motion when reduced-motion is 0.
    assert!(record_enable_animations(&slot, true));
    assert!(!resolved_reduced_motion(slot.load(Ordering::Relaxed)));

    // Turning reduced-motion on overrides enable-animations=true.
    assert!(record_reduced_motion(&slot, 1));
    assert!(resolved_reduced_motion(slot.load(Ordering::Relaxed)));
}

#[test]
fn fallback_reduced_motion_applies_only_until_a_portal_key_answers() {
    let slot = AtomicU8::new(0);

    // Before any portal key answers, the one-shot off-thread fallback sets the state.
    assert!(record_fallback_reduced_motion(&slot, true));
    assert!(!has_portal_motion(slot.load(Ordering::Relaxed)));
    assert!(resolved_reduced_motion(slot.load(Ordering::Relaxed)));
    assert!(!record_fallback_reduced_motion(&slot, true));

    // A later portal SettingChanged supersedes the fallback and triggers a redraw if changed.
    assert!(record_reduced_motion(&slot, 0));
    assert!(has_portal_motion(slot.load(Ordering::Relaxed)));
    assert!(!resolved_reduced_motion(slot.load(Ordering::Relaxed)));

    // Subsequent fallback writes do not override the portal.
    assert!(!record_fallback_reduced_motion(&slot, true));
    assert!(!resolved_reduced_motion(slot.load(Ordering::Relaxed)));
}

#[cfg(all(target_os = "linux", feature = "dbus"))]
#[test]
fn reads_the_scheme_however_deeply_the_portal_wraps_it() {
    use zbus::zvariant::Value;
    assert_eq!(dbus::scheme(&Value::U32(2)), Some(2));
    assert_eq!(dbus::scheme(&Value::Value(Box::new(Value::U32(1)))), Some(1));
    assert_eq!(dbus::scheme(&Value::Value(Box::new(Value::Value(Box::new(Value::U32(2)))))), Some(2));
    assert_eq!(dbus::scheme(&Value::Str("prefer-dark".into())), None);
}

#[cfg(all(target_os = "linux", feature = "dbus"))]
#[test]
fn reads_enable_animations_however_deeply_the_portal_wraps_it() {
    use zbus::zvariant::Value;
    assert_eq!(dbus::bool_value(&Value::Bool(false)), Some(false));
    assert_eq!(dbus::bool_value(&Value::Value(Box::new(Value::Bool(true)))), Some(true));
    assert_eq!(dbus::bool_value(&Value::Value(Box::new(Value::Value(Box::new(Value::Bool(false)))))), Some(false));
    assert_eq!(dbus::bool_value(&Value::U32(0)), None);
}

#[test]
fn the_last_runs_scheme_stands_until_the_portal_answers_and_redraws_only_when_it_differs() {
    let slot = AtomicU8::new(UNKNOWN);
    assert!(seed_scheme(&slot, false));
    assert_eq!(stored_dark(slot.load(Ordering::Relaxed)), Some(false));
    // The portal agrees: nothing changed, no window redraws.
    assert!(!record_scheme(&slot, 2));
    // The desktop went dark while the app was closed: the answer replaces the seed, and redraws.
    let slot = AtomicU8::new(UNKNOWN);
    seed_scheme(&slot, false);
    assert!(record_scheme(&slot, 1));
    assert_eq!(stored_dark(slot.load(Ordering::Relaxed)), Some(true));
}

#[test]
fn a_portal_answer_is_never_replaced_by_the_last_runs_scheme() {
    let slot = AtomicU8::new(UNKNOWN);
    record_scheme(&slot, 1);
    assert!(!seed_scheme(&slot, false));
    assert_eq!(stored_dark(slot.load(Ordering::Relaxed)), Some(true));
}

#[test]
fn the_last_runs_reduced_motion_stands_until_a_portal_key_or_the_fallback_answers() {
    let slot = AtomicU8::new(0);
    seed_reduced_motion(&slot, true);
    assert!(resolved_reduced_motion(slot.load(Ordering::Relaxed)));
    // The portal's key says motion is fine now: it decides, and the change is reported.
    assert!(record_reduced_motion(&slot, 0));
    assert!(!resolved_reduced_motion(slot.load(Ordering::Relaxed)));
    // No portal key answers: the fallback's read replaces the seed.
    let slot = AtomicU8::new(0);
    seed_reduced_motion(&slot, true);
    assert!(record_fallback_reduced_motion(&slot, false));
    assert!(!resolved_reduced_motion(slot.load(Ordering::Relaxed)));
    // A seed of "not reduced" stores nothing, and never clears what a portal key said.
    let slot = AtomicU8::new(0);
    record_enable_animations(&slot, false);
    seed_reduced_motion(&slot, false);
    seed_reduced_motion(&slot, true);
    assert!(resolved_reduced_motion(slot.load(Ordering::Relaxed)));
    assert!(record_enable_animations(&slot, true));
    assert!(!resolved_reduced_motion(slot.load(Ordering::Relaxed)));
}
