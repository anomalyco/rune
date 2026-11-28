//! Android `InputConnection` text-window cache, UTF-16 offset translation, and
//! recomposition/range-delete planning.
//!
//! Compiles on every host target so UTF-16/UTF-8 surrogate-pair conversions, mid-composition
//! queries from the pre-composition paragraph window, `setComposingRegion`, `setSelection`,
//! `deleteSurroundingText` / `deleteSurroundingTextInCodePoints`, `updateSelection`, and
//! paragraph-change `restartInput` detection are verified off-device without an emulator.

use std::ops::Range;

use crate::edit::{Ime, ImeText};
use crate::runtime::RuntimeCore;
use crate::{Key, KeyEvent};

use super::input::{
    MAX_DELETE_SURROUNDING, TextInputAction, apply_text_input, clamp_delete_count,
    commit_text_event, composing_text_event,
};

/// UTF-16 selection and composing-region offsets in the active paragraph window, matching
/// `InputMethodManager.updateSelection(view, selStart, selEnd, candidatesStart, candidatesEnd)`.
/// `composing_start` and `composing_end` are `-1` when no composition is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SelectionState {
    pub sel_start: i32,
    pub sel_end: i32,
    pub composing_start: i32,
    pub composing_end: i32,
}

impl SelectionState {
    #[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
    pub(crate) fn to_array(self) -> [i32; 4] {
        [
            self.sel_start,
            self.sel_end,
            self.composing_start,
            self.composing_end,
        ]
    }
}

/// Post-frame synchronization instructions emitted by [`ImeWindow::sync_after_frame`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct WindowSync {
    /// True when the paragraph window's byte origin (`ImeText::start`) moved while IME stayed
    /// enabled (e.g. caret crossed a newline or slid past the 1 KiB window edge), so Android
    /// must call `InputMethodManager.restartInput(view)` to reset its UTF-16 offset origin.
    pub restart_input: bool,
    /// Updated selection/composing offsets to deliver via `InputMethodManager.updateSelection`.
    /// `None` when unchanged or when `restartInput` is already firing on the same frame.
    pub update_selection: Option<SelectionState>,
}

impl WindowSync {
    pub(crate) const NONE: Self = Self {
        restart_input: false,
        update_selection: None,
    };
}

/// One `InputConnection` edit request routed through [`ImeWindow`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ImeEdit {
    CommitText(String),
    SetComposingText {
        text: String,
        new_cursor_position: i32,
    },
    FinishComposingText,
    SetComposingRegion {
        start: i32,
        end: i32,
    },
    SetSelection {
        start: i32,
        end: i32,
    },
    DeleteSurrounding {
        before: i32,
        after: i32,
        in_code_points: bool,
    },
}

/// Active preedit spliced over `base.selection` while `TextEdit::ime_text()` returns `None`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ActiveComposition {
    text: String,
    /// Caret or selection byte range inside `text`.
    cursor: (usize, usize),
}

#[derive(Clone, Debug, Default, PartialEq)]
enum WindowPhase {
    /// No readable field window (unfocused, secure, read-only, or app-owned `App::on_ime`).
    #[default]
    Inactive,
    /// Committed paragraph window from `RuntimeCore::ime_text()`.
    Committed(ImeText),
    /// Open composition: `base` is the pre-composition paragraph window kept because
    /// `TextEdit::ime_text()` returns `None` while composing, and `comp` is spliced over
    /// `base.selection`.
    Composing {
        base: ImeText,
        comp: ActiveComposition,
    },
}

/// Visible paragraph snapshot derived from [`WindowPhase`], with all ranges in UTF-8 bytes.
struct VisibleWindow {
    text: String,
    selection: Range<usize>,
    composing: Option<Range<usize>>,
}

/// Host-side Android IME paragraph-window state machine.
///
/// Tracks the focused field's current paragraph window (`ImeText`, up to 1 KiB either side of the
/// selection), preserves the pre-composition window while a composition is open, applies
/// optimistic shadow updates across synchronous `InputConnection` batch edits, and re-anchors to
/// `RuntimeCore` after every drawn frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ImeWindow {
    phase: WindowPhase,
    last_reported: Option<SelectionState>,
}

/// Total UTF-16 code units in `s`.
pub(crate) fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Converts a UTF-8 byte offset in `s` (clamped and floored to a char boundary) into its
/// UTF-16 code-unit offset.
pub(crate) fn byte_to_utf16(s: &str, byte: usize) -> usize {
    let mut b = byte.min(s.len());
    while b > 0 && !s.is_char_boundary(b) {
        b -= 1;
    }
    s[..b].encode_utf16().count()
}

/// Converts a UTF-16 code-unit offset into a UTF-8 byte offset in `s`, rounding DOWN to the
/// start of any supplementary character whose surrogate pair would be split.
pub(crate) fn utf16_to_byte_floor(s: &str, utf16: usize) -> usize {
    let mut acc = 0usize;
    for (idx, ch) in s.char_indices() {
        let len16 = ch.len_utf16();
        if acc + len16 > utf16 {
            return idx;
        }
        acc += len16;
    }
    s.len()
}

/// Converts a UTF-16 code-unit offset into a UTF-8 byte offset in `s`, rounding UP to the
/// end of any supplementary character whose surrogate pair would be split.
pub(crate) fn utf16_to_byte_ceil(s: &str, utf16: usize) -> usize {
    let mut acc = 0usize;
    for (idx, ch) in s.char_indices() {
        if acc >= utf16 {
            return idx;
        }
        acc += ch.len_utf16();
    }
    s.len()
}

fn clamp_char_boundary(s: &str, byte: usize) -> usize {
    let mut b = byte.min(s.len());
    while b > 0 && !s.is_char_boundary(b) {
        b -= 1;
    }
    b
}

fn step_back_chars(s: &str, from: usize, count: usize) -> (usize, usize) {
    let mut pos = clamp_char_boundary(s, from);
    let mut consumed = 0usize;
    for (idx, _) in s[..pos].char_indices().rev() {
        if consumed >= count {
            break;
        }
        pos = idx;
        consumed += 1;
    }
    (pos, consumed)
}

fn step_forward_chars(s: &str, from: usize, count: usize) -> (usize, usize) {
    let base = clamp_char_boundary(s, from);
    let mut pos = base;
    let mut consumed = 0usize;
    for (rel, ch) in s[base..].char_indices() {
        if consumed >= count {
            break;
        }
        pos = base + rel + ch.len_utf8();
        consumed += 1;
    }
    (pos, consumed)
}

fn utf16_to_i32(offset: usize) -> i32 {
    i32::try_from(offset).unwrap_or(i32::MAX)
}

fn normalize_utf16_bounds(s: &str, start_u16: i32, end_u16: i32) -> (usize, usize) {
    let total = utf16_len(s);
    let a = (start_u16.max(0) as usize).min(total);
    let b = (end_u16.max(0) as usize).min(total);
    if a <= b { (a, b) } else { (b, a) }
}

impl ImeWindow {
    /// Clears all cached window and selection state (on blur, pause, or destroy).
    pub(crate) fn clear(&mut self) {
        self.phase = WindowPhase::Inactive;
        self.last_reported = None;
    }

    #[cfg(test)]
    pub(crate) fn is_active(&self) -> bool {
        !matches!(self.phase, WindowPhase::Inactive)
    }

    #[cfg(test)]
    pub(crate) fn is_composing(&self) -> bool {
        matches!(self.phase, WindowPhase::Composing { .. })
    }

    fn visible(&self) -> Option<VisibleWindow> {
        match &self.phase {
            WindowPhase::Inactive => None,
            WindowPhase::Committed(base) => {
                let a = clamp_char_boundary(&base.text, base.selection.start);
                let b = clamp_char_boundary(&base.text, base.selection.end).max(a);
                Some(VisibleWindow {
                    text: base.text.clone(),
                    selection: a..b,
                    composing: None,
                })
            }
            WindowPhase::Composing { base, comp } => {
                let a = clamp_char_boundary(&base.text, base.selection.start);
                let b = clamp_char_boundary(&base.text, base.selection.end).max(a);
                let mut text =
                    String::with_capacity(base.text.len() - (b - a) + comp.text.len());
                text.push_str(&base.text[..a]);
                text.push_str(&comp.text);
                text.push_str(&base.text[b..]);
                let comp_end = a + comp.text.len();
                let cur_a = clamp_char_boundary(&comp.text, comp.cursor.0);
                let cur_b = clamp_char_boundary(&comp.text, comp.cursor.1).max(cur_a);
                Some(VisibleWindow {
                    text,
                    selection: (a + cur_a)..(a + cur_b),
                    composing: Some(a..comp_end),
                })
            }
        }
    }

    /// Full visible text of the active paragraph window (including any open preedit), or `None`
    /// when no readable text field is focused.
    pub(crate) fn window_text(&self) -> Option<String> {
        self.visible().map(|v| v.text)
    }

    /// Up to `max_utf16` UTF-16 code units before the selection start, never splitting a
    /// surrogate pair. Returns `None` if `max_utf16 < 0` or the window is inactive.
    pub(crate) fn text_before_cursor(&self, max_utf16: i32) -> Option<String> {
        if max_utf16 < 0 {
            return None;
        }
        let v = self.visible()?;
        let n = max_utf16 as usize;
        if n == 0 || v.selection.start == 0 {
            return Some(String::new());
        }
        let sel_u16 = byte_to_utf16(&v.text, v.selection.start);
        let start = utf16_to_byte_ceil(&v.text, sel_u16.saturating_sub(n)).min(v.selection.start);
        Some(v.text[start..v.selection.start].to_owned())
    }

    /// Up to `max_utf16` UTF-16 code units after the selection end, never splitting a surrogate
    /// pair. Returns `None` if `max_utf16 < 0` or the window is inactive.
    pub(crate) fn text_after_cursor(&self, max_utf16: i32) -> Option<String> {
        if max_utf16 < 0 {
            return None;
        }
        let v = self.visible()?;
        let n = max_utf16 as usize;
        if n == 0 || v.selection.end >= v.text.len() {
            return Some(String::new());
        }
        let sel_end_u16 = byte_to_utf16(&v.text, v.selection.end);
        let end = utf16_to_byte_floor(&v.text, sel_end_u16.saturating_add(n)).max(v.selection.end);
        Some(v.text[v.selection.end..end].to_owned())
    }

    /// Currently selected text, or `None` when the caret is collapsed or the window is inactive.
    pub(crate) fn selected_text(&self) -> Option<String> {
        let v = self.visible()?;
        if v.selection.is_empty() {
            return None;
        }
        Some(v.text[v.selection].to_owned())
    }

    /// Current UTF-16 selection and composing-region offsets in the active paragraph window, or
    /// `None` when inactive.
    pub(crate) fn selection_state(&self) -> Option<SelectionState> {
        let v = self.visible()?;
        let sel_start = utf16_to_i32(byte_to_utf16(&v.text, v.selection.start));
        let sel_end = utf16_to_i32(byte_to_utf16(&v.text, v.selection.end));
        let (composing_start, composing_end) = match v.composing {
            Some(r) => (
                utf16_to_i32(byte_to_utf16(&v.text, r.start)),
                utf16_to_i32(byte_to_utf16(&v.text, r.end)),
            ),
            None => (-1, -1),
        };
        Some(SelectionState {
            sel_start,
            sel_end,
            composing_start,
            composing_end,
        })
    }

    /// Reconciles the cached window with `RuntimeCore` after a frame completes.
    ///
    /// - When `ime_text` is `Some(next)`, updates the committed window and signals
    ///   `restart_input` if `next.start` shifted from the previous window's origin.
    /// - When `ime_text` is `None` while `preedit` is `Some`, retains the pre-composition
    ///   paragraph window and splices the active preedit over it.
    /// - Emits `update_selection` whenever UTF-16 selection or composing offsets changed and no
    ///   `restartInput` is already firing on the same frame.
    pub(crate) fn sync_after_frame(
        &mut self,
        ime_allowed: bool,
        ime_text: Option<ImeText>,
        preedit: Option<(&str, Option<(usize, usize)>)>,
        host_restarted: bool,
    ) -> WindowSync {
        if !ime_allowed {
            self.clear();
            return WindowSync::NONE;
        }

        let prev_start = match &self.phase {
            WindowPhase::Inactive => None,
            WindowPhase::Committed(base) | WindowPhase::Composing { base, .. } => Some(base.start),
        };

        let origin_changed = match (ime_text, preedit) {
            (Some(next), _) => {
                let changed = prev_start.is_some_and(|prev| prev != next.start);
                self.phase = WindowPhase::Committed(next);
                changed
            }
            (None, Some((text, cursor))) => {
                let cur = match cursor {
                    Some((a, b)) => {
                        let a = clamp_char_boundary(text, a);
                        let b = clamp_char_boundary(text, b).max(a);
                        (a, b)
                    }
                    None => (text.len(), text.len()),
                };
                let comp = ActiveComposition {
                    text: text.to_owned(),
                    cursor: cur,
                };
                let base = match std::mem::take(&mut self.phase) {
                    WindowPhase::Committed(base) | WindowPhase::Composing { base, .. } => base,
                    WindowPhase::Inactive => ImeText {
                        text: String::new(),
                        start: 0,
                        selection: 0..0,
                    },
                };
                self.phase = WindowPhase::Composing { base, comp };
                false
            }
            (None, None) => {
                self.clear();
                return WindowSync::NONE;
            }
        };

        let Some(sel) = self.selection_state() else {
            self.last_reported = None;
            return WindowSync::NONE;
        };

        if host_restarted || origin_changed {
            self.last_reported = Some(sel);
            return WindowSync {
                restart_input: origin_changed,
                update_selection: None,
            };
        }

        if self.last_reported != Some(sel) {
            self.last_reported = Some(sel);
            WindowSync {
                restart_input: false,
                update_selection: Some(sel),
            }
        } else {
            WindowSync::NONE
        }
    }

    /// Folds an open composition into its pre-composition base window and returns whether a
    /// composition was closed, the visible selection range inside the folded text, and the byte
    /// offset where `FinishCompositionFromIme` places the caret.
    fn fold_composing_into_committed(&mut self) -> Option<(bool, Range<usize>, usize)> {
        match std::mem::take(&mut self.phase) {
            WindowPhase::Inactive => None,
            WindowPhase::Committed(base) => {
                let sel = base.selection.clone();
                let caret = sel.end;
                self.phase = WindowPhase::Committed(base);
                Some((false, sel, caret))
            }
            WindowPhase::Composing { mut base, comp } => {
                let a = clamp_char_boundary(&base.text, base.selection.start);
                let b = clamp_char_boundary(&base.text, base.selection.end).max(a);
                let cur_a = clamp_char_boundary(&comp.text, comp.cursor.0);
                let cur_b = clamp_char_boundary(&comp.text, comp.cursor.1).max(cur_a);
                base.text.replace_range(a..b, &comp.text);
                let after_finish_caret = a + comp.text.len();
                let visible_sel = (a + cur_a)..(a + cur_b);
                base.selection = visible_sel.clone();
                self.phase = WindowPhase::Committed(base);
                Some((true, visible_sel, after_finish_caret))
            }
        }
    }

    /// Translates one Android `InputConnection` edit into ordered [`TextInputAction`]s while
    /// keeping the optimistic paragraph shadow in sync for subsequent queries in the same batch.
    pub(crate) fn apply_edit(&mut self, edit: ImeEdit) -> Vec<TextInputAction> {
        match edit {
            ImeEdit::CommitText(text) => self.edit_commit_text(text),
            ImeEdit::SetComposingText {
                text,
                new_cursor_position,
            } => self.edit_set_composing_text(text, new_cursor_position),
            ImeEdit::FinishComposingText => self.edit_finish_composing_text(),
            ImeEdit::SetComposingRegion { start, end } => {
                self.edit_set_composing_region(start, end)
            }
            ImeEdit::SetSelection { start, end } => self.edit_set_selection(start, end),
            ImeEdit::DeleteSurrounding {
                before,
                after,
                in_code_points,
            } => self.edit_delete_surrounding(before, after, in_code_points),
        }
    }

    /// Handles empty `commitText("")` or `setComposingText("", _)`: when a non-empty range is
    /// selected (e.g. from `setComposingRegion`), deletes it with one Backspace because
    /// `TextEdit::commit_composed` ignores empty strings; otherwise clears any open preedit.
    fn clear_or_delete_empty(&mut self, mut base: ImeText, a: usize, b: usize) -> Vec<TextInputAction> {
        if a < b {
            base.text.replace_range(a..b, "");
            base.selection = a..a;
            self.phase = WindowPhase::Committed(base);
            vec![TextInputAction::KeyDown { event: KeyEvent::from(Key::Backspace), repeat: false }]
        } else {
            base.selection = a..a;
            self.phase = WindowPhase::Committed(base);
            vec![TextInputAction::Ime(Ime::Preedit(String::new(), None, Vec::new()))]
        }
    }

    fn edit_commit_text(&mut self, text: String) -> Vec<TextInputAction> {
        match std::mem::take(&mut self.phase) {
            WindowPhase::Inactive => vec![TextInputAction::Ime(commit_text_event(text))],
            WindowPhase::Committed(mut base) | WindowPhase::Composing { mut base, .. } => {
                let a = clamp_char_boundary(&base.text, base.selection.start);
                let b = clamp_char_boundary(&base.text, base.selection.end).max(a);
                if text.is_empty() {
                    return self.clear_or_delete_empty(base, a, b);
                }
                base.text.replace_range(a..b, &text);
                let end = a + text.len();
                base.selection = end..end;
                self.phase = WindowPhase::Committed(base);
                vec![TextInputAction::Ime(commit_text_event(text))]
            }
        }
    }

    fn edit_set_composing_text(
        &mut self,
        text: String,
        new_cursor_position: i32,
    ) -> Vec<TextInputAction> {
        match std::mem::take(&mut self.phase) {
            WindowPhase::Inactive => {
                vec![TextInputAction::Ime(composing_text_event(text, new_cursor_position))]
            }
            WindowPhase::Committed(base) | WindowPhase::Composing { base, .. } => {
                let a = clamp_char_boundary(&base.text, base.selection.start);
                let b = clamp_char_boundary(&base.text, base.selection.end).max(a);
                if text.is_empty() {
                    return self.clear_or_delete_empty(base, a, b);
                }
                let pos = if new_cursor_position <= 0 { 0 } else { text.len() };
                let comp = ActiveComposition { text: text.clone(), cursor: (pos, pos) };
                self.phase = WindowPhase::Composing { base, comp };
                vec![TextInputAction::Ime(composing_text_event(text, new_cursor_position))]
            }
        }
    }

    fn edit_finish_composing_text(&mut self) -> Vec<TextInputAction> {
        if let WindowPhase::Composing { mut base, comp } = std::mem::take(&mut self.phase) {
            let a = clamp_char_boundary(&base.text, base.selection.start);
            let b = clamp_char_boundary(&base.text, base.selection.end).max(a);
            base.text.replace_range(a..b, &comp.text);
            let end = a + comp.text.len();
            base.selection = end..end;
            self.phase = WindowPhase::Committed(base);
        }
        vec![TextInputAction::FinishCompositionFromIme]
    }

    fn edit_set_composing_region(&mut self, start_u16: i32, end_u16: i32) -> Vec<TextInputAction> {
        let Some((was_composing, visible_sel, _)) = self.fold_composing_into_committed() else {
            return Vec::new();
        };
        let mut actions = Vec::new();
        if was_composing {
            actions.push(TextInputAction::FinishCompositionFromIme);
        }

        let WindowPhase::Committed(mut base) = std::mem::take(&mut self.phase) else {
            return actions;
        };

        let (lo, hi) = normalize_utf16_bounds(&base.text, start_u16, end_u16);
        if lo == hi {
            self.phase = WindowPhase::Committed(base);
            return actions;
        }
        let b_start = utf16_to_byte_floor(&base.text, lo);
        let b_end = utf16_to_byte_ceil(&base.text, hi);
        if b_start >= b_end {
            self.phase = WindowPhase::Committed(base);
            return actions;
        }

        let caret_in_region = visible_sel.end.clamp(b_start, b_end) - b_start;
        let region_text = base.text[b_start..b_end].to_owned();
        let field_range = (base.start + b_start)..(base.start + b_end);

        actions.push(TextInputAction::Ime(Ime::Select(field_range)));
        actions.push(TextInputAction::Ime(Ime::Preedit(
            region_text.clone(),
            Some((caret_in_region, caret_in_region)),
            Vec::new(),
        )));

        base.selection = b_start..b_end;
        self.phase = WindowPhase::Composing {
            base,
            comp: ActiveComposition {
                text: region_text,
                cursor: (caret_in_region, caret_in_region),
            },
        };
        actions
    }

    fn edit_set_selection(&mut self, start_u16: i32, end_u16: i32) -> Vec<TextInputAction> {
        let Some((was_composing, _, _)) = self.fold_composing_into_committed() else {
            return Vec::new();
        };
        let mut actions = Vec::new();
        if was_composing {
            actions.push(TextInputAction::FinishCompositionFromIme);
        }

        let WindowPhase::Committed(mut base) = std::mem::take(&mut self.phase) else {
            return actions;
        };

        let (lo, hi) = normalize_utf16_bounds(&base.text, start_u16, end_u16);
        let b_start = utf16_to_byte_floor(&base.text, lo);
        let b_end = if lo == hi { b_start } else { utf16_to_byte_ceil(&base.text, hi) };
        let field_range = (base.start + b_start)..(base.start + b_end);
        actions.push(TextInputAction::Ime(Ime::Select(field_range)));

        base.selection = b_start..b_end;
        self.phase = WindowPhase::Committed(base);
        actions
    }

    fn edit_delete_surrounding(
        &mut self,
        before_raw: i32,
        after_raw: i32,
        in_code_points: bool,
    ) -> Vec<TextInputAction> {
        let before = clamp_delete_count(before_raw);
        let after = clamp_delete_count(after_raw);
        if before == 0 && after == 0 {
            return Vec::new();
        }

        let Some((was_composing, visible_sel, after_finish_caret)) =
            self.fold_composing_into_committed()
        else {
            return vec![TextInputAction::DeleteSurrounding { before, after }];
        };

        let mut actions = Vec::new();
        let WindowPhase::Committed(mut base) = std::mem::take(&mut self.phase) else {
            return vec![TextInputAction::DeleteSurrounding { before, after }];
        };

        if was_composing {
            actions.push(TextInputAction::FinishCompositionFromIme);
            if visible_sel != (after_finish_caret..after_finish_caret) {
                actions.push(TextInputAction::Ime(Ime::Select(
                    (base.start + visible_sel.start)..(base.start + visible_sel.end),
                )));
            }
        }

        let sel_start = clamp_char_boundary(&base.text, base.selection.start);
        let sel_end = clamp_char_boundary(&base.text, base.selection.end).max(sel_start);

        let (del_start, excess_before, del_end, excess_after) = if in_code_points {
            let (ds, consumed_b) = step_back_chars(&base.text, sel_start, before);
            let eb = if before <= consumed_b {
                0
            } else if base.start == 0 {
                usize::from(sel_start == 0)
            } else {
                before - consumed_b
            };
            let (de, consumed_a) = step_forward_chars(&base.text, sel_end, after);
            let ea = after.saturating_sub(consumed_a);
            (ds, eb, de, ea)
        } else {
            let sel_start_u16 = byte_to_utf16(&base.text, sel_start);
            let sel_end_u16 = byte_to_utf16(&base.text, sel_end);
            let total_u16 = utf16_len(&base.text);
            let after_u16 = total_u16.saturating_sub(sel_end_u16);
            let ds = utf16_to_byte_floor(&base.text, sel_start_u16.saturating_sub(before))
                .min(sel_start);
            let eb = if before <= sel_start_u16 {
                0
            } else if base.start == 0 {
                usize::from(sel_start == 0)
            } else {
                before - sel_start_u16
            };
            let de = utf16_to_byte_ceil(&base.text, sel_end_u16.saturating_add(after).min(total_u16))
                .max(sel_end);
            let ea = after.saturating_sub(after_u16);
            (ds, eb, de, ea)
        };

        let backspace = || TextInputAction::KeyDown {
            event: KeyEvent::from(Key::Backspace),
            repeat: false,
        };

        if sel_start == sel_end {
            // Collapsed caret: merge before+after inside the window into one single range delete.
            if del_start < del_end {
                actions.push(TextInputAction::Ime(Ime::Select(
                    (base.start + del_start)..(base.start + del_end),
                )));
                actions.push(backspace());
                base.text.replace_range(del_start..del_end, "");
                base.selection = del_start..del_start;
            }
            if excess_before > 0 {
                actions.push(TextInputAction::DeleteSurrounding {
                    before: excess_before.min(MAX_DELETE_SURROUNDING),
                    after: 0,
                });
            }
            if excess_after > 0 {
                actions.push(TextInputAction::DeleteSurrounding {
                    before: 0,
                    after: excess_after.min(MAX_DELETE_SURROUNDING),
                });
            }
        } else {
            // Non-empty selection: Android excludes the selection itself. Delete after first so
            // `sel_start`/`sel_end` stay valid, then delete before, then restore the selection.
            let sel_len = sel_end - sel_start;
            if sel_end < del_end {
                actions.push(TextInputAction::Ime(Ime::Select(
                    (base.start + sel_end)..(base.start + del_end),
                )));
                actions.push(backspace());
                base.text.replace_range(sel_end..del_end, "");
            }
            if excess_after > 0 {
                if sel_end == del_end {
                    actions.push(TextInputAction::Ime(Ime::Select(
                        (base.start + sel_end)..(base.start + sel_end),
                    )));
                }
                actions.push(TextInputAction::DeleteSurrounding {
                    before: 0,
                    after: excess_after.min(MAX_DELETE_SURROUNDING),
                });
            }
            if del_start < sel_start {
                actions.push(TextInputAction::Ime(Ime::Select(
                    (base.start + del_start)..(base.start + sel_start),
                )));
                actions.push(backspace());
                base.text.replace_range(del_start..sel_start, "");
            }
            if excess_before > 0 {
                if del_start == sel_start {
                    actions.push(TextInputAction::Ime(Ime::Select(
                        (base.start + del_start)..(base.start + del_start),
                    )));
                }
                actions.push(TextInputAction::DeleteSurrounding {
                    before: excess_before.min(MAX_DELETE_SURROUNDING),
                    after: 0,
                });
            }
            if excess_before == 0 {
                let new_start = del_start;
                let new_end = del_start + sel_len;
                actions.push(TextInputAction::Ime(Ime::Select(
                    (base.start + new_start)..(base.start + new_end),
                )));
                base.selection = new_start..new_end;
            }
        }

        self.phase = WindowPhase::Committed(base);
        actions
    }
}

/// Applies one Android `InputConnection` edit through `window` and queues the resulting actions
/// on `rt`. Returns whether any action was accepted and a redraw should be scheduled.
pub(crate) fn apply_ime_edit(
    rt: &mut RuntimeCore,
    window: &mut ImeWindow,
    edit: ImeEdit,
) -> bool {
    let actions = window.apply_edit(edit);
    let mut accepted = false;
    for action in actions {
        accepted |= apply_text_input(rt, action);
    }
    accepted
}
