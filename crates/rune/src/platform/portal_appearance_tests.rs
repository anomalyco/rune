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

#[cfg(all(target_os = "linux", feature = "dbus"))]
#[test]
fn reads_the_scheme_however_deeply_the_portal_wraps_it() {
    use zbus::zvariant::Value;
    assert_eq!(dbus::scheme(&Value::U32(2)), Some(2));
    assert_eq!(dbus::scheme(&Value::Value(Box::new(Value::U32(1)))), Some(1));
    assert_eq!(dbus::scheme(&Value::Value(Box::new(Value::Value(Box::new(Value::U32(2)))))), Some(2));
    assert_eq!(dbus::scheme(&Value::Str("prefer-dark".into())), None);
}
