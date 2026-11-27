use crate::dpi::{PhysicalPosition, PhysicalSize, Size};
use crate::icon::Icon;
use crate::keyboard::ModifiersState;
use crate::platform_impl::platform::{event_loop, util, Fullscreen, SelectedCursor};
use crate::window::{Theme, WindowAttributes};
use bitflags::bitflags;
use std::io;
use std::sync::MutexGuard;
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{InvalidateRgn, MonitorFromRect, MONITOR_DEFAULTTONULL};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, EnableMenuItem, GetMenu, GetSystemMenu, GetWindowLongW, SendMessageW,
    SetWindowLongW, SetWindowPlacement, SetWindowPos, ShowWindow, GWL_EXSTYLE, GWL_STYLE,
    HWND_BOTTOM, HWND_NOTOPMOST, HWND_TOPMOST, MF_BYCOMMAND, MF_DISABLED, MF_ENABLED, SC_CLOSE,
    SWP_ASYNCWINDOWPOS, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOREPOSITION, SWP_NOSIZE,
    SWP_NOZORDER, SW_HIDE, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, SW_SHOW, SW_SHOWMAXIMIZED,
    SW_SHOWMINIMIZED, SW_SHOWNOACTIVATE, WINDOWPLACEMENT, WINDOW_EX_STYLE, WINDOW_STYLE, WS_BORDER,
    WS_CAPTION, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_ACCEPTFILES, WS_EX_APPWINDOW,
    WS_EX_CLIENTEDGE, WS_EX_DLGMODALFRAME, WS_EX_LAYERED, WS_EX_NOREDIRECTIONBITMAP,
    WS_EX_STATICEDGE, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_EX_WINDOWEDGE, WS_MAXIMIZE,
    WS_MAXIMIZEBOX, WS_MINIMIZE, WS_MINIMIZEBOX, WS_OVERLAPPEDWINDOW, WS_POPUP, WS_SIZEBOX,
    WS_SYSMENU, WS_VISIBLE,
};

/// Contains information about states and the window that the callback is going to use.
pub(crate) struct WindowState {
    pub mouse: MouseProperties,

    /// Used by `WM_GETMINMAXINFO`.
    pub min_size: Option<Size>,
    pub max_size: Option<Size>,

    pub resize_increments: Option<Size>,

    pub window_icon: Option<Icon>,
    pub taskbar_icon: Option<Icon>,

    pub saved_window: Option<SavedWindow>,
    pub scale_factor: f64,

    pub modifiers_state: ModifiersState,
    pub fullscreen: Option<Fullscreen>,
    pub current_theme: Theme,
    pub preferred_theme: Option<Theme>,

    pub window_flags: WindowFlags,

    pub ime_state: ImeState,
    pub ime_allowed: bool,

    // Used by WM_NCACTIVATE, WM_SETFOCUS and WM_KILLFOCUS
    pub is_active: bool,
    pub is_focused: bool,

    // Flag whether redraw was requested.
    pub redraw_requested: bool,

    pub dragging: bool,

    pub skip_taskbar: bool,
}

#[derive(Clone)]
pub struct SavedWindow {
    pub placement: WINDOWPLACEMENT,
    pub rect: RECT,
}

/// Bounds applied atomically with a fullscreen style change so Windows never composes an
/// intermediate frame with fullscreen styles at windowed bounds or windowed styles at
/// fullscreen bounds.
pub(crate) enum FullscreenBounds {
    Rect(RECT),
    Restore(SavedWindow),
    FrameOnly,
}

#[derive(Clone)]
pub struct MouseProperties {
    pub(crate) selected_cursor: SelectedCursor,
    pub capture_count: u32,
    cursor_flags: CursorFlags,
    pub last_position: Option<PhysicalPosition<f64>>,
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct CursorFlags: u8 {
        const GRABBED   = 1 << 0;
        const HIDDEN    = 1 << 1;
        const IN_WINDOW = 1 << 2;
        const LOCKED    = 1 << 3;
    }
}
bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct WindowFlags: u32 {
        const RESIZABLE         = 1 << 0;
        const MINIMIZABLE       = 1 << 1;
        const MAXIMIZABLE       = 1 << 2;
        const CLOSABLE          = 1 << 3;
        const VISIBLE           = 1 << 4;
        const ON_TASKBAR        = 1 << 5;
        const ALWAYS_ON_TOP     = 1 << 6;
        const ALWAYS_ON_BOTTOM  = 1 << 7;
        const NO_BACK_BUFFER    = 1 << 8;
        const TRANSPARENT       = 1 << 9;
        const CHILD             = 1 << 10;
        const MAXIMIZED         = 1 << 11;
        const POPUP             = 1 << 12;

        /// Marker flag for fullscreen. Should always match `WindowState::fullscreen`, but is
        /// included here to make masking easier.
        const MARKER_EXCLUSIVE_FULLSCREEN = 1 << 13;
        const MARKER_BORDERLESS_FULLSCREEN = 1 << 14;

        /// The `WM_SIZE` event contains some parameters that can effect the state of `WindowFlags`.
        /// In most cases, it's okay to let those parameters change the state. However, when we're
        /// running the `WindowFlags::apply_diff` function, we *don't* want those parameters to
        /// effect our stored state, because the purpose of `apply_diff` is to update the actual
        /// window's state to match our stored state. This controls whether to accept those changes.
        const MARKER_RETAIN_STATE_ON_SIZE = 1 << 15;

        const MARKER_IN_SIZE_MOVE = 1 << 16;

        const MINIMIZED = 1 << 17;

        const IGNORE_CURSOR_EVENT = 1 << 18;

        /// Fully decorated window (incl. caption, border and drop shadow).
        const MARKER_DECORATIONS = 1 << 19;
        /// Drop shadow for undecorated windows.
        const MARKER_UNDECORATED_SHADOW = 1 << 20;

        const MARKER_ACTIVATE = 1 << 21;

        const CLIP_CHILDREN = 1 << 22;

        const EXCLUSIVE_FULLSCREEN_OR_MASK = WindowFlags::ALWAYS_ON_TOP.bits();
    }
}

#[derive(Eq, PartialEq)]
pub enum ImeState {
    Disabled,
    Enabled,
    Preedit,
}

impl WindowState {
    pub(crate) fn new(
        attributes: &WindowAttributes,
        scale_factor: f64,
        current_theme: Theme,
        preferred_theme: Option<Theme>,
    ) -> WindowState {
        WindowState {
            mouse: MouseProperties {
                selected_cursor: SelectedCursor::default(),
                capture_count: 0,
                cursor_flags: CursorFlags::empty(),
                last_position: None,
            },

            min_size: attributes.min_inner_size,
            max_size: attributes.max_inner_size,

            resize_increments: attributes.resize_increments,

            window_icon: attributes.window_icon.clone(),
            taskbar_icon: None,

            saved_window: None,
            scale_factor,

            modifiers_state: ModifiersState::default(),
            fullscreen: None,
            current_theme,
            preferred_theme,
            window_flags: WindowFlags::empty(),

            ime_state: ImeState::Disabled,
            ime_allowed: false,

            is_active: false,
            is_focused: false,
            redraw_requested: false,

            dragging: false,

            skip_taskbar: false,
        }
    }

    pub fn window_flags(&self) -> WindowFlags {
        self.window_flags
    }

    pub fn set_window_flags<F>(mut this: MutexGuard<'_, Self>, window: HWND, f: F)
    where
        F: FnOnce(&mut WindowFlags),
    {
        let old_flags = this.window_flags;
        f(&mut this.window_flags);
        let new_flags = this.window_flags;

        drop(this);
        old_flags.apply_diff(window, new_flags);
    }

    pub fn set_fullscreen_flags<F>(
        mut this: MutexGuard<'_, Self>,
        window: HWND,
        bounds: FullscreenBounds,
        f: F,
    ) where
        F: FnOnce(&mut WindowFlags),
    {
        let old_flags = this.window_flags;
        f(&mut this.window_flags);
        let new_flags = this.window_flags;

        drop(this);
        old_flags.apply_fullscreen_diff(window, new_flags, bounds);
    }

    pub fn set_window_flags_in_place<F>(&mut self, f: F)
    where
        F: FnOnce(&mut WindowFlags),
    {
        f(&mut self.window_flags);
    }

    pub fn has_active_focus(&self) -> bool {
        self.is_active && self.is_focused
    }

    // Updates is_active and returns whether active-focus state has changed
    pub fn set_active(&mut self, is_active: bool) -> bool {
        let old = self.has_active_focus();
        self.is_active = is_active;
        old != self.has_active_focus()
    }

    // Updates is_focused and returns whether active-focus state has changed
    pub fn set_focused(&mut self, is_focused: bool) -> bool {
        let old = self.has_active_focus();
        self.is_focused = is_focused;
        old != self.has_active_focus()
    }
}

impl MouseProperties {
    pub fn cursor_flags(&self) -> CursorFlags {
        self.cursor_flags
    }

    pub fn set_cursor_flags<F>(&mut self, window: HWND, f: F) -> Result<(), io::Error>
    where
        F: FnOnce(&mut CursorFlags),
    {
        let old_flags = self.cursor_flags;
        f(&mut self.cursor_flags);
        match self.cursor_flags.refresh_os_cursor(window) {
            Ok(()) => (),
            Err(e) => {
                self.cursor_flags = old_flags;
                return Err(e);
            },
        }

        Ok(())
    }
}

impl WindowFlags {
    fn mask(mut self) -> WindowFlags {
        if self.contains(WindowFlags::MARKER_EXCLUSIVE_FULLSCREEN) {
            self |= WindowFlags::EXCLUSIVE_FULLSCREEN_OR_MASK;
        }
        self
    }

    pub fn to_window_styles(self) -> (WINDOW_STYLE, WINDOW_EX_STYLE) {
        // Required styles to properly support common window functionality like aero snap.
        let mut style = WS_CAPTION | WS_BORDER | WS_CLIPSIBLINGS | WS_SYSMENU;
        let mut style_ex = WS_EX_WINDOWEDGE | WS_EX_ACCEPTFILES;

        if self.contains(WindowFlags::RESIZABLE) {
            style |= WS_SIZEBOX;
        }
        if self.contains(WindowFlags::MAXIMIZABLE) {
            style |= WS_MAXIMIZEBOX;
        }
        if self.contains(WindowFlags::MINIMIZABLE) {
            style |= WS_MINIMIZEBOX;
        }
        if self.contains(WindowFlags::VISIBLE) {
            style |= WS_VISIBLE;
        }
        if self.contains(WindowFlags::ON_TASKBAR) {
            style_ex |= WS_EX_APPWINDOW;
        }
        if self.contains(WindowFlags::ALWAYS_ON_TOP) {
            style_ex |= WS_EX_TOPMOST;
        }
        if self.contains(WindowFlags::NO_BACK_BUFFER) {
            style_ex |= WS_EX_NOREDIRECTIONBITMAP;
        }
        if self.contains(WindowFlags::CHILD) {
            style |= WS_CHILD; // This is incompatible with WS_POPUP if that gets added eventually.

            // Remove decorations window styles for child
            if !self.contains(WindowFlags::MARKER_DECORATIONS) {
                style &= !(WS_CAPTION | WS_BORDER);
                style_ex &= !WS_EX_WINDOWEDGE;
            }
        }
        if self.contains(WindowFlags::POPUP) {
            style |= WS_POPUP;
        }
        if self.contains(WindowFlags::MINIMIZED) {
            style |= WS_MINIMIZE;
        }
        if self.contains(WindowFlags::MAXIMIZED) {
            style |= WS_MAXIMIZE;
        }
        if self.contains(WindowFlags::IGNORE_CURSOR_EVENT) {
            style_ex |= WS_EX_TRANSPARENT | WS_EX_LAYERED;
        }
        if self.contains(WindowFlags::CLIP_CHILDREN) {
            style |= WS_CLIPCHILDREN;
        }

        if self.intersects(
            WindowFlags::MARKER_EXCLUSIVE_FULLSCREEN | WindowFlags::MARKER_BORDERLESS_FULLSCREEN,
        ) {
            style &= !WS_OVERLAPPEDWINDOW;
            style_ex &=
                !(WS_EX_DLGMODALFRAME | WS_EX_WINDOWEDGE | WS_EX_CLIENTEDGE | WS_EX_STATICEDGE);
        }

        (style, style_ex)
    }

    /// Adjust the window client rectangle to the return value, if present.
    fn apply_diff(mut self, window: HWND, mut new: WindowFlags) {
        self = self.mask();
        new = new.mask();

        let mut diff = self ^ new;

        if diff == WindowFlags::empty() {
            return;
        }

        if new.contains(WindowFlags::VISIBLE) {
            let flag = if !self.contains(WindowFlags::MARKER_ACTIVATE) {
                self.set(WindowFlags::MARKER_ACTIVATE, true);
                SW_SHOWNOACTIVATE
            } else {
                SW_SHOW
            };
            unsafe {
                ShowWindow(window, flag);
            }
        }

        if diff.intersects(WindowFlags::ALWAYS_ON_TOP | WindowFlags::ALWAYS_ON_BOTTOM) {
            unsafe {
                SetWindowPos(
                    window,
                    match (
                        new.contains(WindowFlags::ALWAYS_ON_TOP),
                        new.contains(WindowFlags::ALWAYS_ON_BOTTOM),
                    ) {
                        (true, false) => HWND_TOPMOST,
                        (false, false) => HWND_NOTOPMOST,
                        (false, true) => HWND_BOTTOM,
                        (true, true) => unreachable!(),
                    },
                    0,
                    0,
                    0,
                    0,
                    SWP_ASYNCWINDOWPOS | SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
                InvalidateRgn(window, 0, false.into());
            }
        }

        if diff.contains(WindowFlags::MAXIMIZED) || new.contains(WindowFlags::MAXIMIZED) {
            unsafe {
                ShowWindow(window, match new.contains(WindowFlags::MAXIMIZED) {
                    true => SW_MAXIMIZE,
                    false => SW_RESTORE,
                });
            }
        }

        // Minimize operations should execute after maximize for proper window animations
        if diff.contains(WindowFlags::MINIMIZED) {
            unsafe {
                ShowWindow(window, match new.contains(WindowFlags::MINIMIZED) {
                    true => SW_MINIMIZE,
                    false => SW_RESTORE,
                });
            }

            diff.remove(WindowFlags::MINIMIZED);
        }

        // A new window's system menu has Close enabled: the menu (`GetSystemMenu` makes the
        // window its own copy, about 1,200 us before its first frame) is touched only to change it.
        if diff.contains(WindowFlags::CLOSABLE) && !(self.is_empty() && new.contains(WindowFlags::CLOSABLE)) {
            let flags = MF_BYCOMMAND
                | if new.contains(WindowFlags::CLOSABLE) { MF_ENABLED } else { MF_DISABLED };

            unsafe {
                EnableMenuItem(GetSystemMenu(window, 0), SC_CLOSE, flags);
            }
        }

        if !new.contains(WindowFlags::VISIBLE) {
            unsafe {
                ShowWindow(window, SW_HIDE);
            }
        }

        // A show or a hide alone: ShowWindow set WS_VISIBLE itself. Writing the styles again and
        // refreshing the frame (SWP_FRAMECHANGED) cost a window's first show about 2,200 us.
        if diff == WindowFlags::VISIBLE {
            return;
        }

        if diff != WindowFlags::empty() {
            let (style, style_ex) = new.to_window_styles();

            unsafe {
                SendMessageW(window, event_loop::SET_RETAIN_STATE_ON_SIZE_MSG_ID.get(), 1, 0);

                // This condition is necessary to avoid having an unrestorable window
                if !new.contains(WindowFlags::MINIMIZED) {
                    SetWindowLongW(window, GWL_STYLE, style as i32);
                    SetWindowLongW(window, GWL_EXSTYLE, style_ex as i32);
                }

                let mut flags = SWP_NOZORDER | SWP_NOMOVE | SWP_NOSIZE | SWP_FRAMECHANGED;

                // We generally don't want style changes here to affect window
                // focus, but for fullscreen windows they must be activated
                // (i.e. focused) so that they appear on top of the taskbar
                if !new.contains(WindowFlags::MARKER_EXCLUSIVE_FULLSCREEN)
                    && !new.contains(WindowFlags::MARKER_BORDERLESS_FULLSCREEN)
                {
                    flags |= SWP_NOACTIVATE;
                }

                // Refresh the window frame
                SetWindowPos(window, 0, 0, 0, 0, 0, flags);
                SendMessageW(window, event_loop::SET_RETAIN_STATE_ON_SIZE_MSG_ID.get(), 0, 0);
            }
        }
    }

    /// Applies fullscreen style and bounds changes together so the frame is recalculated once at
    /// the target rectangle instead of once at the old rectangle before moving.
    fn apply_fullscreen_diff(
        mut self,
        window: HWND,
        mut new: WindowFlags,
        bounds: FullscreenBounds,
    ) {
        self = self.mask();
        new = new.mask();

        let z_order_changed =
            (self ^ new).intersects(WindowFlags::ALWAYS_ON_TOP | WindowFlags::ALWAYS_ON_BOTTOM);
        let (insert_after, z_flag) = if z_order_changed {
            let hwnd_after = match (
                new.contains(WindowFlags::ALWAYS_ON_TOP),
                new.contains(WindowFlags::ALWAYS_ON_BOTTOM),
            ) {
                (true, false) => HWND_TOPMOST,
                (false, false) => HWND_NOTOPMOST,
                (false, true) => HWND_BOTTOM,
                (true, true) => unreachable!(),
            };
            (hwnd_after, 0)
        } else {
            (0, SWP_NOZORDER)
        };

        let (style, style_ex) = new.to_window_styles();
        let entering = new.intersects(
            WindowFlags::MARKER_EXCLUSIVE_FULLSCREEN | WindowFlags::MARKER_BORDERLESS_FULLSCREEN,
        );
        let activate_flag = if entering { 0 } else { SWP_NOACTIVATE };

        unsafe {
            SendMessageW(window, event_loop::SET_RETAIN_STATE_ON_SIZE_MSG_ID.get(), 1, 0);

            if !new.contains(WindowFlags::MINIMIZED) {
                SetWindowLongW(window, GWL_STYLE, style as i32);
                SetWindowLongW(window, GWL_EXSTYLE, style_ex as i32);
            }

            match bounds {
                FullscreenBounds::Rect(r) => {
                    SetWindowPos(
                        window,
                        insert_after,
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        z_flag | activate_flag | SWP_FRAMECHANGED,
                    );
                },
                FullscreenBounds::Restore(saved) => {
                    let monitor_exists =
                        MonitorFromRect(&saved.rect, MONITOR_DEFAULTTONULL) != 0;
                    if can_restore_rect_directly(
                        saved.placement.showCmd,
                        saved.rect,
                        monitor_exists,
                    ) {
                        SetWindowPos(
                            window,
                            insert_after,
                            saved.rect.left,
                            saved.rect.top,
                            saved.rect.right - saved.rect.left,
                            saved.rect.bottom - saved.rect.top,
                            z_flag | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                        );
                    } else {
                        SetWindowPlacement(window, &saved.placement);
                        SetWindowPos(
                            window,
                            insert_after,
                            0,
                            0,
                            0,
                            0,
                            z_flag | SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                        );
                    }
                },
                FullscreenBounds::FrameOnly => {
                    SetWindowPos(
                        window,
                        insert_after,
                        0,
                        0,
                        0,
                        0,
                        z_flag | SWP_NOMOVE | SWP_NOSIZE | activate_flag | SWP_FRAMECHANGED,
                    );
                },
            }

            SendMessageW(window, event_loop::SET_RETAIN_STATE_ON_SIZE_MSG_ID.get(), 0, 0);
        }
    }

    pub fn adjust_rect(self, hwnd: HWND, mut rect: RECT) -> Result<RECT, io::Error> {
        unsafe {
            let mut style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
            let style_ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;

            // Frameless style implemented by manually overriding the non-client area in
            // `WM_NCCALCSIZE`.
            if !self.contains(WindowFlags::MARKER_DECORATIONS) {
                style &= !(WS_CAPTION | WS_SIZEBOX);
            }

            util::win_to_err({
                let b_menu = GetMenu(hwnd) != 0;
                if let (Some(get_dpi_for_window), Some(adjust_window_rect_ex_for_dpi)) =
                    (*util::GET_DPI_FOR_WINDOW, *util::ADJUST_WINDOW_RECT_EX_FOR_DPI)
                {
                    let dpi = get_dpi_for_window(hwnd);
                    adjust_window_rect_ex_for_dpi(&mut rect, style, b_menu.into(), style_ex, dpi)
                } else {
                    AdjustWindowRectEx(&mut rect, style, b_menu.into(), style_ex)
                }
            })?;
            Ok(rect)
        }
    }

    pub fn adjust_size(self, hwnd: HWND, size: PhysicalSize<u32>) -> PhysicalSize<u32> {
        let (width, height): (u32, u32) = size.into();
        let rect = RECT { left: 0, right: width as i32, top: 0, bottom: height as i32 };
        let rect = self.adjust_rect(hwnd, rect).unwrap_or(rect);

        let outer_x = (rect.right - rect.left).abs();
        let outer_y = (rect.top - rect.bottom).abs();

        PhysicalSize::new(outer_x as _, outer_y as _)
    }

    pub fn set_size(self, hwnd: HWND, size: PhysicalSize<u32>) {
        unsafe {
            let (width, height): (u32, u32) = self.adjust_size(hwnd, size).into();
            SetWindowPos(
                hwnd,
                0,
                0,
                0,
                width as _,
                height as _,
                SWP_ASYNCWINDOWPOS | SWP_NOZORDER | SWP_NOREPOSITION | SWP_NOMOVE | SWP_NOACTIVATE,
            );
            InvalidateRgn(hwnd, 0, false.into());
        }
    }
}

impl CursorFlags {
    fn refresh_os_cursor(self, window: HWND) -> Result<(), io::Error> {
        let client_rect = util::WindowArea::Inner.get_rect(window)?;

        if util::is_focused(window) {
            let cursor_clip = match self.contains(CursorFlags::GRABBED) {
                true => {
                    if self.contains(CursorFlags::LOCKED) {
                        if let Ok(pos) = util::get_cursor_position() {
                            Some(RECT {
                                left: pos.x,
                                right: pos.x + 1,
                                top: pos.y,
                                bottom: pos.y + 1,
                            })
                        } else {
                            // If lock is applied while the cursor is not available, lock it to the
                            // middle of the window.
                            let cx = (client_rect.left + client_rect.right) / 2;
                            let cy = (client_rect.top + client_rect.bottom) / 2;
                            Some(RECT { left: cx, right: cx + 1, top: cy, bottom: cy + 1 })
                        }
                    } else if self.contains(CursorFlags::HIDDEN) {
                        // Confine the cursor to the center of the window if the cursor is hidden.
                        // This avoids problems with the cursor activating
                        // the taskbar if the window borders or overlaps that.
                        let cx = (client_rect.left + client_rect.right) / 2;
                        let cy = (client_rect.top + client_rect.bottom) / 2;
                        Some(RECT { left: cx, right: cx + 1, top: cy, bottom: cy + 1 })
                    } else {
                        Some(client_rect)
                    }
                },
                false => None,
            };

            let rect_to_tuple = |rect: RECT| (rect.left, rect.top, rect.right, rect.bottom);
            let active_cursor_clip = rect_to_tuple(util::get_cursor_clip()?);
            let desktop_rect = rect_to_tuple(util::get_desktop_rect());

            let active_cursor_clip = match desktop_rect == active_cursor_clip {
                true => None,
                false => Some(active_cursor_clip),
            };

            // We do this check because calling `set_cursor_clip` incessantly will flood the event
            // loop with `WM_MOUSEMOVE` events, and `refresh_os_cursor` is called by
            // `set_cursor_flags` which at times gets called once every iteration of the
            // eventloop.
            if active_cursor_clip != cursor_clip.map(rect_to_tuple) {
                util::set_cursor_clip(cursor_clip)?;
            }
        }

        let cursor_in_client = self.contains(CursorFlags::IN_WINDOW);
        if cursor_in_client {
            util::set_cursor_hidden(self.contains(CursorFlags::HIDDEN));
        } else {
            util::set_cursor_hidden(false);
        }

        Ok(())
    }
}

/// Whether leaving fullscreen can restore the saved outer rect in one atomic `SetWindowPos`
/// (normal window on a still-attached monitor), or must delegate to `SetWindowPlacement`
/// (maximized/minimized state or a disconnected monitor).
fn can_restore_rect_directly(show_cmd: u32, rect: RECT, monitor_exists: bool) -> bool {
    show_cmd != SW_SHOWMAXIMIZED as u32
        && show_cmd != SW_SHOWMINIMIZED as u32
        && rect.right > rect.left
        && rect.bottom > rect.top
        && monitor_exists
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fullscreen_styles_strip_frame_and_edge_bits_and_restore_on_exit() {
        let base = WindowFlags::RESIZABLE
            | WindowFlags::MINIMIZABLE
            | WindowFlags::MAXIMIZABLE
            | WindowFlags::CLOSABLE
            | WindowFlags::VISIBLE
            | WindowFlags::MARKER_DECORATIONS;

        let (win_style, win_ex) = base.to_window_styles();
        assert_eq!(win_style & WS_CAPTION, WS_CAPTION);
        assert_eq!(win_style & WS_SIZEBOX, WS_SIZEBOX);
        assert_eq!(win_ex & WS_EX_WINDOWEDGE, WS_EX_WINDOWEDGE);

        for marker in [
            WindowFlags::MARKER_BORDERLESS_FULLSCREEN,
            WindowFlags::MARKER_EXCLUSIVE_FULLSCREEN,
        ] {
            let (fs_style, fs_ex) = (base | marker).to_window_styles();
            assert_eq!(fs_style & WS_OVERLAPPEDWINDOW, 0);
            assert_eq!(
                fs_ex & (WS_EX_DLGMODALFRAME | WS_EX_WINDOWEDGE | WS_EX_CLIENTEDGE | WS_EX_STATICEDGE),
                0
            );
        }

        // Clearing the fullscreen marker restores every windowed style and extended-style edge bit.
        let restored = (base | WindowFlags::MARKER_BORDERLESS_FULLSCREEN)
            - WindowFlags::MARKER_BORDERLESS_FULLSCREEN;
        assert_eq!(restored.to_window_styles(), (win_style, win_ex));
    }

    #[test]
    fn direct_restore_requires_normal_valid_rect_on_live_monitor() {
        let valid = RECT { left: 100, top: 80, right: 720, bottom: 720 };
        assert!(can_restore_rect_directly(SW_RESTORE as u32, valid, true));
        assert!(!can_restore_rect_directly(SW_SHOWMAXIMIZED as u32, valid, true));
        assert!(!can_restore_rect_directly(SW_SHOWMINIMIZED as u32, valid, true));
        assert!(!can_restore_rect_directly(SW_RESTORE as u32, valid, false));
        let empty = RECT { left: 100, top: 80, right: 100, bottom: 720 };
        assert!(!can_restore_rect_directly(SW_RESTORE as u32, empty, true));
    }
}
