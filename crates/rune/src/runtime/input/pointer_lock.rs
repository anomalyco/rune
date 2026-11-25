//! Declarative pointer lock (`El::pointer_lock`) for relative mouse-look capture on desktop
//! windows and web browsers.
//!
//! Modelled as an explicit state machine (`LockState` / `LockEvent` / `LockEffect`) owned by
//! `PointerState`. Opposite owed host effects (`Grab` and `Ungrab`) cancel out before the host
//! syncs, so a grab the host never applied needs no release. Desktop windows grant immediately
//! (`LockPolicy::Immediate`), web browsers grant or refuse asynchronously (`LockPolicy::Async`,
//! entering `LockState::Pending` until `pointerlockchange` or `pointerlockerror`), and iOS and
//! native overlays disable capture (`LockPolicy::Unsupported`).
use super::*;
use crate::machine::Effects;

/// Host grant policy for declarative pointer lock.
// All three host policies exist on every target so desktop tests exercise browser/iOS transitions.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LockPolicy {
    /// Host does not support pointer lock (iOS, native overlays): never captures.
    Unsupported,
    /// Desktop / headless host: a primary press transitions `Ready -> Locked` immediately.
    Immediate,
    /// Browser host (W3C Pointer Lock API): a primary press transitions `Ready -> Pending` and
    /// awaits `LockEvent::Granted` (`pointerlockchange`) or `LockEvent::PlatformReleased`
    /// (`pointerlockerror` / rejected promise).
    Async,
}

impl LockPolicy {
    #[inline]
    pub(crate) const fn default_for_target() -> Self {
        #[cfg(target_os = "ios")]
        { Self::Unsupported }
        #[cfg(target_arch = "wasm32")]
        { Self::Async }
        #[cfg(not(any(target_arch = "wasm32", target_os = "ios")))]
        { Self::Immediate }
    }
}

/// Phase of declarative pointer lock in a window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LockState {
    /// No eligible drawn element currently asks for pointer lock.
    #[default]
    Off,
    /// An eligible drawn element asks for pointer lock; a primary press on it captures.
    Ready,
    /// Browser host requested pointer lock inside a user gesture; awaiting `pointerlockchange`
    /// (`Granted`) or refusal (`PlatformReleased`). `pointer_locked()` stays `false` until granted.
    Pending,
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
    /// The platform asynchronously confirmed a pending lock (`pointerlockchange` -> canvas).
    Granted,
    /// Escape, focus loss, or pointer cancellation released the lock from Rune's side.
    Released,
    /// The platform itself exited or refused the lock (`pointerlockchange` -> null,
    /// `pointerlockerror`, or rejected `requestPointerLock` promise): no `Ungrab` is owed.
    PlatformReleased,
}

/// Cursor-grab transition owed to the window or browser host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LockEffect {
    /// Grab the OS cursor (`CursorGrabMode::Locked`, fallback `Confined`, or web `requestPointerLock`).
    Grab,
    /// Release the OS cursor grab (`CursorGrabMode::None` or web `document.exitPointerLock`).
    Ungrab,
}

/// Pure state transition for declarative pointer lock under `policy`.
///
/// - `Off     + Asked(true)       -> Ready`
/// - `Ready   + Asked(false)      -> Off`
/// - `Ready   + Pressed (Immed)   -> Locked,  Grab`
/// - `Ready   + Pressed (Async)   -> Pending, Grab`
/// - `Pending + Granted           -> Locked`
/// - `Pending + Asked(false)      -> Off,     Ungrab`
/// - `Pending + Released          -> Ready,   Ungrab`
/// - `Pending + PlatformReleased  -> Ready`
/// - `Locked  + Asked(false)      -> Off,     Ungrab`
/// - `Locked  + Released          -> Ready,   Ungrab`
/// - `Locked  + PlatformReleased  -> Ready`
/// - `Off | Ready + Granted       -> unchanged, Ungrab` (release stale late grant)
/// - Anything else: no change.
pub(crate) fn step_with(state: &mut LockState, policy: LockPolicy, event: LockEvent) -> Effects<LockEffect> {
    if policy == LockPolicy::Unsupported {
        let was_active = matches!(*state, LockState::Pending | LockState::Locked);
        *state = LockState::Off;
        return if was_active { Effects::one(LockEffect::Ungrab) } else { Effects::none() };
    }
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
            *state = match policy {
                LockPolicy::Immediate => LockState::Locked,
                LockPolicy::Async => LockState::Pending,
                LockPolicy::Unsupported => unreachable!(),
            };
            Effects::one(LockEffect::Grab)
        }
        (LockState::Pending, LockEvent::Granted) => {
            *state = LockState::Locked;
            Effects::none()
        }
        (LockState::Pending, LockEvent::Asked(false)) => {
            *state = LockState::Off;
            Effects::one(LockEffect::Ungrab)
        }
        (LockState::Pending, LockEvent::Released) => {
            *state = LockState::Ready;
            Effects::one(LockEffect::Ungrab)
        }
        (LockState::Pending, LockEvent::PlatformReleased) => {
            *state = LockState::Ready;
            Effects::none()
        }
        (LockState::Locked, LockEvent::Asked(false)) => {
            *state = LockState::Off;
            Effects::one(LockEffect::Ungrab)
        }
        (LockState::Locked, LockEvent::Released) => {
            *state = LockState::Ready;
            Effects::one(LockEffect::Ungrab)
        }
        (LockState::Locked, LockEvent::PlatformReleased) => {
            *state = LockState::Ready;
            Effects::none()
        }
        (LockState::Off | LockState::Ready, LockEvent::Granted) => Effects::one(LockEffect::Ungrab),
        _ => Effects::none(),
    }
}

/// Pure state transition for desktop (`LockPolicy::Immediate`).
#[cfg(test)]
pub(crate) fn step(state: &mut LockState, event: LockEvent) -> Effects<LockEffect> {
    step_with(state, LockPolicy::Immediate, event)
}

/// Pointer-lock state and at-most-one owed host effect owned by `PointerState`.
#[derive(Clone, Copy, Debug)]
pub(in crate::runtime) struct PointerLock {
    state: LockState,
    owed: Option<LockEffect>,
    policy: LockPolicy,
}

impl PointerLock {
    #[inline]
    pub(in crate::runtime) fn new() -> Self {
        Self {
            state: LockState::Off,
            owed: None,
            policy: LockPolicy::default_for_target(),
        }
    }

    #[inline]
    pub(in crate::runtime) fn supported(&self) -> bool {
        self.policy != LockPolicy::Unsupported
    }

    /// Sets the host grant policy (`Immediate` on desktop, `Async` on web, `Unsupported` on
    /// iOS/overlays). Returns whether `is_locked()` flipped.
    #[inline]
    pub(in crate::runtime) fn set_policy(&mut self, policy: LockPolicy) -> bool {
        let mut flipped = false;
        if policy == LockPolicy::Unsupported {
            flipped = self.step(LockEvent::Asked(false));
        }
        self.policy = policy;
        flipped
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
    pub(in crate::runtime) fn is_pending(&self) -> bool {
        self.state == LockState::Pending
    }

    #[inline]
    pub(in crate::runtime) fn is_off(&self) -> bool {
        self.state == LockState::Off
    }

    /// Advances the state machine and coalesces/cancels owed host effects.
    /// Returns `true` when `is_locked()` flipped.
    pub(in crate::runtime) fn step(&mut self, event: LockEvent) -> bool {
        let before = self.is_locked();
        if matches!(event, LockEvent::PlatformReleased) && self.owed == Some(LockEffect::Grab) {
            self.owed = None;
        }
        for effect in step_with(&mut self.state, self.policy, event) {
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

    /// Sets the host's pointer-lock grant policy (`Immediate`, `Async`, or `Unsupported`).
    #[inline]
    pub(crate) fn set_pointer_lock_policy(&mut self, policy: LockPolicy) {
        if self.pointer.pointer_lock.set_policy(policy) {
            self.commit.dirty = true;
        }
    }

    /// Marks whether the current host supports pointer lock capture (disabled on iOS and
    /// native overlays).
    #[inline]
    pub(crate) fn set_pointer_lock_supported(&mut self, supported: bool) {
        let policy = if supported {
            LockPolicy::default_for_target()
        } else {
            LockPolicy::Unsupported
        };
        self.set_pointer_lock_policy(policy);
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

    /// Captures pointer lock (or requests an async browser grab) when a primary mouse press
    /// lands directly on the asking element (not on a child control that takes primary input
    /// itself or a covering hit sibling).
    pub(in crate::runtime) fn try_capture_pointer_lock(
        &mut self,
        target: usize,
        x: f32,
        y: f32,
        touch: bool,
    ) {
        if touch
            || self.pointer_locked()
            || self.pointer.pointer_lock.is_pending()
            || !self.pointer.pointer_lock.supported()
        {
            return;
        }
        let Some(lock_i) = self.pointer_lock_target() else { return };
        if target == lock_i && self.top_hit(x, y).is_none_or(|top| self.node_holds(lock_i, top)) {
            self.step_pointer_lock(LockEvent::Pressed);
        }
    }

    /// Releases an active or pending pointer lock (Escape, window focus loss, or pointer
    /// cancellation). Returns `true` when the lock was active (`Locked`) and released.
    pub(in crate::runtime) fn release_pointer_lock(&mut self) -> bool {
        self.step_pointer_lock(LockEvent::Released)
    }

    /// Called by an async host (web `pointerlockchange` -> canvas) when the browser grants
    /// pointer lock. Returns `true` when `pointer_locked()` flipped to `true`.
    #[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
    pub(crate) fn pointer_lock_granted(&mut self) -> bool {
        self.sync_pointer_lock();
        self.step_pointer_lock(LockEvent::Granted)
    }

    /// Called by an async host (web `pointerlockchange` -> null, `pointerlockerror`, or rejected
    /// promise) when the browser exits or refuses pointer lock. Returns `true` when
    /// `pointer_locked()` flipped to `false`.
    #[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
    pub(crate) fn pointer_lock_lost(&mut self) -> bool {
        self.step_pointer_lock(LockEvent::PlatformReleased)
    }

    /// Takes any grab/ungrab effect owed to the desktop or web host, syncing against the latest
    /// tree first.
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
