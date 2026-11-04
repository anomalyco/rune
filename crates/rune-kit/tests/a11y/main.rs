//! What every rune-kit component tells assistive technology: role, name, value, state and the
//! actions a screen reader runs, read from `Runtime::accessibility_json` and driven with
//! `Runtime::accessibility_action`, pointer and keys. Each scene's tree (without bounds) is
//! also a golden under `tests/a11y/golden/`; accept an intended change with `RUNE_BLESS=1`
//! and review the diff.
//!
//! Run: `cargo test -p rune-kit --features markdown --test a11y`.

mod controls;
mod data;
mod disclosure;
#[cfg(feature = "markdown")]
mod docs;
mod overlays;
mod shell;
mod support;
