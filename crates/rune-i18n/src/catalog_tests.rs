use super::Messages;

#[test]
fn get_returns_none_when_key_index_overflows_span_offset() {
    let messages = Messages::new("hi", &[0, 0, 0, 0, 2, 0, 0, 0]);
    assert_eq!(messages.get(0), Some("hi"));
    assert_eq!(messages.get(1), None);
    assert_eq!(messages.get(usize::MAX / 8), None);
    assert_eq!(messages.get(usize::MAX / 8 + 1), None);
    assert_eq!(messages.get(usize::MAX / 4), None);
    assert_eq!(messages.get(usize::MAX), None);
}
