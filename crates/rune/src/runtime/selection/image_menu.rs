//! Opt-in image context menu ([`ImageMenu`], [`El::image_menu`]): Save Image As…, Copy Image,
//! and Copy Image Address.
//!
//! No image element gets a menu by default because `El::pixels` / `El::pixels_fit` also draw
//! icons, avatars and chrome. Opt in per element with [`El::image_menu`]; the item list goes
//! through the same pure [`build_context_menu`](super::link_menu::build_context_menu) function
//! and unified `Popups` queue as links and selectable text.
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use super::link_menu::{ContextMenuTarget, ImageMenuTarget, build_context_menu};
use super::*;

/// Maximum source file size copied by `Save Image As…` (256 MiB, matching `rune-image`'s
/// `MAX_ALLOC`).
#[cfg(not(target_arch = "wasm32"))]
const MAX_SAVE_FILE_BYTES: u64 = 256 * 1024 * 1024;

/// Original image source handed to `Save Image As…` so SVG, SVGZ, APNG, animated GIF and
/// animated WebP are saved verbatim off the UI thread rather than re-encoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SaveSource {
    Bytes(Arc<[u8]>),
    Path(PathBuf),
}

/// Configuration for an opt-in image context menu ([`El::image_menu`](crate::El::image_menu)):
/// **Save Image As…**, **Copy Image**, and **Copy Image Address** in Chromium's order.
///
/// Composes directly with `rune_kit::Markdown::image_frame(|el, frame| el.image_menu(...))`.
///
/// ```
/// # use std::sync::Arc;
/// # use rune::{Image, ImageMenu, SurfaceFit, el};
/// # fn _view(img: &Image, raw_bytes: &Arc<[u8]>) {
/// let _photo = el()
///     .size(320.0, 200.0)
///     .pixels_fit(img.pixels(), SurfaceFit::Fit)
///     .image_menu(
///         ImageMenu::new(img)
///             .save_bytes(raw_bytes.clone())
///             .file_name("diagram.svg")
///             .address("https://example.com/diagram.svg"),
///     );
/// # }
/// ```
#[derive(Clone)]
pub struct ImageMenu {
    image: crate::Image,
    save: Option<SaveSource>,
    file_name: Option<String>,
    address: Option<String>,
}

impl std::fmt::Debug for ImageMenu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("ImageMenu");
        s.field("image", &self.image);
        match &self.save {
            Some(SaveSource::Bytes(b)) => { s.field("save_bytes", &b.len()); }
            Some(SaveSource::Path(p)) => { s.field("save_path", p); }
            None => { s.field("save", &None::<()>); }
        }
        s.field("file_name", &self.file_name)
            .field("address", &self.address)
            .finish()
    }
}

impl PartialEq for ImageMenu {
    fn eq(&self, other: &Self) -> bool {
        self.image.id() == other.image.id()
            && self.file_name == other.file_name
            && self.address == other.address
            && match (&self.save, &other.save) {
                (None, None) => true,
                (Some(SaveSource::Bytes(a)), Some(SaveSource::Bytes(b))) => Arc::ptr_eq(a, b),
                (Some(SaveSource::Path(a)), Some(SaveSource::Path(b))) => a == b,
                _ => false,
            }
    }
}

impl ImageMenu {
    /// Start an image context menu for `image`. By itself this offers **Copy Image** (enabled
    /// once `image` has decoded with no error and within 64 megapixels). Add
    /// [`Self::save_bytes`] or [`Self::save_path`] to offer **Save Image As…** on native targets,
    /// and [`Self::address`] to offer **Copy Image Address**.
    pub fn new(image: &crate::Image) -> Self {
        Self {
            image: image.clone(),
            save: None,
            file_name: None,
            address: None,
        }
    }

    /// Original encoded bytes written verbatim on a background thread when the user picks
    /// **Save Image As…** (preserving SVG, SVGZ, APNG, animated GIF and animated WebP). Clears
    /// any earlier [`Self::save_path`]. On `wasm32`, **Save Image As…** is omitted because
    /// [`Tx::save_file`](crate::Tx::save_file) is not yet supported on the web.
    pub fn save_bytes(mut self, bytes: impl Into<Arc<[u8]>>) -> Self {
        self.save = Some(SaveSource::Bytes(bytes.into()));
        self
    }

    /// Original regular file copied verbatim on a background thread when the user picks
    /// **Save Image As…** (refusing non-regular files and files over 256 MiB). Clears any
    /// earlier [`Self::save_bytes`]. On `wasm32`, **Save Image As…** is omitted because
    /// [`Tx::save_file`](crate::Tx::save_file) is not yet supported on the web.
    pub fn save_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.save = Some(SaveSource::Path(path.into()));
        self
    }

    /// Suggested filename for the **Save Image As…** dialog (path separators, control
    /// characters and `..` are stripped). An empty string clears it.
    pub fn file_name(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        self.file_name = (!name.is_empty()).then_some(name);
        self
    }

    /// Source URL offered by **Copy Image Address** (shown only when valid under
    /// `http:`, `https:` or `mailto:`, matching Rune's link menu) and used to infer a
    /// filename when [`Self::file_name`] and [`Self::save_path`] are unset. An empty string
    /// clears it.
    pub fn address(mut self, url: impl Into<String>) -> Self {
        let url = url.into();
        self.address = (!url.is_empty()).then_some(url);
        self
    }

    /// Validated address eligible for **Copy Image Address** (`http:`, `https:`, `mailto:`).
    pub(crate) fn valid_address(&self) -> Option<&str> {
        let raw = self.address.as_deref()?;
        crate::open_url::check(raw).is_ok().then(|| raw.trim())
    }

    /// Pure menu state derived in O(1) from `self` without touching the filesystem or encoding.
    pub(crate) fn target(&self, can_save: bool) -> ImageMenuTarget {
        let ready = self.image.is_ready() && self.image.error().is_none();
        let within_pixels = self.image.size().is_some_and(|[w, h]| crate::clipboard::valid_image_size(w, h));
        ImageMenuTarget {
            has_save: can_save && self.save.is_some(),
            ready,
            copy_enabled: ready && within_pixels,
            has_address: self.valid_address().is_some(),
        }
    }
}

impl crate::El {
    /// Attach an opt-in image context menu (**Save Image As…**, **Copy Image**, **Copy Image
    /// Address**) to this element.
    ///
    /// Stored cold behind an `Rc` so elements without it pay zero bytes. An explicit
    /// [`El::on_secondary_press`] on this element always wins; otherwise this menu wins over an
    /// ancestor's `on_secondary_press`. Does not make the element implicitly focusable; if the
    /// element is focusable or active descendant, Menu / Shift+F10 and AccessKit
    /// `ShowContextMenu` open it too.
    ///
    /// Composes straight onto `rune_kit::Markdown::image_frame(|el, frame| el.image_menu(...))`.
    ///
    /// ```
    /// # use rune::{Image, ImageMenu, SurfaceFit, el};
    /// # fn _photo(img: &Image) {
    /// let _el = el()
    ///     .size(200.0, 120.0)
    ///     .pixels_fit(img.pixels(), SurfaceFit::Fit)
    ///     .image_menu(ImageMenu::new(img).address("https://example.com/photo.png"));
    /// # }
    /// ```
    pub fn image_menu(mut self, menu: ImageMenu) -> Self {
        self.vis.cold.image_menu = Some(Rc::new(menu));
        self
    }
}

#[cfg(any(not(target_arch = "wasm32"), test))]
const KNOWN_IMAGE_EXTS: [&str; 11] = [
    "png", "jpg", "jpeg", "webp", "gif", "avif", "svg", "svgz", "bmp", "ico", "icns",
];

/// Sanitizes a single filename component: drops directory prefixes, strips path separators,
/// control characters and `..`, and trims spaces/dots.
#[cfg(any(not(target_arch = "wasm32"), test))]
pub(crate) fn sanitize_file_name(raw: &str) -> Option<String> {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let cleaned: String = base
        .chars()
        .filter(|&c| !c.is_control() && !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .collect();
    let trimmed = cleaned.trim().trim_matches('.');
    if trimmed.is_empty() || trimmed.contains("..") {
        return None;
    }
    Some(trimmed.to_owned())
}

#[cfg(any(not(target_arch = "wasm32"), test))]
fn known_ext_of(name: &str) -> Option<&'static str> {
    let (_, ext) = name.rsplit_once('.')?;
    let lower = ext.to_ascii_lowercase();
    KNOWN_IMAGE_EXTS.iter().copied().find(|&k| k == lower)
}

/// Infers an image extension from magic bytes (falling back to `"png"`).
#[cfg(any(not(target_arch = "wasm32"), test))]
pub(crate) fn infer_image_ext(bytes: &[u8]) -> &'static str {
    if bytes.len() >= 8 && &bytes[..8] == b"\x89PNG\r\n\x1a\n" {
        return "png";
    }
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return "jpg";
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return "webp";
    }
    if bytes.len() >= 6 && (&bytes[..6] == b"GIF87a" || &bytes[..6] == b"GIF89a") {
        return "gif";
    }
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        let head = &bytes[8..bytes.len().min(32)];
        if head.windows(4).any(|w| w == b"avif" || w == b"avis") {
            return "avif";
        }
    }
    if bytes.len() >= 2 && bytes[0] == 0x1F && bytes[1] == 0x8B {
        return "svgz";
    }
    if bytes.len() >= 2 && &bytes[..2] == b"BM" {
        return "bmp";
    }
    if bytes.len() >= 4 && bytes[..4] == [0x00, 0x00, 0x01, 0x00] {
        return "ico";
    }
    if bytes.len() >= 4 && &bytes[..4] == b"icns" {
        return "icns";
    }
    let prefix = &bytes[..bytes.len().min(1024)];
    if let Ok(text) = std::str::from_utf8(prefix) {
        let trimmed = text.trim_start_matches('\u{feff}').trim_start();
        if trimmed.len() >= 4 && trimmed[..4].eq_ignore_ascii_case("<svg") {
            return "svg";
        }
        if trimmed.len() >= 5
            && trimmed[..5].eq_ignore_ascii_case("<?xml")
            && text.to_ascii_lowercase().contains("<svg")
        {
            return "svg";
        }
    }
    "png"
}

/// Suggested filename and extension for **Save Image As…**:
/// sanitized `file_name` -> `save_path.file_name()` -> sanitized last URL segment with a known
/// image extension -> `"image.<ext>"` inferred from magic bytes.
#[cfg(any(not(target_arch = "wasm32"), test))]
pub(crate) fn suggested_file_name(
    file_name: Option<&str>,
    save: Option<&SaveSource>,
    address: Option<&str>,
) -> (String, &'static str) {
    let magic_ext = match save {
        Some(SaveSource::Bytes(b)) => Some(infer_image_ext(b)),
        _ => None,
    };
    let path_name = match save {
        Some(SaveSource::Path(p)) => p.file_name().and_then(|n| n.to_str()).and_then(sanitize_file_name),
        _ => None,
    };
    let url_name = address.and_then(|url| {
        let without_q = url.split(['?', '#']).next()?;
        let seg = without_q.rsplit('/').next()?;
        let clean = sanitize_file_name(seg)?;
        known_ext_of(&clean).map(|_| clean)
    });
    if let Some(name) = file_name.and_then(sanitize_file_name) {
        let ext = known_ext_of(&name)
            .or_else(|| path_name.as_deref().and_then(known_ext_of))
            .or_else(|| url_name.as_deref().and_then(known_ext_of))
            .or(magic_ext)
            .unwrap_or("png");
        return (name, ext);
    }
    if let Some(name) = path_name {
        let ext = known_ext_of(&name)
            .or_else(|| url_name.as_deref().and_then(known_ext_of))
            .or(magic_ext)
            .unwrap_or("png");
        return (name, ext);
    }
    if let Some(name) = url_name {
        let ext = known_ext_of(&name).unwrap_or("png");
        return (name, ext);
    }
    let ext = magic_ext.unwrap_or("png");
    (format!("image.{ext}"), ext)
}

#[cfg(any(not(target_arch = "wasm32"), test))]
pub(crate) fn save_filter(ext: &'static str) -> (&'static str, &'static [&'static str]) {
    match ext {
        "png" => ("PNG Image", &["png"]),
        "jpg" | "jpeg" => ("JPEG Image", &["jpg", "jpeg"]),
        "webp" => ("WebP Image", &["webp"]),
        "gif" => ("GIF Image", &["gif"]),
        "avif" => ("AVIF Image", &["avif"]),
        "svg" => ("SVG Image", &["svg"]),
        "svgz" => ("Compressed SVG Image", &["svgz"]),
        "bmp" => ("BMP Image", &["bmp"]),
        "ico" => ("Icon", &["ico"]),
        "icns" => ("Apple Icon", &["icns"]),
        _ => ("Image", &["png"]),
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
type SaveSeam = Box<dyn FnMut(SaveSource, PathBuf)>;

#[cfg(all(test, not(target_arch = "wasm32")))]
thread_local! {
    static SAVE_SEAM: std::cell::RefCell<Option<SaveSeam>> = std::cell::RefCell::new(None);
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) struct SaveSeamGuard;

#[cfg(all(test, not(target_arch = "wasm32")))]
impl Drop for SaveSeamGuard {
    fn drop(&mut self) {
        SAVE_SEAM.with(|s| *s.borrow_mut() = None);
    }
}

/// Intercepts `Save Image As…` writes in tests so headless tests never touch the filesystem.
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn intercept_saves(f: impl FnMut(SaveSource, PathBuf) + 'static) -> SaveSeamGuard {
    SAVE_SEAM.with(|s| *s.borrow_mut() = Some(Box::new(f)));
    SaveSeamGuard
}

/// Writes original bytes or copies a regular source file verbatim off the UI thread.
#[cfg(not(target_arch = "wasm32"))]
fn write_saved_image(source: SaveSource, dest: PathBuf) {
    #[cfg(test)]
    {
        let handled = SAVE_SEAM.with(|s| {
            if let Some(hook) = s.borrow_mut().as_mut() {
                hook(source.clone(), dest.clone());
                true
            } else {
                false
            }
        });
        if handled {
            return;
        }
    }
    let dest_err = dest.clone();
    if let Err(err) = rune_core::task::spawn("rune-save-image", move || {
        if let Err(err) = write_saved_image_sync(&source, &dest) {
            eprintln!("rune: save image to {}: {err}", dest.display());
        }
    }) {
        eprintln!("rune: spawn save image for {}: {err}", dest_err.display());
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn write_saved_image_sync(source: &SaveSource, dest: &Path) -> std::io::Result<()> {
    use std::io::Read;
    match source {
        SaveSource::Bytes(bytes) => std::fs::write(dest, bytes),
        SaveSource::Path(src) => {
            let meta = std::fs::metadata(src)?;
            if !meta.file_type().is_file() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "source image path is not a regular file",
                ));
            }
            if meta.len() > MAX_SAVE_FILE_BYTES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "source image file exceeds 256 MiB limit",
                ));
            }
            let file = std::fs::File::open(src)?;
            let fmeta = file.metadata()?;
            if !fmeta.file_type().is_file() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "source image path is not a regular file",
                ));
            }
            let mut out = Vec::with_capacity(fmeta.len().min(MAX_SAVE_FILE_BYTES) as usize);
            file.take(MAX_SAVE_FILE_BYTES + 1).read_to_end(&mut out)?;
            if out.len() as u64 > MAX_SAVE_FILE_BYTES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "source image file exceeds 256 MiB limit",
                ));
            }
            std::fs::write(dest, &out)
        }
    }
}

impl RuntimeCore {
    /// Whether node `i` carries an opt-in [`El::image_menu`](crate::El::image_menu).
    #[inline]
    pub(crate) fn has_image_menu(&self, i: usize) -> bool {
        self.tree.nodes[i].vis.cold.get().is_some_and(|c| c.image_menu.is_some())
    }

    /// Nearest allowed link enclosing node `i` (an `El::href` on `i` or an ancestor with a valid
    /// openable URL), for linked images: `(url, on_press)`.
    fn enclosing_href_link(&self, i: usize) -> Option<(String, Option<crate::el::PressFn>)> {
        std::iter::successors(Some(i), |&j| self.tree.nodes[j].parent)
            .take_while(|&j| self.allowed(j) && !self.subtree_exiting(j))
            .find_map(|j| {
                let n = &self.tree.nodes[j];
                if self.node_disabled(j) { return None; }
                let raw = n.a11y.more.as_deref()?.href.as_deref()?;
                let url = crate::open_url::check(raw).ok()?;
                Some((url, n.handlers.on_press.clone()))
            })
    }

    /// Opens the opt-in image context menu for node `path` at `(x, y)`, composing with any
    /// enclosing link (in browser order: link items, separator, image items) and selectable text.
    /// Returns `false` when node `path` has an explicit `on_secondary_press` or no `image_menu`.
    pub(in crate::runtime) fn open_image_menu_at(
        &mut self,
        app: &dyn ViewDriver,
        path: &str,
        x: f32,
        y: f32,
    ) -> bool {
        let Some(i) = self.live(path) else { return false };
        if self.node_disabled(i) { return false; }
        let n = &self.tree.nodes[i];
        if n.handlers.on_secondary_press.is_some() { return false; }
        let Some(img_menu) = n.vis.cold.get().and_then(|c| c.image_menu.clone()) else {
            return false;
        };
        let link = self.enclosing_href_link(i);
        let selectable = self.is_selectable(i);
        let text = selectable.then(|| self.copy_text(app)).flatten();
        let html = text.as_ref().and_then(|_| self.copy_html());
        let can_save = !cfg!(target_arch = "wasm32");
        let img_target = img_menu.target(can_save);
        let Some(menu) = build_context_menu(ContextMenuTarget {
            has_link: link.is_some(),
            has_on_link: link.as_ref().is_some_and(|(_, p)| p.is_some()),
            image: Some(img_target),
            selectable,
            copy_enabled: text.is_some(),
        }) else { return false };
        let address = img_menu.valid_address().map(str::to_owned);
        let image = img_menu.image.clone();
        #[cfg(not(target_arch = "wasm32"))]
        let save_info = img_menu.save.clone().map(|source| {
            let (name, ext) = suggested_file_name(
                img_menu.file_name.as_deref(),
                Some(&source),
                address.as_deref(),
            );
            (source, name, ext)
        });
        log_event!(self, "image menu {path}");
        self.host.popups.push_back(crate::popup_menu::Request {
            menu,
            anchor: crate::PopupAnchor::Point(x, y),
            on_pick: Box::new(move |states, picked, tx| match picked {
                Some("open-link") => {
                    if let Some((_, Some(press))) = link {
                        press(states, tx);
                    }
                }
                Some("copy-link") => {
                    if let Some((url, _)) = link {
                        tx.copy(url);
                    }
                }
                Some("copy-image") => {
                    tx.copy_image_any(&image, |res, tx| {
                        if let Err(err) = res && !matches!(err, crate::CopyImageError::Superseded) {
                            eprintln!("rune: copy image: {err}");
                            tx.announce(format!("Couldn't copy image: {err}"), crate::LiveRegion::Assertive);
                        }
                    });
                }
                Some("copy-image-address") => {
                    if let Some(url) = address {
                        tx.copy(url);
                    }
                }
                #[cfg(not(target_arch = "wasm32"))]
                Some("save-image-as") => {
                    if let Some((source, name, ext)) = save_info {
                        let (filter_name, exts) = save_filter(ext);
                        let options = crate::FileDialogOptions {
                            title: crate::labels::label(crate::Label::SaveImageAs),
                            file_name: Some(name),
                            ..Default::default()
                        }.filter(filter_name, exts);
                        tx.save_file_any(options, move |result, tx| match result {
                            Ok(Some(dest)) => write_saved_image(source, dest),
                            Ok(None) => {}
                            Err(err) => {
                                eprintln!("rune: save image dialog: {err}");
                                tx.announce(format!("Couldn't save image: {err}"), crate::LiveRegion::Assertive);
                            }
                        });
                    }
                }
                Some("copy") => {
                    if let Some(text) = text {
                        match html {
                            Some(html) => tx.copy_rich(crate::ClipboardContent::new(text).with_html(html)),
                            None => tx.copy(text),
                        }
                    }
                }
                _ => {}
            }),
            field: None,
        });
        true
    }
}

#[cfg(test)]
#[path = "image_menu_tests.rs"]
mod tests;
