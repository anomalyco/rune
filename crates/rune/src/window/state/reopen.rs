//! What a window reopens as when its native window is destroyed while hidden
//! (`App::release_gpu_when_closed`, Wayland unmapping, or GPU surface loss) and recreated on a
//! later `show` or `open`.
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize, Position};
use winit::window::{Fullscreen, WindowAttributes};

use crate::windows::OpenAs;

/// Restored (non-maximized, non-full-screen, non-minimized) content size captured from a live
/// window so a recreated window opens with the same restored frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::window) enum RestoredSize {
    Logical(f32, f32),
    Physical(u32, u32),
}

/// Snapshot of a live native window's presentation state and geometry.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(in crate::window) struct LiveWindowState {
    pub maximized: bool,
    pub full_screen: bool,
    pub minimized: bool,
    /// Content size in logical points (`inner_size / scale`), when non-zero.
    pub logical_size: Option<(f32, f32)>,
    /// Content size in physical pixels (`inner_size`), when non-zero.
    pub physical_size: Option<(u32, u32)>,
    /// Outer window position (physical on Windows/X11, logical on macOS, `None` on Wayland).
    pub position: Option<Position>,
    /// OS-recorded restored frame `((outer_x, outer_y), (content_w, content_h))` in physical
    /// pixels while maximized or minimized (Windows `GetWindowPlacement`), when available.
    pub restore_frame: Option<((i32, i32), (u32, u32))>,
}

/// What `Host::try_create` asks the platform for when creating or recreating a native window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::window) struct CreatePlan {
    pub open_as: OpenAs,
    pub size: Option<RestoredSize>,
    pub position: Option<Position>,
}

/// Whether recreated windows should pass physical pixels for their restored content size
/// (Windows and X11, matching `WindowOptions::physical_size` and multi-monitor DPI handling)
/// instead of logical points (macOS AppKit points and Wayland `xdg_toplevel` logical units).
pub(in crate::window) fn use_physical_size(wayland: bool) -> bool {
    cfg!(target_os = "windows") || (!cfg!(target_os = "macos") && !wayland)
}

/// Tracks a window's opening mode and restored frame across hide/release and recreation.
///
/// Before the window has ever been shown, `open_as` holds the app's configured opening mode
/// (`App::maximized`, `App::full_screen`). Once the window is live, observed state replaces that
/// opening preference so the user's last state wins on recreation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::window) struct ReopenState {
    open_as: OpenAs,
    maximized_before_full_screen: bool,
    restored_size: Option<RestoredSize>,
    restored_position: Option<Position>,
}

impl ReopenState {
    pub(in crate::window) fn new(initial: OpenAs) -> Self {
        Self {
            open_as: initial,
            maximized_before_full_screen: initial.maximizes(),
            restored_size: None,
            restored_position: None,
        }
    }

    /// Records a live window's presentation state and restored bounds.
    ///
    /// While minimized, `open_as` and live bounds are left untouched so a minimized window's zero
    /// size or off-screen coordinates never clobber the window's real state; an OS-reported
    /// `restore_frame` still updates the restored frame. While maximized or full screen, the
    /// monitor-filling live bounds never overwrite the restored size or position.
    pub(in crate::window) fn record_live(
        &mut self,
        state: LiveWindowState,
        use_physical_size: bool,
        allow_position: bool,
        track_size: bool,
    ) {
        if state.minimized {
            self.apply_restore_frame(state.restore_frame, allow_position, track_size);
            return;
        }
        if state.full_screen {
            let maximized = state.maximized || self.maximized_before_full_screen;
            self.open_as = OpenAs::FullScreen { maximized };
            return;
        }
        if state.maximized {
            self.open_as = OpenAs::Maximized;
            self.maximized_before_full_screen = true;
            self.apply_restore_frame(state.restore_frame, allow_position, track_size);
            return;
        }
        self.open_as = OpenAs::Normal;
        self.maximized_before_full_screen = false;
        if track_size {
            let size = if use_physical_size {
                state.physical_size
                    .filter(|&(w, h)| w > 0 && h > 0)
                    .map(|(w, h)| RestoredSize::Physical(w, h))
            } else {
                state.logical_size
                    .filter(|&(w, h)| w > 0.0 && h > 0.0 && w.is_finite() && h.is_finite())
                    .map(|(w, h)| RestoredSize::Logical(w, h))
            };
            if let Some(size) = size {
                self.restored_size = Some(size);
            }
        }
        if allow_position && let Some(pos) = state.position {
            self.restored_position = Some(pos);
        }
    }

    fn apply_restore_frame(
        &mut self,
        restore_frame: Option<((i32, i32), (u32, u32))>,
        allow_position: bool,
        track_size: bool,
    ) {
        let Some(((x, y), (w, h))) = restore_frame else { return };
        if allow_position {
            self.restored_position = Some(Position::Physical(PhysicalPosition::new(x, y)));
        }
        if track_size && w > 0 && h > 0 {
            self.restored_size = Some(RestoredSize::Physical(w, h));
        }
    }

    pub(in crate::window) fn clear_size(&mut self) {
        self.restored_size = None;
    }

    pub(in crate::window) fn clear_position(&mut self) {
        self.restored_position = None;
    }

    pub(in crate::window) fn create_plan(&self, resizable: bool, panel: bool) -> CreatePlan {
        let open_as = if resizable && !panel {
            self.open_as
        } else {
            OpenAs::Normal
        };
        CreatePlan {
            open_as,
            size: self.restored_size,
            position: self.restored_position,
        }
    }
}

/// Applies `plan` to `attrs` before window creation.
///
/// On Windows (`windows_deferred_open_as = true`), maximizing and full-screening happen once the
/// window is cloaked because maximizing shows a window; `plan.open_as` is applied there instead.
pub(in crate::window) fn apply_create_plan(
    mut attrs: WindowAttributes,
    plan: CreatePlan,
    windows_deferred_open_as: bool,
) -> WindowAttributes {
    if let Some(size) = plan.size {
        attrs = match size {
            RestoredSize::Logical(w, h) => attrs.with_inner_size(LogicalSize::new(w, h)),
            RestoredSize::Physical(w, h) => attrs.with_inner_size(PhysicalSize::new(w, h)),
        };
    }
    if let Some(pos) = plan.position {
        attrs = attrs.with_position(pos);
    }
    if !windows_deferred_open_as {
        attrs = attrs.with_maximized(plan.open_as.maximizes());
        if plan.open_as.full_screens() {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }
    }
    attrs
}

#[cfg(test)]
#[path = "reopen_tests.rs"]
mod tests;
