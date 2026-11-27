//! The builders that place an app in a web page: its canvas, a whole-page document, its
//! fonts by URL, same-tab link routing, and GPU fallback notification.

use super::*;

/// Which stage of a web app's GPU lifecycle permanently fell back off the GPU.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GpuFallbackStage {
    /// Initial async GPU acquisition failed (no adapter, software/fallback adapter rejected under
    /// `CpuFirst::Auto`, `requestDevice` error, surface configuration failure, or pipeline warm
    /// failure).
    Init,
    /// The GPU asynchronously rejected Rune's primary renderer shader or pipeline.
    Shader,
    /// The GPU did not finish its first hand-over frame within the hand-over timeout.
    HandoverTimeout,
    /// The GPU device was lost and bounded recovery exhausted its retry budget (or cannot recover).
    DeviceLost,
}

impl GpuFallbackStage {
    /// Stable lowercase identifier exposed as `detail.stage` on the bubbling `runegpufallback`
    /// DOM `CustomEvent`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Init => "init",
            Self::Shader => "shader",
            Self::HandoverTimeout => "handover_timeout",
            Self::DeviceLost => "device_lost",
        }
    }
}

/// Details delivered to [`App::on_gpu_fallback`] (and mirrored on `canvas[data-rune-canvas]` as a
/// bubbling `runegpufallback` DOM `CustomEvent` with `detail: { stage, reason }`) when a web
/// page permanently falls back off the GPU or exhausts GPU recovery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuFallback {
    stage: GpuFallbackStage,
    reason: String,
}

impl GpuFallback {
    #[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
    pub(crate) fn new(stage: GpuFallbackStage, reason: impl Into<String>) -> Self {
        Self { stage, reason: reason.into() }
    }

    /// Which GPU lifecycle stage failed.
    pub const fn stage(&self) -> GpuFallbackStage {
        self.stage
    }

    /// Human-readable diagnostic reason for the failure.
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// `App::on_gpu_fallback`'s hook, run on the browser's main thread when a web app permanently
/// falls back off the GPU or exhausts GPU recovery.
pub(crate) type OnGpuFallback = Box<dyn FnMut(&GpuFallback)>;

/// `fonts` and one more, kept for the life of the process as `AppSpec::fonts` requires.
pub(crate) fn with_font(fonts: &'static [&'static [u8]], bytes: Vec<u8>) -> &'static [&'static [u8]] {
    let mut all = fonts.to_vec();
    all.push(Box::leak(bytes.into_boxed_slice()));
    Box::leak(all.into_boxed_slice())
}

#[cfg(not(target_arch = "wasm32"))]
fn load_font_url(fonts: &mut &'static [&'static [u8]], url: &str) {
    match crate::image::font_file(url) {
        Ok(bytes) => *fonts = with_font(*fonts, bytes),
        Err(e) => eprintln!("rune: font {url}: {e}"),
    }
}

impl<S: 'static> App<S> {
    /// Mount onto an existing `<canvas id="…">` in the web document instead of appending a
    /// full-viewport canvas and locking `document.body`. The canvas sits inline in the page at
    /// its parent's width: pair with [`Self::fit_content_height`] to embed a Rune view in a
    /// scrollable page, or add [`Self::full_viewport`] to cover the screen. Ignored on native.
    pub fn canvas_id(mut self, id: impl Into<String>) -> Self {
        self.canvas_id = Some(id.into());
        self
    }

    /// On web, cover the browser viewport even when [`Self::canvas_id`] names the canvas, as an
    /// app without one does: the canvas is fixed to the visual viewport, the body stops
    /// scrolling, `Cx::safe_area` reports the page's insets and the title sets
    /// `document.title`. For a single-page app that mounts Rune full screen in a route's own
    /// `<canvas>`. [`Self::fit_content_height`] wins over it. Ignored on native.
    pub fn full_viewport(mut self) -> Self {
        self.full_viewport = true;
        self
    }

    /// On web, make the app the page: the root lays out at its natural height for the
    /// viewport's width and the browser scrolls it like any document, with its own momentum,
    /// keys, anchors, find-in-page and zoom. The canvas is a band of the document that scrolls
    /// with it, drawn tall enough to cover the window plus a little overscan at each end, so the
    /// browser's compositor moves the pixels Rune drew and a frame is drawn only when the window
    /// nears the band's edge; a semantic DOM mirror as tall as the content carries the scroll, the
    /// text, the headings, landmarks and real `<a href>` links. Give the root no fixed or `fill`
    /// height. Elsewhere it acts like [`Self::fit_content_height`], and `--html` exports it as the
    /// page's static first frame.
    pub fn web_document(mut self) -> Self {
        self.web_document = true;
        self.fit_content_height = true;
        self
    }

    /// Add a font file (TrueType or OpenType, not WOFF) to [`AppSpec::fonts`] by URL, so a web
    /// app shares one download with the page's own `@font-face`. On web the browser fetches
    /// it, relative to the page, while the GPU starts, and the first frame waits for it; a
    /// font that fails to load is reported in the console and skipped. On native a path or a
    /// `file:` URL is read now, as [`ImageCache::url`](crate::ImageCache::url) reads one.
    pub fn font_url(mut self, url: &str) -> Self {
        #[cfg(target_arch = "wasm32")]
        self.font_urls.push(url.to_owned());
        #[cfg(not(target_arch = "wasm32"))]
        load_font_url(&mut self.spec.fonts, url);
        self
    }

    /// [`Self::font_url`] for a font the first frame doesn't need, such as one a setting picks:
    /// on web the browser fetches it once the first frame is up, so it costs the page neither
    /// download size nor start time. Until it arrives, text that names its family sets in the
    /// slot's fallback (Inter, IBM Plex Mono); then every window takes it up on its next frame,
    /// in place of any registered face of the same family, width, style and weight. A font that
    /// fails to load is reported in the console and skipped. On native it's [`Self::font_url`].
    pub fn font_url_later(self, url: &str) -> Self {
        #[cfg(target_arch = "wasm32")]
        {
            let mut app = self;
            app.late_font_urls.push(url.to_owned());
            app
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.font_url(url)
    }

    /// On web, routes [`Tx::open_url_same_tab`] links inside a single-page app: `route` gets the
    /// absolute URL of each link on the page's own origin and returns true when it handled it
    /// (`history.pushState` and a render), or false to let the browser load it with
    /// `location.assign`. Links to other origins always load. Ignored on native.
    pub fn same_tab_router(mut self, route: impl Fn(&str) -> bool + 'static) -> Self {
        self.same_tab_router = Some(Box::new(route));
        self
    }

    /// On web, runs on the browser's main thread when the page permanently falls back off the GPU
    /// or exhausts GPU recovery (initial async GPU acquisition failure including a rejected
    /// software/fallback adapter under [`crate::CpuFirst::Auto`], async shader rejection,
    /// hand-over timeout, or exhausted WebGPU device-loss recovery).
    ///
    /// Rune also dispatches a bubbling `runegpufallback` DOM `CustomEvent` on
    /// `canvas[data-rune-canvas]` with `detail: { stage, reason }` so split-bundle web hosts can
    /// unmount the WebGPU build (`AppHandle::quit`) and mount a WebGL2 build on a fresh
    /// `<canvas>` from JS without custom glue. Ignored on native and under
    /// [`crate::CpuFirst::Only`].
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, GpuFallback, el};
    ///
    /// let spec = AppSpec::new("App", (640.0, 480.0), || (), |_, _| el());
    /// let app = App::new(spec);
    /// let handle = app.handle();
    /// app.on_gpu_fallback(move |fallback: &GpuFallback| {
    ///     let _ = (fallback.stage(), fallback.reason());
    ///     handle.quit();
    /// }).run();
    /// ```
    pub fn on_gpu_fallback(mut self, f: impl FnMut(&GpuFallback) + 'static) -> Self {
        self.on_gpu_fallback = Some(Box::new(f));
        self
    }
}
