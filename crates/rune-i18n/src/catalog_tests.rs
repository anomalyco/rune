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

#[test]
fn get_returns_none_when_span_end_overflows_u32() {
    const fn entry(start: u32, len: u32) -> [u8; 8] {
        let s = start.to_le_bytes();
        let l = len.to_le_bytes();
        [s[0], s[1], s[2], s[3], l[0], l[1], l[2], l[3]]
    }
    static SPANS: [[u8; 8]; 7] = [
        entry(0, 2),
        entry(1, u32::MAX),
        entry(u32::MAX - 1, 2),
        entry(u32::MAX - 1, 3),
        entry(0x8000_0000, 0x8000_0000),
        entry(u32::MAX, 1),
        entry(1, u32::MAX - 1),
    ];
    let messages = Messages::new("hi", SPANS.as_flattened());
    assert_eq!(messages.get(0), Some("hi"));
    assert_eq!(messages.get(1), None);
    assert_eq!(messages.get(2), None);
    assert_eq!(messages.get(3), None);
    assert_eq!(messages.get(4), None);
    assert_eq!(messages.get(5), None);
    assert_eq!(messages.get(6), None);
}
