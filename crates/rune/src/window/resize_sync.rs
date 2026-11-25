//! Windows drag-only synchronized resize policy ([`ResizeSync`]).

/// Controls whether a Windows app window opts into Rune's drag-only synchronized resize path
/// between `WM_ENTERSIZEMOVE` and `WM_EXITSIZEMOVE`.
///
/// Configure per window with [`App::resize_sync`](crate::App::resize_sync) or
/// [`WindowSpec::resize_sync`](crate::WindowSpec::resize_sync). Defaults to [`ResizeSync::Auto`].
///
/// # What the path does
///
/// During a live border drag on an eligible Windows window (opaque, hardware D3D12 adapter,
/// 8-bit swap-chain format, DirectComposition `DxgiFromVisual` with a settled Rune-owned
/// `Handover`), Rune releases any active video layers, renders drag frames into a capacited
/// 2-slot D3D12 shared texture ring, synchronizes via shared fences into a D3D11
/// `DXGI_SWAP_EFFECT_DISCARD` copy-model HWND swap chain presented inside `WM_PAINT`
/// (`BeginPaint`/`EndPaint`), detaches the DirectComposition root (`IDCompositionTarget::SetRoot`),
/// and pairs with `WM_NCCALCSIZE` `WVR_VALIDRECTS` and `WM_ERASEBKGND` suppression so content
/// stays in step with left- and top-edge drags with zero presents to the hidden D3D12
/// composition swap chain. On `WM_EXITSIZEMOVE`, one normal D3D12 flip-model frame completes
/// before DirectComposition re-attaches and drag textures and the copy-model swap chain are freed.
///
/// # Cost and documented-API limit
///
/// - **Launch and non-drag cost**: zero. When disabled (the default today) or before the first
///   drag, no D3D11 device, shared texture, or second swap chain is created.
/// - **Drag and post-drag memory**: the capacited 2-slot shared texture ring and copy-model swap
///   chain are freed immediately when the drag ends; the lazily created D3D11 device (~15–40 MiB)
///   stays cached on that window for subsequent drags until device loss or window destruction.
/// - **Hybrid GPU laptops**: per Microsoft Learn (*For best performance, use DXGI flip model*),
///   bitblt (`DXGI_SWAP_EFFECT_DISCARD`) presents can incur an extra cross-adapter copy on hybrid
///   laptops.
/// - **Occasional late step**: per Microsoft Learn (`DXGI_SWAP_EFFECT`, `DXGI_SCALING`,
///   `WM_NCCALCSIZE`, `IDCompositionTarget::SetRoot`), `DXGI_SWAP_EFFECT_DISCARD` requires
///   `DXGI_SCALING_STRETCH` (only flip-model supports `DXGI_SCALING_NONE`), so the HWND swap
///   chain resizes on each size step and an occasional step that straddles a refresh boundary
///   (~5% on fast 239 Hz drags) can still appear one step out of place. Rune uses public
///   Microsoft-documented APIs only; undocumented Windows resize-sync interfaces are not used.
///
/// # Environment override and kill switch
///
/// `RUNE_RESIZE_SYNC` overrides the configured builder value for every window without a rebuild:
///
/// - `"1"`, `"true"`, `"yes"`, or `"on"` (ASCII case-insensitive) selects [`ResizeSync::On`].
/// - `"0"`, `"false"`, `"no"`, or `"off"` selects [`ResizeSync::Off`] and acts as an unconditional
///   kill switch over [`ResizeSync::On`].
/// - `"auto"` selects [`ResizeSync::Auto`].
/// - Unset, empty, or unrecognized values leave the builder value in effect.
///
/// # Where it does nothing
///
/// Ignored on macOS, Linux, web (`wasm32`), iOS, headless runs, overlays, transparent windows
/// (including `WindowBackdrop` and `El::system_material`), `WGPU_DX12_PRESENTATION_SYSTEM=hwnd`,
/// software/WARP adapters, non-8-bit (HDR) surface formats, and windows still on CPU-first,
/// handover, or rebinding modes.
///
/// ```no_run
/// use rune::{App, AppSpec, ResizeSync, el};
///
/// App::new(AppSpec::new("Settings", (620.0, 520.0), || (), |_, _| el()))
///     .resize_sync(ResizeSync::On)
///     .run();
/// ```
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ResizeSync {
    /// Framework default: currently resolves to off so Windows windows stay on DirectComposition
    /// flip-model presentation, and may turn on for eligible Windows windows in a later release
    /// once more GPU and Windows configurations have been verified.
    #[default]
    Auto,
    /// Opt into the drag-only synchronized resize path on eligible Windows hardware windows.
    On,
    /// Always stay on normal DirectComposition flip-model presentation during live drags.
    Off,
}

impl ResizeSync {
    /// Environment variable overriding [`App::resize_sync`](crate::App::resize_sync) and
    /// [`WindowSpec::resize_sync`](crate::WindowSpec::resize_sync) without a rebuild.
    pub(crate) const ENV_VAR: &str = "RUNE_RESIZE_SYNC";

    /// Parses a `RUNE_RESIZE_SYNC` environment value.
    pub(crate) fn parse_env(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if trimmed == "1"
            || trimmed.eq_ignore_ascii_case("true")
            || trimmed.eq_ignore_ascii_case("yes")
            || trimmed.eq_ignore_ascii_case("on")
        {
            Some(Self::On)
        } else if trimmed == "0"
            || trimmed.eq_ignore_ascii_case("false")
            || trimmed.eq_ignore_ascii_case("no")
            || trimmed.eq_ignore_ascii_case("off")
        {
            Some(Self::Off)
        } else if trimmed.eq_ignore_ascii_case("auto") {
            Some(Self::Auto)
        } else {
            None
        }
    }

    /// Resolves the effective mode from this builder value and an optional `RUNE_RESIZE_SYNC`
    /// override. A recognized override wins over `self` (so `0`/`false`/`no`/`off` is an
    /// unconditional kill switch over [`Self::On`]).
    pub(crate) fn resolve(self, env: Option<&str>) -> Self {
        env.and_then(Self::parse_env).unwrap_or(self)
    }

    /// Effective mode for this window, resolving `RUNE_RESIZE_SYNC` over `self`.
    #[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
    pub(crate) fn effective(self) -> Self {
        self.resolve(crate::env::var(Self::ENV_VAR).as_deref())
    }

    /// Whether the Windows drag-only synchronized resize path is enabled for this effective mode.
    ///
    /// Today [`Self::Auto`] resolves to `false`; [`Self::On`] resolves to `true`; [`Self::Off`]
    /// resolves to `false`.
    #[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
    pub(crate) const fn is_enabled(self) -> bool {
        matches!(self, Self::On)
    }
}

impl<S: 'static> crate::App<S> {
    /// Controls the Windows drag-only synchronized resize path for this app's window
    /// ([`ResizeSync`]).
    ///
    /// Defaults to [`ResizeSync::Auto`], which resolves to off today so Windows windows keep
    /// normal DirectComposition flip-model presentation, and may turn on for eligible Windows
    /// windows in a later release once more GPU and OS configurations have been verified.
    /// [`ResizeSync::On`] opts eligible opaque hardware D3D12 windows into the drag-only
    /// copy-model path between `WM_ENTERSIZEMOVE` and `WM_EXITSIZEMOVE` (with an accepted ~5%
    /// occasional late step on very fast high-refresh drags, ~15–40 MiB lazily cached D3D11
    /// device after the first drag, and extra cross-adapter copy work on hybrid GPU laptops per
    /// Microsoft Learn *For best performance, use DXGI flip model*). [`ResizeSync::Off`] stays
    /// on normal DirectComposition flip-model presentation.
    ///
    /// `RUNE_RESIZE_SYNC` overrides this setting without a rebuild (`1`/`true`/`yes`/`on` =>
    /// `On`, `0`/`false`/`no`/`off` => `Off` kill switch, `auto` => `Auto`). Ignored on macOS,
    /// Linux, web, iOS, headless runs, overlays, transparent windows, `DxgiFromHwnd`, WARP,
    /// HDR formats, and CPU/handover/rebinding windows.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, ResizeSync, el};
    ///
    /// App::new(AppSpec::new("Settings", (620.0, 520.0), || (), |_, _| el()))
    ///     .resize_sync(ResizeSync::On)
    ///     .run();
    /// ```
    pub fn resize_sync(mut self, mode: ResizeSync) -> Self {
        self.resize_sync = mode;
        self
    }
}

#[cfg(test)]
#[path = "resize_sync_tests.rs"]
mod tests;
