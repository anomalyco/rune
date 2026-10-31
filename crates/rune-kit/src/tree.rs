//! Tree: a virtualised outline over a [`TreeModel`] the app keeps in its state. A file tree is
//! the main use ([`FileTree`] builds one from paths), but rows hold any data.
//!
//! The model owns the nodes, their open state, the selection and the keyboard cursor; the
//! [`Tree`] builder draws the rows that are on screen ([`El::virtual_rows`]), so 50,000 nodes
//! cost what the visible forty do. The tree is one focus stop that keeps its keyboard while rows
//! scroll in and out: Up and Down move, Right opens or goes to the first child, Left closes or
//! goes to the parent, Home and End go to the ends, Page Up and Page Down move by a screen,
//! Return opens, and typing a name jumps to it. With [`TreeModel::multi_select`], Shift extends
//! the selection and Cmd-A selects every row.
//!
//! ```ignore
//! struct App { files: TreeModel<FileNode<Change>> }
//! let files = FileTree::new().build(changes.iter().map(|c| (c.path.as_str(), c.clone())));
//! // In the view:
//! Tree::new(|s: &App| &s.files, |s: &mut App| &mut s.files, FileNode::label)
//!     .row(|n, r| TreeItem::file(n, r).changes(n.data.as_ref().map_or((0, 0), |c| (c.added, c.removed))))
//!     .on_select(|s: &mut App, id, _| s.diff.show(id))
//!     .build(s, &kit)
//!     .h(480.0)
//! // Following another view: open the folders above `path`, select it and scroll to it.
//! s.files.reveal(path, tx);
//! ```

use std::borrow::Cow;
use std::cell::{Ref, RefCell};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;
use std::rc::Rc;

use rune::{Color, El, GrowthSpec, Icon, Key, KeyEvent, ScrollSpec, Span, Tx, col, el, icon, rich, row, text};

use crate::Kit;

/// What a key did to a [`TreeModel`] ([`TreeModel::key`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeKey {
    /// Not a tree key; it goes on to the window.
    Ignored,
    /// It moved the cursor, the selection or a folder.
    Handled,
    /// Return or Space on a row without children: the app opens this id.
    Open(String),
}

struct Node<T> {
    id: String,
    data: T,
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

/// An outline's nodes, open folders, selection and keyboard cursor, kept in the app's state and
/// drawn by [`Tree`]. Nodes are keyed by string ids unique across the tree (a file tree uses
/// paths). Mutating methods take `&mut self`; the visible rows are worked out on the next read.
pub struct TreeModel<T> {
    nodes: Vec<Node<T>>,
    index: HashMap<String, usize>,
    roots: Vec<usize>,
    cursor: Option<usize>,
    anchor: Option<usize>,
    multi: bool,
    filter: Option<Filter>,
    visible: RefCell<Option<Visible>>,
    typed: String,
    typed_at: Option<rune::time::Instant>,
    key: String,
    /// The viewport's height in points, for Page Up and Page Down.
    viewport: f32,
    row_height: f32,
}

impl<T> Default for TreeModel<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Seconds of pause after which typing starts a new name (AppKit's type select).
const TYPE_PAUSE: f64 = 1.0;

impl<T> TreeModel<T> {
    pub fn new() -> TreeModel<T> {
        TreeModel {
            nodes: Vec::new(),
            index: HashMap::new(),
            roots: Vec::new(),
            cursor: None,
            anchor: None,
            multi: false,
            filter: None,
            visible: RefCell::new(None),
            typed: String::new(),
            typed_at: None,
            key: "tree".into(),
            viewport: 0.0,
            row_height: 24.0,
        }
    }

    /// The tree's element key (`"tree"` by default). Its rows are keyed by id inside it, and
    /// [`Self::reveal`] looks for the row there, so give each tree on screen its own.
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = key.into();
        self
    }

    /// Several rows can be selected: Shift with the arrows extends the selection, Cmd-A selects
    /// every row, and [`Self::set_selected`] and [`Self::select_range`] add to it.
    pub fn multi_select(mut self, on: bool) -> Self {
        self.multi = on;
        self
    }

    fn dirty(&mut self) {
        *self.visible.get_mut() = None;
    }

    /// Adds `id` under `parent` (`None` for the top level), after its siblings. Returns false
    /// and adds nothing when `id` is already in the tree or `parent` isn't.
    pub fn insert(&mut self, parent: Option<&str>, id: impl Into<String>, data: T) -> bool {
        let id = id.into();
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
        self.nodes.push(Node { id, data, parent, children: Vec::new(), depth, open: false, selected: false });
        if let Some(f) = &mut self.filter {
            f.shown.push(Shown::Hidden);
            f.closed.push(false);
        }
        self.dirty();
        true
    }

    /// Removes every node, the selection and the filter.
    pub fn clear(&mut self) {
        *self = TreeModel { key: std::mem::take(&mut self.key), multi: self.multi, viewport: self.viewport, row_height: self.row_height, ..TreeModel::new() };
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
    pub fn contains(&self, id: &str) -> bool {
        self.index.contains_key(id)
    }
    pub fn get(&self, id: &str) -> Option<&T> {
        self.index.get(id).map(|&i| &self.nodes[i].data)
    }
    pub fn get_mut(&mut self, id: &str) -> Option<&mut T> {
        self.index.get(id).map(|&i| &mut self.nodes[i].data)
    }
    pub fn parent(&self, id: &str) -> Option<&str> {
        let p = self.nodes[*self.index.get(id)?].parent?;
        Some(&self.nodes[p].id)
    }
    /// `id`'s children in order, or the top-level nodes for `None`.
    pub fn children(&self, id: Option<&str>) -> impl Iterator<Item = &str> {
        let list = match id {
            Some(id) => self.index.get(id).map_or(&[][..], |&i| &self.nodes[i].children[..]),
            None => &self.roots[..],
        };
        list.iter().map(|&c| self.nodes[c].id.as_str())
    }
    pub fn has_children(&self, id: &str) -> bool {
        self.index.get(id).is_some_and(|&i| !self.nodes[i].children.is_empty())
    }
    /// 0 at the top level.
    pub fn depth(&self, id: &str) -> Option<u32> {
        self.index.get(id).map(|&i| self.nodes[i].depth)
    }
    /// The data of `id` and everything under it, depth first.
    pub fn subtree<'a>(&'a self, id: &str) -> impl Iterator<Item = &'a T> + 'a {
        let mut stack: Vec<usize> = self.index.get(id).copied().into_iter().collect();
        std::iter::from_fn(move || {
            let i = stack.pop()?;
            stack.extend(self.nodes[i].children.iter().rev());
            Some(&self.nodes[i].data)
        })
    }

    /// Sorts every node's children (and the top level) by `cmp`.
    pub fn sort_by(&mut self, mut cmp: impl FnMut(&T, &T) -> Ordering) {
        let nodes = &self.nodes;
        let mut order = |list: &mut Vec<usize>| list.sort_by(|&a, &b| cmp(&nodes[a].data, &nodes[b].data));
        let mut lists: Vec<Vec<usize>> = self.nodes.iter().map(|n| n.children.clone()).collect();
        lists.iter_mut().for_each(&mut order);
        order(&mut self.roots);
        for (n, l) in self.nodes.iter_mut().zip(lists) {
            n.children = l;
        }
        self.dirty();
    }

    // Open folders ----------------------------------------------------------------------------

    /// Whether `id` shows its children. Under a filter, folders leading to a match show open
    /// unless closed since.
    pub fn is_expanded(&self, id: &str) -> bool {
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

    pub fn set_expanded(&mut self, id: &str, open: bool) {
        if let Some(&i) = self.index.get(id) {
            self.set_open(i, open);
        }
    }
    pub fn toggle(&mut self, id: &str) {
        if let Some(&i) = self.index.get(id) {
            self.set_open(i, !self.open(i));
        }
    }
    pub fn expand_all(&mut self) {
        for i in 0..self.nodes.len() {
            if !self.nodes[i].children.is_empty() {
                self.set_open(i, true);
            }
        }
    }
    pub fn collapse_all(&mut self) {
        for i in 0..self.nodes.len() {
            self.set_open(i, false);
        }
    }
    /// Opens every folder above `id`, so its row shows.
    pub fn expand_to(&mut self, id: &str) {
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

    /// Selected ids in tree order.
    pub fn selected(&self) -> impl Iterator<Item = &str> {
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
    pub fn is_selected(&self, id: &str) -> bool {
        self.index.get(id).is_some_and(|&i| self.nodes[i].selected)
    }
    /// The keyboard cursor's row: where the arrows move from (the selected row unless Shift or
    /// Cmd made more).
    pub fn cursor(&self) -> Option<&str> {
        self.cursor.map(|c| self.nodes[c].id.as_str())
    }
    pub fn clear_selection(&mut self) {
        self.nodes.iter_mut().for_each(|n| n.selected = false);
    }
    /// Selects `id` alone and puts the cursor on it.
    pub fn select(&mut self, id: &str) {
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
    /// Adds `id` to the selection or takes it out (a multi-select tree; otherwise it selects
    /// `id` alone or clears it).
    pub fn set_selected(&mut self, id: &str, on: bool) {
        let Some(&i) = self.index.get(id) else { return };
        if !self.multi {
            self.clear_selection();
        }
        self.nodes[i].selected = on;
        self.cursor = Some(i);
        self.anchor = Some(i);
    }
    /// Selects the visible rows from the anchor (the last row clicked or moved to without
    /// Shift) to `id`, as Shift-click does.
    pub fn select_range(&mut self, id: &str) {
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

    /// A click on `id`'s row with `mods` held: it selects the row alone, or with Shift extends
    /// the selection to it and with Cmd adds or removes it (a multi-select tree). Returns
    /// whether it was a plain click.
    pub fn click(&mut self, id: &str, mods: rune::Mods) -> bool {
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

    /// Opens the folders above `id`, selects it alone and scrolls its row into view, as a tree
    /// following an editor does. Returns false when `id` isn't in the tree.
    pub fn reveal(&mut self, id: &str, tx: &mut Tx) -> bool {
        if !self.contains(id) {
            return false;
        }
        self.expand_to(id);
        self.select(id);
        tx.scroll_into_view_in(self.key.clone(), id);
        true
    }

    // Filter ---------------------------------------------------------------------------------

    /// Shows only the nodes `keep` matches, with the folders that lead to them (open), as a
    /// file tree's search does. A folder that matches shows without the children that don't.
    pub fn filter(&mut self, keep: impl Fn(&T) -> bool) {
        let n = self.nodes.len();
        let mut shown = vec![Shown::Hidden; n];
        for i in 0..n {
            if keep(&self.nodes[i].data) {
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
    pub fn clear_filter(&mut self) {
        if self.filter.take().is_some() {
            self.dirty();
        }
    }
    pub fn is_filtering(&self) -> bool {
        self.filter.is_some()
    }
    /// Whether the filter matched `id` itself (not just a folder leading to a match); true for
    /// every node without a filter.
    pub fn is_match(&self, id: &str) -> bool {
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

    /// How many rows show.
    pub fn visible_len(&self) -> usize {
        self.rows().rows.len()
    }
    /// The id of visible row `row`.
    pub fn visible_id(&self, row: usize) -> Option<&str> {
        let i = *self.rows().rows.get(row)?;
        Some(&self.nodes[i].id)
    }
    /// The row `id` shows at, if it shows.
    pub fn visible_row(&self, id: &str) -> Option<usize> {
        let i = *self.index.get(id)?;
        Some(self.rows().row_of[i]).filter(|&r| r != usize::MAX)
    }

    /// Visible row `row`'s data and state, for drawing it.
    pub fn row(&self, row: usize) -> Option<(&T, TreeRow<'_>)> {
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
        Some((&n.data, state))
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

    /// Applies a key (see the module docs); `label` gives a node's name for typing to jump.
    /// [`Tree`] calls this from its key handler, and scrolls the cursor's row into view.
    pub fn handle_key(&mut self, ev: &KeyEvent, label: impl Fn(&T) -> &str, tx: &mut Tx) -> TreeKey {
        let m = ev.mods;
        if m.ctrl && !m.cmd || m.alt || m.logo && !m.cmd {
            return TreeKey::Ignored;
        }
        let len = self.visible_len();
        if len == 0 {
            return TreeKey::Ignored;
        }
        if ev.is_command('a') && self.multi {
            let rows = self.rows().rows.clone();
            for i in rows {
                self.nodes[i].selected = true;
            }
            return TreeKey::Handled;
        }
        if m.cmd {
            return TreeKey::Ignored;
        }
        let at = self.cursor.map(|c| self.rows().row_of[c]).filter(|&r| r != usize::MAX);
        let page = ((self.viewport / self.row_height.max(1.0)).floor() as usize).saturating_sub(1).max(1);
        let target = match &ev.key {
            Key::Up => Some(at.map_or(len - 1, |r| r.saturating_sub(1))),
            Key::Down => Some(at.map_or(0, |r| (r + 1).min(len - 1))),
            Key::Home => Some(0),
            Key::End => Some(len - 1),
            Key::PageUp => Some(at.map_or(0, |r| r.saturating_sub(page))),
            Key::PageDown => Some(at.map_or(0, |r| (r + page).min(len - 1))),
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
            Key::Enter => {
                let Some(r) = at else { return TreeKey::Ignored };
                if tx.key_repeat() {
                    return TreeKey::Handled;
                }
                let i = self.rows().rows[r];
                if self.openable(i) {
                    self.set_open(i, !self.open(i));
                    return TreeKey::Handled;
                }
                return TreeKey::Open(self.nodes[i].id.clone());
            }
            Key::Text(t) if t == " " => {
                let Some(r) = at else { return TreeKey::Ignored };
                if tx.key_repeat() {
                    return TreeKey::Handled;
                }
                let i = self.rows().rows[r];
                if self.openable(i) {
                    self.set_open(i, !self.open(i));
                    return TreeKey::Handled;
                }
                return TreeKey::Open(self.nodes[i].id.clone());
            }
            Key::Text(t) if !t.trim().is_empty() => {
                let now = rune::time::Instant::now();
                let pause = self.typed_at.is_none_or(|a| now.duration_since(a).as_secs_f64() > TYPE_PAUSE);
                self.typed_at = Some(now);
                return match self.type_select(t, pause, &label) {
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

    fn go(&mut self, row: usize, extend: bool, tx: &mut Tx) -> TreeKey {
        let i = self.rows().rows[row];
        self.move_to(i, extend);
        tx.scroll_into_view_in(self.key.clone(), self.nodes[i].id.clone());
        TreeKey::Handled
    }

    /// The row typing `typed` jumps to: the next whose name starts with everything typed since
    /// the last pause, and the same letter again cycles through names starting with it.
    fn type_select(&mut self, typed: &str, pause: bool, label: impl Fn(&T) -> &str) -> Option<usize> {
        if pause {
            self.typed.clear();
        }
        self.typed.push_str(&typed.to_lowercase());
        let first = self.typed.chars().next()?;
        let cycle = self.typed.chars().all(|c| c == first);
        let prefix = if cycle { first.to_string() } else { self.typed.clone() };
        let v = self.rows();
        let len = v.rows.len();
        let at = self.cursor.map(|c| v.row_of[c]).filter(|&r| r != usize::MAX);
        // A longer name keeps matching the row it's on; a repeated letter moves on.
        let start = match at {
            Some(r) if cycle => r + 1,
            Some(r) => r,
            None => 0,
        };
        (0..len).map(|k| (start + k) % len).find(|&r| label(&self.nodes[v.rows[r]].data).to_lowercase().starts_with(&prefix))
    }
}

/// A row's state as the tree draws it.
#[derive(Clone, Copy, Debug)]
pub struct TreeRow<'a> {
    pub id: &'a str,
    /// 0 at the top level.
    pub depth: u32,
    /// `Some(open)` for a row with children (shown ones, under a filter), `None` for a leaf.
    pub expanded: Option<bool>,
    pub selected: bool,
    /// The keyboard cursor is on it.
    pub cursor: bool,
    /// A filter matched it (false without a filter).
    pub matched: bool,
    multi: bool,
}

impl TreeRow<'_> {
    /// A row's state for drawing a [`TreeItem`] outside a [`Tree`] (a sidebar, say).
    pub fn new(id: &str, depth: u32) -> TreeRow<'_> {
        TreeRow { id, depth, expanded: None, selected: false, cursor: false, matched: false, multi: false }
    }
    pub fn expanded(mut self, open: Option<bool>) -> Self {
        self.expanded = open;
        self
    }
    pub fn selected(mut self, on: bool) -> Self {
        self.selected = on;
        self
    }
}

/// What a standard tree row shows after its indent and chevron: an icon, a label and trailing
/// content (change counts, a status dot, a badge, or any element).
pub struct TreeItem {
    label: String,
    icon: Option<&'static Icon>,
    icon_color: Option<Color>,
    label_color: Option<Color>,
    highlight: Option<String>,
    changes: Option<(u32, u32)>,
    dot: Option<Color>,
    badge: Option<String>,
    trailing: Vec<El>,
}

impl TreeItem {
    pub fn new(label: impl Into<String>) -> TreeItem {
        TreeItem {
            label: label.into(),
            icon: None,
            icon_color: None,
            label_color: None,
            highlight: None,
            changes: None,
            dot: None,
            badge: None,
            trailing: Vec::new(),
        }
    }
    /// A file tree's row: the node's name and its folder or file icon ([`file_icon`]).
    pub fn file<D>(node: &FileNode<D>, row: &TreeRow) -> TreeItem {
        TreeItem::new(node.name.clone()).icon(node.icon(row.expanded == Some(true)))
    }
    pub fn icon(mut self, i: &'static Icon) -> Self {
        self.icon = Some(i);
        self
    }
    pub fn icon_color(mut self, c: Color) -> Self {
        self.icon_color = Some(c);
        self
    }
    /// The label's colour (the theme's text colour by default): dimmed for an ignored file, say.
    pub fn label_color(mut self, c: Color) -> Self {
        self.label_color = Some(c);
        self
    }
    /// Marks the first case-insensitive occurrence of `query` in the label, as a search does.
    pub fn highlight(mut self, query: impl Into<String>) -> Self {
        let q = query.into();
        self.highlight = (!q.is_empty()).then_some(q);
        self
    }
    /// Lines added and removed, as `+12 −3` (a zero side is left out; both zero shows nothing).
    pub fn changes(mut self, added: u32, removed: u32) -> Self {
        self.changes = (added > 0 || removed > 0).then_some((added, removed));
        self
    }
    /// A small status dot at the end of the row.
    pub fn dot(mut self, c: Color) -> Self {
        self.dot = Some(c);
        self
    }
    /// A short status letter or count at the end of the row (`M`, `3`).
    pub fn badge(mut self, b: impl Into<String>) -> Self {
        self.badge = Some(b.into());
        self
    }
    /// Any element at the end of the row.
    pub fn trailing(mut self, e: El) -> Self {
        self.trailing.push(e);
        self
    }

    /// The row as [`Tree`] draws it, without handlers: indent guides, chevron, icon, label and
    /// trailing content, `style.row_height` tall. A sidebar can draw its rows with this.
    pub fn build(self, k: &Kit, r: &TreeRow, style: &TreeStyle) -> El {
        let c = &k.c;
        let pad = style.padding;
        let chevron_w = 16.0;
        let mut e = row()
            .role("treeitem")
            .label(self.label.clone())
            .level(r.depth + 1)
            .focus_selected(r.selected)
            .w_full()
            .h(style.row_height)
            .pl(pad + r.depth as f32 * style.indent)
            .pr(pad)
            .gap(4.0)
            .center_y()
            .radius(k.radius.sm)
            .motion(k.motion.control);
        if let Some(open) = r.expanded {
            e = e.expanded(open);
        }
        e = if r.selected { e.fill(style.selected) } else { e.hover(c.hover) };
        if r.cursor && r.multi {
            e = e.outline(1.0, -1.0, c.border_focus);
        }
        if style.guides {
            for level in 0..r.depth {
                let x = pad + level as f32 * style.indent + chevron_w * 0.5 - 0.5;
                e = e.child(el().key(format!("guide{level}")).pin(Some(x), Some(0.0), None, Some(0.0)).w(1.0).fill(style.guide));
            }
        }
        let chevron = el().key("chevron").size(chevron_w, chevron_w).center();
        e = e.child(match r.expanded {
            Some(open) => chevron.child(
                icon(k.glyph.chevron_down, 16.0)
                    .key("glyph")
                    .color(c.icon_muted)
                    .icon_rotation(if open { 0.0 } else { -FRAC_PI_2 })
                    .motion(k.motion.control),
            ),
            None => chevron,
        });
        if let Some(i) = self.icon {
            e = e.child(icon(i, 16.0).key("icon").color(self.icon_color.unwrap_or(c.icon_muted)));
        }
        let color = self.label_color.unwrap_or(c.text_base);
        let font = k.ty.ui;
        let spans = self
            .highlight
            .as_deref()
            .and_then(|q| find_ci(&self.label, q))
            .map(|(a, b)| vec![Span { color: Some(c.text_base), weight: Some(650.0), bg: Some(k.alpha(c.text_accent, 0.22)), bg_pad: 1.0, ..Span::new(a, b) }])
            .unwrap_or_default();
        let full = self.label.clone();
        let label = if spans.is_empty() { text(self.label) } else { rich(self.label, spans) };
        e = e.child(label.key("label").font(font).color(color).grow(1.0).shrink().min_w(0.0).truncate().help_when_truncated(full));
        if let Some((add, del)) = self.changes {
            let mut counts = row().key("changes").gap(4.0).center_y();
            let f = k.ty.text_xsmall.features("'tnum' 1");
            if add > 0 {
                counts = counts.child(text(format!("+{add}")).key("added").font(f).color(c.success.icon));
            }
            if del > 0 {
                counts = counts.child(text(format!("\u{2212}{del}")).key("removed").font(f).color(c.danger.icon));
            }
            e = e.child(counts);
        }
        if let Some(b) = self.badge {
            e = e.child(text(b).key("badge").font(k.ty.text_xsmall_medium).color(c.text_muted));
        }
        if let Some(d) = self.dot {
            e = e.child(el().key("dot").size(6.0, 6.0).radius(3.0).fill(d).ml(5.0).mr(5.0));
        }
        e.children(self.trailing)
    }
}

/// The byte range of `q` in `s`, ignoring ASCII case.
fn find_ci(s: &str, q: &str) -> Option<(usize, usize)> {
    let (sl, ql) = (s.to_lowercase(), q.to_lowercase());
    // Lowercasing can change byte lengths outside ASCII; only trust a same-length mapping.
    if sl.len() != s.len() {
        return s.find(q).map(|a| (a, a + q.len()));
    }
    sl.find(&ql).map(|a| (a, a + ql.len()))
}

/// A tree's metrics and colours. [`TreeStyle::from_kit`] takes them from the theme.
#[derive(Clone, Copy, Debug)]
pub struct TreeStyle {
    pub row_height: f32,
    /// Points each level steps in.
    pub indent: f32,
    /// Space at the row's ends.
    pub padding: f32,
    /// Hairlines down each level's indent.
    pub guides: bool,
    pub guide: Color,
    pub selected: Color,
}

impl TreeStyle {
    pub fn from_kit(k: &Kit) -> TreeStyle {
        TreeStyle {
            row_height: 24.0,
            indent: 14.0,
            padding: 6.0,
            guides: true,
            guide: k.c.border_muted,
            selected: k.alpha(k.c.text_accent, if k.dark { 0.24 } else { 0.2 }),
        }
    }
}

type RowFn<T> = Rc<dyn for<'a> Fn(&T, &TreeRow<'a>) -> TreeItem>;
type IdFn<S> = Rc<dyn Fn(&mut S, &str, &mut Tx)>;

/// A virtualised tree over a [`TreeModel`] in the app's state `S`.
pub struct Tree<S, T> {
    get: fn(&S) -> &TreeModel<T>,
    get_mut: fn(&mut S) -> &mut TreeModel<T>,
    label: fn(&T) -> &str,
    row: Option<RowFn<T>>,
    on_select: Option<IdFn<S>>,
    on_open: Option<IdFn<S>>,
    style: Option<TreeStyle>,
    name: Option<String>,
}

impl<S: 'static, T: 'static> Tree<S, T> {
    /// `get` and `get_mut` find the model in the state; `label` names a node (rows show it, and
    /// typing jumps by it).
    pub fn new(get: fn(&S) -> &TreeModel<T>, get_mut: fn(&mut S) -> &mut TreeModel<T>, label: fn(&T) -> &str) -> Self {
        Tree { get, get_mut, label, row: None, on_select: None, on_open: None, style: None, name: None }
    }
    /// What each row shows; by default its label alone.
    pub fn row(mut self, f: impl for<'a> Fn(&T, &TreeRow<'a>) -> TreeItem + 'static) -> Self {
        self.row = Some(Rc::new(f));
        self
    }
    /// The cursor moved to `id`, by click or key: a diff view can follow it.
    pub fn on_select(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }
    /// A row without children was double-clicked, or Return or Space pressed on it.
    pub fn on_open(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self {
        self.on_open = Some(Rc::new(f));
        self
    }
    pub fn style(mut self, s: TreeStyle) -> Self {
        self.style = Some(s);
        self
    }
    /// The tree's accessibility label ("Files").
    pub fn name(mut self, n: impl Into<String>) -> Self {
        self.name = Some(n.into());
        self
    }

    /// The tree, filling the height its parent gives it (set one with `.h(..)` or `.grow(..)`).
    pub fn build(self, s: &S, k: &Kit) -> El {
        let Tree { get, get_mut, label, row, on_select, on_open, style, name } = self;
        let style = style.unwrap_or_else(|| TreeStyle::from_kit(k));
        let model = get(s);
        let count = model.visible_len();
        let tree_key = model.key.clone();
        let kit = Rc::new(k.clone());
        let row_fn = row;
        let (sel, open) = (on_select.clone(), on_open.clone());
        let build_row = move |s: &S, i: usize| -> El {
            let m = get(s);
            let Some((data, r)) = m.row(i) else { return el() };
            let item = match &row_fn {
                Some(f) => f(data, &r),
                None => TreeItem::new(label(data)),
            };
            let id = r.id.to_owned();
            let openable = r.expanded.is_some();
            let mut e = item.build(&kit, &r, &style).key(r.id);
            // A click selects the row (Shift extends and Cmd toggles, in a multi-select tree);
            // on a folder it also opens or closes it, as a file tree's does.
            let pick = id.clone();
            let sel = sel.clone();
            e = e.on_press_tx(move |s: &mut S, tx: &mut Tx| {
                let m = get_mut(s);
                let plain = m.click(&pick, tx.mods());
                if openable && plain {
                    m.toggle(&pick);
                }
                if let Some(f) = &sel {
                    f(s, &pick, tx);
                }
            });
            if !openable && let Some(f) = open.clone() {
                e = e.on_double_press_tx(move |s: &mut S, tx: &mut Tx| f(s, &id, tx));
            }
            e
        };
        let rows = col()
            .key("rows")
            .w_full()
            .growth(GrowthSpec::default())
            .virtual_rows(
                count,
                style.row_height,
                move |s: &S, i| Cow::Borrowed(get(s).visible_id(i).unwrap_or_default()),
                build_row,
            );
        let viewport = col()
            .key("viewport")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec { stick_bottom: false })
            // Inside a scrolling page this reports every frame of the scroll; only a new height
            // changes what the view reads.
            .on_layout_tx(move |s: &mut S, r: [f32; 4], tx: &mut Tx| {
                let m = get_mut(s);
                if m.viewport == r[3] && m.row_height == style.row_height { tx.live_only(); }
                m.viewport = r[3];
                m.row_height = style.row_height;
            })
            .child(rows);
        let mut root = col()
            .key(tree_key)
            .role("tree")
            .focusable(true)
            .w_full();
        if let Some(n) = name {
            root = root.label(n);
        }
        root
            .on_key(move |s: &mut S, ev: &KeyEvent, tx: &mut Tx| {
                let m = get_mut(s);
                let before = m.cursor;
                let out = m.handle_key(ev, label, tx);
                let after = m.cursor;
                let id = after.map(|c| m.nodes[c].id.clone());
                match out {
                    TreeKey::Ignored => false,
                    TreeKey::Handled => {
                        if before != after && let (Some(f), Some(id)) = (&on_select, id) {
                            f(s, &id, tx);
                        }
                        true
                    }
                    TreeKey::Open(id) => {
                        if let Some(f) = &on_open {
                            f(s, &id, tx);
                        }
                        true
                    }
                }
            })
            .child(viewport)
    }
}

// Files ------------------------------------------------------------------------------------

/// A node of a file tree: a folder (`dir`) or a file carrying the app's data.
#[derive(Clone, Debug, PartialEq)]
pub struct FileNode<D> {
    /// What the row shows: the file or folder name, or `a/b/c` for folders compacted into one.
    pub name: String,
    /// The path from the root, which is also the node's id.
    pub path: String,
    pub dir: bool,
    /// The file's data; `None` for folders.
    pub data: Option<D>,
}

impl<D> FileNode<D> {
    pub fn label(&self) -> &str {
        &self.name
    }
    /// The folder or file icon for this node ([`file_icon`]).
    pub fn icon(&self, open: bool) -> &'static Icon {
        if self.dir {
            if open { &crate::glyphs::FOLDER_OPEN } else { &crate::glyphs::FOLDER }
        } else {
            file_icon(&self.name)
        }
    }
}

/// An icon for a file by its extension: code, text and docs, images, or a plain page.
pub fn file_icon(name: &str) -> &'static Icon {
    let ext = name.rsplit_once('.').map_or("", |(_, e)| e).to_ascii_lowercase();
    match ext.as_str() {
        "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "py" | "go" | "swift" | "c" | "h" | "cc" | "cpp" | "hpp" | "m" | "mm"
        | "java" | "kt" | "rb" | "php" | "cs" | "zig" | "lua" | "sh" | "bash" | "zsh" | "fish" | "wgsl" | "glsl" | "metal" | "html"
        | "css" | "scss" | "vue" | "svelte" | "sql" | "ex" | "exs" | "hs" | "ml" | "scala" | "dart" => &crate::glyphs::FILE_CODE,
        "md" | "mdx" | "txt" | "rst" | "adoc" | "json" | "jsonc" | "toml" | "yaml" | "yml" | "lock" | "xml" | "csv" | "ini" | "cfg"
        | "conf" | "env" => &crate::glyphs::FILE_TEXT,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "bmp" | "tiff" | "avif" | "heic" => &crate::glyphs::FILE_IMAGE,
        _ => &crate::glyphs::FILE,
    }
}

/// Builds a [`TreeModel`] of [`FileNode`]s from file paths (`/`-separated, relative to a root).
///
/// By default folders sort before files, names sort naturally (`file2` before `file10`, case
/// aside), and a chain of folders that each hold only one folder shows as one row named
/// `a/b/c`, as an editor's compact folders do. Folders start closed; open them on the model.
pub struct FileTree {
    compact: bool,
    dirs_first: bool,
    key: String,
}

impl Default for FileTree {
    fn default() -> Self {
        Self::new()
    }
}

impl FileTree {
    pub fn new() -> FileTree {
        FileTree { compact: true, dirs_first: true, key: "files".into() }
    }
    /// Show a chain of single-folder folders as one `a/b/c` row (on by default).
    pub fn compact_folders(mut self, on: bool) -> Self {
        self.compact = on;
        self
    }
    /// Sort folders before files (on by default); off, they mix by name.
    pub fn dirs_first(mut self, on: bool) -> Self {
        self.dirs_first = on;
        self
    }
    /// The model's row-key prefix ([`TreeModel::key`], `"files"` here).
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = key.into();
        self
    }

    pub fn build<P: AsRef<str>, D>(self, files: impl IntoIterator<Item = (P, D)>) -> TreeModel<FileNode<D>> {
        // A trie of folders; files hang off their folder.
        struct Dir<D> {
            dirs: Vec<(String, Dir<D>)>,
            files: Vec<(String, D)>,
        }
        impl<D> Dir<D> {
            fn dir(&mut self, name: &str) -> &mut Dir<D> {
                let i = match self.dirs.iter().position(|(n, _)| n == name) {
                    Some(i) => i,
                    None => {
                        self.dirs.push((name.to_owned(), Dir { dirs: Vec::new(), files: Vec::new() }));
                        self.dirs.len() - 1
                    }
                };
                &mut self.dirs[i].1
            }
        }
        let mut root: Dir<D> = Dir { dirs: Vec::new(), files: Vec::new() };
        for (path, data) in files {
            let path = path.as_ref().trim_matches('/');
            if path.is_empty() {
                continue;
            }
            let mut parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
            let file = parts.pop().unwrap();
            let dir = parts.iter().fold(&mut root, |d, p| d.dir(p));
            dir.files.push((file.to_owned(), data));
        }
        let mut model = TreeModel::new().key(self.key.clone());
        fn emit<D>(model: &mut TreeModel<FileNode<D>>, compact: bool, parent: Option<&str>, prefix: &str, dir: Dir<D>) {
            for (name, mut d) in dir.dirs {
                let mut name = name;
                let mut path = join(prefix, &name);
                while compact && d.files.is_empty() && d.dirs.len() == 1 {
                    let (n, inner) = d.dirs.pop().unwrap();
                    name = format!("{name}/{n}");
                    path = join(&path, &n);
                    d = inner;
                }
                model.insert(parent, path.clone(), FileNode { name, path: path.clone(), dir: true, data: None });
                emit(model, compact, Some(&path), &path, d);
            }
            for (name, data) in dir.files {
                let path = join(prefix, &name);
                // A file sharing a folder's path (`a` and `a/b`) is left out.
                model.insert(parent, path.clone(), FileNode { name, path, dir: false, data: Some(data) });
            }
        }
        fn join(a: &str, b: &str) -> String {
            if a.is_empty() { b.to_owned() } else { format!("{a}/{b}") }
        }
        emit(&mut model, self.compact, None, "", root);
        let dirs_first = self.dirs_first;
        model.sort_by(|a, b| {
            let by_kind = if dirs_first { b.dir.cmp(&a.dir) } else { Ordering::Equal };
            by_kind.then_with(|| natural_cmp(&a.name, &b.name))
        });
        model
    }
}

/// Compares names as people sort them: case aside, and runs of digits by value (`file2` before
/// `file10`). Ties fall back to the exact text so the order is total.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(p), Some(q)) if p.is_ascii_digit() && q.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(&c) = it.peek().filter(|c| c.is_ascii_digit()) {
                        s.push(c);
                        it.next();
                    }
                    s
                };
                let (m, n) = (take(&mut x), take(&mut y));
                let (mt, nt) = (m.trim_start_matches('0'), n.trim_start_matches('0'));
                let o = mt.len().cmp(&nt.len()).then_with(|| mt.cmp(nt));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(p), Some(q)) => {
                let o = p.to_lowercase().cmp(q.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> TreeModel<&'static str> {
        let mut m = TreeModel::new();
        m.insert(None, "a", "alpha");
        m.insert(Some("a"), "a/1", "one");
        m.insert(Some("a"), "a/2", "two");
        m.insert(None, "b", "beta");
        m.insert(None, "c", "bravo");
        m
    }

    #[test]
    fn typing_jumps_by_prefix_and_a_repeated_letter_cycles() {
        let mut m = model();
        fn label<'a>(t: &'a &'static str) -> &'a str {
            t
        }
        let r = m.type_select("b", true, label);
        assert_eq!(r, Some(1), "beta: rows are alpha, beta, bravo");
        m.select("b");
        assert_eq!(m.type_select("b", false, label), Some(2), "b again cycles to bravo");
        m.select("c");
        assert_eq!(m.type_select("r", false, label), None, "\"bbr\" is no name");
        assert_eq!(m.type_select("b", true, label), Some(1), "after a pause, a new prefix from the next row");
        m.select("a");
        assert_eq!(m.type_select("b", true, label), Some(1));
        assert_eq!(m.type_select("r", false, label), Some(2), "\"br\" keeps looking past beta to bravo");
    }

    #[test]
    fn natural_order_counts_digits_and_ignores_case() {
        let mut v = vec!["file10", "File2", "file1", "a", "B"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["a", "B", "file1", "File2", "file10"]);
    }
}
