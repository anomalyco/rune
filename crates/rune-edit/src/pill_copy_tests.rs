use super::*;

#[test]
fn selected_with_pills_replaces_wholly_selected_pills_and_falls_back_on_none() {
    let mut e = TextEdit::new("See ").rich();
    e.insert_pill("https://example.com/1", "Ben in #general");
    e.insert(" and ");
    e.insert_pill("missing:id", "@Fallback");
    e.insert(" plus ");
    e.insert_pill("empty:id", "@Silent");
    e.insert("!");

    // Single pill selected.
    e.set_selection(4, 4 + "Ben in #general".len());
    let single = e.selected_with_pills(|id| match id {
        "https://example.com/1" => Some(id.to_owned()),
        _ => None,
    });
    assert_eq!(single, "https://example.com/1");

    // Mixed text + multiple pills + None fallback + Some("") empty payload.
    e.set_selection(0, e.text.len());
    let mut seen = Vec::new();
    let mixed = e.selected_with_pills(|id| {
        seen.push(id.to_owned());
        match id {
            "https://example.com/1" => Some(id.to_owned()),
            "empty:id" => Some(String::new()),
            _ => None,
        }
    });
    assert_eq!(seen, ["https://example.com/1", "missing:id", "empty:id"]);
    assert_eq!(mixed, "See https://example.com/1 and @Fallback plus !");
    // Original TextEdit::selected() still returns display labels unchanged.
    assert_eq!(e.selected(), "See Ben in #general and @Fallback plus @Silent!");
}

#[test]
fn selected_with_pills_handles_adjacent_multiline_utf8_and_rtl_pills() {
    let mut e = TextEdit::new("héllo ").rich();
    e.insert_pill("u:1", "👋@مرحبا");
    e.insert_pill("u:2", "日本語");
    e.insert("\nمرحبا ");
    e.insert_pill("u:3", "שלום");
    e.insert(" 🦀");

    e.set_selection(0, e.text.len());
    let out = e.selected_with_pills(|id| match id {
        "u:1" => Some("<https://example.com/1>".into()),
        "u:2" => Some("<https://example.com/2>".into()),
        "u:3" => Some("https://rtl.example/שלום".into()),
        _ => None,
    });
    assert_eq!(
        out,
        "héllo <https://example.com/1><https://example.com/2>\nمرحبا https://rtl.example/שלום 🦀"
    );

    // Selection covering only a subset of pills does not invoke callback for unselected pills.
    let second_line = e.text.find('\n').unwrap() + 1;
    e.set_selection(second_line, e.text.len());
    let mut called = Vec::new();
    let line2 = e.selected_with_pills(|id| {
        called.push(id.to_owned());
        Some("URL".into())
    });
    assert_eq!(called, ["u:3"]);
    assert_eq!(line2, "مرحبا URL 🦀");
}

#[test]
fn selected_with_pills_skips_partial_ranges_empty_selections_and_secure_fields() {
    let mut e = TextEdit::new("a ").rich();
    e.insert_pill("agent:x", "@x-agent");
    e.insert(" b");

    // Empty selection returns empty without calling closure.
    e.set_caret(2);
    let mut calls = 0usize;
    assert_eq!(
        e.selected_with_pills(|_| {
            calls += 1;
            Some("X".into())
        }),
        ""
    );
    assert_eq!(calls, 0);

    // Directly mutated partial range inside a pill keeps raw bytes and skips the callback.
    e.anchor = 3;
    e.focus = 8;
    let partial = e.selected_with_pills(|_| {
        calls += 1;
        Some("REPLACED".into())
    });
    assert_eq!(partial, "x-age");
    assert_eq!(calls, 0);

    // A field with no pills returns the exact selected slice without calling the closure.
    let mut plain = TextEdit::new("plain text");
    plain.set_selection(0, 5);
    assert_eq!(
        plain.selected_with_pills(|_| {
            calls += 1;
            Some("NO".into())
        }),
        "plain"
    );
    assert_eq!(calls, 0);

    // Secure field returns empty without invoking callback.
    let mut secret = TextEdit::new("hunter2").secure();
    secret.set_selection(0, 7);
    assert_eq!(
        secret.selected_with_pills(|_| {
            calls += 1;
            Some("LEAK".into())
        }),
        ""
    );
    assert_eq!(calls, 0);
}
