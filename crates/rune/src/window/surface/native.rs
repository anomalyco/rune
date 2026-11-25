//! What a window holds per OS: on macOS its display link, present hook, panel and native
//! views; on Linux desktops its activation helper; on Windows its first-frame cloak; elsewhere
//! nothing.

#[cfg(target_os = "macos")]
use std::sync::Arc;

/// Dropped before the surface and the window, so its AppKit observers and views go while both
/// still exist.
#[cfg(target_os = "macos")]
pub(in crate::window) struct Native {
    pub(in crate::window) pacing: Option<crate::platform::display_link::DisplayLink>,
    /// Reports when each drawable reached the screen, while tracing or for `shadow`.
    pub(in crate::window) present: Option<crate::frame::present_time::metal::LayerHook>,
    /// The log `present` and `shadow` report to (traced, or an untraced one that only re-samples
    /// the shadow), kept for the window's life: a surface replaced after a device loss attaches
    /// the new hook to the same log, so the shadow keeps re-sampling.
    pub(in crate::window) present_log: Option<Arc<crate::frame::present_time::metal::Log>>,
    /// Re-samples a transparent window's system shadow once its new pixels land.
    pub(in crate::window) shadow: Option<crate::platform::macos::window_shape::Shadow>,
    pub(in crate::window) panel: Option<crate::platform::macos::Panel>,
    /// `El::system_material` views behind the Metal layer.
    pub(in crate::window) materials: crate::system_material::macos::Materials,
    pub(in crate::window) help: crate::platform::macos::help::NativeHelp,
    /// Redraws on full screen changes.
    pub(in crate::window) full_screen: crate::window::chrome::mac::FullScreenWatch,
    /// Fn presses, which winit doesn't send, while the app wants modifier keys.
    pub(in crate::window) fn_key: Option<crate::input::fn_key::FnKey>,
    /// Hardware media keys (`NSEventTypeSystemDefined`), while the app opts into `App::media_keys`.
    pub(in crate::window) media_keys: Option<crate::input::media_keys::MediaKeys>,
    /// The new window's first frames, which may draw before AppKit reports it on screen.
    pub(in crate::window) prime: Prime,
    pub(in crate::window) launch_drawables: super::LaunchDrawables,
    /// Forgets the window's traffic-light spot.
    pub(in crate::window) _spot: crate::platform::macos::traffic_lights::Spot,
    /// Forgets the window's drag-destination state.
    pub(in crate::window) _drop: Option<crate::platform::macos::drop::DropTarget>,
}

/// Whether a frame may presume the window on screen before AppKit says so: `Host::try_create`'s
/// frame, a panel ordered in (`Win::show`, an overlay's show), and one follow-up do, since the
/// window reaches the screen with that turn's Core Animation commit. Once AppKit answers, or the
/// window leaves the screen, none does.
#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Prime {
    Launching { left: u8 },
    Steady,
}

#[cfg(target_os = "macos")]
impl Prime {
    pub(in crate::window) fn launching() -> Self { Prime::Launching { left: 2 } }

    /// Presumes the window shown for the next two frames (used when ordering a panel in).
    pub(crate) fn arm(&mut self) { *self = Self::launching(); }

    pub(crate) fn presumes(self) -> bool { matches!(self, Prime::Launching { .. }) }

    /// A frame drew presuming it shown.
    pub(crate) fn spend(&mut self) {
        if let Prime::Launching { left } = *self {
            *self = if left > 1 { Prime::Launching { left: left - 1 } } else { Prime::Steady };
        }
    }

    pub(crate) fn stop(&mut self) { *self = Prime::Steady; }
}

#[cfg(all(test, target_os = "macos"))]
#[path = "native_tests.rs"]
mod tests;

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
#[derive(Default)]
pub(in crate::window) struct Native {
    pub(in crate::window) activator: Option<crate::platform::activation::Activator>,
    /// Whether the CPU renderer or the GPU draws its frames (`linux::cpu_frame`).
    #[cfg(target_os = "linux")]
    pub(in crate::window) mode: crate::window::linux::handover::SurfaceMode,
    /// Lazy `wl_data_device` binding state machine (pre-first-frame, bound, teardown grace, unavailable).
    pub(in crate::window) drop_binding: crate::platform::wayland::drop::binding::State,
    pub(in crate::window) drop_seq: crate::machine::Seq,
    /// Unregisters the window's Wayland drop target before the window's `wl_surface` drops.
    pub(in crate::window) drop_target: Option<crate::platform::wayland::drop::DropTarget>,
    /// Per-surface Wayland behind-window blur controller, dropped strictly before `wl_surface`.
    pub(in crate::window) wayland_backdrop: Option<crate::platform::wayland::backdrop::SurfaceBlur>,
}

#[cfg(target_os = "windows")]
#[derive(Default)]
pub(in crate::window) struct Native {
    /// Holds a window that's just been shown off screen until its frame lands.
    pub(in crate::window) cloak: crate::platform::win32::Cloak,
    /// The frame without a title bar a window with `App::titlebar` has.
    pub(in crate::window) frame: Option<crate::platform::win32::CustomFrame>,
    /// Lightweight `WM_NCHITTEST` subclass for windows without `frame` that have `.window_drag()`.
    pub(in crate::window) drag_regions: Option<crate::platform::win32::DragRegions>,
    /// Tracks the last commit/zoom/fullscreen/motion state synced into Win32 drag hit-testing so
    /// idle and non-committing frames do zero work.
    pub(in crate::window) drag_sync: crate::platform::win32::DragHitSync,
    /// Whether other windows cover it whole, as the last frame that looked found (`look`).
    pub(in crate::window) look: crate::window::win32::look::Look,
    /// Whether its swap chain can take the next frame (`slot`).
    pub(in crate::window) slot: crate::window::win32::slot::Slot,
    /// Paces sub-frame `poll` wakes onto the display's vblank grid (`poll_pace`).
    pub(in crate::window) poll_pace: crate::window::win32::poll_pace::PollPace,
    /// What its last frames changed, so the next redraws only that (`damage`).
    pub(in crate::window) damage: crate::window::win32::damage::Damage,
    /// Opt-in Windows 11 `El::system_material` visuals behind the swap chain. Declared before
    /// `mode` so `DesktopWindowTarget` closes before `Handover` drops.
    #[cfg(feature = "windows-system-material")]
    pub(in crate::window) materials: crate::system_material::windows::Materials,
    /// Whether the CPU renderer or the GPU draws its frames (`cpu_frame`).
    pub(in crate::window) mode: crate::window::win32::handover::SurfaceMode,
    /// Whether its accessibility adapter waits for the first frame (`Host::a11y_after_frame`).
    pub(in crate::window) a11y_start: A11yStart,
    /// Opt-in synchronized drag resize state and D3D11 copy-model resources (`resize_sync`).
    pub(in crate::window) resize_sync: crate::window::win32::resize_sync::DragResizeSync,
    /// Playing clips shown on their own layer under the UI (`video_layer`).
    pub(in crate::window) video_layers: crate::window::win32::video_layer::VideoLayers,
    /// Revokes the window's OLE drop target and prunes its registry entry when the window goes.
    pub(in crate::window) drop_target: Option<crate::platform::win32::drop_target::DropTarget>,
    /// Cached DWM system backdrop state for this live `HWND` (`platform::win32::backdrop`).
    pub(in crate::window) backdrop: crate::platform::win32::backdrop::BackdropTracker,
    /// Last observed maximized state, so maximize/unmaximize transitions re-apply an active DWM
    /// backdrop without touching plain drag-resizes.
    pub(in crate::window) last_maximized: bool,
}

/// When a window's UI Automation adapter is made: with the window (a screen reader is running),
/// or on the loop's first idle turn after the first frame.
#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::window) enum A11yStart {
    #[default]
    Made,
    AfterFirstFrame,
}

#[cfg(not(any(target_os = "macos", target_os = "windows", all(unix, not(target_os = "ios"), not(target_os = "android")))))]
#[derive(Default)]
pub(in crate::window) struct Native;
