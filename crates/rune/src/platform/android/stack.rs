//! In-activity full-screen page stack for Android (`open_window`, `App::window`,
//! `WindowHandle`, Back navigation, `on_close_request`, and `on_close`).
//!
//! Compiles on `target_os = "android"` and on every host test target so multi-window stacking,
//! `WindowHandle` routing, Escape-first Back navigation, `on_close_request` deferral, and
//! `on_close` teardown are verified off-device against real `Runtime<S>` instances.

use std::sync::{Arc, Mutex};

use crate::app::{Activation, CloseResponse, WindowRequest};
#[cfg(target_os = "android")]
use crate::platform::embed::EmbedFrame;
use crate::runtime::Waker;
#[cfg(test)]
use crate::runtime::RuntimeCore;
use crate::window::WakeEvent;
use crate::windows::AnyWindowSpec;
#[cfg(target_os = "android")]
use crate::input::host_events::ImeStep;
#[cfg(target_os = "android")]
use crate::Haptic;
use crate::{ColorScheme, KeyEvent, TouchPhase};

use super::page::{AndroidPage, PageShellAction, StackEnv};
pub(crate) use super::page::PageEntry;
#[cfg(target_os = "android")]
use super::host::ViewBridge;
#[cfg(target_os = "android")]
use super::input::TextInputAction;
#[cfg(target_os = "android")]
use super::picker::{PickAnswer, PickStep};
#[cfg(target_os = "android")]
use super::surface::AndroidSurface;

/// Maximum nested pump iterations per host turn (guards against re-entrant open/close loops).
const MAX_PUMP_ROUNDS: usize = 16;

/// Outcome of handling an Android system Back press on the top page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BackOutcome {
    /// Rune handled Back (consumed `Key::Escape`, deferred close, or popped a stacked page).
    Consumed,
    /// Root window did not consume `Key::Escape`; the activity should finish / go back.
    Unhandled,
}

/// Full-screen in-activity page stack for Android.
///
/// Slot `0` is always the root window (pinned at `visible_stack[0]`). Windows added through
/// `App::window` or opened dynamically with `rune::open_window` get stable slot indices; visible
/// pages stack full-screen above slot `0`, while hidden or `keep_running` pages stay retained in
/// `slots` until shown or closed.
pub(crate) struct PageStack {
    slots: Vec<Option<Box<dyn AndroidPage>>>,
    /// Bottom-to-top order of currently presented pages. Slot `0` (root) is always at index `0`.
    visible_stack: Vec<usize>,
    handle_events: Arc<Mutex<Vec<WakeEvent>>>,
    waker: Waker,
    env: StackEnv,
    #[cfg(target_os = "android")]
    pending_pick_owner: Option<(u64, usize)>,
}

impl PageStack {
    pub(crate) fn new(waker: Waker) -> Self {
        Self {
            slots: Vec::new(),
            visible_stack: Vec::new(),
            handle_events: Arc::new(Mutex::new(Vec::new())),
            waker,
            env: StackEnv::default(),
            #[cfg(target_os = "android")]
            pending_pick_owner: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn depth(&self) -> usize {
        self.visible_stack.len()
    }

    #[cfg(test)]
    pub(crate) fn top_key(&self) -> Option<&str> {
        let slot = *self.visible_stack.last()?;
        self.slots.get(slot)?.as_deref().map(AndroidPage::key)
    }

    #[cfg(test)]
    pub(crate) fn visible_keys(&self) -> Vec<&str> {
        self.visible_stack
            .iter()
            .filter_map(|&slot| self.slots.get(slot)?.as_deref().map(AndroidPage::key))
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn has_live_key(&self, key: &str) -> bool {
        self.slots.iter().flatten().any(|p| p.key() == key)
    }

    /// Replaces the stack with the initial windows from `App::run` or `run_windows`.
    /// The first window becomes the root page (slot `0`); any additional windows from
    /// `App::window` or the factory are registered behind or above it according to visibility.
    pub(crate) fn set_initial(&mut self, specs: Vec<AnyWindowSpec>) {
        self.shutdown();
        let flat = crate::windows::flatten(specs);
        for (index, spec) in flat.into_iter().enumerate() {
            self.add_spec(spec, index == 0);
        }
    }

    pub(crate) fn set_viewport(&mut self, viewport: (f32, f32), scale: f32) {
        self.env.viewport = viewport;
        self.env.scale = scale;
        let env = self.env;
        if let Some(top) = self.top_page_mut() {
            top.sync_env(env);
        }
    }

    pub(crate) fn set_safe_area(&mut self, insets: [f32; 4]) {
        self.env.safe_area = insets;
        let env = self.env;
        if let Some(top) = self.top_page_mut() {
            top.sync_env(env);
        }
    }

    pub(crate) fn set_keyboard_inset(&mut self, points: f32) {
        self.env.keyboard_inset = points;
        let env = self.env;
        if let Some(top) = self.top_page_mut() {
            top.sync_env(env);
        }
    }

    pub(crate) fn set_dark(&mut self, dark: bool) {
        self.env.dark = dark;
        let env = self.env;
        if let Some(top) = self.top_page_mut() {
            top.sync_env(env);
        }
    }

    pub(crate) fn set_touch_long_press(&mut self, secs: Option<f64>) {
        self.env.touch_long_press = secs;
        let env = self.env;
        if let Some(top) = self.top_page_mut() {
            top.sync_env(env);
        }
    }

    pub(crate) fn touch(&mut self, id: u64, phase: TouchPhase, x: f32, y: f32, time_secs: f64) {
        let env = self.env;
        let wake = self.waker.clone();
        if let Some(top) = self.top_page_mut() {
            top.touch(id, phase, x, y, time_secs, env, &wake);
        }
    }

    #[cfg(target_os = "android")]
    pub(crate) fn take_haptics(&mut self) -> Vec<Haptic> {
        self.top_page_mut().map(|p| p.take_haptics()).unwrap_or_default()
    }

    #[cfg(target_os = "android")]
    pub(crate) fn media_keys(&self) -> bool {
        self.top_page().is_some_and(AndroidPage::media_keys)
    }

    #[cfg(target_os = "android")]
    pub(crate) fn text_input(&mut self, action: TextInputAction) -> bool {
        let env = self.env;
        let wake = self.waker.clone();
        self.top_page_mut().is_some_and(|p| p.text_input(action, env, &wake))
    }

    #[cfg(target_os = "android")]
    pub(crate) fn key_up(&mut self, ev: KeyEvent) -> bool {
        let env = self.env;
        let wake = self.waker.clone();
        self.top_page_mut().is_some_and(|p| p.key_up(ev, env, &wake))
    }

    #[cfg(target_os = "android")]
    pub(crate) fn finish_composition(&mut self) {
        if let Some(top) = self.top_page_mut() {
            top.finish_composition();
        }
    }

    #[cfg(target_os = "android")]
    pub(crate) fn clear_ime_sync(&mut self) {
        for page in self.slots.iter_mut().flatten() {
            page.clear_ime_sync();
        }
    }

    #[cfg(all(target_os = "android", feature = "accessibility"))]
    pub(crate) fn a11y_receive(&mut self, a11y: &mut crate::a11y::android::Adapter) -> bool {
        self.top_page_mut().is_some_and(|p| p.a11y_receive(a11y))
    }

    #[cfg(all(target_os = "android", feature = "accessibility"))]
    pub(crate) fn a11y_sync(
        &mut self,
        a11y: &mut crate::a11y::android::Adapter,
        now: crate::time::Instant,
    ) -> Option<crate::a11y::android::SyncOutcome> {
        self.top_page_mut()?.a11y_sync(a11y, now)
    }

    #[cfg(all(target_os = "android", feature = "accessibility"))]
    pub(crate) fn a11y_wake(
        &mut self,
        a11y: &mut crate::a11y::android::Adapter,
        attempt: crate::machine::Attempt,
        now: crate::time::Instant,
    ) -> Option<crate::a11y::android::SyncOutcome> {
        self.top_page_mut()?.a11y_wake(a11y, attempt, now)
    }

    #[cfg(target_os = "android")]
    pub(crate) fn top_srgb_blend(&self) -> bool {
        self.top_page().map_or(true, AndroidPage::srgb_blend)
    }

    #[cfg(target_os = "android")]
    fn top_page(&self) -> Option<&dyn AndroidPage> {
        let slot = *self.visible_stack.last()?;
        self.slots.get(slot)?.as_deref()
    }

    fn top_page_mut(&mut self) -> Option<&mut Box<dyn AndroidPage>> {
        let slot = *self.visible_stack.last()?;
        self.slots.get_mut(slot)?.as_mut()
    }

    fn key_is_open(&self, key: &str) -> bool {
        self.slots.iter().flatten().any(|p| p.key() == key)
    }

    fn add_spec(&mut self, spec: AnyWindowSpec, force_root: bool) -> bool {
        if self.key_is_open(spec.key()) {
            eprintln!("rune: a window keyed {:?} is already open", spec.key());
            return false;
        }
        let PageEntry(page) = spec.into_android_page();
        let slot = self.slots.len();
        let start_visible = force_root || page.start_visible();
        let queue = Arc::clone(&self.handle_events);
        let wake = self.waker.clone();
        page.handle().bind_callback(
            Arc::new(move |ev| {
                queue.lock().unwrap_or_else(|e| e.into_inner()).push(ev);
                wake();
            }),
            slot,
        );
        page.connect_shared(self.waker.clone());
        self.slots.push(Some(page));
        if start_visible {
            self.push_visible(slot);
            return true;
        }
        false
    }

    fn push_visible(&mut self, slot: usize) {
        let prev_top = self.visible_stack.last().copied();
        if prev_top == Some(slot) {
            return;
        }
        if let Some(prev) = prev_top
            && let Some(Some(page)) = self.slots.get_mut(prev)
        {
            page.set_active(false, self.env, &self.waker);
        }
        if let Some(pos) = self.visible_stack.iter().position(|&s| s == slot) {
            self.visible_stack.remove(pos);
        }
        self.visible_stack.push(slot);
        let env = self.env;
        let wake = self.waker.clone();
        if let Some(Some(page)) = self.slots.get_mut(slot) {
            page.set_active(true, env, &wake);
            page.announce_created();
        }
    }

    fn show_slot(&mut self, slot: usize) -> bool {
        if slot == 0 || self.slots.get(slot).and_then(Option::as_ref).is_none() {
            return false;
        }
        if self.visible_stack.last().copied() == Some(slot) {
            return false;
        }
        self.push_visible(slot);
        true
    }

    fn focus_slot(&mut self, slot: usize) -> bool {
        if slot == 0 || !self.visible_stack.contains(&slot) {
            return false;
        }
        if self.visible_stack.last().copied() == Some(slot) {
            return false;
        }
        self.push_visible(slot);
        true
    }

    fn hide_slot(&mut self, slot: usize) -> bool {
        if slot == 0 {
            return false;
        }
        let Some(pos) = self.visible_stack.iter().position(|&s| s == slot) else {
            return false;
        };
        let was_top = pos + 1 == self.visible_stack.len();
        self.visible_stack.remove(pos);
        let env = self.env;
        let wake = self.waker.clone();
        if let Some(Some(page)) = self.slots.get_mut(slot) {
            page.set_active(false, env, &wake);
        }
        if was_top
            && let Some(&next) = self.visible_stack.last()
            && let Some(Some(page)) = self.slots.get_mut(next)
        {
            page.set_active(true, env, &wake);
        }
        true
    }

    fn close_slot(&mut self, slot: usize) -> bool {
        if slot == 0 {
            return false;
        }
        let Some(page_opt) = self.slots.get_mut(slot) else {
            return false;
        };
        let Some(page) = page_opt.as_mut() else {
            return false;
        };
        let visible_pos = self.visible_stack.iter().position(|&s| s == slot);
        let keep = page.keep_running();
        // Closing an already-hidden keep_running page is a no-op (does not double-run on_close).
        if visible_pos.is_none() && keep {
            return false;
        }
        let env = self.env;
        let wake = self.waker.clone();
        let was_top = visible_pos.is_some_and(|pos| pos + 1 == self.visible_stack.len());
        if let Some(pos) = visible_pos {
            self.visible_stack.remove(pos);
        }
        if visible_pos.is_some() {
            page.run_on_close(env, &wake);
        }
        page.cancel_file_dialogs();
        page.mark_closed();
        #[cfg(target_os = "android")]
        if self.pending_pick_owner.is_some_and(|(_, owner)| owner == slot) {
            self.pending_pick_owner = None;
        }
        if !keep {
            self.slots[slot] = None;
        }
        if was_top
            && let Some(&next) = self.visible_stack.last()
            && let Some(Some(next_page)) = self.slots.get_mut(next)
        {
            next_page.set_active(true, env, &wake);
        }
        true
    }

    fn request_close_slot(&mut self, slot: usize) -> bool {
        if slot == 0 {
            return false;
        }
        let env = self.env;
        let wake = self.waker.clone();
        let response = match self.slots.get_mut(slot).and_then(Option::as_mut) {
            Some(page) => page.run_on_close_request(env, &wake),
            None => return false,
        };
        match response {
            Some(CloseResponse::Defer) => {
                if let Some(Some(page)) = self.slots.get_mut(slot)
                    && page.take_shell_action() == PageShellAction::Close
                {
                    return self.close_slot(slot);
                }
                true
            }
            None | Some(CloseResponse::Close) => self.close_slot(slot),
        }
    }

    fn set_slot_color_scheme(&mut self, slot: usize, scheme: ColorScheme) -> bool {
        let Some(Some(page)) = self.slots.get_mut(slot) else {
            return false;
        };
        page.set_color_scheme(scheme, self.env.dark);
        self.visible_stack.last().copied() == Some(slot)
    }

    fn drain_opened(&mut self) -> bool {
        let specs = crate::windows::flatten(crate::windows::take_opened());
        if specs.is_empty() {
            return false;
        }
        let mut changed = false;
        for spec in specs {
            if self.add_spec(spec, false) {
                changed = true;
            }
        }
        changed
    }

    fn drain_handle_events(&mut self) -> bool {
        let events = std::mem::take(
            &mut *self.handle_events.lock().unwrap_or_else(|e| e.into_inner()),
        );
        if events.is_empty() {
            return false;
        }
        let mut changed = false;
        for event in events {
            match event {
                WakeEvent::Window(slot, WindowRequest::Open(_) | WindowRequest::Show) => {
                    changed |= self.show_slot(slot);
                }
                WakeEvent::Window(slot, WindowRequest::Hide) => {
                    changed |= self.hide_slot(slot);
                }
                WakeEvent::Window(slot, WindowRequest::Close) => {
                    changed |= self.close_slot(slot);
                }
                WakeEvent::Focus(slot) => {
                    changed |= self.focus_slot(slot);
                }
                WakeEvent::ColorScheme(slot, scheme) => {
                    changed |= self.set_slot_color_scheme(slot, scheme);
                }
                _ => {}
            }
        }
        changed
    }

    fn drain_shared_requests(&mut self) -> bool {
        let mut changed = false;
        let mut slot = 0;
        while slot < self.slots.len() {
            let env = self.env;
            let wake = self.waker.clone();
            let batch = match self.slots[slot].as_mut() {
                Some(page) => page.drain_shared(env, &wake),
                None => {
                    slot += 1;
                    continue;
                }
            };
            if batch.woken {
                changed = true;
            }
            if let Some(scheme) = batch.color_scheme {
                changed |= self.set_slot_color_scheme(slot, scheme);
            }
            match batch.window {
                Some(WindowRequest::Open(_) | WindowRequest::Show) => {
                    changed |= self.show_slot(slot);
                }
                Some(WindowRequest::Hide) => {
                    changed |= self.hide_slot(slot);
                }
                Some(WindowRequest::Close) => {
                    changed |= self.close_slot(slot);
                }
                None => {}
            }
            if batch.activation == Some(Activation::Focus) {
                changed |= self.focus_slot(slot);
            }
            slot += 1;
        }
        changed
    }

    fn drain_shell_requests(&mut self) -> bool {
        let mut changed = false;
        let mut slot = 0;
        while slot < self.slots.len() {
            let action = match self.slots[slot].as_mut() {
                Some(page) => page.take_shell_action(),
                None => {
                    slot += 1;
                    continue;
                }
            };
            match action {
                PageShellAction::Close => changed |= self.close_slot(slot),
                PageShellAction::UserCloseRequest => changed |= self.request_close_slot(slot),
                PageShellAction::None => {}
            }
            slot += 1;
        }
        changed
    }

    /// Drains `open_window`, `WindowHandle` events, `AppHandle`/`Shared` requests, and
    /// `Tx::close_window` / `Tx::request_user_close` requests until settled (bounded).
    /// Returns whether the visible stack or top page changed and needs a redraw.
    pub(crate) fn pump(&mut self) -> bool {
        let mut any = false;
        for _ in 0..MAX_PUMP_ROUNDS {
            let mut round = false;
            round |= self.drain_opened();
            round |= self.drain_handle_events();
            round |= self.drain_shared_requests();
            round |= self.drain_shell_requests();
            any |= round;
            if !round {
                break;
            }
        }
        any
    }

    /// Handles Android system Back on the top page:
    /// 1. Deliver `Key::Escape` to the top page and step its input.
    /// 2. Stay if `Key::Escape` was consumed (or if its handler closed the page).
    /// 3. Otherwise, on a stacked page (`depth > 1`), consult `on_close_request`: stay on
    ///    `CloseResponse::Defer`, or pop and run `on_close` on `CloseResponse::Close` / none.
    /// 4. On the root window (`depth == 1`), return [`BackOutcome::Unhandled`] so the activity
    ///    goes back.
    pub(crate) fn handle_back(&mut self) -> BackOutcome {
        let _ = self.pump();
        let Some(&top_slot) = self.visible_stack.last() else {
            return BackOutcome::Unhandled;
        };
        let env = self.env;
        let wake = self.waker.clone();
        let escape_consumed = match self.slots.get_mut(top_slot).and_then(Option::as_mut) {
            Some(page) => page.dispatch_escape(env, &wake),
            None => return BackOutcome::Unhandled,
        };
        let shell_action = self
            .slots
            .get_mut(top_slot)
            .and_then(Option::as_mut)
            .map_or(PageShellAction::None, |p| p.take_shell_action());

        let outcome = match shell_action {
            PageShellAction::Close => {
                if top_slot != 0 {
                    let _ = self.close_slot(top_slot);
                    BackOutcome::Consumed
                } else if escape_consumed {
                    BackOutcome::Consumed
                } else {
                    BackOutcome::Unhandled
                }
            }
            PageShellAction::UserCloseRequest if top_slot != 0 => {
                let _ = self.request_close_slot(top_slot);
                BackOutcome::Consumed
            }
            _ if escape_consumed => BackOutcome::Consumed,
            _ if self.visible_stack.len() > 1 => {
                let _ = self.request_close_slot(top_slot);
                BackOutcome::Consumed
            }
            _ => BackOutcome::Unhandled,
        };

        let _ = self.pump();
        outcome
    }

    /// Draws the top visible page onto the shared `AndroidSurface`, then pumps any windows or
    /// close requests queued during the frame.
    #[cfg(target_os = "android")]
    pub(crate) fn deliver_file_pick(&mut self, id: u64, answer: PickAnswer, bridge: &Arc<Mutex<Option<ViewBridge>>>) {
        let target = self
            .pending_pick_owner
            .filter(|&(req_id, _)| req_id == id)
            .map(|(_, slot)| slot)
            .or_else(|| self.visible_stack.last().copied());
        if self.pending_pick_owner.is_some_and(|(req_id, _)| req_id == id) {
            self.pending_pick_owner = None;
        }
        if let Some(slot) = target
            && let Some(Some(page)) = self.slots.get_mut(slot)
        {
            page.deliver_file_pick(id, answer, bridge);
        }
    }

    #[cfg(target_os = "android")]
    pub(crate) fn draw_top(&mut self, surface: &mut AndroidSurface, resumed: bool) -> Option<(EmbedFrame, Vec<Haptic>, PickStep, ImeStep)> {
        let _ = self.pump();
        let top_slot = *self.visible_stack.last()?;
        let env = self.env;
        let wake = self.waker.clone();
        let (result, haptics, pick_step) = match self.slots.get_mut(top_slot).and_then(Option::as_mut) {
            Some(page) => (page.draw(surface, env, &wake), page.take_haptics(), page.take_file_pick()),
            None => return None,
        };
        if let PickStep::Start(ref req) = pick_step {
            self.pending_pick_owner = Some((req.id, top_slot));
        }
        let mut frame = match result {
            Ok(frame) => frame,
            Err(err) => {
                eprintln!("rune: android page draw failed: {err}");
                return None;
            }
        };
        if self.pump() {
            frame.animating = true;
        }
        let ime = self
            .top_page_mut()
            .map_or(ImeStep { cancel: false, set_allowed: None, cursor_area: None }, |p| p.ime_sync_step(resumed));
        Some((frame, haptics, pick_step, ime))
    }

    /// Runs `on_close` on all visible pages (top-to-bottom) and drops every page.
    pub(crate) fn shutdown(&mut self) {
        let visible = std::mem::take(&mut self.visible_stack);
        let env = self.env;
        let wake = self.waker.clone();
        for slot in visible.into_iter().rev() {
            if let Some(Some(page)) = self.slots.get_mut(slot) {
                page.cancel_file_dialogs();
                page.run_on_close(env, &wake);
                page.mark_closed();
            }
        }
        #[cfg(target_os = "android")]
        {
            self.pending_pick_owner = None;
        }
        self.slots.clear();
        self.handle_events.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    #[cfg(test)]
    pub(crate) fn step_top_frame(&mut self, dt: f64) -> bool {
        let _ = self.pump();
        let Some(&top_slot) = self.visible_stack.last() else {
            return false;
        };
        let env = self.env;
        let wake = self.waker.clone();
        let mut animating = self
            .slots
            .get_mut(top_slot)
            .and_then(Option::as_mut)
            .is_some_and(|p| p.step_headless_frame(dt, env, &wake));
        if self.pump() {
            animating = true;
        }
        animating
    }

    #[cfg(test)]
    pub(crate) fn tap_top(&mut self, element_key: &str) -> bool {
        let _ = self.pump();
        let Some(&top_slot) = self.visible_stack.last() else {
            return false;
        };
        let env = self.env;
        let wake = self.waker.clone();
        let tapped = self
            .slots
            .get_mut(top_slot)
            .and_then(Option::as_mut)
            .is_some_and(|p| p.tap_key(element_key, env, &wake));
        let _ = self.pump();
        tapped
    }

    #[cfg(test)]
    pub(crate) fn send_key_top(&mut self, ev: KeyEvent) {
        let _ = self.pump();
        let Some(&top_slot) = self.visible_stack.last() else {
            return;
        };
        let env = self.env;
        let wake = self.waker.clone();
        if let Some(Some(page)) = self.slots.get_mut(top_slot) {
            page.send_key(ev, env, &wake);
        }
        let _ = self.pump();
    }

    #[cfg(test)]
    pub(crate) fn top_has_focus(&self) -> bool {
        self.visible_stack
            .last()
            .and_then(|&slot| self.slots.get(slot))
            .and_then(Option::as_deref)
            .is_some_and(AndroidPage::has_focus)
    }

    #[cfg(test)]
    pub(crate) fn top_runtime_core(&self) -> Option<&RuntimeCore> {
        let slot = *self.visible_stack.last()?;
        self.slots.get(slot)?.as_deref()?.runtime_core()
    }
}
