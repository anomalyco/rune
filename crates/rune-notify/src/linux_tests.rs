use super::*;

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
