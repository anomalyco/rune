//! What other threads ask of an overlay, applied on its next event-loop turn, and the
//! requests that change a live window's native options.

use std::time::Duration;

use crate::el::Tx;

#[cfg(not(target_arch = "wasm32"))]
use super::OverlayCore;

pub(super) type Update<T> = Box<dyn FnOnce(&mut T, &mut Tx) -> bool + Send>;

/// `OverlayHandle::request_keys` or `release_keys`; the latest of the two in a turn wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum KeyRequest {
    Take,
    Release,
}

/// Takes the answer to `OverlayHandle::activation_token`.
#[cfg(not(target_arch = "wasm32"))]
pub(super) type TokenReply = Box<dyn FnOnce(Option<String>) + Send>;

/// Pending `OverlayHandle::activation_token` replies, plus whether the overlay is still alive
/// to answer them. Once `Closed` (when `OverlayCore` drops), late callers answer `None`
/// immediately instead of hanging forever.
#[cfg(not(target_arch = "wasm32"))]
pub(super) enum TokenQueue {
    Open(Vec<TokenReply>),
    Closed,
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for TokenQueue {
    fn default() -> Self { Self::Open(Vec::new()) }
}

#[cfg(not(target_arch = "wasm32"))]
impl TokenQueue {
    /// Queues `reply` while open, or returns it back when closed so the caller can invoke
    /// `reply(None)` outside the requests mutex.
    pub(super) fn push(&mut self, reply: TokenReply) -> Option<TokenReply> {
        match self {
            Self::Open(pending) => {
                pending.push(reply);
                None
            }
            Self::Closed => Some(reply),
        }
    }

    /// Takes every queued reply while keeping the queue in its current state.
    pub(super) fn take(&mut self) -> Vec<TokenReply> {
        match self {
            Self::Open(pending) => std::mem::take(pending),
            Self::Closed => Vec::new(),
        }
    }

    /// Transitions to `Closed` and returns every previously queued reply.
    pub(super) fn close(&mut self) -> Vec<TokenReply> {
        match std::mem::replace(self, Self::Closed) {
            Self::Open(pending) => pending,
            Self::Closed => Vec::new(),
        }
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Open(pending) => pending.len(),
            Self::Closed => 0,
        }
    }
}

/// A show or a hide, fading the window's alpha over its duration (zero: at once; see
/// `OverlayHandle::fade_in` and `fade_out` for where windows fade).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Visibility {
    Show(Duration),
    Hide(Duration),
}

#[derive(Default)]
pub(super) struct Requests {
    pub input: Option<crate::OverlayInput>,
    pub follow_pointer: Option<bool>,
    pub placement: Option<Option<crate::OverlayPlacement>>,
    pub display: Option<crate::DisplayTarget>,
    /// `request_keys` or `release_keys`: the latest wins.
    pub keys: Option<KeyRequest>,
    /// Move to the display under the pointer now (`OverlayHandle::refollow_pointer`).
    pub refollow: bool,
    /// The latest show or hide wins.
    pub visibility: Option<Visibility>,
    pub edge: Option<super::ScreenEdge>,
    pub margin: Option<f32>,
    pub align: Option<(crate::Align, f32)>,
    pub size: Option<(f32, f32)>,
    /// The content's new height, for an overlay that fits its content and is on screen.
    #[cfg(not(target_arch = "wasm32"))]
    pub fit_height: Option<f32>,
    pub macos_level: Option<isize>,
    pub window_backdrop: Option<crate::WindowBackdrop>,
    pub exclude_from_capture: Option<bool>,
    pub xwayland_input: Option<bool>,
    /// `OverlayHandle::activation_token` replies, answered before this turn's show or hide,
    /// while the surface may still have the keys.
    #[cfg(not(target_arch = "wasm32"))]
    pub tokens: TokenQueue,
    /// The `setup` waker fired.
    #[cfg(not(target_arch = "wasm32"))]
    pub woken: bool,
}

#[cfg(test)]
impl Requests {
    /// Whether the latest request shows (`Some(true)`) or hides the overlay.
    pub fn visible(&self) -> Option<bool> { self.visibility.map(|v| matches!(v, Visibility::Show(_))) }
}

#[cfg(not(target_arch = "wasm32"))]
impl OverlayCore {
    /// None off Wayland's layer shell, while the overlay is not shown, or while it has no surface.
    pub(super) fn activation_token(&self) -> Option<String> {
        if !self.presence.shown() { return None; }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(super::Live { backing: super::Backing::Layer(layer), .. }) = self.surface.live() {
            return layer.activation_token();
        }
        None
    }

    /// Answers every queued `OverlayHandle::activation_token` reply once using the live surface
    /// while shown (or `None` otherwise), outside the requests mutex.
    pub(super) fn answer_tokens(&self) {
        let pending = self.shared.requests.lock().unwrap_or_else(std::sync::PoisonError::into_inner).tokens.take();
        for reply in pending { reply(self.activation_token()); }
    }

    /// Answers every queued `OverlayHandle::activation_token` reply once with `None`, outside
    /// the requests mutex (used when tearing down a closed/refused surface or finishing a hide).
    pub(super) fn drain_tokens_none(&self) {
        let pending = self.shared.requests.lock().unwrap_or_else(std::sync::PoisonError::into_inner).tokens.take();
        for reply in pending { reply(None); }
    }

    /// Marks the overlay dropped and answers every queued `OverlayHandle::activation_token`
    /// reply once with `None`; later calls on any surviving handle answer `None` immediately.
    pub(super) fn close_tokens(&self) {
        let pending = self.shared.requests.lock().unwrap_or_else(std::sync::PoisonError::into_inner).tokens.close();
        for reply in pending { reply(None); }
    }

    /// Keeps the window out of screen recordings, or lets recorders see it again. A window
    /// made later (Wayland recreates on hide) starts from the same option.
    pub(super) fn set_capture_exclusion(&mut self, exclude: bool) {
        self.options.exclude_from_capture = exclude;
        #[cfg(target_os = "macos")]
        let support = self.panel.as_ref().map(|panel| panel.set_capture_exclusion(exclude));
        #[cfg(target_os = "windows")]
        let support = self.native.as_mut().map(|native| native.set_capture_exclusion(exclude));
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let support: Option<crate::OverlaySupport> = None;
        if let Some(support) = support { self.shared.native.lock().unwrap().1.capture_exclusion = support; }
    }
}
