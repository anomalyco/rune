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

#[test]
fn a_locale_left_for_run_time_takes_its_blob_once_and_refuses_another_builds() {
    use super::{Catalog, Key, LocaleInfo};
    static LOCALES: [LocaleInfo; 2] = [LocaleInfo::new("en", "en", "English", false), LocaleInfo::new("de", "de", "Deutsch", false)];
    static MESSAGES: [Messages; 2] = [Messages::new("hi", &[0, 0, 0, 0, 2, 0, 0, 0]), Messages::later()];
    static CATALOG: Catalog = Catalog::new(&["greeting"], &[], &[], &LOCALES, &MESSAGES);
    let key = Key::named(CATALOG.keys(), "greeting");

    assert!(!CATALOG.present(1));
    assert!(CATALOG.load_blob(1, "hallo", &[0, 0, 0, 0, 5]).is_err());
    assert!(!CATALOG.present(1));
    assert_eq!(CATALOG.load_blob(1, "hallo", &[0, 0, 0, 0, 5, 0, 0, 0]), Ok(()));
    assert_eq!(MESSAGES[1].get(key.index()), Some("hallo"));
    // Here already: a second blob changes nothing.
    assert_eq!(CATALOG.load_blob(1, "moin", &[0, 0, 0, 0, 4, 0, 0, 0]), Ok(()));
    assert_eq!(MESSAGES[1].get(key.index()), Some("hallo"));
    assert!(CATALOG.load_blob(2, "", &[]).is_err());
}