use super::Prime;

#[test]
fn a_new_window_presumes_itself_shown_for_two_frames_or_until_stopped() {
    let mut prime = Prime::launching();
    assert!(prime.presumes());
    prime.spend();
    assert!(prime.presumes(), "the follow-up frame still presumes");
    prime.spend();
    assert_eq!(prime, Prime::Steady);
    prime.spend();
    assert_eq!(prime, Prime::Steady);
    let mut stopped = Prime::launching();
    stopped.stop();
    assert!(!stopped.presumes());
}
