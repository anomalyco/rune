//! Keyboard caret browsing in selectable text (`El::caret_browsing`): the selection's focus is a
//! caret the arrow keys, Home and End move, and Shift extends the selection with, as in a
//! browser's caret browsing. Inside one text the keys move as they do in a read-only text field
//! (the same `TextEdit` steps over the same layout); at a text's edge the caret steps into the
//! next or previous text of its selection group.
use super::*;
use crate::Key;
use virtual_selection::EndRef;

impl RuntimeCore {
    /// Node `i` takes a keyboard caret: opted-in selectable text, laid out and drawn whole.
    fn browses(&self, i: usize) -> bool {
        let n = &self.tree.nodes[i];
        self.is_selectable(i) && n.elided.is_none() && n.text_layout.is_some() && n.text.as_ref().is_some_and(|t| t.cold.caret_browsing)
    }

    /// The mounted opted-in text holding the selection's focus, and the focus there: where the
    /// keyboard caret is.
    pub(in crate::runtime) fn browse_focus(&self) -> Option<(usize, usize)> {
        let h = self.selection_holder_end()?;
        let EndRef::Mounted(f) = self.focus_holder_end(h)? else { return None };
        let sel = self.edit.selectable.phase.selection()?;
        self.browses(f).then(|| (f, floor_boundary(self.content_of(f), sel.focus)))
    }

    /// The caret paint draws, as an empty range `(text, at, at)`: an empty selection in
    /// opted-in text while the window has focus.
    pub(in crate::runtime) fn browse_caret(&self) -> Option<(usize, usize, usize)> {
        let sel = self.edit.selectable.phase.selection()?;
        if sel.far.is_some() || sel.anchor != sel.focus || self.host.window_focused == Some(false) { return None; }
        self.browse_focus().map(|(f, at)| (f, at, at))
    }

    /// The keyboard caret's line `[x, y, w, h]` in window points at its laid-out place, when it's
    /// in node `i`.
    pub(in crate::runtime) fn browse_caret_rect(&self, i: usize) -> Option<[f32; 4]> {
        let (f, at) = self.browse_focus().filter(|c| c.0 == i)?;
        self.caret_line(f, at)
    }

    /// The caret line at byte `at` of text `i`, in window points.
    fn caret_line(&self, i: usize, at: usize) -> Option<[f32; 4]> {
        let n = &self.tree.nodes[i];
        let (spec, laid) = (n.text.as_ref()?, n.text_layout.as_ref()?);
        let [x, y] = self.text_origin(&self.tree.nodes, i, spec, laid);
        let s = self.env.scale;
        let bb = laid.view().caret(at, parley::Affinity::Downstream, 1.5 * s);
        let h = if bb.y1 > bb.y0 { (bb.y1 - bb.y0) as f32 / s } else { spec.style.line_px() };
        Some([x + bb.x0 as f32 / s, y + bb.y0 as f32 / s, 1.5, h])
    }

    /// The texts the caret in text `f` moves through: its selection group's, or itself.
    fn browse_texts(&self, f: usize) -> Vec<usize> {
        self.group_of(f).map_or_else(|| vec![f], |g| self.group_texts(g))
    }

    /// The byte of text `j` at window x `x` on its last (`last`) or first line.
    fn browse_column(&self, j: usize, x: f32, last: bool) -> usize {
        let n = &self.tree.nodes[j];
        let (Some(spec), Some(laid)) = (n.text.as_ref(), n.text_layout.as_ref()) else { return 0 };
        let [ox, _] = self.text_origin(&self.tree.nodes, j, spec, laid);
        let flow = laid.view();
        let y = if last { (flow.height() - 0.5).max(0.0) } else { 0.5 };
        flow.hit((x - ox) * self.env.scale, y).index.min(spec.content.len())
    }

    /// An arrow key, Home or End with selectable text's keyboard caret placed and no text field
    /// focused: moves the caret, or with Shift the selection's focus. Inside a focused element
    /// only when the text is inside it too. True when it took the key.
    pub(in crate::runtime) fn browse_key(&mut self, ev: &KeyEvent) -> bool {
        if !matches!(ev.key, Key::Left | Key::Right | Key::Up | Key::Down | Key::Home | Key::End) { return false; }
        if self.focused_input().is_some() || self.edit.selectable.phase.dragging() { return false; }
        let Some((f, at)) = self.browse_focus() else { return false };
        if let Some(p) = self.kbd.focus.path() {
            let path = self.tree.nodes[f].path.as_str();
            if !(path == p || path.strip_prefix(p).is_some_and(|r| r.starts_with('/'))) { return false; }
        }
        let Some(sel) = self.edit.selectable.phase.selection() else { return false };
        let (anchor, focus, single) = (sel.anchor, sel.focus, sel.far.is_none());
        let m = ev.mods;
        let back = matches!(ev.key, Key::Left | Key::Up | Key::Home);
        let plain = !m.alt && !m.cmd && !m.ctrl && !m.logo;
        let doc = (matches!(ev.key, Key::Home | Key::End) && m.cmd)
            || (cfg!(target_os = "macos") && m.cmd && matches!(ev.key, Key::Up | Key::Down));
        let ranges = self.selection_ranges();
        let (j, to) = if doc {
            let texts = self.browse_texts(f);
            let (Some(&first), Some(&last)) = (texts.first(), texts.last()) else { return false };
            if back { (first, 0) } else { (last, self.content_of(last).len()) }
        } else if let (false, true, Some(&(j0, a, _)), Some(&(j1, _, b))) =
            (m.shift || single, plain && matches!(ev.key, Key::Left | Key::Right), ranges.first(), ranges.last())
        {
            // Left and Right collapse a selection across texts to its start or end.
            if back { (j0, a) } else { (j1, b) }
        } else {
            // In one text the field's own steps; the far end's text starts from the caret.
            let mut e = crate::TextEdit::new(self.content_of(f)).with_line_home_end(true);
            (e.anchor, e.focus) = if single { (anchor, focus) } else { (at, at) };
            let (a0, f0) = (e.anchor, e.focus);
            let flow = self.tree.nodes[f].text_layout.as_ref().map(|l| l.view());
            // Up on the first line and Down on the last step into the next text at the caret's
            // column, as a browser's caret does between paragraphs, not to the text's ends.
            let edge = flow.is_some_and(|l| match ev.key {
                Key::Up => l.line_edge(at, false) == 0,
                Key::Down => l.move_line(at, true, None).is_none(),
                _ => false,
            });
            if !edge { let _ = e.key_in_read_only(ev, flow); }
            let moved = !edge && (e.focus != f0 || (!m.shift && e.anchor != a0));
            match (moved, &ev.key) {
                (true, _) => (f, e.focus),
                (false, Key::Left | Key::Right | Key::Up | Key::Down) => {
                    // At the text's edge: into the previous or next text of its group.
                    let texts = self.browse_texts(f);
                    let k = texts.iter().position(|&t| t == f);
                    let next = k.and_then(|k| if back { k.checked_sub(1) } else { Some(k + 1) }).and_then(|k| texts.get(k).copied());
                    match (next, &ev.key) {
                        (None, _) => (f, e.focus),
                        (Some(j), Key::Up | Key::Down) => {
                            let x = self.caret_line(f, at).map_or(0.0, |r| r[0]);
                            (j, self.browse_column(j, x, back))
                        }
                        (Some(j), _) if back => (j, self.content_of(j).len()),
                        (Some(j), _) => (j, 0),
                    }
                }
                (false, _) => (f, e.focus),
            }
        };
        let path = self.tree.nodes[j].path.clone();
        let content = self.content_of(j).to_owned();
        if m.shift {
            let holder = self.selection_holder_end() == Some(EndRef::Mounted(j));
            self.select_step(phase::Event::Caret { focus: to, far: (!holder).then(|| phase::Far::new(path, content)) });
        } else {
            self.select_step(phase::Event::Select(phase::Selection::new(path, content, (to, to), (to, to))));
        }
        log_event!(self, "caret key {:?}", ev.key);
        self.edit.caret_follow = true;
        true
    }
}

#[cfg(test)]
#[path = "browse_keys_tests.rs"]
mod tests;
