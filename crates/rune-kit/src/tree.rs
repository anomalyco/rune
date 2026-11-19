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

use std::cmp::Ordering;

use rune::{KeyEvent, Tx};

mod file_tree;
mod meta;
mod view;

#[cfg(test)]
mod tests;

pub use file_tree::{FileNode, FileTree, file_icon, natural_cmp};
pub use view::{Tree, TreeItem, TreeStyle};

use meta::TreeMeta;

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

/// An outline's nodes, open folders, selection and keyboard cursor, kept in the app's state and
/// drawn by [`Tree`]. Nodes are keyed by string ids unique across the tree (a file tree uses
/// paths). Mutating methods take `&mut self`; the visible rows are worked out on the next read.
pub struct TreeModel<T> {
    meta: TreeMeta,
    data: Vec<T>,
}

impl<T> Default for TreeModel<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> TreeModel<T> {
    pub fn new() -> TreeModel<T> {
        TreeModel { meta: TreeMeta::new(), data: Vec::new() }
    }

    /// The tree's element key (`"tree"` by default). Its rows are keyed by id inside it, and
    /// [`Self::reveal`] looks for the row there, so give each tree on screen its own.
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.meta.set_key(key.into());
        self
    }

    /// Several rows can be selected: Shift with the arrows extends the selection, Cmd-A selects
    /// every row, and [`Self::set_selected`] and [`Self::select_range`] add to it.
    pub fn multi_select(mut self, on: bool) -> Self {
        self.meta.set_multi(on);
        self
    }

    /// The tree lies in a right-to-left window, its rows indenting from the right: Left opens a
    /// folder and goes into it, Right closes it and goes to its parent, as Explorer and Finder
    /// swap them.
    pub fn set_right_to_left(&mut self, rtl: bool) {
        self.meta.set_right_to_left(rtl);
    }

    /// Adds `id` under `parent` (`None` for the top level), after its siblings. Returns false
    /// and adds nothing when `id` is already in the tree or `parent` isn't.
    pub fn insert(&mut self, parent: Option<&str>, id: impl Into<String>, data: T) -> bool {
        if !self.meta.insert(parent, id.into()) {
            return false;
        }
        self.data.push(data);
        true
    }

    /// Removes every node, the selection and the filter.
    pub fn clear(&mut self) {
        self.meta.clear();
        self.data.clear();
    }

    pub fn len(&self) -> usize { self.meta.len() }
    pub fn is_empty(&self) -> bool { self.meta.is_empty() }
    pub fn contains(&self, id: &str) -> bool { self.meta.contains(id) }
    pub fn get(&self, id: &str) -> Option<&T> { self.meta.index_of(id).map(|i| &self.data[i]) }
    pub fn get_mut(&mut self, id: &str) -> Option<&mut T> { self.meta.index_of(id).map(|i| &mut self.data[i]) }
    pub fn parent(&self, id: &str) -> Option<&str> { self.meta.parent(id) }
    /// `id`'s children in order, or the top-level nodes for `None`.
    pub fn children(&self, id: Option<&str>) -> impl Iterator<Item = &str> { self.meta.children(id) }
    pub fn has_children(&self, id: &str) -> bool { self.meta.has_children(id) }
    /// 0 at the top level.
    pub fn depth(&self, id: &str) -> Option<u32> { self.meta.depth(id) }
    /// The data of `id` and everything under it, depth first.
    pub fn subtree<'a>(&'a self, id: &str) -> impl Iterator<Item = &'a T> + 'a {
        self.meta.subtree_indices(id).map(|i| &self.data[i])
    }

    /// Sorts every node's children (and the top level) by `cmp`.
    pub fn sort_by(&mut self, mut cmp: impl FnMut(&T, &T) -> Ordering) {
        let data = &self.data;
        self.meta.sort_by(&mut |a, b| cmp(&data[a], &data[b]));
    }

    /// Whether `id` shows its children. Under a filter, folders leading to a match show open
    /// unless closed since.
    pub fn is_expanded(&self, id: &str) -> bool { self.meta.is_expanded(id) }
    pub fn set_expanded(&mut self, id: &str, open: bool) { self.meta.set_expanded(id, open); }
    pub fn toggle(&mut self, id: &str) { self.meta.toggle(id); }
    pub fn expand_all(&mut self) { self.meta.expand_all(); }
    pub fn collapse_all(&mut self) { self.meta.collapse_all(); }
    /// Opens every folder above `id`, so its row shows.
    pub fn expand_to(&mut self, id: &str) { self.meta.expand_to(id); }

    /// Selected ids in tree order.
    pub fn selected(&self) -> impl Iterator<Item = &str> { self.meta.selected() }
    pub fn is_selected(&self, id: &str) -> bool { self.meta.is_selected(id) }
    /// The keyboard cursor's row: where the arrows move from (the selected row unless Shift or
    /// Cmd made more).
    pub fn cursor(&self) -> Option<&str> { self.meta.cursor() }
    pub fn clear_selection(&mut self) { self.meta.clear_selection(); }
    /// Selects `id` alone and puts the cursor on it.
    pub fn select(&mut self, id: &str) { self.meta.select(id); }
    /// Adds `id` to the selection or takes it out (a multi-select tree; otherwise it selects
    /// `id` alone or clears it).
    pub fn set_selected(&mut self, id: &str, on: bool) { self.meta.set_selected(id, on); }
    /// Selects the visible rows from the anchor (the last row clicked or moved to without
    /// Shift) to `id`, as Shift-click does.
    pub fn select_range(&mut self, id: &str) { self.meta.select_range(id); }
    /// A click on `id`'s row with `mods` held: it selects the row alone, or with Shift extends
    /// the selection to it and with Cmd adds or removes it (a multi-select tree). Returns
    /// whether it was a plain click.
    pub fn click(&mut self, id: &str, mods: rune::Mods) -> bool { self.meta.click(id, mods) }
    /// Opens the folders above `id`, selects it alone and scrolls its row into view, as a tree
    /// following an editor does. Returns false when `id` isn't in the tree.
    pub fn reveal(&mut self, id: &str, tx: &mut Tx) -> bool { self.meta.reveal(id, tx) }

    /// Shows only the nodes `keep` matches, with the folders that lead to them (open), as a
    /// file tree's search does. A folder that matches shows without the children that don't.
    pub fn filter(&mut self, keep: impl Fn(&T) -> bool) {
        let data = &self.data;
        self.meta.filter(&|i| keep(&data[i]));
    }
    pub fn clear_filter(&mut self) { self.meta.clear_filter(); }
    pub fn is_filtering(&self) -> bool { self.meta.is_filtering() }
    /// Whether the filter matched `id` itself (not just a folder leading to a match); true for
    /// every node without a filter.
    pub fn is_match(&self, id: &str) -> bool { self.meta.is_match(id) }

    /// How many rows show.
    pub fn visible_len(&self) -> usize { self.meta.visible_len() }
    /// The id of visible row `row`.
    pub fn visible_id(&self, row: usize) -> Option<&str> { self.meta.visible_id(row) }
    /// The row `id` shows at, if it shows.
    pub fn visible_row(&self, id: &str) -> Option<usize> { self.meta.visible_row(id) }
    /// Visible row `row`'s data and state, for drawing it.
    pub fn row(&self, row: usize) -> Option<(&T, TreeRow<'_>)> {
        let (i, state) = self.meta.row(row)?;
        Some((&self.data[i], state))
    }

    /// Applies a key (see the module docs); `label` gives a node's name for typing to jump.
    /// [`Tree`] calls this from its key handler, and scrolls the cursor's row into view.
    pub fn handle_key(&mut self, ev: &KeyEvent, label: impl Fn(&T) -> &str, tx: &mut Tx) -> TreeKey {
        let data = &self.data;
        self.meta.handle_key(ev, &|i| label(&data[i]), tx)
    }

    #[cfg(test)]
    fn type_select(&self, prefix: &str, label: impl Fn(&T) -> &str) -> Option<usize> {
        let data = &self.data;
        self.meta.type_select(prefix, &|i| label(&data[i]))
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
