//! Window descriptions shared by the native shell and deterministic headless selection.
use crate::{App, AppSpec};

#[derive(Clone, Debug)]
pub(crate) struct PlaceRequest {
    pub display: crate::DisplayTarget,
    pub placement: crate::OverlayPlacement,
    #[cfg(not(target_arch = "wasm32"))]
    pub pointer: Option<(i32, i32)>,
    /// The caller's target, before sampling the pointer; headless runs resolve this.
    pub requested: crate::DisplayTarget,
}

impl PlaceRequest {
    pub fn new(display: crate::DisplayTarget, placement: crate::OverlayPlacement) -> Self {
        #[allow(unused_mut)] // Only desktop hosts can sample a pointer's display.
        let mut request = Self {
            requested: display.clone(),
            display,
            placement,
            #[cfg(not(target_arch = "wasm32"))]
            pointer: None,
        };
        if request.display == crate::DisplayTarget::UnderPointer {
            #[cfg(target_os = "macos")]
            { request.display = crate::platform::macos::pointer_display().map_or(crate::DisplayTarget::Primary,
                |id| crate::DisplayTarget::Native(crate::NativeDisplayId::MacOS(id))); }
            #[cfg(target_os = "windows")]
            { request.pointer = crate::platform::win32::pointer_position(); }
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            { request.pointer = crate::platform::x11::overlay::pointer_position(); }
        }
        request
    }

    pub fn at(size: (f32, f32)) -> crate::overlay::Placement {
        crate::overlay::Placement { size, edge: crate::ScreenEdge::Top, margin: 0.0, align: crate::Align::Center, inset: 0.0 }
    }
}

/// Which edge or corner of a window an interactive pointer resize drags
/// ([`El::window_resize`](crate::El::window_resize),
/// [`Tx::drag_resize_window`](crate::Tx::drag_resize_window)).
///
/// Maps directly to `winit::window::ResizeDirection` (Win32 sizing borders, X11
/// `_NET_WM_MOVERESIZE`, Wayland `xdg_toplevel.resize`).
///
/// ```
/// use rune::{ResizeEdge, el};
///
/// # let _ =
/// el().pin(None, None, Some(0.0), Some(0.0))
///     .size(12.0, 12.0)
///     .window_resize(ResizeEdge::SouthEast)
/// # ;
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResizeEdge {
    North,
    South,
    East,
    West,
    NorthEast,
    NorthWest,
    SouthEast,
    SouthWest,
}

impl ResizeEdge {
    /// The default hover cursor for a bare [`El::window_resize`](crate::El::window_resize) region.
    pub(crate) const fn cursor(self) -> crate::el::Cursor {
        match self {
            ResizeEdge::North | ResizeEdge::South => crate::el::Cursor::ResizeNs,
            ResizeEdge::East | ResizeEdge::West => crate::el::Cursor::ResizeEw,
            ResizeEdge::NorthWest | ResizeEdge::SouthEast => crate::el::Cursor::ResizeNwse,
            ResizeEdge::NorthEast | ResizeEdge::SouthWest => crate::el::Cursor::ResizeNesw,
        }
    }
}

impl From<ResizeEdge> for winit::window::ResizeDirection {
    fn from(edge: ResizeEdge) -> Self {
        match edge {
            ResizeEdge::North => Self::North,
            ResizeEdge::South => Self::South,
            ResizeEdge::East => Self::East,
            ResizeEdge::West => Self::West,
            ResizeEdge::NorthEast => Self::NorthEast,
            ResizeEdge::NorthWest => Self::NorthWest,
            ResizeEdge::SouthEast => Self::SouthEast,
            ResizeEdge::SouthWest => Self::SouthWest,
        }
    }
}

/// Process-level settings applied once, before constructing the native event loop.
#[derive(Clone, Copy, Debug, Default)]
pub struct RunOptions {
    /// macOS: no Dock icon/default menu and no activation at launch. Ignored elsewhere.
    pub accessory: bool,
}

/// A display in desktop physical-pixel coordinates (origins may be negative).
/// Headless runs provide one deterministic virtual display.
#[derive(Clone, Debug, PartialEq)]
pub struct Display {
    /// Native identity for overlay targeting; absent for the headless virtual display.
    pub id: Option<crate::DisplayId>,
    pub name: Option<String>,
    pub position: (i32, i32),
    pub size: (u32, u32),
    pub scale: f64,
    pub primary: bool,
}

impl Display {
    pub fn native_id(&self) -> Option<crate::NativeDisplayId> { self.id.as_ref().and_then(|id| id.native_id()) }

    /// (x, y, width, height) in the native global, top-left coordinate space. macOS uses
    /// CGDisplayBounds points; Windows/X11 use desktop pixels; Wayland uses compositor
    /// logical coordinates. Keep the origin in this space; scale only local offsets.
    pub fn native_bounds(&self) -> [f64; 4] {
        #[cfg(target_os = "macos")]
        if let Some(crate::NativeDisplayId::MacOS(id)) = self.native_id() { return crate::platform::macos::display_bounds(id); }
        native_bounds_for(self.native_id(), self.position, self.size, self.scale)
    }

    /// Usable work area as `(origin, size)` in the same physical desktop-pixel coordinates as
    /// [`position`](Self::position) and [`size`](Self::size).
    ///
    /// On Windows, this queries `GetMonitorInfoW`'s `rcWork` (excluding the taskbar and docked
    /// app bars) for this display's `HMONITOR`, matching the region used by edge-placed
    /// overlays. If the display is no longer available, or on other platforms and headless
    /// virtual displays, this returns `(self.position, self.size)`.
    ///
    /// ```no_run
    /// rune::run_windows(|displays| {
    ///     for display in displays {
    ///         let ((x, y), (w, h)) = display.work_area();
    ///         let _ = (x, y, w, h);
    ///     }
    ///     Vec::<rune::AnyWindowSpec>::new()
    /// });
    /// ```
    pub fn work_area(&self) -> ((i32, i32), (u32, u32)) {
        #[cfg(target_os = "windows")]
        if let Some(work) = self.id.as_ref().and_then(|id| crate::platform::win32::work_area(&id.0)) {
            return work;
        }
        (self.position, self.size)
    }
}

pub(crate) fn native_bounds_for(
    native_id: Option<crate::NativeDisplayId>,
    position: (i32, i32),
    size: (u32, u32),
    scale: f64,
) -> [f64; 4] {
    let divisor = if matches!(native_id, Some(crate::NativeDisplayId::Wayland(_))) { scale } else { 1.0 };
    [f64::from(position.0), f64::from(position.1), f64::from(size.0) / divisor, f64::from(size.1) / divisor]
}

/// A window's minimum content size when neither `WindowOptions` nor `AppSpec` sets one.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const DEFAULT_MIN_SIZE: (f32, f32) = (420.0, 400.0);

#[derive(Clone, Debug)]
pub struct WindowOptions {
    pub transparent: bool,
    /// A bare window without OS decorations: for HUDs, launchers, overlays and tray pop-ups.
    /// On Windows, borderless windows stay bare with no outer resize borders or edge hit-test
    /// interception so exact display bounds and edge controls are preserved; apps that draw a
    /// custom titlebar and want OS resize edges, Aero Snap, and the system window menu
    /// should use [`App::titlebar`](crate::App::titlebar) (with
    /// [`Titlebar::controls(false)`](crate::Titlebar::controls) when drawing their own caption
    /// buttons).
    pub borderless: bool,
    /// Clicks, and so hover, pass to whatever is below.
    pub click_through: bool,
    pub always_on_top: bool,
    pub visible: bool,
    /// Physical desktop coordinates, not points. Use a display's position unchanged.
    pub position: Option<(i32, i32)>,
    /// Physical content size; overrides AppSpec's logical size when set.
    pub physical_size: Option<(u32, u32)>,
    /// Logical minimum; None inherits AppSpec's minimum, Some((0, 0)) removes the floor.
    /// Requested sizes (`Tx::resize_window`) stay inside the minimum and maximum.
    pub min_size: Option<(f32, f32)>,
    /// Logical maximum content size. None leaves it unconstrained.
    pub max_size: Option<(f32, f32)>,
    /// Whether the user can resize the window. Programmatic sizing is unaffected. A window
    /// that isn't resizable doesn't maximize either: its maximize (zoom) button is disabled.
    /// On Windows, `borderless: true` windows stay bare and do not add edge resize borders;
    /// use [`App::titlebar`](crate::App::titlebar) (or `Titlebar::new(h).controls(false)`) for
    /// a custom-titlebar window with OS edge resizing.
    pub resizable: bool,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            transparent: false, borderless: false, click_through: false,
            always_on_top: false, visible: true, position: None, physical_size: None,
            min_size: None, max_size: None, resizable: true,
        }
    }
}

impl WindowOptions {
    /// A hidden, retained launcher. On macOS, prepares its first frame at alpha zero;
    /// show/open takes keys without activating the app. Use `on_ready` to register a shortcut.
    /// Other platforms use an ordinary initially hidden window.
    pub fn launcher_panel() -> ConfiguredWindowOptions {
        let mut native = MacWindowOptions::default().nonactivating_panel(true);
        native.launcher = true;
        Self { visible: false, borderless: true, transparent: true, resizable: false,
            min_size: Some((0.0, 0.0)), ..Default::default() }.macos(native)
    }
    /// Optional AppKit settings without adding fields to existing WindowOptions literals.
    /// Ignored on other platforms.
    pub fn macos(self, native: MacWindowOptions) -> ConfiguredWindowOptions {
        ConfiguredWindowOptions { options: self, native, windows: WindowsWindowOptions::default() }
    }
    /// Optional Win32 settings without adding fields to existing `WindowOptions` literals.
    /// Ignored on other platforms.
    ///
    /// ```no_run
    /// use rune::{WindowOptions, WindowsWindowOptions};
    ///
    /// let configured = WindowOptions { borderless: true, ..Default::default() }
    ///     .windows(WindowsWindowOptions::default().undecorated_shadow(true));
    /// let _ = configured;
    /// ```
    pub fn windows(self, windows: WindowsWindowOptions) -> ConfiguredWindowOptions {
        ConfiguredWindowOptions { options: self, native: MacWindowOptions::default(), windows }
    }
    /// One transparent, borderless, topmost, click-through overlay covering this display.
    pub fn overlay(display: &Display) -> Self {
        Self {
            transparent: true, borderless: true, click_through: true, always_on_top: true,
            visible: true, position: Some(display.position), physical_size: Some(display.size),
            min_size: Some((0.0, 0.0)), max_size: None, resizable: true,
        }
    }

    /// The window's minimum and maximum content sizes, with `AppSpec::min_size` as the
    /// fallback minimum.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn size_limits(&self, app_min: Option<(f32, f32)>) -> ((f32, f32), Option<(f32, f32)>) {
        let mut min = self.min_size.or(app_min).unwrap_or(DEFAULT_MIN_SIZE);
        if let Some(max) = self.max_size { min = (min.0.min(max.0), min.1.min(max.1)); }
        (min, self.max_size)
    }

    /// The minimum a window that fits its content height opens with: the content sets the
    /// height, so the default minimum height doesn't apply.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn fit_min_size(&self, app_min: Option<(f32, f32)>) -> Option<(f32, f32)> {
        if app_min.is_none() && self.min_size.is_none() { Some((DEFAULT_MIN_SIZE.0, 0.0)) } else { app_min }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn attributes_with_theme(
        &self,
        title: &str,
        size: (f32, f32),
        app_min: Option<(f32, f32)>,
        visible: bool,
        theme: Option<winit::window::Theme>,
    ) -> winit::window::WindowAttributes {
        self.attributes(title, size, app_min, visible).with_theme(theme)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn attributes(&self, title: &str, size: (f32, f32), app_min: Option<(f32, f32)>, visible: bool) -> winit::window::WindowAttributes {
        use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
        use winit::window::{Window, WindowButtons, WindowLevel};
        let (min, _) = self.size_limits(app_min);
        let buttons = if self.resizable { WindowButtons::all() } else { WindowButtons::all() - WindowButtons::MAXIMIZE };
        let mut attrs = Window::default_attributes()
            .with_title(title)
            .with_inner_size(LogicalSize::new(size.0, size.1))
            .with_min_inner_size(LogicalSize::new(min.0, min.1))
            .with_resizable(self.resizable)
            .with_enabled_buttons(buttons)
            .with_transparent(self.transparent)
            .with_decorations(!self.borderless)
            .with_visible(visible)
            .with_window_level(if self.always_on_top { WindowLevel::AlwaysOnTop } else { WindowLevel::Normal });
        if let Some((w, h)) = self.max_size { attrs = attrs.with_max_inner_size(LogicalSize::new(w, h)); }
        if let Some((x, y)) = self.position { attrs = attrs.with_position(PhysicalPosition::new(x, y)); }
        if let Some((w, h)) = self.physical_size { attrs = attrs.with_inner_size(PhysicalSize::new(w, h)); }
        #[cfg(target_os = "windows")]
        if self.transparent && needs_no_redirection_bitmap(true, crate::render::Gpu::dx12_swapchain_kind()) {
            use winit::platform::windows::WindowAttributesExtWindows;
            // DirectComposition draws transparent windows (`Gpu::instance`); without a redirection
            // bitmap nothing beneath the swapchain paints opaque pixels behind translucent ones.
            // Plain HWND swap chains (`DxgiFromHwnd`) are opaque-only and do not use DirectComposition.
            attrs = attrs.with_no_redirection_bitmap(true);
        }
        attrs
    }
}

/// Whether a Windows window should set `WS_EX_NOREDIRECTIONBITMAP`: only when the window is
/// transparent and DirectComposition (`DxgiFromVisual`) is active.
#[cfg(any(target_os = "windows", test))]
pub(crate) const fn needs_no_redirection_bitmap(
    transparent: bool,
    swapchain: wgpu::Dx12SwapchainKind,
) -> bool {
    transparent && matches!(swapchain, wgpu::Dx12SwapchainKind::DxgiFromVisual)
}

/// Colour space for a macOS window's presentation (`NSWindow.colorSpace` and its
/// `CAMetalLayer.colorspace`).
///
/// Defaults to [`Self::Srgb`] so both the native Metal backend and the wgpu backend colour-manage
/// sRGB colours on wide-gamut Mac displays, matching AppKit windows, browsers, Windows DXGI, and
/// Linux Vulkan. Ignored on other platforms.
///
/// ```no_run
/// use rune::{App, AppSpec, MacColorSpace, MacWindowOptions, el};
///
/// App::new(AppSpec::new("Raw", (640.0, 420.0), || (), |_, _| el()))
///     .macos_window(MacWindowOptions::default().color_space(MacColorSpace::Unmanaged))
///     .run();
/// ```
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MacColorSpace {
    /// Colour-manage the window as sRGB (`NSColorSpace.sRGBColorSpace` and
    /// `kCGColorSpaceSRGB`), matching AppKit windows and Rune's native Metal backend.
    #[default]
    Srgb,
    /// Leave `NSWindow.colorSpace` unset (`nil`) and clear `CAMetalLayer.colorspace` so
    /// framebuffer values reach the display unmodified.
    Unmanaged,
}

/// AppKit properties for an ordinary window, reapplied on native recreation.
/// Omitted properties keep winit's defaults.
#[derive(Clone, Debug, Default)]
pub struct MacWindowOptions {
    pub(crate) shadow: Option<bool>,
    pub(crate) level: Option<isize>,
    pub(crate) collection_behavior: Option<usize>,
    pub(crate) exclude_from_capture: Option<bool>,
    pub(crate) nonactivating_panel: bool,
    pub(crate) launcher: bool,
    pub(crate) corner_radius: Option<f32>,
    pub(crate) corner_curve_circular: bool,
    pub(crate) accepts_first_mouse: Option<bool>,
    pub(crate) represented_file: Option<std::path::PathBuf>,
    pub(crate) title_from_file: bool,
    pub(crate) document_edited: bool,
    pub(crate) show_title: bool,
    pub(crate) color_space: MacColorSpace,
}

impl MacWindowOptions {
    /// Use a key-taking NSPanel. Open/focus never activates the app or its other windows.
    pub fn nonactivating_panel(mut self, enabled: bool) -> Self {
        self.nonactivating_panel = enabled;
        if !enabled { self.launcher = false; }
        self
    }
    pub fn shadow(mut self, enabled: bool) -> Self { self.shadow = Some(enabled); self }
    /// AppKit NSWindow.Level value (normal 0, floating 3, status 25).
    pub fn level(mut self, level: isize) -> Self { self.level = Some(level); self }
    /// AppKit NSWindow.CollectionBehavior bit mask. Replaces the existing mask.
    pub fn collection_behavior(mut self, bits: usize) -> Self { self.collection_behavior = Some(bits); self }
    /// Requests NSWindowSharingNone; capture clients should also filter window_number().
    /// false restores NSWindowSharingReadOnly. This is not a capture guarantee. A nonactivating
    /// panel applies it only while shown (see `OverlayOptions::exclude_from_capture`).
    pub fn exclude_from_capture(mut self, exclude: bool) -> Self { self.exclude_from_capture = Some(exclude); self }
    /// Rounds a transparent, borderless window or panel to `radius` points with a continuous
    /// (squircle) curve. The window's root layer clips everything it shows, Rune's drawing and
    /// native material views alike, so the root view needn't clip itself to the radius. A
    /// border drawn inside the edge is still the app's. The system shadow, when on, follows
    /// the rounded shape. Ignored for opaque or bordered windows, where AppKit owns the frame,
    /// and on other platforms.
    pub fn corner_radius(mut self, radius: f32) -> Self { self.corner_radius = Some(radius.max(0.0)); self }
    /// Use a circular corner arc instead of the continuous curve for `corner_radius`.
    pub fn corner_curve_circular(mut self) -> Self { self.corner_curve_circular = true; self }
    /// Sets `NSView.acceptsFirstMouse(for:)` through winit's `with_accepts_first_mouse` for all
    /// macOS windows (bordered, full-size-content, borderless, and non-activating panels).
    /// Pass `false` so clicking an inactive window activates it without delivering that click to
    /// the view. Omitted keeps winit's default (`true`). Retained across native recreation and
    /// ignored on other platforms.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, MacWindowOptions, el};
    ///
    /// App::new(AppSpec::new("Review", (640.0, 420.0), || (), |_, _| el()))
    ///     .macos_window(MacWindowOptions::default().accepts_first_mouse(false))
    ///     .run();
    /// ```
    pub fn accepts_first_mouse(mut self, enabled: bool) -> Self {
        self.accepts_first_mouse = Some(enabled);
        self
    }
    /// Sets the document path associated with this window (`NSWindow.representedURL`), showing
    /// AppKit's titlebar proxy icon and path menu when the native title is visible. Constructed
    /// from the path string with no filesystem `stat`. An empty path clears it. Update at
    /// runtime with [`Tx::set_represented_file`](crate::Tx::set_represented_file). Retained across
    /// native recreation and ignored on other platforms.
    ///
    /// ```no_run
    /// use rune::{App, AppSpec, MacWindowOptions, el};
    ///
    /// App::new(AppSpec::new("Notes", (640.0, 420.0), || (), |_, _| el()))
    ///     .macos_window(
    ///         MacWindowOptions::default()
    ///             .represented_file("/tmp/notes.md")
    ///             .show_title(true),
    ///     )
    ///     .run();
    /// ```
    pub fn represented_file(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.represented_file = normalize_represented_file(Some(path.into()));
        self
    }
    /// When `true`, derives the macOS window title from the represented file's final path
    /// component on creation and on each [`Tx::set_represented_file`](crate::Tx::set_represented_file)
    /// update (restoring the window's configured title when cleared). Default `false` leaves the
    /// window title untouched. Ignored on other platforms.
    ///
    /// ```no_run
    /// use rune::MacWindowOptions;
    ///
    /// let opts = MacWindowOptions::default()
    ///     .represented_file("/tmp/notes.md")
    ///     .title_from_file(true);
    /// let _ = opts;
    /// ```
    pub fn title_from_file(mut self, enabled: bool) -> Self {
        self.title_from_file = enabled;
        self
    }
    /// Sets the window's initial unsaved-changes state (`NSWindow.documentEdited`), showing the
    /// dot inside the macOS close button. Default `false`. Update at runtime with
    /// [`Tx::set_document_edited`](crate::Tx::set_document_edited). Retained across native
    /// recreation and ignored on other platforms.
    ///
    /// Following AppKit's default, marking a document as edited disables dragging its proxy icon
    /// until `document_edited` is cleared.
    ///
    /// ```no_run
    /// use rune::MacWindowOptions;
    ///
    /// let opts = MacWindowOptions::default()
    ///     .represented_file("/tmp/notes.md")
    ///     .document_edited(true);
    /// let _ = opts;
    /// ```
    pub fn document_edited(mut self, edited: bool) -> Self {
        self.document_edited = edited;
        self
    }
    /// Keeps the native macOS window title (and therefore AppKit's document proxy icon) visible
    /// when `Chrome::full_size_content` is enabled. Default `false` hides the title under a
    /// transparent full-size-content titlebar, preserving existing behavior. Ignored for
    /// borderless windows and on other platforms.
    ///
    /// ```no_run
    /// use rune::MacWindowOptions;
    ///
    /// let opts = MacWindowOptions::default()
    ///     .represented_file("/tmp/notes.md")
    ///     .show_title(true);
    /// let _ = opts;
    /// ```
    pub fn show_title(mut self, show: bool) -> Self {
        self.show_title = show;
        self
    }
    /// Sets how macOS colour-manages this window (`NSWindow.colorSpace` and its
    /// `CAMetalLayer.colorspace`).
    ///
    /// Defaults to [`MacColorSpace::Srgb`], matching AppKit windows, browsers, and Rune's
    /// native Metal backend on wide-gamut displays across both the Metal and wgpu backends.
    /// Pass [`MacColorSpace::Unmanaged`] to leave `NSWindow.colorSpace` unset (`nil`) and
    /// clear `CAMetalLayer.colorspace` so framebuffer values reach the display unmodified.
    /// Retained across native recreation and GPU surface reattach; ignored on other platforms.
    ///
    /// ```no_run
    /// use rune::{MacColorSpace, MacWindowOptions};
    ///
    /// let opts = MacWindowOptions::default().color_space(MacColorSpace::Unmanaged);
    /// let _ = opts;
    /// ```
    pub fn color_space(mut self, color_space: MacColorSpace) -> Self {
        self.color_space = color_space;
        self
    }

    /// Returns a copy with portable `exclude_from_capture` taking precedence when set, falling
    /// back to `self.exclude_from_capture` when unset.
    #[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
    pub(crate) fn with_portable_exclude(&self, portable: Option<bool>) -> Self {
        let mut next = self.clone();
        if let Some(exclude) = portable {
            next.exclude_from_capture = Some(exclude);
        }
        next
    }

    /// Returns a copy with runtime document state (`Tx::set_represented_file`,
    /// `Tx::set_document_edited`) overlaid when latched, preserving creation-time defaults when
    /// unset.
    #[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
    pub(crate) fn with_runtime_document(
        &self,
        represented_file: Option<&Option<std::path::PathBuf>>,
        document_edited: Option<&bool>,
    ) -> Self {
        let mut next = self.clone();
        if let Some(path) = represented_file {
            next.represented_file = normalize_represented_file(path.clone());
        }
        if let Some(&edited) = document_edited {
            next.document_edited = edited;
        }
        next
    }

    /// Applies macOS creation-time winit attributes (`accepts_first_mouse` for all windows, and
    /// full-size-content titlebar visibility) without changing defaults when unset.
    #[cfg(target_os = "macos")]
    pub(crate) fn apply_attributes(
        &self,
        mut attrs: winit::window::WindowAttributes,
        borderless: bool,
        full_size_content: bool,
    ) -> winit::window::WindowAttributes {
        use winit::platform::macos::WindowAttributesExtMacOS;
        if let Some(accept) = self.accepts_first_mouse {
            attrs = attrs.with_accepts_first_mouse(accept);
        }
        if !borderless && full_size_content {
            attrs = attrs
                .with_titlebar_transparent(true)
                .with_title_hidden(!self.show_title)
                .with_fullsize_content_view(true);
        }
        attrs
    }
}

/// Normalizes a represented-file path so empty paths clear the represented file, matching AppKit's
/// `NSWindow.representedFilename = ""` semantics.
pub(crate) fn normalize_represented_file(
    path: Option<std::path::PathBuf>,
) -> Option<std::path::PathBuf> {
    path.filter(|p| !p.as_os_str().is_empty())
}

/// Derives a window title from a represented file path without touching the filesystem.
/// Returns the final path component when available, falling back to the full path string for
/// root/dot paths, or `None` when empty.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(crate) fn represented_file_title(path: &std::path::Path) -> Option<String> {
    if path.as_os_str().is_empty() {
        return None;
    }
    Some(
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned()),
    )
}

/// Whether `path` explicitly indicates a directory via a trailing slash, avoiding filesystem
/// `stat` calls when constructing `NSURL::fileURLWithPath_isDirectory`.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(crate) fn represented_path_is_dir(path: &std::path::Path) -> bool {
    path.to_string_lossy().ends_with('/')
}

/// Win32 properties for an ordinary window, reapplied on native recreation and ignored on other
/// platforms. Omitted properties keep winit's defaults.
///
/// ```no_run
/// use rune::{App, AppSpec, WindowOptions, WindowsWindowOptions, el};
///
/// let options = WindowOptions { borderless: true, always_on_top: true, ..Default::default() }
///     .windows(
///         WindowsWindowOptions::default()
///             .exclude_from_capture(true)
///             .no_activate(true)
///             .tool_window(true)
///             .undecorated_shadow(true),
///     );
/// App::new(AppSpec::new("Panel", (320.0, 200.0), || (), |_, _| el()))
///     .configured_options(options)
///     .run();
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowsWindowOptions {
    pub(crate) exclude_from_capture: Option<bool>,
    pub(crate) no_activate: bool,
    pub(crate) tool_window: bool,
    pub(crate) undecorated_shadow: Option<bool>,
}

impl WindowsWindowOptions {
    /// Requests `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` (`true`) or `WDA_NONE`
    /// (`false`) before the window shows. Unset leaves Windows' default affinity unchanged.
    /// Portable [`App::exclude_from_capture`] and
    /// [`WindowHandle::set_exclude_from_capture`](crate::WindowHandle::set_exclude_from_capture)
    /// override this when set. Capture exclusion is native best effort, not a security boundary.
    ///
    /// ```no_run
    /// use rune::WindowsWindowOptions;
    ///
    /// let opts = WindowsWindowOptions::default().exclude_from_capture(true);
    /// let _ = opts;
    /// ```
    pub fn exclude_from_capture(mut self, exclude: bool) -> Self {
        self.exclude_from_capture = Some(exclude);
        self
    }

    /// Sets `WS_EX_NOACTIVATE` on the window and skips foreground activation on every reveal
    /// (create, show, and open), so clicking or showing the window does not take focus from
    /// another app. Reapplied after style-changing window operations and across recreation.
    ///
    /// ```no_run
    /// use rune::WindowsWindowOptions;
    ///
    /// let opts = WindowsWindowOptions::default().no_activate(true);
    /// let _ = opts;
    /// ```
    pub fn no_activate(mut self, enabled: bool) -> Self {
        self.no_activate = enabled;
        self
    }

    /// Sets `WS_EX_TOOLWINDOW` (and clears `WS_EX_APPWINDOW`) so the window stays off the
    /// taskbar and Alt-Tab switcher. Reapplied after style-changing window operations and across
    /// recreation.
    ///
    /// ```no_run
    /// use rune::WindowsWindowOptions;
    ///
    /// let opts = WindowsWindowOptions::default().tool_window(true);
    /// let _ = opts;
    /// ```
    pub fn tool_window(mut self, enabled: bool) -> Self {
        self.tool_window = enabled;
        self
    }

    /// Requests the system drop shadow on a borderless (`WindowOptions { borderless: true, .. }`)
    /// window via winit's `with_undecorated_shadow`. Omitted keeps winit's default (`false`).
    /// Retained across native recreation and ignored on non-Windows platforms.
    ///
    /// Client-area sizing stays unchanged; visible system borders and rounded corners depend on
    /// the Windows version and window state (for example maximized or snapped), and Windows may
    /// show a thin 1px top border line when enabled.
    ///
    /// ```no_run
    /// use rune::{WindowOptions, WindowsWindowOptions};
    ///
    /// let opts = WindowOptions { borderless: true, ..Default::default() }
    ///     .windows(WindowsWindowOptions::default().undecorated_shadow(true));
    /// let _ = opts;
    /// ```
    pub fn undecorated_shadow(mut self, enabled: bool) -> Self {
        self.undecorated_shadow = Some(enabled);
        self
    }

    /// Effective capture exclusion, with portable `exclude_from_capture` taking precedence over
    /// `self.exclude_from_capture` when set.
    #[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
    pub(crate) fn effective_exclude_from_capture(&self, portable: Option<bool>) -> Option<bool> {
        portable.or(self.exclude_from_capture)
    }

    /// Applies Windows creation-time winit attributes without changing defaults when unset.
    #[cfg(target_os = "windows")]
    pub(crate) fn apply_attributes(&self, mut attrs: winit::window::WindowAttributes) -> winit::window::WindowAttributes {
        if let Some(shadow) = self.undecorated_shadow {
            use winit::platform::windows::WindowAttributesExtWindows;
            attrs = attrs.with_undecorated_shadow(shadow);
        }
        attrs
    }
}

/// Ordinary options plus native builders, accepted by WindowSpec::configured_options.
#[derive(Clone, Debug)]
pub struct ConfiguredWindowOptions {
    pub(crate) options: WindowOptions,
    pub(crate) native: MacWindowOptions,
    pub(crate) windows: WindowsWindowOptions,
}

impl From<WindowOptions> for ConfiguredWindowOptions {
    fn from(options: WindowOptions) -> Self {
        Self { options, native: MacWindowOptions::default(), windows: WindowsWindowOptions::default() }
    }
}

impl ConfiguredWindowOptions {
    /// Amends the AppKit settings in place, keeping what a preset already set:
    /// `WindowOptions::launcher_panel().macos(|native| native.corner_radius(16.0))` stays a
    /// launcher. Ignored on other platforms.
    pub fn macos(mut self, amend: impl FnOnce(MacWindowOptions) -> MacWindowOptions) -> Self {
        self.native = amend(self.native);
        self
    }

    /// Amends the Win32 settings in place, keeping what a preset or prior `.macos(...)` call
    /// already set. Ignored on other platforms.
    ///
    /// ```no_run
    /// use rune::{MacWindowOptions, WindowOptions};
    ///
    /// let opts = WindowOptions { borderless: true, ..Default::default() }
    ///     .macos(MacWindowOptions::default().shadow(true))
    ///     .windows(|win| win.undecorated_shadow(true).tool_window(true));
    /// let _ = opts;
    /// ```
    pub fn windows(mut self, amend: impl FnOnce(WindowsWindowOptions) -> WindowsWindowOptions) -> Self {
        self.windows = amend(self.windows);
        self
    }
}

/// Forward per-window `App<S>` builder methods onto `WindowSpec<S>` so new builders don't drift.
macro_rules! forward_app_builders {
    ($(
        $(#[$meta:meta])*
        $name:ident $(=> $target:ident)? ($($arg:ident : $ty:ty),* $(,)?)
    );* $(;)?) => {
        $(
            $(#[$meta])*
            pub fn $name(mut self, $($arg: $ty),*) -> Self {
                self.app = forward_app_builders!(@call self.app, $name $(, $target)?, ($($arg),*));
                self
            }
        )*
    };
    (@call $app:expr, $name:ident, ($($arg:ident),*)) => { $app.$name($($arg),*) };
    (@call $app:expr, $_name:ident, $target:ident, ($($arg:ident),*)) => { $app.$target($($arg),*) };
}

/// One independent retained tree and app state. Share Live handles or application services
/// between states; events, focus, animation clocks and redraws remain window-local.
pub struct WindowSpec<S: 'static> {
    pub key: String,
    pub app: App<S>,
    pub options: WindowOptions,
    /// Runs on the event thread after native creation. Not called by headless tooling.
    pub on_created: Option<Box<dyn FnOnce(crate::window::WindowHandle)>>,
}

impl<S: 'static> WindowSpec<S> {
    pub fn new(key: impl Into<String>, app: AppSpec<S>) -> Self {
        Self::from_app(key, App::new(app))
    }
    /// Preserve AppHandle, windowless, keep-running and GPU-release behavior for this window.
    /// Closing a default window removes only it; quitting any app exits the whole event loop.
    /// RunOptions controls factory display discovery and tooling/window selection. App::args
    /// replaces the selected window's arguments (including native/headless mode). Isolate a
    /// host CLI before factory discovery with run_windows_with_args(options, [], factory).
    /// App::accessory is process-level: use run_windows_with(RunOptions, factory) instead.
    /// AppHandle::set_dock_icon remains global.
    pub fn from_app(key: impl Into<String>, app: App<S>) -> Self {
        Self { key: key.into(), options: app.window_options.clone(), app, on_created: None }
    }
    forward_app_builders! {
        /// Configure this window's frame, including headless traffic-light placement.
        chrome(chrome: crate::Chrome);
        /// This window draws its own titlebar, like `App::titlebar`.
        titlebar(titlebar: crate::Titlebar);
        /// Composite this window's translucent fills, blurs, shadows, images and video in sRGB
        /// (gamma) space instead of linear light, matching CSS and Core Animation. With a
        /// `Compositor`, `Frame::srgb_blend` says which encoding its passes must match.
        srgb_blend(enabled: bool);
        /// This window's font smoothing (`App::font_smoothing`).
        font_smoothing(mode: crate::FontSmoothing);
        /// Maximum number of native `El::system_material` views this window may host at once
        /// (default [`crate::MAX_NATIVE_MATERIALS`], clamped to 64), like
        /// [`App::max_native_materials`].
        max_native_materials(max: usize);
        /// Cap the wake rate (Hz) used when `El::reveal` word fades are the only active animation
        /// in this window (`0.0`, default = full display rate; `30.0` divides 60 Hz and 120 Hz
        /// displays cleanly), like [`App::reveal_fps`].
        ///
        /// ```no_run
        /// use rune::{AppSpec, WindowSpec, el};
        ///
        /// let spec = WindowSpec::new("chat", AppSpec::new("Chat", (720.0, 540.0), || (), |_, _| el()))
        ///     .reveal_fps(30.0);
        /// let _ = spec;
        /// ```
        reveal_fps(fps: f32);
        /// Size this window's height to its content, like `App::fit_content_height`.
        fit_content_height();
        /// Ask a tiling window manager to float this window when it opens, like `App::float_on_open`.
        float_on_open();
        /// Open this window without taking focus from the window that has it, such as a window a
        /// tab is dragged into while the press stays with its strip. A later `open` or `focus`
        /// activates it.
        inactive => open_inactive();
        /// Open this window maximized, like `App::maximized`.
        maximized(maximized: bool);
        /// Open this window full screen, like `App::full_screen`.
        full_screen(full_screen: bool);
        /// AppKit native window policy, like [`App::macos_window`]. Ignored on other platforms.
        ///
        /// ```no_run
        /// use rune::{AppSpec, MacWindowOptions, WindowSpec, el};
        ///
        /// let spec = WindowSpec::new("doc", AppSpec::new("Notes", (480.0, 360.0), || (), |_, _| el()))
        ///     .macos_window(MacWindowOptions::default().accepts_first_mouse(false).show_title(true));
        /// let _ = spec;
        /// ```
        macos_window(options: crate::MacWindowOptions);
        /// Win32 native window policy, like `App::windows_window`. Ignored on other platforms.
        ///
        /// ```no_run
        /// use rune::{AppSpec, WindowSpec, WindowsWindowOptions, el};
        ///
        /// let spec = WindowSpec::new("panel", AppSpec::new("Panel", (300.0, 200.0), || (), |_, _| el()))
        ///     .windows_window(WindowsWindowOptions::default().tool_window(true));
        /// let _ = spec;
        /// ```
        windows_window(options: crate::WindowsWindowOptions);
        /// Exclude this window from screen captures on Windows and macOS, like `App::exclude_from_capture`.
        ///
        /// ```no_run
        /// use rune::{AppSpec, WindowSpec, el};
        ///
        /// let spec = WindowSpec::new("notes", AppSpec::new("Notes", (480.0, 360.0), || (), |_, _| el()))
        ///     .exclude_from_capture(true);
        /// let _ = spec;
        /// ```
        exclude_from_capture(exclude: bool);
        /// Preferred native light or dark chrome for this window, like [`App::color_scheme`].
        ///
        /// ```no_run
        /// use rune::{AppSpec, ColorScheme, WindowSpec, el};
        ///
        /// let spec = WindowSpec::new("notes", AppSpec::new("Notes", (480.0, 360.0), || (), |_, _| el()))
        ///     .color_scheme(ColorScheme::Dark);
        /// let _ = spec;
        /// ```
        color_scheme(scheme: crate::ColorScheme);
        /// Opt-in whole-window system backdrop material, like [`App::window_backdrop`].
        /// On Windows 11, DWM draws the material only while the window is the foreground window
        /// and shows its own flat fallback while unfocused (so a non-activating window never shows
        /// the live material even while [`Cx::window_backdrop`](crate::Cx::window_backdrop) is
        /// active); pair with [`Self::on_focus`] if the view needs to distinguish focused state.
        ///
        /// ```no_run
        /// use rune::{AppSpec, WindowBackdrop, WindowOptions, WindowSpec, el};
        ///
        /// let spec = WindowSpec::new("notes", AppSpec::new("Notes", (480.0, 360.0), || (), |_, _| el()))
        ///     .options(WindowOptions { transparent: true, ..Default::default() })
        ///     .window_backdrop(WindowBackdrop::Mica);
        /// let _ = spec;
        /// ```
        window_backdrop(backdrop: crate::WindowBackdrop);
        /// Send `Key::Modifier` presses and releases to this window, like `App::modifier_keys`.
        modifier_keys();
        /// Deliver hardware media transport keys ([`Key::Media`](crate::Key::Media)) to this window, like [`App::media_keys`](crate::App::media_keys).
        ///
        /// ```no_run
        /// use rune::{AppSpec, WindowSpec, el};
        ///
        /// let spec = WindowSpec::new("player", AppSpec::new("Player", (480.0, 320.0), || (), |_, _| el()))
        ///     .media_keys();
        /// let _ = spec;
        /// ```
        media_keys();
        /// Tab visits every focusable control in this window, like `App::tab_visits_controls`.
        tab_visits_controls(on: bool);
        /// Release this window's GPU device and renderer when it closes, like `App::release_gpu_when_closed`.
        release_gpu_when_closed();
        /// Sets how this window responds to GPU device loss or uncaptured errors, like
        /// [`App::gpu_loss_policy`].
        ///
        /// ```no_run
        /// use rune::{AppSpec, GpuLossPolicy, WindowSpec, el};
        ///
        /// let spec = WindowSpec::new("main", AppSpec::new("Main", (800.0, 600.0), || (), |_, _| el()))
        ///     .gpu_loss_policy(GpuLossPolicy::Terminal);
        /// let _ = spec;
        /// ```
        gpu_loss_policy(policy: crate::GpuLossPolicy);
        /// Configure CPU-first startup and handover for this window, like [`App::cpu_first`].
        ///
        /// ```no_run
        /// use rune::{AppSpec, CpuFirst, WindowSpec, el};
        ///
        /// let spec = WindowSpec::new("plan", AppSpec::new("Plan", (960.0, 640.0), || (), |_, _| el()))
        ///     .cpu_first(CpuFirst::Only);
        /// let _ = spec;
        /// ```
        cpu_first(mode: crate::CpuFirst);
        /// Configure the Windows drag-only synchronized resize path for this window, like
        /// [`App::resize_sync`].
        ///
        /// ```no_run
        /// use rune::{AppSpec, ResizeSync, WindowSpec, el};
        ///
        /// let spec = WindowSpec::new("settings", AppSpec::new("Settings", (620.0, 520.0), || (), |_, _| el()))
        ///     .resize_sync(ResizeSync::On);
        /// let _ = spec;
        /// ```
        resize_sync(mode: crate::ResizeSync);
        /// Name the effect pipelines this window compiles ahead of their first use, like [`App::prewarm_effects`].
        ///
        /// ```no_run
        /// use rune::{AppSpec, PrewarmEffects, WindowSpec, el};
        ///
        /// let spec = WindowSpec::new("notes", AppSpec::new("Notes", (480.0, 360.0), || (), |_, _| el()))
        ///     .prewarm_effects(PrewarmEffects::BACKDROP_BLUR);
        /// let _ = spec;
        /// ```
        prewarm_effects(effects: impl Into<crate::PrewarmEffects>);
        on_focus(f: fn(&mut S, bool, &mut crate::Tx));
        on_modifiers(f: fn(&mut S, crate::Mods, &mut crate::Tx));
        /// This window going off screen and back, like `App::on_visibility`.
        on_visibility(f: fn(&mut S, bool, &mut crate::Tx));
        /// Runs when this window rests, like `App::on_rest`.
        on_rest(f: fn(&mut S, &mut crate::Tx) -> bool);
        /// This window's editor's input method events, like `App::on_ime`.
        on_ime(f: fn(&mut S, &crate::Ime, &mut crate::Tx) -> bool);
        /// Fallback handler for text-field menus when native pop-ups are unavailable, like `App::on_field_menu`.
        on_field_menu(f: fn(&mut S, crate::FieldMenuRequest, &mut crate::Tx));
        /// Fallback handler for pop-up and context menus when native pop-ups are unavailable
        /// (such as Linux), like [`App::on_popup_menu`].
        ///
        /// ```no_run
        /// use rune::{AppSpec, PopupMenuRequest, Tx, WindowSpec, el};
        ///
        /// let _spec = WindowSpec::new("main", AppSpec::new("Main", (480.0, 320.0), || (), |_, _| el()))
        ///     .on_popup_menu(|_: &mut (), _req: PopupMenuRequest, _tx: &mut Tx| {});
        /// ```
        on_popup_menu(f: fn(&mut S, crate::PopupMenuRequest, &mut crate::Tx));
        /// Intercepts a native close request before hiding or closing this window, like
        /// [`App::on_close_request`].
        ///
        /// ```no_run
        /// use rune::{AppSpec, CloseResponse, WindowSpec, el};
        ///
        /// let spec = WindowSpec::new("draft", AppSpec::new("Draft", (480.0, 320.0), || true, |_, _| el()))
        ///     .on_close_request(|dirty, _| {
        ///         if *dirty { CloseResponse::Defer } else { CloseResponse::Close }
        ///     });
        /// let _ = spec;
        /// ```
        on_close_request(f: fn(&mut S, &mut crate::Tx) -> crate::CloseResponse);
        /// [`App::on_quit_request`].
        ///
        /// ```no_run
        /// use rune::{AppSpec, QuitResponse, WindowSpec, el};
        ///
        /// let _window = WindowSpec::new("notes", AppSpec::new("Notes", (480.0, 320.0), || true, |_, _| el()))
        ///     .on_quit_request(|dirty, _| {
        ///         if *dirty { QuitResponse::Defer } else { QuitResponse::Quit }
        ///     });
        /// ```
        on_quit_request(f: fn(&mut S, &mut crate::Tx) -> crate::QuitResponse);
        /// Runs whenever this window closes, like `App::on_close`.
        on_close(f: fn(&mut S, &mut crate::Tx));
        /// Runs in this window when the app starts to quit, like `App::on_quit`.
        on_quit(f: fn(&mut S, &mut crate::Tx));
        /// Runs `enter` before and `leave` after each event-handling turn of this window, like `App::turn_scope`.
        turn_scope(enter: fn(&mut S), leave: fn(&mut S));
        /// Runs when the app is reopened, like `App::on_reopen`.
        on_reopen(f: fn(&mut S, bool, &mut crate::Tx) -> bool);
        /// Receives links opened with the app, like `App::on_open_url`.
        on_open_url(f: fn(&mut S, &str, &mut crate::Tx));
        /// Receives files opened with the app, like [`App::on_open_files`].
        ///
        /// ```no_run
        /// use std::path::PathBuf;
        /// use rune::{AppSpec, WindowSpec, el};
        ///
        /// let spec = WindowSpec::new("viewer", AppSpec::new("Viewer", (640.0, 420.0), Vec::<PathBuf>::new, |_, _| el()))
        ///     .on_open_files(|opened, paths, _| opened.extend(paths.iter().cloned()));
        /// let _ = spec;
        /// ```
        on_open_files(f: fn(&mut S, &[std::path::PathBuf], &mut crate::Tx));
        /// Runs on the UI thread each time the native window is created, like `App::on_window`.
        on_window(f: impl FnMut(&crate::WindowHandle) + 'static);
        /// Called on the event thread after the first frame is ready, once per native creation.
        on_ready(f: impl FnMut(&crate::WindowHandle) + 'static);
        /// Hears this window's applied content size, scale and output presence, like
        /// [`App::on_window_geometry`](crate::App::on_window_geometry).
        on_window_geometry(f: fn(&mut S, crate::WindowGeometry, &mut crate::Tx) -> bool);
        /// Runs once on the UI thread immediately before the event loop drops windows and closes
        /// the native display connection, like [`App::on_display_close`].
        ///
        /// ```no_run
        /// use rune::{AppSpec, WindowSpec, el};
        ///
        /// let spec = WindowSpec::new("main", AppSpec::new("Display", (320.0, 200.0), || (), |_, _| el()))
        ///     .on_display_close(|| {});
        /// let _ = spec;
        /// ```
        on_display_close(f: impl FnOnce() + 'static);
        /// Reports each of this window's frames, like `App::on_frame_encoded`. A spec made with
        /// `WindowSpec::from_app` keeps its app's hook unless this replaces it; windows `App::window`
        /// adds have their own.
        on_frame_encoded(f: impl FnMut(&crate::FrameEncoded) + 'static);
        /// Reports when each drawn frame of this window actually reached the screen, like
        /// [`App::on_frame_presented`]. A spec made with [`WindowSpec::from_app`] keeps its app's
        /// hook unless this replaces it; windows [`App::window`] adds have their own.
        ///
        /// ```no_run
        /// use rune::{AppSpec, WindowSpec, el};
        ///
        /// let spec = WindowSpec::new("main", AppSpec::new("Timing", (320.0, 200.0), || (), |_, _| el()))
        ///     .on_frame_presented(|p| {
        ///         let _ = (p.seq(), p.presented_at());
        ///     });
        /// let _ = spec;
        /// ```
        on_frame_presented(f: impl FnMut(&crate::FramePresented) + 'static);
        /// Runs on the UI thread after each drawn frame of this window with its runtime, like `App::on_frame_drawn`.
        on_frame_drawn(f: impl FnMut(&crate::Runtime<S>) + 'static);
        /// Cap frame pacing (Hz) for motion no recent user input is steering in this window
        /// (`0.0`, default = uncapped display rate), like [`App::ambient_fps`].
        ///
        /// Enforced on macOS `DisplayLink` windows today; ignored on Windows, Linux, web,
        /// overlays, and headless runs.
        ///
        /// ```no_run
        /// use rune::{AppSpec, WindowSpec, text};
        ///
        /// let spec = WindowSpec::new(
        ///     "chat",
        ///     AppSpec::new("Chat", (720.0, 540.0), String::new, |s, _| text(s.as_str()).wrap()),
        /// )
        /// .ambient_fps(60.0);
        /// let _ = spec;
        /// ```
        ambient_fps(fps: f32);
        /// Cap the swap chain's frame latency on macOS (`1` = 2 drawables, `2` = 3 drawables;
        /// clamped to `1..=2`, default `2`), like [`App::frame_latency`]. Ignored on Windows,
        /// Linux, and the web.
        ///
        /// With two drawables a frame that runs late waits instead of overlapping the next; at
        /// 120 Hz that can show as a dropped frame during fast animation, whereas for a rarely
        /// animating window it is the right trade. Applied at surface creation rather than live.
        ///
        /// ```no_run
        /// use rune::{AppSpec, WindowSpec, el};
        ///
        /// let _spec = WindowSpec::new(
        ///     "panel",
        ///     AppSpec::new("Panel", (488.0, 873.0), || (), |_, _| el()),
        /// )
        /// .frame_latency(1);
        /// ```
        frame_latency(frames: u32);
        /// Keep this window's layout reads across launches, like `App::layout_seed`.
        layout_seed(seed: Option<crate::LayoutSeed>, keep: impl FnMut(crate::LayoutSeed) + 'static);
        /// Draw every frame of this window through `compositor`, like `App::compositor`.
        compositor(compositor: impl crate::compose::Compositor<S>);
        /// This window's own counters for native trace summaries, like `App::dev_stats`.
        dev_stats(stats: fn() -> String);
        /// This window's playground, like `App::playground`.
        playground(playground: crate::Playground<S>);
    }
    /// Configure the underlying `App` directly for any per-window builder not forwarded here.
    pub fn map_app(mut self, f: impl FnOnce(App<S>) -> App<S>) -> Self {
        self.app = f(self.app);
        self
    }
    pub fn options(mut self, options: WindowOptions) -> Self {
        self.options = options;
        self.app.macos_window = MacWindowOptions::default();
        self.app.windows_window = WindowsWindowOptions::default();
        self
    }
    pub fn configured_options(mut self, options: ConfiguredWindowOptions) -> Self {
        let ConfiguredWindowOptions { options, native, windows } = options;
        self.options = options;
        self.app.macos_window = native;
        self.app.windows_window = windows;
        self
    }
    pub fn state(mut self, state: S) -> Self { self.app = self.app.init(move || state); self }
    pub fn on_created(mut self, callback: impl FnOnce(crate::window::WindowHandle) + 'static) -> Self {
        self.on_created = Some(Box::new(callback));
        self
    }
    /// Erase the state type, so one factory can return windows of different state types.
    pub fn boxed(self) -> AnyWindowSpec { self.into() }
}

thread_local! {
    /// Windows [`open_window`] asked for, which the event loop starts at the end of its turn.
    static OPENED: std::cell::RefCell<Vec<AnyWindowSpec>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Opens another window in the running app's event loop, with its own state, retained tree,
/// focus and clocks, as the windows a `run_windows` factory returns do: a second document
/// window, or a tab dragged out of its strip. Call it on the UI thread (a handler, an update,
/// `on_ready`); the window starts at the end of the turn. It keeps the app running as the
/// factory's windows do, so the app ends when the last of them closes. Its key must differ
/// from every open window's; a window with a key already open isn't opened. The handle places,
/// focuses and closes it, and its requests wait for it to start. Headless runs open nothing.
pub fn open_window<S: 'static>(spec: WindowSpec<S>) -> crate::WindowHandle {
    let handle = crate::WindowHandle::new(&spec.key);
    if !crate::is_headless() {
        let opened = AnyWindowSpec(Box::new(Erased { spec, handle: Some(handle.clone()), secondary: false }));
        OPENED.with(|o| o.borrow_mut().push(opened));
    }
    handle
}

/// The windows `open_window` queued since the last call.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn take_opened() -> Vec<AnyWindowSpec> {
    OPENED.with(|o| std::mem::take(&mut *o.borrow_mut()))
}

/// A window's place on the desktop, in physical pixels: what to save to reopen it there.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WindowPlacement {
    /// The frame's top-left, the space `WindowOptions::position` takes.
    pub position: (i32, i32),
    /// The content's top-left.
    pub content_position: (i32, i32),
    /// The content's size, the space `WindowOptions::physical_size` takes.
    pub content_size: (u32, u32),
    /// Physical pixels per point on the window's display.
    pub scale: f64,
    pub maximized: bool,
    pub minimized: bool,
    pub full_screen: bool,
    /// While the window is maximized or minimized, the frame restoring it returns to, as
    /// `position` and `content_size` give a frame: the system's own record on Windows
    /// (`GetWindowPlacement`), which follows the window to another display (Win+Shift+Arrow).
    /// None otherwise, and where the system keeps no such record (macOS, Linux).
    pub restore: Option<((i32, i32), (u32, u32))>,
    /// The display the window is on (the one with the most of it): its position and size, as
    /// `Display::position` and `Display::size` give them.
    pub display: Option<((i32, i32), (u32, u32))>,
}

/// How a window opens or reopens (`App::maximized`, `App::full_screen`, and state captured before hiding).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum OpenAs {
    #[default]
    Normal,
    Maximized,
    /// Full screen; leaving it returns to the frame, `maximized` or not.
    FullScreen { maximized: bool },
}

impl OpenAs {
    pub(crate) fn maximized(self, on: bool) -> OpenAs {
        match (self, on) {
            (OpenAs::FullScreen { .. }, maximized) => OpenAs::FullScreen { maximized },
            (_, true) => OpenAs::Maximized,
            (_, false) => OpenAs::Normal,
        }
    }

    pub(crate) fn full_screen(self, on: bool) -> OpenAs {
        match (self, on) {
            (OpenAs::FullScreen { maximized: true }, false) => OpenAs::Maximized,
            (OpenAs::FullScreen { maximized: false }, false) => OpenAs::Normal,
            (OpenAs::Normal, true) => OpenAs::FullScreen { maximized: false },
            (OpenAs::Maximized, true) => OpenAs::FullScreen { maximized: true },
            (state, _) => state,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn maximizes(self) -> bool {
        matches!(self, OpenAs::Maximized | OpenAs::FullScreen { maximized: true })
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn full_screens(self) -> bool {
        matches!(self, OpenAs::FullScreen { .. })
    }
}

/// A `WindowSpec` of any state type, for factories that mix them:
/// `vec![review.boxed(), settings.into()]`.
pub struct AnyWindowSpec(Box<dyn ErasedWindow>);

impl AnyWindowSpec {
    pub fn key(&self) -> &str { self.0.key() }

    /// A window `App::window` added: it closes by hiding and never keeps the loop running alone.
    pub(crate) fn secondary<T: 'static>(mut spec: WindowSpec<T>, handle: crate::WindowHandle) -> Self {
        spec.app.keep_running = true;
        Self(Box::new(Erased { spec, handle: Some(handle), secondary: true }))
    }

    pub(crate) fn resolve_args(&mut self, inherited: crate::tooling::Args) -> crate::tooling::Args { self.0.resolve_args(inherited) }

    /// Render this window headless; `scale` converts a physical size option to points.
    pub(crate) fn headless(self, args: crate::tooling::Args, scale: f32) -> Result<(), crate::tooling::StartError> { self.0.headless(args, scale) }

    /// This window as one member of a `--desktop` composite; `selected` takes `--scene`.
    pub(crate) fn desktop(self, args: &crate::tooling::Args, selected: bool, scale: f32) -> Box<dyn crate::desktop::Member> {
        self.0.desktop(args, selected, scale)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn start(self, inherited: crate::tooling::Args, cx: crate::window::Start<'_>) -> Option<Box<dyn crate::window::AnyHost>> {
        self.0.start(inherited, cx)
    }
}

impl<S: 'static> From<WindowSpec<S>> for AnyWindowSpec {
    fn from(spec: WindowSpec<S>) -> Self { Self(Box::new(Erased { spec, handle: None, secondary: false })) }
}

trait ErasedWindow {
    fn key(&self) -> &str;
    fn take_windows(&mut self) -> Vec<AnyWindowSpec>;
    fn resolve_args(&mut self, inherited: crate::tooling::Args) -> crate::tooling::Args;
    fn headless(self: Box<Self>, args: crate::tooling::Args, scale: f32) -> Result<(), crate::tooling::StartError>;
    fn desktop(self: Box<Self>, args: &crate::tooling::Args, selected: bool, scale: f32) -> Box<dyn crate::desktop::Member>;
    #[cfg(not(target_arch = "wasm32"))]
    fn start(self: Box<Self>, inherited: crate::tooling::Args, cx: crate::window::Start<'_>) -> Option<Box<dyn crate::window::AnyHost>>;
}

struct Erased<S: 'static> {
    spec: WindowSpec<S>,
    handle: Option<crate::WindowHandle>,
    secondary: bool,
}

impl<S: 'static> ErasedWindow for Erased<S> {
    fn key(&self) -> &str { &self.spec.key }
    fn take_windows(&mut self) -> Vec<AnyWindowSpec> { std::mem::take(&mut self.spec.app.windows) }
    fn resolve_args(&mut self, inherited: crate::tooling::Args) -> crate::tooling::Args { self.spec.app.resolve_args(inherited) }
    // A trait object's methods are all linked with it: without the `tooling` feature these
    // bodies (never called then: `run` refuses a headless run first) keep headless rendering
    // out of a release app.
    #[cfg(not(any(feature = "tooling", debug_assertions)))]
    fn headless(self: Box<Self>, _: crate::tooling::Args, _: f32) -> Result<(), crate::tooling::StartError> { unreachable!("{}", crate::tooling::WITHOUT_TOOLING) }
    #[cfg(not(any(feature = "tooling", debug_assertions)))]
    fn desktop(self: Box<Self>, _: &crate::tooling::Args, _: bool, _: f32) -> Box<dyn crate::desktop::Member> { unreachable!("{}", crate::tooling::WITHOUT_TOOLING) }
    #[cfg(any(feature = "tooling", debug_assertions))]
    fn headless(self: Box<Self>, args: crate::tooling::Args, scale: f32) -> Result<(), crate::tooling::StartError> {
        crate::tooling::headless_window_spec(self.spec.app, self.spec.options, args, scale)
    }
    #[cfg(any(feature = "tooling", debug_assertions))]
    fn desktop(self: Box<Self>, args: &crate::tooling::Args, selected: bool, scale: f32) -> Box<dyn crate::desktop::Member> {
        let Erased { spec, handle, secondary } = *self;
        Box::new(crate::desktop::Window::new(spec, handle, args, selected, !secondary, scale))
    }
    #[cfg(not(target_arch = "wasm32"))]
    fn start(self: Box<Self>, inherited: crate::tooling::Args, cx: crate::window::Start<'_>) -> Option<Box<dyn crate::window::AnyHost>> {
        let inherited = secondary_args(self.secondary, &self.spec.key, inherited);
        crate::window::start(self.spec, self.handle, !self.secondary, inherited, cx)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn secondary_args(secondary: bool, key: &str, mut inherited: crate::tooling::Args) -> crate::tooling::Args {
    // A secondary window is not what --scene or --size describe unless --window selects it.
    if secondary && inherited.window.as_deref() != Some(key) {
        inherited.scene = None;
        inherited.size = None;
    }
    inherited
}

/// Each window followed by those its app added with `App::window`, depth first.
pub(crate) fn flatten(windows: impl IntoIterator<Item = AnyWindowSpec>) -> Vec<AnyWindowSpec> {
    let mut out = Vec::new();
    for mut window in windows {
        let children = window.0.take_windows();
        out.push(window);
        out.extend(flatten(children));
    }
    out
}

pub(crate) fn validate(windows: &[AnyWindowSpec]) -> Result<(), String> {
    let mut keys = std::collections::HashSet::new();
    for window in windows {
        if window.key().is_empty() || !keys.insert(window.key()) { return Err(format!("empty or duplicate window key: {:?}", window.key())); }
    }
    Ok(())
}

#[cfg(test)]
#[path = "spec_tests.rs"]
mod tests;
