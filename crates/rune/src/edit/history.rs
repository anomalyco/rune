//! Undo and redo for a [`TextEdit`]: each step the text it changed and the selection, marks and
//! blocks either side ([`Change`]), typing runs coalesced as Chromium's editor does ([`Run`],
//! [`step`]), undo groups, and a history bounded in steps and bytes that always keeps its newest
//! step, however large (a paste of a whole file can always be undone).

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

/// What a document holds besides its text, either side of a [`Change`].
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Meta {
    anchor: usize,
    focus: usize,
    marks: Vec<MarkRange>,
    blocks: Vec<Block>,
    soft_breaks: Vec<u32>,
}

impl Meta {
    fn of(s: &Snapshot) -> Meta {
        Meta { anchor: s.anchor, focus: s.focus, marks: s.marks.clone(), blocks: s.blocks.clone(), soft_breaks: s.soft_breaks.clone() }
    }

    fn bytes(&self) -> usize {
        self.marks.len() * std::mem::size_of::<MarkRange>() + self.blocks.len() * std::mem::size_of::<Block>() + self.soft_breaks.len() * std::mem::size_of::<u32>()
    }
}

/// One undo step: text bytes `at..at + removed.len()` of the document before it became
/// `inserted`, and what the document held besides its text before and after. Its texts are
/// shared: a field cloned for a view and edited after copies none of them.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Change {
    at: usize,
    removed: Arc<Zeroizing<String>>,
    inserted: Arc<Zeroizing<String>>,
    /// The text's length before and after, to tell a document the app replaced from this one.
    lens: [usize; 2],
    before: Meta,
    after: Meta,
}

impl Change {
    fn bytes(&self) -> usize {
        self.removed.len() + self.inserted.len() + self.before.bytes() + self.after.bytes()
    }

    /// The step from `before` to `after`.
    fn between(before: &Snapshot, after: &Snapshot) -> Change {
        Change::to_text(before, &after.text, Meta::of(after))
    }

    /// The step from `before` to text `b` with `after` besides.
    fn to_text(before: &Snapshot, b: &str, after: Meta) -> Change {
        let a = before.text.as_str();
        let (at, removed, inserted) = diff(a, b);
        Change {
            at,
            removed: shared(&a[at..at + removed]),
            inserted: shared(&b[at..at + inserted]),
            lens: [a.len(), b.len()],
            before: Meta::of(before),
            after,
        }
    }

    /// Whether `text` is this step's document after it (`after`) or before it: its length, and
    /// the bytes the step wrote there.
    fn leads_to(&self, text: &str, after: bool) -> bool {
        let (len, there) = if after { (self.lens[1], &self.inserted) } else { (self.lens[0], &self.removed) };
        text.len() == len && text.get(self.at..self.at + there.len()) == Some(there.as_str())
    }

    /// `next`, from this step's document after, folded into this step: one step from this one's
    /// before to `next`'s after.
    fn join(&mut self, mid: &str, next: Change) {
        let (s, e) = (self.at.min(next.at), (self.at + self.inserted.len()).max(next.at + next.removed.len()));
        let mut removed = String::with_capacity(e - s);
        removed.push_str(&mid[s..self.at]);
        removed.push_str(&self.removed);
        removed.push_str(&mid[self.at + self.inserted.len()..e.max(self.at + self.inserted.len())]);
        let mut inserted = String::with_capacity(e - s);
        inserted.push_str(&mid[s..next.at]);
        inserted.push_str(&next.inserted);
        inserted.push_str(&mid[(next.at + next.removed.len()).min(e)..e]);
        (self.at, self.removed, self.inserted) = (s, Arc::new(Zeroizing::new(removed)), Arc::new(Zeroizing::new(inserted)));
        (self.lens[1], self.after) = (next.lens[1], next.after);
    }
}

fn shared(s: &str) -> Arc<Zeroizing<String>> {
    Arc::new(Zeroizing::new(s.to_owned()))
}

/// Where `a` and `b` differ: the byte both share up to (a character boundary), and how many
/// bytes of each follow before what they share at the end.
fn diff(a: &str, b: &str) -> (usize, usize, usize) {
    let (x, y) = (a.as_bytes(), b.as_bytes());
    let n = x.len().min(y.len());
    // In blocks first: a long document's comparison is a few memcmps.
    let mut pre = 0;
    while pre + 4096 <= n && x[pre..pre + 4096] == y[pre..pre + 4096] { pre += 4096; }
    while pre < n && x[pre] == y[pre] { pre += 1; }
    while !a.is_char_boundary(pre) { pre -= 1; }
    let room = n - pre;
    let mut suf = 0;
    while suf + 4096 <= room && x[x.len() - suf - 4096..x.len() - suf] == y[y.len() - suf - 4096..y.len() - suf] { suf += 4096; }
    while suf < room && x[x.len() - 1 - suf] == y[y.len() - 1 - suf] { suf += 1; }
    while !a.is_char_boundary(x.len() - suf) || !b.is_char_boundary(y.len() - suf) { suf -= 1; }
    (pre, x.len() - pre - suf, y.len() - pre - suf)
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

    /// Text bytes `at..at + len` become `with`, and the rest of the document `meta`.
    fn restore(&mut self, (at, len, with): (usize, usize, &str), meta: &Meta) {
        self.text.replace_range(at..at + len, with);
        self.anchor = meta.anchor;
        self.focus = meta.focus;
        self.marks = meta.marks.clone();
        self.blocks = meta.blocks.clone();
        self.soft_breaks = meta.soft_breaks.clone();
        self.typing = Typing::Inherited;
        self.cancel_composition();
        self.sync_invariants();
    }

    pub(super) fn record_change(&mut self, before: Option<Snapshot>, kind: EditKind) {
        let Some(before) = before.filter(|_| self.recording()) else {
            return;
        };
        // What the edit left, read in place: copying a long document to compare it costs more
        // than the edit.
        let after = Meta { anchor: self.anchor, focus: self.focus, marks: self.marks.clone(), blocks: self.blocks.clone(), soft_breaks: self.soft_breaks.clone() };
        if before.text.as_str() == self.text && before.marks == after.marks && before.blocks == after.blocks && before.soft_breaks == after.soft_breaks {
            return;
        }
        let change = Change::to_text(&before, &self.text, after);
        let h = Arc::make_mut(&mut self.history);
        // Public model text may have been replaced by the app: a history that doesn't lead to
        // the document this edit started from is dropped.
        let leads = match (h.redo.last(), h.undo.back()) {
            (Some(c), _) => c.leads_to(&before.text, false),
            (None, Some(c)) => c.leads_to(&before.text, true),
            (None, None) => true,
        };
        if !leads {
            *h = History::default();
        }
        h.bytes -= h.redo.iter().map(Change::bytes).sum::<usize>();
        h.redo.clear();
        // A rich contenteditable creates its first text node here; a plain field just types.
        let record = step(&mut h.run, kind, self.rich && before.text.is_empty());
        if record == Record::Join
            && before.anchor == before.focus
            && h.undo.back().is_some_and(|c| c.after == change.before && c.leads_to(&before.text, true))
        {
            let last = h.undo.back_mut().unwrap();
            h.bytes -= last.bytes();
            last.join(&before.text, change);
            h.bytes += last.bytes();
        } else {
            h.bytes += change.bytes();
            h.undo.push_back(change);
        }
        // The newest step stays, however large: it's the way back from what was just done.
        while h.undo.len() > 1 && (h.undo.len() > HISTORY_STEPS || h.bytes > HISTORY_BYTES) {
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
        if !change.leads_to(&self.text, true) { *h = History::default(); return; }
        self.restore((change.at, change.inserted.len(), &change.removed), &change.before);
        Arc::make_mut(&mut self.history).redo.push(change);
    }

    pub fn redo(&mut self) {
        if !self.recording() { return; }
        self.break_history_group();
        if self.preedit().is_some() { self.cancel_composition(); return; }
        let h = Arc::make_mut(&mut self.history);
        let Some(change) = h.redo.pop() else { return };
        if !change.leads_to(&self.text, false) { *h = History::default(); return; }
        self.restore((change.at, change.removed.len(), &change.inserted), &change.after);
        Arc::make_mut(&mut self.history).undo.push_back(change);
    }
}

/// What a stored history looks like (`TextEdit::history_json`): version 2, each step a change;
/// version 1 stored the document before and after each step, and still reads.
const HISTORY_FORMAT: u64 = 2;

impl TextEdit {
    /// How many steps undo and redo hold. A change says the history moved, for an app that
    /// stores it ([`TextEdit::history_json`]) only when it has.
    pub fn history_len(&self) -> (usize, usize) {
        (self.history.undo.len(), self.history.redo.len())
    }

    /// The undo and redo history as JSON, for an app to keep past a restart and give back with
    /// [`TextEdit::restore_history_json`]: the document's text as it is, and the newest steps,
    /// at most `max_steps` of them and about `max_bytes` of changes between them (each step the
    /// text it replaced and the selection, marks and blocks either side). The newest undo step
    /// is always there, however large. `None` for a secure field, which records nothing, and for
    /// an empty history.
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
        // The newest undo step first: it's the way back from what was done last.
        let newest = self.history.undo.back().map(|c| { fits(c); change_json(c) });
        let redo: Vec<_> = self.history.redo.iter().rev().take_while(|c| fits(c)).map(change_json).collect();
        let older = self.history.undo.len().saturating_sub(1);
        let mut undo: Vec<_> = self.history.undo.iter().take(older).rev().take_while(|c| fits(c)).map(change_json).collect();
        undo.reverse();
        undo.extend(newest);
        let redo: Vec<_> = redo.into_iter().rev().collect();
        Some(serde_json::json!({ "v": HISTORY_FORMAT, "d": self.text.as_str(), "undo": undo, "redo": redo }))
    }

    /// Takes back a history [`TextEdit::history_json`] wrote, when it leads to this document.
    /// A history stored a few keystrokes before the document was (an app stores it less often
    /// than the text) gets one more step, from the document it was stored with to this one, so
    /// the first undo goes back to where the stored history stood. Anything that doesn't fit (a
    /// step whose text or offsets aren't in its document) is left out and the field keeps no
    /// history. Reads version 1 too. Returns whether it was taken.
    pub fn restore_history_json(&mut self, v: &serde_json::Value) -> bool {
        if self.secure || self.group.is_open() {
            return false;
        }
        let Some((doc, mut undo, mut redo)) = (match v["v"].as_u64() {
            Some(2) => read_v2(v),
            Some(1) => read_v1(v),
            _ => None,
        }) else { return false };
        if !chain_holds(&doc, &undo, &redo) {
            return false;
        }
        self.sync_invariants();
        let now = self.snapshot_now();
        let same = |m: &Meta| *doc == now.text.as_str() && m.marks == now.marks && m.blocks == now.blocks && m.soft_breaks == now.soft_breaks;
        let leads_here = undo.last().map(|c| &c.after).or_else(|| redo.last().map(|c| &c.before)).is_some_and(same);
        if !leads_here {
            let Some(last) = undo.last().map(|c| c.after.clone()) else { return false };
            if *doc == now.text.as_str() {
                return false;
            }
            redo.clear();
            let stored = Snapshot { text: Arc::new(Zeroizing::new(doc.to_string())), anchor: last.anchor, focus: last.focus, marks: last.marks, blocks: last.blocks, soft_breaks: last.soft_breaks };
            undo.push(Change::between(&stored, &now));
        }
        let mut h = History { undo: undo.into(), redo, bytes: 0, run: Run::Closed };
        h.bytes = h.undo.iter().chain(h.redo.iter()).map(Change::bytes).sum();
        while h.undo.len() > 1 && (h.undo.len() > HISTORY_STEPS || h.bytes > HISTORY_BYTES) {
            let Some(c) = h.undo.pop_front() else { break };
            h.bytes -= c.bytes();
        }
        self.history = Arc::new(h);
        true
    }
}

/// Version 2: the document as written, and each step's change.
fn read_v2(v: &serde_json::Value) -> Option<(Zeroizing<String>, Vec<Change>, Vec<Change>)> {
    let doc = Zeroizing::new(v["d"].as_str()?.to_owned());
    let read = |key: &str| -> Option<Vec<Change>> { v[key].as_array()?.iter().map(change_from).collect() };
    Some((doc, read("undo")?, read("redo")?))
}

/// Version 1: each step the document before and after it; the document as written is the
/// newest undo step's after (or the next redo step's before).
fn read_v1(v: &serde_json::Value) -> Option<(Zeroizing<String>, Vec<Change>, Vec<Change>)> {
    let read = |key: &str| -> Option<Vec<(Snapshot, Snapshot)>> {
        v[key].as_array()?.iter().map(|c| Some((snapshot_from(&c["b"])?, snapshot_from(&c["a"])?))).collect()
    };
    let (undo, redo) = (read("undo")?, read("redo")?);
    let doc = undo.last().map(|c| &c.1).or_else(|| redo.last().map(|c| &c.0))?.text.as_str().to_owned();
    let change = |(b, a): &(Snapshot, Snapshot)| Change::between(b, a);
    Some((Zeroizing::new(doc), undo.iter().map(change).collect(), redo.iter().map(change).collect()))
}

/// Whether `undo` leads back from `doc` step by step, and `redo` forward, every offset on a
/// character boundary of the document it's in.
fn chain_holds(doc: &str, undo: &[Change], redo: &[Change]) -> bool {
    let mut text = doc.to_owned();
    for c in undo.iter().rev() {
        if !c.leads_to(&text, true) || !meta_holds(&c.after, &text) { return false; }
        text.replace_range(c.at..c.at + c.inserted.len(), &c.removed);
        if !meta_holds(&c.before, &text) { return false; }
    }
    text.clear();
    text.push_str(doc);
    for c in redo.iter().rev() {
        if !c.leads_to(&text, false) || !meta_holds(&c.before, &text) { return false; }
        text.replace_range(c.at..c.at + c.removed.len(), &c.inserted);
        if !meta_holds(&c.after, &text) { return false; }
    }
    text.zeroize();
    true
}

/// Whether `m`'s offsets are on character boundaries of `text`, its blocks one a line (or
/// none) and its soft breaks inside it.
fn meta_holds(m: &Meta, text: &str) -> bool {
    let at = |n: usize| text.is_char_boundary(n);
    let lines = text.split('\n').count();
    at(m.anchor) && at(m.focus)
        && m.marks.iter().all(|r| at(r.start) && at(r.end) && r.start < r.end)
        && (m.blocks.is_empty() || m.blocks.len() == lines)
        && m.soft_breaks.iter().all(|&n| n > 0 && (n as usize) < lines)
}

fn change_json(c: &Change) -> serde_json::Value {
    serde_json::json!({ "at": c.at, "r": c.removed.as_str(), "i": c.inserted.as_str(), "bl": c.lens[0], "al": c.lens[1], "b": meta_json(&c.before), "a": meta_json(&c.after) })
}

fn change_from(v: &serde_json::Value) -> Option<Change> {
    let n = |k: &str| v[k].as_u64().map(|n| n as usize);
    Some(Change {
        at: n("at")?,
        removed: shared(v["r"].as_str()?),
        inserted: shared(v["i"].as_str()?),
        lens: [n("bl")?, n("al")?],
        before: meta_from(&v["b"])?,
        after: meta_from(&v["a"])?,
    })
}

fn meta_json(m: &Meta) -> serde_json::Value {
    let marks: Vec<_> = m
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
    let blocks: Vec<_> = m
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
    serde_json::json!({ "a": m.anchor, "f": m.focus, "m": marks, "k": blocks, "s": m.soft_breaks })
}

/// A stored document's selection, marks, blocks and soft breaks, as numbers: `meta_holds`
/// checks them against the text they're for.
fn meta_from(v: &serde_json::Value) -> Option<Meta> {
    let n = |n: &serde_json::Value| n.as_u64().map(|n| n as usize);
    let marks = v["m"]
        .as_array()?
        .iter()
        .map(|m| {
            let (start, end) = (n(&m[0])?, n(&m[1])?);
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
            Some(MarkRange { start, end, mark })
        })
        .collect::<Option<Vec<_>>>()?;
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
        .collect::<Option<Vec<_>>>()?;
    let soft_breaks = v["s"].as_array()?.iter().map(|n| n.as_u64().map(|n| n as u32)).collect::<Option<Vec<_>>>()?;
    Some(Meta { anchor: n(&v["a"])?, focus: n(&v["f"])?, marks, blocks, soft_breaks })
}

/// A version-1 snapshot, when every offset in it is on a character boundary of its text.
fn snapshot_from(v: &serde_json::Value) -> Option<Snapshot> {
    let text = v["t"].as_str()?.to_owned();
    let m = meta_from(v)?;
    if !meta_holds(&m, &text) { return None; }
    Some(Snapshot { text: Arc::new(Zeroizing::new(text)), anchor: m.anchor, focus: m.focus, marks: m.marks, blocks: m.blocks, soft_breaks: m.soft_breaks })
}#[cfg(test)]
#[path = "history_tests.rs"]
mod tests;
