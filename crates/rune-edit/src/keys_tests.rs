use super::*;

const PLATFORMS: [KeyPlatform; 3] = [KeyPlatform::Mac, KeyPlatform::Windows, KeyPlatform::Linux];

/// Expected `[Mac, Windows, Linux]` results of `primary_alone_on` for every combination of
/// `(cmd, ctrl, logo, alt, shift)` (bit 0..4 respectively) on targets where `cmd` repeats Ctrl
/// (Windows, Linux, iOS).
const EXPECTED_OTHER: [[bool; 3]; 32] = [
    [true, true, true],    // 00: cmd=false ctrl=false logo=false alt=false shift=false
    [true, true, true],    // 01: cmd=true  ctrl=false logo=false alt=false shift=false
    [false, true, true],   // 02: cmd=false ctrl=true  logo=false alt=false shift=false
    [false, true, true],   // 03: cmd=true  ctrl=true  logo=false alt=false shift=false
    [true, false, false],  // 04: cmd=false ctrl=false logo=true  alt=false shift=false
    [false, false, false], // 05: cmd=true  ctrl=false logo=true  alt=false shift=false
    [false, false, false], // 06: cmd=false ctrl=true  logo=true  alt=false shift=false
    [false, false, false], // 07: cmd=true  ctrl=true  logo=true  alt=false shift=false
    [false, false, false], // 08: cmd=false ctrl=false logo=false alt=true  shift=false
    [false, false, false], // 09: cmd=true  ctrl=false logo=false alt=true  shift=false
    [false, false, false], // 10: cmd=false ctrl=true  logo=false alt=true  shift=false
    [false, false, false], // 11: cmd=true  ctrl=true  logo=false alt=true  shift=false
    [false, false, false], // 12: cmd=false ctrl=false logo=true  alt=true  shift=false
    [false, false, false], // 13: cmd=true  ctrl=false logo=true  alt=true  shift=false
    [false, false, false], // 14: cmd=false ctrl=true  logo=true  alt=true  shift=false
    [false, false, false], // 15: cmd=true  ctrl=true  logo=true  alt=true  shift=false
    [true, true, true],    // 16: cmd=false ctrl=false logo=false alt=false shift=true
    [true, true, true],    // 17: cmd=true  ctrl=false logo=false alt=false shift=true
    [false, true, true],   // 18: cmd=false ctrl=true  logo=false alt=false shift=true
    [false, true, true],   // 19: cmd=true  ctrl=true  logo=false alt=false shift=true
    [true, false, false],  // 20: cmd=false ctrl=false logo=true  alt=false shift=true
    [false, false, false], // 21: cmd=true  ctrl=false logo=true  alt=false shift=true
    [false, false, false], // 22: cmd=false ctrl=true  logo=true  alt=false shift=true
    [false, false, false], // 23: cmd=true  ctrl=true  logo=true  alt=false shift=true
    [false, false, false], // 24: cmd=false ctrl=false logo=false alt=true  shift=true
    [false, false, false], // 25: cmd=true  ctrl=false logo=false alt=true  shift=true
    [false, false, false], // 26: cmd=false ctrl=true  logo=false alt=true  shift=true
    [false, false, false], // 27: cmd=true  ctrl=true  logo=false alt=true  shift=true
    [false, false, false], // 28: cmd=false ctrl=false logo=true  alt=true  shift=true
    [false, false, false], // 29: cmd=true  ctrl=false logo=true  alt=true  shift=true
    [false, false, false], // 30: cmd=false ctrl=true  logo=true  alt=true  shift=true
    [false, false, false], // 31: cmd=true  ctrl=true  logo=true  alt=true  shift=true
];

/// Expected `[Mac, Windows, Linux]` results of `primary_alone_on` on macOS, where `cmd` repeats
/// `logo` (⌘).
const EXPECTED_MACOS: [[bool; 3]; 32] = [
    [true, true, true],    // 00: cmd=false ctrl=false logo=false alt=false shift=false
    [true, true, true],    // 01: cmd=true  ctrl=false logo=false alt=false shift=false
    [false, true, true],   // 02: cmd=false ctrl=true  logo=false alt=false shift=false
    [false, false, false], // 03: cmd=true  ctrl=true  logo=false alt=false shift=false
    [true, false, false],  // 04: cmd=false ctrl=false logo=true  alt=false shift=false
    [true, false, false],  // 05: cmd=true  ctrl=false logo=true  alt=false shift=false
    [false, false, false], // 06: cmd=false ctrl=true  logo=true  alt=false shift=false
    [false, false, false], // 07: cmd=true  ctrl=true  logo=true  alt=false shift=false
    [false, false, false], // 08: cmd=false ctrl=false logo=false alt=true  shift=false
    [false, false, false], // 09: cmd=true  ctrl=false logo=false alt=true  shift=false
    [false, false, false], // 10: cmd=false ctrl=true  logo=false alt=true  shift=false
    [false, false, false], // 11: cmd=true  ctrl=true  logo=false alt=true  shift=false
    [false, false, false], // 12: cmd=false ctrl=false logo=true  alt=true  shift=false
    [false, false, false], // 13: cmd=true  ctrl=false logo=true  alt=true  shift=false
    [false, false, false], // 14: cmd=false ctrl=true  logo=true  alt=true  shift=false
    [false, false, false], // 15: cmd=true  ctrl=true  logo=true  alt=true  shift=false
    [true, true, true],    // 16: cmd=false ctrl=false logo=false alt=false shift=true
    [true, true, true],    // 17: cmd=true  ctrl=false logo=false alt=false shift=true
    [false, true, true],   // 18: cmd=false ctrl=true  logo=false alt=false shift=true
    [false, false, false], // 19: cmd=true  ctrl=true  logo=false alt=false shift=true
    [true, false, false],  // 20: cmd=false ctrl=false logo=true  alt=false shift=true
    [true, false, false],  // 21: cmd=true  ctrl=false logo=true  alt=false shift=true
    [false, false, false], // 22: cmd=false ctrl=true  logo=true  alt=false shift=true
    [false, false, false], // 23: cmd=true  ctrl=true  logo=true  alt=false shift=true
    [false, false, false], // 24: cmd=false ctrl=false logo=false alt=true  shift=true
    [false, false, false], // 25: cmd=true  ctrl=false logo=false alt=true  shift=true
    [false, false, false], // 26: cmd=false ctrl=true  logo=false alt=true  shift=true
    [false, false, false], // 27: cmd=true  ctrl=true  logo=false alt=true  shift=true
    [false, false, false], // 28: cmd=false ctrl=false logo=true  alt=true  shift=true
    [false, false, false], // 29: cmd=true  ctrl=false logo=true  alt=true  shift=true
    [false, false, false], // 30: cmd=false ctrl=true  logo=true  alt=true  shift=true
    [false, false, false], // 31: cmd=true  ctrl=true  logo=true  alt=true  shift=true
];

/// Expected `[Mac, Windows, Linux]` results of `primary_alone_on` on wasm32, where `cmd` can
/// repeat either `ctrl` or `logo`.
const EXPECTED_WASM: [[bool; 3]; 32] = [
    [true, true, true],    // 00: cmd=false ctrl=false logo=false alt=false shift=false
    [true, true, true],    // 01: cmd=true  ctrl=false logo=false alt=false shift=false
    [false, true, true],   // 02: cmd=false ctrl=true  logo=false alt=false shift=false
    [false, true, true],   // 03: cmd=true  ctrl=true  logo=false alt=false shift=false
    [true, false, false],  // 04: cmd=false ctrl=false logo=true  alt=false shift=false
    [true, false, false],  // 05: cmd=true  ctrl=false logo=true  alt=false shift=false
    [false, false, false], // 06: cmd=false ctrl=true  logo=true  alt=false shift=false
    [false, false, false], // 07: cmd=true  ctrl=true  logo=true  alt=false shift=false
    [false, false, false], // 08: cmd=false ctrl=false logo=false alt=true  shift=false
    [false, false, false], // 09: cmd=true  ctrl=false logo=false alt=true  shift=false
    [false, false, false], // 10: cmd=false ctrl=true  logo=false alt=true  shift=false
    [false, false, false], // 11: cmd=true  ctrl=true  logo=false alt=true  shift=false
    [false, false, false], // 12: cmd=false ctrl=false logo=true  alt=true  shift=false
    [false, false, false], // 13: cmd=true  ctrl=false logo=true  alt=true  shift=false
    [false, false, false], // 14: cmd=false ctrl=true  logo=true  alt=true  shift=false
    [false, false, false], // 15: cmd=true  ctrl=true  logo=true  alt=true  shift=false
    [true, true, true],    // 16: cmd=false ctrl=false logo=false alt=false shift=true
    [true, true, true],    // 17: cmd=true  ctrl=false logo=false alt=false shift=true
    [false, true, true],   // 18: cmd=false ctrl=true  logo=false alt=false shift=true
    [false, true, true],   // 19: cmd=true  ctrl=true  logo=false alt=false shift=true
    [true, false, false],  // 20: cmd=false ctrl=false logo=true  alt=false shift=true
    [true, false, false],  // 21: cmd=true  ctrl=false logo=true  alt=false shift=true
    [false, false, false], // 22: cmd=false ctrl=true  logo=true  alt=false shift=true
    [false, false, false], // 23: cmd=true  ctrl=true  logo=true  alt=false shift=true
    [false, false, false], // 24: cmd=false ctrl=false logo=false alt=true  shift=true
    [false, false, false], // 25: cmd=true  ctrl=false logo=false alt=true  shift=true
    [false, false, false], // 26: cmd=false ctrl=true  logo=false alt=true  shift=true
    [false, false, false], // 27: cmd=true  ctrl=true  logo=false alt=true  shift=true
    [false, false, false], // 28: cmd=false ctrl=false logo=true  alt=true  shift=true
    [false, false, false], // 29: cmd=true  ctrl=false logo=true  alt=true  shift=true
    [false, false, false], // 30: cmd=false ctrl=true  logo=true  alt=true  shift=true
    [false, false, false], // 31: cmd=true  ctrl=true  logo=true  alt=true  shift=true
];

#[test]
fn primary_alone_matches_truth_table_for_all_modifier_combinations_and_platforms() {
    let table = if cfg!(target_arch = "wasm32") {
        &EXPECTED_WASM
    } else if cfg!(target_os = "macos") {
        &EXPECTED_MACOS
    } else {
        &EXPECTED_OTHER
    };

    for bits in 0u8..32 {
        let mods = Mods {
            cmd: bits & 1 != 0,
            ctrl: bits & 2 != 0,
            logo: bits & 4 != 0,
            alt: bits & 8 != 0,
            shift: bits & 16 != 0,
        };
        for (idx, platform) in PLATFORMS.into_iter().enumerate() {
            assert_eq!(
                primary_alone_on(mods, platform),
                table[bits as usize][idx],
                "bits={bits:02} {mods:?} on {platform:?}"
            );
        }
        assert_eq!(
            primary_alone(mods),
            primary_alone_on(mods, KeyPlatform::current()),
            "current() wrapper mismatch for bits={bits:02} {mods:?}"
        );
    }
}

#[test]
fn fold_merges_primary_alias_per_platform() {
    for bits in 0u8..32 {
        assert_eq!(fold(bits, KeyPlatform::Mac), if bits & SUPER != 0 { (bits & !SUPER) | PRIMARY } else { bits });
        assert_eq!(fold(bits, KeyPlatform::Windows), if bits & CTRL != 0 { (bits & !CTRL) | PRIMARY } else { bits });
        assert_eq!(fold(bits, KeyPlatform::Linux), if bits & CTRL != 0 { (bits & !CTRL) | PRIMARY } else { bits });
    }
}

#[test]
fn is_command_chord_matches_platform_alt_and_altgr_conventions() {
    let text = |s: &str, mods: Mods| KeyEvent { key: Key::Text(s.into()), mods };
    let cases = [
        // Plain and Shift text (including Linux and Windows Right-Alt AltGr) always types.
        (text("d", Mods::default()), [false, false, false]),
        (text("D", Mods { shift: true, ..Mods::default() }), [false, false, false]),
        // Alt alone (or with Shift/Super) types on macOS (Option), but is a chord elsewhere.
        (text("d", Mods { alt: true, ..Mods::default() }), [false, true, true]),
        (text("D", Mods { alt: true, shift: true, ..Mods::default() }), [false, true, true]),
        (text("d", Mods { alt: true, logo: true, ..Mods::default() }), [false, true, true]),
        // Ctrl+Alt is AltGr off macOS (types), but a command chord on macOS.
        (text("€", Mods { ctrl: true, cmd: true, alt: true, ..Mods::default() }), [true, false, false]),
        (text("€", Mods { ctrl: true, alt: true, ..Mods::default() }), [true, false, false]),
        // Cmd or Ctrl without Alt is always a command chord.
        (text(" ", Mods { cmd: true, ..Mods::default() }), [true, true, true]),
        (text("a", Mods { ctrl: true, ..Mods::default() }), [true, true, true]),
        // Non-text keys are never text command chords.
        (KeyEvent { key: Key::Left, mods: Mods { alt: true, ..Mods::default() } }, [false, false, false]),
    ];
    for (ev, want) in cases {
        for (idx, platform) in PLATFORMS.into_iter().enumerate() {
            assert_eq!(
                ev.is_command_chord_on(platform),
                want[idx],
                "{ev:?} on {platform:?}"
            );
        }
        assert_eq!(ev.is_command_chord(), ev.is_command_chord_on(KeyPlatform::current()), "{ev:?}");
    }
}
