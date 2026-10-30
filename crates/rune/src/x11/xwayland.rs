//! An overlay on XWayland, for a Wayland compositor without layer-shell (GNOME's Mutter): an
//! override-redirect ARGB window of the notification type on its own libxcb connection, with
//! an empty input shape, so it never takes focus or a click. Like a layer surface it is mapped
//! while it lives and destroyed when hidden. Only overlays that take no input use it; the rest
//! of the app stays on Wayland.

use std::num::NonZeroU32;
use std::ptr::NonNull;

use winit::raw_window_handle::{RawDisplayHandle, RawWindowHandle, XcbDisplayHandle, XcbWindowHandle};
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
use x11rb::protocol::xproto::{
    AtomEnum, ClipOrdering, ColormapAlloc, ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, PropMode, StackMode,
    VisualClass, WindowClass,
};
use x11rb::wrapper::ConnectionExt as _;
use x11rb::xcb_ffi::XCBConnection;

use crate::overlay::Placement;
use crate::{DisplayTarget, OverlayOptions};

pub(crate) struct XwaylandSurface {
    connection: XCBConnection,
    screen: usize,
    root: u32,
    window: u32,
    colormap: u32,
    scale: f32,
}

impl XwaylandSurface {
    /// `None` without an X server (`DISPLAY`), a 32-bit visual, or the SHAPE extension the
    /// empty input region needs.
    pub fn new(name: &str, at: Placement, options: &OverlayOptions) -> Option<Self> {
        std::env::var_os("DISPLAY").filter(|display| !display.is_empty())?;
        let (connection, screen) = XCBConnection::connect(None).ok()?;
        let version = connection.shape_query_version().ok()?.reply().ok()?;
        if (version.major_version, version.minor_version) < (1, 1) { return None; }
        let root_screen = connection.setup().roots.get(screen)?;
        let root = root_screen.root;
        let visual = root_screen.allowed_depths.iter().filter(|d| d.depth == 32)
            .flat_map(|d| d.visuals.iter()).find(|v| v.class == VisualClass::TRUE_COLOR)?.visual_id;
        let scale = xft_scale(&connection, root);
        let colormap = connection.generate_id().ok()?;
        connection.create_colormap(ColormapAlloc::NONE, colormap, root, visual).ok()?.check().ok()?;
        let window = connection.generate_id().ok()?;
        let mut this = Self { connection, screen, root, window: 0, colormap, scale };
        let (x, y, w, h) = this.rect(at, options)?;
        let aux = CreateWindowAux::new().override_redirect(1).background_pixel(0).border_pixel(0).colormap(colormap);
        this.connection.create_window(32, window, root, x, y, w, h, 0, WindowClass::INPUT_OUTPUT, visual, &aux).ok()?.check().ok()?;
        this.window = window;
        let atom = |name: &[u8]| Some(this.connection.intern_atom(false, name).ok()?.reply().ok()?.atom);
        let (kind, notification) = (atom(b"_NET_WM_WINDOW_TYPE")?, atom(b"_NET_WM_WINDOW_TYPE_NOTIFICATION")?);
        let c = &this.connection;
        c.change_property32(PropMode::REPLACE, window, kind, AtomEnum::ATOM, &[notification]).ok()?;
        c.change_property8(PropMode::REPLACE, window, AtomEnum::WM_NAME, AtomEnum::STRING, name.as_bytes()).ok()?;
        let id = crate::tooling::app_id().unwrap_or(name);
        let class = [id.as_bytes(), b"\0", id.as_bytes(), b"\0"].concat();
        c.change_property8(PropMode::REPLACE, window, AtomEnum::WM_CLASS, AtomEnum::STRING, &class).ok()?;
        c.shape_rectangles(SO::SET, SK::INPUT, ClipOrdering::UNSORTED, window, 0, 0, &[]).ok()?.check().ok()?;
        c.map_window(window).ok()?;
        c.configure_window(window, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE)).ok()?;
        c.flush().ok()?;
        Some(this)
    }

    /// The desktop's scale (`Xft.dpi` / 96), which XWayland's native scaling sets.
    pub fn scale(&self) -> f32 { self.scale }

    pub fn place(&self, at: Placement, options: &OverlayOptions) {
        let Some((x, y, w, h)) = self.rect(at, options) else { return };
        let aux = ConfigureWindowAux::new().x(i32::from(x)).y(i32::from(y)).width(u32::from(w)).height(u32::from(h)).stack_mode(StackMode::ABOVE);
        let _ = self.connection.configure_window(self.window, &aux);
        let _ = self.connection.flush();
    }

    /// Drops what the server sent: the window asks for no events, so only errors arrive.
    pub fn drain(&self) {
        while let Ok(Some(_)) = self.connection.poll_for_event() {}
    }

    pub fn raw_handles(&self) -> Option<(RawDisplayHandle, RawWindowHandle)> {
        let display = XcbDisplayHandle::new(NonNull::new(self.connection.get_raw_xcb_connection()), i32::try_from(self.screen).ok()?);
        let window = XcbWindowHandle::new(NonZeroU32::new(self.window)?);
        Some((RawDisplayHandle::Xcb(display), RawWindowHandle::Xcb(window)))
    }

    /// The window's pixels on the target monitor: the primary unless the pointer's is asked for.
    fn rect(&self, at: Placement, options: &OverlayOptions) -> Option<(i16, i16, u16, u16)> {
        let monitors = self.connection.randr_get_monitors(self.root, true).ok()?.reply().ok()?.monitors;
        let bounds: Vec<_> = monitors.iter().map(|m| {
            let (x, y) = (f64::from(m.x), f64::from(m.y));
            [x, y, x + f64::from(m.width), y + f64::from(m.height)]
        }).collect();
        let primary = monitors.iter().position(|m| m.primary);
        let pointer = (options.display == DisplayTarget::UnderPointer).then(|| {
            let p = self.connection.query_pointer(self.root).ok()?.reply().ok()?;
            p.same_screen.then_some((f64::from(p.root_x), f64::from(p.root_y)))
        }).flatten();
        let monitor = &monitors[crate::overlay_options::choose_display(&bounds, primary, None, pointer)?];
        let scale = self.scale;
        let area = (f32::from(monitor.width) / scale, f32::from(monitor.height) / scale);
        let r = crate::overlay_options::local_rect(at, options.placement, area);
        let px = |v: f32| (v * scale).round();
        let x = (f32::from(monitor.x) + px(r[0])).clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
        let y = (f32::from(monitor.y) + px(r[1])).clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
        let size = |v: f32| px(v).clamp(1.0, f32::from(u16::MAX)) as u16;
        Some((x, y, size(r[2]), size(r[3])))
    }
}

impl Drop for XwaylandSurface {
    fn drop(&mut self) {
        if self.window != 0 { let _ = self.connection.destroy_window(self.window); }
        let _ = self.connection.free_colormap(self.colormap);
        let _ = self.connection.flush();
    }
}

/// `Xft.dpi` from the root window's resource database, as a scale; 1 when unset.
fn xft_scale(connection: &XCBConnection, root: u32) -> f32 {
    let Some(reply) = connection.get_property(false, root, AtomEnum::RESOURCE_MANAGER, AtomEnum::STRING, 0, 1 << 16)
        .ok().and_then(|cookie| cookie.reply().ok()) else { return 1.0 };
    String::from_utf8_lossy(&reply.value).lines()
        .find_map(|line| line.strip_prefix("Xft.dpi:")?.trim().parse::<f32>().ok())
        .map_or(1.0, |dpi| (dpi / 96.0).clamp(1.0, 4.0))
}
