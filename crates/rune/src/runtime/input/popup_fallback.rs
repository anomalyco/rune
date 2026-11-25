//! Opt-in in-window pop-up menu fallback (`App::on_popup_menu`, `PopupMenuRequest`,
//! `Tx::resolve_popup_menu`) for platforms without native pop-up menus (such as Linux).
//!
//! Lifecycle is one explicit state machine (`PopupFallbackState`, `step`) sitting beside
//! `self.host.popups` (`Popups`): at most one fallback menu is open at a time, picks from an
//! in-window modal menu defer until right after the commit that unmounts the modal (#2082
//! parity), and every menu resolves exactly once (on pick, Escape/outside click, window blur,
//! anchor removal, superseding request, or window close).

use super::*;
use crate::machine::Effects;
use crate::popup_menu::{PopupAnchor, PopupMenuRequest};

/// Maximum fallback deliveries in one frame turn (guards a hook that always opens another menu).
const MAX_FALLBACK_DELIVERIES: usize = 16;

/// Lifecycle state of the in-window fallback pop-up menu delivered to `App::on_popup_menu`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum PopupFallbackState {
    /// No fallback pop-up menu is open.
    #[default]
    Idle,
    /// One fallback pop-up menu is open in the app's view, awaiting `Tx::resolve_popup_menu`.
    Open {
        /// Monotonic token distinguishing this open menu from earlier/later ones.
        token: u64,
        /// Key of the anchor element when `PopupAnchor::Key(key)` opened this menu.
        anchor_key: Option<String>,
        /// Target text-field path when this menu is a field menu (for modal-close deferral).
        field_path: Option<String>,
        /// Number of requests already queued behind this one when it opened (so a batch of
        /// menus queued together opens one after another, while a fresh request arriving
        /// later supersedes the open menu).
        queued_at_open: usize,
    },
    /// `Tx::resolve_popup_menu(Some(id))` arrived while a modal (the closing in-window menu)
    /// was still mounted: executes immediately after the commit unmounts it.
    DeferredPick {
        token: u64,
        id: String,
        field_path: Option<String>,
    },
}

impl PopupFallbackState {
    /// Whether a fallback menu is open or waiting on a post-commit pick.
    #[inline]
    pub(crate) fn is_active(&self) -> bool {
        !matches!(self, Self::Idle)
    }
}

/// Events that drive [`step`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PopupFallbackEvent {
    /// A validated menu from `Popups` is opening for `App::on_popup_menu`.
    Open {
        anchor_key: Option<String>,
        field_path: Option<String>,
        queued_behind: usize,
    },
    /// `Tx::resolve_popup_menu(picked)` arrived; `defer` is true when a pick (`Some`) arrived
    /// while an active modal is still mounted.
    Resolve {
        picked: Option<String>,
        defer: bool,
    },
    /// The frame's commit finished, unmounting any closing modal.
    PostCommit,
    /// A fresh pop-up request arrived while a fallback menu was already open.
    Supersede,
    /// Window blur, anchor element removal, or window hide/close dismissed the open menu.
    Dismiss,
}

/// Effects returned by [`step`] for the caller to carry out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PopupFallbackEffect {
    /// Deliver the newly opened [`PopupMenuRequest`] to `App::on_popup_menu`.
    Deliver,
    /// Resolve the open menu in `self.host.popups` with `picked` (or `None` on dismissal).
    Resolve(Option<String>),
}

/// Pure state transition for the in-window fallback pop-up menu lifecycle.
pub(crate) fn step(
    state: &mut PopupFallbackState,
    next_token: &mut u64,
    event: PopupFallbackEvent,
) -> Effects<PopupFallbackEffect> {
    match event {
        PopupFallbackEvent::Open { anchor_key, field_path, queued_behind } => {
            if !matches!(*state, PopupFallbackState::Idle) {
                return Effects::none();
            }
            *next_token = next_token.wrapping_add(1);
            *state = PopupFallbackState::Open {
                token: *next_token,
                anchor_key,
                field_path,
                queued_at_open: queued_behind,
            };
            Effects::one(PopupFallbackEffect::Deliver)
        }
        PopupFallbackEvent::Resolve { picked, defer } => match std::mem::take(state) {
            PopupFallbackState::Idle => Effects::none(),
            PopupFallbackState::Open { token, field_path, .. } => match (picked, defer) {
                (Some(id), true) => {
                    *state = PopupFallbackState::DeferredPick { token, id, field_path };
                    Effects::none()
                }
                (picked, _) => Effects::one(PopupFallbackEffect::Resolve(picked)),
            },
            PopupFallbackState::DeferredPick { token, id, field_path } => match picked {
                None => Effects::one(PopupFallbackEffect::Resolve(None)),
                Some(_) => {
                    *state = PopupFallbackState::DeferredPick { token, id, field_path };
                    Effects::none()
                }
            },
        },
        PopupFallbackEvent::PostCommit => match std::mem::take(state) {
            PopupFallbackState::DeferredPick { id, .. } => Effects::one(PopupFallbackEffect::Resolve(Some(id))),
            other => {
                *state = other;
                Effects::none()
            }
        },
        PopupFallbackEvent::Supersede | PopupFallbackEvent::Dismiss => match std::mem::take(state) {
            PopupFallbackState::Idle => Effects::none(),
            PopupFallbackState::Open { .. } | PopupFallbackState::DeferredPick { .. } => {
                Effects::one(PopupFallbackEffect::Resolve(None))
            }
        },
    }
}

impl RuntimeCore {
    /// Whether the opt-in in-window pop-up menu fallback (`App::on_popup_menu`) is active for
    /// this runtime: only when native pop-up menus are unavailable, NOT on `wasm32` (where
    /// `bridge.js:showMenu` renders `take_popup()`), and the app registered `on_popup_menu`.
    pub(crate) fn popup_menu_fallback_active(&self, app: &dyn AppDriver) -> bool {
        if !app.has_on_popup_menu() {
            return false;
        }
        #[cfg(test)]
        if self.host.force_popup_menu_fallback {
            return true;
        }
        !crate::popup_menu_supported() && !cfg!(target_arch = "wasm32")
    }

    /// Latches `Tx::resolve_popup_menu` during request latching.
    ///
    /// Unknown or disabled item ids are ignored and leave the menu open. Dismissals (`None`)
    /// resolve immediately; picks (`Some(id)`) defer until right after commit when an active
    /// modal (such as a closing `rune_kit::Menu`) is currently mounted.
    pub(in crate::runtime) fn latch_popup_menu_resolution(
        &mut self,
        app: &mut dyn AppDriver,
        picked: Option<String>,
        one: &mut Tx,
    ) {
        if !self.host.popup_fallback.is_active() {
            return;
        }
        if let Some(id) = picked.as_deref() {
            let Some(request) = self.host.popups.front() else { return };
            let Some(item) = request.menu.find(id) else { return };
            if !item.enabled { return; }
        }
        let defer = picked.is_some() && self.active_modal().is_some();
        let effects = step(
            &mut self.host.popup_fallback,
            &mut self.host.popup_fallback_seq,
            PopupFallbackEvent::Resolve { picked, defer },
        );
        if defer && matches!(self.host.popup_fallback, PopupFallbackState::DeferredPick { .. }) {
            self.commit.dirty = true;
            return;
        }
        for effect in effects {
            if let PopupFallbackEffect::Resolve(picked) = effect {
                // Validated against front() above; if an earlier callback already closed it, ignore.
                self.resolve_popup_into(app, picked.as_deref(), one).ok();
                self.latch_requests(app, one);
            }
        }
    }

    /// Moves newly queued pop-ups from `tx.popups` into `self.host.popups`. If a fallback menu
    /// is already open when a fresh request arrives, dismisses the open one with `None` first.
    pub(in crate::runtime) fn latch_popups(&mut self, app: &mut dyn AppDriver, tx: &mut Tx) {
        let incoming: Vec<crate::popup_menu::Request> = tx.popups.drain(..).filter_map(|call| call.take()).collect();
        if incoming.is_empty() {
            return;
        }
        if self.host.popup_fallback.is_active() {
            let effects = step(
                &mut self.host.popup_fallback,
                &mut self.host.popup_fallback_seq,
                PopupFallbackEvent::Supersede,
            );
            for effect in effects {
                if let PopupFallbackEffect::Resolve(picked) = effect {
                    // Dismissing an open menu with None before superseding; ignore if already closed.
                    self.resolve_popup_into(app, picked.as_deref(), tx).ok();
                }
            }
        }
        self.host.popups.extend(incoming);
    }

    /// Dismisses an open fallback pop-up menu (e.g. on window blur), answering `None` once.
    pub(in crate::runtime) fn dismiss_popup_menu_fallback(&mut self, app: &mut dyn AppDriver) {
        if !self.host.popup_fallback.is_active() {
            return;
        }
        let effects = step(
            &mut self.host.popup_fallback,
            &mut self.host.popup_fallback_seq,
            PopupFallbackEvent::Dismiss,
        );
        for effect in effects {
            if let PopupFallbackEffect::Resolve(picked) = effect {
                // Dismissing on window blur; ignore if already closed.
                self.resolve_popup(app, picked.as_deref()).ok();
            }
        }
    }

    /// Runs right after a frame commit: executes any deferred modal-close pick now that the
    /// closing in-window modal has unmounted (#2082 parity).
    pub(in crate::runtime) fn finish_deferred_popup_menu(
        &mut self,
        app: &mut dyn AppDriver,
        tx: &mut Tx,
        mut end_of_frame: Option<&mut crate::runtime::frame::EndOfFrameRequests>,
    ) {
        let PopupFallbackState::DeferredPick { field_path, .. } = &self.host.popup_fallback else {
            return;
        };
        let field_missing = field_path.as_deref().is_some_and(|p| self.live(p).is_none());
        let effects = step(
            &mut self.host.popup_fallback,
            &mut self.host.popup_fallback_seq,
            PopupFallbackEvent::PostCommit,
        );
        for effect in effects {
            if let PopupFallbackEffect::Resolve(picked) = effect {
                let effective = if field_missing { None } else { picked };
                let mut post_tx = self.transaction(app);
                // If the item changed or closed while closing the modal, fall back to None.
                if self.resolve_popup_into(app, effective.as_deref(), &mut post_tx).is_err() {
                    self.resolve_popup_into(app, None, &mut post_tx).ok();
                }
                self.flush_popup_post_tx(app, post_tx, tx, end_of_frame.as_deref_mut());
            }
        }
    }

    /// Runs after `step_motion` has populated fresh `self.paint.drawn` rects: auto-dismisses
    /// an open fallback menu whose key anchor left the tree or was superseded by a fresh
    /// request, and delivers any queued pop-up to `App::on_popup_menu`. Returns `true` when
    /// state changed and was committed (so the caller repaints the newly mounted menu).
    pub(in crate::runtime) fn deliver_popup_menu_fallback_after_paint(
        &mut self,
        app: &mut dyn AppDriver,
        tx: &mut Tx,
        mut end_of_frame: Option<&mut crate::runtime::frame::EndOfFrameRequests>,
    ) -> bool {
        let mut changed = false;
        let auto_event = match &self.host.popup_fallback {
            PopupFallbackState::Open { queued_at_open, .. } if self.host.popups.len() > *queued_at_open + 1 => {
                Some(PopupFallbackEvent::Supersede)
            }
            PopupFallbackState::Open { anchor_key: Some(key), .. } if self.anchor_rect(key).is_none() => {
                Some(PopupFallbackEvent::Dismiss)
            }
            _ => None,
        };
        if let Some(ev) = auto_event {
            let effects = step(&mut self.host.popup_fallback, &mut self.host.popup_fallback_seq, ev);
            for effect in effects {
                if let PopupFallbackEffect::Resolve(picked) = effect {
                    let mut post_tx = self.transaction(app);
                    // Auto-dismissing on anchor removal or supersede; ignore if already closed.
                    self.resolve_popup_into(app, picked.as_deref(), &mut post_tx).ok();
                    self.flush_popup_post_tx(app, post_tx, tx, end_of_frame.as_deref_mut());
                    changed = true;
                }
            }
        }
        if !self.popup_menu_fallback_active(app) {
            return changed;
        }
        for _ in 0..MAX_FALLBACK_DELIVERIES {
            if self.host.popups.is_empty() || self.host.popups.is_open() || self.popup_waits_for_layout() {
                break;
            }
            if !self.deliver_one_popup_fallback(app, tx, end_of_frame.as_deref_mut()) {
                break;
            }
            changed = true;
        }
        changed
    }

    /// Called by `popup_menu::Presenter::open_core` when `on_popup_menu` handles pop-ups on this
    /// platform (e.g. a pop-up queued via `AppHandle::update` between frames). Returns `true`
    /// when the host should request a redraw.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn deliver_popup_menu_fallback_from_host(&mut self, app: &mut dyn AppDriver) -> bool {
        let mut redraw = false;
        for _ in 0..MAX_FALLBACK_DELIVERIES {
            if self.host.popups.is_empty() || self.host.popups.is_open() {
                break;
            }
            if self.popup_waits_for_layout() {
                return true;
            }
            let mut tx = self.transaction(app);
            if !self.deliver_one_popup_fallback_no_commit(app, &mut tx) {
                break;
            }
            merge(&mut self.frame.pending, tx);
            redraw = true;
        }
        redraw
    }

    fn deliver_one_popup_fallback(
        &mut self,
        app: &mut dyn AppDriver,
        tx: &mut Tx,
        end_of_frame: Option<&mut crate::runtime::frame::EndOfFrameRequests>,
    ) -> bool {
        let mut post_tx = self.transaction(app);
        // Land the newly opened in-window fallback menu immediately so it is opaque and hit-testable on frame 0.
        post_tx.commit = post_tx.commit.instant();
        if !self.deliver_one_popup_fallback_no_commit(app, &mut post_tx) {
            return false;
        }
        self.commit.dirty = true;
        self.flush_popup_post_tx(app, post_tx, tx, end_of_frame);
        true
    }

    fn deliver_one_popup_fallback_no_commit(&mut self, app: &mut dyn AppDriver, post_tx: &mut Tx) -> bool {
        let Some(front) = self.host.popups.front() else { return false };
        let anchor = front.anchor.clone();
        let anchor_key = match &anchor {
            PopupAnchor::Key(k) => Some(k.clone()),
            PopupAnchor::Point(..) => None,
        };
        let field_path = front.field.as_deref().map(|f| f.path().to_owned());
        let queued_behind = self.host.popups.len().saturating_sub(1);
        match self.take_popup() {
            None => false,
            Some(None) => {
                // Invalid menu (missing key or duplicate id) answers None immediately.
                self.resolve_popup_into(app, None, post_tx).ok();
                self.latch_requests(app, post_tx);
                true
            }
            Some(Some(show)) => {
                let request = PopupMenuRequest::from_show(&show, anchor, self.zoom());
                let effects = step(
                    &mut self.host.popup_fallback,
                    &mut self.host.popup_fallback_seq,
                    PopupFallbackEvent::Open { anchor_key, field_path, queued_behind },
                );
                for effect in effects {
                    if matches!(effect, PopupFallbackEffect::Deliver) {
                        post_tx.popup_menu_opened = true;
                        app.on_popup_menu(request.clone(), post_tx);
                        self.latch_requests(app, post_tx);
                    }
                }
                self.commit.dirty = true;
                true
            }
        }
    }

    /// Flushes a post-commit popup resolution or delivery transaction: drains any memory-clipboard
    /// paste queued by a field menu, carries `EndOfFrameRequests` and wakes, and commits dirty
    /// state changes in the same frame (matching `finish_deferred_field_menu`).
    fn flush_popup_post_tx(
        &mut self,
        app: &mut dyn AppDriver,
        mut post_tx: Tx,
        tx: &mut Tx,
        end_of_frame: Option<&mut crate::runtime::frame::EndOfFrameRequests>,
    ) {
        let wanted_paste = post_tx.paste;
        let mut deferred = crate::runtime::frame::EndOfFrameRequests::take_from(&mut post_tx);
        self.latch_requests(app, &mut post_tx);
        if wanted_paste && self.edit.clipboard().is_some() {
            for queued in std::mem::take(&mut self.frame.queue) {
                if !matches!(queued.action, Action::Paste(..)) {
                    self.frame.queue.push_queued(queued);
                    continue;
                }
                if self.run_action(app, queued.action, &mut post_tx) {
                    self.commit.dirty = true;
                }
            }
            deferred.extend_from(&mut post_tx);
            self.latch_requests(app, &mut post_tx);
        }
        if let Some(dst) = end_of_frame {
            dst.extend(deferred);
        } else {
            deferred.restore_into(&mut post_tx);
        }
        self.schedule_wakes(post_tx.wakes.drain(..));
        if self.commit.dirty || self.commit.relayout || self.kbd.focus_request.is_some() {
            self.capture_keeps(&mut post_tx);
            self.commit.set_state_unchanged(false);
            self.commit_for(app, &post_tx, CommitWhy::DIRTY);
            self.apply_keeps(app);
        }
        merge(tx, post_tx);
    }
}

#[cfg(test)]
#[path = "popup_fallback_tests.rs"]
mod tests;
