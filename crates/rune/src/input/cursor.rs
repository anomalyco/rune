//! The pointer's shape over an element.

pub(crate) mod image;
pub(crate) mod os;

/// The pointer's shape over an element (CSS `cursor`). Elements without one inherit their
/// nearest ancestor's; text fields default to [`Cursor::Text`], everything else to the arrow.
///
/// Platform mapping for the one-way resize, alias, and copy variants:
///
/// | Variant | Web (CSS) | macOS (AppKit) | Windows | Linux X11 (`cursor-icon`) | Linux Wayland (`wp-cursor-shape-v1`) |
/// |---|---|---|---|---|---|
/// | [`Cursor::ResizeN`] | `n-resize` | `+[NSCursor resizeUpCursor]` | `IDC_SIZENS` (two-way; no one-way system cursor, matching Chrome) | `n-resize` (alt `top_side`) | `Shape::NResize` (`n-resize` / `top_side` theme fallback) |
/// | [`Cursor::ResizeE`] | `e-resize` | `+[NSCursor resizeRightCursor]` | `IDC_SIZEWE` (two-way; no one-way system cursor, matching Chrome) | `e-resize` (alt `right_side`) | `Shape::EResize` (`e-resize` / `right_side` theme fallback) |
/// | [`Cursor::ResizeS`] | `s-resize` | `+[NSCursor resizeDownCursor]` | `IDC_SIZENS` (two-way; no one-way system cursor, matching Chrome) | `s-resize` (alt `bottom_side`) | `Shape::SResize` (`s-resize` / `bottom_side` theme fallback) |
/// | [`Cursor::ResizeW`] | `w-resize` | `+[NSCursor resizeLeftCursor]` | `IDC_SIZEWE` (two-way; no one-way system cursor, matching Chrome) | `w-resize` (alt `left_side`) | `Shape::WResize` (`w-resize` / `left_side` theme fallback) |
/// | [`Cursor::Alias`] | `alias` | `+[NSCursor dragLinkCursor]` | Chromium's `aliasb.cur` bitmap (no system cursor; winit falls back to `IDC_ARROW`) | `alias` (alt `link`) | `Shape::Alias` (`alias` / `link` theme fallback) |
/// | [`Cursor::Copy`] | `copy` | `+[NSCursor dragCopyCursor]` | Chromium's `copy.cur` bitmap (no system cursor; winit falls back to `IDC_ARROW`) | `copy` | `Shape::Copy` (`copy` theme fallback) |
///
/// A macOS overlay is rarely the key window, so it sets its cursor directly while the pointer
/// is over its hit area. macOS honours that only while the overlay's app is frontmost, unless
/// `rune` is built with the `macos-background-cursor` feature. That feature uses a private
/// WindowServer property (`SetsCursorInBackground`), so App Store review may reject the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Cursor {
    #[default]
    Default,
    Pointer,
    Text,
    /// An open hand (CSS `grab`), over something a drag moves. Windows has no such system
    /// cursor, so there it draws Chromium's.
    Grab,
    /// A closed hand (CSS `grabbing`), while the drag holds it. Chromium's on Windows, as for
    /// [`Cursor::Grab`].
    Grabbing,
    NotAllowed,
    Move,
    ResizeColumn,
    ResizeRow,
    /// Horizontal bidirectional resize (CSS `ew-resize`), for left and right frame edges.
    ///
    /// ```
    /// # use rune::{Cursor, el};
    /// # let _ =
    /// el().size(8.0, 120.0).hit().cursor(Cursor::ResizeEw)
    /// # ;
    /// ```
    ResizeEw,
    /// Vertical bidirectional resize (CSS `ns-resize`), for top and bottom frame edges.
    ///
    /// ```
    /// # use rune::{Cursor, el};
    /// # let _ =
    /// el().size(120.0, 8.0).hit().cursor(Cursor::ResizeNs)
    /// # ;
    /// ```
    ResizeNs,
    /// Diagonal resize from north-west to south-east (CSS `nwse-resize`), for top-left and
    /// bottom-right corners.
    ///
    /// ```
    /// # use rune::{Cursor, el};
    /// # let _ =
    /// el().size(12.0, 12.0).hit().cursor(Cursor::ResizeNwse)
    /// # ;
    /// ```
    ResizeNwse,
    /// Diagonal resize from north-east to south-west (CSS `nesw-resize`), for top-right and
    /// bottom-left corners.
    ///
    /// ```
    /// # use rune::{Cursor, el};
    /// # let _ =
    /// el().size(12.0, 12.0).hit().cursor(Cursor::ResizeNesw)
    /// # ;
    /// ```
    ResizeNesw,
    /// One-way upward resize (CSS `n-resize`), for a horizontal edge at its lower limit that can
    /// only move north. Shows AppKit's `resizeUpCursor` on macOS, `n-resize`/`top_side` on Linux
    /// and the web, and Windows's two-way vertical `IDC_SIZENS` (Windows has no one-way resize
    /// cursor, matching Chrome).
    ///
    /// ```
    /// # use rune::{Cursor, el};
    /// # let _ =
    /// el().size(120.0, 8.0).hit().cursor(Cursor::ResizeN)
    /// # ;
    /// ```
    ResizeN,
    /// One-way rightward resize (CSS `e-resize`), for a vertical edge at its left limit that can
    /// only move east. Shows AppKit's `resizeRightCursor` on macOS, `e-resize`/`right_side` on
    /// Linux and the web, and Windows's two-way horizontal `IDC_SIZEWE` (matching Chrome).
    ///
    /// ```
    /// # use rune::{Cursor, el};
    /// # let _ =
    /// el().size(8.0, 120.0).hit().cursor(Cursor::ResizeE)
    /// # ;
    /// ```
    ResizeE,
    /// One-way downward resize (CSS `s-resize`), for a horizontal edge at its upper limit that can
    /// only move south. Shows AppKit's `resizeDownCursor` on macOS, `s-resize`/`bottom_side` on
    /// Linux and the web, and Windows's two-way vertical `IDC_SIZENS` (matching Chrome).
    ///
    /// ```
    /// # use rune::{Cursor, el};
    /// # let _ =
    /// el().size(120.0, 8.0).hit().cursor(Cursor::ResizeS)
    /// # ;
    /// ```
    ResizeS,
    /// One-way leftward resize (CSS `w-resize`), for a vertical edge at its right limit that can
    /// only move west. Shows AppKit's `resizeLeftCursor` on macOS, `w-resize`/`left_side` on
    /// Linux and the web, and Windows's two-way horizontal `IDC_SIZEWE` (matching Chrome).
    ///
    /// ```
    /// # use rune::{Cursor, el};
    /// # let _ =
    /// el().size(8.0, 120.0).hit().cursor(Cursor::ResizeW)
    /// # ;
    /// ```
    ResizeW,
    /// An alias or shortcut will be created (CSS `alias`). Shows AppKit's `dragLinkCursor` on
    /// macOS, `alias`/`link` on Linux and the web, and Chromium's Windows `alias` bitmap on
    /// Windows (Windows has no system alias cursor).
    ///
    /// ```
    /// # use rune::{Cursor, el};
    /// # let _ =
    /// el().size(80.0, 28.0).hit().cursor(Cursor::Alias)
    /// # ;
    /// ```
    Alias,
    /// Something is to be copied (CSS `copy`). Shows AppKit's `dragCopyCursor` on macOS, `copy`
    /// on Linux and the web, and Chromium's Windows `copy` bitmap on Windows (Windows has no
    /// system copy cursor).
    ///
    /// ```
    /// # use rune::{Cursor, el};
    /// # let _ =
    /// el().size(80.0, 28.0).hit().cursor(Cursor::Copy)
    /// # ;
    /// ```
    Copy,
    /// A precise-selection cross (CSS `crosshair`), for picking a point or dragging out a rect.
    Crosshair,
    /// A thick white plus (CSS `cell`), over a spreadsheet's cells. Windows has no such system
    /// cursor, so there it draws Chromium's.
    Cell,
    /// The system's magnifier with a plus (CSS `zoom-in`), at the size of every other system
    /// cursor: over a picture that opens larger. Windows has no such system cursor, so there it
    /// draws Chromium's.
    ZoomIn,
    /// The magnifier with a minus (CSS `zoom-out`). Chromium's on Windows, as for
    /// [`Cursor::ZoomIn`].
    ZoomOut,
    /// The arrow with a question mark (CSS `help`): over something that explains itself when
    /// the pointer rests on it.
    Help,
    /// The arrow with a busy mark (CSS `progress`): work is under way and the app still takes
    /// input.
    Progress,
    /// No OS cursor (CSS `none`), for views that draw their own. It shows again as soon as the
    /// pointer leaves the element or the window, the element goes away, or the host loses the
    /// pointer: a window losing focus, an overlay hiding.
    Hidden,
    /// An app's own image (CSS `url()`), such as a pencil over a canvas. The OS draws it, so
    /// it keeps up with the pointer and shows right to the window's edge. It comes and goes
    /// like any other cursor. If its PNG doesn't decode, the arrow shows instead, and that's
    /// logged once.
    Image(CursorImage),
}

impl Cursor {
    /// The CSS name of this shape (`default`, `pointer`, `col-resize`, ...), which the tree dump
    /// (`--tree`) writes: `none` for [`Cursor::Hidden`] and `image` for an app's own picture.
    pub(crate) fn css_name(self) -> &'static str {
        match self {
            Cursor::Default => "default",
            Cursor::Pointer => "pointer",
            Cursor::Text => "text",
            Cursor::Grab => "grab",
            Cursor::Grabbing => "grabbing",
            Cursor::NotAllowed => "not-allowed",
            Cursor::Move => "move",
            Cursor::ResizeColumn => "col-resize",
            Cursor::ResizeRow => "row-resize",
            Cursor::ResizeEw => "ew-resize",
            Cursor::ResizeNs => "ns-resize",
            Cursor::ResizeNwse => "nwse-resize",
            Cursor::ResizeNesw => "nesw-resize",
            Cursor::ResizeN => "n-resize",
            Cursor::ResizeE => "e-resize",
            Cursor::ResizeS => "s-resize",
            Cursor::ResizeW => "w-resize",
            Cursor::Alias => "alias",
            Cursor::Copy => "copy",
            Cursor::Crosshair => "crosshair",
            Cursor::Cell => "cell",
            Cursor::ZoomIn => "zoom-in",
            Cursor::ZoomOut => "zoom-out",
            Cursor::Help => "help",
            Cursor::Progress => "progress",
            Cursor::Hidden => "none",
            Cursor::Image(_) => "image",
        }
    }
}

/// The picture for [`Cursor::Image`]: a PNG drawn at 2x, so a 64×64 px PNG makes a
/// 32×32 pt cursor, and its hot spot (the point that clicks) in points from the top-left,
/// clamped into the image.
///
/// ```
/// # use rune::{Cursor, CursorImage, el};
/// # const PENCIL_PNG: &[u8] = &[];
/// # let canvas = el();
/// const PENCIL: CursorImage = CursorImage::png(PENCIL_PNG, [2.0, 30.0]); // e.g. include_bytes!("pencil.png")
/// # let _ =
/// canvas.cursor(Cursor::Image(PENCIL))
/// # ;
/// ```
///
/// Two images are the same cursor when they have the same bytes and hot spot. Comparing one
/// image with itself checks only the address, so it costs nothing per frame; a `const` may be
/// copied to another address at each use, which then compares the bytes. Each image is
/// decoded once and its OS cursor kept, up to 32 of them; a side may be up to 256 px.
#[derive(Clone, Copy)]
pub struct CursorImage {
    bytes: &'static [u8],
    hot: [f32; 2],
    /// Resampled to the OS cursor's scale pixel for pixel, for a 1-bit cursor drawn on a grid.
    crisp: bool,
}

/// [`Cursor::Cell`] where the OS has none: Chromium's Windows `cell` cursor, each pixel of its
/// 32 px bitmap doubled.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const CELL: CursorImage = CursorImage { bytes: include_bytes!("cursor/cell.png"), hot: [7.0, 7.0], crisp: true };

/// The picture Windows shows for `c` where it has no system cursor of its own: Chromium's Windows
/// cursors (`ui/resources/cursors/*.cur`), each pixel of their 32 px bitmaps doubled, with their
/// hot spots. winit's stand-ins there are the four-way move arrows for both hands and the plain
/// arrow for `Cell`, `Alias`, `Copy`, and both magnifiers. (Chromium's `col-resize` and
/// `row-resize` invert the screen under them, which a colour cursor can't, and one-way resizes
/// stay Windows's two-way arrows as Chrome shows them.)
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn windows_image(c: Cursor) -> Option<CursorImage> {
    let picture = |bytes: &'static [u8], hot: [f32; 2]| CursorImage { bytes, hot, crisp: true };
    match c {
        Cursor::Alias => Some(picture(include_bytes!("cursor/alias.png"), [7.0, 4.0])),
        Cursor::Copy => Some(picture(include_bytes!("cursor/copy.png"), [7.0, 4.0])),
        Cursor::Cell => Some(CELL),
        Cursor::Grab => Some(picture(include_bytes!("cursor/grab.png"), [13.0, 13.0])),
        Cursor::Grabbing => Some(picture(include_bytes!("cursor/grabbing.png"), [13.0, 13.0])),
        Cursor::ZoomIn => Some(picture(include_bytes!("cursor/zoom_in.png"), [6.0, 6.0])),
        Cursor::ZoomOut => Some(picture(include_bytes!("cursor/zoom_out.png"), [6.0, 6.0])),
        _ => None,
    }
}

/// The pan cursor a middle-click autoscroll shows (`runtime/input/autoscroll.rs`): Chromium's
/// Windows `pan_*` cursors, each pixel of their 32 px bitmaps doubled, which Blink picks the way
/// this does: while it scrolls, an arrow the way it goes; at rest, a mark of the axes
/// (horizontal, vertical) it can scroll on.
pub(crate) fn pan(axes: (bool, bool), velocity: (f32, f32)) -> CursorImage {
    let pan = |bytes: &'static [u8]| CursorImage { bytes, hot: [16.0, 16.0], crisp: true };
    let (east, west, south, north) = (velocity.0 > 0.0, velocity.0 < 0.0, velocity.1 > 0.0, velocity.1 < 0.0);
    match (north, south, east, west) {
        (true, _, true, _) => pan(include_bytes!("cursor/pan_north_east.png")),
        (true, _, _, true) => pan(include_bytes!("cursor/pan_north_west.png")),
        (true, ..) => pan(include_bytes!("cursor/pan_north.png")),
        (_, true, true, _) => pan(include_bytes!("cursor/pan_south_east.png")),
        (_, true, _, true) => pan(include_bytes!("cursor/pan_south_west.png")),
        (_, true, ..) => pan(include_bytes!("cursor/pan_south.png")),
        (_, _, true, _) => pan(include_bytes!("cursor/pan_east.png")),
        (_, _, _, true) => pan(include_bytes!("cursor/pan_west.png")),
        _ => match axes {
            (true, false) => pan(include_bytes!("cursor/pan_middle_horizontal.png")),
            (false, true) => pan(include_bytes!("cursor/pan_middle_vertical.png")),
            _ => pan(include_bytes!("cursor/pan_middle.png")),
        },
    }
}

impl CursorImage {
    /// A PNG drawn at 2x, with its hot spot in points from the top-left.
    pub const fn png(bytes: &'static [u8], hot: [f32; 2]) -> Self {
        CursorImage { bytes, hot, crisp: false }
    }

    pub(crate) fn bytes(&self) -> &'static [u8] {
        self.bytes
    }

    pub(crate) fn hot(&self) -> [f32; 2] {
        self.hot
    }

    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn crisp(&self) -> bool {
        self.crisp
    }

}

impl PartialEq for CursorImage {
    fn eq(&self, other: &Self) -> bool {
        self.hot.map(f32::to_bits) == other.hot.map(f32::to_bits)
            && self.crisp == other.crisp
            && (std::ptr::eq(self.bytes, other.bytes) || self.bytes == other.bytes)
    }
}

impl Eq for CursorImage {}

impl std::hash::Hash for CursorImage {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (self.bytes.len(), self.hot.map(f32::to_bits)).hash(state);
    }
}

impl std::fmt::Debug for CursorImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CursorImage").field("png_bytes", &self.bytes.len()).field("hot", &self.hot).finish()
    }
}

/// Which AppKit cursor backs each [`Cursor`] variant, kept cross-platform so adding a new
/// [`Cursor`] variant fails to compile until its macOS mapping is explicit.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MacCursor {
    Arrow,
    PointingHand,
    IBeam,
    OpenHand,
    ClosedHand,
    NotAllowed,
    ResizeLeftRight,
    ResizeUpDown,
    ResizeEw,
    ResizeNs,
    ResizeNwse,
    ResizeNesw,
    ResizeN,
    ResizeE,
    ResizeS,
    ResizeW,
    Alias,
    Copy,
    Crosshair,
    ZoomIn,
    ZoomOut,
    Help,
    Progress,
    Transparent,
    MoveImage,
    CellImage,
    Custom(CursorImage),
}

#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(crate) fn mac_cursor(cursor: Cursor) -> MacCursor {
    match cursor {
        Cursor::Default => MacCursor::Arrow,
        Cursor::Pointer => MacCursor::PointingHand,
        Cursor::Text => MacCursor::IBeam,
        Cursor::Grab => MacCursor::OpenHand,
        Cursor::Grabbing => MacCursor::ClosedHand,
        Cursor::NotAllowed => MacCursor::NotAllowed,
        Cursor::ResizeColumn => MacCursor::ResizeLeftRight,
        Cursor::ResizeRow => MacCursor::ResizeUpDown,
        Cursor::ResizeEw => MacCursor::ResizeEw,
        Cursor::ResizeNs => MacCursor::ResizeNs,
        Cursor::ResizeNwse => MacCursor::ResizeNwse,
        Cursor::ResizeNesw => MacCursor::ResizeNesw,
        Cursor::ResizeN => MacCursor::ResizeN,
        Cursor::ResizeE => MacCursor::ResizeE,
        Cursor::ResizeS => MacCursor::ResizeS,
        Cursor::ResizeW => MacCursor::ResizeW,
        Cursor::Alias => MacCursor::Alias,
        Cursor::Copy => MacCursor::Copy,
        Cursor::Crosshair => MacCursor::Crosshair,
        Cursor::ZoomIn => MacCursor::ZoomIn,
        Cursor::ZoomOut => MacCursor::ZoomOut,
        Cursor::Help => MacCursor::Help,
        Cursor::Progress => MacCursor::Progress,
        Cursor::Hidden => MacCursor::Transparent,
        Cursor::Move => MacCursor::MoveImage,
        Cursor::Cell => MacCursor::CellImage,
        Cursor::Image(image) => MacCursor::Custom(image),
    }
}
