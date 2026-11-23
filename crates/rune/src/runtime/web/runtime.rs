//! Browser-only access to the focused edit, kept beside Runtime so the shell sees no nodes.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
use super::*;

/// The last context sent to the DOM. Secure text never enters this cache.
#[derive(Default)]
pub(crate) struct Mirror {
    pub key: String,
    pub text: String,
    /// UTF-8 byte offset in the model; DOM offsets are UTF-16 relative to this window.
    pub base: usize,
    pub anchor: u32,
    pub focus: u32,
    pub secure: bool,
    /// True after the browser mutates the hidden textarea outside a Rust sync (a DOM edit or a
    /// deferred composition sync), so the next diff resyncs even if the model matches the cache.
    stale: bool,
}

/// Read at most 512 UTF-16 units on each side, without scanning the document prefix.
fn context_window(edit: &crate::TextEdit) -> (usize, &str, u32, u32) {
    let (a, b) = edit.range();
    let (raw_anchor, raw_focus) = if edit.anchor <= edit.focus { (a, b) } else { (b, a) };
    let mut start = a;
    let mut units = 0;
    for (i, ch) in edit.text[..a].char_indices().rev() {
        if units + ch.len_utf16() > 512 { break; }
        units += ch.len_utf16();
        start = i;
    }
    let mut end = b;
    let mut units = 0;
    for ch in edit.text[b..].chars() {
        if units + ch.len_utf16() > 512 { break; }
        units += ch.len_utf16();
        end += ch.len_utf8();
    }
    let anchor = edit.text[start..raw_anchor].encode_utf16().count() as u32;
    let focus = edit.text[start..raw_focus].encode_utf16().count() as u32;
    (start, &edit.text[start..end], anchor, focus)
}

/// UTF-16 units a window may grow by at each end to take in the rest of a line it cuts.
const LINE_MORE: usize = 2048;

/// `window` grown to whole lines at either end, where the line it cuts ends within
/// [`LINE_MORE`] more units: a screen reader reads the page's field a line at a time, so the
/// caret's lines are whole however long the draft. A longer line stays cut.
fn whole_lines<'a>(edit: &'a crate::TextEdit, (start, text, anchor, focus): (usize, &'a str, u32, u32)) -> (usize, &'a str, u32, u32) {
    let s = edit.text.as_str();
    let end = start + text.len();
    // Only as far back and on as could be taken in (4 bytes a unit at most).
    let from = s.floor_char_boundary(start.saturating_sub(LINE_MORE * 4));
    let head = match s[from..start].rfind('\n') {
        Some(p) => from + p + 1,
        None if from == 0 => 0,
        None => start,
    };
    let to = s.ceil_char_boundary((end + LINE_MORE * 4).min(s.len()));
    let tail = match s[end..to].find('\n') {
        Some(p) => end + p,
        None if to == s.len() => to,
        None => end,
    };
    let units = |a: usize, b: usize| s[a..b].encode_utf16().count();
    let head = if units(head, start) <= LINE_MORE { head } else { start };
    let tail = if units(end, tail) <= LINE_MORE { tail } else { end };
    let shift = units(head, start) as u32;
    (head, &s[head..tail], anchor + shift, focus + shift)
}

impl Mirror {
    pub(crate) fn invalidate(&mut self) { self.stale = true; }

    fn update(&mut self, key: &str, edit: Option<&crate::TextEdit>) -> bool {
        let secure = edit.is_some_and(|e| e.is_secure());
        let (base, text, anchor, focus) = edit.filter(|_| !secure).map(|e| whole_lines(e, context_window(e))).unwrap_or((0, "", 0, 0));
        if !self.stale && self.key == key && self.base == base && self.text == text && self.anchor == anchor
            && self.focus == focus && self.secure == secure { return false; }
        self.stale = false;
        self.key.clear(); self.key.push_str(key);
        self.text.clear(); self.text.push_str(text);
        self.base = base; self.anchor = anchor; self.focus = focus; self.secure = secure;
        true
    }
}

/// Reject offsets in the middle of a surrogate pair, rather than corrupting the Rust string.
fn byte_at(text: &str, utf16: usize) -> Option<usize> {
    let mut units = 0;
    for (byte, ch) in text.char_indices() {
        if units == utf16 { return Some(byte); }
        units += ch.len_utf16();
    }
    (units == utf16).then_some(text.len())
}

impl RuntimeCore {
    /// An action whose handler may ask for what a browser allows only inside the input's event
    /// (the clipboard, a new tab, a menu, a dialog) or that routes keys and focus: it runs in a
    /// frame inside that event. The rest (a move's hover, enter and leave, what a frame reports
    /// for the next: layout, visibility, rows shown, a selection's change) waits for the display.
    fn web_needs_event(action: &Action) -> bool {
        matches!(action, Action::Press(..) | Action::DoublePress(_) | Action::MiddlePress(_) | Action::Link(..) | Action::LinkSecondary(..)
            | Action::SecondaryPress(..) | Action::Drag(..) | Action::DragRepeat(..) | Action::DragStart(_) | Action::DragEnd(..) | Action::Dismiss(_)
            | Action::FieldMenu(..) | Action::Files(_) | Action::Paste(..) | Action::ClipboardRead(..) | Action::FileDialog(..) | Action::Key { .. }
            | Action::KeyUp(_) | Action::Physical(..) | Action::Focus(..) | Action::FocusControl(_) | Action::Blur)
    }

    /// Whether a release (or an edit) left a handler that has to run inside the input's event.
    pub(crate) fn web_release_needs_frame(&self) -> bool {
        self.frame.queue.iter().any(Self::web_needs_event)
    }

    /// A press whose only such action focuses a text field runs that action now, as a frame
    /// would (its double click and its composition included), so the page's own field takes
    /// focus inside the press and the next key goes to it; the rest of the queue and the frame
    /// wait for the display's. False, with nothing run, when any other such action is queued or
    /// a focus is already asked for.
    pub(crate) fn web_focus_field(&mut self, app: &mut dyn AppDriver) -> bool {
        if self.commit.relayout || self.kbd.focus_request.is_some() || self.frame.pending.focus_to.is_some() { return false; }
        let needs: Vec<usize> = self.frame.queue.iter().enumerate().filter(|(_, a)| Self::web_needs_event(a)).map(|(i, _)| i).collect();
        let [pos] = needs[..] else { return false };
        if !matches!(self.frame.queue.iter().nth(pos), Some(Action::Focus(..))) { return false; }
        let mut tx = self.transaction(app);
        let action = self.frame.queue.remove(pos);
        let changed = self.run_action(app, action, &mut tx);
        self.commit.dirty |= changed;
        merge(&mut self.frame.pending, tx);
        true
    }

    /// The focused field's context from the model, which an edit or a focus just changed, for
    /// the page's field before its next key; None when what's pending needs a whole frame now
    /// (a handler that asked for the clipboard, a link, focus or a selection).
    pub(crate) fn web_edit_context(&mut self, app: &mut dyn AppDriver, mirror: &mut Mirror) -> Option<bool> {
        let tx = &self.frame.pending;
        if self.web_release_needs_frame() || self.kbd.focus_request.is_some() || tx.blur || tx.focus_to.is_some() || tx.select.is_some() || !tx.keys.is_empty()
            || tx.copy.is_some() || tx.paste || !tx.clipboard_reads.is_empty() || !tx.pick_reads.is_empty()
            || !tx.open_urls.is_empty() || !tx.file_dialogs.is_empty() || !tx.popups.is_empty() || tx.clear_secret {
            return None;
        }
        let access = self.focused_input()?.access.clone()?;
        let key = self.kbd.focus.path()?;
        Some(mirror.update(key, Some(access(app.states()))))
    }
    pub(crate) fn web_context(&self, mirror: &mut Mirror) -> bool {
        match self.focused_input().filter(|i| i.access.is_some()) {
            Some(input) => mirror.update(self.kbd.focus.path().unwrap_or_default(), Some(&input.edit)),
            None => mirror.update("", None),
        }
    }

    pub(crate) fn web_key(&mut self, app: &mut dyn AppDriver, event: &serde_json::Value) -> bool {
        let flag = |key: &str| event[key].as_bool().unwrap_or(false);
        let mods = crate::input::key_table::web_mods(flag("shift"), flag("ctrl"), flag("alt"), flag("meta"));
        let name = event["key"].as_str().unwrap_or_default();
        let code = event["code"].as_str().filter(|c| !c.is_empty() && !c.contains('+'))
            .and_then(|c| c.parse::<crate::Shortcut>().ok())
            .map(|s| s.physical_key());
        let key = match crate::input::key_table::named_key_from_str(name) {
            Some(key) => key,
            None if name == " " => crate::Key::Text(" ".into()),
            None if mods.cmd => {
                let mut chars = name.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => {
                        let c = if !c.is_ascii() { code.and_then(crate::shortcuts::us_char).unwrap_or(c) } else { c };
                        crate::Key::Command(c.to_ascii_lowercase())
                    }
                    _ => return false,
                }
            }
            None => return false,
        };
        let event_key = crate::KeyEvent { key, mods };
        let activate = event_key.key == crate::Key::Tab;
        let repeat = flag("repeat");
        if let Some(key) = code {
            self.physical_with_fallback(app, crate::PhysicalInput::Key { key, mods, pressed: true, repeat }, Some(event_key));
        } else if repeat {
            self.repeat_key(event_key);
        } else {
            self.key(event_key);
        }
        activate
    }

    pub(crate) fn web_secure(&mut self, key: &str, text: String) {
        let event = crate::KeyEvent { key: crate::Key::Text(text), mods: crate::Mods::default() };
        if self.kbd.focus.path() == Some(key) && self.has_secure_focus() {
            self.key(event);
        }
    }

    /// Replaces the focused secure field's whole text as one edit (browser autofill or password
    /// manager fill). Keeps the incoming value in a `Zeroizing` buffer and writes nothing to
    /// event logs, traces or DOM mirrors.
    pub(crate) fn web_secure_replace(&mut self, app: &mut dyn AppDriver, key: &str, text: String) {
        let text = zeroize::Zeroizing::new(text);
        if self.kbd.focus.path() != Some(key) || !self.has_secure_focus() { return; }
        let mut tx = self.transaction(app);
        let changed = self.with_focused(app, |edit| {
            let len = edit.text.len();
            edit.set_selection(0, len);
            edit.paste(&text)
        }, &mut tx);
        if changed { self.commit.dirty = true; }
        merge(&mut self.frame.pending, tx);
    }

    pub(crate) fn web_edit(&mut self, app: &mut dyn AppDriver, event: &serde_json::Value, mirror: &mut Mirror) {
        // The browser just mutated its hidden textarea, so the cached mirror no longer matches
        // the DOM even if the edit is rejected or `on_input_change` reverts or clears the text.
        if event["kind"] == "edit" { mirror.invalidate(); }
        let Some(key) = event["key"].as_str() else { return };
        if self.kbd.focus.path() != Some(key) || self.has_secure_focus() { return; }
        let number = |key: &str| event[key].as_u64().and_then(|n| usize::try_from(n).ok());
        let Some(base) = number("base") else { return };
        let Some(expected) = event["expected"].as_str() else { return };
        let mut tx = self.transaction(app);
        let composing_before = self.composing(app);
        let changed = self.with_focused(app, |edit| {
            // Browser edits belong to the snapshot from which its context was mirrored.
            if !edit.text.get(base..).is_some_and(|s| s.starts_with(expected)) { return EditOutcome::default(); }
            if event["kind"] == "edit" || event["kind"] == "preedit" {
                let Some((start, end)) = number("start").zip(number("end")) else { return EditOutcome::default() };
                let Some((a, b)) = byte_at(expected, start).zip(byte_at(expected, end)) else { return EditOutcome::default() };
                if a > b { return EditOutcome::default(); }
                edit.ime(&Ime::Preedit(String::new(), None, Vec::new()));
                let raw = (base + a, base + b);
                edit.anchor = raw.0; edit.focus = raw.1;
                edit.snap_to_pills();
                let snapped = (edit.anchor, edit.focus) != raw;
                let text = event["text"].as_str().unwrap_or_default();
                if event["kind"] == "preedit" {
                    edit.ime(&Ime::Preedit(text.to_owned(), Some((text.len(), text.len())), Vec::new()));
                    return EditOutcome::default();
                }
                edit.insert(text);
                if snapped { return EditOutcome::default(); }
            }
            if let Some((anchor, focus)) = number("anchor").zip(number("focus"))
                && let Some((a, f)) = byte_at(&edit.text[base..], anchor).zip(byte_at(&edit.text[base..], focus)) {
                edit.anchor = base + a; edit.focus = base + f;
                edit.snap_to_pills();
            }
            EditOutcome::default()
        }, &mut tx);
        if composing_before || event["kind"] == "preedit" { self.kbd.ime_took_key(); }
        if changed { self.commit.dirty = true; }
        merge(&mut self.frame.pending, tx);
    }
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
