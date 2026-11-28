//! Android `EditorInfo` (`inputType` / `imeOptions`) mapping and soft-keyboard sampling.
//!
//! Compiles on every host target so field-kind mapping (`InputMode`, `single_line`, `El::secure`),
//! `InputMode::None` keyboard suppression, and re-tap detection are verified by unit tests
//! off-device.

use crate::InputMode;
use crate::runtime::RuntimeCore;

// Android `android.text.InputType` constants.
pub(crate) const TYPE_CLASS_TEXT: i32 = 0x0000_0001;
pub(crate) const TYPE_CLASS_NUMBER: i32 = 0x0000_0002;
pub(crate) const TYPE_CLASS_PHONE: i32 = 0x0000_0003;
pub(crate) const TYPE_TEXT_VARIATION_URI: i32 = 0x0000_0010;
pub(crate) const TYPE_TEXT_VARIATION_EMAIL_ADDRESS: i32 = 0x0000_0020;
pub(crate) const TYPE_TEXT_VARIATION_PASSWORD: i32 = 0x0000_0080;
pub(crate) const TYPE_TEXT_FLAG_MULTI_LINE: i32 = 0x0002_0000;
pub(crate) const TYPE_NUMBER_VARIATION_PASSWORD: i32 = 0x0000_0010;
pub(crate) const TYPE_NUMBER_FLAG_DECIMAL: i32 = 0x0000_2000;

// Android `android.view.inputmethod.EditorInfo` constants.
pub(crate) const IME_ACTION_SEARCH: i32 = 0x0000_0003;
pub(crate) const IME_ACTION_DONE: i32 = 0x0000_0006;
pub(crate) const IME_FLAG_NO_PERSONALIZED_LEARNING: i32 = 0x0100_0000;
pub(crate) const IME_FLAG_NO_FULLSCREEN: i32 = 0x0200_0000;
pub(crate) const IME_FLAG_NO_EXTRACT_UI: i32 = 0x1000_0000;

/// Android `EditorInfo` attributes for an active editable text field or `El::ime_cursor` host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EditorConfig {
    pub(crate) input_type: i32,
    pub(crate) ime_options: i32,
}

impl EditorConfig {
    /// Default multi-line text keyboard configuration used by multi-line `InputMode::Text`
    /// fields and app-owned `El::ime_cursor` editors.
    pub(crate) const fn default_multiline() -> Self {
        Self {
            input_type: TYPE_CLASS_TEXT | TYPE_TEXT_FLAG_MULTI_LINE,
            ime_options: IME_FLAG_NO_EXTRACT_UI | IME_FLAG_NO_FULLSCREEN,
        }
    }
}

/// Snapshot of Android soft-keyboard state sampled from `RuntimeCore` after a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct EditorSnapshot {
    /// Active soft-keyboard configuration, or `None` when no editable field wants the keyboard.
    pub(crate) config: Option<EditorConfig>,
    /// Whether the runtime requested an input-method reset since the previous frame (focus moved
    /// between fields or composition was finished externally).
    pub(crate) reset: bool,
    /// Whether a finger tap landed on an editable text field since the previous frame, so the
    /// host can re-show a keyboard hidden by Back while the field kept focus.
    pub(crate) field_tap: bool,
}

/// Maps Rune's `InputMode`, `single_line`, and `El::secure` state to Android's
/// `EditorInfo.inputType` and `EditorInfo.imeOptions`.
///
/// Returns `None` for `InputMode::None` so no soft keyboard is raised while hardware keyboard
/// events continue to arrive through `RuneView.onKeyDown`.
pub(crate) const fn editor_config(
    mode: InputMode,
    single_line: bool,
    secure: bool,
) -> Option<EditorConfig> {
    if matches!(mode, InputMode::None) {
        return None;
    }

    let input_type = if secure {
        match mode {
            InputMode::Numeric => TYPE_CLASS_NUMBER | TYPE_NUMBER_VARIATION_PASSWORD,
            InputMode::Decimal => {
                TYPE_CLASS_NUMBER | TYPE_NUMBER_VARIATION_PASSWORD | TYPE_NUMBER_FLAG_DECIMAL
            }
            InputMode::Text
            | InputMode::Email
            | InputMode::Url
            | InputMode::Search
            | InputMode::Tel => {
                let base = TYPE_CLASS_TEXT | TYPE_TEXT_VARIATION_PASSWORD;
                if single_line || matches!(mode, InputMode::Search) {
                    base
                } else {
                    base | TYPE_TEXT_FLAG_MULTI_LINE
                }
            }
            InputMode::None => unreachable!(),
        }
    } else {
        match mode {
            InputMode::Text => {
                if single_line {
                    TYPE_CLASS_TEXT
                } else {
                    TYPE_CLASS_TEXT | TYPE_TEXT_FLAG_MULTI_LINE
                }
            }
            InputMode::Email => TYPE_CLASS_TEXT | TYPE_TEXT_VARIATION_EMAIL_ADDRESS,
            InputMode::Url => TYPE_CLASS_TEXT | TYPE_TEXT_VARIATION_URI,
            InputMode::Search => TYPE_CLASS_TEXT,
            InputMode::Numeric => TYPE_CLASS_NUMBER,
            InputMode::Decimal => TYPE_CLASS_NUMBER | TYPE_NUMBER_FLAG_DECIMAL,
            InputMode::Tel => TYPE_CLASS_PHONE,
            InputMode::None => unreachable!(),
        }
    };

    let mut ime_options = IME_FLAG_NO_EXTRACT_UI | IME_FLAG_NO_FULLSCREEN;
    if secure {
        ime_options |= IME_FLAG_NO_PERSONALIZED_LEARNING;
    }
    if matches!(mode, InputMode::Search) {
        ime_options |= IME_ACTION_SEARCH;
    } else if single_line {
        ime_options |= IME_ACTION_DONE;
    }

    Some(EditorConfig {
        input_type,
        ime_options,
    })
}

/// Returns the active Android `EditorConfig` for `rt`, if an editable text field (including a
/// secure password/PIN field) or an app-owned `El::ime_cursor` wants a soft keyboard.
pub(crate) fn active_editor_config(rt: &RuntimeCore) -> Option<EditorConfig> {
    if let Some((mode, single_line, secure)) = rt.focused_editable_input() {
        return editor_config(mode, single_line, secure);
    }
    rt.ime_area.map(|_| EditorConfig::default_multiline())
}

/// Samples the current Android soft-keyboard configuration and consumes pending `ime_reset` and
/// `take_field_tap` flags from `rt`.
pub(crate) fn sample_editor(rt: &mut RuntimeCore) -> EditorSnapshot {
    let reset = std::mem::take(&mut rt.edit.ime_reset);
    let field_tap = rt.take_field_tap();
    let config = active_editor_config(rt);
    EditorSnapshot {
        config,
        reset,
        field_tap,
    }
}
