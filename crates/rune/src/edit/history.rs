//! Undo and redo for a [`TextEdit`]: document snapshots, typing runs coalesced as Chromium's
//! editor does ([`Run`], [`step`]), undo groups and a history bounded in steps and bytes.

use std::collections::VecDeque;

use super::*;

pub(super) const HISTORY_STEPS: usize = 128;
pub(super) const HISTORY_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Snapshot {
    text: Arc<Zeroizing<String>>,
    anchor: usize,
    focus: usize,
    marks: Vec<MarkRange>,
    blocks: Vec<Block>,
    soft_breaks: Vec<u32>,
}

impl Snapshot {
    fn same_doc(&self, other: &Self) -> bool {
        self.text == other.text
            && self.marks == other.marks
            && self.blocks == other.blocks
            && self.soft_breaks == other.soft_breaks
    }
}

/// Whether a [`TextEdit::undo_group`] is open, and what its one step undoes to. Never cloned
/// or compared, so cloning or comparing a [`TextEdit`] mid-group stays clean.
#[derive(Default)]
pub(super) enum Group {
    #[default]
    Closed,
    /// Open on a field that records: the document as the group began.
    Open(Snapshot),
    /// Open on a secure field, which records nothing.
    Unrecorded,
}

impl Group {
    pub(super) fn is_open(&self) -> bool {
        !matches!(self, Group::Closed)
    }
}

impl Clone for Group {
    fn clone(&self) -> Self {
        Group::Closed
    }
}

impl PartialEq for Group {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

/// Non-content-derived edit state. Never retain a fingerprint of a secret.
#[derive(Clone, Copy, Default)]
pub(super) struct EditedTag(pub(super) bool);

impl PartialEq for EditedTag {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Change {
    before: Snapshot,
    after: Snapshot,
}

impl Change {
    fn bytes(&self) -> usize {
        self.before.text.len()
            + self.after.text.len()
            + (self.before.marks.len() + self.after.marks.len()) * std::mem::size_of::<MarkRange>()
            + (self.before.blocks.len() + self.after.blocks.len()) * std::mem::size_of::<Block>()
            + (self.before.soft_breaks.len() + self.after.soft_breaks.len()) * std::mem::size_of::<u32>()
    }
}

// Rendering clones TextEdit. Share immutable history so a view never copies the undo stack.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct History {
    pub(super) undo: VecDeque<Change>,
    pub(super) redo: Vec<Change>,
    pub(super) bytes: usize,
    pub(super) run: Run,
}

/// The undo step the last edit leaves open for the next to join, as a browser's editor keeps
/// its typing command open. A caret move, a pointer selection, a focus change, undo, redo and
/// any edit but typing close it. Pauses and word boundaries don't: Chromium's textarea and
/// contenteditable undo a whole typing run in one step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Run {
    #[default]
    Closed,
    /// Typed text or line breaks: more join; a deletion starts a new step.
    Inserting,
    /// Backspace or Delete: more deletions and typed text join.
    Deleting,
}

/// What an edit is, for joining it to the open undo step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EditKind {
    /// Typed text without a line break.
    Text,
    /// A typed line break (Return, Shift+Return).
    LineBreak,
    /// Backspace or Delete over a character or a selection.
    Delete,
    /// A word or line deleted at once (Ctrl+Backspace, Ctrl+K): always a new step, which
    /// further deletions and typing join.
    DeleteRun,
    /// Paste, an IME commit, a pill, formatting: a step of its own.
    Other,
}

/// Where an edit's change goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Record {
    /// Into the open step.
    Join,
    /// Into a new step.
    Push,
}

/// Steps the open run with an edit of `kind`; `into_empty` when typed text lands in a field
/// that held nothing. The first character typed into an empty field is a step of its own, as
/// in OpenCode's composer (a contenteditable whose first input closes Chromium's typing step),
/// so undoing a typed message leaves that character, then empties the field.
pub(super) fn step(run: &mut Run, kind: EditKind, into_empty: bool) -> Record {
    let (record, next) = match (*run, kind) {
        (_, EditKind::Other) => (Record::Push, Run::Closed),
        (_, EditKind::DeleteRun) | (Run::Closed | Run::Inserting, EditKind::Delete) => (Record::Push, Run::Deleting),
        (Run::Deleting, EditKind::Delete) => (Record::Join, Run::Deleting),
        (Run::Closed, EditKind::Text | EditKind::LineBreak) => (Record::Push, Run::Inserting),
        (Run::Inserting | Run::Deleting, EditKind::Text | EditKind::LineBreak) => (Record::Join, Run::Inserting),
    };
    *run = if kind == EditKind::Text && into_empty { Run::Closed } else { next };
    record
}

impl TextEdit {
    /// Drop the undo and redo stacks, keeping text, marks, blocks, soft breaks and the selection
    /// (after loading a sent message back into a composer to edit it, say).
    pub fn clear_history(&mut self) {
        self.history = Arc::default();
        if self.group.is_open() {
            self.sync_invariants();
            self.group = self.group_from_here();
        }
    }

    /// An undo group opened on the document as it is now.
    pub(super) fn group_from_here(&self) -> Group {
        if self.secure { Group::Unrecorded } else { Group::Open(self.snapshot_now()) }
    }

    /// Edits record undo steps: not on a secure field, and not inside an undo group, whose
    /// edits become one step when it closes.
    fn recording(&self) -> bool {
        !self.secure && !self.group.is_open()
    }

    /// End a typing group on a pointer selection or focus change.
    pub fn break_history_group(&mut self) {
        if self.history.run != Run::Closed { Arc::make_mut(&mut self.history).run = Run::Closed; }
    }

    pub(super) fn snapshot_now(&self) -> Snapshot {
        Snapshot {
            text: Arc::new(Zeroizing::new(self.text.clone())),
            anchor: self.anchor,
            focus: self.focus,
            marks: self.marks.clone(),
            blocks: self.blocks.clone(),
            soft_breaks: self.soft_breaks.clone(),
        }
    }

    pub(super) fn snapshot(&self) -> Option<Snapshot> {
        self.recording().then(|| self.snapshot_now())
    }

    fn restore(&mut self, s: &Snapshot) {
        self.text.zeroize();
        self.text.push_str(&s.text);
        self.anchor = s.anchor;
        self.focus = s.focus;
        self.marks = s.marks.clone();
        self.blocks = s.blocks.clone();
        self.soft_breaks = s.soft_breaks.clone();
        self.typing = Typing::Inherited;
        self.cancel_composition();
        self.sync_invariants();
    }

    pub(super) fn record_change(&mut self, before: Option<Snapshot>, kind: EditKind) {
        let Some(before) = before.filter(|_| self.recording()) else {
            return;
        };
        let after = self.snapshot_now();
        if before.same_doc(&after) {
            return;
        }
        let h = Arc::make_mut(&mut self.history);
        let expected = h.redo.last().map(|c| &c.before).or_else(|| h.undo.back().map(|c| &c.after));
        if expected.is_some_and(|last| last.text != before.text) {
            *h = History::default();
        }
        h.bytes -= h.redo.iter().map(Change::bytes).sum::<usize>();
        h.redo.clear();
        // A rich contenteditable creates its first text node here; a plain field just types.
        let record = step(&mut h.run, kind, self.rich && before.text.is_empty());
        if record == Record::Join
            && before.anchor == before.focus
            && h.undo.back().is_some_and(|c| c.after == before)
        {
            let last = h.undo.back_mut().unwrap();
            h.bytes -= last.bytes();
            last.after = after;
            h.bytes += last.bytes();
        } else {
            let change = Change { before, after };
            h.bytes += change.bytes();
            h.undo.push_back(change);
        }
        while h.undo.len() > HISTORY_STEPS || h.bytes > HISTORY_BYTES {
            h.bytes -= h.undo.pop_front().unwrap().bytes();
        }
    }

    /// Run `f`, whose edits become one undo step: one undo reverts all of them and one redo
    /// restores them, with the selection from before the first and after the last. Groups
    /// nest and flatten into the outermost. A group that changes nothing records no step, and
    /// `undo` and `redo` do nothing inside one. Typing after the group starts a new step.
    ///
    /// Picking a mention is one step, trailing space included:
    ///
    /// ```
    /// # use rune::TextEdit;
    /// let mut e = TextEdit::default();
    /// e.insert("hi @ag");
    /// e.undo_group(|e| {
    ///     e.replace_with_pill(3..6, "agent:1", "@agent");
    ///     e.insert(" ");
    /// });
    /// assert_eq!(e.text, "hi @agent ");
    /// e.undo();
    /// assert_eq!(e.text, "hi @ag");
    /// ```
    pub fn undo_group<R>(&mut self, f: impl FnOnce(&mut TextEdit) -> R) -> R {
        if self.group.is_open() {
            return f(self);
        }
        self.sync_invariants();
        self.break_history_group();
        self.group = self.group_from_here();
        struct Reset<'a>(&'a mut TextEdit);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.group = Group::Closed;
            }
        }
        let g = Reset(self);
        let r = f(g.0);
        let before = match std::mem::take(&mut g.0.group) {
            Group::Open(baseline) => Some(baseline),
            Group::Closed | Group::Unrecorded => None,
        };
        drop(g);
        self.sync_invariants();
        self.break_history_group();
        self.record_change(before, EditKind::Other);
        r
    }

    /// Undo one typing group or discrete edit. Active IME composition is cancelled first.
    pub fn undo(&mut self) {
        if !self.recording() { return; }
        self.break_history_group();
        if self.preedit().is_some() { self.cancel_composition(); return; }
        let h = Arc::make_mut(&mut self.history);
        let Some(change) = h.undo.pop_back() else { return };
        // Public model text may have been replaced by the app. Never restore stale history.
        if self.text != change.after.text.as_str() { *h = History::default(); return; }
        self.restore(&change.before);
        Arc::make_mut(&mut self.history).redo.push(change);
    }

    pub fn redo(&mut self) {
        if !self.recording() { return; }
        self.break_history_group();
        if self.preedit().is_some() { self.cancel_composition(); return; }
        let h = Arc::make_mut(&mut self.history);
        let Some(change) = h.redo.pop() else { return };
        if self.text != change.before.text.as_str() { *h = History::default(); return; }
        self.restore(&change.after);
        Arc::make_mut(&mut self.history).undo.push_back(change);
    }
}

/// What a stored history looks like (`TextEdit::history_json`): version 1.
const HISTORY_FORMAT: u64 = 1;

impl TextEdit {
    /// How many steps undo and redo hold. A change says the history moved, for an app that
    /// stores it ([`TextEdit::history_json`]) only when it has.
    pub fn history_len(&self) -> (usize, usize) {
        (self.history.undo.len(), self.history.redo.len())
    }

    /// The undo and redo history as JSON, for an app to keep past a restart and give back with
    /// [`TextEdit::restore_history_json`]: the newest steps, at most `max_steps` of them and
    /// about `max_bytes` of document between them (each step is the document before and after
    /// it: text, selection, marks, blocks). `None` for a secure field, which records nothing,
    /// and for an empty history.
    pub fn history_json(&self, max_steps: usize, max_bytes: usize) -> Option<serde_json::Value> {
        if self.secure || (self.history.undo.is_empty() && self.history.redo.is_empty()) {
            return None;
        }
        let (mut steps, mut bytes) = (0, 0);
        let mut fits = |c: &Change| {
            steps += 1;
            bytes += c.bytes();
            steps <= max_steps && bytes <= max_bytes
        };
        // Redo first: the steps nearest the document as it is.
        let redo: Vec<_> = self.history.redo.iter().rev().take_while(|c| fits(c)).map(change_json).collect();
        let mut undo: Vec<_> = self.history.undo.iter().rev().take_while(|c| fits(c)).map(change_json).collect();
        undo.reverse();
        let redo: Vec<_> = redo.into_iter().rev().collect();
        Some(serde_json::json!({ "v": HISTORY_FORMAT, "undo": undo, "redo": redo }))
    }

    /// Takes back a history [`TextEdit::history_json`] wrote, when it leads to this document:
    /// its newest undo step ends in this text (or its newest redo step starts from it). A history
    /// stored a few keystrokes before the document was (an app stores it less often than the
    /// text) gets one more step, from where it ends to this document, so the first undo goes
    /// back to where the stored history stood. Anything that doesn't fit (another document, a
    /// step whose offsets aren't in its text) is left out and the field keeps no history.
    /// Returns whether it was taken.
    pub fn restore_history_json(&mut self, v: &serde_json::Value) -> bool {
        if self.secure || self.group.is_open() || v["v"].as_u64() != Some(HISTORY_FORMAT) {
            return false;
        }
        let read = |key: &str| -> Option<Vec<Change>> { v[key].as_array()?.iter().map(change_from).collect() };
        let (Some(mut undo), Some(mut redo)) = (read("undo"), read("redo")) else { return false };
        self.sync_invariants();
        let now = self.snapshot_now();
        let leads_here = undo.last().map(|c| &c.after).or_else(|| redo.last().map(|c| &c.before)).is_some_and(|s| s.same_doc(&now));
        if !leads_here {
            let Some(last) = undo.last().map(|c| c.after.clone()) else { return false };
            if last.text == now.text {
                return false;
            }
            redo.clear();
            undo.push(Change { before: last, after: now });
        }
        let mut h = History { undo: undo.into(), redo, bytes: 0, run: Run::Closed };
        h.bytes = h.undo.iter().chain(h.redo.iter()).map(Change::bytes).sum();
        while h.undo.len() > HISTORY_STEPS || h.bytes > HISTORY_BYTES {
            let Some(c) = h.undo.pop_front() else { break };
            h.bytes -= c.bytes();
        }
        self.history = Arc::new(h);
        true
    }
}

fn change_json(c: &Change) -> serde_json::Value {
    serde_json::json!({ "b": snapshot_json(&c.before), "a": snapshot_json(&c.after) })
}

fn change_from(v: &serde_json::Value) -> Option<Change> {
    Some(Change { before: snapshot_from(&v["b"])?, after: snapshot_from(&v["a"])? })
}

fn snapshot_json(s: &Snapshot) -> serde_json::Value {
    let marks: Vec<_> = s
        .marks
        .iter()
        .map(|m| {
            let kind = match &m.mark {
                Mark::Bold => serde_json::json!("b"),
                Mark::Italic => serde_json::json!("i"),
                Mark::Underline => serde_json::json!("u"),
                Mark::Strike => serde_json::json!("s"),
                Mark::Code => serde_json::json!("c"),
                Mark::Link(url) => serde_json::json!(["l", url]),
                Mark::Pill(id) => serde_json::json!(["p", id]),
            };
            serde_json::json!([m.start, m.end, kind])
        })
        .collect();
    let blocks: Vec<_> = s
        .blocks
        .iter()
        .map(|b| match b {
            Block::Paragraph => serde_json::json!("p"),
            Block::Quote => serde_json::json!("q"),
            Block::Code => serde_json::json!("c"),
            Block::Bullet(l) => serde_json::json!(["b", l]),
            Block::Numbered(l) => serde_json::json!(["n", l]),
        })
        .collect();
    serde_json::json!({ "t": s.text.as_str(), "a": s.anchor, "f": s.focus, "m": marks, "k": blocks, "s": s.soft_breaks })
}

/// A stored snapshot, when every offset in it is on a character boundary of its text.
fn snapshot_from(v: &serde_json::Value) -> Option<Snapshot> {
    let text = v["t"].as_str()?.to_owned();
    let at = |n: &serde_json::Value| n.as_u64().map(|n| n as usize).filter(|&n| text.is_char_boundary(n));
    let (anchor, focus) = (at(&v["a"])?, at(&v["f"])?);
    let marks = v["m"]
        .as_array()?
        .iter()
        .map(|m| {
            let (start, end) = (at(&m[0])?, at(&m[1])?);
            let mark = match (&m[2], m[2][0].as_str(), m[2][1].as_str()) {
                (k, _, _) if k == "b" => Mark::Bold,
                (k, _, _) if k == "i" => Mark::Italic,
                (k, _, _) if k == "u" => Mark::Underline,
                (k, _, _) if k == "s" => Mark::Strike,
                (k, _, _) if k == "c" => Mark::Code,
                (_, Some("l"), Some(url)) => Mark::Link(url.to_owned()),
                (_, Some("p"), Some(id)) => Mark::Pill(id.to_owned()),
                _ => return None,
            };
            (start < end).then_some(MarkRange { start, end, mark })
        })
        .collect::<Option<Vec<_>>>()?;
    let lines = text.split('\n').count();
    let blocks = v["k"]
        .as_array()?
        .iter()
        .map(|b| match (b.as_str(), b[0].as_str(), b[1].as_u64()) {
            (Some("p"), ..) => Some(Block::Paragraph),
            (Some("q"), ..) => Some(Block::Quote),
            (Some("c"), ..) => Some(Block::Code),
            (_, Some("b"), Some(l)) => Some(Block::Bullet((l as u8).min(MAX_LIST_LEVEL))),
            (_, Some("n"), Some(l)) => Some(Block::Numbered((l as u8).min(MAX_LIST_LEVEL))),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
        .filter(|b| b.is_empty() || b.len() == lines)?;
    let soft_breaks = v["s"].as_array()?.iter().map(|n| n.as_u64().map(|n| n as u32).filter(|&n| n > 0 && (n as usize) < lines)).collect::<Option<Vec<_>>>()?;
    Some(Snapshot { text: Arc::new(Zeroizing::new(text)), anchor, focus, marks, blocks, soft_breaks })
}
#[cfg(test)]
#[path = "history_tests.rs"]
mod tests;
