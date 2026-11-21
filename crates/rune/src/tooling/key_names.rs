//! Key names, modifiers and unmodified characters in headless scripts and `--keys`.
use crate::edit::Key;
use crate::input::key_table;

pub(crate) fn key_named(name: &str) -> Option<Key> {
    key_table::named_key_from_str(name).or_else(|| {
        (name.chars().count() == 1).then(|| Key::Text(name.to_owned()))
    })
}

/// A script step's modifier flags. `"cmd"` is the primary shortcut modifier, held as the shell
/// reports it: with Super on macOS and with Ctrl elsewhere, so a script's "mod+k" works on every
/// platform.
pub(crate) fn script_mods(e: &serde_json::Value) -> crate::Mods {
    let flag = |k: &str| e[k].as_bool().unwrap_or(false);
    key_table::mods_from_script(flag("shift"), flag("ctrl"), flag("alt"), flag("logo"), flag("cmd"))
}

/// A script key's `Tx::key_base`: its `"base"`, else a one-character name typed without Shift
/// or Option (which change the character the key types), lowercased.
pub(crate) fn script_key_base(e: &serde_json::Value, name: &str, mods: crate::Mods) -> Option<char> {
    let one = |s: &str| { let mut c = s.chars(); c.next().filter(|_| c.next().is_none()) };
    match e["base"].as_str() {
        Some(base) => one(base),
        None => one(name).filter(|_| !mods.shift && !mods.alt).map(|c| c.to_ascii_lowercase()),
    }
}
