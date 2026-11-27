//! Web accessibility mirror action dispatch and custom action button emission.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use super::{A11yItem, RuntimeCore};
use crate::runtime::Action;

impl RuntimeCore {
    /// Appends hidden custom-action mirror buttons for node `i` under `host` when enabled and allowed.
    pub(super) fn push_web_custom_actions(
        &self,
        i: usize,
        rect: [f32; 4],
        host: Option<usize>,
        node_flow: crate::el::Flow,
        dir: Option<&'static str>,
        out: &mut Vec<A11yItem>,
        flows: &mut Vec<crate::el::Flow>,
    ) {
        if self.node_disabled(i) || !self.allowed(i) {
            return;
        }
        let n = &self.tree.nodes[i];
        let Some(actions) = n.handlers.get().and_then(|h| h.custom_a11y_actions.as_deref()) else {
            return;
        };
        for a in actions {
            let mut item = A11yItem::new("button", &n.path, [rect[0], rect[1], 1.0, 1.0]);
            item.text = a.label.clone();
            item.custom_action = Some(a.id.clone());
            item.parent = host;
            item.dir = dir;
            out.push(item);
            flows.push(node_flow);
        }
    }

    /// Dispatches assistive-tech activation of a mirrored button, inline link, or custom action.
    pub(crate) fn web_a11y_action(&mut self, event: &serde_json::Value) -> bool {
        let Some(path) = event["path"].as_str() else { return false };
        match event["kind"].as_str() {
            Some("a11y_press") => {
                let Some(i) = self.live(path) else { return false };
                if self.node_disabled(i) || !self.allowed(i) { return false; }
                if self.tree.nodes[i].handlers.on_press.is_some() {
                    self.frame.queue.push(Action::Press(path.to_owned(), crate::PointerButton::Primary, false, None));
                    true
                } else if self.tree.nodes[i].input.is_some() {
                    self.queue_a11y(path.to_owned(), crate::a11y::actions::A11yAct::Focus(None));
                    true
                } else {
                    self.queue_a11y_delegate(i, accesskit::Action::Click)
                }
            }
            Some("a11y_link") => {
                let Some(k) = event["link"].as_u64().and_then(|n| usize::try_from(n).ok()) else { return false };
                let Some(i) = self.live(path) else { return false };
                if self.node_disabled(i) || !self.allowed(i) || self.tree.nodes[i].text.as_ref().is_none_or(|t| k >= t.cold.links.len()) { return false; }
                self.frame.queue.push(Action::Link(path.to_owned(), k, crate::PointerButton::Primary));
                true
            }
            Some("a11y_custom") => {
                let Some(id) = event["id"].as_str() else { return false };
                let Some(i) = self.live(path) else { return false };
                if self.node_disabled(i) || !self.allowed(i) { return false; }
                let has = self.tree.nodes[i]
                    .handlers
                    .get()
                    .and_then(|h| h.custom_a11y_actions.as_deref())
                    .is_some_and(|list| list.iter().any(|a| a.id == id));
                if !has { return false; }
                self.queue_a11y(path.to_owned(), crate::a11y::actions::A11yAct::CustomAction(id.to_owned()));
                true
            }
            _ => false,
        }
    }
}
