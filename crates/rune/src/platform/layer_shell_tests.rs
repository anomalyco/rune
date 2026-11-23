use super::{feed_compose, named_keysym, read_keymap_fd};
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

#[test]
fn wait_for_configure_times_out_on_silent_socket_and_errors_on_hangup() -> std::io::Result<()> {
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};
    use wayland_client::Connection;
    use crate::platform::layer_shell_machine::configure_wait::Outcome;

    let (client, _server) = UnixStream::pair()?;
    let Ok(conn) = Connection::from_socket(client) else { return Ok(()); };
    let mut queue = conn.new_event_queue::<super::State>();
    let mut state = super::State::default();
    let started = Instant::now();
    let outcome = super::wait_for_configure(&mut queue, &mut state, Duration::from_millis(40));
    assert_eq!(outcome, Outcome::TimedOut);
    assert!(started.elapsed() >= Duration::from_millis(30));
    assert!(started.elapsed() < Duration::from_secs(2));

    let (client, server) = UnixStream::pair()?;
    drop(server);
    let Ok(conn) = Connection::from_socket(client) else { return Ok(()); };
    let mut queue = conn.new_event_queue::<super::State>();
    let mut state = super::State::default();
    let started = Instant::now();
    let outcome = super::wait_for_configure(&mut queue, &mut state, Duration::from_secs(2));
    assert_eq!(outcome, Outcome::Error);
    assert!(started.elapsed() < Duration::from_secs(1));
    Ok(())
}

#[test]
fn xkb_compose_feeds_dead_keys_and_resets_after_composed_or_cancelled() {
    use xkbcommon::xkb;
    use crate::platform::layer_shell_machine::keyboard::ComposeOutcome;

    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    let table_src = concat!(
        "<dead_acute> <a> : \"á\" aacute\n",
        "<dead_acute> <A> : \"Á\" Aacute\n",
        "<dead_acute> <dead_acute> : \"´\" acute\n",
    );
    let table = xkb::compose::Table::new_from_buffer(
        &context,
        table_src.as_bytes(),
        "en_US.UTF-8",
        xkb::compose::FORMAT_TEXT_V1,
        xkb::compose::COMPILE_NO_FLAGS,
    )
    .expect("in-memory compose table");
    let mut compose = xkb::compose::State::new(&table, xkb::compose::STATE_NO_FLAGS);

    let dead_acute = xkb::Keysym::new(xkb::keysyms::KEY_dead_acute);
    let shift = xkb::Keysym::new(xkb::keysyms::KEY_Shift_L);
    let key_a = xkb::Keysym::new('a' as u32);
    let key_cap_a = xkb::Keysym::new('A' as u32);
    let key_q = xkb::Keysym::new('q' as u32);

    // Dead key + a -> Composing then Composed("á"), then plain a -> Nothing.
    assert_eq!(feed_compose(&mut compose, dead_acute), ComposeOutcome::Composing);
    assert_eq!(feed_compose(&mut compose, key_a), ComposeOutcome::Composed(Some("á".into())));
    assert_eq!(feed_compose(&mut compose, key_a), ComposeOutcome::Nothing);

    // Modifier between dead key and letter is ignored by xkb compose and keeps Composing alive.
    assert_eq!(feed_compose(&mut compose, dead_acute), ComposeOutcome::Composing);
    assert_eq!(feed_compose(&mut compose, shift), ComposeOutcome::Nothing);
    assert_eq!(feed_compose(&mut compose, key_cap_a), ComposeOutcome::Composed(Some("Á".into())));

    // Double dead key emits the standalone acute accent.
    assert_eq!(feed_compose(&mut compose, dead_acute), ComposeOutcome::Composing);
    assert_eq!(feed_compose(&mut compose, dead_acute), ComposeOutcome::Composed(Some("´".into())));

    // Invalid follow-up cancels compose and leaves the next key uncomposed.
    assert_eq!(feed_compose(&mut compose, dead_acute), ComposeOutcome::Composing);
    assert_eq!(feed_compose(&mut compose, key_q), ComposeOutcome::Cancelled);
    assert_eq!(feed_compose(&mut compose, key_a), ComposeOutcome::Nothing);

    // Explicit reset (focus loss / keymap change / command chord) clears a pending dead key.
    assert_eq!(feed_compose(&mut compose, dead_acute), ComposeOutcome::Composing);
    compose.reset();
    assert_eq!(feed_compose(&mut compose, key_a), ComposeOutcome::Nothing);
}
