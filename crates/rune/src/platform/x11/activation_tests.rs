use super::*;

/// Reassembles 20-byte startup-notification chunks up to the first NUL byte, checking that the
/// first chunk uses `_NET_STARTUP_INFO_BEGIN` and every following chunk uses `_NET_STARTUP_INFO`.
fn reassemble_chunks(chunks: &[StartupChunk]) -> String {
    assert!(!chunks.is_empty(), "startup notification broadcast must contain at least one chunk");
    assert_eq!(chunks[0].kind, StartupChunkKind::Begin);
    for chunk in &chunks[1..] {
        assert_eq!(chunk.kind, StartupChunkKind::Continue);
    }
    let mut raw = Vec::new();
    let mut terminated = false;
    for chunk in chunks {
        for &byte in &chunk.data {
            if byte == 0 {
                terminated = true;
                break;
            }
            assert!(!terminated, "non-NUL bytes must not appear after the terminator");
            raw.push(byte);
        }
        if terminated {
            // Remaining bytes after the first NUL in the last chunk must be NUL padding.
            let nul_pos = chunk.data.iter().position(|&b| b == 0).unwrap();
            assert!(chunk.data[nul_pos..].iter().all(|&b| b == 0));
            break;
        }
    }
    assert!(terminated, "startup notification chunks must contain a NUL terminator");
    String::from_utf8(raw).expect("startup notification payload must be valid UTF-8")
}

/// Parses a `remove: ID=<value>` startup-notification message according to the spec's quoting and
/// backslash-escaping rules.
fn parse_remove_id(message: &str) -> String {
    let value = message.strip_prefix("remove: ID=").expect("message must start with `remove: ID=`");
    let mut out = String::new();
    let mut chars = value.chars();
    let mut in_quotes = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let escaped = chars.next().expect("dangling backslash in startup notification value");
                out.push(escaped);
            }
            '"' => in_quotes = !in_quotes,
            c if c.is_ascii_whitespace() && !in_quotes => {
                panic!("unexpected unquoted whitespace in remove: ID value: {message:?}");
            }
            c => out.push(c),
        }
    }
    assert!(!in_quotes, "unterminated double quote in startup notification value: {message:?}");
    out
}

#[test]
fn parses_startup_timestamp_from_time_suffix() {
    assert_eq!(parse_startup_timestamp("demo_TIME12345"), Some(12345));
    assert_eq!(parse_startup_timestamp("demo_TIME0"), Some(0));
    assert_eq!(parse_startup_timestamp("host+1234+0_TIME4294967295"), Some(u32::MAX));
    assert_eq!(parse_startup_timestamp("my_TIME_app_TIME99"), Some(99));
    assert_eq!(parse_startup_timestamp("_TIME42"), Some(42));

    assert_eq!(parse_startup_timestamp("demo"), None);
    assert_eq!(parse_startup_timestamp("demo_TIME"), None);
    assert_eq!(parse_startup_timestamp("demo_TIME4294967296"), None);
    assert_eq!(parse_startup_timestamp("demo_TIME+123"), None);
    assert_eq!(parse_startup_timestamp("demo_TIME-1"), None);
    assert_eq!(parse_startup_timestamp("demo_TIME123abc"), None);
    assert_eq!(parse_startup_timestamp("demo_TIME 123"), None);
    assert_eq!(parse_startup_timestamp("demo_TIME123 "), None);
    assert_eq!(parse_startup_timestamp("demo_TIME123\0"), None);
    assert_eq!(parse_startup_timestamp(""), None);
}

#[test]
fn active_window_data_sets_application_source_and_timestamp() {
    assert_eq!(active_window_data("demo_TIME12345"), [1, 12345, 0, 0, 0]);
    assert_eq!(active_window_data("demo_TIME0"), [1, 0, 0, 0, 0]);
    assert_eq!(active_window_data("demo_without_time"), [1, 0, 0, 0, 0]);
}

#[test]
fn remove_message_matches_spec_for_repro_token() {
    let chunks = remove_message_chunks("demo_TIME12345").expect("valid token");
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0], StartupChunk { kind: StartupChunkKind::Begin, data: *b"remove: ID=demo_TIME" });
    assert_eq!(
        chunks[1],
        StartupChunk { kind: StartupChunkKind::Continue, data: *b"12345\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0" },
    );
    assert_eq!(reassemble_chunks(&chunks), "remove: ID=demo_TIME12345");
    assert_eq!(parse_remove_id(&reassemble_chunks(&chunks)), "demo_TIME12345");
}

#[test]
fn remove_message_chunk_boundaries_keep_nul_terminator() {
    // 11 ("remove: ID=") + 7 ("a_TIME1") + 1 (NUL) = 19 bytes -> 1 chunk, 2 trailing NULs.
    let len19 = remove_message_chunks("a_TIME1").unwrap();
    assert_eq!(len19.len(), 1);
    assert_eq!(len19[0].kind, StartupChunkKind::Begin);
    assert_eq!(&len19[0].data, b"remove: ID=a_TIME1\0\0");

    // 11 + 8 ("ab_TIME1") + 1 (NUL) = 20 bytes -> 1 chunk ending in NUL.
    let len20 = remove_message_chunks("ab_TIME1").unwrap();
    assert_eq!(len20.len(), 1);
    assert_eq!(len20[0].kind, StartupChunkKind::Begin);
    assert_eq!(&len20[0].data, b"remove: ID=ab_TIME1\0");

    // 11 + 9 ("abc_TIME1") + 1 (NUL) = 21 bytes -> 2 chunks; second chunk carries the NUL terminator.
    let len21 = remove_message_chunks("abc_TIME1").unwrap();
    assert_eq!(len21.len(), 2);
    assert_eq!(len21[0], StartupChunk { kind: StartupChunkKind::Begin, data: *b"remove: ID=abc_TIME1" });
    assert_eq!(len21[1], StartupChunk { kind: StartupChunkKind::Continue, data: [0u8; 20] });
    assert_eq!(reassemble_chunks(&len21), "remove: ID=abc_TIME1");

    // Long token (> 40 bytes): Begin, Continue, Continue.
    let long = "gnome-shell/strata/1234-5-host_TIME98765432";
    let chunks = remove_message_chunks(long).unwrap();
    assert_eq!(chunks.len(), 3);
    assert_eq!(parse_remove_id(&reassemble_chunks(&chunks)), long);
}

#[test]
fn remove_message_escapes_whitespace_quotes_and_backslashes() {
    let cases = [
        ("hello world_TIME1", "remove: ID=\"hello world_TIME1\""),
        ("tab\tid_TIME2", "remove: ID=\"tab\tid_TIME2\""),
        ("a\"b_TIME3", "remove: ID=\"a\\\"b_TIME3\""),
        ("a\\b_TIME4", "remove: ID=\"a\\\\b_TIME4\""),
        ("a \"b\" \\c_TIME5", "remove: ID=\"a \\\"b\\\" \\\\c_TIME5\""),
        ("café_TIME6", "remove: ID=café_TIME6"),
        ("café ☕ space_TIME7", "remove: ID=\"café ☕ space_TIME7\""),
    ];
    for (token, expected) in cases {
        let chunks = remove_message_chunks(token).expect("valid token");
        let reassembled = reassemble_chunks(&chunks);
        assert_eq!(reassembled, expected, "token={token:?}");
        assert_eq!(parse_remove_id(&reassembled), token, "round-trip failed for token={token:?}");
    }
}

#[test]
fn remove_message_rejects_empty_and_embedded_nul_tokens() {
    assert_eq!(remove_message_bytes(""), None);
    assert_eq!(remove_message_chunks(""), None);
    assert_eq!(remove_message_bytes("demo\0_TIME123"), None);
    assert_eq!(remove_message_chunks("demo\0_TIME123"), None);
}
