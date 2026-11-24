use super::*;
use crate::html::color::{NAMED_COLORS, normalize_hex, parse_css_color, parse_font_color};
use rune_core::color::hex as srgb_hex;
use rune_text::flatten_spans;

fn undo(e: &mut TextEdit, redo: bool) {
    e.key(&KeyEvent { key: Key::Command('z'), mods: Mods { cmd: true, shift: redo, ..Mods::default() } }, None);
}

fn color_ranges(e: &TextEdit) -> Vec<(usize, usize, String)> {
    e.marks()
        .iter()
        .filter_map(|m| m.mark.color_hex().map(|c| (m.start, m.end, c.to_owned())))
        .collect()
}

#[test]
fn mark_color_helpers_and_same_kind() {
    let red = Mark::Color("#d93025".into());
    let blue = Mark::Color("#1a73e8".into());
    assert!(red.same_kind(&blue));
    assert!(red.same_kind(&Mark::Color(String::new())));
    assert!(!red.same_kind(&Mark::Bold));
    assert!(!red.same_kind(&Mark::Link("https://example.com".into())));
    assert_eq!(red.color_hex(), Some("#d93025"));
    assert_eq!(Mark::Bold.color_hex(), None);
    assert_eq!(Mark::Link("https://example.com".into()).color_hex(), None);

    // DEFAULT_ALLOWED_MARKS holds exactly the 7 pre-existing mark kinds, excluding Mark::Color.
    let expected = [
        Mark::Bold,
        Mark::Italic,
        Mark::Underline,
        Mark::Strike,
        Mark::Code,
        Mark::Link(String::new()),
        Mark::Pill(String::new()),
    ]
    .iter()
    .fold(0u16, |bits, m| bits | (1 << m.kind_order()));
    assert_eq!(TextEdit::DEFAULT_ALLOWED_MARKS, expected);
    assert_eq!(TextEdit::DEFAULT_ALLOWED_MARKS & (1 << Mark::Color(String::new()).kind_order()), 0);
}

#[test]
fn color_is_opt_in_by_default_for_rich_from_html_and_paste() {
    // Default rich field allows the 7 pre-existing marks and disallows Mark::Color.
    let mut def = TextEdit::new("hello").rich();
    assert!(def.allows_mark(&Mark::Bold));
    assert!(def.allows_mark(&Mark::Italic));
    assert!(def.allows_mark(&Mark::Underline));
    assert!(def.allows_mark(&Mark::Strike));
    assert!(def.allows_mark(&Mark::Code));
    assert!(def.allows_mark(&Mark::Link(String::new())));
    assert!(def.allows_mark(&Mark::Pill(String::new())));
    assert!(!def.allows_mark(&Mark::Color(String::new())));
    assert!(!def.allows_mark(&Mark::Color("#d93025".into())));

    // Editing methods ignore Mark::Color until opted in.
    def.set_selection(0, 5);
    def.set_color(Some("#d93025"));
    def.set_mark(Mark::Color("#d93025".into()));
    def.toggle_mark(Mark::Color("#d93025".into()));
    def.set_marks(vec![MarkRange::new(0..5, Mark::Color("#d93025".into()))]);
    assert!(def.marks().is_empty());
    assert_eq!(def.active_color(), None);

    // Chrome black-ink paste into a default rich field drops colour and keeps Bold.
    let chrome_html = concat!(
        r#"<meta charset='utf-8'><span style="color: rgb(0, 0, 0); font-weight: 700;">Hello</span>"#,
        r#"<span style="color: rgb(0, 0, 0);"> world</span>"#,
    );
    let mut def_chrome = TextEdit::new("").rich();
    def_chrome.paste_html(chrome_html, "Hello world");
    assert_eq!(def_chrome.text, "Hello world");
    assert_eq!(def_chrome.marks(), &[MarkRange::new(0..5, Mark::Bold)]);

    // Slack paste into a default rich field drops Slack's rgb(29, 28, 29) ink colour.
    let slack_html = concat!(
        r#"<b style="color: rgb(29, 28, 29);">bold</b>"#,
        r#"<span style="color: rgb(29, 28, 29);"> and </span><i>it</i>"#,
    );
    let mut def_slack = TextEdit::new("").rich();
    def_slack.paste_html(slack_html, "bold and it");
    assert_eq!(def_slack.text, "bold and it");
    assert!(color_ranges(&def_slack).is_empty());
    assert_eq!(def_slack.marks(), &[MarkRange::new(0..4, Mark::Bold), MarkRange::new(9..11, Mark::Italic)]);

    // VS Code colour-only paste into a default rich field falls back to plain whitespace.
    let vscode_html = concat!(
        r#"<meta charset='utf-8'><div style="color: #d4d4d4;white-space: pre;">"#,
        r#"<div><span style="color: #569cd6;">fn</span><span>&nbsp;a()&nbsp;{</span></div>"#,
        r#"<div><span>&nbsp; &nbsp; b</span></div><div><span>}</span></div></div>"#,
    );
    let mut def_vscode = TextEdit::new("").rich();
    def_vscode.paste_html(vscode_html, "fn a() {\n    b\n}");
    assert_eq!(def_vscode.text, "fn a() {\n    b\n}");
    assert!(def_vscode.marks().is_empty());

    // Default from_html strips CSS color and <font color> just like pre-Mark::Color dev.
    let loaded = TextEdit::from_html(r#"<p><span style="color:#ff0000"><b>red</b></span> <font color="blue">blue</font></p>"#);
    assert_eq!(loaded.text, "red blue");
    assert_eq!(loaded.marks(), &[MarkRange::new(0..3, Mark::Bold)]);
    assert!(!loaded.allows_mark(&Mark::Color(String::new())));

    // Opting in via allow_color(true) keeps pasted Chrome, Slack, VS Code, and <font color> colours.
    let mut opt_chrome = TextEdit::new("").rich().allow_color(true);
    opt_chrome.paste_html(chrome_html, "Hello world");
    assert_eq!(color_ranges(&opt_chrome), [(0, 11, "#000000".into())]);

    let mut opt_slack = TextEdit::new("").rich().allow_color(true);
    opt_slack.paste_html(slack_html, "bold and it");
    assert_eq!(color_ranges(&opt_slack), [(0, 9, "#1d1c1d".into())]);

    let mut opt_vscode = TextEdit::new("").rich().allow_color(true);
    opt_vscode.paste_html(vscode_html, "fn a() {\n    b\n}");
    assert!(color_ranges(&opt_vscode).iter().any(|(_, _, c)| c == "#569cd6"));

    // allow_color(true) composes with allow_marks without re-enabling disallowed marks.
    let restricted = TextEdit::new("hi").rich().allow_marks([Mark::Bold]).allow_color(true);
    assert!(restricted.allows_mark(&Mark::Bold));
    assert!(restricted.allows_mark(&Mark::Color(String::new())));
    assert!(!restricted.allows_mark(&Mark::Italic));

    // allow_color(false) strips existing colours and pending colour typing style.
    let mut off = TextEdit::from_html_with_color(r#"<p><span style="color:#ff0000">red</span></p>"#);
    assert_eq!(color_ranges(&off), [(0, 3, "#ff0000".into())]);
    off.set_caret(3);
    off.set_color(Some("#00ff00"));
    assert_eq!(off.active_color(), Some("#00ff00"));
    let off = off.allow_color(false);
    assert!(!off.allows_mark(&Mark::Color(String::new())));
    assert!(color_ranges(&off).is_empty());
    assert_eq!(off.active_color(), None);
}

#[test]
fn set_color_and_active_color_on_selection_replace_merge_and_invalid() {
    let mut e = TextEdit::new("hello world").rich().allow_color(true);
    assert_eq!(e.active_color(), None);

    // Shorthand uppercase hex normalizes to lowercase #rrggbb.
    e.set_selection(0, 5);
    e.set_color(Some(" #F0A "));
    assert_eq!(e.active_color(), Some("#ff00aa"));
    assert!(e.has_mark(&Mark::Color("#ff00aa".into())));
    assert!(e.has_mark(&Mark::Color("#F0A".into())));
    assert!(e.has_mark(&Mark::Color(String::new())));
    assert_eq!(color_ranges(&e), [(0, 5, "#ff00aa".into())]);

    // Invalid inputs are ignored without changing marks or recording an undo step.
    let hist_before = e.history_len();
    for bad in ["red", "rgb(1,2,3)", "#12", "#1234", "#12345", "#12345678", "#gggggg", "ff00aa", "currentColor"] {
        e.set_color(Some(bad));
        assert_eq!(e.active_color(), Some("#ff00aa"), "invalid {bad:?} must be ignored");
    }
    assert_eq!(e.history_len(), hist_before, "invalid set_color must not push undo history");

    // Replacing a subrange splits the existing colour and overwrites the middle.
    e.set_selection(1, 4);
    e.set_color(Some("#1A73E8"));
    assert_eq!(e.active_color(), Some("#1a73e8"));
    assert_eq!(
        color_ranges(&e),
        [
            (0, 1, "#ff00aa".into()),
            (1, 4, "#1a73e8".into()),
            (4, 5, "#ff00aa".into()),
        ]
    );

    // Selection spanning different colours reports None.
    e.set_selection(0, 5);
    assert_eq!(e.active_color(), None);

    // Adjacent equal colours merge into one range.
    e.set_selection(5, 11);
    e.set_color(Some("#ff00aa"));
    assert_eq!(
        color_ranges(&e),
        [
            (0, 1, "#ff00aa".into()),
            (1, 4, "#1a73e8".into()),
            (4, 11, "#ff00aa".into()),
        ]
    );

    // Removing with None or empty string clears the selection's colour.
    e.set_selection(0, 5);
    e.set_color(None);
    assert_eq!(color_ranges(&e), [(5, 11, "#ff00aa".into())]);
    e.set_selection(5, 11);
    e.set_color(Some("   "));
    assert!(e.marks().is_empty());
    assert_eq!(e.active_color(), None);
}

#[test]
fn collapsed_caret_pending_typing_style_and_inheritance() {
    // Empty field: arming a colour before typing applies to the first characters typed.
    let mut e = TextEdit::new("").rich().allow_color(true);
    e.set_color(Some("#0a84ff"));
    assert_eq!(e.active_color(), Some("#0a84ff"));
    e.insert("red");
    assert_eq!(color_ranges(&e), [(0, 3, "#0a84ff".into())]);
    assert_eq!(e.active_color(), Some("#0a84ff"), "caret at trailing edge inherits colour");

    // Clearing pending style at the caret stops inheriting colour for subsequent text.
    e.set_color(None);
    assert_eq!(e.active_color(), None);
    e.insert(" plain");
    assert_eq!(color_ranges(&e), [(0, 3, "#0a84ff".into())]);
    assert_eq!(e.active_color(), None);

    // Moving the caret disarms a pending colour.
    e.set_color(Some("#00ff00"));
    assert_eq!(e.active_color(), Some("#00ff00"));
    e.set_caret(0);
    assert_eq!(e.active_color(), None, "caret at start of coloured run does not extend it");
    e.insert("pre ");
    assert_eq!(color_ranges(&e), [(4, 7, "#0a84ff".into())]);

    // toggle_mark with empty Color removes active colour without arming an invalid empty mark.
    e.set_caret(7);
    assert_eq!(e.active_color(), Some("#0a84ff"));
    e.toggle_mark(Mark::Color(String::new()));
    assert_eq!(e.active_color(), None);
    e.toggle_mark(Mark::Color(String::new()));
    assert_eq!(e.active_color(), None);
    e.insert("!");
    assert_eq!(color_ranges(&e), [(4, 7, "#0a84ff".into())]);
}

#[test]
fn undo_redo_and_history_json_preserve_color() {
    let mut e = TextEdit::new("hello").rich().allow_color(true);
    e.set_selection(0, 5);
    e.set_color(Some("#d93025"));
    e.set_color(Some("#1a73e8"));
    assert_eq!(e.active_color(), Some("#1a73e8"));

    undo(&mut e, false);
    assert_eq!(e.active_color(), Some("#d93025"));
    undo(&mut e, false);
    assert_eq!(e.active_color(), None);
    undo(&mut e, true);
    assert_eq!(e.active_color(), Some("#d93025"));
    undo(&mut e, true);
    assert_eq!(e.active_color(), Some("#1a73e8"));

    // Persisted JSON undo/redo history round-trips Mark::Color.
    let saved = e.history_json(32, 64 * 1024).expect("history json");
    let mut restored = TextEdit::from_parts(e.text.clone(), e.marks().to_vec(), e.blocks().to_vec());
    restored.set_selection(0, 5);
    assert!(restored.restore_history_json(&saved));
    undo(&mut restored, false);
    assert_eq!(restored.active_color(), Some("#d93025"));
    undo(&mut restored, true);
    assert_eq!(restored.active_color(), Some("#1a73e8"));

    // Invalid hex in persisted history JSON is rejected cleanly.
    let mut bad = saved.clone();
    let step = bad["undo"].as_array_mut().unwrap().last_mut().unwrap();
    step["a"]["m"] = serde_json::json!([[0, 5, ["fg", "not-a-hex"]]]);
    let mut clean = TextEdit::from_parts(e.text.clone(), e.marks().to_vec(), e.blocks().to_vec());
    assert!(!clean.restore_history_json(&bad));
}

#[test]
fn allow_marks_code_block_and_secure_gate_color() {
    // Disallowing Mark::Color strips existing colours, blocks set_color/set_mark/toggle_mark/paste/undo.
    let e = TextEdit::from_html_with_color(r#"<p><span style="color:#ff0000"><b>red</b></span></p>"#);
    assert_eq!(color_ranges(&e), [(0, 3, "#ff0000".into())]);
    let mut e = e.allow_marks([Mark::Bold, Mark::Link(String::new())]);
    assert!(!e.allows_mark(&Mark::Color(String::new())));
    assert!(!e.allows_mark(&Mark::Color("#ff0000".into())));
    assert!(color_ranges(&e).is_empty(), "existing colour dropped");
    assert_eq!(e.marks(), &[MarkRange::new(0..3, Mark::Bold)]);

    e.set_selection(0, 3);
    e.set_color(Some("#00ff00"));
    e.set_mark(Mark::Color("#00ff00".into()));
    e.toggle_mark(Mark::Color("#00ff00".into()));
    e.set_marks(vec![MarkRange::new(0..3, Mark::Color("#00ff00".into()))]);
    assert!(color_ranges(&e).is_empty());

    e.set_caret(3);
    e.paste_html(r#"<span style="color:#1a73e8"> blue</span>"#, " blue");
    assert_eq!(e.text, "red blue");
    assert!(color_ranges(&e).is_empty(), "disallowed colour stripped on paste");

    // Allowing Mark::Color(String::new()) permits any colour.
    let mut allowed = TextEdit::new("hi").rich().allow_marks([Mark::Color(String::new())]);
    assert!(allowed.allows_mark(&Mark::Color("#123456".into())));
    allowed.set_selection(0, 2);
    allowed.set_color(Some("#123456"));
    assert_eq!(allowed.active_color(), Some("#123456"));

    // Code blocks strip colour and report None for active_color.
    let mut code = TextEdit::new("let x = 1;").rich().allow_color(true);
    code.set_selection(0, 3);
    code.set_color(Some("#ff0000"));
    code.set_block(Block::Code);
    assert!(color_ranges(&code).is_empty());
    code.set_caret(3);
    code.set_color(Some("#00ff00"));
    assert_eq!(code.active_color(), None);

    // Secure fields ignore colour.
    let mut sec = TextEdit::new("secret").secure().allow_color(true);
    sec.set_selection(0, 6);
    sec.set_color(Some("#ff0000"));
    assert!(sec.marks().is_empty());
    assert_eq!(sec.active_color(), None);
}

#[test]
fn ime_preedit_and_rtl_and_unicode_with_color() {
    // IME preedit inside a coloured span splits the committed span and styles the preedit.
    let mut e = TextEdit::new("hello").rich().allow_color(true);
    e.set_selection(0, 5);
    e.set_color(Some("#d93025"));
    e.set_caret(2);
    e.ime(&Ime::Preedit("に".into(), Some((0, 3)), Vec::new()));
    let (spans, _, _, _) = e.rich_display_specs(rune_core::tok::TextStyle::new(14.0, 400.0), RichStyle::default());
    let red = srgb_hex("#d93025").unwrap();
    let flat = flatten_spans(&spans);
    assert_eq!(flat.len(), 1, "committed halves + preedit merge into one coloured run: {flat:?}");
    assert_eq!((flat[0].start, flat[0].end, flat[0].color), (0, 8, Some(red)));
    e.ime(&Ime::Commit("日".into()));
    assert_eq!(e.text, "he日llo");
    assert_eq!(color_ranges(&e), [(0, 8, "#d93025".into())]);

    // RTL Arabic + emoji round-trip through HTML keeping dir="rtl" and colour.
    let mut rtl = TextEdit::new("مرحبا 👋 عالم").rich().allow_color(true);
    let end_first = "مرحبا".len();
    rtl.set_selection(0, end_first);
    rtl.set_color(Some("#0b8043"));
    let html = rtl.to_html();
    assert!(html.contains(r#"dir="rtl""#), "{html}");
    assert!(html.contains(r#"<span style="color:#0b8043">مرحبا</span>"#), "{html}");
    let back = TextEdit::from_html_with_color(&html);
    assert_eq!((&back.text, back.marks()), (&rtl.text, rtl.marks()));
}

#[test]
fn css_color_parser_covers_formats_named_table_and_rejects_unsafe_values() {
    // Verify the 148-entry CSS named colour table is strictly sorted and every entry is valid #rrggbb.
    assert_eq!(NAMED_COLORS.len(), 148);
    for w in NAMED_COLORS.windows(2) {
        assert!(w[0].0 < w[1].0, "NAMED_COLORS not sorted at {:?} >= {:?}", w[0].0, w[1].0);
    }
    for &(name, hex) in NAMED_COLORS {
        assert_eq!(normalize_hex(hex).as_deref(), Some(hex), "invalid hex for {name}: {hex}");
        assert_eq!(parse_css_color(name).as_deref(), Some(hex));
    }

    let cases = [
        ("#abc", Some("#aabbcc")),
        ("#AbC", Some("#aabbcc")),
        ("#123456", Some("#123456")),
        ("#123456 !important", Some("#123456")),
        ("#123456!IMPORTANT", Some("#123456")),
        ("#f0a8", Some("#ff00aa")),
        ("#f0a0", None),
        ("#11223380", Some("#112233")),
        ("#11223300", None),
        ("rgb(255, 0, 128)", Some("#ff0080")),
        ("RGB( 0 , 128 , 255 )", Some("#0080ff")),
        ("rgb(100%, 0%, 50%)", Some("#ff0080")),
        ("rgb(255 0 128)", Some("#ff0080")),
        ("rgb(255 0 128 / 0.5)", Some("#ff0080")),
        ("rgb(255 0 128 / 50%)", Some("#ff0080")),
        ("rgb(255 0 128 / 0)", None),
        ("rgb(255 0 128 / 0%)", None),
        ("rgba(29, 28, 29, 1)", Some("#1d1c1d")),
        ("rgba(29, 28, 29, 0)", None),
        ("rgb(-10, 300, 127.6)", Some("#00ff80")),
        ("hsl(0, 100%, 50%)", Some("#ff0000")),
        ("hsl(120deg 100% 50%)", Some("#00ff00")),
        ("hsl(240 100% 50% / 1)", Some("#0000ff")),
        ("hsl(0.5turn 100% 50%)", Some("#00ffff")),
        ("hsla(210, 100%, 50%, 0)", None),
        ("ReBeCcApUrPlE", Some("#663399")),
        ("crimson", Some("#dc143c")),
        ("darkslategrey", Some("#2f4f4f")),
        ("currentColor", None),
        ("transparent", None),
        ("inherit", None),
        ("initial", None),
        ("unset", None),
        ("revert", None),
        ("var(--brand)", None),
        ("rgb(var(--r), 0, 0)", None),
        ("rgb(255, 0, 0 / 0.5)", None),
        ("rgb(255, 0, 0,)", None),
        ("rgb(NaN, 0, 0)", None),
        ("notacolor", None),
        ("", None),
    ];
    for (input, expected) in cases {
        assert_eq!(parse_css_color(input).as_deref(), expected, "parse_css_color({input:?})");
    }

    // Legacy <font color> accepts bare 3- and 6-digit hex too.
    assert_eq!(parse_font_color("ff00aa").as_deref(), Some("#ff00aa"));
    assert_eq!(parse_font_color("F0A").as_deref(), Some("#ff00aa"));
    assert_eq!(parse_font_color("navy").as_deref(), Some("#000080"));
    assert_eq!(parse_font_color("currentColor"), None);
}

#[test]
fn html_import_nested_font_precedence_and_round_trip() {
    // Style attributes (#rgb, #rrggbb, rgb(), named), <font color>, ignored currentColor/var/background-color.
    let src = concat!(
        "<p>",
        r#"<span style="color:#f00">a</span>"#,
        r#"<span style="color: rgb(0, 128, 255)">b</span>"#,
        r#"<span style="color: Crimson !important">c</span>"#,
        r##"<font color="#00aa00">d</font>"##,
        r#"<font color="336699">e</font>"#,
        r#"<span style="color: currentColor; background-color: yellow">f</span>"#,
        r#"<span style="color: var(--ink)">g</span>"#,
        "</p>",
    );
    let e = TextEdit::from_html_with_color(src);
    assert_eq!(e.text, "abcdefg");
    assert_eq!(
        color_ranges(&e),
        [
            (0, 1, "#ff0000".into()),
            (1, 2, "#0080ff".into()),
            (2, 3, "#dc143c".into()),
            (3, 4, "#00aa00".into()),
            (4, 5, "#336699".into()),
        ]
    );
    let exported = e.to_html();
    assert_eq!(
        exported,
        r#"<p><span style="color:#ff0000">a</span><span style="color:#0080ff">b</span><span style="color:#dc143c">c</span><span style="color:#00aa00">d</span><span style="color:#336699">e</span>fg</p>"#
    );
    let back = TextEdit::from_html_with_color(&exported);
    assert_eq!((&back.text, back.marks()), (&e.text, e.marks()));

    // Nested colours restore the outer colour when the inner tag closes.
    let nested = TextEdit::from_html_with_color(r#"<span style="color:red">a<span style="color:blue">b</span>c</span>d"#);
    assert_eq!(nested.text, "abcd");
    assert_eq!(
        color_ranges(&nested),
        [
            (0, 1, "#ff0000".into()),
            (1, 2, "#0000ff".into()),
            (2, 3, "#ff0000".into()),
        ]
    );

    // Same element with both <font color> and style="color:..." (or duplicate style color) lets
    // the later declaration win without leaking the earlier one after the closing tag.
    let same_el = TextEdit::from_html_with_color(r#"<font color="red" style="color:blue">x</font>y<span style="color:green; color:#123456">z</span>w"#);
    assert_eq!(same_el.text, "xyzw");
    assert_eq!(color_ranges(&same_el), [(0, 1, "#0000ff".into()), (2, 3, "#123456".into())]);

    // Round-trip combined with Bold, Italic, Underline, Link, and Pill.
    let mut combo = TextEdit::from_html_with_color(
        r#"<p><a href="https://example.com"><span style="color:#d93025"><strong><em>link</em></strong></span></a> <span data-pill="id:1"><span style="color:#1a73e8">@tag</span></span></p>"#,
    );
    let html = combo.to_html();
    let round = TextEdit::from_html_with_color(&html);
    assert_eq!((&round.text, round.marks()), (&combo.text, combo.marks()));
    combo.set_selection(0, 4);
    assert!(combo.selected_html().contains(r#"<span style="color:#d93025">"#));
}

#[test]
fn rich_display_specs_emits_color_and_keeps_link_on_top() {
    let base = rune_core::tok::TextStyle::new(14.0, 400.0);
    let rs = RichStyle::default();
    let red = srgb_hex("#d93025").unwrap();

    // Plain field without marks stays plain and produces no spans.
    let plain = TextEdit::new("plain");
    assert!(plain.is_plain());
    let (spans, _, _, _) = plain.rich_display_specs(base, rs);
    assert!(spans.is_empty());

    // Link covering 0..10 and Color covering 2..6 (Color starts AFTER Link): Link must still win.
    let e = TextEdit::from_parts(
        "0123456789",
        vec![
            MarkRange::new(0..10, Mark::Link("https://example.com".into())),
            MarkRange::new(2..6, Mark::Color("#d93025".into())),
        ],
        vec![Block::Paragraph],
    );
    let (spans, _, links, _) = e.rich_display_specs(base, rs);
    assert_eq!(links.len(), 1);
    let flat = flatten_spans(&spans);
    assert_eq!(flat.len(), 1, "Link should cover 0..10 uniformly over inner Color: {flat:?}");
    assert_eq!(flat[0].color, Some(rs.link));
    assert_eq!(flat[0].underline, Some(rs.link.alpha(0.65)));

    // Color covering 0..10 and Link covering 3..7: Color shows outside 3..7, Link wins on 3..7.
    let e2 = TextEdit::from_parts(
        "0123456789",
        vec![
            MarkRange::new(0..10, Mark::Color("#d93025".into())),
            MarkRange::new(3..7, Mark::Link("https://example.com".into())),
        ],
        vec![Block::Paragraph],
    );
    let (spans2, _, _, _) = e2.rich_display_specs(base, rs);
    let flat2 = flatten_spans(&spans2);
    assert_eq!(
        flat2.iter().map(|s| (s.start, s.end, s.color)).collect::<Vec<_>>(),
        [(0, 3, Some(red)), (3, 7, Some(rs.link)), (7, 10, Some(red))]
    );

    // IME preedit inside a coloured link also keeps Link colour on top.
    let mut e3 = e2.clone();
    e3.set_caret(5);
    e3.ime(&Ime::Preedit("あ".into(), None, Vec::new()));
    let (spans3, _, _, _) = e3.rich_display_specs(base, rs);
    let flat3 = flatten_spans(&spans3);
    assert!(
        flat3.iter().any(|s| s.start <= 5 && s.end >= 8 && s.color == Some(rs.link)),
        "preedit inside coloured link must draw in link colour: {flat3:?}"
    );
}
