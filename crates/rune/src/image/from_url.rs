//! `ImageCache::url`: an image by URL. On web the browser fetches it and decodes it with its
//! own codecs (`createImageBitmap`, so WebP and AVIF too) off the main thread, and the straight
//! RGBA lands through the same mip path as `ImageCache::rgba`. On native a URL without a scheme
//! is a path and a `file:` URL names one; both load like `ImageCache::file`.
use super::{Decoded, Image, ImageCache, ImageError, Input, icon_pixels, mip_chain, svg_rgba};

/// The largest side a fetched image may have, as for decoded files: 8192 pixels, so at most
/// 256 MiB of decoded RGBA before mips.
#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
pub(super) const MAX_SIDE: u32 = 8192;

impl ImageCache {
    /// The image at `url`, shared by URL, size and scale like [`ImageCache::file`] (SVG
    /// rasterizes to fit `size_pt` at `scale`; other formats ignore both). Pending until it
    /// arrives, then the cache's waker redraws.
    ///
    /// On web the browser fetches `url`, relative to the page, and decodes it off the main
    /// thread: PNG, JPEG, GIF's first frame, WebP and AVIF, whatever the browser reads, turned
    /// upright by EXIF orientation. `image/svg+xml` and `.svg`/`.svgz` URLs rasterize with
    /// Rune's SVG renderer. Images over 8192 pixels a side, failed requests and cross-origin
    /// images without CORS resolve to [`Image::error`]. On native, where Rune has no HTTP
    /// client, a path or `file:` URL loads like [`ImageCache::file`] and other URLs fail; fetch
    /// those bytes yourself and call [`ImageCache::load`].
    pub fn url(&self, url: &str, size_pt: f32, scale: f32) -> Image {
        let px = icon_pixels(size_pt, scale);
        let mut hash = blake3::Hasher::new();
        hash.update(b"rune-url\0");
        hash.update(url.as_bytes());
        hash.update(&[0]);
        hash.update(&px.to_le_bytes());
        self.request(*hash.finalize().as_bytes(), Input::Url(url.to_owned(), px))
    }
}

/// What the browser handed back for a URL.
#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
pub(super) enum Fetched {
    /// Straight-alpha sRGB RGBA8 the browser decoded.
    Rgba(u32, u32, Vec<u8>),
    /// SVG or SVGZ bytes, which Rune rasterizes at `px`.
    Svg(Vec<u8>),
}

/// The mips for a fetched image, or why not, naming the URL.
#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
pub(super) fn land(url: &str, fetched: Fetched, px: u32) -> Result<Decoded, ImageError> {
    match fetched {
        Fetched::Rgba(w, h, bytes) => mip_chain(w, h, &bytes),
        Fetched::Svg(bytes) => svg_rgba(&bytes, px).and_then(|(w, h, bytes)| mip_chain(w, h, &bytes)),
    }
    .map_err(|e| ImageError(format!("{url}: {e}")))
}

/// Fetch and decode `url` in the browser.
#[cfg(target_arch = "wasm32")]
pub(super) async fn fetch(url: &str, px: u32) -> Result<Decoded, ImageError> {
    use wasm_bindgen::JsCast;
    let reply = wasm_bindgen_futures::JsFuture::from(crate::web::platform::fetch_image(url, MAX_SIDE)).await.map_err(|e| {
        let why = e.dyn_ref::<js_sys::Error>().map_or_else(|| format!("{e:?}"), |e| String::from(e.message()));
        ImageError(format!("{url}: {why}"))
    })?;
    let reply: js_sys::Array = reply.unchecked_into();
    let side = |i| reply.get(i).as_f64().unwrap_or(0.0) as u32;
    let bytes = reply.get(2).unchecked_into::<js_sys::Uint8Array>().to_vec();
    let fetched = if side(0) == 0 { Fetched::Svg(bytes) } else { Fetched::Rgba(side(0), side(1), bytes) };
    land(url, fetched, px)
}

/// The file a native `url` names: a path with no scheme, or a `file:` URL.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(super) fn file_path(url: &str) -> Result<std::path::PathBuf, ImageError> {
    let scheme = url.split_once(':').map(|(s, _)| s)
        // A one-letter "scheme" is a Windows drive.
        .filter(|s| s.len() > 1 && s.starts_with(|c: char| c.is_ascii_alphabetic())
            && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')));
    let Some(scheme) = scheme else { return Ok(url.into()) };
    let unsupported = || ImageError(format!("{url}: only web fetches URLs; on native fetch the bytes and call ImageCache::load"));
    if !scheme.eq_ignore_ascii_case("file") { return Err(unsupported()); }
    let rest = &url[scheme.len() + 1..];
    let path = match rest.strip_prefix("//") {
        Some(rest) => rest.strip_prefix("localhost").unwrap_or(rest),
        None => rest,
    };
    if !path.starts_with('/') { return Err(unsupported()); }
    let path = percent_decode(path).ok_or_else(|| ImageError(format!("{url}: not a valid file URL")))?;
    // `file:///C:/x` names `C:/x` on Windows.
    #[cfg(windows)]
    let path = match path.as_bytes() { [b'/', d, b':', ..] if d.is_ascii_alphabetic() => path[1..].to_owned(), _ => path };
    Ok(path.into())
}

fn percent_decode(s: &str) -> Option<String> {
    let mut out = Vec::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b != b'%' { out.push(b); continue; }
        let digit = |b: Option<u8>| (b? as char).to_digit(16);
        out.push((digit(bytes.next())? * 16 + digit(bytes.next())?) as u8);
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
#[path = "from_url_tests.rs"]
mod tests;
