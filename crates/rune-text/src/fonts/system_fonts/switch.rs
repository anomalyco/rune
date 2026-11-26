//! Whether system fonts may load at all: `RUNE_SYSTEM_FONTS=off` or [`set_system_fonts`]`(false)`
//! keeps every text system to its bundled and registered fonts.

use std::sync::atomic::{AtomicU8, Ordering};

/// 0: not decided yet (read `RUNE_SYSTEM_FONTS` on first use), 1: on, 2: off.
static STATE: AtomicU8 = AtomicU8::new(0);

/// Turns system font lookup on or off for the whole process.
///
/// Off, no text system loads a system font: not for a `families` slot that names an installed
/// family, not for `ui-monospace` or `Family::SystemMono` (they fall back to the sans slot), and
/// not as a fallback for scripts, CJK, symbols or emoji. Text uses only the bundled faces,
/// `AppSpec::fonts` and fonts added with `add_font`; a character none of them has draws as
/// the font's missing glyph. Fonts already loaded stay, so call it before the first text
/// lays out (`RUNE_SYSTEM_FONTS=off` in the environment does the same from the start).
///
/// Headless runs whose output should match across operating systems (`--tree` goldens) turn
/// it off.
pub fn set_system_fonts(enabled: bool) {
    STATE.store(if enabled { 1 } else { 2 }, Ordering::Relaxed);
}

/// Whether system fonts may load: [`set_system_fonts`]'s last value, else off when
/// `RUNE_SYSTEM_FONTS` is `off`, `0`, `false` or `no`, else on.
pub fn system_fonts_enabled() -> bool {
    match STATE.load(Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => {
            let enabled = !from_env(rune_core::env::var("RUNE_SYSTEM_FONTS").as_deref());
            // A concurrent `set_system_fonts` wins over the environment.
            match STATE.compare_exchange(0, if enabled { 1 } else { 2 }, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => enabled,
                Err(set) => set == 1,
            }
        }
    }
}

/// Whether a `RUNE_SYSTEM_FONTS` value turns system fonts off.
pub(super) fn from_env(value: Option<&str>) -> bool {
    value.is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "off" | "0" | "false" | "no"))
}

