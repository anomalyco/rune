//! X11: CPU frames kept in one buffer and put to the window as an image (`PutImage`), on a
//! connection of this window's own: the server takes the bytes as the frame's, with nothing to
//! convert, for a 24-bit true-colour visual (B, G, R, X in memory) or, when alpha is requested
//! for a transparent window, a 32-bit ARGB true-colour visual (premultiplied B, G, R, A, falling
//! back to 24-bit opaque when the server gave the window no 32-bit visual).
//!
//! The server keeps nothing of a window that isn't on screen yet or was covered (no backing
//! store), and says so with an Expose: this connection asks for the window's, and the frame that
//! finds one sends everything again.

use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt, CreateGCAux, EventMask, ImageFormat, ImageOrder, VisualClass};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use super::{Canvas, Put, bounds};

/// A `PutImage` request's bytes before its image.
const HEADER: usize = 24;

/// Pixel depth and alpha support matched for an X11 window's visual.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct VisualFormat {
    /// X11 depth passed to `PutImage` (`32` for ARGB TrueColor, `24` for opaque BGRX TrueColor).
    pub(super) depth: u8,
    /// Whether the visual composites per-pixel alpha (`depth == 32`).
    pub(super) has_alpha: bool,
}

/// Pure visual entry for matching without a live X server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct VisualInfo {
    pub(super) id: u32,
    pub(super) depth: u8,
    pub(super) true_color: bool,
    pub(super) rgb_masks: (u32, u32, u32),
}

/// Pure X11 pixmap format entry (`depth`, `bits_per_pixel`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PixmapFormat {
    pub(super) depth: u8,
    pub(super) bits_per_pixel: u8,
}

/// Matches `visual_id` against the X11 server's byte order, pixmap formats, and visual table.
///
/// - When `want_alpha` is `true`: accepts a 32-bit ARGB TrueColor visual (`has_alpha = true`) or
///   falls back to a 24-bit BGRX TrueColor visual (`has_alpha = false`) if the server gave the
///   window no 32-bit visual.
/// - When `want_alpha` is `false`: accepts only a 24-bit BGRX TrueColor visual (unchanged).
pub(super) fn match_visual(
    lsb_first: bool,
    pixmap_formats: &[PixmapFormat],
    visuals: &[VisualInfo],
    visual_id: u32,
    want_alpha: bool,
) -> Option<VisualFormat> {
    if !lsb_first { return None; }
    let visual = visuals.iter().find(|v| {
        v.id == visual_id && v.true_color && v.rgb_masks == (0xff0000, 0xff00, 0xff)
    })?;
    let accepted_depth = match (want_alpha, visual.depth) {
        (true, 32) => 32,
        (_, 24) => 24,
        _ => return None,
    };
    let has_pixmap = pixmap_formats
        .iter()
        .any(|f| f.depth == accepted_depth && f.bits_per_pixel == 32);
    if !has_pixmap { return None; }
    Some(VisualFormat { depth: accepted_depth, has_alpha: accepted_depth == 32 })
}

pub(crate) struct Frames {
    conn: RustConnection,
    window: u32,
    gc: u32,
    depth: u8,
    buffer: Vec<u8>,
    size: (u32, u32),
    /// The buffer holds the frame painted last.
    painted: bool,
    /// The server asked for the window's pixels and hasn't had them since.
    exposed: bool,
    /// The most image bytes one request carries.
    most: usize,
}

impl Frames {
    pub(super) fn new(window: u32, visual: u32, alpha: bool) -> Option<Frames> {
        let (conn, _) = x11rb::connect(None).ok()?;
        let setup = conn.setup();
        let formats: Vec<PixmapFormat> = setup
            .pixmap_formats
            .iter()
            .map(|f| PixmapFormat { depth: f.depth, bits_per_pixel: f.bits_per_pixel })
            .collect();
        let visuals: Vec<VisualInfo> = setup
            .roots
            .iter()
            .flat_map(|root| &root.allowed_depths)
            .flat_map(|d| {
                d.visuals.iter().map(move |v| VisualInfo {
                    id: v.visual_id,
                    depth: d.depth,
                    true_color: v.class == VisualClass::TRUE_COLOR,
                    rgb_masks: (v.red_mask, v.green_mask, v.blue_mask),
                })
            })
            .collect();
        let matched = match_visual(
            setup.image_byte_order == ImageOrder::LSB_FIRST,
            &formats,
            &visuals,
            visual,
            alpha,
        )?;
        let gc = conn.generate_id().ok()?;
        conn.create_gc(gc, window, &CreateGCAux::new().graphics_exposures(0)).ok()?;
        conn.change_window_attributes(window, &ChangeWindowAttributesAux::new().event_mask(EventMask::EXPOSURE)).ok()?;
        // Asks the server once how long a request may be.
        let most = conn.maximum_request_bytes().saturating_sub(HEADER);
        Some(Frames { conn, window, gc, depth: matched.depth, buffer: Vec::new(), size: (0, 0), painted: false, exposed: false, most })
    }

    pub(super) fn has_alpha(&self) -> bool { self.depth == 32 }

    pub(super) fn begin(&mut self, size: (u32, u32)) -> Option<Canvas<'_>> {
        let size = (size.0.clamp(1, u32::from(u16::MAX)), size.1.clamp(1, u32::from(u16::MAX)));
        if self.size != size {
            self.buffer = vec![0; size.0 as usize * size.1 as usize * 4];
            self.size = size;
            self.painted = false;
        }
        Some(Canvas { bytes: &mut self.buffer, stride: size.0 as usize * 4, kept: self.painted })
    }

    pub(super) fn present(&mut self, put: Put<'_>) -> bool {
        self.painted = true;
        let all = [0, 0, self.size.0, self.size.1];
        // One image for all that changed: the server may show the window between two.
        let Some([x0, y0, x1, y1]) = (match put { Put::Rects(rects) => bounds(rects, self.size), Put::All => Some(all) }) else { return false };
        let (stride, row) = (self.size.0 as usize * 4, (x1 - x0) as usize * 4);
        if row > self.most { return false; }
        let whole_rows = x0 == 0 && x1 == self.size.0;
        let mut strip = Vec::new();
        let mut y = y0;
        while y < y1 {
            let rows = ((self.most / row) as u32).min(y1 - y);
            let image = if whole_rows {
                &self.buffer[y as usize * stride..(y + rows) as usize * stride]
            } else {
                strip.clear();
                for line in y..y + rows {
                    let at = line as usize * stride + x0 as usize * 4;
                    strip.extend_from_slice(&self.buffer[at..at + row]);
                }
                &strip[..]
            };
            if self.conn.put_image(ImageFormat::Z_PIXMAP, self.window, self.gc, (x1 - x0) as u16, rows as u16, x0 as i16, y as i16, 0, self.depth, image).is_err() { return false; }
            y += rows;
        }
        if matches!(put, Put::All) { self.exposed = false; }
        self.conn.flush().is_ok()
    }

    pub(super) fn exposed(&mut self, wait: bool) -> bool {
        if wait { self.conn.sync().ok(); }
        while let Ok(Some(event)) = self.conn.poll_for_event() {
            if let Event::Expose(_) = event { self.exposed = true; }
        }
        self.exposed
    }

    pub(super) fn forget(&mut self) { self.painted = false; }

    pub(super) fn last(&self) -> Option<(Vec<u8>, (u32, u32))> {
        self.painted.then(|| (self.buffer.clone(), self.size))
    }
}

impl Drop for Frames {
    fn drop(&mut self) {
        self.conn.free_gc(self.gc).ok();
        self.conn.flush().ok();
    }
}
