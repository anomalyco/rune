//! Native keyboard dispatch policy ([`KeyboardDispatch`]) and immediate zero-dt drain helper.

/// Controls when a native desktop window or key-taking overlay drains queued keyboard input.
///
/// By default ([`KeyboardDispatch::Batched`]), native hosts queue keyboard events into the
/// runtime action queue and drain them on the next display-paced frame. For latency-sensitive
/// native apps—where a control key needs to update authoritative state, send a command, or start
/// a state machine before the next vblank—[`KeyboardDispatch::Immediate`] drains queued keyboard
/// input immediately on receipt without acquiring, encoding, or presenting a drawable; visual
/// updates still wait for normal frame pacing.
///
/// Configure per window with [`App::keyboard_dispatch`](crate::App::keyboard_dispatch) or
/// [`WindowSpec::keyboard_dispatch`](crate::WindowSpec::keyboard_dispatch), and per key-taking
/// overlay with [`ConfiguredOverlay::keyboard_dispatch`](crate::ConfiguredOverlay::keyboard_dispatch).
///
/// # What immediate dispatch covers
///
/// - Native desktop windows (macOS, Windows, Linux X11/Wayland) and key-taking overlays
///   ([`KeyPolicy::OnDemand`](crate::KeyPolicy::OnDemand) /
///   [`KeyPolicy::Exclusive`](crate::KeyPolicy::Exclusive)).
/// - Non-synthetic keyboard events (presses, repeats, releases, macOS `Fn` and media keys),
///   direct IME commits, and committed `Ime::Commit` text. Ongoing IME preedit composition stays
///   ordered ahead of any subsequent key or commit in the runtime action queue.
/// - Drains the already-ordered runtime queue with a zero-dt runtime pass (plus bounded
///   same-turn follow-ups), then flushes clipboard writes, platform/IME state, haptics, and
///   window shell requests. Because the queue is consumed, the next paced draw never handles the
///   same key twice.
/// - Animations started from rest during an immediate step hold the frame clock at the receipt
///   time so the next paced frame does not charge the prior idle gap to newly started tracks;
///   animations that were already running keep real elapsed time.
///
/// # Environment override and kill switch
///
/// `RUNE_KEYBOARD_DISPATCH` overrides the configured builder value without a rebuild:
///
/// - `"immediate"`, `"1"`, `"true"`, `"yes"`, or `"on"` (ASCII case-insensitive) selects
///   [`KeyboardDispatch::Immediate`].
/// - `"batched"`, `"0"`, `"false"`, `"no"`, or `"off"` selects [`KeyboardDispatch::Batched`]
///   and acts as an unconditional kill switch over [`KeyboardDispatch::Immediate`].
/// - Unset, empty, or unrecognized values leave the builder value in effect.
///
/// # Where it does nothing
///
/// Ignored on web (`wasm32`), iOS, headless hosts, and overlays with
/// [`KeyPolicy::None`](crate::KeyPolicy::None).
///
/// ```no_run
/// use rune::{App, AppSpec, KeyboardDispatch, el};
///
/// App::new(AppSpec::new("Transport", (800.0, 520.0), || (), |_, _| el()))
///     .keyboard_dispatch(KeyboardDispatch::Immediate)
///     .run();
/// ```
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum KeyboardDispatch {
    /// Framework default: queue keyboard input and drain it on the next display-paced frame.
    #[default]
    Batched,
    /// Native hosts: drain queued keyboard input immediately on receipt without acquiring,
    /// encoding, or presenting a drawable; visual changes still wait for normal frame pacing.
    Immediate,
}

impl KeyboardDispatch {
    /// Environment variable overriding [`App::keyboard_dispatch`](crate::App::keyboard_dispatch),
    /// [`WindowSpec::keyboard_dispatch`](crate::WindowSpec::keyboard_dispatch), and
    /// [`ConfiguredOverlay::keyboard_dispatch`](crate::ConfiguredOverlay::keyboard_dispatch)
    /// without a rebuild.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) const ENV_VAR: &str = "RUNE_KEYBOARD_DISPATCH";

    /// Parses a `RUNE_KEYBOARD_DISPATCH` environment value.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn parse_env(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if trimmed.eq_ignore_ascii_case("immediate")
            || trimmed == "1"
            || trimmed.eq_ignore_ascii_case("true")
            || trimmed.eq_ignore_ascii_case("yes")
            || trimmed.eq_ignore_ascii_case("on")
        {
            Some(Self::Immediate)
        } else if trimmed.eq_ignore_ascii_case("batched")
            || trimmed == "0"
            || trimmed.eq_ignore_ascii_case("false")
            || trimmed.eq_ignore_ascii_case("no")
            || trimmed.eq_ignore_ascii_case("off")
        {
            Some(Self::Batched)
        } else {
            None
        }
    }

    /// Resolves the effective policy from this builder value and an optional
    /// `RUNE_KEYBOARD_DISPATCH` override. A recognized override wins over `self`.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn resolve(self, env: Option<&str>) -> Self {
        env.and_then(Self::parse_env).unwrap_or(self)
    }

    /// Effective policy for this host, resolving `RUNE_KEYBOARD_DISPATCH` over `self`.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn effective(self) -> Self {
        self.resolve(crate::env::var(Self::ENV_VAR).as_deref())
    }

    /// Whether immediate keyboard dispatch is enabled for this effective policy.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) const fn is_immediate(self) -> bool {
        matches!(self, Self::Immediate)
    }
}

/// Drains already-queued keyboard input with a zero-dt runtime step (plus bounded chained-input
/// follow-ups) without acquiring or presenting a drawable.
///
/// Preserves [`FrameClock`](crate::frame::clock::FrameClock) timing:
/// - If nothing was animating before the step and the step started motion, holds `clock` at
///   `now` so the next display-paced frame does not charge the prior idle gap to newly started
///   tracks.
/// - If motion was already running (`rt.animating || compositing`), leaves `clock` untouched so
///   running tracks keep real elapsed time on the next paced frame.
///
/// Returns `true` when queued input was drained, or `false` when `policy` is batched or no user
/// input is waiting in `rt`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn step_immediate_core(
    policy: KeyboardDispatch,
    rt: &mut crate::runtime::RuntimeCore,
    app: &mut dyn crate::runtime::AppDriver,
    clock: &mut crate::frame::clock::FrameClock,
    now: crate::time::Instant,
    compositing: bool,
) -> bool {
    if !policy.is_immediate() || !rt.has_input_waiting() {
        return false;
    }
    let was_animating = rt.animating || compositing;
    rt.hidden_timer_frame(app);
    let wake_clock = rt.polled_at();
    rt.drain_hidden_input(app, wake_clock);
    if !was_animating && rt.animating {
        clock.hold(now);
    }
    true
}

impl<S: 'static> crate::App<S> {
    /// Controls when this native window drains queued keyboard input ([`KeyboardDispatch`]).
    ///
    /// Defaults to [`KeyboardDispatch::Batched`], which queues keyboard events and drains them on
    /// the next display-paced frame. [`KeyboardDispatch::Immediate`] drains queued keyboard input
    /// immediately on receipt without acquiring, encoding, or presenting a drawable while keeping
    /// visual presentation paced to the display.
    ///
    /// `RUNE_KEYBOARD_DISPATCH` overrides this setting without a rebuild
    /// (`immediate`/`1`/`true`/`yes`/`on` => `Immediate`,
    /// `batched`/`0`/`false`/`no`/`off` => `Batched` kill switch). Ignored on web, iOS,
    /// and headless hosts.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, KeyboardDispatch, el};
    ///
    /// App::new(AppSpec::new("Transport", (800.0, 520.0), || (), |_, _| el()))
    ///     .keyboard_dispatch(KeyboardDispatch::Immediate)
    ///     .run();
    /// ```
    pub fn keyboard_dispatch(mut self, mode: KeyboardDispatch) -> Self {
        self.keyboard_dispatch = mode;
        self
    }
}

#[cfg(test)]
#[path = "keyboard_dispatch_tests.rs"]
mod tests;
