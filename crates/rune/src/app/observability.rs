//! Per-window frame and input observability hooks on [`App`].

use super::*;

impl<S: 'static> App<S> {
    /// Runs on the UI thread after each frame of the app's window, saying what it came to
    /// ([`FrameOutcome`](crate::FrameOutcome)): `Drawn` once it was encoded, submitted and
    /// presented, the first frame during creation and a hidden launcher's frames included;
    /// `Unchanged` for a frame only a timer asked for that drew what was on screen; `Skipped`
    /// for one a covered, minimised or occluded window, or a drawable that never came, kept
    /// from drawing. For logging frame timing: pair [`FrameEncoded::at`](crate::FrameEncoded::at)
    /// with `Tx::input_time` for input latency. It changes nothing about when or what the window
    /// draws; without it, each frame checks an `Option`. A single-window headless run reports
    /// every frame on its virtual clock as `Drawn`, with wall-clock instants and `refresh_hz` at
    /// its `--fps`, so an app's own meter reads its build and encode costs there too. Overlays,
    /// multi-window headless runs and the web never call it.
    pub fn on_frame_encoded(mut self, f: impl FnMut(&crate::FrameEncoded) + 'static) -> Self {
        self.on_frame_encoded = Some(Box::new(f));
        self
    }

    /// Runs on the UI thread in submission order once a `Drawn` frame actually reaches the screen
    /// (or is skipped by the system, reported as
    /// [`FramePresented::presented_at`](crate::FramePresented::presented_at) returning `None`).
    ///
    /// [`FramePresented::seq`](crate::FramePresented::seq) matches
    /// [`FrameEncoded::seq`](crate::FrameEncoded::seq) for the same `Drawn` frame, and
    /// `on_frame_encoded` always runs before `on_frame_presented` for a given sequence number.
    /// Pair them to measure real input-to-photon latency or detect missed vsyncs against
    /// [`FrameEncoded::presents_at`](crate::FrameEncoded::presents_at).
    ///
    /// Reports real presentation timestamps on macOS Metal (`MTLDrawable::addPresentedHandler`),
    /// Windows DX12/DXGI flip-model swap chains (`IDXGISwapChain::GetFrameStatistics` /
    /// `GetFrameStatisticsMedia`), and Linux Wayland compositors advertising `wp_presentation`.
    /// Platforms without a reliable scanout clock (Linux X11, Wayland without `wp_presentation`,
    /// Windows CPU-first / bitblt / unsupported DXGI, web, overlays, and headless runs) never
    /// call it instead of faking submit or target times. Without it, no platform hook is
    /// attached and each frame pays only an `Option` check; once in-flight frames land or
    /// settle, watches and poll timers clear automatically so an idle window never wakes.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, el};
    ///
    /// let spec = AppSpec::new("Demo", (320.0, 240.0), || (), |_, _| el());
    /// App::new(spec)
    ///     .on_frame_presented(|p| {
    ///         let _ = (p.seq(), p.window(), p.presented_at());
    ///     })
    ///     .run();
    /// ```
    pub fn on_frame_presented(mut self, f: impl FnMut(&crate::FramePresented) + 'static) -> Self {
        self.on_frame_presented = Some(Box::new(f));
        self
    }

    /// Runs on the UI thread after each `Drawn` frame of the app's window (desktop windows and
    /// the web runner, and never on unchanged or skipped frames), with the runtime as that frame
    /// presented it (`Runtime::elements`, `Runtime::state`), for an app that watches its own
    /// motion or waits for presented frames. Pair it with [`last_input`](crate::last_input) to
    /// tell motion the user caused from motion they didn't. Like `on_frame_encoded`, it changes
    /// nothing about when or what the window draws.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, el};
    ///
    /// let spec = AppSpec::new("Demo", (320.0, 240.0), || 0u32, |_, _| el());
    /// App::new(spec)
    ///     .on_frame_drawn(|rt| {
    ///         let _count = rt.elements().len();
    ///     })
    ///     .run();
    /// ```
    pub fn on_frame_drawn(mut self, f: impl FnMut(&crate::Runtime<S>) + 'static) -> Self {
        self.on_frame_drawn = Some(Box::new(f));
        self
    }

    /// Runs on the UI thread at each CPU wall-time stage boundary ([`FrameStage`](crate::FrameStage),
    /// [`StageBoundary`](crate::StageBoundary)) of this window's frames, nested runtime passes,
    /// glyph/atlas work, text setup warming, and system-font fallback lookups.
    ///
    /// Events inside a frame carry [`FrameStageEvent::seq`](crate::FrameStageEvent::seq) matching
    /// [`FrameEncoded::seq`](crate::FrameEncoded::seq) and
    /// [`FramePresented::seq`](crate::FramePresented::seq), and all stage events for a frame
    /// finish before `on_frame_encoded` runs for that frame. Out-of-frame work (window startup
    /// text setup, idle `Tx::prepare_text` warming, or event-turn fallback) reports `seq() == None`.
    ///
    /// Changes nothing about when or what the window draws; without it, every boundary checks one
    /// `Option` or thread-local flag and reads no clocks. Runs on desktop windows (GPU, CPU-first,
    /// and Windows drag-resize sync), web, and single-window headless runs.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, FrameStage, StageBoundary, el};
    ///
    /// let spec = AppSpec::new("Demo", (320.0, 240.0), || (), |_, _| el());
    /// App::new(spec)
    ///     .on_frame_stage(|e| {
    ///         if e.stage() == FrameStage::Acquire && e.boundary() == StageBoundary::End {
    ///             let _ = (e.seq(), e.at());
    ///         }
    ///     })
    ///     .run();
    /// ```
    pub fn on_frame_stage(mut self, f: impl FnMut(&crate::FrameStageEvent) + 'static) -> Self {
        self.observability.on_frame_stage = Some(Box::new(f));
        self
    }

    /// Runs on the UI thread when a native key event reaches this window, before Rune maps or
    /// routes it to focused controls or shortcuts.
    ///
    /// Reports only non-sensitive timing and routing metadata
    /// ([`WindowKeyEvent`](crate::WindowKeyEvent): receipt time, platform `input_time`,
    /// press/release, `repeat`, and whether a control had focus) — never key identity or text.
    /// Pair with [`App::on_frame_stage`], [`App::on_frame_encoded`], and
    /// [`App::on_frame_presented`] to separate OS/winit delivery, queueing, runtime/text work,
    /// and presentation latency. Zero-cost when unset.
    pub fn on_window_key(mut self, f: impl FnMut(&crate::WindowKeyEvent) + 'static) -> Self {
        self.observability.on_window_key = Some(Box::new(f));
        self
    }

    /// Runs on the UI thread when this window receives a native focus change, before Rune
    /// dispatches `App::on_focus` or `El::on_window_focus`.
    ///
    /// Unlike [`App::on_focus`] (a `fn` pointer for state updates), this accepts a read-only
    /// observability closure with the receipt timestamp and window key so telemetry stays
    /// independent of app logic. Zero-cost when unset.
    pub fn on_window_focus(mut self, f: impl FnMut(&crate::WindowFocusEvent) + 'static) -> Self {
        self.observability.on_window_focus = Some(Box::new(f));
        self
    }
}
