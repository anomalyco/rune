//! The modal focus machine: the open modals, innermost last, each holding where focus returns
//! when it closes, as a web dialog stack does (`dialog-machine.ts` in OpenCode's UI: an open
//! entry's `focus` is where focus was before it opened, and only the top layer traps focus and
//! hands it back). `step` decides; `Runtime::sync_modals` feeds it what the commit shows and
//! carries out its effects. A menu or a select's listbox the pointer opened hands focus back
//! without a ring, as Desktop's menu machine does (`ui/src/navigation/menu/machine.ts`,
//! `selectMenuRestoreTriggerWithoutRing`): keys used inside it don't make its trigger ring.
use super::*;

/// Modal roles that close as Desktop's menus do: a menu, and a select's listbox.
const MENU_ROLES: [&str; 2] = ["menu", "listbox"];

/// One open modal: its element, where focus was before it opened, and how it opened.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ModalLayer {
    path: String,
    restore: Option<String>,
    opened: Opened,
}

/// What opened a modal, for how focus shows as it hands focus back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Opened {
    /// A dialog, or a menu the keyboard opened: focus returns showing as the keys left it.
    Dialog,
    /// A menu or listbox the pointer opened: focus returns without a ring.
    PointerMenu,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum ModalFocus {
    #[default]
    Closed,
    /// Never empty: the last layer is the top one, which holds the trap.
    Open(Vec<ModalLayer>),
}

pub(crate) enum ModalEvent {
    /// A modal mounted; `restore` is the focus it hands back on closing.
    Opened { path: String, restore: Option<String>, opened: Opened },
    /// The top modal's element is gone. `kept` is focus the closing change already put outside
    /// it (a field the handler focused), which stays.
    TopRemoved { kept: bool },
    /// The top `removed` modal(s) unmounted in the same commit that `path` mounted in their
    /// place: `restore` carries the outermost removed layer's focus return forward without
    /// emitting an intermediate `Return`.
    Replaced { removed: usize, path: String, restore: Option<String>, opened: Opened },
    /// Focus is outside the top modal (nothing focused, or behind it).
    FocusOutside,
}

#[derive(Debug, PartialEq)]
pub(crate) enum ModalEffect {
    /// Drop gestures captured before the modal opened.
    CancelGestures,
    /// Focus what the top modal opens on (`FocusOutside`, which a new modal is in until a field
    /// inside it asks for focus): its `autofocus` control, else its field, else itself.
    FocusInitial,
    /// Focus returns where it was before the closed modal opened; `ring` false keeps it from
    /// showing whatever the keys did.
    Return { restore: Option<String>, ring: bool },
}

impl ModalFocus {
    pub(crate) fn top(&self) -> Option<&str> {
        match self {
            Self::Closed => None,
            Self::Open(layers) => layers.last().map(|l| l.path.as_str()),
        }
    }

    pub(crate) fn holds(&self, path: &str) -> bool {
        matches!(self, Self::Open(layers) if layers.iter().any(|l| l.path == path))
    }

    /// How many top layers satisfy `gone`, together with the outermost of those layers.
    pub(crate) fn removed_top(&self, mut gone: impl FnMut(&str) -> bool) -> (usize, Option<&ModalLayer>) {
        let Self::Open(layers) = self else { return (0, None) };
        let count = layers.iter().rev().take_while(|l| gone(&l.path)).count();
        let outermost = (count > 0).then(|| &layers[layers.len() - count]);
        (count, outermost)
    }

    pub(crate) fn step(self, event: ModalEvent) -> (Self, Vec<ModalEffect>) {
        match (self, event) {
            (state, ModalEvent::Opened { path, .. }) if state.holds(&path) => (state, vec![]),
            (Self::Closed, ModalEvent::Opened { path, restore, opened }) => {
                (Self::Open(vec![ModalLayer { path, restore, opened }]), vec![ModalEffect::CancelGestures])
            }
            (Self::Open(mut layers), ModalEvent::Opened { path, restore, opened }) => {
                layers.push(ModalLayer { path, restore, opened });
                (Self::Open(layers), vec![ModalEffect::CancelGestures])
            }
            (Self::Open(mut layers), ModalEvent::Replaced { removed, path, restore, opened }) => {
                let keep = layers.len().saturating_sub(removed);
                layers.truncate(keep);
                if !layers.iter().any(|l| l.path == path) {
                    layers.push(ModalLayer { path, restore, opened });
                }
                (Self::Open(layers), vec![ModalEffect::CancelGestures])
            }
            (Self::Open(mut layers), ModalEvent::TopRemoved { kept }) => {
                let closed = layers.pop();
                let state = if layers.is_empty() { Self::Closed } else { Self::Open(layers) };
                let effects = closed.filter(|_| !kept)
                    .map(|l| ModalEffect::Return { restore: l.restore, ring: l.opened == Opened::Dialog })
                    .into_iter().collect();
                (state, effects)
            }
            (state @ Self::Open(_), ModalEvent::FocusOutside) => (state, vec![ModalEffect::FocusInitial]),
            (state, _) => (state, vec![]),
        }
    }
}

impl focus_machine::KeyboardState {
    #[inline]
    pub(super) fn modal_top(&self) -> Option<&str> {
        self.modal_focus.top()
    }

    #[inline]
    pub(super) fn modal_holds(&self, path: &str) -> bool {
        self.modal_focus.holds(path)
    }

    #[inline]
    pub(super) fn modal_removed_top(&self, gone: impl FnMut(&str) -> bool) -> (usize, Option<&ModalLayer>) {
        self.modal_focus.removed_top(gone)
    }

    #[inline]
    pub(super) fn step_modal(&mut self, event: ModalEvent) -> Vec<ModalEffect> {
        let (state, effects) = std::mem::take(&mut self.modal_focus).step(event);
        self.modal_focus = state;
        effects
    }
}

impl RuntimeCore {
    fn is_menu_or_listbox_modal(&self, i: usize) -> bool {
        let menu = |j: usize| MENU_ROLES.contains(&self.tree.nodes[j].role);
        menu(i) || (i + 1..self.tree.nodes.len())
            .take_while(|&j| self.tree.nodes[j].parent.is_some_and(|p| p >= i))
            .any(|j| self.tree.nodes[j].parent == Some(i) && menu(j))
    }

    /// Feeds the modal focus machine what this commit shows: modals that went (innermost first),
    /// a modal that opened, and focus that left the top modal.
    pub(in crate::runtime) fn sync_modals(&mut self, app: &mut dyn AppDriver) -> bool {
        let returned = self.kbd.take_focus_returned();
        let mut cb_dirty = false;
        // When one or more top modals unmount and a new modal mounts in the same commit,
        // `modal_scope` already belongs to the replacement modal, so running `Return` against
        // `can_focus` would reject the original opener behind it. Carry the outermost removed
        // layer's restore target forward after validating intrinsic focusability, matching
        // `dialogA.close(); dialogB.showModal()` in the HTML specification.
        if let Some(i) = self.active_modal() {
            let path = self.tree.nodes[i].path.clone();
            if !self.kbd.modal_holds(&path) {
                let (removed, outermost) = self.kbd.modal_removed_top(|p| self.node(p).is_none());
                if removed > 0 {
                    let outermost = outermost.cloned();
                    let inside = self.focused_index().is_some_and(|j| self.allowed(j));
                    let carried = outermost.as_ref().and_then(|l| l.restore.as_ref())
                        .filter(|p| self.node_index(p).is_some_and(|idx| self.can_focus_unblocked(idx)))
                        .cloned();
                    let fallback = (!inside && !returned).then(|| self.kbd.focus.owned()).flatten()
                        .filter(|p| self.node_index(p).is_some_and(|idx| self.can_focus_unblocked(idx)));
                    let restore = carried.or(fallback);
                    let is_menu = self.is_menu_or_listbox_modal(i);
                    let carried_pointer_menu = outermost.is_some_and(|l| l.opened == Opened::PointerMenu);
                    let opened = if is_menu && (!self.kbd.focus.keyboard() || carried_pointer_menu) {
                        Opened::PointerMenu
                    } else {
                        Opened::Dialog
                    };
                    cb_dirty |= self.modal_step(app, ModalEvent::Replaced { removed, path, restore, opened });
                    if !self.focused_index().is_some_and(|j| self.allowed(j)) {
                        cb_dirty |= self.modal_step(app, ModalEvent::FocusOutside);
                    }
                    return cb_dirty;
                }
            }
        }
        // Exiting visuals are already inert, so a closing modal counts as gone.
        while let Some(top) = self.kbd.modal_top().map(str::to_owned).filter(|p| self.node(p).is_none()) {
            let inside = |f: &str| f.strip_prefix(top.as_str()).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'));
            let kept = self.kbd.focus.path().is_some_and(|f| !inside(f) && self.node_index(f).is_some_and(|i| self.can_focus(i)));
            cb_dirty |= self.modal_step(app, ModalEvent::TopRemoved { kept });
        }
        let Some(i) = self.active_modal() else { return cb_dirty };
        let path = self.tree.nodes[i].path.clone();
        // A field in the modal that asked for focus took it earlier in this commit, or a closing
        // menu handed focus back to its trigger as its pick opened this modal (`Tx::return_focus`,
        // which a web menu does only after the dialog has noted the item): closing returns to
        // what had focus before.
        let inside = self.focused_index().is_some_and(|j| self.allowed(j));
        let restore = if inside || returned { self.kbd.focus_before_commit.clone() } else { self.kbd.focus.owned() };
        let is_menu = self.is_menu_or_listbox_modal(i);
        let opened = if is_menu && !self.kbd.focus.keyboard() { Opened::PointerMenu } else { Opened::Dialog };
        cb_dirty |= self.modal_step(app, ModalEvent::Opened { path, restore, opened });
        if !self.focused_index().is_some_and(|j| self.allowed(j)) {
            cb_dirty |= self.modal_step(app, ModalEvent::FocusOutside);
        }
        cb_dirty
    }

    fn modal_step(&mut self, app: &mut dyn AppDriver, event: ModalEvent) -> bool {
        let effects = self.kbd.step_modal(event);
        let mut cb_dirty = false;
        for effect in effects {
            match effect {
                ModalEffect::CancelGestures => {
                    if !self.cancel_drag() { self.drop_press(); }
                    self.pointer.clear_secondary();
                    self.pointer.hovered = None;
                }
                ModalEffect::FocusInitial => {
                    if let Some(target) = self.modal_initial_focus() {
                        self.kbd.focus_lost_dirty = false;
                        let cb = self.set_focus_notifying(app, Some(target), self.kbd.focus.keyboard()).1;
                        cb_dirty |= cb || std::mem::take(&mut self.kbd.focus_lost_dirty);
                    }
                }
                ModalEffect::Return { restore, ring } => {
                    let restore = restore.filter(|p| self.node_index(p).is_some_and(|i| self.can_focus(i)));
                    self.kbd.focus_lost_dirty = false;
                    let cb = self.set_focus_notifying(app, restore, ring && self.kbd.focus.keyboard()).1;
                    cb_dirty |= cb || std::mem::take(&mut self.kbd.focus_lost_dirty);
                }
            }
        }
        cb_dirty
    }

    /// What the top modal focuses: its `El::autofocus` control, else under `Cx::web_focus` its
    /// first text field and otherwise its first Tab stop, else the modal itself, as a web dialog
    /// focuses its content (Kobalte's `onOpenAutoFocus` in OpenCode's `Dialog`).
    fn modal_initial_focus(&self) -> Option<String> {
        let i = self.active_modal()?;
        let inside = |j: usize| j != i && self.modal_member(j);
        let autofocus = (0..self.tree.nodes.len()).find(|&j| inside(j) && self.tree.nodes[j].focus.autofocus && self.can_focus(j));
        let first = if self.commit.defaults.web_focus {
            (0..self.tree.nodes.len()).find(|&j| inside(j) && self.tree.nodes[j].input.is_some() && self.can_focus(j))
        } else {
            self.tab_order().first().copied()
        };
        Some(autofocus.or(first).map_or_else(|| self.tree.nodes[i].path.clone(), |j| self.tree.nodes[j].path.clone()))
    }
}

#[cfg(test)]
#[path = "modal_focus_tests.rs"]
mod tests;
