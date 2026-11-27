//! What other threads ask of an overlay, applied on its next event-loop turn, and the
//! requests that change a live window's native options.

use std::time::Duration;

use crate::el::Tx;
#[cfg(not(target_arch = "wasm32"))]
use crate::time::Instant;

#[cfg(not(target_arch = "wasm32"))]
use super::OverlayCore;

pub(super) type Update<T> = Box<dyn FnOnce(&mut T, &mut Tx) -> bool + Send>;

/// One-shot callback queued through `OverlayHandle::after_hidden`.
pub(super) type AfterHidden<T> = Box<dyn FnOnce(&mut T, &mut Tx) + Send>;

/// Pending `OverlayHandle::after_hidden` callbacks, plus whether the overlay is still open to
/// receive them. Once `Closed` (when the overlay closes, terminates, or drops, or on a
/// disconnected web handle), late callers drop `f` outside the mutex unrun.
pub(super) enum AfterHiddenQueue<T> {
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    Open(Vec<AfterHidden<T>>),
    Closed,
}

impl<T> Default for AfterHiddenQueue<T> {
    fn default() -> Self { Self::Open(Vec::new()) }
}

impl<T> AfterHiddenQueue<T> {
    /// Queues `f` while open, or returns it back when closed so the caller drops it outside the
    /// mutex without running it.
    pub(super) fn push(&mut self, f: AfterHidden<T>) -> Option<AfterHidden<T>> {
        match self {
            Self::Open(pending) => {
                pending.push(f);
                None
            }
            Self::Closed => Some(f),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn take(&mut self) -> Vec<AfterHidden<T>> {
        match self {
            Self::Open(pending) => std::mem::take(pending),
            Self::Closed => Vec::new(),
        }
    }

    pub(super) fn close(&mut self) -> Vec<AfterHidden<T>> {
        match std::mem::replace(self, Self::Closed) {
            Self::Open(pending) => pending,
            Self::Closed => Vec::new(),
        }
    }
}

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
/// to answer them. Once `Closed` (when `OverlayCore` drops, terminates, or enters terminal GPU
/// loss), late callers answer `None` immediately instead of hanging forever.
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

/// Source polled without blocking for one committed `OverlayHandle::activation_token` request.
#[cfg(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
pub(in crate::overlay) enum TokenSource {
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    Wayland(crate::platform::layer_shell::PendingActivationToken),
    #[cfg(test)]
    Mock(Box<dyn FnMut() -> crate::platform::layer_shell_machine::activation::TokenPoll>),
}

#[cfg(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
impl TokenSource {
    fn dispatch(&mut self) -> crate::platform::layer_shell_machine::activation::TokenPoll {
        match self {
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            Self::Wayland(token) => token.dispatch(),
            #[cfg(test)]
            Self::Mock(poll) => poll(),
        }
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    fn is_live(&self) -> bool {
        match self {
            Self::Wayland(token) => token.is_live(),
            #[cfg(test)]
            Self::Mock(_) => true,
        }
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    fn has_pending_events(&self) -> bool {
        match self {
            Self::Wayland(token) => token.has_pending_events(),
            #[cfg(test)]
            Self::Mock(_) => false,
        }
    }
}

#[cfg(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
struct InFlightToken {
    source: TokenSource,
    state: crate::platform::layer_shell_machine::activation::State,
    reply: Option<TokenReply>,
}

/// Owned subsystem state for in-flight non-blocking `OverlayHandle::activation_token` requests
/// inside [`OverlayCore`]. Independent of `LayerSurface` so hiding the overlay or releasing
/// exclusive keyboard focus in the same turn never blocks or cancels committed token replies.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
pub(in crate::overlay) struct ActivationTokens {
    #[cfg(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
    in_flight: Vec<InFlightToken>,
}

#[cfg(not(target_arch = "wasm32"))]
impl ActivationTokens {
    #[cfg(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
    fn push(&mut self, source: TokenSource, reply: TokenReply, now: Instant) {
        use crate::platform::layer_shell_machine::activation::{State, TIMEOUT};
        self.in_flight.push(InFlightToken {
            source,
            state: State::Pending { deadline: now + TIMEOUT },
            reply: Some(reply),
        });
    }

    pub(super) fn deadline(&self) -> Option<Instant> {
        #[cfg(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
        return self.in_flight.iter().filter_map(|token| token.state.deadline()).min();
        #[cfg(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))))]
        None
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    fn has_live_wayland(&self) -> bool {
        self.in_flight.iter().any(|token| token.state.is_pending() && token.source.is_live())
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    fn has_pending_wayland_events(&self) -> bool {
        self.in_flight.iter().any(|token| token.state.is_pending() && token.source.has_pending_events())
    }

    #[cfg(test)]
    pub(super) fn in_flight_len(&self) -> usize {
        self.in_flight.len()
    }

    fn poll(&mut self, now: Instant) -> Vec<(TokenReply, Option<String>)> {
        #[cfg(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
        {
            use crate::platform::layer_shell_machine::activation::{Effect, Event, TokenPoll, step};
            if self.in_flight.is_empty() {
                return Vec::new();
            }
            let mut completed = Vec::new();
            let mut remaining = Vec::with_capacity(self.in_flight.len());
            for mut entry in std::mem::take(&mut self.in_flight) {
                let event = match entry.source.dispatch() {
                    TokenPoll::Done(Some(token)) => Event::Done(token),
                    TokenPoll::Done(None) => Event::Failed,
                    TokenPoll::Pending => Event::Tick { now },
                };
                for effect in step(&mut entry.state, event) {
                    let Effect::Complete(answer) = effect;
                    if let Some(reply) = entry.reply.take() {
                        completed.push((reply, answer));
                    }
                }
                if entry.state.is_pending() {
                    remaining.push(entry);
                }
            }
            self.in_flight = remaining;
            return completed;
        }
        #[cfg(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))))]
        {
            let _ = now;
            Vec::new()
        }
    }

    fn cancel_all(&mut self) -> Vec<TokenReply> {
        #[cfg(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
        {
            use crate::platform::layer_shell_machine::activation::{Effect, Event, step};
            let mut cancelled = Vec::with_capacity(self.in_flight.len());
            for mut entry in std::mem::take(&mut self.in_flight) {
                for Effect::Complete(_) in step(&mut entry.state, Event::Cancel) {
                    if let Some(reply) = entry.reply.take() {
                        cancelled.push(reply);
                    }
                }
            }
            return cancelled;
        }
        #[cfg(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))))]
        Vec::new()
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
    /// The content's new size, for an overlay that fits its content and is on screen.
    #[cfg(not(target_arch = "wasm32"))]
    pub fit_size: Option<(f32, f32)>,
    pub macos_level: Option<isize>,
    pub window_backdrop: Option<crate::WindowBackdrop>,
    pub exclude_from_capture: Option<bool>,
    pub xwayland_input: Option<bool>,
    /// `OverlayHandle::activation_token` replies, started before this turn's show, hide, or
    /// `release_keys`, while the surface may still have the keys.
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
    /// Starts every activation-token request in `pending` on the live surface while shown
    /// (non-blocking on Wayland layer-shell) or answers it immediately with `None` otherwise.
    pub(super) fn start_tokens(&mut self, pending: Vec<TokenReply>, now: Instant) {
        if pending.is_empty() { return; }
        let mut immediate = Vec::new();
        for reply in pending {
            if !self.presence.shown() {
                immediate.push(reply);
                continue;
            }
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            if let Some(token) = match self.surface.live() {
                Some(super::Live { backing: super::Backing::Layer(layer), .. }) => layer.start_activation_token(),
                _ => self.pending_layer.layer().and_then(|layer| layer.start_activation_token()),
            } {
                self.activation_tokens.push(TokenSource::Wayland(token), reply, now);
                continue;
            }
            #[cfg(test)]
            if let Some(factory) = &mut self.test_activation_source
                && let Some(source) = factory()
            {
                self.activation_tokens.push(source, reply, now);
                continue;
            }
            immediate.push(reply);
        }
        #[cfg(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))))]
        let _ = now;
        for reply in immediate { reply(None); }
    }

    /// Takes every queued `OverlayHandle::activation_token` reply and starts it while the
    /// surface is still shown and live (or answers `None` otherwise), outside the requests mutex.
    pub(super) fn start_queued_tokens(&mut self, now: Instant) {
        let pending = self.shared.requests.lock().unwrap_or_else(std::sync::PoisonError::into_inner).tokens.take();
        self.start_tokens(pending, now);
    }

    /// Dispatches in-flight non-blocking activation-token replies without reading the Wayland
    /// socket and expires any request past its fallback deadline, running finished callbacks
    /// outside all locks.
    pub(super) fn poll_activation_tokens(&mut self, now: Instant) {
        let completed = self.activation_tokens.poll(now);
        for (reply, token) in completed {
            reply(token);
        }
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) fn has_live_wayland_activation_tokens(&self) -> bool {
        self.activation_tokens.has_live_wayland()
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    pub(super) fn has_pending_wayland_activation_token_events(&self) -> bool {
        self.activation_tokens.has_pending_wayland_events()
    }

    /// Answers every queued `OverlayHandle::activation_token` reply once with `None`, outside
    /// the requests mutex (used when tearing down a closed/refused surface or finishing a hide).
    /// Already-committed in-flight Wayland token requests keep waiting for their `done` reply.
    pub(super) fn drain_tokens_none(&self) {
        let pending = self.shared.requests.lock().unwrap_or_else(std::sync::PoisonError::into_inner).tokens.take();
        for reply in pending { reply(None); }
    }

    /// Marks the overlay closed for token requests, cancels every in-flight Wayland token
    /// request, and answers all in-flight and queued replies once with `None`; later calls on
    /// any surviving handle answer `None` immediately.
    pub(super) fn close_tokens(&mut self) {
        let queued = self.shared.requests.lock().unwrap_or_else(std::sync::PoisonError::into_inner).tokens.close();
        let in_flight = self.activation_tokens.cancel_all();
        for reply in in_flight { reply(None); }
        for reply in queued { reply(None); }
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
