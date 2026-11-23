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
