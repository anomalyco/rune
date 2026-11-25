//! Declarative pointer lock (`El::pointer_lock`) for relative mouse-look capture on desktop
//! windows.
//!
//! Modelled as an explicit state machine (`LockState` / `LockEvent` / `LockEffect`) owned by
//! `PointerState`. Opposite owed host effects (`Grab` and `Ungrab`) cancel out before the host
//! syncs, so a grab the window never applied needs no release. On web and iOS (and overlays),
//! `PointerLock` is marked unsupported so elements never capture.
use super::*;
use crate::machine::Effects;

/// Phase of declarative pointer lock in a window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LockState {
    /// No eligible drawn element currently asks for pointer lock.
    #[default]
    Off,
    /// An eligible drawn element asks for pointer lock; a primary press on it captures.
    Ready,
    /// Pointer lock is active; raw relative motion routes to the asking element.
    Locked,
}

/// Inputs that advance [`LockState`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LockEvent {
    /// Whether an eligible drawn element asks for pointer lock.
    Asked(bool),
    /// A primary mouse press landed on the asking element.
    Pressed,
    /// Escape, focus loss, or pointer cancellation released the lock.
    Released,
}

/// Cursor-grab transition owed to the desktop window host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LockEffect {
    /// Grab the OS cursor (`CursorGrabMode::Locked`, falling back to `Confined`).
    Grab,
    /// Release the OS cursor grab (`CursorGrabMode::None`).
    Ungrab,
}

/// Pure state transition for declarative pointer lock.
///
/// - `Off    + Asked(true)  -> Ready`
/// - `Ready  + Asked(false) -> Off`
/// - `Ready  + Pressed      -> Locked, Grab`
/// - `Locked + Asked(false) -> Off,    Ungrab`
/// - `Locked + Released     -> Ready,  Ungrab`
/// - Anything else: no change.
pub(crate) fn step(state: &mut LockState, event: LockEvent) -> Effects<LockEffect> {
    match (*state, event) {
        (LockState::Off, LockEvent::Asked(true)) => {
            *state = LockState::Ready;
            Effects::none()
        }
        (LockState::Ready, LockEvent::Asked(false)) => {
            *state = LockState::Off;
            Effects::none()
        }
        (LockState::Ready, LockEvent::Pressed) => {
            *state = LockState::Locked;
            Effects::one(LockEffect::Grab)
        }
        (LockState::Locked, LockEvent::Asked(false)) => {
            *state = LockState::Off;
            Effects::one(LockEffect::Ungrab)
        }
        (LockState::Locked, LockEvent::Released) => {
            *state = LockState::Ready;
            Effects::one(LockEffect::Ungrab)
        }
        _ => Effects::none(),
    }
}

/// Pointer-lock state and at-most-one owed host effect owned by `PointerState`.
#[derive(Clone, Copy, Debug)]
pub(in crate::runtime) struct PointerLock {
    state: LockState,
    owed: Option<LockEffect>,
    supported: bool,
}

impl PointerLock {
    #[inline]
    pub(in crate::runtime) fn new() -> Self {
        Self {
            state: LockState::Off,
            owed: None,
            supported: !cfg!(any(target_arch = "wasm32", target_os = "ios")),
        }
    }

    #[inline]
    pub(in crate::runtime) fn supported(&self) -> bool {
        self.supported
    }

    /// Enables or disables pointer lock for the current host (web, iOS and overlays disable it).
    /// Returns whether `is_locked()` flipped.
    #[inline]
    pub(in crate::runtime) fn set_supported(&mut self, supported: bool) -> bool {
        self.supported = supported;
        if !supported {
            self.step(LockEvent::Asked(false))
        } else {
            false
        }
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn state(&self) -> LockState {
        self.state
    }

    #[inline]
    pub(in crate::runtime) fn is_locked(&self) -> bool {
        self.state == LockState::Locked
    }

    #[inline]
    pub(in crate::runtime) fn is_off(&self) -> bool {
        self.state == LockState::Off
    }

    /// Advances the state machine and coalesces/cancels owed host effects.
    /// Returns `true` when `is_locked()` flipped.
    pub(in crate::runtime) fn step(&mut self, event: LockEvent) -> bool {
        let before = self.is_locked();
        for effect in step(&mut self.state, event) {
            self.owed = match (self.owed, effect) {
                (Some(LockEffect::Grab), LockEffect::Ungrab)
                | (Some(LockEffect::Ungrab), LockEffect::Grab) => None,
                (_, effect) => Some(effect),
            };
        }
        self.is_locked() != before
    }

    /// Takes any pending grab or release effect owed to the host.
    #[inline]
    pub(in crate::runtime) fn take_effect(&mut self) -> Option<LockEffect> {
        self.owed.take()
    }
}

impl RuntimeCore {
    /// Whether an element currently holds pointer lock.
    #[inline]
    pub fn pointer_locked(&self) -> bool {
        self.pointer.pointer_lock.is_locked()
    }

    /// Marks whether the current host supports pointer lock capture (disabled on web, iOS and
    /// native overlays).
    #[inline]
    pub(crate) fn set_pointer_lock_supported(&mut self, supported: bool) {
        if self.pointer.pointer_lock.set_supported(supported) {
            self.commit.dirty = true;
        }
    }

    /// Returns the frozen press point while pointer lock is active, or `(x, y)` otherwise.
    #[inline]
    pub(in crate::runtime) fn locked_point(&self, x: f32, y: f32) -> (f32, f32) {
        if self.pointer_locked() {
            self.pointer.pos.unwrap_or((x, y))
        } else {
            (x, y)
        }
    }

    /// The topmost drawn, allowed, non-exiting, non-disabled element with `El::pointer_lock`
    /// in this frame's hit list.
    pub(in crate::runtime) fn pointer_lock_target(&self) -> Option<usize> {
        if !self.pointer.pointer_lock.supported() || !self.tree.has_pointer_lock {
            return None;
        }
        self.hit.find_hit(|id, r| {
            if r[2] <= r[0] || r[3] <= r[1] {
                return None;
            }
            let i = *self.tree.node_index.get(&id)?;
            (!self.subtree_exiting(i)
                && self.allowed(i)
                && !self.node_disabled(i)
                && self.tree.nodes[i].handlers.get().is_some_and(|h| h.pointer_lock.is_some()))
            .then_some(i)
        })
    }

    /// Steps the pointer-lock state machine and invalidates the view when `pointer_locked()`
    /// flips, so `Cx::pointer_locked()` updates.
    pub(in crate::runtime) fn step_pointer_lock(&mut self, event: LockEvent) -> bool {
        let flipped = self.pointer.pointer_lock.step(event);
        if flipped {
            self.commit.dirty = true;
        }
        flipped
    }

    /// Synchronises pointer lock with whether an eligible element currently asks for it.
    /// Returns `true` when `pointer_locked()` flipped (for example when the asking element
    /// unmounted, stopped asking, became disabled/inert, or was covered by a modal).
    pub(in crate::runtime) fn sync_pointer_lock(&mut self) -> bool {
        if !self.tree.has_pointer_lock && self.pointer.pointer_lock.is_off() {
            return false;
        }
        let asked = self.pointer_lock_target().is_some();
        self.step_pointer_lock(LockEvent::Asked(asked))
    }

    /// Captures pointer lock when a primary mouse press lands directly on the asking element
    /// (not on a child control that takes primary input itself or a covering hit sibling).
    pub(in crate::runtime) fn try_capture_pointer_lock(
        &mut self,
        target: usize,
        x: f32,
        y: f32,
        touch: bool,
    ) {
        if touch || self.pointer_locked() || !self.pointer.pointer_lock.supported() {
            return;
        }
        let Some(lock_i) = self.pointer_lock_target() else { return };
        if target == lock_i && self.top_hit(x, y).is_none_or(|top| self.node_holds(lock_i, top)) {
            self.step_pointer_lock(LockEvent::Pressed);
        }
    }

    /// Releases an active pointer lock (Escape, window focus loss, or pointer cancellation).
    /// Returns `true` when the lock was active and released.
    pub(in crate::runtime) fn release_pointer_lock(&mut self) -> bool {
        self.step_pointer_lock(LockEvent::Released)
    }

    /// Takes any grab/ungrab effect owed to the desktop window host, syncing against the latest
    /// tree first.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn take_pointer_lock_effect(&mut self) -> Option<LockEffect> {
        self.sync_pointer_lock();
        self.pointer.pointer_lock.take_effect()
    }

    /// Delivers a relative mouse-motion delta `(dx, dy)` in points while pointer lock is held.
    /// Dropped when not locked, when the delta is zero or non-finite, or when no target asks.
    /// Coalesces into the last queued `Action::PointerLockMotion` when it targets the same
    /// element.
    pub fn pointer_motion(&mut self, dx: f32, dy: f32) {
        if !self.pointer_locked()
            || !dx.is_finite()
            || !dy.is_finite()
            || (dx == 0.0 && dy == 0.0)
        {
            return;
        }
        let Some(i) = self.pointer_lock_target() else { return };
        let path = &self.tree.nodes[i].path;
        if let Some(Action::PointerLockMotion(queued_path, delta)) = self.frame.queue.last_mut()
            && queued_path == path
        {
            delta[0] += dx;
            delta[1] += dy;
            return;
        }
        self.frame.queue.push(Action::PointerLockMotion(path.clone(), [dx, dy]));
    }
}

#[cfg(test)]
#[path = "pointer_lock_tests.rs"]
mod tests;
