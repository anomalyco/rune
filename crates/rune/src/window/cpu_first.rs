//! CPU-first startup and handover policy ([`CpuFirst`]) for platforms with a live CPU renderer
//! (Windows, Linux, and full-window web pages).

/// Controls whether a window opens on Rune's live CPU renderer (`CpuPainter`) before handing over
/// to the GPU, stays on the CPU renderer, or skips CPU-first startup altogether.
///
/// Configure per window with [`App::cpu_first`](crate::App::cpu_first) or
/// [`WindowSpec::cpu_first`](crate::WindowSpec::cpu_first). Defaults to [`CpuFirst::Auto`].
///
/// # Environment override
///
/// `RUNE_CPU_FIRST` (on Windows and Linux) and `globalThis.RUNE_CPU_FIRST` (on web) override the
/// configured builder value without a rebuild, matching `RUNE_GPU_BACKEND`:
///
/// - `"0"` or `"off"` selects [`CpuFirst::Off`].
/// - `"only"` selects [`CpuFirst::Only`].
/// - `"auto"` selects [`CpuFirst::Auto`].
/// - Unset, empty, or unrecognized values leave the builder value in effect.
///
/// # Platform notes
///
/// - **Windows and Linux**: applies to visible opaque windows that do not already hold a GPU
///   device. Under [`CpuFirst::Only`], background GPU handover does not start on idle, so the
///   window stays on `CpuPainter` unless a frame requires a GPU-only feature (such as linear
///   blending, native video surfaces, `FontSmoothing::Slug`, `El::blur`, rotated layers, nested
///   `El::backdrop_blur`, or a compositor effect slot), in which case Rune starts the GPU and hands
///   over so that frame can draw. Transparent windows always open on the GPU.
/// - **Web (`wasm32`)**: applies to full-window apps (`App::web_document` and inline
///   `App::canvas_id` pages always start on the GPU). Under [`CpuFirst::Only`], the page stays on
///   the 2D CPU canvas and never requests WebGPU or WebGL2.
/// - **macOS, iOS, and headless**: ignored (windows open on the GPU as before).
///
/// ```no_run
/// use rune::{App, AppSpec, CpuFirst, el};
///
/// App::new(AppSpec::new("Plan", (960.0, 640.0), || (), |_, _| el()))
///     .cpu_first(CpuFirst::Only)
///     .run();
/// ```
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CpuFirst {
    /// Default startup: open opaque windows on the CPU while no GPU exists yet and hand over to
    /// the GPU in the background once the first frame is shown (and on full-window web pages, paint
    /// first frames on the 2D canvas until WebGPU/WebGL2 takes over).
    #[default]
    Auto,
    /// Open opaque windows on the CPU renderer and do not start background GPU handover on idle.
    ///
    /// On Windows and Linux, a frame that hits a GPU-only feature still triggers GPU handover so
    /// the frame can render; transparent windows and windows that already have a GPU keep using
    /// the GPU. On full-window web pages, stays on the 2D CPU canvas without requesting WebGPU or
    /// WebGL2.
    Only,
    /// Skip CPU-first startup and open directly on the GPU (on web, skip the 2D CPU canvas underlay
    /// and start directly on WebGPU/WebGL2).
    Off,
}

impl CpuFirst {
    /// Parses a `RUNE_CPU_FIRST` environment or browser-global value.
    pub(crate) fn parse_env(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if trimmed == "0" || trimmed.eq_ignore_ascii_case("off") {
            Some(Self::Off)
        } else if trimmed.eq_ignore_ascii_case("only") {
            Some(Self::Only)
        } else if trimmed.eq_ignore_ascii_case("auto") {
            Some(Self::Auto)
        } else {
            None
        }
    }

    /// Resolves the effective mode from this builder value and an optional `RUNE_CPU_FIRST`
    /// override. A recognized override wins over `self`.
    pub(crate) fn resolve(self, env: Option<&str>) -> Self {
        env.and_then(Self::parse_env).unwrap_or(self)
    }

    /// Effective mode on desktop, resolving `RUNE_CPU_FIRST` over `self`.
    #[cfg_attr(not(any(target_os = "windows", target_os = "linux", test)), allow(dead_code))]
    pub(crate) fn effective(self) -> Self {
        self.resolve(crate::env::var("RUNE_CPU_FIRST").as_deref())
    }

    /// Whether a desktop window opens on the CPU renderer given its transparency, whether this
    /// host already owns a GPU, and whether another window in the event loop holds a live GPU.
    ///
    /// - `Auto`: opaque, no own GPU, and no live shared GPU (unchanged default).
    /// - `Only`: opaque and no own GPU, even if another window already holds a live GPU.
    /// - `Off`: never opens on the CPU.
    #[cfg_attr(not(any(target_os = "windows", target_os = "linux", test)), allow(dead_code))]
    pub(crate) const fn opens_on_cpu(self, transparent: bool, has_gpu: bool, live_pool_gpu: bool) -> bool {
        if transparent || has_gpu {
            return false;
        }
        match self {
            Self::Auto => !live_pool_gpu,
            Self::Only => true,
            Self::Off => false,
        }
    }

    /// Whether a CPU-opened desktop window should start background GPU handover on idle once its
    /// first frame is ready and any retry backoff has elapsed.
    #[cfg_attr(not(any(target_os = "windows", target_os = "linux", test)), allow(dead_code))]
    pub(crate) const fn gpu_owed(self, ready_and_due: bool) -> bool {
        ready_and_due && !matches!(self, Self::Only)
    }

    /// Resolves how a web page's frames start from this effective mode and page layout flags.
    ///
    /// Document and inline embedded canvases always start on the GPU; full-window apps follow
    /// `self`.
    #[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
    pub(crate) const fn web_start(self, document: bool, embedded: bool) -> WebStart {
        if document || embedded {
            return WebStart::GpuFirst;
        }
        match self {
            Self::Auto => WebStart::CpuFirst,
            Self::Only => WebStart::CpuOnly,
            Self::Off => WebStart::GpuFirst,
        }
    }
}

/// How a web page's frames start: on the CPU until WebGPU/WebGL2 takes over (a full-window app
/// under `CpuFirst::Auto`), on the CPU only (`CpuFirst::Only`), or on the GPU from the first
/// frame (`CpuFirst::Off`, or a document/inline canvas).
#[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WebStart {
    GpuFirst,
    CpuFirst,
    CpuOnly,
}

#[cfg(test)]
#[path = "cpu_first_tests.rs"]
mod tests;
