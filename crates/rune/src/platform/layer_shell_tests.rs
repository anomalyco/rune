use super::{named_keysym, read_keymap_fd};
use crate::platform::layer_shell_machine::keyboard::MAX_KEYMAP_BYTES;
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

#[test]
fn read_keymap_fd_ignores_and_preserves_shared_file_offset() -> std::io::Result<()> {
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Seek, SeekFrom, Write};

    let payload = b"xkb_keymap { xkb_keycodes {}; };\0";
    let size = payload.len() as u32;
    let path = std::env::temp_dir().join(format!("rune-keymap-fd-test-{}", std::process::id()));
    {
        let mut writer = OpenOptions::new().create(true).truncate(true).write(true).open(&path)?;
        writer.write_all(payload)?;
    }
    let mut shared = File::open(&path)?;
    std::fs::remove_file(&path)?;

    // First bind reads the complete keymap without moving the shared open file description's offset.
    let first = read_keymap_fd(shared.try_clone()?.into(), size);
    assert_eq!(first.as_deref(), Some("xkb_keymap { xkb_keycodes {}; };"));
    assert_eq!(shared.stream_position()?, 0);

    // Second bind on the same shared open file description still reads the full keymap.
    let second = read_keymap_fd(shared.try_clone()?.into(), size);
    assert_eq!(second.as_deref(), Some("xkb_keymap { xkb_keycodes {}; };"));
    assert_eq!(shared.stream_position()?, 0);

    // Another client advancing the shared offset to EOF does not break a later bind.
    let mut drained = Vec::new();
    shared.read_to_end(&mut drained)?;
    assert_eq!(shared.stream_position()?, u64::from(size));
    let after_eof = read_keymap_fd(shared.try_clone()?.into(), size);
    assert_eq!(after_eof.as_deref(), Some("xkb_keymap { xkb_keycodes {}; };"));
    assert_eq!(shared.stream_position()?, u64::from(size));

    // A mid-file offset is likewise ignored and preserved.
    shared.seek(SeekFrom::Start(5))?;
    let after_seek = read_keymap_fd(shared.try_clone()?.into(), size);
    assert_eq!(after_seek.as_deref(), Some("xkb_keymap { xkb_keycodes {}; };"));
    assert_eq!(shared.stream_position()?, 5);

    // Empty, truncated, and over-cap sizes are rejected cleanly.
    assert_eq!(read_keymap_fd(shared.try_clone()?.into(), 0), None);
    assert_eq!(read_keymap_fd(shared.try_clone()?.into(), size + 1), None);
    assert_eq!(read_keymap_fd(shared.try_clone()?.into(), MAX_KEYMAP_BYTES + 1), None);

    Ok(())
}
