//! What a window holds per OS: on macOS its display link, present hook, panel and native
//! views; on Linux desktops its activation helper; elsewhere nothing.

/// Dropped before the surface and the window, so its AppKit observers and views go while both
/// still exist.
#[cfg(target_os = "macos")]
pub(super) struct Native {
    pub(super) pacing: Option<crate::display_link::DisplayLink>,
    /// Reports when each drawable reached the screen, while tracing or for `shadow`.
    pub(super) present: Option<crate::present_time::metal::LayerHook>,
    /// Re-samples a transparent window's system shadow once its new pixels land.
    pub(super) shadow: Option<crate::macos_window_shape::Shadow>,
    pub(super) panel: Option<crate::macos::Panel>,
    /// `El::system_material` views behind the Metal layer.
    pub(super) materials: crate::system_material::macos::Materials,
    pub(super) help: crate::macos_help::NativeHelp,
    /// Redraws on full screen changes.
    pub(super) full_screen: super::mac::FullScreenWatch,
    /// Fn presses, which winit doesn't send, while the app wants modifier keys.
    pub(super) fn_key: Option<crate::fn_key::FnKey>,
    /// Initial frames (`Host::try_create` and one follow-up) that may draw before AppKit
    /// reports the new window on screen.
    pub(super) prime: u8,
    pub(super) launch_drawables: super::LaunchDrawables,
    /// Forgets the window's traffic-light spot.
    pub(super) _spot: crate::macos_traffic_lights::Spot,
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
#[derive(Default)]
pub(super) struct Native {
    pub(super) activator: Option<crate::activation::Activator>,
}

#[cfg(not(any(target_os = "macos", all(unix, not(target_os = "ios"), not(target_os = "android")))))]
#[derive(Default)]
pub(super) struct Native;
