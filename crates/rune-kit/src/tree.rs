//! Tree: a virtualised outline over a [`TreeModel`] the app keeps in its state. A file tree is
//! the main use ([`FileTree`] builds one from paths), but rows hold any data.
//!
//! The model owns the nodes, their open state, the selection and the keyboard cursor; the
//! [`Tree`] builder draws the rows that are on screen ([`rune::El::virtual_rows`]), so 50,000 nodes
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

use std::cell::{Ref, RefCell};
use std::cmp::Ordering;
use std::collections::HashMap;

use rune::{Key, KeyEvent, Tx};

mod file_tree;
mod view;

#[cfg(test)]
mod tests;

pub use file_tree::{FileNode, FileTree, file_icon, natural_cmp};
pub use view::{Tree, TreeItem, TreeStyle};

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
            Key::Enter => return self.activate(at, tx),
            Key::Text(t) if t == " " => return self.activate(at, tx),
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
