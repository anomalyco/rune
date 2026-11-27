//! Bounded headless `--settle` readiness hook (`App::settle_ready`).

use std::time::Duration;

use super::App;

/// Timeout and readiness predicate installed by [`App::settle_ready`].
pub(crate) type SettleReady<S> = (Duration, Box<dyn FnMut(&S) -> bool>);

impl<S: 'static> App<S> {
    /// Waits for app-managed background work (such as tile rendering, image decoding, or async
    /// loads) during a headless `--settle` capture (`--snap --settle` / `--tree --settle`), up to
    /// `timeout`.
    ///
    /// At the capture frame, after layout callbacks settle, Rune checks `ready(&state)`. While it
    /// returns `false` and `timeout` has not elapsed, Rune drains queued [`AppHandle::update`]s,
    /// runs zero-dt frames (so [`AppSpec::poll`] and layout callbacks run without advancing
    /// virtual time), and wakes immediately when a background thread calls [`AppHandle::update`],
    /// [`AppHandle::wake`], or [`Waker::wake`](crate::Waker::wake).
    ///
    /// If `timeout` expires while `ready` is still `false`, capture proceeds so headless CI
    /// cannot hang, `--tree` records `"settle": { "ready": false, "timedOut": true, "waitMs": ... }`,
    /// and stderr prints a warning so a timed-out `--snap` is not mistaken for a settled one.
    /// Ignored when `--settle` is not passed and in windowed or web runs.
    ///
    /// ```no_run
    /// use std::time::Duration;
    /// use rune::{App, AppSpec, el};
    ///
    /// struct Doc { pending_tiles: usize }
    /// let spec = AppSpec::new("Doc", (800.0, 600.0), || Doc { pending_tiles: 0 }, |_, _| el());
    /// App::new(spec)
    ///     .settle_ready(Duration::from_secs(5), |doc| doc.pending_tiles == 0)
    ///     .run();
    /// ```
    pub fn settle_ready(
        mut self,
        timeout: Duration,
        ready: impl FnMut(&S) -> bool + 'static,
    ) -> Self {
        self.settle_ready = Some((timeout, Box::new(ready)));
        self
    }
}
