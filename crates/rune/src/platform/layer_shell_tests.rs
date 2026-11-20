use super::named_keysym;
use crate::Key;

#[test]
fn special_keysyms_are_named_keys() {
    for (symbol, key) in [
        (0xff14, Key::ScrollLock), (0xff13, Key::Pause), (0xff6b, Key::Pause), (0xff67, Key::ContextMenu),
        (0xff61, Key::PrintScreen), (0xff15, Key::PrintScreen), (0xff63, Key::Insert),
        (0xffbe, Key::Function(1)), (0xffd5, Key::Function(24)), (0xff55, Key::PageUp),
    ] {
        assert_eq!(named_keysym(symbol), Some(key), "keysym {symbol:#x}");
    }
    assert_eq!(named_keysym(0x61), None);
}
