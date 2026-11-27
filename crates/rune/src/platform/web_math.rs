//! The arithmetic behind the browser shell: the canvas's size in device pixels, when a resize
//! needs the surface configured again, and the band a page's canvas covers. Kept out of
//! `platform/web.rs`, which only builds on `wasm32`, so the tests run headless.
#![cfg_attr(not(test), allow(dead_code))]

pub(crate) mod cpu_canvas;
pub(crate) mod gpu_recovery;
pub(crate) mod handover_machine;
pub(crate) mod platform_events;

/// The canvas's size in device pixels. `css` is the canvas's CSS box and `reported` what winit
/// reported for it. A canvas reports pixels, but an emulated device scale reports CSS pixels, so
/// where a side disagrees with the canvas's own box the box wins. A window that fits its content
/// (`fit_height`) keeps the reported height: the app measures its content and sets the height
/// itself, so the box is only as tall as the last frame's content.
pub(crate) fn fix_canvas_size(css: (f32, f32), reported: (u32, u32), spec: (f32, f32), scale: f32, fit_height: bool) -> (u32, u32) {
    let scale = scale.max(1e-3);
    let want = |css: f32| (css * scale).round().max(1.0) as u32;
    // Within a pixel of the box the browser's own number is the box's: it knows the device
    // grid, and a fractional CSS box rounds by a fraction of a pixel either way.
    let fixed = |got: u32, css: f32| {
        if css > 0.0 && (got as f32 - want(css) as f32).abs() > scale.ceil() + 1.0 { want(css) } else { got }
    };
    let (w, h) = css;
    let mut size = reported;
    if w > 0.0 { size.0 = fixed(size.0, w); }
    if !fit_height && h > 0.0 { size.1 = fixed(size.1, h); }
    if size.0 == 0 { size.0 = if w > 0.0 { want(w) } else { want(spec.0) }; }
    if size.1 == 0 { size.1 = if !fit_height && h > 0.0 { want(h) } else { want(spec.1) }; }
    size
}

/// The surface's next size and whether it needs configuring again. `current` is the size it is
/// configured at. A page (`document`) never shrinks its surface: a phone's toolbar expanding
/// shrinks the viewport for a moment mid-scroll, and a web surface configured again destroys and
/// reallocates its swapchain textures in the GPU process right then.
pub(crate) fn should_reconfigure_surface(current: (u32, u32), next: (u32, u32), document: bool) -> ((u32, u32), bool) {
    if next.0 == 0 || next.1 == 0 || next == current { return (current, false); }
    if document && next.0 == current.0 && next.1 < current.1 { return (current, false); }
    (next, true)
}

/// The surface format, optional `view_formats` entry, and render-target format for a web canvas.
///
/// Mirrors native `window::surface::surface_config`:
/// - `srgb_blend == true`: wants a non-sRGB render target (`remove_srgb_suffix`) so the target
///   writes and blends already sRGB-encoded values without encoding a second time.
/// - `srgb_blend == false`: wants an `*Srgb` render target (`add_srgb_suffix`) so the target
///   encodes linear shader outputs once on write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SurfaceFormats {
    pub surface: wgpu::TextureFormat,
    pub view_format: Option<wgpu::TextureFormat>,
    pub render: wgpu::TextureFormat,
}

impl SurfaceFormats {
    /// Whether the renderer must encode sampled sRGB images in the shader (non-sRGB target).
    pub(crate) fn srgb_blend(self) -> bool {
        !self.render.is_srgb()
    }
}

/// Chooses the canvas configuration format and render target format for `srgb_blend`.
///
/// Prefers configuring the surface directly in the wanted format when `supported` lists it
/// (e.g. WebGL2 exposes both `Rgba8UnormSrgb` and `Rgba8Unorm`). Otherwise keeps `preferred`
/// and exposes the wanted twin via `view_format` on backends that support view-format aliasing
/// (WebGPU canvases configure only non-sRGB formats and accept sRGB view formats).
pub(crate) fn select_surface_formats(
    preferred: wgpu::TextureFormat,
    supported: &[wgpu::TextureFormat],
    backend: wgpu::Backend,
    srgb_blend: bool,
) -> SurfaceFormats {
    let wanted = if srgb_blend {
        preferred.remove_srgb_suffix()
    } else {
        preferred.add_srgb_suffix()
    };
    if wanted == preferred {
        return SurfaceFormats { surface: preferred, view_format: None, render: preferred };
    }
    if supported.contains(&wanted) {
        return SurfaceFormats { surface: wanted, view_format: None, render: wanted };
    }
    if backend != wgpu::Backend::Gl {
        return SurfaceFormats { surface: preferred, view_format: Some(wanted), render: wanted };
    }
    SurfaceFormats { surface: preferred, view_format: None, render: preferred }
}

/// Surface texture usages for a web canvas.
///
/// WebGPU canvases accept `COPY_DST` (for retained loop-only frames) and `COPY_SRC` (for backdrop
/// copies in `ActivePass::split` and drift readback) even though wgpu's capabilities report only
/// `RENDER_ATTACHMENT`. WebGL2 runs through `wgpu-core`, which rejects any usage absent from
/// `caps_usages`.
pub(crate) fn surface_usage(
    caps_usages: wgpu::TextureUsages,
    backend: wgpu::Backend,
) -> wgpu::TextureUsages {
    let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT
        | (caps_usages & wgpu::TextureUsages::COPY_SRC);
    if backend == wgpu::Backend::BrowserWebGpu {
        usage |= wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC;
    }
    usage
}

/// Whether `backend` requires explicit `Device::poll` calls to deliver `on_submitted_work_done`
/// and `map_async` callbacks. WebGPU delivers them from browser promises (and `Device::poll` is a
/// no-op there); `wgpu-core`'s WebGL2 backend only fires them when polled.
pub(crate) fn needs_device_poll(backend: wgpu::Backend) -> bool {
    backend != wgpu::Backend::BrowserWebGpu
}

/// GPU backends the web host can attempt for a canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WebBackend {
    WebGpu,
    WebGl2,
}

/// Adapter options for a web GPU backend (`WebGPU` with `compatible_surface: None`, or `WebGL2`
/// with `compatible_surface: Some(&surface)`).
///
/// Leaves `power_preference` at [`wgpu::PowerPreference::None`] so `navigator.gpu.requestAdapter`
/// omits `powerPreference` and lets the browser choose the GPU (e.g. Chromium picks `LowPower` on
/// battery and `HighPerformance` on AC power, avoids forcing an integrated GPU that cannot import
/// textures across to a discrete display GPU on Linux, and avoids Chrome's Windows console warning
/// for explicit `powerPreference`).
pub(crate) fn adapter_options<'a, 'b>(
    compatible_surface: Option<&'a wgpu::Surface<'b>>,
) -> wgpu::RequestAdapterOptions<'a, 'b> {
    wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::None,
        compatible_surface,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }
}

/// Backends to attempt in order, given whether `navigator.gpu` is exposed and whether the
/// `webgl` cargo feature is enabled.
///
/// When `navigator.gpu` is absent (insecure origin, WebGPU disabled by flag while
/// `canvas.getContext("webgpu")` still exists, or a browser without WebGPU), WebGPU is skipped
/// so the canvas is never locked to a `"webgpu"` context before WebGL2 runs. When `navigator.gpu`
/// is present, WebGPU probes for an adapter and device before creating a canvas surface, leaving
/// the canvas free for WebGL2 if either fails.
pub(crate) fn backend_order(has_navigator_gpu: bool, webgl_enabled: bool) -> &'static [WebBackend] {
    match (has_navigator_gpu, webgl_enabled) {
        (true, true) => &[WebBackend::WebGpu, WebBackend::WebGl2],
        (true, false) => &[WebBackend::WebGpu],
        (false, true) => &[WebBackend::WebGl2],
        (false, false) => &[],
    }
}

/// How the web host handles a lost GPU device or WebGL2 context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LossAction {
    /// Fall back to the CPU 2D canvas under the GPU canvas. `recoverable` is true for WebGL2
    /// context loss, where a later `webglcontextrestored` hands back over to WebGL2.
    FallbackToCpu { recoverable: bool },
    /// Drop the lost GPU side and pause rendering until `webglcontextrestored` rebuilds it.
    PauseForRestore,
    /// Show the fatal GPU-lost message and exit the loop.
    Fatal,
    /// `GpuLossPolicy::Terminal`: keep the page and canvas alive in the DOM, drop the GPU side,
    /// refuse CPU fallback and WebGL2 restore, publish `window_error`, and wake `poll` once.
    TerminalLatch,
}

/// Whether the `webglcontextlost` listener should call `event.preventDefault()` so the browser
/// can later fire `webglcontextrestored`.
pub(crate) const fn webgl_prevent_default(policy: crate::GpuLossPolicy) -> bool {
    !matches!(policy, crate::GpuLossPolicy::Terminal)
}

/// Decides how the web host responds when the GPU device or WebGL2 context is lost under
/// `policy`.
pub(crate) fn loss_action_for_policy(
    policy: crate::GpuLossPolicy,
    is_webgl: bool,
    gpu_first: bool,
    embedded: bool,
    document: bool,
) -> LossAction {
    if matches!(policy, crate::GpuLossPolicy::Terminal) {
        return LossAction::TerminalLatch;
    }
    loss_action(is_webgl, gpu_first, embedded, document)
}

/// Decides how the web host responds when the GPU device or WebGL2 context is lost under the
/// default `GpuLossPolicy::Recover` policy (while bounded recovery is still available).
///
/// WebGL2 context loss is recoverable when `webglcontextrestored` fires: full-window pages
/// (`!embedded && !document`) fall back to the CPU canvas immediately so they are never blank
/// while waiting (or if restore never arrives), while inline and document pages pause in place
/// until restore.
///
/// WebGPU device loss mirrors that split under bounded `WebGpuRecovery` (3 losses per 5-minute
/// window): full-window CPU-first pages fall back to the CPU canvas while re-acquiring, while
/// `GpuFirst` (`CpuFirst::Off`, inline, and document) pages pause in place (`Draw::ContextLost`)
/// until re-acquisition succeeds or exhausts its retry budget.
pub(crate) fn loss_action(is_webgl: bool, gpu_first: bool, embedded: bool, document: bool) -> LossAction {
    if is_webgl {
        if !embedded && !document {
            LossAction::FallbackToCpu { recoverable: true }
        } else {
            LossAction::PauseForRestore
        }
    } else if gpu_first || embedded || document {
        LossAction::PauseForRestore
    } else {
        LossAction::FallbackToCpu { recoverable: true }
    }
}

/// Web draw state reduced to the facts `WebFrameHost::gate` needs to decide whether a frame runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WebDrawKind {
    Cpu,
    HandingOver,
    Gpu,
    ContextLost,
    TerminalFailed,
    Moving,
}

/// Decides whether a web frame skips, steps before acquiring (`Gate::QuietTimer`), or draws
/// (`Gate::Draw`). `HandingOver` always draws so CPU animations stay live during `WaitingDone`.
pub(crate) fn web_frame_gate(
    suspended: bool,
    draw: WebDrawKind,
    quiet_candidate: bool,
) -> crate::frame::host_step::Gate {
    if suspended || matches!(draw, WebDrawKind::ContextLost | WebDrawKind::TerminalFailed | WebDrawKind::Moving) {
        crate::frame::host_step::Gate::Skip
    } else if draw == WebDrawKind::Gpu && quiet_candidate {
        crate::frame::host_step::Gate::QuietTimer
    } else {
        crate::frame::host_step::Gate::Draw
    }
}

/// Whether a web wake (due timer or app wake) runs `covered_wake` directly instead of calling
/// `request_redraw`: browsers pause `requestAnimationFrame` while a tab is hidden, and a
/// zero-size suspended canvas skips drawing in [`web_frame_gate`].
#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
pub(crate) fn web_wake_covered(hidden: bool, suspended: bool) -> bool {
    hidden || suspended
}

/// Next timer wait target on the web while hidden or suspended: floors overdue/zero timers by
/// one frame interval after `polled_at` so `wake_after(Duration::ZERO)` never spins.
#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
pub(crate) fn web_hidden_timer_target(deadline: f64, polled_at: f64) -> f64 {
    deadline.max(polled_at + crate::window::TIMER_FRAME.as_secs_f64())
}

/// Whether a presented GPU frame can be recorded in [`crate::window::Presented`] so a later
/// redraw that builds the same pixels skips acquiring and presenting a canvas texture.
pub(crate) fn should_record_presented(
    drawn: bool,
    moving: bool,
    stale: bool,
    has_window_frame: bool,
) -> bool {
    drawn && !moving && !stale && !has_window_frame
}

/// Outcome of one `wgpu::Surface::get_current_texture` call without the texture payload, so
/// acquisition policy can be tested off `wasm32`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WebAcquireStatus {
    Ready,
    Timeout,
    Occluded,
    OutdatedOrLost,
    Validation,
}

/// Action taken after `get_current_texture` returns [`WebAcquireStatus`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WebAcquireAction {
    UseTexture,
    ReconfigureAndRetry,
    Miss(crate::window::Missed),
}

/// Maps a canvas texture acquisition status and remaining reconfigure retries to an action.
///
/// Transient `Timeout` and `Occluded` statuses return missed frames instead of terminating the
/// app; `Outdated` and `Lost` reconfigure once before becoming fatal `Missed::Unconfigured`.
pub(crate) fn web_acquire_action(status: WebAcquireStatus, retries_left: u8) -> WebAcquireAction {
    use crate::window::Missed;
    match status {
        WebAcquireStatus::Ready => WebAcquireAction::UseTexture,
        WebAcquireStatus::Timeout => WebAcquireAction::Miss(Missed::Timeout),
        WebAcquireStatus::Occluded => WebAcquireAction::Miss(Missed::Occluded),
        WebAcquireStatus::OutdatedOrLost if retries_left > 0 => WebAcquireAction::ReconfigureAndRetry,
        WebAcquireStatus::OutdatedOrLost | WebAcquireStatus::Validation => {
            WebAcquireAction::Miss(Missed::Unconfigured)
        }
    }
}

/// How the web host responds inside `FrameHost::on_missed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WebMissAction {
    /// Canvas acquisition failed fatally: report the error and end the event loop.
    Fatal,
    /// Surface is occluded: step covered work if unstepped, without requesting another rAF.
    CoveredOnly,
    /// Transient miss: step covered work if unstepped and request a redraw on the next rAF.
    RetryNextFrame,
}

/// Classifies a missed web frame for `WebFrameHost::on_missed`.
pub(crate) fn web_miss_action(missed: crate::window::Missed) -> WebMissAction {
    use crate::window::Missed;
    match missed {
        Missed::Unconfigured => WebMissAction::Fatal,
        Missed::Occluded => WebMissAction::CoveredOnly,
        Missed::Timeout => WebMissAction::RetryNextFrame,
        #[cfg(not(target_arch = "wasm32"))]
        Missed::Outdated => WebMissAction::RetryNextFrame,
    }
}

/// Whether `WebFrameHost::post_step_check` should drop the acquired drawable and reblend.
///
/// Skips re-acquiring when this frame already tried reblending for `rt_srgb` (for example on a
/// WebGL2 surface without an sRGB twin where the chosen format cannot match `rt_srgb`).
pub(crate) fn should_post_step_reblend(
    renderer_srgb: bool,
    rt_srgb: bool,
    reblend_tried: Option<bool>,
) -> bool {
    renderer_srgb != rt_srgb && reblend_tried != Some(rt_srgb)
}

/// How a browser represents `Element.scrollLeft` on a `direction: rtl` container.
///
/// Matches `detectRtlScrollType()` in `platform/web/bridge.js`:
/// - `Negative` (CSSOM View standard: Chromium >= 85, Firefox, Safari >= 15.4): `0` at the right
///   (start) edge, `-max` at the left (end) edge.
/// - `Reverse` (legacy Edge/IE): `0` at the right (start) edge, `+max` at the left (end) edge.
/// - `Default` (legacy Blink/WebKit): `+max` at the right (start) edge, `0` at the left (end) edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RtlScrollModel {
    Negative,
    Reverse,
    Default,
}

/// Converts a DOM `Element.scrollLeft` reading into Rune's logical horizontal scroll offset
/// (`0..=max_scroll`, measuring distance from the inline-start edge).
pub(crate) fn scroll_x_from_dom(
    scroll_left: f32,
    max_scroll: f32,
    rtl: bool,
    model: RtlScrollModel,
) -> f32 {
    if !scroll_left.is_finite() || !max_scroll.is_finite() {
        return 0.0;
    }
    let max = max_scroll.max(0.0);
    let logical = if !rtl {
        scroll_left
    } else {
        match model {
            RtlScrollModel::Negative => -scroll_left,
            RtlScrollModel::Reverse => scroll_left,
            RtlScrollModel::Default => max - scroll_left,
        }
    };
    logical.clamp(0.0, max)
}

/// Converts Rune's logical horizontal scroll offset (`0..=max_scroll`) into the DOM
/// `Element.scrollLeft` value expected by the browser's RTL scroll model.
pub(crate) fn scroll_x_to_dom(
    offset: f32,
    max_scroll: f32,
    rtl: bool,
    model: RtlScrollModel,
) -> f32 {
    if !offset.is_finite() || !max_scroll.is_finite() {
        return 0.0;
    }
    let max = max_scroll.max(0.0);
    let clamped = offset.clamp(0.0, max);
    if !rtl {
        clamped
    } else {
        match model {
            RtlScrollModel::Negative => if clamped == 0.0 { 0.0 } else { -clamped },
            RtlScrollModel::Reverse => clamped,
            RtlScrollModel::Default => max - clamped,
        }
    }
}

/// Builds the web host's `Runtime` and splits off its shared state, compositor, frame-drawn
/// hook and fonts. Kept out of `platform/web/app.rs` so web startup configuration runs in
/// native unit tests.
pub(crate) fn start_web_runtime<S: 'static>(
    mut app: crate::App<S>,
    viewport: (f32, f32),
    env: crate::Env,
    wake: crate::Waker,
    live_wake: crate::Waker,
) -> (
    crate::Runtime<S>,
    std::sync::Arc<crate::app::Shared<S>>,
    Option<Box<dyn crate::compose::Compositor<S>>>,
    Option<crate::frame::encoded::OnFrameDrawn<S>>,
    &'static [&'static [u8]],
) {
    let compositor = app.compositor.take();
    let on_frame_drawn = app.on_frame_drawn.take();
    let (playground, attach_playground) = (app.playground.take(), app.attach_playground);
    let spec_fonts = app.spec.fonts;
    let (spec, state, shared, runtime) = app.into_parts(None);
    let mut rt = crate::tooling::runtime_for_configured(&spec, state, viewport, env, wake, Some(runtime));
    // Live values wake on their own lane so a value already drawn in the frame that published
    // it does not trigger Wake::App's invalidate and redundant redraw.
    rt.set_live_waker(live_wake);
    if let Some(attach) = attach_playground {
        attach(&mut rt, playground, &spec, None, None, true);
    }
    rt.core_mut().set_touch_hold_policy(crate::runtime::TouchHoldPolicy::ConfirmingTimer);
    rt.track_marks = compositor.is_some();
    (rt, shared, compositor, on_frame_drawn, spec_fonts)
}

/// Chooses the web touch hold policy from whether the browser dispatches live touch
/// `contextmenu` while the finger is still down (Android, Linux, ChromeOS) versus deferring it
/// to lift or omitting it (Windows, iOS/iPadOS, macOS).
pub(crate) fn web_touch_hold_policy(live_contextmenu: bool) -> crate::runtime::TouchHoldPolicy {
    if live_contextmenu {
        crate::runtime::TouchHoldPolicy::HostContextMenu
    } else {
        crate::runtime::TouchHoldPolicy::ConfirmingTimer
    }
}

/// Maps a browser `PointerEvent.timeStamp` (milliseconds since `timeOrigin`) onto the runtime
/// clock (`rt_now`, seconds since `wall_start`). Clamps future timestamps to `perf_now_ms`, and
/// falls back to `rt_now` (delivery time) when either browser reading is malformed or non-positive.
pub(crate) fn web_touch_event_time(rt_now: f64, event_ts_ms: f64, perf_now_ms: f64) -> f64 {
    if !event_ts_ms.is_finite() || event_ts_ms <= 0.0 || !perf_now_ms.is_finite() || perf_now_ms <= 0.0 {
        return rt_now;
    }
    let clamped = event_ts_ms.min(perf_now_ms);
    rt_now - (perf_now_ms - clamped) / 1000.0
}

/// Whether a resize's frame waits in a queued microtask.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ResizeRedraw {
    #[default]
    Idle,
    Queued,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResizeRedrawEvent {
    /// winit's ResizeObserver or scale callback resized the canvas, which clears it.
    Resized,
    /// The queued microtask ran.
    Fired,
    /// A frame is drawing.
    Drawing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResizeRedrawEffect {
    QueueMicrotask,
    Draw,
}

/// A resize's frame is drawn in a microtask after winit's callback returns (it holds the canvas
/// borrowed while it runs), which is still before the browser paints the cleared canvas. A scale
/// change and the resize after it share one microtask, and a frame drawn first makes it a no-op.
pub(crate) fn step_resize_redraw(state: &mut ResizeRedraw, event: ResizeRedrawEvent) -> crate::machine::Effects<ResizeRedrawEffect> {
    use crate::machine::Effects;
    match (*state, event) {
        (ResizeRedraw::Idle, ResizeRedrawEvent::Resized) => { *state = ResizeRedraw::Queued; Effects::one(ResizeRedrawEffect::QueueMicrotask) }
        (ResizeRedraw::Queued, ResizeRedrawEvent::Fired) => { *state = ResizeRedraw::Idle; Effects::one(ResizeRedrawEffect::Draw) }
        (_, ResizeRedrawEvent::Drawing) => { *state = ResizeRedraw::Idle; Effects::none() }
        (ResizeRedraw::Queued, ResizeRedrawEvent::Resized) | (ResizeRedraw::Idle, ResizeRedrawEvent::Fired) => Effects::none(),
    }
}

/// Initial retry delay after a refused web surface configuration; doubles per consecutive failure.
pub(crate) const CONFIGURE_RETRY_BASE: std::time::Duration = std::time::Duration::from_millis(50);
/// Consecutive surface configuration failures before waiting for a resize instead of retrying.
pub(crate) const CONFIGURE_MAX_FAILURES: u8 = 5;

/// Health and retry state of a web canvas surface's configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SurfaceHealth {
    /// Configured and ready to draw; `failures` counts prior consecutive failures until a clean present.
    Ready { failures: u8 },
    /// Configure `attempt` is in flight (awaiting its sync or async error-scope outcome).
    Configuring { failures: u8, attempt: crate::machine::Attempt },
    /// Configure `attempt` is in flight and a frame already presented; clears `failures` once `attempt` succeeds.
    ConfiguringPresented { failures: u8, attempt: crate::machine::Attempt },
    /// A configure was refused; a retry timer is armed for `attempt`.
    WaitingRetry { failures: u8, attempt: crate::machine::Attempt },
    /// Exhausted retries; no further configures or draws run until a resize arrives.
    GaveUp,
}

impl Default for SurfaceHealth {
    fn default() -> Self {
        Self::Ready { failures: 0 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SurfaceHealthEvent {
    /// A configure call was issued with `attempt`.
    Begin(crate::machine::Attempt),
    /// A configure call resolved (synchronously on WebGL2, or via JS error scopes on WebGPU).
    Outcome { attempt: crate::machine::Attempt, error: Option<String> },
    /// The scheduled retry timer fired for `attempt`.
    RetryDue(crate::machine::Attempt),
    /// A frame presented cleanly, ending any run of failures.
    Presented,
    /// The canvas resized to a non-zero extent, allowing a fresh configure.
    Resized,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SurfaceHealthEffect {
    /// Issue a new surface configure now.
    Configure,
    /// Schedule a `RetryDue(attempt)` wake after `delay`.
    ScheduleRetry { attempt: crate::machine::Attempt, delay: std::time::Duration },
    /// Log the first failure in a run.
    ReportFirst(String),
    /// Log that retries ran out and the canvas waits for a resize.
    ReportGaveUp(String),
}

impl SurfaceHealth {
    /// Whether the surface can currently acquire and draw frames.
    pub(crate) fn can_draw(&self) -> bool {
        matches!(self, Self::Ready { .. } | Self::Configuring { .. } | Self::ConfiguringPresented { .. })
    }

    /// Consumes a due retry attempt when not suspended and returns whether to reconfigure now.
    pub(crate) fn drain_retry(
        &mut self,
        seq: &mut crate::machine::Seq,
        retry_due: &mut Option<crate::machine::Attempt>,
        suspended: bool,
    ) -> bool {
        if suspended { return false; }
        let Some(attempt) = retry_due.take() else { return false };
        self.step(seq, SurfaceHealthEvent::RetryDue(attempt))
            .into_iter()
            .any(|e| matches!(e, SurfaceHealthEffect::Configure))
    }

    pub(crate) fn step(
        &mut self,
        seq: &mut crate::machine::Seq,
        event: SurfaceHealthEvent,
    ) -> crate::machine::Effects<SurfaceHealthEffect> {
        use crate::machine::Effects;
        match (*self, event) {
            (Self::Ready { failures } | Self::Configuring { failures, .. } | Self::ConfiguringPresented { failures, .. } | Self::WaitingRetry { failures, .. }, SurfaceHealthEvent::Begin(attempt)) => {
                *self = Self::Configuring { failures, attempt };
                Effects::none()
            }
            (Self::GaveUp, SurfaceHealthEvent::Begin(attempt)) => {
                *self = Self::Configuring { failures: 0, attempt };
                Effects::none()
            }
            (Self::Configuring { failures, attempt: expected }, SurfaceHealthEvent::Outcome { attempt, error: None })
                if attempt == expected =>
            {
                *self = Self::Ready { failures };
                Effects::none()
            }
            (Self::ConfiguringPresented { attempt: expected, .. }, SurfaceHealthEvent::Outcome { attempt, error: None })
                if attempt == expected =>
            {
                *self = Self::Ready { failures: 0 };
                Effects::none()
            }
            (
                Self::Configuring { failures, attempt: expected }
                | Self::ConfiguringPresented { failures, attempt: expected },
                SurfaceHealthEvent::Outcome { attempt, error: Some(why) },
            ) if attempt == expected => {
                let next = failures.saturating_add(1);
                if next >= CONFIGURE_MAX_FAILURES {
                    *self = Self::GaveUp;
                    return Effects::one(SurfaceHealthEffect::ReportGaveUp(why));
                }
                let retry = seq.next();
                let delay = CONFIGURE_RETRY_BASE * (1u32 << (next - 1));
                *self = Self::WaitingRetry { failures: next, attempt: retry };
                let schedule = SurfaceHealthEffect::ScheduleRetry { attempt: retry, delay };
                if next == 1 {
                    Effects::two(SurfaceHealthEffect::ReportFirst(why), schedule)
                } else {
                    Effects::one(schedule)
                }
            }
            (Self::WaitingRetry { failures, attempt: expected }, SurfaceHealthEvent::RetryDue(attempt))
                if attempt == expected =>
            {
                *self = Self::Ready { failures };
                Effects::one(SurfaceHealthEffect::Configure)
            }
            (Self::Ready { .. }, SurfaceHealthEvent::Presented) => {
                *self = Self::Ready { failures: 0 };
                Effects::none()
            }
            (Self::Configuring { failures, attempt }, SurfaceHealthEvent::Presented) => {
                *self = Self::ConfiguringPresented { failures, attempt };
                Effects::none()
            }
            (Self::GaveUp | Self::WaitingRetry { .. }, SurfaceHealthEvent::Resized) => {
                *self = Self::Ready { failures: 0 };
                Effects::none()
            }
            _ => Effects::none(),
        }
    }
}

/// How long a CPU-first web page draws nothing non-ambient before starting synchronous GPU
/// setup, and the longest after its first CPU frame that it waits for quiet.
pub(crate) const GPU_START_QUIET: std::time::Duration = std::time::Duration::from_millis(250);
pub(crate) const GPU_START_QUIET_LIMIT: std::time::Duration = std::time::Duration::from_secs(3);

/// Whether a resolved GPU device must yield a browser macrotask before waking `Wake::Ready`
/// so `gpu_side` (`Renderer::new`) runs in a separate browser task from `try_webgl2`.
///
/// WebGPU resolves from browser promises and warms pipelines asynchronously off the main thread;
/// WebGL2 (`wgpu-core`) resolves `request_adapter` and `request_device` synchronously without
/// yielding, and winit's `EventLoopProxy::send_event` dispatches via `queueMicrotask`.
pub(crate) fn needs_ready_yield(backend: wgpu::Backend) -> bool {
    backend == wgpu::Backend::Gl
}

/// Which backend a scheduled CPU-first GPU startup timer is waiting to start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GpuStartPhase {
    /// WebGPU advertises async pipeline compilation, so probe WebGPU right after the first frame.
    ProbeWebGpu,
    /// Synchronous WebGPU (no `createRenderPipelineAsync`): wait for quiet before probing WebGPU.
    WaitQuietForWebGpu,
    /// Synchronous WebGL2 (no `navigator.gpu`, or WebGPU probe failed): wait for quiet before
    /// starting WebGL2 setup.
    WaitQuietForWebGl2,
    /// Start WebGL2 on the next browser macrotask without waiting for quiet (non-degradable `NeedsGpu`).
    StartWebGl2Now,
    /// Neither WebGPU nor WebGL2 is available: report no GPU on the next turn.
    NoBackend,
}

/// Explicit state machine scheduling CPU-first GPU acquisition after the first CPU frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum GpuStartup {
    #[default]
    Idle,
    Scheduled {
        attempt: crate::machine::Attempt,
        first_at: crate::time::Instant,
        drawn_at: crate::time::Instant,
        phase: GpuStartPhase,
        webgl_enabled: bool,
        urgent: bool,
    },
    ProbingWebGpu {
        first_at: crate::time::Instant,
        drawn_at: crate::time::Instant,
        webgl_enabled: bool,
        urgent: bool,
    },
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GpuStartupEvent {
    FirstPresented {
        at: crate::time::Instant,
        has_navigator_gpu: bool,
        compiles_async: bool,
        webgl_enabled: bool,
    },
    /// A frame hit a non-degradable `NeedsGpu` error that the CPU canvas cannot paint, so GPU
    /// acquisition must start immediately on the next macrotask instead of waiting for frame 1 or
    /// the quiet window.
    NeedsGpu {
        at: crate::time::Instant,
        has_navigator_gpu: bool,
        webgl_enabled: bool,
    },
    FrameDrawn { at: crate::time::Instant },
    CheckDue { attempt: crate::machine::Attempt, now: crate::time::Instant },
    WebGpuSucceeded,
    WebGpuFailed { now: crate::time::Instant },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GpuStartupEffect {
    ScheduleCheck { attempt: crate::machine::Attempt, delay: std::time::Duration },
    AskWebGpu,
    AskWebGl2,
    FailNoBackend,
}

fn quiet_remaining(
    first_at: crate::time::Instant,
    drawn_at: crate::time::Instant,
    now: crate::time::Instant,
) -> Option<std::time::Duration> {
    let since_first = now.saturating_duration_since(first_at);
    if since_first >= GPU_START_QUIET_LIMIT {
        return None;
    }
    let since_draw = now.saturating_duration_since(drawn_at);
    if since_draw >= GPU_START_QUIET {
        return None;
    }
    let until_quiet = GPU_START_QUIET.saturating_sub(since_draw);
    let until_limit = GPU_START_QUIET_LIMIT.saturating_sub(since_first);
    Some(until_quiet.min(until_limit))
}

impl GpuStartup {
    pub(crate) fn step(
        &mut self,
        seq: &mut crate::machine::Seq,
        event: GpuStartupEvent,
    ) -> crate::machine::Effects<GpuStartupEffect> {
        use crate::machine::Effects;
        use std::time::Duration;
        match (*self, event) {
            (
                Self::Idle,
                GpuStartupEvent::FirstPresented { at, has_navigator_gpu, compiles_async, webgl_enabled },
            ) => {
                let (phase, delay) = if has_navigator_gpu && compiles_async {
                    (GpuStartPhase::ProbeWebGpu, Duration::ZERO)
                } else if has_navigator_gpu {
                    (GpuStartPhase::WaitQuietForWebGpu, GPU_START_QUIET)
                } else if webgl_enabled {
                    (GpuStartPhase::WaitQuietForWebGl2, GPU_START_QUIET)
                } else {
                    (GpuStartPhase::NoBackend, Duration::ZERO)
                };
                let attempt = seq.next();
                *self = Self::Scheduled { attempt, first_at: at, drawn_at: at, phase, webgl_enabled, urgent: false };
                Effects::one(GpuStartupEffect::ScheduleCheck { attempt, delay })
            }
            (
                Self::Idle,
                GpuStartupEvent::NeedsGpu { at, has_navigator_gpu, webgl_enabled },
            ) => {
                let phase = if has_navigator_gpu {
                    GpuStartPhase::ProbeWebGpu
                } else if webgl_enabled {
                    GpuStartPhase::StartWebGl2Now
                } else {
                    GpuStartPhase::NoBackend
                };
                let attempt = seq.next();
                *self = Self::Scheduled { attempt, first_at: at, drawn_at: at, phase, webgl_enabled, urgent: true };
                Effects::one(GpuStartupEffect::ScheduleCheck { attempt, delay: Duration::ZERO })
            }
            (
                Self::Scheduled { first_at, drawn_at, phase, webgl_enabled, urgent: false, .. },
                GpuStartupEvent::NeedsGpu { .. },
            ) => {
                let next_phase = match phase {
                    GpuStartPhase::WaitQuietForWebGpu => GpuStartPhase::ProbeWebGpu,
                    GpuStartPhase::WaitQuietForWebGl2 => GpuStartPhase::StartWebGl2Now,
                    other => other,
                };
                let attempt = seq.next();
                *self = Self::Scheduled { attempt, first_at, drawn_at, phase: next_phase, webgl_enabled, urgent: true };
                Effects::one(GpuStartupEffect::ScheduleCheck { attempt, delay: Duration::ZERO })
            }
            (
                Self::ProbingWebGpu { first_at, drawn_at, webgl_enabled, urgent: false },
                GpuStartupEvent::NeedsGpu { .. },
            ) => {
                *self = Self::ProbingWebGpu { first_at, drawn_at, webgl_enabled, urgent: true };
                Effects::none()
            }
            (
                Self::Scheduled { attempt, first_at, drawn_at, phase, webgl_enabled, urgent },
                GpuStartupEvent::FrameDrawn { at },
            ) => {
                *self = Self::Scheduled { attempt, first_at, drawn_at: drawn_at.max(at), phase, webgl_enabled, urgent };
                Effects::none()
            }
            (
                Self::ProbingWebGpu { first_at, drawn_at, webgl_enabled, urgent },
                GpuStartupEvent::FrameDrawn { at },
            ) => {
                *self = Self::ProbingWebGpu { first_at, drawn_at: drawn_at.max(at), webgl_enabled, urgent };
                Effects::none()
            }
            (
                Self::Scheduled { attempt: expected, first_at, drawn_at, phase, webgl_enabled, urgent },
                GpuStartupEvent::CheckDue { attempt, now },
            ) if attempt == expected => match phase {
                GpuStartPhase::ProbeWebGpu => {
                    *self = Self::ProbingWebGpu { first_at, drawn_at, webgl_enabled, urgent };
                    Effects::one(GpuStartupEffect::AskWebGpu)
                }
                GpuStartPhase::WaitQuietForWebGpu => match quiet_remaining(first_at, drawn_at, now) {
                    Some(delay) => {
                        let next = seq.next();
                        *self = Self::Scheduled { attempt: next, first_at, drawn_at, phase, webgl_enabled, urgent };
                        Effects::one(GpuStartupEffect::ScheduleCheck { attempt: next, delay })
                    }
                    None => {
                        *self = Self::ProbingWebGpu { first_at, drawn_at, webgl_enabled, urgent };
                        Effects::one(GpuStartupEffect::AskWebGpu)
                    }
                },
                GpuStartPhase::WaitQuietForWebGl2 => match quiet_remaining(first_at, drawn_at, now) {
                    Some(delay) => {
                        let next = seq.next();
                        *self = Self::Scheduled { attempt: next, first_at, drawn_at, phase, webgl_enabled, urgent };
                        Effects::one(GpuStartupEffect::ScheduleCheck { attempt: next, delay })
                    }
                    None => {
                        *self = Self::Done;
                        Effects::one(GpuStartupEffect::AskWebGl2)
                    }
                },
                GpuStartPhase::StartWebGl2Now => {
                    *self = Self::Done;
                    Effects::one(GpuStartupEffect::AskWebGl2)
                }
                GpuStartPhase::NoBackend => {
                    *self = Self::Done;
                    Effects::one(GpuStartupEffect::FailNoBackend)
                }
            },
            (Self::ProbingWebGpu { .. }, GpuStartupEvent::WebGpuSucceeded) => {
                *self = Self::Done;
                Effects::none()
            }
            (
                Self::ProbingWebGpu { first_at, drawn_at, webgl_enabled, urgent },
                GpuStartupEvent::WebGpuFailed { now },
            ) => {
                if !webgl_enabled {
                    *self = Self::Done;
                    return Effects::one(GpuStartupEffect::FailNoBackend);
                }
                // Always schedule via a macrotask (remaining quiet time, or Duration::ZERO if
                // already quiet or urgent due to non-degradable NeedsGpu) so synchronous WebGL2
                // setup never runs inside requestAdapter's Promise microtask.
                let (phase, delay) = if urgent {
                    (GpuStartPhase::StartWebGl2Now, Duration::ZERO)
                } else {
                    (GpuStartPhase::WaitQuietForWebGl2, quiet_remaining(first_at, drawn_at, now).unwrap_or(Duration::ZERO))
                };
                let attempt = seq.next();
                *self = Self::Scheduled {
                    attempt,
                    first_at,
                    drawn_at,
                    phase,
                    webgl_enabled: true,
                    urgent,
                };
                Effects::one(GpuStartupEffect::ScheduleCheck { attempt, delay })
            }
            _ => Effects::none(),
        }
    }
}

#[cfg(test)]
#[path = "web_math_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "web_startup_tests.rs"]
mod startup_tests;
