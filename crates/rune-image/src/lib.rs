//! Content-addressed image decoding, linear-light mip generation, dynamic pixel buffers
//! (`pixels`), SVG and AVIF codecs, OS app icon rasterization and surface placement math. Decode
//! and mip generation run off the UI thread on shared decoder workers, never in a view or a render
//! frame. Apps normally reach this crate through `rune` (`rune::image`, `rune::Pixels`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};
#[cfg(all(test, not(target_arch = "wasm32")))]
use std::sync::mpsc;

pub(crate) mod animated;
mod app_icon;
mod avif;
mod from_url;
#[cfg(test)]
mod mip_tests;
mod mips;
#[doc(hidden)]
pub mod pixels;
#[doc(hidden)]
pub mod place;
#[cfg(test)]
mod tests;

pub use app_icon::app_icon_rgba;
#[cfg(not(target_arch = "wasm32"))]
#[doc(hidden)]
pub use from_url::font_file;
pub use pixels::Pixels;
pub use place::Align;
pub use place::SurfaceFit;

#[cfg(test)]
use mips::byte;
use mips::mip_chain;

pub type ImageId = [u8; 32];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageError(pub String);

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) }
}
impl std::error::Error for ImageError {}

/// Pixels are sRGB-encoded *linear-premultiplied* RGB plus linear alpha. Sampling with
/// Rgba8UnormSrgb yields linear-premultiplied RGBA, including between mip levels.
pub(crate) struct Mip {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}
pub(crate) struct Decoded {
    /// The image, or an animation's first frame.
    pub mips: Vec<Mip>,
    /// An animation's other frames and timing.
    pub motion: Option<Box<animated::Motion>>,
}
struct State {
    id: ImageId,
    pixels: Pixels,
    decoded: OnceLock<Result<Arc<Decoded>, ImageError>>,
}

/// A cheap shared handle. Pending images paint nothing until their worker wakes the app.
#[derive(Clone)]
pub struct Image(Arc<State>, Pixels);
impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Image").field("id", &self.id()).field("size", &self.size()).finish()
    }
}
impl Image {
    /// A handle on `state`, whose pixels keep it in the cache while drawn.
    fn new(state: Arc<State>) -> Image {
        let pixels = state.pixels.clone().keeping(state.clone());
        Image(state, pixels)
    }

    /// The shared static pixel handle, ready to pass to `El::pixels` after decoding. While
    /// any clone of it lives (an element drawing it), the cache hands back this image.
    pub fn pixels(&self) -> &Pixels { &self.1 }
    pub fn id(&self) -> ImageId { self.0.id }
    pub fn size(&self) -> Option<[u32; 2]> {
        let mip = self.decoded()?.ok()?.mips.first()?;
        Some([mip.width, mip.height])
    }
    pub fn is_ready(&self) -> bool { self.0.decoded.get().is_some() }
    pub fn error(&self) -> Option<&ImageError> { self.0.decoded.get()?.as_ref().err() }
    /// Wait for fixture assets before advancing a deterministic headless scene. Interactive
    /// apps use the cache's waker instead; decode still happens on the cache worker.
    pub fn wait(&self) -> Result<[u32; 2], &ImageError> {
        #[cfg(target_arch = "wasm32")]
        let decoded = self.0.decoded.get().expect("Image::wait cannot block on web; use is_ready and the cache waker");
        #[cfg(not(target_arch = "wasm32"))]
        let decoded = self.0.decoded.wait();
        let mip = &decoded.as_ref()?.mips[0];
        Ok([mip.width, mip.height])
    }
    pub(crate) fn decoded(&self) -> Option<Result<&Decoded, &ImageError>> {
        self.0.decoded.get().map(|r| r.as_ref().map(Arc::as_ref))
    }
    // The handle is ready before its pixels wake any window, so a woken frame sees its size.
    fn finish(&self, decoded: Result<Decoded, ImageError>) {
        let decoded = decoded.map(Arc::new);
        let mips = decoded.as_ref().ok().cloned();
        self.0.decoded.set(decoded).ok();
        if let Some(mips) = mips { self.0.pixels.set_mips(mips); }
    }
}

enum Input {
    Encoded(Arc<[u8]>),
    Rgba(u32, u32, Arc<[u8]>),
    Icon(#[cfg_attr(target_arch = "wasm32", allow(dead_code))] std::path::PathBuf, #[cfg_attr(target_arch = "wasm32", allow(dead_code))] u32),
    Svg(Arc<[u8]>, u32),
    #[cfg(not(target_arch = "wasm32"))]
    File(std::path::PathBuf, u32),
    /// `ImageCache::url`, and `ImageCache::file` on web.
    Url(String, u32),
    /// A decoder that panics, standing in for a codec bug.
    #[cfg(test)]
    Panic,
    #[cfg(all(test, not(target_arch = "wasm32")))]
    Gated(Box<Input>, mpsc::Receiver<()>),
}

impl Input {
    fn decode(self) -> Result<Decoded, ImageError> {
        match self {
            Input::Encoded(bytes) => decode(&bytes),
            Input::Rgba(w, h, bytes) => mip_chain(w, h, &bytes),
            Input::Svg(bytes, px) => svg_rgba(&bytes, px).and_then(|(w, h, bytes)| mip_chain(w, h, &bytes)),
            #[cfg(not(target_arch = "wasm32"))]
            Input::Icon(path, px) => app_icon_rgba(&path, px).and_then(|(w, h, bytes)| mip_chain(w, h, &bytes)),
            #[cfg(not(target_arch = "wasm32"))]
            Input::File(path, px) => {
                let bytes = std::fs::read(&path).map_err(|e| ImageError(format!("{}: {e}", path.display())))?;
                if has_svg_extension(&path) || is_svg(&bytes) {
                    svg_rgba(&bytes, px).and_then(|(w, h, bytes)| mip_chain(w, h, &bytes))
                } else {
                    decode(&bytes)
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            Input::Url(url, px) => Input::File(from_url::file_path(&url)?, px).decode(),
            #[cfg(target_arch = "wasm32")]
            Input::Icon(..) => Err(ImageError("filesystem images are unsupported on web".into())),
            // Fetched asynchronously by `ImageCache::request`.
            #[cfg(target_arch = "wasm32")]
            Input::Url(url, _) => Err(ImageError(format!("{url}: fetched, not decoded"))),
            #[cfg(test)]
            Input::Panic => panic!("decoder bug"),
            #[cfg(all(test, not(target_arch = "wasm32")))]
            Input::Gated(input, release) => {
                release.recv().map_err(|_| ImageError("test gate dropped".into()))?;
                input.decode()
            }
        }
    }

    /// `decode`, with a panic in a codec (a malformed file can still trip one) failing this
    /// image instead of unwinding the worker, which would leave it pending and fail every later one.
    #[cfg(not(target_arch = "wasm32"))]
    fn decode_caught(self) -> Result<Decoded, ImageError> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.decode())).unwrap_or_else(|panic| {
            let why = panic.downcast_ref::<&str>().copied().or_else(|| panic.downcast_ref::<String>().map(String::as_str));
            Err(ImageError(format!("image decoder panicked: {}", why.unwrap_or("no message"))))
        })
    }
}
#[cfg(not(target_arch = "wasm32"))]
struct Job { image: Image, input: Input, waker: rune_core::Waker }
/// Decoder threads for the process, at most one per core.
#[cfg(not(target_arch = "wasm32"))]
const DECODERS: usize = 4;
/// Straight RGBA images up to this many pixels (64 x 64) finish on the calling thread.
#[cfg(not(target_arch = "wasm32"))]
const INLINE_RGBA_PIXELS: u64 = 64 * 64;

/// The images waiting to decode, shared by every cache, and how many decoders are draining them.
/// A decoder is a task on Rune's shared threads (`task::spawn`) that decodes until none is left:
/// a session opening on several screenshots decodes them side by side, and with nothing to
/// decode no thread stays for it (each kept decoder thread held its stack and its allocator
/// pages for the process's life). Task threads reserve the stack `MAX_SVG_DEPTH` needs
/// (`rune_core::task`'s `STACK`).
#[cfg(not(target_arch = "wasm32"))]
static DECODING: Mutex<(std::collections::VecDeque<Job>, usize)> = Mutex::new((std::collections::VecDeque::new(), 0));

/// Queues `job`, and starts a decoder for it while fewer than one per core (at most
/// [`DECODERS`]) run. False when no decoder runs and none could start.
#[cfg(not(target_arch = "wasm32"))]
fn decode_later(job: Job) -> bool {
    let mut d = DECODING.lock().unwrap_or_else(|e| e.into_inner());
    d.0.push_back(job);
    let most = DECODERS.min(std::thread::available_parallelism().map_or(1, |n| n.get()));
    if d.1 >= most {
        return true;
    }
    d.1 += 1;
    drop(d);
    if rune_core::task::spawn("rune-images", decode_jobs).is_ok() {
        return true;
    }
    let mut d = DECODING.lock().unwrap_or_else(|e| e.into_inner());
    d.1 -= 1;
    if d.1 > 0 {
        return true;
    }
    d.0.pop_back();
    false
}

#[cfg(not(target_arch = "wasm32"))]
fn decode_jobs() {
    loop {
        let job = {
            let mut d = DECODING.lock().unwrap_or_else(|e| e.into_inner());
            let Some(job) = d.0.pop_front() else {
                d.1 -= 1;
                return;
            };
            job
        };
        job.image.finish(job.input.decode_caught());
        (job.waker)();
    }
}

/// Construct once during app setup. The cache deduplicates by content, not a path or pointer;
/// dropping all handles releases decoded pixels. Its images decode on the process's decoder
/// threads, which every cache shares. The waker should be the one supplied by `AppSpec::setup`.
pub struct ImageCache {
    entries: Mutex<HashMap<ImageId, Weak<State>>>,
    #[cfg(not(target_arch = "wasm32"))]
    waker: rune_core::Waker,
    #[cfg(target_arch = "wasm32")]
    wake: rune_core::Waker,
}
impl ImageCache {
    #[cfg(target_arch = "wasm32")]
    pub fn new(wake: rune_core::Waker) -> Self {
        Self { entries: Mutex::new(HashMap::new()), wake }
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(waker: rune_core::Waker) -> Self {
        Self { entries: Mutex::new(HashMap::new()), waker }
    }

    /// PNG (APNG animates), JPEG, GIF and WebP (both animate), BMP, ICO, AVIF (with the `avif`
    /// feature; its first frame), modern ICNS containing a PNG representation, or SVG (plain or
    /// gzipped SVGZ, recognised by its content). An SVG rasterizes at twice its own size; use
    /// [`ImageCache::svg`] to rasterize it at the size it draws at. Legacy ICNS codecs return an
    /// explicit error; callers can provide platform-decoded RGBA instead. An animation decodes
    /// every frame on the worker, and plays only while an element draws it in a shown window.
    /// On web the browser decodes everything but PNG, SVG and ICNS, so GIF and WebP show their
    /// first frame there.
    pub fn load(&self, bytes: impl Into<Arc<[u8]>>) -> Image {
        let bytes = bytes.into();
        let mut hash = blake3::Hasher::new();
        hash.update(b"rune-encoded\0");
        hash.update(&bytes);
        let id = *hash.finalize().as_bytes();
        self.request(id, Input::Encoded(bytes))
    }

    /// SVG or SVGZ bytes rasterized to fit `size_pt` points square at `scale` device pixels per
    /// point (pass the window's `Env::scale`), keeping the drawing's aspect ratio.
    /// Rasterize at the largest size the image draws at: smaller draws use its mips, so 16 pt at
    /// 2× stays crisp. Handles are shared by content, size and scale.
    pub fn svg(&self, bytes: impl Into<Arc<[u8]>>, size_pt: f32, scale: f32) -> Image {
        let bytes = bytes.into();
        let px = icon_pixels(size_pt, scale);
        let mut hash = blake3::Hasher::new();
        hash.update(b"rune-svg\0");
        hash.update(&px.to_le_bytes());
        hash.update(&bytes);
        self.request(*hash.finalize().as_bytes(), Input::Svg(bytes, px))
    }

    /// Bytes with a MIME type, as from a data URL or a `Content-Type` header: `image/svg+xml`
    /// rasterizes like [`ImageCache::svg`], anything else decodes like [`ImageCache::load`].
    pub fn load_mime(&self, bytes: impl Into<Arc<[u8]>>, mime: &str, size_pt: f32, scale: f32) -> Image {
        let essence = mime.split(';').next().unwrap_or("").trim();
        if essence.eq_ignore_ascii_case("image/svg+xml") { self.svg(bytes, size_pt, scale) } else { self.load(bytes) }
    }

    /// An image file, read and decoded on the worker. `.svg` and `.svgz` files (or SVG content)
    /// rasterize like [`ImageCache::svg`]; other formats decode like [`ImageCache::load`], ignoring
    /// the size. Handles are shared by path, size and scale, so an edited file needs a new handle
    /// once the old ones drop. On web the path is a URL relative to the page, fetched as
    /// [`ImageCache::url`] does, so `cache.file("assets/logo.webp", ..)` works in both builds.
    pub fn file(&self, path: impl AsRef<std::path::Path>, size_pt: f32, scale: f32) -> Image {
        let path = path.as_ref().to_path_buf();
        let px = icon_pixels(size_pt, scale);
        let mut hash = blake3::Hasher::new();
        hash.update(b"rune-file\0");
        hash.update(path.as_os_str().as_encoded_bytes());
        hash.update(&[0]);
        hash.update(&px.to_le_bytes());
        #[cfg(target_arch = "wasm32")]
        let input = Input::Url(path.to_string_lossy().into_owned(), px);
        #[cfg(not(target_arch = "wasm32"))]
        let input = Input::File(path, px);
        self.request(*hash.finalize().as_bytes(), input)
    }

    /// Straight-alpha sRGB RGBA8. Validation, premultiplication and mips happen off-thread.
    pub fn rgba(&self, width: u32, height: u32, bytes: impl Into<Arc<[u8]>>) -> Image {
        let bytes = bytes.into();
        let mut hash = blake3::Hasher::new();
        hash.update(b"rune-rgba8\0");
        hash.update(&width.to_le_bytes());
        hash.update(&height.to_le_bytes());
        hash.update(&bytes);
        self.request(*hash.finalize().as_bytes(), Input::Rgba(width, height, bytes))
    }

    /// The Finder icon for an app bundle or any file path, rasterized at 2× `size_pt`.
    /// See [`ImageCache::app_icon_scaled`].
    pub fn app_icon(&self, path: impl AsRef<std::path::Path>, size_pt: f32) -> Image {
        self.app_icon_scaled(path, size_pt, 2.0)
    }

    /// The Finder icon for `path`, rasterized at `size_pt * scale` pixels square (pass the
    /// window's display scale). Handles are shared by (path, size, scale) rather than by
    /// content, so repeated calls never re-rasterize while a handle is alive. Lookup and
    /// rasterization run on the cache worker like any decode, so this is safe to call from a view.
    /// Missing paths still get the system's generic document icon on macOS. Windows reads an
    /// executable's or icon file's first icon; Linux reads a `.desktop` entry's `Icon=` from the
    /// hicolor theme or pixmaps (PNG or SVG), or an image file itself. Paths without an icon
    /// resolve to an error, so callers can draw their own fallback.
    pub fn app_icon_scaled(&self, path: impl AsRef<std::path::Path>, size_pt: f32, scale: f32) -> Image {
        let path = path.as_ref().to_path_buf();
        let px = icon_pixels(size_pt, scale);
        let mut hash = blake3::Hasher::new();
        hash.update(b"rune-app-icon\0");
        hash.update(path.as_os_str().as_encoded_bytes());
        hash.update(&[0]);
        hash.update(&size_pt.to_bits().to_le_bytes());
        hash.update(&scale.to_bits().to_le_bytes());
        self.request(*hash.finalize().as_bytes(), Input::Icon(path, px))
    }

    fn request(&self, id: ImageId, input: Input) -> Image {
        self.request_set(id, input, None)
    }

    fn request_set(&self, id: ImageId, input: Input, srcset: Option<Arc<str>>) -> Image {
        let mut entries = self.entries.lock().unwrap();
        if let Some(state) = entries.get(&id).and_then(Weak::upgrade) { return Image::new(state); }
        entries.retain(|_, value| value.strong_count() > 0);
        let url: Option<Arc<str>> = match &input {
            Input::Url(url, _) => Some(url.as_str().into()),
            #[cfg(not(target_arch = "wasm32"))]
            Input::File(path, _) => Some(path.to_string_lossy().into()),
            _ => None,
        };
        let image = Image::new(Arc::new(State { id, pixels: Pixels::from_url(url, srcset), decoded: OnceLock::new() }));
        entries.insert(id, Arc::downgrade(&image.0));
        // A small straight RGBA image (a 16 pt tile at 2x) is ready at once: its mips take
        // microseconds, less than handing it to a decoder, and the view that asked for it draws it
        // in the same frame rather than a placeholder first. It still wakes the app once, as a
        // decoded one does: a caller outside a view's build (an overlay's state, a poll) is
        // redrawn by that wake alone.
        #[cfg(not(target_arch = "wasm32"))]
        if let Input::Rgba(w, h, _) = &input && u64::from(*w) * u64::from(*h) <= INLINE_RGBA_PIXELS {
            image.finish(input.decode_caught());
            drop(entries);
            (self.waker)();
            return image;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if !decode_later(Job { image: image.clone(), input, waker: self.waker.clone() }) {
            image.0.decoded.set(Err(ImageError("image worker stopped".into()))).ok();
        }
        #[cfg(target_arch = "wasm32")]
        {
            // The task holds the image weakly, so a request dropped before it runs (a view's
            // guess, replaced in the same frame once layout knows the size) fetches nothing,
            // and one dropped while its bytes download is never decoded.
            let weak = Arc::downgrade(&image.0);
            let wake = self.wake.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let decoded = match input {
                    Input::Url(url, px) => {
                        if !wanted(&weak) { return; }
                        let fetched = from_url::fetch(&url).await;
                        if !wanted(&weak) { return; }
                        match fetched { Ok(fetched) => from_url::land_in_slices(&url, fetched, px).await, Err(e) => Err(e) }
                    }
                    Input::Encoded(bytes) if browser_decodes(&bytes) => {
                        let decoded = from_url::decode_in_browser(&bytes).await;
                        if !wanted(&weak) { return; }
                        match decoded { Ok(fetched) => from_url::land_in_slices("image", fetched, 0).await, Err(e) => Err(e) }
                    }
                    input => input.decode(),
                };
                let Some(state) = weak.upgrade() else { return };
                Image::new(state).finish(decoded);
                wake();
            });
        }
        image
    }
}

/// Encoded bytes a web build hands to the browser's codecs: everything but PNG (decoded exactly,
/// alpha and all, by Rune), SVG and ICNS.
#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
fn browser_decodes(bytes: &[u8]) -> bool {
    !bytes.starts_with(b"\x89PNG\r\n\x1a\n") && !bytes.starts_with(b"icns") && !is_svg(bytes)
}

/// Whether anything still holds the image a fetch is for.
#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
fn wanted(state: &std::sync::Weak<State>) -> bool { state.strong_count() > 0 }

fn icon_pixels(size_pt: f32, scale: f32) -> u32 {
    let px = (size_pt * scale).ceil();
    if px.is_finite() { px.clamp(1.0, 1024.0) as u32 } else { 1 }
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn has_svg_extension(path: &std::path::Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("svg") || e.eq_ignore_ascii_case("svgz"))
}

/// Gzip (SVGZ), or markup after an optional BOM and whitespace. No raster format starts with `<`.
fn is_svg(bytes: &[u8]) -> bool {
    if bytes.starts_with(&[0x1f, 0x8b]) { return true; }
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    bytes.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'<')
}

/// Options for `usvg` that resolve nothing outside the SVG's own bytes: non-`data:` `<image>` and
/// `<feImage>` `href`s never touch the filesystem or UNC paths, and `data:` URLs accept raster
/// formats only (`image/svg+xml` and `text/plain` SVG fall-through are refused).
///
/// Rune builds `resvg` with `raster-images` off, so those raster arms are resolved in the tree but
/// never drawn today; they—and raster decoder pixel/memory bounds—become live if `raster-images` is
/// ever enabled (for example via feature unification).
#[cfg(feature = "svg")]
fn svg_options() -> resvg::usvg::Options<'static> {
    use resvg::usvg::{ImageHrefResolver, ImageKind, Options};
    let raster = |data: Arc<Vec<u8>>| {
        if data.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(ImageKind::PNG(data))
        } else if data.starts_with(&[0xff, 0xd8, 0xff]) {
            Some(ImageKind::JPEG(data))
        } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
            Some(ImageKind::GIF(data))
        } else if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
            Some(ImageKind::WEBP(data))
        } else {
            None
        }
    };
    Options {
        image_href_resolver: ImageHrefResolver {
            resolve_string: Box::new(|_, _| None),
            resolve_data: Box::new(move |mime, data, _| match mime {
                "image/png" => Some(ImageKind::PNG(data)),
                "image/jpg" | "image/jpeg" => Some(ImageKind::JPEG(data)),
                "image/gif" => Some(ImageKind::GIF(data)),
                "image/webp" => Some(ImageKind::WEBP(data)),
                "text/plain" => raster(data),
                _ => None,
            }),
        },
        ..Options::default()
    }
}

/// Maximum nested XML container elements inside an SVG's root element (513 open elements in all,
/// matching Blink's 512-element HTML/SVG tree bound; Chromium's standalone XML limit of 5,000
/// exceeds both `usvg`'s 1,024 node cap and a worker stack at ~4.5 KiB per `<g>`).
#[cfg(feature = "svg")]
const MAX_SVG_DEPTH: usize = 512;

#[cfg(feature = "svg")]
fn svg_tree(bytes: &[u8]) -> Result<resvg::usvg::Tree, ImageError> {
    use resvg::usvg;
    const MAX_BYTES: usize = 32 << 20;
    if bytes.len() > MAX_BYTES { return Err(ImageError("SVG is too large".into())); }
    let unzipped;
    let raw = if bytes.starts_with(&[0x1f, 0x8b]) {
        unzipped = usvg::decompress_svgz(bytes).map_err(|e| ImageError(format!("SVG: {e}")))?;
        if unzipped.len() > MAX_BYTES { return Err(ImageError("SVG is too large".into())); }
        unzipped.as_slice()
    } else {
        bytes
    };
    let text = std::str::from_utf8(raw).map_err(|_| ImageError("SVG is not UTF-8".into()))?;
    if svg_exceeds_depth(text, MAX_SVG_DEPTH) {
        return Err(ImageError(format!("SVG exceeds maximum nesting depth of {MAX_SVG_DEPTH}")));
    }
    usvg::Tree::from_str(text, &svg_options()).map_err(|e| ImageError(format!("SVG: {e}")))
}

/// Iterative byte scan for open XML element depth, run before `roxmltree` so deeply nested input
/// cannot overflow the decoder stack inside the XML parser or `usvg`/`resvg`.
#[cfg(feature = "svg")]
fn svg_exceeds_depth(text: &str, max_nested: usize) -> bool {
    let b = text.as_bytes();
    let limit = max_nested + 1;
    let (mut i, mut depth) = (0usize, 0usize);
    while i < b.len() {
        let Some(rel) = b[i..].iter().position(|&c| c == b'<') else { return false };
        i += rel;
        if b[i..].starts_with(b"<!--") {
            i += 4;
            let Some(end) = b[i..].windows(3).position(|w| w == b"-->") else { return false };
            i += end + 3;
        } else if b[i..].starts_with(b"<![CDATA[") {
            i += 9;
            let Some(end) = b[i..].windows(3).position(|w| w == b"]]>") else { return false };
            i += end + 3;
        } else if b[i..].starts_with(b"<?") {
            i += 2;
            let Some(end) = b[i..].windows(2).position(|w| w == b"?>") else { return false };
            i += end + 2;
        } else if b[i..].starts_with(b"<!") {
            i += 2;
            let (mut decl, mut quote) = (1usize, 0u8);
            while i < b.len() && decl > 0 {
                let c = b[i];
                if quote != 0 {
                    if c == quote { quote = 0; }
                } else if c == b'"' || c == b'\'' {
                    quote = c;
                } else if c == b'<' {
                    decl += 1;
                } else if c == b'>' {
                    decl -= 1;
                }
                i += 1;
            }
        } else if b[i..].starts_with(b"</") {
            i += 2;
            depth = depth.saturating_sub(1);
            let mut quote = 0u8;
            while i < b.len() {
                let c = b[i];
                i += 1;
                if quote != 0 {
                    if c == quote { quote = 0; }
                } else if c == b'"' || c == b'\'' {
                    quote = c;
                } else if c == b'>' {
                    break;
                }
            }
        } else {
            i += 1;
            let (mut quote, mut last_non_ws, mut closed) = (0u8, 0u8, false);
            while i < b.len() {
                let c = b[i];
                i += 1;
                if quote != 0 {
                    if c == quote { quote = 0; }
                } else if c == b'"' || c == b'\'' {
                    quote = c;
                    last_non_ws = c;
                } else if c == b'>' {
                    closed = true;
                    break;
                } else if !c.is_ascii_whitespace() {
                    last_non_ws = c;
                }
            }
            if !closed { return false; }
            if last_non_ws != b'/' {
                depth += 1;
                if depth > limit { return true; }
            }
        }
    }
    false
}

#[cfg(feature = "svg")]
fn demultiply_rgba(pixmap: &resvg::tiny_skia::Pixmap) -> Vec<u8> {
    pixmap.pixels().iter().flat_map(|p| {
        let c = p.demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    }).collect()
}

/// An SVG scaled to fit `px` square, keeping its aspect ratio, as straight-alpha sRGB RGBA8.
/// `px == 0` rasterizes at twice the drawing's own size, at most 4096 pixels per side.
#[cfg(feature = "svg")]
#[doc(hidden)]
pub fn svg_rgba(bytes: &[u8], px: u32) -> Result<(u32, u32, Vec<u8>), ImageError> {
    use resvg::tiny_skia;
    let tree = svg_tree(bytes)?;
    let size = tree.size();
    let px = if px == 0 { (size.width().max(size.height()) * 2.0).ceil().clamp(1.0, 4096.0) as u32 } else { px };
    let k = px as f32 / size.width().max(size.height());
    let (w, h) = (((size.width() * k).round() as u32).clamp(1, px), ((size.height() * k).round() as u32).clamp(1, px));
    let mut pixmap = tiny_skia::Pixmap::new(w, h).ok_or_else(|| ImageError("empty SVG".into()))?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(k, k), &mut pixmap.as_mut());
    Ok((w, h, demultiply_rgba(&pixmap)))
}

#[cfg(not(feature = "svg"))]
#[doc(hidden)]
pub fn svg_rgba(_: &[u8], _: u32) -> Result<(u32, u32, Vec<u8>), ImageError> {
    Err(ImageError("SVG images need rune's `svg` feature".into()))
}

/// An SVG centre-cropped to fill `px` square (`object-fit: cover`), as straight-alpha sRGB RGBA8,
/// decoded through the same sandboxed `usvg` options as [`ImageCache`].
#[cfg(feature = "svg")]
pub fn svg_rgba_cover(bytes: &[u8], px: u32) -> Result<Vec<u8>, ImageError> {
    use resvg::tiny_skia;
    if px == 0 || px > 4096 { return Err(ImageError("invalid SVG raster size".into())); }
    let tree = svg_tree(bytes)?;
    let size = tree.size();
    let k = px as f32 / size.width().min(size.height());
    let t = tiny_skia::Transform::from_row(k, 0.0, 0.0, k, (px as f32 - size.width() * k) / 2.0, (px as f32 - size.height() * k) / 2.0);
    let mut pixmap = tiny_skia::Pixmap::new(px, px).ok_or_else(|| ImageError("empty SVG".into()))?;
    resvg::render(&tree, t, &mut pixmap.as_mut());
    Ok(demultiply_rgba(&pixmap))
}

#[cfg(not(feature = "svg"))]
pub fn svg_rgba_cover(_: &[u8], _: u32) -> Result<Vec<u8>, ImageError> {
    Err(ImageError("SVG images need rune's `svg` feature".into()))
}

fn decode(bytes: &[u8]) -> Result<Decoded, ImageError> {
    if is_svg(bytes) {
        let (w, h, rgba) = svg_rgba(bytes, 0)?;
        return mip_chain(w, h, &rgba);
    }
    if avif::is_avif(bytes) {
        let (w, h, rgba) = avif::rgba(bytes)?;
        return mip_chain(w, h, &rgba);
    }
    let bytes = if bytes.starts_with(b"icns") { icns_png(bytes)? } else { bytes };
    if let Some(animation) = animated::decode(bytes) { return animation; }
    use image::ImageDecoder;
    let error = |e: image::ImageError| ImageError(e.to_string());
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format().map_err(|e| ImageError(e.to_string()))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits.clone());
    // `ImageReader::decode`, keeping the decoder to read its EXIF orientation (a phone's JPEG
    // stores its pixels as the sensor saw them and says how to turn them upright).
    let mut decoder = reader.into_decoder().map_err(error)?;
    limits.reserve(decoder.total_bytes()).map_err(error)?;
    decoder.set_limits(limits).map_err(error)?;
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image = image::DynamicImage::from_decoder(decoder).map_err(error)?;
    image.apply_orientation(orientation);
    let image = image.into_rgba8();
    mip_chain(image.width(), image.height(), image.as_raw())
}

/// A PNG decoded to 8-bit RGBA by the PNG decoder alone. `image::load_from_memory_with_format`
/// dispatches on the format at run time, which links every codec the crate is built with (JPEG,
/// WebP, GIF, BMP, ICO) into an app that only ever reads PNGs from the clipboard or a cursor.
/// The same decoder and limits as that call's PNG arm.
#[doc(hidden)]
pub fn png_rgba8(bytes: &[u8]) -> image::ImageResult<image::RgbaImage> {
    let decoder = image::codecs::png::PngDecoder::with_limits(std::io::Cursor::new(bytes), image::Limits::default())?;
    Ok(image::DynamicImage::from_decoder(decoder)?.into_rgba8())
}

fn icns_png(bytes: &[u8]) -> Result<&[u8], ImageError> {
    let invalid = || ImageError("invalid ICNS container or no supported PNG representation".into());
    let len = u32::from_be_bytes(bytes.get(4..8).ok_or_else(invalid)?.try_into().unwrap()) as usize;
    if len != bytes.len() { return Err(invalid()); }
    let mut pos = 8;
    let mut best = None;
    let mut area = 0u64;
    while pos < len {
        let header = bytes.get(pos..pos + 8).ok_or_else(invalid)?;
        let size = u32::from_be_bytes(header[4..8].try_into().unwrap()) as usize;
        if size < 8 { return Err(invalid()); }
        let end = pos.checked_add(size).filter(|end| *end <= len).ok_or_else(invalid)?;
        let chunk = &bytes[pos + 8..end];
        if chunk.starts_with(b"\x89PNG\r\n\x1a\n") && chunk.len() >= 24 {
            let w = u32::from_be_bytes(chunk[16..20].try_into().unwrap());
            let h = u32::from_be_bytes(chunk[20..24].try_into().unwrap());
            let candidate = u64::from(w) * u64::from(h);
            if w <= 8192 && h <= 8192 && candidate > area { best = Some(chunk); area = candidate; }
        }
        pos = end;
    }
    best.ok_or_else(invalid)
}
