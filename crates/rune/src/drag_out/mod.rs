//! Dragging data out of a window into other apps: an OS drag source.
//!
//! An element marked [`crate::El::drag_out`] starts a system drag session once the pointer
//! travels [`DRAG_THRESHOLD`] points from the press. The session carries a [`DragItem`] (file
//! URLs, PNG data, plain text) and shows a [`DragImage`], by default a snapshot of the element
//! as the window last drew it, starting exactly over the element. [`crate::El::on_drag_start`]
//! and [`crate::El::on_drag_end`] bracket the session, so the app can style the source while
//! it's dragged; [`DragOutcome`] says how the drop ended. Rune windows take no drops from their
//! own app's drags unless [`DragItem::within_app`] says so, so a drag let go over the app ends
//! [`DragOutcome::Cancelled`] and the image slides back.
//!
//! macOS runs the session with `NSDraggingSource`, Windows with OLE's `DoDragDrop` and the shell's
//! drag image (`windows`), and Wayland with `wl_data_source` sharing `platform::wayland::drop`'s
//! per-display `wl_data_device` (`platform::wayland::drag_source`). X11 doesn't start one yet:
//! there `drag_out` is ignored and the element keeps its ordinary press and `on_drag` behaviour.
//!
//! Windows start the session from the mouse-dragged event AppKit is delivering. Overlay panels
//! track the pointer by polling, so there may be no such event; the overlay then builds a
//! dragged event at the current mouse location. Both paths have GUI tests on a real desktop
//! (`tests/macos_drag_out.rs`, and `tests/macos_overlay_drag_out.rs` for a non-activating
//! overlay that takes input only on its hit regions).

use crate::color::Color;
#[cfg(not(target_arch = "wasm32"))]
use crate::color::{srgb_decode, srgb_encode};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

/// Whether a host window on this platform starts OS drag sessions (`wayland` enables Linux
/// Wayland `wl_data_source`; X11 remains unsupported).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const fn supported(wayland: bool) -> bool {
    cfg!(any(target_os = "macos", target_os = "windows"))
        || (cfg!(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))
            && wayland)
}

/// Pointer travel (points, straight-line) from the press before a drag session starts.
/// AppKit views commonly use 4; GTK's default is 8; Windows' `SM_CXDRAG` is 4 pixels.
pub const DRAG_THRESHOLD: f32 = if cfg!(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))) { 8.0 } else { 4.0 };

/// Maximum total byte size across all custom MIME entries on one [`DragItem`] (32 MiB, matching
/// [`crate::clipboard::MAX_IMAGE_BYTES`]).
pub const MAX_CUSTOM_DATA_BYTES: usize = 32 * 1024 * 1024;

/// Maximum number of distinct custom MIME entries on one [`DragItem`].
pub const MAX_CUSTOM_DATA_ENTRIES: usize = 64;

/// Whether a pointer that went down at `from` and is now at `to` has travelled past
/// `threshold` points. [`crate::El::drag_threshold`] sets it per element.
pub(crate) fn past_threshold(from: (f32, f32), to: (f32, f32), threshold: f32) -> bool {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    dx * dx + dy * dy > threshold * threshold
}

/// What a drag carries to another app: any of file URLs, PNG data, plain text, and custom MIME
/// byte entries, plus the picture shown under the pointer.
///
/// ```
/// # use rune::drag_out::DragItem;
/// let item = DragItem::new().file("/Applications/Example.app");
/// assert_eq!(item.paths().len(), 1);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct DragItem {
    files: Vec<PathBuf>,
    png: Option<Vec<u8>>,
    text: Option<String>,
    custom: Vec<(String, Arc<[u8]>)>,
    image: Option<DragImage>,
    operations: u8,
    backing: Option<Color>,
    within_app: bool,
}

impl Default for DragItem {
    fn default() -> Self {
        Self { files: Vec::new(), png: None, text: None, custom: Vec::new(), image: None, operations: DragOperation::Copy.bit(), backing: None, within_app: false }
    }
}

impl DragItem {
    pub fn new() -> Self { Self::default() }

    /// Add a file (or folder or app bundle) by path; repeat for several. Finder copies it,
    /// and System Settings' privacy lists accept an app bundle.
    pub fn file(mut self, path: impl Into<PathBuf>) -> Self {
        self.files.push(path.into());
        self
    }

    /// PNG-encoded image data, for targets that take an image rather than a file.
    pub fn png(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.png = Some(bytes.into());
        self
    }

    /// Straight-alpha sRGB RGBA8 pixels, encoded as PNG data. Ignored if the sizes disagree.
    pub fn rgba(self, width: u32, height: u32, pixels: &[u8]) -> Self {
        match encode_png(width, height, pixels) {
            Some(png) => self.png(png),
            None => self,
        }
    }

    /// Plain text. Without it, a file's path is offered as text.
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Attach raw bytes under a custom MIME type (for example `"text/uri-list"`,
    /// `"x-special/gnome-copied-files"`, `"application/x-kde-cutselection"`, or an app-private
    /// type). Entries keep their insertion order (offer preference); repeating a MIME type
    /// replaces its bytes in place without moving it.
    ///
    /// Invalid MIME strings (empty, over 255 bytes, non-ASCII, NUL/control characters, or
    /// leading/trailing whitespace) and entries that would exceed [`MAX_CUSTOM_DATA_ENTRIES`]
    /// or [`MAX_CUSTOM_DATA_BYTES`] are ignored.
    ///
    /// On macOS each MIME type is written to the first `NSPasteboardItem` under its UTI (or a
    /// deterministic `dyn.*` UTI for unregistered types). On Windows each MIME type is registered
    /// as a clipboard format (`RegisterClipboardFormatW`, cached process-wide up to 256 distinct
    /// custom names) and attached to the OLE `IDataObject`. On Wayland each MIME type is offered
    /// on `wl_data_source` in insertion order (overriding any colliding built-in MIME type) and
    /// streamed off the UI thread over a non-blocking pipe.
    ///
    /// ```
    /// # use rune::DragItem;
    /// let item = DragItem::new()
    ///     .data("text/uri-list", "sftp://host/file.txt\r\n")
    ///     .data("x-special/gnome-copied-files", "copy\nsftp://host/file.txt")
    ///     .data("application/x-kde-cutselection", b"0".as_slice());
    /// assert_eq!(item.data_for("application/x-kde-cutselection"), Some(b"0".as_slice()));
    /// assert_eq!(item.custom_data().len(), 3);
    /// ```
    pub fn data(mut self, mime: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        let mime = mime.into();
        if !valid_mime(&mime) { return self; }
        let bytes: Vec<u8> = bytes.into();
        let current_total: usize = self.custom.iter().map(|(_, b)| b.len()).sum();
        if let Some((_, existing)) = self.custom.iter_mut().find(|(m, _)| m == &mime) {
            let base = current_total - existing.len();
            if base.checked_add(bytes.len()).is_some_and(|total| total <= MAX_CUSTOM_DATA_BYTES) {
                *existing = Arc::from(bytes);
            }
            return self;
        }
        if self.custom.len() >= MAX_CUSTOM_DATA_ENTRIES { return self; }
        if current_total.checked_add(bytes.len()).is_some_and(|total| total <= MAX_CUSTOM_DATA_BYTES) {
            self.custom.push((mime, Arc::from(bytes)));
        }
        self
    }

    /// The picture under the pointer. Defaults to a snapshot of the element.
    pub fn image(mut self, image: DragImage) -> Self {
        self.image = Some(image);
        self
    }

    /// Show the drag image composited over `color`, so it's opaque even where the element or
    /// its window is transparent (an overlay panel, a translucent window material). AppKit
    /// still draws its drag window at about 90% opacity, as it does for every app's drag.
    ///
    /// ```
    /// # use rune::{Color, DragItem};
    /// let item = DragItem::new().text("row").opaque(Color::WHITE);
    /// assert_eq!(item.backing(), Some(Color::WHITE));
    /// ```
    pub fn opaque(mut self, color: Color) -> Self {
        self.backing = Some(color.alpha(1.0));
        self
    }

    /// Offer only `operation` to the target, instead of the default [`DragOperation::Copy`].
    ///
    /// ```
    /// # use rune::{DragItem, DragOperation};
    /// let item = DragItem::new().text("row").operation(DragOperation::Link);
    /// assert!(item.offers(DragOperation::Link) && !item.offers(DragOperation::Copy));
    /// ```
    pub fn operation(mut self, operation: DragOperation) -> Self {
        self.operations = operation.bit();
        self
    }

    /// Offer `operation` as well as those already offered; the target picks one.
    pub fn allow(mut self, operation: DragOperation) -> Self {
        self.operations |= operation.bit();
        self
    }

    /// Let the target move files instead of copying them (Finder moves within one volume).
    /// By default a drag only offers a copy, whatever modifier keys are held.
    pub fn allow_move(self) -> Self { self.allow(DragOperation::Move) }

    /// Offer the drag to this app's own windows too, so their `on_drop_files` targets receive
    /// its files as a drop from Finder would (an image dragged from a timeline into the
    /// composer). By default the app's windows take no drops from its own drags. A drag let go
    /// over a window with no target that accepts it ends [`DragOutcome::Copied`] without sliding
    /// back, as the window takes any file drag.
    ///
    /// ```
    /// # use rune::DragItem;
    /// assert!(DragItem::new().file("/a.png").within_app().drops_within_app());
    /// assert!(!DragItem::new().drops_within_app());
    /// ```
    pub fn within_app(mut self) -> Self {
        self.within_app = true;
        self
    }

    pub fn paths(&self) -> &[PathBuf] { &self.files }
    pub fn png_data(&self) -> Option<&[u8]> { self.png.as_deref() }
    pub fn plain_text(&self) -> Option<&str> { self.text.as_deref() }

    /// The custom bytes attached for `mime` via [`DragItem::data`], if any.
    ///
    /// ```
    /// # use rune::DragItem;
    /// let item = DragItem::new().data("application/x-example", b"ok".as_slice());
    /// assert_eq!(item.data_for("application/x-example"), Some(b"ok".as_slice()));
    /// assert_eq!(item.data_for("text/plain"), None);
    /// ```
    pub fn data_for(&self, mime: &str) -> Option<&[u8]> {
        self.custom.iter().find(|(m, _)| m == mime).map(|(_, b)| b.as_ref())
    }

    /// Custom `(mime, bytes)` entries attached via [`DragItem::data`], in insertion order.
    ///
    /// ```
    /// # use rune::DragItem;
    /// let item = DragItem::new()
    ///     .data("text/uri-list", "sftp://host/a\r\n")
    ///     .data("application/x-kde-cutselection", b"0".as_slice());
    /// let mimes: Vec<&str> = item.custom_data().map(|(m, _)| m).collect();
    /// assert_eq!(mimes, ["text/uri-list", "application/x-kde-cutselection"]);
    /// ```
    pub fn custom_data(&self) -> impl ExactSizeIterator<Item = (&str, &[u8])> {
        self.custom.iter().map(|(m, b)| (m.as_str(), b.as_ref()))
    }

    pub fn drag_image(&self) -> Option<&DragImage> { self.image.as_ref() }
    pub fn allows_move(&self) -> bool { self.offers(DragOperation::Move) }
    /// Whether the app's own windows take this drag's drop ([`DragItem::within_app`]).
    pub fn drops_within_app(&self) -> bool { self.within_app }
    /// Whether the drag offers `operation` to the target.
    pub fn offers(&self, operation: DragOperation) -> bool { self.operations & operation.bit() != 0 }
    /// All operations this drag offers to the target.
    pub fn operations(&self) -> DragOperations { DragOperations(self.operations) }
    /// The colour the drag image is shown over, from [`DragItem::opaque`].
    pub fn backing(&self) -> Option<Color> { self.backing }
    /// Carries nothing a target could receive.
    pub fn is_empty(&self) -> bool { self.files.is_empty() && self.png.is_none() && self.text.is_none() && self.custom.is_empty() }
}

/// What a drag lets the target do with the data. A drag offers [`DragOperation::Copy`] unless
/// [`DragItem::operation`] or [`DragItem::allow`] say otherwise, and modifier keys don't change
/// the offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DragOperation {
    /// The target copies the data.
    Copy,
    /// The target moves the files.
    Move,
    /// The target makes a link or alias to the files.
    Link,
}

impl DragOperation {
    const fn bit(self) -> u8 {
        match self { Self::Copy => 1, Self::Move => 2, Self::Link => 4 }
    }
}

/// Set of [`DragOperation`]s a drag source allows (or a drop target accepts).
///
/// ```
/// # use rune::{DragOperation, DragOperations, Mods};
/// let allowed = DragOperations::COPY | DragOperations::MOVE;
/// assert!(allowed.contains(DragOperation::Move));
/// assert_eq!(allowed.resolve(Mods { shift: true, ..Mods::default() }), Some(DragOperation::Move));
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DragOperations(u8);

impl DragOperations {
    pub const NONE: Self = Self(0);
    pub const COPY: Self = Self(DragOperation::Copy.bit());
    pub const MOVE: Self = Self(DragOperation::Move.bit());
    pub const LINK: Self = Self(DragOperation::Link.bit());
    pub const ALL: Self = Self(DragOperation::Copy.bit() | DragOperation::Move.bit() | DragOperation::Link.bit());

    /// A set containing only `operation`.
    pub const fn from_operation(operation: DragOperation) -> Self { Self(operation.bit()) }
    /// Whether no operations are allowed.
    pub const fn is_empty(self) -> bool { self.0 == 0 }
    /// Whether `operation` is allowed.
    pub const fn contains(self, operation: DragOperation) -> bool { self.0 & operation.bit() != 0 }
    /// Returns `self` with `operation` added.
    pub const fn with(self, operation: DragOperation) -> Self { Self(self.0 | operation.bit()) }
    /// Operations allowed by either `self` or `other`.
    pub const fn union(self, other: Self) -> Self { Self(self.0 | other.0) }
    /// Operations allowed by both `self` and `other`.
    pub const fn intersection(self, other: Self) -> Self { Self(self.0 & other.0) }

    /// Standard desktop modifier resolution among the operations in `self`:
    /// `Ctrl+Shift` or `Alt` prefers [`DragOperation::Link`], `Shift` prefers
    /// [`DragOperation::Move`], `Ctrl` prefers [`DragOperation::Copy`], and no modifier falls
    /// back to `Copy`, then `Move`, then `Link`. Returns `None` when `self` is empty.
    pub fn resolve(self, mods: crate::Mods) -> Option<DragOperation> {
        if self.is_empty() { return None; }
        if (mods.alt || (mods.ctrl && mods.shift)) && self.contains(DragOperation::Link) {
            return Some(DragOperation::Link);
        }
        if mods.shift && !mods.ctrl && self.contains(DragOperation::Move) {
            return Some(DragOperation::Move);
        }
        if mods.ctrl && !mods.shift && self.contains(DragOperation::Copy) {
            return Some(DragOperation::Copy);
        }
        if self.contains(DragOperation::Copy) { Some(DragOperation::Copy) }
        else if self.contains(DragOperation::Move) { Some(DragOperation::Move) }
        else if self.contains(DragOperation::Link) { Some(DragOperation::Link) }
        else { None }
    }
}

impl From<DragOperation> for DragOperations {
    fn from(op: DragOperation) -> Self { Self::from_operation(op) }
}

impl std::ops::BitOr for DragOperations {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self { self.union(rhs) }
}

impl std::ops::BitOrAssign for DragOperations {
    fn bitor_assign(&mut self, rhs: Self) { *self = self.union(rhs); }
}

impl std::ops::BitAnd for DragOperations {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self { self.intersection(rhs) }
}

impl std::ops::BitAndAssign for DragOperations {
    fn bitand_assign(&mut self, rhs: Self) { *self = self.intersection(rhs); }
}

/// The picture a drag shows under the pointer, as PNG data.
#[derive(Clone, Debug, PartialEq)]
pub struct DragImage {
    png: Vec<u8>,
    /// Display size in points; `None` draws one pixel per point at the window's scale.
    size: Option<(f32, f32)>,
    placement: Placement,
}

/// Whether `mime` is non-empty, <= 255 bytes, printable ASCII without leading/trailing
/// whitespace, NUL, or control characters (< 0x20 or 0x7F).
fn valid_mime(mime: &str) -> bool {
    let bytes = mime.as_bytes();
    if bytes.is_empty() || bytes.len() > 255 { return false; }
    if bytes[0] == b' ' || bytes[bytes.len() - 1] == b' ' { return false; }
    bytes.iter().all(|&b| (0x20..=0x7E).contains(&b))
}

/// Where a drag image starts, in window points.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Placement {
    /// Centred under the press.
    Pointer,
    /// Over the element's top-left, at the element's full (unclipped) size unless sized.
    Element,
    /// Positioned so `(x, y)` in image-local points sits under the press.
    Hotspot(f32, f32),
    /// Top-left at this point, at the image's own size (the snapshot's pixel-aligned crop).
    #[cfg(not(target_arch = "wasm32"))]
    At(f32, f32),
}

impl DragImage {
    pub fn png(bytes: impl Into<Vec<u8>>) -> Self { Self { png: bytes.into(), size: None, placement: Placement::Pointer } }

    /// Straight-alpha sRGB RGBA8 pixels. `None` if `pixels` isn't `width * height * 4` bytes.
    pub fn rgba(width: u32, height: u32, pixels: &[u8]) -> Option<Self> {
        encode_png(width, height, pixels).map(Self::png)
    }

    /// Draw it this many points wide and high.
    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.size = Some((width, height));
        self
    }

    /// Start the drag with the image over the element, filling its bounds (scaled to fit), as
    /// the default snapshot does, instead of centred under the pointer. Use it for a picture
    /// rendered at the element's size, so the drag appears to lift the element itself. With
    /// [`DragImage::size`] it keeps that size, from the element's top-left. A row scrolled
    /// partly out of view still gets its whole size, not the visible strip.
    pub fn over_element(mut self) -> Self {
        self.placement = Placement::Element;
        self
    }

    /// Position the image so the image-local point `(x, y)` in points sits directly under the
    /// pointer instead of centring the image under the press. Non-finite values are treated as
    /// `0.0`.
    ///
    /// ```
    /// # use rune::DragImage;
    /// let img = DragImage::rgba(4, 4, &[0; 64]).unwrap().size(48.0, 48.0).hotspot(12.0, 12.0);
    /// assert_eq!(img.hotspot_point(), Some((12.0, 12.0)));
    /// ```
    pub fn hotspot(mut self, x: f32, y: f32) -> Self {
        let x = if x.is_finite() { x } else { 0.0 };
        let y = if y.is_finite() { y } else { 0.0 };
        self.placement = Placement::Hotspot(x, y);
        self
    }

    /// The image-local hotspot in points set by [`DragImage::hotspot`], if any.
    pub fn hotspot_point(&self) -> Option<(f32, f32)> {
        match self.placement {
            Placement::Hotspot(x, y) => Some((x, y)),
            _ => None,
        }
    }

    pub fn png_data(&self) -> &[u8] { &self.png }
    pub fn points(&self) -> Option<(f32, f32)> { self.size }

    /// Where the drag image starts, x, y, w, h in window points: `bounds` is the element's
    /// unclipped rect, and `natural` the image's size in points when [`DragImage::size`]
    /// wasn't set.
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
    pub(crate) fn frame(&self, bounds: [f32; 4], press: (f32, f32), natural: (f32, f32)) -> [f32; 4] {
        let (w, h) = self.size.unwrap_or(natural);
        match self.placement {
            Placement::Pointer => [press.0 - w / 2.0, press.1 - h / 2.0, w, h],
            Placement::Element => {
                let (w, h) = self.size.unwrap_or((bounds[2], bounds[3]));
                [bounds[0], bounds[1], w.max(1.0), h.max(1.0)]
            }
            Placement::Hotspot(hx, hy) => [press.0 - hx, press.1 - hy, w, h],
            #[cfg(not(target_arch = "wasm32"))]
            Placement::At(x, y) => [x, y, w, h],
        }
    }

    /// The image composited over an opaque `backing`, blended in linear light as the window
    /// blends. `None` if the PNG doesn't decode.
    #[cfg(not(target_arch = "wasm32"))]
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows", all(unix, not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
    pub(crate) fn over(&self, backing: Color) -> Option<Self> {
        let mut rgba = crate::image::png_rgba8(&self.png).ok()?;
        let bg = [backing.r, backing.g, backing.b];
        for p in rgba.pixels_mut() {
            let a = p[3] as f32 / 255.0;
            for c in 0..3 {
                let v = srgb_decode(p[c] as f32 / 255.0) * a + bg[c] * (1.0 - a);
                p[c] = (srgb_encode(v) * 255.0).round().clamp(0.0, 255.0) as u8;
            }
            p[3] = 255;
        }
        let png = encode_png(rgba.width(), rgba.height(), rgba.as_raw())?;
        Some(Self { png, ..self.clone() })
    }
}

/// How a drag out of the window ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DragOutcome {
    /// The target copied the data.
    Copied,
    /// The target moved the files (only offered after [`DragItem::allow_move`] or
    /// [`DragOperation::Move`]), or the drop
    /// asked the source to delete them (the Trash).
    Moved,
    /// The target made a link or alias to the files.
    Linked,
    /// Dropped nowhere that accepted it, or cancelled with Escape.
    Cancelled,
}

/// A drag the runtime asked the host to start, inside the pointer event that crossed the
/// threshold.
pub(crate) struct DragOutStart {
    pub path: String,
    // Read by desktop drag_out::begin on macOS/Windows; constructed by shared runtime pointer handling.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub item: DragItem,
    /// The element's visible drawn rect in window points: x, y, w, h.
    pub rect: [f32; 4],
    /// The element's whole drawn rect, before scroll clipping.
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows", all(unix, not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
    pub bounds: [f32; 4],
    /// Where the pointer went down, in window points.
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows", all(unix, not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
    pub press: (f32, f32),
}

pub(crate) type DragEnds = std::rc::Rc<std::cell::RefCell<Vec<(String, DragOutcome)>>>;

pub(crate) fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let expected = usize::try_from(width)
        .ok()
        .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)?.checked_mul(4)))?;
    if width == 0 || height == 0 || rgba.len() != expected { return None; }
    #[cfg(any(not(target_arch = "wasm32"), feature = "png"))]
    {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        let mut w = enc.write_header().ok()?;
        w.write_image_data(rgba).ok()?;
        w.finish().ok()?;
        Some(out)
    }
    #[cfg(all(target_arch = "wasm32", not(feature = "png")))]
    {
        encode_png_stored(width, height, rgba)
    }
}

/// Minimal dependency-free RGBA8 PNG encoder (uncompressed RFC 1950/1951 stored deflate blocks
/// with an sRGB Perceptual chunk), used on `wasm32` when the `png` feature is off so
/// `ClipboardImage::from_rgba` and `DragImage::from_rgba` keep working without linking `png`,
/// `fdeflate`, or `miniz_oxide`.
#[cfg(any(test, all(target_arch = "wasm32", not(feature = "png"))))]
pub(crate) fn encode_png_stored(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let row_bytes = usize::try_from(width).ok()?.checked_mul(4)?;
    let h = usize::try_from(height).ok()?;
    let raw_len = row_bytes.checked_add(1)?.checked_mul(h)?;
    if width == 0 || height == 0 || rgba.len() != row_bytes.checked_mul(h)? {
        return None;
    }
    let crc32 = |data: &[u8]| -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if (crc & 1) != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
            }
        }
        !crc
    };
    let push_chunk = |out: &mut Vec<u8>, kind: [u8; 4], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(&kind);
        out.extend_from_slice(data);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    };

    let mut filtered = Vec::with_capacity(raw_len);
    let (mut s1, mut s2) = (1u32, 0u32);
    for row in rgba.chunks_exact(row_bytes) {
        filtered.push(0);
        s2 += s1;
        if s2 >= 65521 { s2 -= 65521; }
        filtered.extend_from_slice(row);
        for &b in row {
            s1 += u32::from(b);
            if s1 >= 65521 { s1 -= 65521; }
            s2 += s1;
            if s2 >= 65521 { s2 -= 65521; }
        }
    }
    let blocks = filtered.len().div_ceil(65535).max(1);
    let mut zlib = Vec::with_capacity(2 + filtered.len() + blocks * 5 + 4);
    zlib.extend_from_slice(&[0x78, 0x01]);
    let mut off = 0;
    while off < filtered.len() {
        let chunk = (filtered.len() - off).min(65535);
        let last = u8::from(off + chunk == filtered.len());
        let len = chunk as u16;
        zlib.push(last);
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(&filtered[off..off + chunk]);
        off += chunk;
    }
    zlib.extend_from_slice(&((s2 << 16) | s1).to_be_bytes());

    let mut out = Vec::with_capacity(8 + 25 + 13 + 12 + zlib.len() + 12);
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = [0u8; 13];
    ihdr[0..4].copy_from_slice(&width.to_be_bytes());
    ihdr[4..8].copy_from_slice(&height.to_be_bytes());
    ihdr[8] = 8; // bit depth
    ihdr[9] = 6; // RGBA
    push_chunk(&mut out, *b"IHDR", &ihdr);
    push_chunk(&mut out, *b"sRGB", &[0]); // Perceptual
    push_chunk(&mut out, *b"IDAT", &zlib);
    push_chunk(&mut out, *b"IEND", &[]);
    Some(out)
}

/// Draws the last frame's instances again offscreen and reads back `rect` (window points), so
/// the drag shows what the user grabbed. `None` for formats it can't read back.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn snapshot(gpu: &crate::render::Gpu, renderer: &crate::render::Renderer, size: (u32, u32), scale: f32, clear: [f32; 4], rect: [f32; 4]) -> Option<DragImage> {
    let format = renderer.format();
    let bgra = match format {
        wgpu::TextureFormat::Bgra8UnormSrgb | wgpu::TextureFormat::Bgra8Unorm => true,
        wgpu::TextureFormat::Rgba8UnormSrgb | wgpu::TextureFormat::Rgba8Unorm => false,
        _ => return None,
    };
    let px = crop(rect, scale, size)?;
    let [x, y, w, h] = px;
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("drag image"),
        size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let readback = rune_staging::Readback::new(&gpu.device, "drag image readback", w, h, 4);
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut enc, &view, clear);
    readback.copy_from_origin(&mut enc, &texture, wgpu::Origin3d { x, y, z: 0 });
    gpu.queue.submit([enc.finish()]);
    readback.buffer().slice(..).map_async(wgpu::MapMode::Read, |_| {});
    rune_gpu::health::poll(&gpu.device, wgpu::PollType::wait_indefinitely()).ok()?;
    let mut rgba = readback.try_take_mapped()?;
    to_straight_rgba(&mut rgba, bgra, format.is_srgb());
    let mut image = DragImage::rgba(w, h, &rgba)?.size(w as f32 / scale, h as f32 / scale);
    // The crop rounds out to whole pixels, so the image sits exactly over what was drawn.
    image.placement = Placement::At(x as f32 / scale, y as f32 / scale);
    Some(image)
}

/// `rect` (points) in device pixels, clamped to the frame: x, y, w, h. `None` when empty.
#[cfg(not(target_arch = "wasm32"))]
fn crop(rect: [f32; 4], scale: f32, size: (u32, u32)) -> Option<[u32; 4]> {
    let x0 = (rect[0] * scale).floor().clamp(0.0, size.0 as f32) as u32;
    let y0 = (rect[1] * scale).floor().clamp(0.0, size.1 as f32) as u32;
    let x1 = ((rect[0] + rect[2]) * scale).ceil().clamp(0.0, size.0 as f32) as u32;
    let y1 = ((rect[1] + rect[3]) * scale).ceil().clamp(0.0, size.1 as f32) as u32;
    (x1 > x0 && y1 > y0).then_some([x0, y0, x1 - x0, y1 - y0])
}

/// Frame pixels are premultiplied, in the surface's channel order. On `*Srgb` targets the bytes
/// hold `srgb_encode(linear_rgb * a)` and must be un-premultiplied in linear light; on non-sRGB
/// targets (`srgb_blend`) they hold `srgb_encode(linear_rgb) * a` and divide directly.
#[cfg(not(target_arch = "wasm32"))]
fn to_straight_rgba(pixels: &mut [u8], bgra: bool, srgb_target: bool) {
    if bgra {
        for p in pixels.chunks_exact_mut(4) {
            p.swap(0, 2);
        }
    }
    if srgb_target {
        crate::render::surface_alpha::straight_alpha(pixels);
    } else {
        for p in pixels.chunks_exact_mut(4) {
            let a = p[3];
            if a == 0 {
                p[..3].fill(0);
            } else if a != 255 {
                for c in &mut p[..3] {
                    *c = ((*c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
                }
            }
        }
    }
}

/// Starts the OS drag session for `start` from inside the pointer event that crossed the
/// threshold. `done` runs once with how it ended (also when it couldn't start: `Cancelled`).
#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(not(any(target_os = "macos", target_os = "windows", all(unix, not(target_os = "ios"), not(target_os = "android")))), allow(unused_variables))]
pub(crate) fn begin(window: &winit::window::Window, start: DragOutStart, snapshot: Option<DragImage>, done: Box<dyn FnOnce(DragOutcome)>) {
    #[cfg(target_os = "macos")]
    { let picture = picture(&start.item, snapshot); macos::begin(window, start, picture, done); }
    #[cfg(target_os = "windows")]
    { let picture = picture(&start.item, snapshot); windows::begin(window, start, picture, done); }
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    {
        let picture = picture(&start.item, snapshot);
        let scale = window.scale_factor() as f32;
        let offers = crate::platform::wayland::drag_source::mime::offers_from_item(&start.item);
        let actions_mask = crate::platform::wayland::drag_source::mime::dnd_action_mask(
            start.item.offers(DragOperation::Copy),
            start.item.offers(DragOperation::Move),
        );
        let icon = picture.as_ref().and_then(|img| {
            crate::platform::wayland::drag_source::icon::prepare_icon(img, start.bounds, start.press, scale)
        });
        crate::platform::wayland::drop::begin_drag(
            window,
            start.item.drops_within_app(),
            offers,
            actions_mask,
            icon,
            done,
        );
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", all(unix, not(target_os = "ios"), not(target_os = "android")))))]
    done(DragOutcome::Cancelled);
}

/// The item's own image, else the snapshot, over the item's backing colour if it has one.
#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(not(any(target_os = "macos", target_os = "windows", all(unix, not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
fn picture(item: &DragItem, snapshot: Option<DragImage>) -> Option<DragImage> {
    let picture = item.drag_image().cloned().or(snapshot)?;
    match item.backing() {
        Some(backing) => picture.over(backing).or(Some(picture)),
        None => Some(picture),
    }
}

/// The path offered as text when a file drag has no text of its own.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
fn path_text(path: &Path) -> String { path.to_string_lossy().into_owned() }

#[cfg(test)]
mod tests;
