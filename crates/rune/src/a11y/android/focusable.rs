//! Per-role Android TalkBack `screenReaderFocusable` and `heading` policy.
//!
//! Android TalkBack treats a `ScrollView`'s single content child as a list item and merges all
//! non-focusable text descendants into one announcement unless text stops opt into
//! `AccessibilityNodeInfo.setScreenReaderFocusable(true)` (API 28+). Marking every `Label`
//! leaf splits composite rows and cards into separate swipes; instead we follow Chromium's
//! `BrowserAccessibilityAndroid` grouping rule:
//!
//! - `ListItem`, `Article`, `Heading`, and `Paragraph` keep their plain text grouped as one
//!   swipe stop (`screenReaderFocusable = true` on the grouping node when not already actionable,
//!   `false` on plain-text descendants inside them).
//! - `Heading` nodes additionally carry `isHeading = true` (API 28+).
//! - Standalone `Label` leaves (and standalone labelled `Image` leaves) outside any grouping
//!   ancestor or leaf control are marked `screenReaderFocusable = true`, so TalkBack walks them
//!   individually instead of collapsing a scroll viewport's content.
//! - The root `Window` node is never screen-reader focusable (it is made passive on the host).
#![cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]

use accesskit::{Action, Affine, Node, NodeId, Rect, Role, TreeUpdate};

use crate::a11y::roles;
use crate::runtime::HashMap;

/// Bitflag returned to `RuneView` for `AccessibilityNodeInfo.setScreenReaderFocusable(true)`.
pub(crate) const FLAG_SCREEN_READER_FOCUSABLE: i32 = 1 << 0;
/// Bitflag returned to `RuneView` for `AccessibilityNodeInfo.setHeading(true)`.
pub(crate) const FLAG_HEADING: i32 = 1 << 1;

/// Per-node Android accessibility presentation overrides computed from an AccessKit tree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AndroidNodeProps {
    /// API 28+ `AccessibilityNodeInfo.setScreenReaderFocusable(true)`.
    pub(crate) screen_reader_focusable: bool,
    /// API 28+ `AccessibilityNodeInfo.setHeading(true)`.
    pub(crate) heading: bool,
}

impl AndroidNodeProps {
    #[inline]
    pub(crate) const fn to_flags(self) -> i32 {
        let mut flags = 0;
        if self.screen_reader_focusable {
            flags |= FLAG_SCREEN_READER_FOCUSABLE;
        }
        if self.heading {
            flags |= FLAG_HEADING;
        }
        flags
    }
}

/// Roles that group plain-text descendants into a single TalkBack swipe stop.
#[inline]
pub(crate) const fn is_grouping_role(role: Role) -> bool {
    matches!(role, Role::ListItem | Role::Article | Role::Heading | Role::Paragraph)
}

#[inline]
const fn is_text_input(role: Role) -> bool {
    matches!(
        role,
        Role::TextInput
            | Role::MultilineTextInput
            | Role::SearchInput
            | Role::EmailInput
            | Role::NumberInput
            | Role::PasswordInput
            | Role::PhoneNumberInput
            | Role::UrlInput
    )
}

#[inline]
const fn resets_grouping_scope(role: Role) -> bool {
    matches!(role, Role::Window | Role::Dialog | Role::AlertDialog | Role::ScrollView)
}

/// Whether `accesskit_android` maps `class_name` from a role that can ever carry Android focus
/// or heading overrides (`Label`, `Image`, or a grouping role mapped to `android.view.View`).
#[inline]
pub(crate) fn needs_flag_lookup(class_name: &str) -> bool {
    matches!(
        class_name,
        "android.widget.TextView" | "android.view.View" | "android.widget.ImageView"
    )
}

/// Android class name matching `accesskit_android::node::NodeWrapper::class_name`.
pub(crate) fn android_class_name(node: &Node) -> &'static str {
    match node.role() {
        Role::TextInput
        | Role::MultilineTextInput
        | Role::SearchInput
        | Role::EmailInput
        | Role::NumberInput
        | Role::PasswordInput
        | Role::PhoneNumberInput
        | Role::UrlInput => "android.widget.EditText",
        Role::Slider => "android.widget.SeekBar",
        Role::ColorWell
        | Role::ComboBox
        | Role::EditableComboBox
        | Role::DateInput
        | Role::DateTimeInput
        | Role::WeekInput
        | Role::MonthInput
        | Role::TimeInput => "android.widget.Spinner",
        Role::Button => {
            if node.toggled().is_some() {
                "android.widget.ToggleButton"
            } else {
                "android.widget.Button"
            }
        }
        Role::PdfActionableHighlight => "android.widget.Button",
        Role::CheckBox => "android.widget.CheckBox",
        Role::RadioButton => "android.widget.RadioButton",
        Role::RadioGroup => "android.widget.RadioGroup",
        Role::Switch => "android.widget.ToggleButton",
        Role::Canvas | Role::Image | Role::SvgRoot => "android.widget.ImageView",
        Role::Meter | Role::ProgressIndicator => "android.widget.ProgressBar",
        Role::TabList => "android.widget.TabWidget",
        Role::Grid | Role::Table | Role::TreeGrid => "android.widget.GridView",
        Role::DescriptionList | Role::List | Role::ListBox | Role::ScrollView => {
            "android.widget.ListView"
        }
        Role::Dialog => "android.app.Dialog",
        Role::RootWebArea => "android.webkit.WebView",
        Role::MenuItem | Role::MenuItemCheckBox | Role::MenuItemRadio => "android.view.MenuItem",
        Role::Label => "android.widget.TextView",
        _ => "android.view.View",
    }
}

/// Lookup key extracted from an `AccessibilityNodeInfo` produced by `accesskit_android`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NodeQuery<'a> {
    pub(crate) class_name: &'a str,
    pub(crate) bounds_px: Option<[i32; 4]>,
    pub(crate) content_description: Option<&'a str>,
    pub(crate) text: Option<&'a str>,
    pub(crate) child_count: usize,
    pub(crate) focusable: bool,
    pub(crate) clickable: bool,
}

#[derive(Clone, Debug)]
struct IndexedNode {
    id: NodeId,
    class_name: &'static str,
    bounds_px: Option<[i32; 4]>,
    content_description: Option<String>,
    text: Option<String>,
    child_count: usize,
    focusable: bool,
    clickable: bool,
    props: AndroidNodeProps,
}

/// Live mirror of the active AccessKit tree used to compute per-role `screenReaderFocusable` and
/// `isHeading` flags without patching `accesskit_android`.
#[derive(Default)]
pub(crate) struct AndroidTreeIndex {
    root: Option<NodeId>,
    focus: Option<NodeId>,
    nodes: HashMap<u64, Node>,
    dirty: bool,
    props_by_id: HashMap<u64, AndroidNodeProps>,
    entries: Vec<IndexedNode>,
}

impl AndroidTreeIndex {
    /// Applies an incremental or full [`TreeUpdate`] to the mirrored tree.
    pub(crate) fn apply_update(&mut self, update: &TreeUpdate) {
        if let Some(tree) = &update.tree {
            self.nodes.clear();
            self.root = Some(tree.root);
        }
        self.focus = Some(update.focus);
        for (id, node) in &update.nodes {
            self.nodes.insert(id.0, node.clone());
        }
        self.dirty = true;
    }

    /// True before the first tree update or while keyboard focus sits on the root `Window` node.
    #[inline]
    pub(crate) fn focus_is_root(&self) -> bool {
        match (self.root, self.focus) {
            (Some(root), Some(focus)) => root == focus,
            _ => true,
        }
    }

    /// Direct per-node classification lookup (primarily for host-side unit tests).
    pub(crate) fn props_for_node(&mut self, id: NodeId) -> AndroidNodeProps {
        self.ensure_indexed();
        self.props_by_id.get(&id.0).copied().unwrap_or_default()
    }

    /// Resolves `FLAG_SCREEN_READER_FOCUSABLE | FLAG_HEADING` for an `AccessibilityNodeInfo`
    /// described by `query`.
    pub(crate) fn flags_for_query(&mut self, query: &NodeQuery<'_>) -> i32 {
        if !needs_flag_lookup(query.class_name) {
            return 0;
        }
        self.ensure_indexed();
        self.lookup_props(query).to_flags()
    }

    fn lookup_props(&self, query: &NodeQuery<'_>) -> AndroidNodeProps {
        // First pass: exact match including child count and actionable flags.
        let mut exact: Option<AndroidNodeProps> = None;
        for entry in &self.entries {
            if entry.class_name != query.class_name
                || entry.focusable != query.focusable
                || entry.clickable != query.clickable
                || entry.child_count != query.child_count
                || entry.content_description.as_deref() != query.content_description
                || entry.text.as_deref() != query.text
                || !bounds_match(entry.bounds_px, query.bounds_px)
            {
                continue;
            }
            if let Some(prev) = exact {
                if prev == entry.props {
                    continue;
                }
                // Ambiguous exact collision: prefer the first (shallowest) entry's flags.
                return prev;
            }
            exact = Some(entry.props);
        }
        if let Some(props) = exact {
            return props;
        }

        // Fallback when common_filter clipped offscreen children inside a scroll container,
        // reducing child_count below the unfiltered count.
        let mut relaxed: Option<AndroidNodeProps> = None;
        for entry in &self.entries {
            if entry.class_name != query.class_name
                || entry.focusable != query.focusable
                || entry.clickable != query.clickable
                || entry.content_description.as_deref() != query.content_description
                || entry.text.as_deref() != query.text
                || !bounds_match(entry.bounds_px, query.bounds_px)
            {
                continue;
            }
            if let Some(prev) = relaxed {
                if prev != entry.props {
                    return AndroidNodeProps::default();
                }
            } else {
                relaxed = Some(entry.props);
            }
        }
        relaxed.unwrap_or_default()
    }

    fn ensure_indexed(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        self.props_by_id.clear();
        self.entries.clear();
        let Some(root) = self.root else {
            return;
        };
        let mut visited = rustc_hash::FxHashSet::default();
        self.walk(root, Affine::IDENTITY, false, true, &mut visited);
    }

    fn walk(
        &mut self,
        id: NodeId,
        parent_transform: Affine,
        in_group: bool,
        is_root: bool,
        visited: &mut rustc_hash::FxHashSet<u64>,
    ) {
        if !visited.insert(id.0) {
            return;
        }
        let Some(node) = self.nodes.get(&id.0).cloned() else {
            return;
        };
        let is_focused = self.focus == Some(id);
        if !is_root && !is_focused && node.is_hidden() {
            return;
        }

        let transform = match node.transform() {
            Some(local) => mul_affine(parent_transform, *local),
            None => parent_transform,
        };

        let role = node.role();
        if !is_root && matches!(role, Role::GenericContainer | Role::TextRun) {
            for &child in node.children() {
                self.walk(child, transform, in_group, false, visited);
            }
            return;
        }

        let filtered_children = self.filtered_children(&node);
        let focusable =
            (node.supports_action(Action::Focus) || (is_focused && !is_root)) && role != Role::ScrollView;
        let clickable = node.supports_action(Action::Click);
        let range_control =
            node.supports_action(Action::Increment) || node.supports_action(Action::Decrement);
        let actionable = focusable || clickable || range_control || is_text_input(role);

        let content_description = self.android_content_description(&node);
        let text = self.android_text(&node);
        let has_own_spoken = content_description.as_deref().is_some_and(non_blank)
            || text.as_deref().is_some_and(non_blank);

        let props = if is_root || role == Role::Window {
            AndroidNodeProps::default()
        } else if is_grouping_role(role) {
            let srf = if actionable {
                false
            } else if filtered_children.is_empty() {
                has_own_spoken
            } else if self.has_ungrouped_speaking_descendant(&filtered_children) {
                true
            } else if has_own_spoken && !self.has_grouping_descendant(&filtered_children) {
                let own = content_description
                    .as_deref()
                    .filter(|s| non_blank(s))
                    .or_else(|| text.as_deref().filter(|s| non_blank(s)))
                    .map(str::trim)
                    .unwrap_or("");
                let child_words = self.actionable_descendant_words(&filtered_children);
                !own.is_empty() && own != child_words
            } else {
                false
            };
            AndroidNodeProps {
                screen_reader_focusable: srf,
                heading: role == Role::Heading,
            }
        } else if role == Role::Label {
            AndroidNodeProps {
                screen_reader_focusable: !in_group && !actionable && has_own_spoken,
                heading: false,
            }
        } else if role == Role::Image {
            AndroidNodeProps {
                screen_reader_focusable: !in_group
                    && !actionable
                    && filtered_children.is_empty()
                    && has_own_spoken,
                heading: false,
            }
        } else {
            AndroidNodeProps::default()
        };

        self.props_by_id.insert(id.0, props);

        let class_name = android_class_name(&node);
        if !is_root && needs_flag_lookup(class_name) {
            let bounds_px = node.bounds().map(|r| transform_rect_px(transform, r));
            let android_clickable = clickable || (focusable && !is_focused && !range_control);
            self.entries.push(IndexedNode {
                id,
                class_name,
                bounds_px,
                content_description,
                text,
                child_count: filtered_children.len(),
                focusable,
                clickable: android_clickable,
                props,
            });
        }

        let child_in_group = if is_root || resets_grouping_scope(role) {
            false
        } else if is_grouping_role(role) || roles::leaf_control(role) {
            true
        } else {
            in_group
        };

        for &child in node.children() {
            self.walk(child, transform, child_in_group, false, visited);
        }
    }

    fn filtered_children(&self, node: &Node) -> Vec<NodeId> {
        let mut out = Vec::new();
        for &child in node.children() {
            self.collect_filtered(child, &mut out);
        }
        out
    }

    fn collect_filtered(&self, id: NodeId, out: &mut Vec<NodeId>) {
        let Some(node) = self.nodes.get(&id.0) else {
            return;
        };
        let is_focused = self.focus == Some(id);
        if !is_focused && node.is_hidden() {
            return;
        }
        if matches!(node.role(), Role::GenericContainer | Role::TextRun) {
            for &child in node.children() {
                self.collect_filtered(child, out);
            }
            return;
        }
        out.push(id);
    }

    fn is_node_actionable(&self, id: NodeId, node: &Node) -> bool {
        let role = node.role();
        let is_focused = self.focus == Some(id);
        let is_root = self.root == Some(id);
        let focusable =
            (node.supports_action(Action::Focus) || (is_focused && !is_root)) && role != Role::ScrollView;
        let clickable = node.supports_action(Action::Click);
        let range =
            node.supports_action(Action::Increment) || node.supports_action(Action::Decrement);
        focusable || clickable || range || is_text_input(role)
    }

    fn has_ungrouped_speaking_descendant(&self, children: &[NodeId]) -> bool {
        for &cid in children {
            let Some(child) = self.nodes.get(&cid.0) else {
                continue;
            };
            let role = child.role();
            if is_grouping_role(role)
                || resets_grouping_scope(role)
                || roles::leaf_control(role)
                || self.is_node_actionable(cid, child)
            {
                continue;
            }
            if role == Role::Label {
                if self.android_content_description(child).as_deref().is_some_and(non_blank)
                    || self.android_text(child).as_deref().is_some_and(non_blank)
                {
                    return true;
                }
                continue;
            }
            if role == Role::Image {
                let grand = self.filtered_children(child);
                if grand.is_empty()
                    && self.android_content_description(child).as_deref().is_some_and(non_blank)
                {
                    return true;
                }
                continue;
            }
            let grand = self.filtered_children(child);
            if self.has_ungrouped_speaking_descendant(&grand) {
                return true;
            }
        }
        false
    }

    fn has_grouping_descendant(&self, children: &[NodeId]) -> bool {
        for &cid in children {
            let Some(child) = self.nodes.get(&cid.0) else {
                continue;
            };
            let role = child.role();
            if is_grouping_role(role) {
                return true;
            }
            if resets_grouping_scope(role) || roles::leaf_control(role) {
                continue;
            }
            let grand = self.filtered_children(child);
            if self.has_grouping_descendant(&grand) {
                return true;
            }
        }
        false
    }

    fn actionable_descendant_words(&self, children: &[NodeId]) -> String {
        let mut words = Vec::new();
        self.collect_actionable_words(children, &mut words);
        words.join(" ")
    }

    fn collect_actionable_words(&self, children: &[NodeId], out: &mut Vec<String>) {
        for &cid in children {
            let Some(child) = self.nodes.get(&cid.0) else {
                continue;
            };
            let role = child.role();
            if is_grouping_role(role) || resets_grouping_scope(role) {
                continue;
            }
            if self.is_node_actionable(cid, child) {
                if let Some(w) = self
                    .android_content_description(child)
                    .or_else(|| self.android_text(child))
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                {
                    out.push(w);
                }
                continue;
            }
            let grand = self.filtered_children(child);
            self.collect_actionable_words(&grand, out);
        }
    }

    fn node_label(&self, node: &Node) -> Option<String> {
        if let Some(label) = node.label() {
            return Some(label.to_owned());
        }
        let mut out = String::new();
        for &ref_id in node.labelled_by() {
            let Some(other) = self.nodes.get(&ref_id.0) else {
                continue;
            };
            let piece = if other.role() == Role::Label {
                other.value()
            } else {
                other.label()
            };
            if let Some(p) = piece {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(p);
            }
        }
        (!out.is_empty()).then_some(out)
    }

    fn text_runs_text(&self, node: &Node) -> Option<String> {
        let mut out = String::new();
        let mut any = false;
        for &cid in node.children() {
            if let Some(child) = self.nodes.get(&cid.0)
                && child.role() == Role::TextRun
            {
                any = true;
                if let Some(v) = child.value() {
                    out.push_str(v);
                }
            }
        }
        any.then_some(out)
    }

    fn android_content_description(&self, node: &Node) -> Option<String> {
        if node.role() == Role::Label {
            node.value().map(str::to_owned).or_else(|| self.text_runs_text(node))
        } else {
            self.node_label(node)
        }
    }

    fn android_text(&self, node: &Node) -> Option<String> {
        if node.role() != Role::Label
            && let Some(value) = node.value()
        {
            return Some(value.to_owned());
        }
        self.text_runs_text(node)
    }

    #[cfg(test)]
    pub(crate) fn indexed_bounds(&mut self, id: NodeId) -> Option<[i32; 4]> {
        self.ensure_indexed();
        self.entries.iter().find(|e| e.id == id).and_then(|e| e.bounds_px)
    }
}

/// Classifies every node in a full [`TreeUpdate`] for host tests and diagnostics.
#[cfg(test)]
pub(crate) fn classify_tree(update: &TreeUpdate) -> HashMap<u64, AndroidNodeProps> {
    let mut index = AndroidTreeIndex::default();
    index.apply_update(update);
    index.ensure_indexed();
    index.props_by_id
}

#[inline]
fn non_blank(s: &str) -> bool {
    !s.trim().is_empty()
}

#[inline]
fn bounds_match(entry: Option<[i32; 4]>, query: Option<[i32; 4]>) -> bool {
    match (entry, query) {
        (Some(a), Some(b)) => a == b,
        (None, None) => true,
        // AccessibilityNodeInfo reports an empty (0,0,0,0) rect when bounds were never set.
        (Some([0, 0, 0, 0]), None) => true,
        _ => false,
    }
}

#[inline]
fn mul_affine(a: Affine, b: Affine) -> Affine {
    let a = a.as_coeffs();
    let b = b.as_coeffs();
    Affine::new([
        a[0] * b[0] + a[2] * b[1],
        a[1] * b[0] + a[3] * b[1],
        a[0] * b[2] + a[2] * b[3],
        a[1] * b[2] + a[3] * b[3],
        a[0] * b[4] + a[2] * b[5] + a[4],
        a[1] * b[4] + a[3] * b[5] + a[5],
    ])
}

#[inline]
fn transform_rect_px(t: Affine, r: Rect) -> [i32; 4] {
    let c = t.as_coeffs();
    let xform = |x: f64, y: f64| (c[0] * x + c[2] * y + c[4], c[1] * x + c[3] * y + c[5]);
    let p0 = xform(r.x0, r.y0);
    let p1 = xform(r.x1, r.y0);
    let p2 = xform(r.x0, r.y1);
    let p3 = xform(r.x1, r.y1);
    let x0 = p0.0.min(p1.0).min(p2.0).min(p3.0);
    let y0 = p0.1.min(p1.1).min(p2.1).min(p3.1);
    let x1 = p0.0.max(p1.0).max(p2.0).max(p3.0);
    let y1 = p0.1.max(p1.1).max(p2.1).max(p3.1);
    [x0 as i32, y0 as i32, x1 as i32, y1 as i32]
}
