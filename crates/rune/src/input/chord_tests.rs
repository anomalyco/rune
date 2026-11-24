use super::{Chord, ChordError, ChordKey, ChordMod, KeyPlatform, Keybind};
use super::super::key_table::NAMED_KEYS;

#[test]
fn plus_key_and_trailing_plus_chords_parse_and_round_trip() {
    let ctrl_plus = Chord::new(ChordKey::Char('+')).with(ChordMod::Ctrl);
    let ctrl_shift_plus = ctrl_plus.with(ChordMod::Shift);
    let mod_plus = Chord::new(ChordKey::Char('+')).with(ChordMod::Primary);
    let bare_plus = Chord::new(ChordKey::Char('+'));
    let ctrl_equal = Chord::new(ChordKey::Char('=')).with(ChordMod::Ctrl);
    let ctrl_minus = Chord::new(ChordKey::Char('-')).with(ChordMod::Ctrl);

    let cases = [
        ("ctrl++", ctrl_plus, "ctrl+plus"),
        ("+", bare_plus, "plus"),
        ("ctrl+shift++", ctrl_shift_plus, "ctrl+shift+plus"),
        ("ctrl+plus", ctrl_plus, "ctrl+plus"),
        ("ctrl+=", ctrl_equal, "ctrl+="),
        ("ctrl+-", ctrl_minus, "ctrl+-"),
        ("ctrl+ +", ctrl_plus, "ctrl+plus"),
        ("ctrl + +", ctrl_plus, "ctrl+plus"),
        ("mod++", mod_plus, "mod+plus"),
        ("mod + +", mod_plus, "mod+plus"),
        ("MOD++", mod_plus, "mod+plus"),
        (" + ", bare_plus, "plus"),
        ("++", bare_plus, "plus"),
    ];

    for (input, expected, canonical) in cases {
        let parsed = Chord::parse(input).unwrap_or_else(|e| panic!("failed to parse {input:?}: {e}"));
        assert_eq!(parsed, expected, "input {input:?}");
        assert_eq!(parsed.to_string(), canonical, "Display for {input:?}");
        assert_eq!(Chord::parse(&parsed.to_string()), Ok(parsed), "round-trip for {input:?}");
    }

    let bind: Keybind = "mod++, mod+-".parse().unwrap();
    assert_eq!(bind.spec_on(KeyPlatform::Windows), "mod+plus, mod+-");
    assert_eq!(bind.spec_on(KeyPlatform::Windows).parse::<Keybind>(), Ok(bind));
}

#[test]
fn rejects_malformed_plus_and_modifier_chords() {
    for input in [
        "",
        " ",
        "ctrl",
        "ctrl+",
        "ctrl+ ",
        "ctrl+++",
        "+++",
        "ctrl+a++",
        "a++",
        "ctrl+plus++",
        "ctrl+ctrl++",
        "+ctrl",
        "+ctrl++",
        "ctrl++a",
        "ctrl+a+",
        "a+ctrl",
        "ctrl+a+b",
    ] {
        assert!(matches!(Chord::parse(input), Err(ChordError(_))), "should reject {input:?}");
    }
}

#[test]
fn display_round_trips_for_every_chord_key() {
    let mut keys: Vec<ChordKey> = NAMED_KEYS.iter().map(|k| k.chord).collect();
    keys.extend((1..=24).map(ChordKey::Function));
    keys.extend(
        [' ', '+', ',', '-', '=', '/', ';', '\'', '[', ']', '\\', '`', '{', '}', 'a', '0', 'é']
            .into_iter()
            .map(ChordKey::Char),
    );

    for key in keys {
        for chord in [
            Chord::new(key),
            Chord::new(key).with(ChordMod::Primary),
            Chord::new(key).with(ChordMod::Ctrl).with(ChordMod::Shift),
            Chord::new(key)
                .with(ChordMod::Primary)
                .with(ChordMod::Ctrl)
                .with(ChordMod::Alt)
                .with(ChordMod::Shift)
                .with(ChordMod::Super),
        ] {
            let printed = chord.to_string();
            assert_eq!(Chord::parse(&printed), Ok(chord), "round-trip failed for {key:?} -> {printed:?}");
        }
    }
}
