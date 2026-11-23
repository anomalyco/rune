//! The arithmetic behind the browser shell: the canvas's size in device pixels, when a resize
//! needs the surface configured again, and the band a page's canvas covers. Kept out of
//! `platform/web.rs`, which only builds on `wasm32`, so the tests run headless.
#![cfg_attr(not(test), allow(dead_code))]

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
}

/// Decides how the web host responds when the GPU device or WebGL2 context is lost.
///
/// WebGPU loss preserves the existing policy: permanent CPU fallback on CPU-first pages and
/// fatal on GPU-first pages. WebGL2 context loss is recoverable when `webglcontextrestored`
/// fires: full-window pages (`!embedded && !document`) fall back to the CPU canvas immediately so
/// they are never blank while waiting (or if restore never arrives), while inline and document
/// pages pause in place until restore.
pub(crate) fn loss_action(is_webgl: bool, gpu_first: bool, embedded: bool, document: bool) -> LossAction {
    if is_webgl {
        if !embedded && !document {
            LossAction::FallbackToCpu { recoverable: true }
        } else {
            LossAction::PauseForRestore
        }
    } else if gpu_first {
        LossAction::Fatal
    } else {
        LossAction::FallbackToCpu { recoverable: false }
    }
}

/// Web draw state reduced to the facts `WebFrameHost::gate` needs to decide whether a frame runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WebDrawKind {
    Cpu,
    HandingOver { waiting_for_gpu: bool },
    Gpu,
    ContextLost,
    Moving,
}

/// Decides whether a web frame skips, steps before acquiring (`Gate::QuietTimer` when a
/// previous GPU presentation is valid so an unchanged frame avoids acquiring a canvas texture),
/// or draws unconditionally (`Gate::Draw`).
pub(crate) fn web_frame_gate(
    suspended: bool,
    draw: WebDrawKind,
    quiet_candidate: bool,
) -> crate::frame::host_step::Gate {
    if suspended
        || matches!(
            draw,
            WebDrawKind::HandingOver { waiting_for_gpu: true }
                | WebDrawKind::ContextLost
                | WebDrawKind::Moving
        )
    {
        crate::frame::host_step::Gate::Skip
    } else if draw == WebDrawKind::Gpu && quiet_candidate {
        crate::frame::host_step::Gate::QuietTimer
    } else {
        crate::frame::host_step::Gate::Draw
    }
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
        Missed::Timeout | Missed::Outdated => WebMissAction::RetryNextFrame,
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

/// Builds the web host's `Runtime` and splits off its shared state, compositor and fonts.
/// Kept out of `platform/web/app.rs` so web startup configuration runs in native unit tests.
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
    &'static [&'static [u8]],
) {
    let compositor = app.compositor.take();
    let (playground, attach_playground) = (app.playground.take(), app.attach_playground);
    let spec_fonts = app.spec.fonts;
    let (spec, state, shared, runtime) = app.into_parts(None);
    let mut rt = crate::tooling::runtime_for(&spec, state, viewport, env, wake);
    // Live values wake on their own lane so a value already drawn in the frame that published
    // it does not trigger Wake::App's invalidate and redundant redraw.
    rt.set_live_waker(live_wake);
    if let Some(attach) = attach_playground {
        attach(&mut rt, playground, &spec, None, None, true);
    }
    rt.configure(runtime);
    rt.track_marks = compositor.is_some();
    (rt, shared, compositor, spec_fonts)
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
    /// Ready to draw (or awaiting the answer to the latest `active` configure).
    Ready { failures: u8, active: Option<crate::machine::Attempt> },
    /// A configure was refused; a retry timer is armed for `attempt`.
    WaitingRetry { failures: u8, attempt: crate::machine::Attempt },
    /// Exhausted retries; no further configures or draws run until a resize arrives.
    GaveUp,
}

impl Default for SurfaceHealth {
    fn default() -> Self {
        Self::Ready { failures: 0, active: None }
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
        matches!(self, Self::Ready { .. })
    }

    pub(crate) fn step(
        &mut self,
        seq: &mut crate::machine::Seq,
        event: SurfaceHealthEvent,
    ) -> crate::machine::Effects<SurfaceHealthEffect> {
        use crate::machine::Effects;
        match (*self, event) {
            (Self::Ready { failures, .. } | Self::WaitingRetry { failures, .. }, SurfaceHealthEvent::Begin(attempt)) => {
                *self = Self::Ready { failures, active: Some(attempt) };
                Effects::none()
            }
            (Self::GaveUp, SurfaceHealthEvent::Begin(attempt)) => {
                *self = Self::Ready { failures: 0, active: Some(attempt) };
                Effects::none()
            }
            (Self::Ready { failures, active: Some(expected) }, SurfaceHealthEvent::Outcome { attempt, error })
                if attempt == expected =>
            {
                match error {
                    None => {
                        *self = Self::Ready { failures, active: None };
                        Effects::none()
                    }
                    Some(why) => {
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
                }
            }
            (Self::WaitingRetry { failures, attempt: expected }, SurfaceHealthEvent::RetryDue(attempt))
                if attempt == expected =>
            {
                *self = Self::Ready { failures, active: None };
                Effects::one(SurfaceHealthEffect::Configure)
            }
            (Self::Ready { active, .. }, SurfaceHealthEvent::Presented) => {
                *self = Self::Ready { failures: 0, active };
                Effects::none()
            }
            (Self::GaveUp | Self::WaitingRetry { .. }, SurfaceHealthEvent::Resized) => {
                *self = Self::Ready { failures: 0, active: None };
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
    },
    ProbingWebGpu {
        first_at: crate::time::Instant,
        drawn_at: crate::time::Instant,
        webgl_enabled: bool,
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
                *self = Self::Scheduled { attempt, first_at: at, drawn_at: at, phase, webgl_enabled };
                Effects::one(GpuStartupEffect::ScheduleCheck { attempt, delay })
            }
            (
                Self::Scheduled { attempt, first_at, drawn_at, phase, webgl_enabled },
                GpuStartupEvent::FrameDrawn { at },
            ) => {
                *self = Self::Scheduled { attempt, first_at, drawn_at: drawn_at.max(at), phase, webgl_enabled };
                Effects::none()
            }
            (
                Self::ProbingWebGpu { first_at, drawn_at, webgl_enabled },
                GpuStartupEvent::FrameDrawn { at },
            ) => {
                *self = Self::ProbingWebGpu { first_at, drawn_at: drawn_at.max(at), webgl_enabled };
                Effects::none()
            }
            (
                Self::Scheduled { attempt: expected, first_at, drawn_at, phase, webgl_enabled },
                GpuStartupEvent::CheckDue { attempt, now },
            ) if attempt == expected => match phase {
                GpuStartPhase::ProbeWebGpu => {
                    *self = Self::ProbingWebGpu { first_at, drawn_at, webgl_enabled };
                    Effects::one(GpuStartupEffect::AskWebGpu)
                }
                GpuStartPhase::WaitQuietForWebGpu => match quiet_remaining(first_at, drawn_at, now) {
                    Some(delay) => {
                        let next = seq.next();
                        *self = Self::Scheduled { attempt: next, first_at, drawn_at, phase, webgl_enabled };
                        Effects::one(GpuStartupEffect::ScheduleCheck { attempt: next, delay })
                    }
                    None => {
                        *self = Self::ProbingWebGpu { first_at, drawn_at, webgl_enabled };
                        Effects::one(GpuStartupEffect::AskWebGpu)
                    }
                },
                GpuStartPhase::WaitQuietForWebGl2 => match quiet_remaining(first_at, drawn_at, now) {
                    Some(delay) => {
                        let next = seq.next();
                        *self = Self::Scheduled { attempt: next, first_at, drawn_at, phase, webgl_enabled };
                        Effects::one(GpuStartupEffect::ScheduleCheck { attempt: next, delay })
                    }
                    None => {
                        *self = Self::Done;
                        Effects::one(GpuStartupEffect::AskWebGl2)
                    }
                },
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
                Self::ProbingWebGpu { first_at, drawn_at, webgl_enabled },
                GpuStartupEvent::WebGpuFailed { now },
            ) => {
                if !webgl_enabled {
                    *self = Self::Done;
                    return Effects::one(GpuStartupEffect::FailNoBackend);
                }
                // Always schedule via a macrotask (remaining quiet time, or Duration::ZERO if
                // already quiet) so synchronous WebGL2 setup never runs inside requestAdapter's
                // Promise microtask or before the page has had a quiet window after drawing.
                let delay = quiet_remaining(first_at, drawn_at, now).unwrap_or(Duration::ZERO);
                let attempt = seq.next();
                *self = Self::Scheduled {
                    attempt,
                    first_at,
                    drawn_at,
                    phase: GpuStartPhase::WaitQuietForWebGl2,
                    webgl_enabled: true,
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
