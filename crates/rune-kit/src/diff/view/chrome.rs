//! Structural rows around diff lines: file headers, folded unchanged-range summaries, and
//! empty/binary file notes.

use std::any::Any;

use rune::{Color, Cursor, El, Icon, col, icon, row, text};

use super::{CODE_PAD, View};
use crate::diff::model::FileStatus;
use crate::diff::state::{DiffState, EXPAND_STEP, Expand, GAP_H, HEADER_H, NOTE_H};

static EXPAND_UP: Icon = Icon {
    name: "kit-diff-expand-up",
    view: 16.0,
    body: r#"<path d="M4.5 9.5L8 6L11.5 9.5" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
static EXPAND_DOWN: Icon = Icon {
    name: "kit-diff-expand-down",
    view: 16.0,
    body: r#"<path d="M4.5 6.5L8 10L11.5 6.5" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
static EXPAND_ALL: Icon = Icon {
    name: "kit-diff-expand-all",
    view: 16.0,
    body: r#"<path d="M5 5.5L8 2.5L11 5.5M5 10.5L8 13.5L11 10.5" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

impl View {
    /// The unchanged lines between two hunks, folded: buttons to show more, and the next
    /// hunk's header.
    pub(super) fn gap(&self, st: &DiffState, f: usize, gap: usize) -> El {
        let l = &self.look;
        let file = &st.files()[f];
        let hidden = st.hidden(f, gap).unwrap_or(0);
        let last = gap == file.hunks.len();
        let expandable = file.expandable();
        let mut buttons = Vec::new();
        if expandable {
            let mut add = |how: Expand, glyph: &'static Icon, label: String| {
                let btn = row()
                    .key(format!("{how:?}"))
                    .size(24.0, 24.0)
                    .center()
                    .radius(6.0)
                    .hover(l.hover)
                    .cursor(Cursor::Pointer)
                    .role("button")
                    .label(label)
                    .child(icon(glyph, 16.0).color(l.muted));
                buttons.push(self.bind.bind_expand(btn, f, gap, how, false));
            };
            let step = hidden.min(EXPAND_STEP);
            if hidden <= EXPAND_STEP {
                add(Expand::All, &EXPAND_ALL, format!("Show {hidden} hidden lines"));
            } else {
                if gap > 0 {
                    add(Expand::Down, &EXPAND_DOWN, format!("Show {step} more lines below"));
                }
                if !last {
                    add(Expand::Up, &EXPAND_UP, format!("Show {step} more lines above"));
                }
            }
        }
        let buttons = row().key("expand").w(self.gutter_w(st, f)).h(GAP_H).center().gap(2.0).children(buttons);
        let mut inner = row().key("inner").h(GAP_H).items_center().gap(12.0).child(buttons);
        if let Some(w) = self.view_w(st, false) {
            inner = inner.max_w(w).sticky_left(0.0);
        }
        if hidden > 0 {
            let count = text(if hidden == 1 { "1 unchanged line".to_owned() } else { format!("{hidden} unchanged lines") }).font(l.small).color(l.muted).key("count");
            let count = if expandable && hidden > EXPAND_STEP {
                let all = row().key("all").px(6.0).h(24.0).items_center().radius(6.0).hover(l.hover).cursor(Cursor::Pointer).role("button").label(format!("Show all {hidden} lines"));
                self.bind.bind_expand(all, f, gap, Expand::All, false).child(count)
            } else {
                row().key("all").px(6.0).child(count)
            };
            inner = inner.child(count);
        }
        if let Some(h) = file.hunks.get(gap) {
            inner = inner.child(text(h.header()).font(l.small).color(l.faint).single_line().key("header"));
        }
        let label = match file.hunks.get(gap) {
            Some(h) => format!("{hidden} unchanged lines, then {}", h.header()),
            None => format!("{hidden} unchanged lines"),
        };
        row().w_full().h(GAP_H).items_center().fill(l.gap_bg).role("group").label(label).child(inner)
    }

    /// A file with no lines to show.
    pub(super) fn note(&self, st: &DiffState, f: usize) -> El {
        let l = &self.look;
        let file = &st.files()[f];
        let msg = if file.binary {
            "Binary file not shown"
        } else if file.status == FileStatus::Renamed || file.status == FileStatus::Copied {
            "File moved without changes"
        } else if file.old_mode != file.new_mode {
            "File mode changed"
        } else if file.status == FileStatus::Added || file.status == FileStatus::Deleted {
            "Empty file"
        } else {
            "No changes"
        };
        let mut inner = row().key("inner").h(NOTE_H).items_center().pl(self.gutter_w(st, f) + CODE_PAD.0).child(text(msg).font(l.small).color(l.muted));
        if let Some(w) = self.view_w(st, false) {
            inner = inner.max_w(w).sticky_left(0.0);
        }
        row().w_full().h(NOTE_H).fill(l.bg).role("note").label(msg).child(inner)
    }

    pub(super) fn file_header(&self, s: &dyn Any, st: &DiffState, f: usize, sticky: bool) -> El {
        let l = &self.look;
        let file = &st.files()[f];
        let collapsed = st.collapsed(f);
        let toggle = row()
            .key("toggle")
            .size(24.0, 24.0)
            .center()
            .radius(6.0)
            .hover(l.hover)
            .cursor(Cursor::Pointer)
            .role("button")
            .label(if collapsed { "Expand file" } else { "Collapse file" }).expanded(!collapsed)
            .child(icon(if collapsed { l.triangle_right } else { l.triangle_down }, 16.0).color(l.muted));
        let toggle = self.bind.bind_toggle_file(toggle, f, sticky);
        let path = |p: &str, c: Color| text(p.to_owned()).font(l.label).color(c).truncate_head().min_w(0.0).shrink();
        let mut name = row().key("path").items_center().gap(6.0).min_w(0.0).shrink();
        let (old, new) = (file.old_path.as_deref(), file.new_path.as_deref());
        name = match (file.status, old, new) {
            (FileStatus::Renamed | FileStatus::Copied, Some(a), Some(b)) => {
                name.child(path(a, l.muted).key("old")).child(text("→").font(l.label).color(l.faint).key("arrow")).child(path(b, l.text).key("new"))
            }
            _ => name.child(path(file.path(), l.text).key("new")),
        };
        let status = match file.status {
            FileStatus::Added => Some(("Added", l.add.count)),
            FileStatus::Deleted => Some(("Deleted", l.del.count)),
            FileStatus::Renamed => Some(("Renamed", l.muted)),
            FileStatus::Copied => Some(("Copied", l.muted)),
            FileStatus::Modified => None,
        };
        let mut inner = row().key("inner").w_full().grow(1.0).px(10.0).gap(8.0).items_center().child(toggle).child(name);
        if let Some((word, color)) = status {
            inner = inner.child(text(word).font(l.small).color(color).key("status"));
        }
        if let (Some(a), Some(b)) = (&file.old_mode, &file.new_mode)
            && a != b
        {
            inner = inner.child(text(format!("{a} → {b}")).font(l.small).color(l.faint).key("mode"));
        }
        inner = inner.child(row().key("space").grow(1.0).a11y_hidden());
        let (adds, dels) = (file.additions(), file.deletions());
        if adds > 0 {
            inner = inner.child(text(format!("+{adds}")).font(l.small).color(l.add.count).key("adds"));
        }
        if dels > 0 {
            inner = inner.child(text(format!("−{dels}")).font(l.small).color(l.del.count).key("dels"));
        }
        if let Some(actions) = self.bind.header_action(s, f) {
            inner = inner.child(actions.key("actions"));
        }
        let mut label = file.path().to_owned();
        if let (FileStatus::Renamed, Some(a)) = (file.status, old) {
            label.push_str(&format!(", renamed from {a}"));
        }
        label.push_str(&format!(", {adds} added, {dels} removed"));
        let line = |key: &'static str, c: Color| col().key(key).w_full().h(1.0).fill(c).a11y_hidden();
        let mut h = col().w_full().h(HEADER_H).fill(l.header_bg).role("heading").label(label);
        // In the list, as wide as the view and held to its left edge while the code scrolls
        // sideways, so the counts and actions stay in sight.
        if let (false, Some(w)) = (sticky, self.view_w(st, false)) {
            h = h.w(w).sticky_left(0.0);
        }
        h = h.child(line("top", if f > 0 && !sticky { l.border } else { l.header_bg }));
        h.child(inner).child(line("bottom", l.border))
    }
}
