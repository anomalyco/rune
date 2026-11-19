use super::*;

#[test]
fn masking_reads_the_secure_setting_the_reveal_toggle_and_the_line_mode() {
    let hidden = Masking::Hidden { multiline: false };
    assert_eq!(TextEdit::new("pw").masking(), Masking::Plain);
    assert_eq!(TextEdit::new("pw").secure().masking(), hidden, "a field not yet mounted is single-line");
    assert_eq!(TextEdit::new("pw").secure().revealed(true).masking(), Masking::Shown);

    // The toggle is kept on a plain field, so builders may set it before `secure`.
    let mut e = TextEdit::new("pw").revealed(true);
    assert_eq!((e.masking(), e.display().0.as_str()), (Masking::Plain, "pw"));
    e.set_secure(true);
    assert_eq!(e.masking(), Masking::Shown);
    e.set_secure(false);
    e.set_secure(true);
    assert_eq!(e.masking(), Masking::Shown, "turning secure off and on keeps the toggle");

    e.set_revealed(false);
    e.set_single_line(false);
    assert_eq!(e.masking(), Masking::Hidden { multiline: true });
    assert!(e.is_masked() && e.is_hidden_multiline());
    e.set_single_line(true);
    assert!(e.is_masked() && !e.is_hidden_multiline());
}

#[test]
fn conceal_keeps_exactly_the_mask_the_hidden_field_drew() {
    for (single, text) in [(true, "hé🙂"), (false, "two\nlines"), (true, ""), (false, "")] {
        let mut e = TextEdit::new(text).secure();
        e.set_single_line(single);
        let drawn = e.display().0;
        e.set_revealed(true);
        e.conceal();
        assert_eq!(e.masking(), Masking::Hidden { multiline: !single });
        assert_eq!((e.text.as_str(), e.display().0), (drawn.as_str(), drawn.clone()));
    }
    let mut plain = TextEdit::new("kept").revealed(true);
    plain.conceal();
    assert_eq!((plain.text.as_str(), plain.is_revealed()), ("kept", true), "a plain field has nothing to conceal");
}
