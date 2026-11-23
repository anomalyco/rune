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

#[test]
fn arming_prime_after_steady_presumes_shown_for_two_frames() {
    let mut prime = Prime::Steady;
    assert!(!prime.presumes());
    prime.arm();
    assert!(prime.presumes(), "ordering a panel in arms prime");
    prime.spend();
    assert!(prime.presumes(), "the follow-up frame still presumes");
    prime.spend();
    assert_eq!(prime, Prime::Steady);
    prime.arm();
    prime.stop();
    assert_eq!(prime, Prime::Steady);
}
