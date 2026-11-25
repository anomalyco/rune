//! Runtime state for [`El::virtual_items`](crate::El::virtual_items): owned in a single
//! `virtual_items` field on [`crate::runtime::commit::CommitState`].

use std::cmp::Ordering;
use std::rc::Rc;

use crate::El;
use crate::el::ElFlags;
use crate::runtime::commit::path::{decode_key_segment, node_path};
use crate::runtime::{HashMap, HashSet, Node};

pub(crate) mod fill;
use fill::{Fill, FillEffect, FillEvent};

/// Pending window commit computed during `measure` or `remount`, finalized into the
/// container's committed state only when `RuntimeCore::apply` commits the pass.
pub(crate) struct StagedWindow {
    seen_version: u64,
    desired_keys: Vec<String>,
    desired_set: HashSet<String>,
    next_mounted: Vec<String>,
    next_pending: Vec<String>,
    pinned_unlisted: Option<String>,
}

/// Per-container state for an `El::virtual_items` node at a tree path.
pub(crate) struct ItemContainer {
    pub(crate) source: Rc<dyn crate::VirtualItems>,
    seen_version: Option<u64>,
    desired_keys: Vec<String>,
    desired_set: HashSet<String>,
    mounted_set: HashSet<String>,
    /// Entering keys still awaiting a frame budget slot, ordered lowest priority first.
    pending: Vec<String>,
    /// Item key currently holding keyboard focus (or containing the focused descendant).
    pub(crate) focused_item: Option<String>,
    /// Focused item key currently mounted solely because focus pinned it (omitted from `keys()`).
    pinned_unlisted: Option<String>,
    staged: Option<StagedWindow>,
}

/// Remount plan for one `El::virtual_items` container.
pub(in crate::runtime) struct ItemsPlan {
    pub(in crate::runtime) path: String,
    pub(in crate::runtime) old_idx: usize,
    pub(in crate::runtime) subtree_end: usize,
    pub(in crate::runtime) mounted_items: HashMap<String, (usize, usize)>,
    pub(in crate::runtime) next_mounted: Vec<String>,
    pub(in crate::runtime) source: Rc<dyn crate::VirtualItems>,
    pub(in crate::runtime) built_items: Vec<(usize, El)>,
}

/// All live `El::virtual_items` containers and their shared fill state machine.
#[derive(Default)]
pub(crate) struct VirtualItemsState {
    containers: HashMap<String, ItemContainer>,
    fill: Fill,
}

#[inline]
fn priority_cmp(a: f32, b: f32) -> Ordering {
    match (a.is_finite(), b.is_finite()) {
        (true, true) => a.total_cmp(&b),
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => Ordering::Equal,
    }
}

fn path_is_inside_item(focused_path: &str, item_path: &str) -> bool {
    focused_path == item_path
        || focused_path
            .strip_prefix(item_path)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn dedupe_keys(
    raw: Vec<String>,
    path: &str,
    key_warnings: &mut Option<Vec<String>>,
) -> (Vec<String>, HashSet<String>) {
    let mut keys = Vec::with_capacity(raw.len());
    let mut set = HashSet::with_capacity_and_hasher(raw.len(), Default::default());
    let mut duplicates = Vec::new();
    for key in raw {
        if set.insert(key.clone()) {
            keys.push(key);
        } else {
            let message = format!(
                "duplicate virtual item key {key:?} under {path:?}; give items unique keys"
            );
            duplicates.push(crate::input::key_diagnostics::Duplicate { key, message });
        }
    }
    if !duplicates.is_empty() {
        match key_warnings.as_mut() {
            Some(out) => out.extend(duplicates.into_iter().map(|d| d.message)),
            None => crate::input::key_diagnostics::warn_once(&duplicates),
        }
    }
    (keys, set)
}

pub(crate) fn build_item_el(
    source: &dyn crate::VirtualItems,
    path: &str,
    key: &str,
    key_warnings: &mut Option<Vec<String>>,
) -> El {
    let tag = crate::memo::virtual_row_tag(path, key);
    let mut item = crate::memo::in_virtual_row(tag, || source.build(key)).key(key.to_owned());
    item.flags.set(ElFlags::VIRTUAL_MOUNT, true);
    if crate::input::key_diagnostics::clean(&item) {
        return item;
    }
    let duplicates = crate::input::key_diagnostics::repair_at(&mut item, &format!("{path}/{key}"));
    match key_warnings.as_mut() {
        Some(out) => out.extend(duplicates.into_iter().map(|d| d.message)),
        None => crate::input::key_diagnostics::warn_once(&duplicates),
    }
    item
}

impl ItemContainer {
    fn new(source: Rc<dyn crate::VirtualItems>) -> Self {
        Self {
            source,
            seen_version: None,
            desired_keys: Vec::new(),
            desired_set: HashSet::default(),
            mounted_set: HashSet::default(),
            pending: Vec::new(),
            focused_item: None,
            pinned_unlisted: None,
            staged: None,
        }
    }

    /// Resolves which currently mounted item key holds focus (or a focused descendant), if any.
    fn focused_mounted_key<'a>(
        &'a self,
        container_path: &str,
        focused_path: Option<&str>,
    ) -> Option<&'a str> {
        let f = focused_path?;
        let rest = f.strip_prefix(container_path)?.strip_prefix('/')?;
        if let Some(fk) = self.focused_item.as_deref() {
            let item_path = node_path(container_path, Some(fk), 0);
            if path_is_inside_item(f, &item_path) && self.mounted_set.contains(fk) {
                return Some(fk);
            }
        }
        let seg = rest.split('/').next().unwrap_or(rest);
        let decoded = decode_key_segment(seg)?;
        self.mounted_set.get(decoded.as_ref()).map(String::as_str)
    }

    /// True if an item was kept mounted solely because it held focus, and focus has now left it.
    fn stale_pinned(&self, container_path: &str, focused_path: Option<&str>) -> bool {
        let Some(fk) = self.pinned_unlisted.as_deref() else {
            return false;
        };
        let Some(f) = focused_path else {
            return true;
        };
        let item_path = node_path(container_path, Some(fk), 0);
        !path_is_inside_item(f, &item_path)
    }

    #[inline]
    pub(crate) fn version_changed(&self) -> bool {
        self.seen_version != Some(self.source.version())
    }

    #[inline]
    pub(crate) fn wants_window(&self, path: &str, focused_path: Option<&str>) -> bool {
        self.version_changed() || !self.pending.is_empty() || self.stale_pinned(path, focused_path)
    }

    #[inline]
    pub(crate) fn unfilled(&self) -> usize {
        self.pending.len()
    }

    /// Computes the next mounted window into `self.staged` without mutating committed fields,
    /// recording leaving item paths in `virtual_culled`.
    fn stage_window(
        &mut self,
        path: &str,
        force_refresh: bool,
        force_all: bool,
        focused_path: Option<&str>,
        key_warnings: &mut Option<Vec<String>>,
        virtual_culled: &mut HashSet<String>,
    ) -> &StagedWindow {
        let ver = self.source.version();
        let need_keys = force_refresh || self.seen_version != Some(ver);
        let (desired_keys, desired_set, mut pending) = if need_keys {
            let (keys, set) = dedupe_keys(self.source.keys(), path, key_warnings);
            let mut entering: Vec<String> = keys
                .iter()
                .filter(|k| !self.mounted_set.contains(k.as_str()))
                .cloned()
                .collect();
            if self.source.mount_budget().is_some() && entering.len() > 1 {
                let mut scored: Vec<(f32, usize, String)> = entering
                    .into_iter()
                    .enumerate()
                    .map(|(idx, k)| (self.source.mount_priority(&k), idx, k))
                    .collect();
                scored.sort_by(|a, b| priority_cmp(a.0, b.0).then_with(|| a.1.cmp(&b.1)));
                entering = scored.into_iter().map(|(_, _, k)| k).collect();
            }
            (keys, set, entering)
        } else {
            let pending = self
                .pending
                .iter()
                .filter(|k| self.desired_set.contains(k.as_str()) && !self.mounted_set.contains(k.as_str()))
                .cloned()
                .collect();
            (self.desired_keys.clone(), self.desired_set.clone(), pending)
        };

        let pinned_focus = self
            .focused_mounted_key(path, focused_path)
            .map(str::to_owned);
        let pinned_unlisted = pinned_focus
            .as_ref()
            .filter(|fk| !desired_set.contains(fk.as_str()))
            .cloned();

        for mounted_key in &self.mounted_set {
            if !desired_set.contains(mounted_key.as_str())
                && pinned_unlisted.as_deref() != Some(mounted_key.as_str())
            {
                virtual_culled.insert(node_path(path, Some(mounted_key), 0));
            }
        }

        let budget = if force_all {
            usize::MAX
        } else {
            self.source.mount_budget().map_or(usize::MAX, |n| n.max(1))
        };
        let take_n = pending.len().min(budget);
        let next_pending = pending.split_off(take_n);
        let entering: HashSet<String> = pending.into_iter().collect();

        let mut next_mounted = Vec::with_capacity(
            self.mounted_set.len() + entering.len() + usize::from(pinned_unlisted.is_some()),
        );
        for key in &desired_keys {
            if self.mounted_set.contains(key.as_str()) || entering.contains(key.as_str()) {
                next_mounted.push(key.clone());
            }
        }
        if let Some(fk) = &pinned_unlisted {
            next_mounted.push(fk.clone());
        }

        self.staged.insert(StagedWindow {
            seen_version: ver,
            desired_keys,
            desired_set,
            next_mounted,
            next_pending,
            pinned_unlisted,
        })
    }

    fn commit_staged(&mut self) {
        let Some(s) = self.staged.take() else {
            return;
        };
        self.seen_version = Some(s.seen_version);
        self.desired_keys = s.desired_keys;
        self.desired_set = s.desired_set;
        self.mounted_set = s.next_mounted.into_iter().collect();
        self.pending = s.next_pending;
        self.pinned_unlisted = s.pinned_unlisted;
    }
}

impl VirtualItemsState {
    #[inline]
    pub(in crate::runtime) fn rest(&mut self) {
        let cap = (self.containers.len().saturating_mul(2)).max(16);
        if self.containers.capacity() > cap {
            self.containers.shrink_to(cap);
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.containers.is_empty()
    }

    #[inline]
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&String, &ItemContainer)> {
        self.containers.iter()
    }

    #[inline]
    pub(crate) fn keys(&self) -> impl Iterator<Item = &String> {
        self.containers.keys()
    }

    /// Builds the mounted children during a full view rebuild (`Measurement::window_tree`).
    pub(crate) fn populate_rebuild(
        &mut self,
        path: &str,
        source: Rc<dyn crate::VirtualItems>,
        force_all: bool,
        focused_path: Option<&str>,
        key_warnings: &mut Option<Vec<String>>,
        virtual_culled: &mut HashSet<String>,
    ) -> Vec<El> {
        let mut force_refresh = false;
        let entry = match self.containers.get_mut(path) {
            Some(c) => {
                if !Rc::ptr_eq(&c.source, &source) {
                    c.source = source;
                    force_refresh = true;
                }
                c
            }
            None => {
                force_refresh = true;
                self.containers.entry(path.to_owned()).or_insert_with(|| ItemContainer::new(source))
            }
        };
        let source = entry.source.clone();
        let staged = entry.stage_window(path, force_refresh, force_all, focused_path, key_warnings, virtual_culled);
        staged
            .next_mounted
            .iter()
            .map(|key| build_item_el(&*source, path, key, key_warnings))
            .collect()
    }

    /// Builds remount plans for every live `virtual_items` container (used by `remount_virtuals`).
    pub(in crate::runtime) fn plan_remounts(
        &mut self,
        nodes: &[Node],
        path_ids: &HashMap<String, rune_motion::NodeId>,
        node_index: &crate::runtime::NodeIndex,
        focused_path: Option<&str>,
        key_warnings: &mut Option<Vec<String>>,
        virtual_culled: &mut HashSet<String>,
    ) -> Vec<ItemsPlan> {
        let mut plans = Vec::with_capacity(self.containers.len());
        for (path, entry) in self.containers.iter_mut() {
            let container_id = path_ids[path];
            let old_idx = node_index[&container_id];
            let mut subtree_end = old_idx + 1;
            while subtree_end < nodes.len()
                && nodes[subtree_end].parent.is_some_and(|p| p >= old_idx)
            {
                subtree_end += 1;
            }
            let mut mounted_items = HashMap::default();
            let mut cur = old_idx + 1;
            while cur < subtree_end {
                debug_assert_eq!(nodes[cur].parent, Some(old_idx));
                let mut item_end = cur + 1;
                while item_end < subtree_end
                    && nodes[item_end].parent.is_some_and(|p| p >= cur)
                {
                    item_end += 1;
                }
                if let Some(k) = &nodes[cur].key {
                    mounted_items.insert(k.clone(), (cur, item_end));
                }
                cur = item_end;
            }

            let source = entry.source.clone();
            let staged = entry.stage_window(path, false, false, focused_path, key_warnings, virtual_culled);
            plans.push(ItemsPlan {
                path: path.clone(),
                old_idx,
                subtree_end,
                mounted_items,
                next_mounted: staged.next_mounted.clone(),
                source,
                built_items: Vec::new(),
            });
        }
        plans
    }

    /// Commits staged window state for all containers after `apply` succeeds.
    pub(crate) fn commit_staged(&mut self) {
        for entry in self.containers.values_mut() {
            entry.commit_staged();
        }
    }

    /// Drops any unapplied staged window state (e.g., on retry or fallback).
    pub(crate) fn clear_staged(&mut self) {
        for entry in self.containers.values_mut() {
            entry.staged = None;
        }
    }

    /// Retains only containers whose path satisfies `keep`.
    pub(crate) fn retain_matching(&mut self, mut keep: impl FnMut(&str) -> bool) {
        self.containers.retain(|path, _| keep(path));
    }

    /// Updates each container's `focused_item` from the committed node tree and focus path.
    pub(crate) fn track_focused(&mut self, nodes: &[Node], focused_path: Option<&str>) {
        if self.containers.is_empty() {
            return;
        }
        let focused_idx = focused_path.and_then(|f| nodes.iter().position(|n| n.path == f));
        for (path, entry) in self.containers.iter_mut() {
            let Some(rest) = focused_path
                .and_then(|f| f.strip_prefix(path.as_str()))
                .and_then(|r| r.strip_prefix('/'))
            else {
                entry.focused_item = None;
                continue;
            };
            if let Some(mut i) = focused_idx {
                let mut found = None;
                while let Some(p) = nodes[i].parent {
                    if nodes[p].path == *path {
                        found = nodes[i].key.clone();
                        break;
                    }
                    i = p;
                }
                entry.focused_item = found;
            } else {
                let seg = rest.split('/').next().unwrap_or(rest);
                let decoded = decode_key_segment(seg);
                entry.focused_item = decoded
                    .filter(|k| entry.mounted_set.contains(k.as_ref()))
                    .map(std::borrow::Cow::into_owned);
            }
        }
    }

    /// Steps the fill state machine at the end of a frame; returns `(draw, wake_due)`.
    pub(crate) fn step_fill(
        &mut self,
        now: f64,
        wanted: bool,
        missing: usize,
        version_changed: bool,
    ) -> (bool, Option<f64>) {
        if !wanted {
            self.fill.step(FillEvent::Frame { now, wanted: false, missing: 0, version_changed: false });
            return (false, None);
        }
        if let Some(attempt) = self.fill.wake(now) {
            let draw = !self.fill.step(FillEvent::Due { attempt, now }).is_empty();
            return (draw, None);
        }
        let mut draw = false;
        let mut wake_due = None;
        for effect in self.fill.step(FillEvent::Frame { now, wanted, missing, version_changed }) {
            match effect {
                FillEffect::Draw => draw = true,
                FillEffect::WakeAt(due) => wake_due = Some(due),
            }
        }
        (draw, wake_due)
    }

    pub(crate) fn reset_fill(&mut self) {
        self.fill.step(FillEvent::Reset);
    }

    #[inline]
    pub(crate) fn fill_resuming(&self, now: f64) -> bool {
        self.fill.resuming(now)
    }
}

#[cfg(test)]
mod tests;
