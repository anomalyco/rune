//! X11 input SHAPE regions. The visual surface stays intact; only pointer ownership changes.

use x11rb::{connection::Connection, protocol::{shape::{ConnectionExt as _, SK, SO}, xproto::{ConnectionExt as _, ClipOrdering, ConfigureWindowAux, Rectangle, StackMode}}, rust_connection::RustConnection};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

thread_local! {
    static SHOWN: std::cell::RefCell<Vec<(u32, crate::OverlayLevel)>> = const { std::cell::RefCell::new(Vec::new()) };
}

pub(crate) struct InputWindow {
    connection: RustConnection,
    root: u32,
    window: u32,
    last: Vec<Rectangle>,
}

impl InputWindow {
    pub fn show(&self, level: crate::OverlayLevel) {
        let windows = SHOWN.with(|shown| {
            let mut shown = shown.borrow_mut();
            shown.retain(|(id, _)| *id != self.window);
            shown.push((self.window, level));
            shown.clone()
        });
        for level in [crate::OverlayLevel::Status, crate::OverlayLevel::AboveStatus] {
            for (id, at) in &windows {
                if *at == level { let _ = self.connection.configure_window(*id, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE)); }
            }
        }
        let _ = self.connection.flush();
    }
    pub fn hide(&self) { SHOWN.with(|shown| shown.borrow_mut().retain(|(id, _)| *id != self.window)); }
    pub fn new(window: &winit::window::Window) -> Option<Self> {
        let id = match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Xlib(w) => u32::try_from(w.window).ok()?,
            RawWindowHandle::Xcb(w) => w.window.get(),
            _ => return None,
        };
        let (connection, screen) = x11rb::connect(None).ok()?;
        let version = connection.shape_query_version().ok()?.reply().ok()?;
        if (version.major_version, version.minor_version) < (1, 1) { return None; }
        let root = connection.setup().roots.get(screen)?.root;
        Some(Self { connection, root, window: id, last: Vec::new() })
    }
    pub fn pointer(&self, scale: f32) -> Option<(f32, f32)> {
        let p = self.connection.query_pointer(self.window).ok()?.reply().ok()?;
        p.same_screen.then_some((p.win_x as f32 / scale, p.win_y as f32 / scale))
    }
    pub fn desktop_pointer(&self) -> Option<(i32, i32)> {
        let p = self.connection.query_pointer(self.root).ok()?.reply().ok()?;
        p.same_screen.then_some((i32::from(p.root_x), i32::from(p.root_y)))
    }
    pub fn regions(&mut self, rects: &[[f32; 4]], scale: f32) {
        let rects: Vec<_> = rects.iter().filter_map(|r| {
            let x = (r[0] * scale).floor().max(0.0).min(i16::MAX as f32) as i16;
            let y = (r[1] * scale).floor().max(0.0).min(i16::MAX as f32) as i16;
            let width = ((r[2] * scale).ceil() - f32::from(x)).clamp(0.0, u16::MAX as f32) as u16;
            let height = ((r[3] * scale).ceil() - f32::from(y)).clamp(0.0, u16::MAX as f32) as u16;
            (width > 0 && height > 0).then_some(Rectangle { x, y, width, height })
        }).collect();
        if self.last.len() == rects.len() && self.last.iter().zip(&rects).all(|(a, b)|
            (a.x, a.y, a.width, a.height) == (b.x, b.y, b.width, b.height)) { return; }
        // X11 retains its implicit button grab through release even if input shape changes.
        if self.connection.shape_rectangles(SO::SET, SK::INPUT, ClipOrdering::UNSORTED, self.window, 0, 0, &rects)
            .ok().and_then(|request| request.check().ok()).is_some() {
            // This connection is separate from wgpu's. Wait for the shape to be applied
            // before presenting pixels, rather than racing two independent X11 streams.
            self.last = rects;
        }
    }
}

impl Drop for InputWindow { fn drop(&mut self) { self.hide(); } }

pub(crate) fn pointer_position() -> Option<(i32, i32)> {
    let (connection, screen) = x11rb::connect(None).ok()?;
    let root = connection.setup().roots.get(screen)?.root;
    let p = connection.query_pointer(root).ok()?.reply().ok()?;
    p.same_screen.then_some((i32::from(p.root_x), i32::from(p.root_y)))
}
