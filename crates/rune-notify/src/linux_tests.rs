use super::*;
use std::ffi::OsStr;

const ALL: Caps = Caps { actions: true, markup: true };

#[test]
fn caps_come_from_the_server_list() {
    let caps = Caps::from_list(&["body".into(), "actions".into(), "body-markup".into()]);
    assert_eq!(caps, ALL);
    assert_eq!(Caps::from_list(&["body".into()]), Caps::default());
}

#[test]
fn actions_list_the_default_then_buttons_but_no_text_boxes() {
    let n = Notification::new("m-1", "New mail").action("archive", "Archive").reply("answer", "Reply…", "Send");
    assert_eq!(actions_for(&n, ALL), ["default", "", "archive", "Archive"]);
    assert!(actions_for(&n, Caps { actions: false, markup: true }).is_empty());
}

#[test]
fn body_is_escaped_only_for_a_markup_server() {
    assert_eq!(body_for("a <b> & c", ALL), "a &lt;b&gt; &amp; c");
    assert_eq!(body_for("a <b> & c", Caps::default()), "a <b> & c");
}

#[test]
fn a_click_carries_the_posted_info_its_button_and_token() {
    let info = vec![("message".to_owned(), "42".to_owned())];
    let click = click_for("m-1", &info, "default", None);
    assert_eq!(click, Click { id: "m-1".into(), info: info.clone() });
    assert_eq!(click.action(), None);
    let click = click_for("m-1", &info, "archive", Some("tok".into()));
    assert_eq!(click.get("message"), Some("42"));
    assert_eq!(click.action(), Some("archive"));
    assert_eq!(click.activation_token(), Some("tok"));
}

#[test]
fn flatpak_detection_checks_env_and_marker_file() {
    assert!(!in_flatpak(None, false));
    assert!(!in_flatpak(Some(OsStr::new("")), false));
    assert!(in_flatpak(Some(OsStr::new("com.example.App")), false));
    assert!(in_flatpak(None, true));
    assert!(in_flatpak(Some(OsStr::new("")), true));
}

#[test]
fn fdo_sound_defaults_to_suppress_and_opts_into_sound_name() {
    assert_eq!(fdo_sound(false), FdoSound::Suppress);
    assert_eq!(fdo_sound(true), FdoSound::Name("message-new-instant"));
}

#[test]
fn portal_sound_is_gated_on_portal_v2() {
    assert_eq!(portal_sound(1, false), None);
    assert_eq!(portal_sound(1, true), None);
    assert_eq!(portal_sound(2, false), Some("silent"));
    assert_eq!(portal_sound(2, true), Some("default"));
    assert_eq!(portal_sound(3, true), Some("default"));
}

#[test]
fn portal_payload_carries_plain_body_buttons_and_version_gated_sound() {
    let n = Notification::new("m-2", "New message")
        .body("a <b> & c")
        .action("archive", "Archive")
        .action("app.open", "Open")
        .reply("answer", "Reply…", "Send")
        .sound(true);

    let v1 = portal_payload(&n, 1);
    assert_eq!(v1.id, "m-2");
    assert_eq!(v1.title, "New message");
    assert_eq!(v1.body, Some("a <b> & c"));
    assert_eq!(v1.default_action, "default");
    assert_eq!(v1.sound, None);
    assert_eq!(
        v1.buttons,
        vec![
            PortalButton { label: "Archive", action: Cow::Borrowed("archive") },
            PortalButton { label: "Open", action: Cow::Owned("rune.action.app.open".into()) },
        ]
    );
    assert_eq!(portal_action_unwire(&v1.buttons[0].action), "archive");
    assert_eq!(portal_action_unwire(&v1.buttons[1].action), "app.open");

    let v2 = portal_payload(&n, 2);
    assert_eq!(v2.sound, Some("default"));

    let silent = Notification::new("m-3", "Quiet");
    let v2_silent = portal_payload(&silent, 2);
    assert_eq!(v2_silent.body, None);
    assert!(v2_silent.buttons.is_empty());
    assert_eq!(v2_silent.sound, Some("silent"));
}

#[test]
fn portal_shown_replaces_removes_and_bounds_entries() {
    let mut shown = PortalShown::with_cap(2);
    shown.record("a".into(), vec![("k".into(), "1".into())]);
    shown.record("b".into(), vec![("k".into(), "2".into())]);
    // Updating "a" refreshes it so "b" is evicted next.
    shown.record("a".into(), vec![("k".into(), "1b".into())]);
    shown.record("c".into(), vec![("k".into(), "3".into())]);
    assert_eq!(shown.info("a"), &[("k".to_owned(), "1b".to_owned())]);
    assert!(shown.info("b").is_empty());
    assert_eq!(shown.info("c"), &[("k".to_owned(), "3".to_owned())]);
    shown.remove("a");
    assert!(shown.info("a").is_empty());
}

#[cfg(target_os = "linux")]
#[test]
fn linux_dbus_hints_and_portal_vardict_encode_expected_keys() {
    use crate::linux::{fdo_hints, portal_activation_token, portal_notification};
    use std::collections::HashMap;
    use zbus::zvariant::{OwnedValue, Value};

    let silent = Notification::new("n-1", "Silent");
    let hints = fdo_hints(&silent, Some("com.example.App"));
    assert_eq!(bool::try_from(hints.get("suppress-sound").unwrap()), Ok(true));
    assert!(!hints.contains_key("sound-name"));
    assert_eq!(<&str>::try_from(hints.get("desktop-entry").unwrap()), Ok("com.example.App"));

    let loud = Notification::new("n-2", "Loud").body("Ping").action("open", "Open").sound(true);
    let hints = fdo_hints(&loud, None);
    assert!(!hints.contains_key("suppress-sound"));
    assert_eq!(<&str>::try_from(hints.get("sound-name").unwrap()), Ok("message-new-instant"));
    assert!(!hints.contains_key("desktop-entry"));

    let v1 = portal_notification(&loud, 1);
    assert_eq!(<&str>::try_from(v1.get("title").unwrap()), Ok("Loud"));
    assert_eq!(<&str>::try_from(v1.get("body").unwrap()), Ok("Ping"));
    assert_eq!(<&str>::try_from(v1.get("default-action").unwrap()), Ok("default"));
    assert_eq!(v1.get("buttons").unwrap().value_signature(), "aa{sv}");
    assert!(!v1.contains_key("sound"));

    let v2 = portal_notification(&loud, 2);
    assert_eq!(<&str>::try_from(v2.get("sound").unwrap()), Ok("default"));
    let v2_silent = portal_notification(&silent, 2);
    assert_eq!(<&str>::try_from(v2_silent.get("sound").unwrap()), Ok("silent"));
    assert!(!v2_silent.contains_key("body"));
    assert!(!v2_silent.contains_key("buttons"));

    let mut platform_data: HashMap<String, OwnedValue> = HashMap::new();
    platform_data.insert("activation-token".into(), Value::from("xdg-tok-1").try_to_owned().unwrap());
    let params = vec![
        Value::from("ignored-target").try_to_owned().unwrap(),
        OwnedValue::try_from(Value::from(platform_data)).unwrap(),
    ];
    assert_eq!(portal_activation_token(&params).as_deref(), Some("xdg-tok-1"));
    assert_eq!(portal_activation_token(&[]), None);
}
