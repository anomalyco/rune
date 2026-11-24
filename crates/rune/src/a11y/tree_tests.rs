//! Live-region suppression across modal open/close cycles in `A11yTree`.

use accesskit::{Live, Node, NodeId, Role, TreeUpdate};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{AccessibilityLink, Cx, El, Env, LiveRegion, Runtime, col, row, text};

/// Minimal mirror of AccessKit's consumer tree and platform live-region announcement rules
/// (`accesskit_macos` `event.rs:236-241,299-309`, `accesskit_windows` `adapter.rs:256-263,313-325`,
/// `accesskit_atspi_common` `adapter.rs:72-78`): tracks active nodes, inherited `live()`, and
/// every announcement fired on `node_added` or `node_updated`.
#[derive(Default)]
struct AdapterMirror {
    root: Option<NodeId>,
    nodes: FxHashMap<NodeId, Node>,
    parent: FxHashMap<NodeId, NodeId>,
}

impl AdapterMirror {
    fn init(&mut self, update: TreeUpdate) {
        self.root = Some(update.tree.expect("initial update carries TreeInfo").root);
        self.nodes.clear();
        self.parent.clear();
        for (id, node) in update.nodes {
            for &child in node.children() {
                self.parent.insert(child, id);
            }
            self.nodes.insert(id, node);
        }
        self.prune_unreachable();
    }

    fn apply(&mut self, update: TreeUpdate) -> Vec<(String, Live)> {
        let mut old_state: FxHashMap<NodeId, (Option<String>, Live)> = FxHashMap::default();
        for (id, _) in &update.nodes {
            if self.nodes.contains_key(id) {
                old_state.insert(*id, (self.spoken_label(*id), self.effective_live(*id)));
            }
        }
        let mut added = Vec::new();
        let mut updated = Vec::new();
        for (id, node) in update.nodes {
            for &child in node.children() {
                self.parent.insert(child, id);
            }
            match self.nodes.get(&id) {
                Some(old) if old == &node => {}
                Some(_) => {
                    self.nodes.insert(id, node);
                    updated.push(id);
                }
                None => {
                    self.nodes.insert(id, node);
                    added.push(id);
                }
            }
        }
        self.prune_unreachable();
        let mut announced = Vec::new();
        for id in added {
            if !self.nodes.contains_key(&id) { continue; }
            let live = self.effective_live(id);
            if live != Live::Off && let Some(label) = self.spoken_label(id) {
                announced.push((label, live));
            }
        }
        for id in updated {
            if !self.nodes.contains_key(&id) { continue; }
            let live = self.effective_live(id);
            let label = self.spoken_label(id);
            if live != Live::Off && let Some(text) = label {
                let (old_label, old_live) = old_state.get(&id).cloned().unwrap_or((None, Live::Off));
                if old_label.as_deref() != Some(text.as_str()) || live != old_live {
                    announced.push((text, live));
                }
            }
        }
        announced
    }

    fn prune_unreachable(&mut self) {
        let Some(root) = self.root else { return };
        let mut reachable = FxHashSet::default();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            if !reachable.insert(id) { continue; }
            if let Some(node) = self.nodes.get(&id) {
                stack.extend_from_slice(node.children());
            }
        }
        self.nodes.retain(|id, _| reachable.contains(id));
        self.parent.retain(|id, _| reachable.contains(id));
    }

    fn effective_live(&self, mut id: NodeId) -> Live {
        loop {
            let Some(node) = self.nodes.get(&id) else { return Live::Off };
            if let Some(live) = node.live() { return live; }
            let Some(&parent) = self.parent.get(&id) else { return Live::Off };
            id = parent;
        }
    }

    fn spoken_label(&self, id: NodeId) -> Option<String> {
        let node = self.nodes.get(&id)?;
        if node.role() == Role::Label {
            node.value().map(str::to_owned)
        } else {
            node.label().map(str::to_owned)
        }
    }

    fn by_key(&self, key: &str) -> Option<(NodeId, &Node)> {
        self.nodes.iter().find(|(_, n)| n.author_id() == Some(key)).map(|(id, n)| (*id, n))
    }
}

struct StatusModal {
    open: bool,
    status: String,
    pad: f32,
}

fn status_modal_view(s: &StatusModal, _: &mut Cx<StatusModal>) -> El {
    let mut root = col().size(400.0, 400.0).padding(s.pad)
        .child(text(s.status.as_str()).key("status").role("status"))
        .child(row().key("more").role("button").label("More").size(80.0, 30.0)
            .on_press(|s: &mut StatusModal| s.open = true));
    if s.open {
        root = root.child(
            col().key("sheet").role("dialog").label("Options").size(200.0, 100.0)
                .modal(true, |s: &mut StatusModal| s.open = false)
                .child(row().key("close").role("button").label("Close").size(60.0, 30.0)
                    .on_press(|s: &mut StatusModal| s.open = false)),
        );
    }
    root
}

#[test]
fn closing_modal_does_not_reannounce_unchanged_live_region() {
    let mut rt = Runtime::new(
        StatusModal { open: false, status: "Synced".into(), pad: 0.0 },
        status_modal_view,
        (400.0, 400.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    let mut link = AccessibilityLink::new();
    link.activate();
    let mut mirror = AdapterMirror::default();
    mirror.init(link.service(&mut rt, "Window").expect("initial tree"));
    assert_eq!(mirror.by_key("status").unwrap().1.live(), Some(Live::Polite));

    // Opening the modal prunes the background status line.
    rt.update(|s, _| s.open = true);
    rt.frame(1.0 / 60.0);
    let open_heard = mirror.apply(link.service(&mut rt, "Window").expect("modal open"));
    assert!(mirror.by_key("status").is_none(), "background status is pruned behind an open modal");
    assert!(open_heard.is_empty(), "opening modal announced: {open_heard:?}");

    // Closing the modal returns the unchanged status line muted, so nothing is re-announced.
    rt.update(|s, _| s.open = false);
    rt.frame(2.0 / 60.0);
    let close_heard = mirror.apply(link.service(&mut rt, "Window").expect("modal close"));
    assert!(mirror.by_key("status").is_some(), "status returns after modal closes");
    assert_eq!(mirror.by_key("status").unwrap().1.live(), Some(Live::Off));
    assert!(close_heard.is_empty(), "unchanged status must not re-announce on modal close: {close_heard:?}");

    // Moving layout bounds while muted must not unmute or announce the status line.
    rt.update(|s, _| s.pad = 16.0);
    rt.frame(3.0 / 60.0);
    let resize_heard = mirror.apply(link.service(&mut rt, "Window").expect("bounds moved"));
    assert_eq!(mirror.by_key("status").unwrap().1.live(), Some(Live::Off));
    assert!(resize_heard.is_empty(), "layout shift must not unmute: {resize_heard:?}");

    // A second modal open/close while still muted keeps it silent.
    rt.update(|s, _| s.open = true);
    rt.frame(4.0 / 60.0);
    assert!(mirror.apply(link.service(&mut rt, "Window").expect("second modal open")).is_empty());
    rt.update(|s, _| s.open = false);
    rt.frame(5.0 / 60.0);
    let second_close = mirror.apply(link.service(&mut rt, "Window").expect("second modal close"));
    assert!(second_close.is_empty(), "second modal close must stay silent: {second_close:?}");

    // Changing the status text restores its live setting and announces the new text once.
    rt.update(|s, _| s.status = "Saving...".into());
    rt.frame(6.0 / 60.0);
    let change_heard = mirror.apply(link.service(&mut rt, "Window").expect("status changed"));
    assert_eq!(mirror.by_key("status").unwrap().1.live(), Some(Live::Polite));
    assert_eq!(change_heard, vec![("Saving...".into(), Live::Polite)]);
}

#[test]
fn closing_modal_announces_live_region_whose_text_changed_while_modal_open() {
    let mut rt = Runtime::new(
        StatusModal { open: false, status: "Synced".into(), pad: 0.0 },
        status_modal_view,
        (400.0, 400.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    let mut link = AccessibilityLink::new();
    link.activate();
    let mut mirror = AdapterMirror::default();
    mirror.init(link.service(&mut rt, "Window").expect("initial tree"));

    rt.update(|s, _| s.open = true);
    rt.frame(1.0 / 60.0);
    assert!(mirror.apply(link.service(&mut rt, "Window").expect("modal open")).is_empty());

    // Text changes while behind the modal.
    rt.update(|s, _| s.status = "Synced 2 files".into());
    rt.frame(2.0 / 60.0);
    if let Some(u) = link.service(&mut rt, "Window") {
        assert!(mirror.apply(u).is_empty());
    }

    // Closing the modal reveals the new text and announces it once.
    rt.update(|s, _| s.open = false);
    rt.frame(3.0 / 60.0);
    let close_heard = mirror.apply(link.service(&mut rt, "Window").expect("modal close"));
    assert_eq!(mirror.by_key("status").unwrap().1.live(), Some(Live::Polite));
    assert_eq!(close_heard, vec![("Synced 2 files".into(), Live::Polite)]);
}

#[test]
fn closing_modal_handles_container_live_regions_and_stacked_modals() {
    struct State {
        modal1: bool,
        modal2: bool,
        lines: Vec<String>,
        banner: String,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut root = col().size(400.0, 400.0)
            .child(
                col().key("log").role("log").size(300.0, 100.0)
                    .children(s.lines.iter().enumerate().map(|(i, l)| text(l.as_str()).key(format!("l{i}")))),
            )
            .child(
                row().key("banner").role("status").size(300.0, 30.0)
                    .child(text(s.banner.as_str()).key("words"))
                    .child(text("2m ago").key("when").live_region(LiveRegion::Off)),
            );
        if s.modal1 {
            let mut m1 = col().key("m1").role("dialog").label("First").size(200.0, 120.0)
                .modal(true, |s: &mut State| s.modal1 = false)
                .child(text("Step 1").key("m1-status").role("status"));
            if s.modal2 {
                m1 = m1.child(
                    col().key("m2").role("dialog").label("Second").size(150.0, 80.0)
                        .modal(true, |s: &mut State| s.modal2 = false)
                        .child(text("Confirm").key("m2-text")),
                );
            }
            root = root.child(m1);
        }
        root
    }

    let mut rt = Runtime::new(
        State { modal1: false, modal2: false, lines: vec!["Line 1".into()], banner: "Online".into() },
        view,
        (400.0, 400.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    let mut link = AccessibilityLink::new();
    link.activate();
    let mut mirror = AdapterMirror::default();
    mirror.init(link.service(&mut rt, "Window").expect("initial tree"));

    // Open modal 1: background pruned; modal 1's own status is new so it announces.
    rt.update(|s, _| s.modal1 = true);
    rt.frame(1.0 / 60.0);
    let m1_open = mirror.apply(link.service(&mut rt, "Window").expect("m1 open"));
    assert_eq!(m1_open, vec![("Step 1".into(), Live::Polite)]);

    // Open modal 2 on top of modal 1: m1-status is now pruned too.
    rt.update(|s, _| s.modal2 = true);
    rt.frame(2.0 / 60.0);
    assert!(mirror.apply(link.service(&mut rt, "Window").expect("m2 open")).is_empty());

    // Close modal 2: m1-status returns unchanged and stays silent; background stays pruned.
    rt.update(|s, _| s.modal2 = false);
    rt.frame(3.0 / 60.0);
    let m2_close = mirror.apply(link.service(&mut rt, "Window").expect("m2 close"));
    assert!(m2_close.is_empty(), "m1-status must not re-announce when m2 closes: {m2_close:?}");
    assert!(mirror.by_key("log").is_none());

    // Close modal 1: log, banner and their text children return unchanged and stay silent.
    rt.update(|s, _| s.modal1 = false);
    rt.frame(4.0 / 60.0);
    let m1_close = mirror.apply(link.service(&mut rt, "Window").expect("m1 close"));
    assert!(m1_close.is_empty(), "background containers must not re-announce: {m1_close:?}");

    // Appending a new line to the log unmutes the log and announces only the new line.
    rt.update(|s, _| s.lines.push("Line 2".into()));
    rt.frame(5.0 / 60.0);
    let log_heard = mirror.apply(link.service(&mut rt, "Window").expect("new log line"));
    assert_eq!(log_heard, vec![("Line 2".into(), Live::Polite)]);

    // Updating the banner unmutes it while keeping its explicit LiveRegion::Off child off.
    rt.update(|s, _| s.banner = "Offline".into());
    rt.frame(6.0 / 60.0);
    let banner_heard = mirror.apply(link.service(&mut rt, "Window").expect("banner changed"));
    assert!(banner_heard.iter().any(|(t, l)| t.contains("Offline") && *l == Live::Polite), "{banner_heard:?}");
    assert_eq!(mirror.by_key("when").unwrap().1.live(), Some(Live::Off));
}

#[test]
fn unmounting_and_remounting_live_region_without_modal_still_announces() {
    fn view(shown: &bool, _: &mut Cx<bool>) -> El {
        let mut root = col().size(300.0, 200.0);
        if *shown {
            root = root.child(text("Saved").key("toast").role("status"));
        }
        root
    }
    let mut rt = Runtime::new(true, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let mut link = AccessibilityLink::new();
    link.activate();
    let mut mirror = AdapterMirror::default();
    mirror.init(link.service(&mut rt, "Window").expect("initial tree"));

    rt.update(|s, _| *s = false);
    rt.frame(1.0 / 60.0);
    assert!(mirror.apply(link.service(&mut rt, "Window").expect("toast gone")).is_empty());

    rt.update(|s, _| *s = true);
    rt.frame(2.0 / 60.0);
    let heard = mirror.apply(link.service(&mut rt, "Window").expect("fresh toast"));
    assert_eq!(heard, vec![("Saved".into(), Live::Polite)]);
}

#[test]
fn activating_while_modal_open_does_not_announce_unchanged_background_on_close() {
    let mut rt = Runtime::new(
        StatusModal { open: true, status: "Synced".into(), pad: 0.0 },
        status_modal_view,
        (400.0, 400.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    let mut link = AccessibilityLink::new();
    link.activate();
    let mut mirror = AdapterMirror::default();
    mirror.init(link.service(&mut rt, "Window").expect("initial tree while modal open"));
    assert!(mirror.by_key("status").is_none());

    rt.update(|s, _| s.open = false);
    rt.frame(1.0 / 60.0);
    let heard = mirror.apply(link.service(&mut rt, "Window").expect("modal close"));
    assert_eq!(mirror.by_key("status").unwrap().1.live(), Some(Live::Off));
    assert!(heard.is_empty(), "unchanged status must stay quiet on close after mid-modal activation: {heard:?}");
}

fn find_json<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    if v["key"] == key { return Some(v); }
    v["children"].as_array()?.iter().find_map(|c| find_json(c, key))
}

#[test]
fn virtual_list_headings_take_no_position_and_options_set_list_size() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().size(300.0, 600.0).scroll(crate::ScrollSpec { stick_bottom: false }).child(
            col().key("list").role("listbox").label("Files").growth(crate::GrowthSpec::default())
                .virtual_rows(20, 24.0, |_: &(), i| format!("r{i}").into(), |_: &(), i| {
                    if i == 0 || i == 11 {
                        text(format!("Group {i}")).h(24.0).role("heading")
                    } else {
                        let pos = if i < 11 { i } else { i - 1 };
                        text(format!("Item {i}")).h(24.0).role("option").position_in_set(pos, 18)
                    }
                }),
        )
    }
    let mut rt = Runtime::new((), view, (300.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(0.016);
    let tree = rt.accessibility_json();
    let list = find_json(&tree, "list").unwrap();
    assert_eq!(list["setSize"], 18, "explicit option set size excludes headings");
    let h0 = find_json(&tree, "r0").unwrap();
    assert_eq!(h0["role"], "heading");
    assert!(h0["position"].is_null(), "heading in virtual_rows must take no position: {h0}");
    let h11 = find_json(&tree, "r11").unwrap();
    assert_eq!(h11["role"], "heading");
    assert!(h11["position"].is_null(), "second heading in virtual_rows must take no position: {h11}");
    let opt1 = find_json(&tree, "r1").unwrap();
    assert_eq!(opt1["position"], 1);
    let opt12 = find_json(&tree, "r12").unwrap();
    assert_eq!(opt12["position"], 11);
}

#[test]
fn item_set_size_never_overwrites_non_set_row_or_shallower_tree_level() {
    fn grid_view(_: &(), _: &mut Cx<()>) -> El {
        col().size(600.0, 300.0).scroll(crate::ScrollSpec { stick_bottom: false }).child(
            col().key("grid").role("grid").label("Icons").growth(crate::GrowthSpec::default())
                .virtual_rows(5, 30.0, |_: &(), i| format!("line{i}").into(), |_: &(), i| {
                    row().h(30.0).role("row").children((0..4).map(move |c| {
                        text(format!("tile {}", i * 4 + c))
                            .key(format!("c{c}"))
                            .role("gridcell")
                            .position_in_set(i * 4 + c + 1, 20)
                    }))
                }),
        )
    }
    let mut rt = Runtime::new((), grid_view, (600.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(0.016);
    let tree = rt.accessibility_json();
    let grid = find_json(&tree, "grid").unwrap();
    assert!(grid["setSize"].is_null(), "grid uses rowCount, not setSize: {grid}");
    assert_eq!(grid["rowCount"], 5);
    let line0 = find_json(&tree, "line0").unwrap();
    assert!(line0["setSize"].is_null(), "gridcell position_in_set must not overwrite row setSize: {line0}");
    assert!(line0["position"].is_null(), "row in plain grid uses rowIndex, not position: {line0}");
    assert_eq!(line0["rowIndex"], 1);

    // A flat tree whose mounted slice ends on level-2 items keeps level 1's set size on the tree.
    fn tree_view(_: &(), _: &mut Cx<()>) -> El {
        const ROWS: [(&str, u32, usize, usize); 3] = [("fruits", 1, 1, 2), ("apple", 2, 1, 3), ("banana", 2, 2, 3)];
        col().key("tree").role("tree").label("Food").children(ROWS.iter().map(|&(key, level, pos, size)| {
            crate::el().key(key).role("treeitem").label(key).focusable(true).level(level).position_in_set(pos, size)
                .size(140.0, 20.0).child(text(key))
        }))
    }
    let mut rt = Runtime::new((), tree_view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let tree = rt.accessibility_json();
    assert_eq!(find_json(&tree, "tree").unwrap()["setSize"], 2, "level-2 items must not overwrite level-1 setSize");
}

#[test]
fn grid_counts_and_cell_indices_reach_accesskit_json_and_incremental_updates() {
    struct GridState {
        rows: usize,
        cols: usize,
        row_idx: usize,
        col_idx: usize,
    }
    fn view(s: &GridState, _: &mut Cx<GridState>) -> El {
        col().size(400.0, 200.0).child(
            col().key("sheet").role("grid").label("Files")
                .row_count(s.rows)
                .column_count(s.cols)
                .child(
                    row().key("r50k").role("row").row_index(s.row_idx)
                        .child(text("Size").key("col-c").role("gridcell").row_index(s.row_idx).column_index(s.col_idx)),
                ),
        )
    }
    let mut rt = Runtime::new(
        GridState { rows: 100_000, cols: 5, row_idx: 50_000, col_idx: 3 },
        view,
        (400.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    let mut link = AccessibilityLink::new();
    link.activate();
    let mut mirror = AdapterMirror::default();
    mirror.init(link.service(&mut rt, "Window").expect("initial tree"));

    let (_, sheet) = mirror.by_key("sheet").unwrap();
    assert_eq!((sheet.row_count(), sheet.column_count()), (Some(100_000), Some(5)));
    let (_, row_node) = mirror.by_key("r50k").unwrap();
    assert_eq!(row_node.row_index(), Some(49_999), "AccessKit row_index is 0-based");
    let (_, cell_node) = mirror.by_key("col-c").unwrap();
    assert_eq!((cell_node.row_index(), cell_node.column_index()), (Some(49_999), Some(2)), "AccessKit indices are 0-based");

    let json = rt.accessibility_json();
    let sheet_j = find_json(&json, "sheet").unwrap();
    assert_eq!((sheet_j["rowCount"].as_u64(), sheet_j["columnCount"].as_u64()), (Some(100_000), Some(5)));
    let cell_j = find_json(&json, "col-c").unwrap();
    assert_eq!((cell_j["rowIndex"].as_u64(), cell_j["columnIndex"].as_u64()), (Some(50_000), Some(3)), "JSON indices are 1-based");

    // Mutating grid counts and indices updates the sent tree via fingerprint checks.
    rt.update(|s, _| {
        s.rows = 100_001;
        s.cols = 6;
        s.row_idx = 50_001;
        s.col_idx = 4;
    });
    rt.frame(1.0 / 60.0);
    let update = link.service(&mut rt, "Window").expect("grid attributes changed");
    mirror.apply(update);
    let (_, sheet) = mirror.by_key("sheet").unwrap();
    assert_eq!((sheet.row_count(), sheet.column_count()), (Some(100_001), Some(6)));
    let (_, cell_node) = mirror.by_key("col-c").unwrap();
    assert_eq!((cell_node.row_index(), cell_node.column_index()), (Some(50_000), Some(3)));
}

#[test]
fn flat_tree_levelled_items_carry_their_own_level_set_size_and_stay_flat() {
    struct TreeState {
        rows: Vec<(&'static str, u32, usize, usize)>,
        treegrid: bool,
    }
    fn view(s: &TreeState, _: &mut Cx<TreeState>) -> El {
        let tree = col().key("tree").role("tree").label("Food").children(
            s.rows.iter().map(|&(key, level, pos, size)| {
                crate::el().key(key).role("treeitem").label(key).focusable(true)
                    .level(level).position_in_set(pos, size)
                    .size(140.0, 20.0).child(text(key))
            }),
        );
        // Unlevelled list: items must keep size_of_set on the container only.
        let flat_list = col().key("plain-list").role("listbox").label("Colors")
            .child(crate::el().key("red").role("option").label("Red").position_in_set(1, 2).size(100.0, 20.0))
            .child(crate::el().key("blue").role("option").label("Blue").position_in_set(2, 2).size(100.0, 20.0));
        // Levelled tree without explicit position_in_set: items take no size_of_set.
        let uncounted = col().key("uncounted").role("tree").label("Outline")
            .child(crate::el().key("u1").role("treeitem").label("One").level(1).size(100.0, 20.0));
        // Levelled row whose container switches between grid and treegrid tests parent-role invalidation.
        let grid = col().key("maybe-treegrid").role(if s.treegrid { "treegrid" } else { "grid" }).label("Grid")
            .child(row().key("grow").role("row").label("Sub").level(2).position_in_set(1, 4).size(120.0, 20.0));
        col().size(300.0, 400.0).child(tree).child(flat_list).child(uncounted).child(grid)
    }

    let mut rt = Runtime::new(
        TreeState {
            rows: vec![
                ("fruits", 1, 1, 2),
                ("apple", 2, 1, 3),
                ("banana", 2, 2, 3),
                ("cherry", 2, 3, 3),
                ("vegetables", 1, 2, 2),
            ],
            treegrid: false,
        },
        view,
        (300.0, 400.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    let mut link = AccessibilityLink::new();
    link.activate();
    let mut mirror = AdapterMirror::default();
    mirror.init(link.service(&mut rt, "Window").expect("initial tree"));

    let json = rt.accessibility_json();
    let tree = find_json(&json, "tree").unwrap();
    assert_eq!(tree["setSize"], 2);
    let direct_keys: Vec<&str> = tree["children"].as_array().unwrap().iter()
        .filter_map(|c| c["key"].as_str()).collect();
    assert_eq!(direct_keys, vec!["fruits", "apple", "banana", "cherry", "vegetables"],
        "flat tree items must stay direct children of tree with no synthetic groups");
    for (key, level, pos, size) in [
        ("fruits", 1, 1, 2),
        ("apple", 2, 1, 3),
        ("banana", 2, 2, 3),
        ("cherry", 2, 3, 3),
        ("vegetables", 1, 2, 2),
    ] {
        let item = find_json(&json, key).unwrap();
        assert_eq!((item["level"].as_u64(), item["position"].as_u64(), item["setSize"].as_u64()),
            (Some(level), Some(pos), Some(size)), "{key}: {item}");
        let (_, ak) = mirror.by_key(key).unwrap();
        assert_eq!(ak.size_of_set(), Some(size as usize), "AccessKit node {key} carries its level set size");
    }

    // Unlevelled listbox and uncounted tree leave item size_of_set unset.
    assert_eq!(find_json(&json, "plain-list").unwrap()["setSize"], 2);
    assert!(find_json(&json, "red").unwrap()["setSize"].is_null());
    assert!(find_json(&json, "u1").unwrap()["setSize"].is_null());

    // Plain grid row is not an ordered-set item, so it carries no size_of_set until its container is a treegrid.
    assert!(mirror.by_key("grow").unwrap().1.size_of_set().is_none());
    rt.update(|s, _| {
        s.treegrid = true;
        s.rows[1] = ("apple", 2, 1, 5);
    });
    rt.frame(1.0 / 60.0);
    mirror.apply(link.service(&mut rt, "Window").expect("treegrid and set size changed"));
    assert_eq!(mirror.by_key("grow").unwrap().1.size_of_set(), Some(4));
    assert_eq!(mirror.by_key("apple").unwrap().1.size_of_set(), Some(5));
}

#[test]
fn live_text_updates_a11y_tree_without_commit_and_only_announces_live_regions() {
    struct State {
        clock: crate::LiveText,
        status: crate::LiveText,
        count: crate::LiveText,
        cased: crate::LiveText,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().size(400.0, 300.0)
            .child(text("00:00").key("clock").live_text(&s.clock))
            .child(
                col().key("live-box").live_region(LiveRegion::Polite).size(300.0, 30.0)
                    .child(text("idle").key("status").live_text(&s.status)),
            )
            .child(
                row().key("tab").role("tab").focusable(true).size(120.0, 30.0)
                    .on_press(|_: &mut State| {})
                    .child(text("0").key("count").live_text(&s.count)),
            )
            .child(text("abc").key("cased").text_case(crate::TextCase::Uppercase).live_text(&s.cased))
    }

    let clock = crate::LiveText::new("00:00");
    let status = crate::LiveText::new("idle");
    let count = crate::LiveText::new("0");
    let cased = crate::LiveText::new("abc");
    let mut rt = Runtime::new(
        State { clock: clock.clone(), status: status.clone(), count: count.clone(), cased: cased.clone() },
        view,
        (400.0, 300.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    let mut link = AccessibilityLink::new();
    link.activate();
    let mut mirror = AdapterMirror::default();
    mirror.init(link.service(&mut rt, "Window").expect("initial tree"));
    assert_eq!(mirror.by_key("clock").unwrap().1.value(), Some("00:00"));
    assert_eq!(mirror.by_key("status").unwrap().1.value(), Some("idle"));
    assert_eq!(mirror.by_key("tab").unwrap().1.label(), Some("0"));
    assert_eq!(mirror.by_key("cased").unwrap().1.value(), Some("abc"));

    let commits = rt.stats.commits;

    // A plain LiveText changing every frame refreshes the tree without a commit and never announces.
    for tick in 1..=5 {
        let next = format!("00:0{tick}");
        clock.set(next.as_str());
        rt.frame(tick as f64 / 60.0);
        assert_eq!(rt.stats.commits, commits, "LiveText::set must not commit");
        let update = link.service(&mut rt, "Window").expect("LiveText update must refresh a11y tree");
        assert!(!rt.core().a11y_changes().structure && !rt.core().a11y_changes().moved);
        let heard = mirror.apply(update);
        assert!(heard.is_empty(), "plain LiveText must not announce: {heard:?}");
        assert_eq!(mirror.by_key("clock").unwrap().1.value(), Some(next.as_str()));
    }

    // Idle frame and equal LiveText::set produce no update.
    rt.frame(6.0 / 60.0);
    assert!(link.service(&mut rt, "Window").is_none(), "idle frame after LiveText must return None");
    assert!(!clock.set("00:05"));
    rt.frame(7.0 / 60.0);
    assert!(link.service(&mut rt, "Window").is_none(), "equal LiveText::set must return None");

    // LiveText inside a LiveRegion::Polite container refreshes and announces once.
    status.set("working for 12s");
    rt.frame(8.0 / 60.0);
    assert_eq!(rt.stats.commits, commits, "live-region LiveText must not commit");
    let update = link.service(&mut rt, "Window").expect("live-region LiveText must emit update");
    let heard = mirror.apply(update);
    assert_eq!(heard, vec![("working for 12s".into(), Live::Polite)]);
    assert_eq!(mirror.by_key("status").unwrap().1.value(), Some("working for 12s"));
    rt.frame(9.0 / 60.0);
    assert!(link.service(&mut rt, "Window").is_none(), "idle frame after live-region update returns None");

    // A control named from a child LiveText refreshes its label without a commit.
    count.set("42");
    rt.frame(10.0 / 60.0);
    assert_eq!(rt.stats.commits, commits);
    let update = link.service(&mut rt, "Window").expect("tab label LiveText must emit update");
    assert!(mirror.apply(update).is_empty());
    assert_eq!(mirror.by_key("tab").unwrap().1.label(), Some("42"));

    // Changing only source casing under TextCase::Uppercase still updates source_text in the tree.
    cased.set("ABC");
    rt.frame(11.0 / 60.0);
    assert_eq!(rt.stats.commits, commits);
    let update = link.service(&mut rt, "Window").expect("cased LiveText must emit update");
    assert!(mirror.apply(update).is_empty());
    assert_eq!(mirror.by_key("cased").unwrap().1.value(), Some("ABC"));
}
