use std::cell::{Ref, RefCell};
use std::cmp::Ordering;
use std::collections::HashMap;

use rune::{Key, KeyEvent, Tx};

use crate::list_nav::{self, NavAction, NavPolicy, TypeAhead, allows_list_mods, page_size};

use super::{TreeKey, TreeRow};

struct NodeMeta {
    id: String,
    parent: Option<usize>,
    children: Vec<usize>,
    depth: u32,
    open: bool,
    selected: bool,
}

/// What a filter left: hidden, shown to reach a match, or a match.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shown {
    Hidden,
    Path,
    Match,
}

struct Filter {
    shown: Vec<Shown>,
    /// Folders the user closed while filtering (the rest show open).
    closed: Vec<bool>,
}

#[derive(Default)]
struct Visible {
    /// Node index of each visible row, top to bottom.
    rows: Vec<usize>,
    /// Row of each node, `usize::MAX` when hidden.
    row_of: Vec<usize>,
}

/// The topology, open folders, filter, selection and keyboard cursor of a [`TreeModel`](super::TreeModel),
/// independent of the data stored at each node.
pub(super) struct TreeMeta {
    nodes: Vec<NodeMeta>,
    index: HashMap<String, usize>,
    roots: Vec<usize>,
    pub(super) cursor: Option<usize>,
    anchor: Option<usize>,
    multi: bool,
    filter: Option<Filter>,
    visible: RefCell<Option<Visible>>,
    pub(super) type_ahead: TypeAhead,
    pub(super) key: String,
    /// The viewport's height in points, for Page Up and Page Down.
    viewport: f32,
    row_height: f32,
    /// Its rows indent from the right (`Self::set_right_to_left`).
    rtl: bool,
}

impl TreeMeta {
    pub(super) fn new() -> TreeMeta {
        TreeMeta {
            nodes: Vec::new(),
            index: HashMap::new(),
            roots: Vec::new(),
            cursor: None,
            anchor: None,
            multi: false,
            filter: None,
            visible: RefCell::new(None),
            type_ahead: TypeAhead::Idle,
            key: "tree".into(),
            viewport: 0.0,
            row_height: 24.0,
            rtl: false,
        }
    }

    pub(super) fn set_key(&mut self, key: String) {
        self.key = key;
    }

    pub(super) fn set_multi(&mut self, on: bool) {
        self.multi = on;
    }

    pub(super) fn set_right_to_left(&mut self, rtl: bool) {
        self.rtl = rtl;
    }

    pub(super) fn update_viewport(&mut self, height: f32, row_height: f32, tx: &mut Tx) {
        if self.viewport == height && self.row_height == row_height {
            tx.live_only();
        }
        self.viewport = height;
        self.row_height = row_height;
    }

    fn dirty(&mut self) {
        *self.visible.get_mut() = None;
    }

    pub(super) fn insert(&mut self, parent: Option<&str>, id: String) -> bool {
        if self.index.contains_key(&id) {
            return false;
        }
        let parent = match parent {
            Some(p) => match self.index.get(p) {
                Some(&p) => Some(p),
                None => return false,
            },
            None => None,
        };
        let i = self.nodes.len();
        let depth = parent.map_or(0, |p| self.nodes[p].depth + 1);
        match parent {
            Some(p) => self.nodes[p].children.push(i),
            None => self.roots.push(i),
        }
        self.index.insert(id.clone(), i);
        self.nodes.push(NodeMeta { id, parent, children: Vec::new(), depth, open: false, selected: false });
        if let Some(f) = &mut self.filter {
            f.shown.push(Shown::Hidden);
            f.closed.push(false);
        }
        self.dirty();
        true
    }

    pub(super) fn clear(&mut self) {
        *self = TreeMeta {
            key: std::mem::take(&mut self.key),
            multi: self.multi,
            viewport: self.viewport,
            row_height: self.row_height,
            ..TreeMeta::new()
        };
    }

    pub(super) fn len(&self) -> usize {
        self.nodes.len()
    }
    pub(super) fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
    pub(super) fn contains(&self, id: &str) -> bool {
        self.index.contains_key(id)
    }
    pub(super) fn index_of(&self, id: &str) -> Option<usize> {
        self.index.get(id).copied()
    }
    pub(super) fn parent(&self, id: &str) -> Option<&str> {
        let p = self.nodes[*self.index.get(id)?].parent?;
        Some(&self.nodes[p].id)
    }
    pub(super) fn children(&self, id: Option<&str>) -> impl Iterator<Item = &str> {
        let list = match id {
            Some(id) => self.index.get(id).map_or(&[][..], |&i| &self.nodes[i].children[..]),
            None => &self.roots[..],
        };
        list.iter().map(|&c| self.nodes[c].id.as_str())
    }
    pub(super) fn has_children(&self, id: &str) -> bool {
        self.index.get(id).is_some_and(|&i| !self.nodes[i].children.is_empty())
    }
    pub(super) fn depth(&self, id: &str) -> Option<u32> {
        self.index.get(id).map(|&i| self.nodes[i].depth)
    }
    pub(super) fn subtree_indices(&self, id: &str) -> impl Iterator<Item = usize> + '_ {
        let mut stack: Vec<usize> = self.index.get(id).copied().into_iter().collect();
        std::iter::from_fn(move || {
            let i = stack.pop()?;
            stack.extend(self.nodes[i].children.iter().rev());
            Some(i)
        })
    }

    pub(super) fn sort_by(&mut self, cmp: &mut dyn FnMut(usize, usize) -> Ordering) {
        for n in &mut self.nodes {
            n.children.sort_by(|&a, &b| cmp(a, b));
        }
        self.roots.sort_by(|&a, &b| cmp(a, b));
        self.dirty();
    }

    // Open folders ----------------------------------------------------------------------------

    pub(super) fn is_expanded(&self, id: &str) -> bool {
        self.index.get(id).is_some_and(|&i| self.open(i))
    }

    fn open(&self, i: usize) -> bool {
        match &self.filter {
            Some(f) => !f.closed[i],
            None => self.nodes[i].open,
        }
    }

    fn set_open(&mut self, i: usize, open: bool) {
        match &mut self.filter {
            Some(f) => f.closed[i] = !open,
            None => self.nodes[i].open = open,
        }
        // A cursor or anchor inside a closing folder moves up to it.
        if !open {
            if let Some(c) = self.cursor && self.is_under(c, i) {
                if !self.multi && self.nodes[c].selected {
                    self.select_only(i);
                } else {
                    self.cursor = Some(i);
                }
            }
            if let Some(a) = self.anchor && self.is_under(a, i) {
                self.anchor = Some(i);
            }
        }
        self.dirty();
    }

    fn is_under(&self, mut node: usize, ancestor: usize) -> bool {
        while let Some(p) = self.nodes[node].parent {
            if p == ancestor {
                return true;
            }
            node = p;
        }
        false
    }

    pub(super) fn set_expanded(&mut self, id: &str, open: bool) {
        if let Some(&i) = self.index.get(id) {
            self.set_open(i, open);
        }
    }
    pub(super) fn toggle(&mut self, id: &str) {
        if let Some(&i) = self.index.get(id) {
            self.set_open(i, !self.open(i));
        }
    }
    pub(super) fn expand_all(&mut self) {
        for i in 0..self.nodes.len() {
            if !self.nodes[i].children.is_empty() {
                self.set_open(i, true);
            }
        }
    }
    pub(super) fn collapse_all(&mut self) {
        for i in 0..self.nodes.len() {
            self.set_open(i, false);
        }
    }
    pub(super) fn expand_to(&mut self, id: &str) {
        let Some(&i) = self.index.get(id) else { return };
        if let Some(f) = &mut self.filter {
            if f.shown[i] == Shown::Hidden {
                f.shown[i] = Shown::Match;
            }
            let mut p = self.nodes[i].parent;
            while let Some(j) = p {
                if f.shown[j] == Shown::Hidden {
                    f.shown[j] = Shown::Path;
                }
                p = self.nodes[j].parent;
            }
            self.dirty();
        }
        let mut p = self.nodes[i].parent;
        while let Some(j) = p {
            self.nodes[j].open = true;
            self.set_open(j, true);
            p = self.nodes[j].parent;
        }
    }

    // Selection ------------------------------------------------------------------------------

    pub(super) fn selected(&self) -> impl Iterator<Item = &str> {
        let mut out = Vec::new();
        let mut stack: Vec<usize> = self.roots.iter().rev().copied().collect();
        while let Some(i) = stack.pop() {
            if self.nodes[i].selected {
                out.push(self.nodes[i].id.as_str());
            }
            stack.extend(self.nodes[i].children.iter().rev());
        }
        out.into_iter()
    }
    pub(super) fn is_selected(&self, id: &str) -> bool {
        self.index.get(id).is_some_and(|&i| self.nodes[i].selected)
    }
    pub(super) fn cursor(&self) -> Option<&str> {
        self.cursor.map(|c| self.nodes[c].id.as_str())
    }
    pub(super) fn clear_selection(&mut self) {
        self.nodes.iter_mut().for_each(|n| n.selected = false);
    }
    pub(super) fn select(&mut self, id: &str) {
        if let Some(&i) = self.index.get(id) {
            self.select_only(i);
        }
    }
    fn select_only(&mut self, i: usize) {
        self.clear_selection();
        self.nodes[i].selected = true;
        self.cursor = Some(i);
        self.anchor = Some(i);
    }
    pub(super) fn set_selected(&mut self, id: &str, on: bool) {
        let Some(&i) = self.index.get(id) else { return };
        if !self.multi {
            self.clear_selection();
        }
        self.nodes[i].selected = on;
        self.cursor = Some(i);
        self.anchor = Some(i);
    }
    pub(super) fn select_range(&mut self, id: &str) {
        let Some(&i) = self.index.get(id) else { return };
        if !self.multi {
            return self.select_only(i);
        }
        let anchor = self.anchor.unwrap_or(i);
        let (a, b) = {
            let v = self.rows();
            (v.row_of[anchor], v.row_of[i])
        };
        if a == usize::MAX || b == usize::MAX {
            return self.select_only(i);
        }
        let range: Vec<usize> = self.rows().rows[a.min(b)..=a.max(b)].to_vec();
        self.clear_selection();
        for j in range {
            self.nodes[j].selected = true;
        }
        self.cursor = Some(i);
    }

    pub(super) fn click(&mut self, id: &str, mods: rune::Mods) -> bool {
        let Some(&i) = self.index.get(id) else { return false };
        if self.multi && mods.shift {
            self.select_range(id);
            return false;
        }
        if self.multi && mods.cmd {
            self.nodes[i].selected = !self.nodes[i].selected;
            self.cursor = Some(i);
            self.anchor = Some(i);
            return false;
        }
        self.select_only(i);
        true
    }

    pub(super) fn click_row(&mut self, id: &str, openable: bool, mods: rune::Mods) {
        let plain = self.click(id, mods);
        if openable && plain {
            self.toggle(id);
        }
    }

    pub(super) fn reveal(&mut self, id: &str, tx: &mut Tx) -> bool {
        if !self.contains(id) {
            return false;
        }
        self.expand_to(id);
        self.select(id);
        tx.scroll_into_view_in(self.key.clone(), id);
        true
    }

    // Filter ---------------------------------------------------------------------------------

    pub(super) fn filter(&mut self, keep: &dyn Fn(usize) -> bool) {
        let n = self.nodes.len();
        let mut shown = vec![Shown::Hidden; n];
        for i in 0..n {
            if keep(i) {
                shown[i] = Shown::Match;
                let mut p = self.nodes[i].parent;
                while let Some(j) = p {
                    if shown[j] != Shown::Hidden {
                        break;
                    }
                    shown[j] = Shown::Path;
                    p = self.nodes[j].parent;
                }
            }
        }
        self.filter = Some(Filter { shown, closed: vec![false; n] });
        self.dirty();
        let (hide_cursor, hide_anchor) = {
            let vis = self.rows();
            (
                self.cursor.is_some_and(|c| vis.row_of[c] == usize::MAX),
                self.anchor.is_some_and(|a| vis.row_of[a] == usize::MAX),
            )
        };
        if hide_cursor {
            self.cursor = None;
        }
        if hide_anchor {
            self.anchor = self.cursor;
        }
    }
    pub(super) fn clear_filter(&mut self) {
        if self.filter.take().is_some() {
            self.dirty();
        }
    }
    pub(super) fn is_filtering(&self) -> bool {
        self.filter.is_some()
    }
    pub(super) fn is_match(&self, id: &str) -> bool {
        let Some(&i) = self.index.get(id) else { return false };
        self.filter.as_ref().is_none_or(|f| f.shown[i] == Shown::Match)
    }

    // Visible rows ---------------------------------------------------------------------------

    fn rows(&self) -> Ref<'_, Visible> {
        if self.visible.borrow().is_none() {
            let mut v = Visible { rows: Vec::new(), row_of: vec![usize::MAX; self.nodes.len()] };
            let mut stack: Vec<usize> = self.roots.iter().rev().copied().collect();
            while let Some(i) = stack.pop() {
                if !self.shown(i) {
                    continue;
                }
                v.row_of[i] = v.rows.len();
                v.rows.push(i);
                if self.open(i) {
                    stack.extend(self.nodes[i].children.iter().rev());
                }
            }
            *self.visible.borrow_mut() = Some(v);
        }
        Ref::map(self.visible.borrow(), |v| v.as_ref().unwrap())
    }

    fn shown(&self, i: usize) -> bool {
        self.filter.as_ref().is_none_or(|f| f.shown[i] != Shown::Hidden)
    }

    /// Whether `i` has children the filter shows.
    fn openable(&self, i: usize) -> bool {
        self.nodes[i].children.iter().any(|&c| self.shown(c))
    }

    pub(super) fn visible_len(&self) -> usize {
        self.rows().rows.len()
    }
    pub(super) fn visible_id(&self, row: usize) -> Option<&str> {
        let i = *self.rows().rows.get(row)?;
        Some(&self.nodes[i].id)
    }
    pub(super) fn visible_row(&self, id: &str) -> Option<usize> {
        let i = *self.index.get(id)?;
        Some(self.rows().row_of[i]).filter(|&r| r != usize::MAX)
    }

    pub(super) fn row(&self, row: usize) -> Option<(usize, TreeRow<'_>)> {
        let i = *self.rows().rows.get(row)?;
        let n = &self.nodes[i];
        let state = TreeRow {
            id: &n.id,
            depth: n.depth,
            expanded: self.openable(i).then(|| self.open(i)),
            selected: n.selected,
            cursor: self.cursor == Some(i),
            matched: self.filter.as_ref().is_some_and(|f| f.shown[i] == Shown::Match),
            multi: self.multi,
        };
        Some((i, state))
    }

    // Keyboard -------------------------------------------------------------------------------

    fn move_to(&mut self, i: usize, extend: bool) {
        if extend && self.multi {
            let id = self.nodes[i].id.clone();
            self.select_range(&id);
        } else {
            self.select_only(i);
        }
    }

    pub(super) fn handle_key<'a>(&mut self, ev: &KeyEvent, label: &dyn Fn(usize) -> &'a str, tx: &mut Tx) -> TreeKey {
        let m = ev.mods;
        if !allows_list_mods(m) {
            return TreeKey::Ignored;
        }
        let len = self.visible_len();
        if len == 0 {
            return TreeKey::Ignored;
        }
        let at = self.cursor.map(|c| self.rows().row_of[c]).filter(|&r| r != usize::MAX);
        let page = page_size(self.viewport, self.row_height);
        if let Some(action) = NavPolicy::list(self.multi).space_activates().decode(ev, page) {
            return match action {
                NavAction::SelectAll => {
                    let rows = self.rows().rows.clone();
                    for i in rows {
                        self.nodes[i].selected = true;
                    }
                    TreeKey::Handled
                }
                NavAction::Activate => self.activate(at, tx),
                NavAction::Escape => TreeKey::Ignored,
                NavAction::Move { step, extend } => self.go(step.target(at, len, false).unwrap(), extend, tx),
            };
        }
        if m.cmd {
            return TreeKey::Ignored;
        }
        let key = match (&ev.key, self.rtl) {
            (Key::Left, true) => Key::Right,
            (Key::Right, true) => Key::Left,
            (k, _) => k.clone(),
        };
        let target = match &key {
            Key::Right | Key::Left if m.shift => None,
            Key::Right => {
                let Some(r) = at else { return self.go(0, false, tx) };
                let i = self.rows().rows[r];
                if !self.openable(i) {
                    return TreeKey::Handled;
                }
                if !self.open(i) {
                    self.set_open(i, true);
                    return TreeKey::Handled;
                }
                Some((r + 1).min(len - 1))
            }
            Key::Left => {
                let Some(r) = at else { return self.go(0, false, tx) };
                let i = self.rows().rows[r];
                if self.openable(i) && self.open(i) {
                    self.set_open(i, false);
                    return TreeKey::Handled;
                }
                match self.nodes[i].parent {
                    Some(p) => Some(self.rows().row_of[p]),
                    None => return TreeKey::Handled,
                }
            }
            Key::Text(t) if !t.trim().is_empty() => {
                let prefix = self.type_ahead.step(t, tx.clock());
                return match self.type_select(&prefix, label) {
                    Some(r) => self.go(r, false, tx),
                    None => TreeKey::Handled,
                };
            }
            _ => None,
        };
        match target {
            Some(r) => self.go(r, m.shift, tx),
            None => TreeKey::Ignored,
        }
    }

    fn activate(&mut self, at: Option<usize>, tx: &Tx) -> TreeKey {
        let Some(r) = at else { return TreeKey::Ignored };
        if tx.key_repeat() {
            return TreeKey::Handled;
        }
        let i = self.rows().rows[r];
        if self.openable(i) {
            self.set_open(i, !self.open(i));
            return TreeKey::Handled;
        }
        TreeKey::Open(self.nodes[i].id.clone())
    }

    fn go(&mut self, row: usize, extend: bool, tx: &mut Tx) -> TreeKey {
        let i = self.rows().rows[row];
        self.move_to(i, extend);
        tx.scroll_into_view_in(self.key.clone(), self.nodes[i].id.clone());
        TreeKey::Handled
    }

    pub(super) fn type_select<'a>(&self, prefix: &str, label: &dyn Fn(usize) -> &'a str) -> Option<usize> {
        let v = self.rows();
        let at = self.cursor.map(|c| v.row_of[c]).filter(|&r| r != usize::MAX);
        list_nav::type_select(prefix, at, v.rows.len(), |r| label(v.rows[r]))
    }
}
