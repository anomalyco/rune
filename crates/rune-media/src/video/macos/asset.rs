//! AVFoundation asset opening, metadata, foreign audio decoding (`ogg-audio`), `CMTime` helpers
//! and still-frame decoding through `AVAssetImageGenerator`.

#![allow(unsafe_code)]

use super::{Decoded, Native};
use crate::native_surface::{ByteFrame, ColorInfo, Format, Orient};
#[cfg(feature = "ogg-audio")]
use crate::video::foreign;
use crate::video::{Kind, MediaSource, Meta};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_av_foundation::{
    AVAsset, AVAssetImageGenerator, AVAssetReferenceRestrictions, AVMediaTypeAudio, AVMediaTypeVideo, AVURLAsset,
    AVURLAssetReferenceRestrictionsKey, AVURLAssetShouldSupportAliasDataReferencesKey,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo, kCGColorSpaceSRGB};
use objc2_core_media::{CMTime, CMTimeFlags, kCMTimePositiveInfinity};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};

/// The source's lowercase extension, if any.
pub(in crate::video) fn extension(source: &MediaSource) -> Option<String> {
    let name = match source {
        MediaSource::File(path) => path.file_name()?.to_string_lossy().into_owned(),
        MediaSource::Url(url) => {
            let path = url.split(['?', '#']).next().unwrap_or_default();
            // The last segment of the path, never the host.
            let path = path.split_once("://").map_or(path, |(_, rest)| rest.split_once('/').map_or("", |(_, path)| path));
            path.rsplit('/').next().unwrap_or_default().to_string()
        }
    };
    Some(name.rsplit_once('.')?.1.to_ascii_lowercase()).filter(|ext| !ext.is_empty())
}

/// Audio in a container AVFoundation can't read, which `ogg-audio` decodes instead.
fn foreign_audio(source: &MediaSource, kind: Kind) -> bool {
    cfg!(feature = "ogg-audio")
        && kind == Kind::Audio
        && extension(source).is_some_and(|ext| matches!(ext.as_str(), "ogg" | "oga" | "opus" | "webm" | "mka" | "mkv"))
}

/// AVFoundation has no Ogg, WebM or Matroska demuxer; fail at once rather than wait on a load
/// that can only fail.
pub(super) fn unsupported_format(source: &MediaSource, kind: Kind) -> Option<String> {
    let ext = extension(source)?;
    (!foreign_audio(source, kind) && matches!(ext.as_str(), "ogg" | "oga" | "ogv" | "opus" | "spx" | "webm" | "mkv" | "mka"))
        .then(|| format!(".{ext} isn't supported on macOS (AVFoundation can't read it); use mp3, m4a, wav, mp4 or mov"))
}

fn url(source: &MediaSource) -> Result<Retained<NSURL>, String> {
    match source {
        MediaSource::File(path) => {
            let meta = std::fs::metadata(path).map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => format!("no such file: {}", path.display()),
                _ => format!("can't open {}: {e}", path.display()),
            })?;
            if !meta.is_file() {
                return Err(format!("not a file: {}", path.display()));
            }
            let path = std::path::absolute(path).unwrap_or_else(|_| path.clone());
            Ok(NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy())))
        }
        MediaSource::Url(url) => {
            let parsed = NSURL::URLWithString(&NSString::from_str(url)).ok_or_else(|| format!("invalid URL: {url}"))?;
            if parsed.isFileURL()
                && let Some(host) = parsed.host()
            {
                let host = host.to_string();
                if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
                    return Err(format!("unsupported file URL host: {url}"));
                }
            }
            Ok(parsed)
        }
    }
}

/// Opens a local `file:` URL with `ForbidAll` reference restrictions so only media data stored
/// inside the container file itself is allowed. Apple's default (`ForbidLocalReferenceToRemote`)
/// leaves `ForbidLocalReferenceToLocal` off, allowing external QuickTime `dref` references to
/// read other local files.
fn local_asset(url: &NSURL) -> Retained<AVURLAsset> {
    let restrictions = NSNumber::new_usize(AVAssetReferenceRestrictions::ForbidAll.0);
    let aliases = NSNumber::new_bool(false);
    let restrictions: &AnyObject = &restrictions;
    let aliases: &AnyObject = &aliases;
    // SAFETY: `AVURLAssetReferenceRestrictionsKey` (macOS 10.7+) and
    // `AVURLAssetShouldSupportAliasDataReferencesKey` (macOS 10.10+) are immutable static
    // AVFoundation string constants, and the values are live `NSNumber` objects.
    let options = unsafe {
        NSDictionary::<NSString, AnyObject>::from_slices(
            &[AVURLAssetReferenceRestrictionsKey, AVURLAssetShouldSupportAliasDataReferencesKey],
            &[restrictions, aliases],
        )
    };
    // SAFETY: `url` is a live `NSURL` and `options` is a live `NSDictionary` of AVURLAsset options.
    unsafe { AVURLAsset::URLAssetWithURL_options(url, Some(&options)) }
}

fn asset(source: &MediaSource, headers: &[(String, String)]) -> Result<Retained<AVURLAsset>, String> {
    let url = url(source)?;
    if matches!(source, MediaSource::File(_)) || url.isFileURL() {
        return Ok(local_asset(&url));
    }
    if headers.is_empty() {
        // SAFETY: `url` is a live `NSURL` and `URLAssetWithURL_options` accepts `None` for options.
        return Ok(unsafe { AVURLAsset::URLAssetWithURL_options(&url, None) });
    }
    let names: Vec<_> = headers.iter().map(|(name, _)| NSString::from_str(name)).collect();
    let values: Vec<_> = headers.iter().map(|(_, value)| NSString::from_str(value)).collect();
    let fields = NSDictionary::from_slices(&names.iter().map(|n| &**n).collect::<Vec<_>>(), &values.iter().map(|v| &**v).collect::<Vec<_>>());
    // Long-standing and widely used, though missing from AVFoundation's headers.
    let key = NSString::from_str("AVURLAssetHTTPHeaderFieldsKey");
    let fields: &AnyObject = &fields;
    let options = NSDictionary::<NSString, AnyObject>::from_slices(&[&*key], &[fields]);
    // SAFETY: `url` is a live `NSURL` and `options` is a live `NSDictionary` of HTTP header fields.
    Ok(unsafe { AVURLAsset::URLAssetWithURL_options(&url, Some(&options)) })
}

/// Opens a clip and reads its facts, blocking. Audio AVFoundation can't read plays from a WAV
/// decoded from it: up front for a foreign extension, or when its own asset has no audio track
/// and its bytes are Ogg or WebM.
pub(super) fn open(source: &MediaSource, headers: &[(String, String)], kind: Kind, decoded: &Decoded) -> Result<(Retained<AVURLAsset>, Meta), String> {
    let opened = |asset: Retained<AVURLAsset>| meta(&asset, kind).map(|meta| (asset, meta));
    #[cfg(feature = "ogg-audio")]
    {
        let wav = |wav: &foreign::Wav| opened(asset(&MediaSource::File(wav.path().into()), &[])?);
        if foreign_audio(source, kind) {
            return match decoded.get_or_init(|| decode_foreign(source, headers, false)) {
                Ok(Some(decoded)) => wav(decoded),
                Ok(None) => Err("no Ogg or WebM audio".into()),
                Err(e) => Err(e.clone()),
            };
        }
        let own = opened(asset(source, headers)?);
        if own.is_err() && kind == Kind::Audio {
            match decoded.get_or_init(|| decode_foreign(source, headers, true)) {
                Ok(Some(decoded)) => return wav(decoded),
                // Ogg or WebM that didn't decode: say why, not that AVFoundation found nothing.
                Err(e) => return Err(e.clone()),
                Ok(None) => {}
            }
        }
        own
    }
    #[cfg(not(feature = "ogg-audio"))]
    {
        let () = decoded;
        opened(asset(source, headers)?)
    }
}

/// The asset a player opens: the decoded WAV once there is one.
pub(super) fn player_asset(source: &MediaSource, headers: &[(String, String)], decoded: &Decoded) -> Result<Retained<AVURLAsset>, String> {
    #[cfg(feature = "ogg-audio")]
    if let Some(Ok(Some(wav))) = decoded.get() {
        return asset(&MediaSource::File(wav.path().into()), &[]);
    }
    #[cfg(not(feature = "ogg-audio"))]
    let () = decoded;
    asset(source, headers)
}

/// Fetches a clip and decodes it to a WAV. When sniffing, bytes that aren't Ogg or WebM give
/// `None`, and a fetch that fails gives `None` too, leaving AVFoundation's error to report.
#[cfg(feature = "ogg-audio")]
fn decode_foreign(source: &MediaSource, headers: &[(String, String)], sniff: bool) -> Result<Option<foreign::Wav>, String> {
    let bytes = match fetch(source, headers) {
        Ok(bytes) => bytes,
        Err(_) if sniff => return Ok(None),
        Err(e) => return Err(e),
    };
    if sniff && !foreign::sniff(&bytes) {
        return Ok(None);
    }
    foreign::Wav::decode(bytes).map(Some)
}

/// A clip's bytes: a file read, or a blocking fetch with its request headers.
#[cfg(feature = "ogg-audio")]
#[allow(deprecated)] // NSURLConnection's synchronous load is what a worker or headless run wants.
fn fetch(source: &MediaSource, headers: &[(String, String)]) -> Result<Vec<u8>, String> {
    use objc2_foundation::{NSHTTPURLResponse, NSMutableURLRequest, NSURLConnection};
    if let MediaSource::File(path) = source {
        return std::fs::read(path).map_err(|e| format!("can't read {}: {e}", path.display()));
    }
    let request = NSMutableURLRequest::requestWithURL(&*url(source)?);
    for (name, value) in headers {
        request.setValue_forHTTPHeaderField(Some(&NSString::from_str(value)), &NSString::from_str(name));
    }
    let mut response = None;
    let data = NSURLConnection::sendSynchronousRequest_returningResponse_error(&request, Some(&mut response))
        .map_err(|e| e.localizedDescription().to_string())?;
    let status = response.and_then(|r| r.downcast::<NSHTTPURLResponse>().ok()).map(|r| r.statusCode());
    if let Some(status) = status.filter(|s| !(200..300).contains(s)) {
        return Err(format!("HTTP {status}"));
    }
    Ok(data.to_vec())
}

pub(super) fn seconds(t: CMTime) -> f64 {
    // SAFETY: `CMTimeGetSeconds` accepts any `CMTime` value and returns non-finite for invalid times.
    let s = unsafe { t.seconds() };
    if s.is_finite() { s } else { 0.0 }
}
pub(super) fn cm(seconds: f64) -> CMTime {
    // SAFETY: `CMTimeMakeWithSeconds` accepts any `f64` and a positive timescale (`600_000`).
    unsafe { CMTime::with_seconds(seconds, 600_000) }
}
pub(super) fn zero() -> CMTime {
    CMTime { value: 0, timescale: 1, flags: CMTimeFlags::Valid, epoch: 0 }
}
pub(super) fn positive_infinity() -> CMTime {
    // SAFETY: `kCMTimePositiveInfinity` is an immutable static CoreMedia `CMTime` constant.
    unsafe { kCMTimePositiveInfinity }
}

/// Blocks until the asset's tracks load (a network fetch for URLs).
#[allow(deprecated)] // The synchronous loaders are what a worker or headless run wants.
fn meta(asset: &AVURLAsset, kind: Kind) -> Result<Meta, String> {
    let duration = || {
        // SAFETY: `asset` is a live `AVURLAsset` queried synchronously on a worker or headless thread.
        seconds(unsafe { asset.duration() })
    };
    if kind == Kind::Audio {
        // SAFETY: `AVMediaTypeAudio` is an immutable static AVFoundation string constant.
        let audio = unsafe { AVMediaTypeAudio }.ok_or("AVMediaTypeAudio unavailable")?;
        // SAFETY: `asset` is a live `AVURLAsset` and `audio` is the static `AVMediaTypeAudio` constant.
        if unsafe { asset.tracksWithMediaType(audio) }.firstObject().is_none() {
            return Err("no playable audio track (unreachable, or a format AVFoundation can't read)".into());
        }
        return Ok(Meta { duration: duration(), size: None, frame: None, sound: true });
    }
    // SAFETY: `AVMediaTypeVideo` is an immutable static AVFoundation string constant.
    let video = unsafe { AVMediaTypeVideo }.ok_or("AVMediaTypeVideo unavailable")?;
    // SAFETY: `asset` is a live `AVURLAsset` and `video` is the static `AVMediaTypeVideo` constant.
    let tracks = unsafe { asset.tracksWithMediaType(video) };
    let Some(track) = tracks.firstObject() else {
        return Err("no playable video track".into());
    };
    // SAFETY: `track` is a live `AVAssetTrack` retained from `tracks.firstObject()`.
    let size = unsafe { track.naturalSize() };
    // SAFETY: `track` is a live `AVAssetTrack` retained from `tracks.firstObject()`.
    let t = unsafe { track.preferredTransform() };
    let w = (size.width * t.a + size.height * t.c).abs().round();
    let h = (size.width * t.b + size.height * t.d).abs().round();
    let frame = track_frame_duration(&track);
    if w < 1.0 || h < 1.0 {
        return Err("video track has no size".into());
    }
    Ok(Meta { duration: duration(), size: Some((w as u32, h as u32)), frame, sound: has_sound(asset) })
}

fn track_frame_duration(track: &objc2_av_foundation::AVAssetTrack) -> Option<f64> {
    // SAFETY: `track` is a live `AVAssetTrack` retained from `tracksWithMediaType`.
    let min = seconds(unsafe { track.minFrameDuration() });
    if min.is_finite() && min > 0.0 {
        return Some(min);
    }
    // SAFETY: `track` is a live `AVAssetTrack` retained from `tracksWithMediaType`.
    let nominal = f64::from(unsafe { track.nominalFrameRate() });
    (nominal.is_finite() && nominal > 0.0).then(|| 1.0 / nominal)
}

/// Nominal seconds per video frame from the first video track (`minFrameDuration`, else
/// `1 / nominalFrameRate`), or `None` when the asset has no video track or no valid positive rate.
#[allow(deprecated)] // As `meta`: the synchronous loader.
pub(super) fn frame_duration(asset: &AVAsset) -> Option<f64> {
    // SAFETY: `AVMediaTypeVideo` is an immutable static AVFoundation string constant.
    let video = unsafe { AVMediaTypeVideo }?;
    // SAFETY: `asset` is a live `AVAsset` and `video` is the static `AVMediaTypeVideo` constant.
    let track = unsafe { asset.tracksWithMediaType(video) }.firstObject()?;
    track_frame_duration(&track)
}

/// Whether the asset has a sound track. Blocks until its tracks load.
#[allow(deprecated)] // As `meta`: the synchronous loader.
pub(super) fn has_sound(asset: &AVAsset) -> bool {
    // SAFETY: `AVMediaTypeAudio` is an immutable static AVFoundation string constant.
    unsafe { AVMediaTypeAudio }.is_some_and(|audio| {
        // SAFETY: `asset` is a live `AVAsset` and `audio` is the static `AVMediaTypeAudio` constant.
        unsafe { asset.tracksWithMediaType(audio) }.firstObject().is_some()
    })
}

/// How the first video track's buffers turn upright. Blocks until tracks load; free once an
/// item using the asset is ready to play.
#[allow(deprecated)]
pub(super) fn orient(asset: &AVAsset) -> Orient {
    // SAFETY: `AVMediaTypeVideo` is an immutable static AVFoundation string constant.
    let Some(video) = (unsafe { AVMediaTypeVideo }) else { return Orient::UP };
    // SAFETY: `asset` is a live `AVAsset` and `video` is the static `AVMediaTypeVideo` constant.
    let Some(track) = unsafe { asset.tracksWithMediaType(video) }.firstObject() else { return Orient::UP };
    // SAFETY: `track` is a live `AVAssetTrack` retained from `tracksWithMediaType`.
    let t = unsafe { track.preferredTransform() };
    Orient::from_transform(t.a, t.b, t.c, t.d)
}

pub(super) fn generator(asset: &AVURLAsset, max: Option<f64>) -> Retained<AVAssetImageGenerator> {
    // SAFETY: `asset` is a live `AVURLAsset`.
    let g = unsafe { AVAssetImageGenerator::assetImageGeneratorWithAsset(asset) };
    // SAFETY: `g` is a newly created, uniquely owned `AVAssetImageGenerator` not yet decoding.
    unsafe {
        g.setAppliesPreferredTrackTransform(true);
        g.setRequestedTimeToleranceBefore(zero());
        g.setRequestedTimeToleranceAfter(zero());
        if let Some(max) = max {
            g.setMaximumSize(CGSize::new(max, max));
        }
    }
    g
}

#[allow(deprecated)] // copyCGImageAtTime is the synchronous form.
pub(super) fn image(generator: &AVAssetImageGenerator, at: f64) -> Result<(ByteFrame, f64), String> {
    let mut actual = zero();
    // SAFETY: `generator` is a live `AVAssetImageGenerator` and `&mut actual` is a valid writable `CMTime` out-pointer.
    let image = unsafe { generator.copyCGImageAtTime_actualTime_error(cm(at), &mut actual) }
        .map_err(|e| e.localizedDescription().to_string())?;
    // SAFETY: `CMTimeGetSeconds` accepts any `CMTime` value and returns non-finite for invalid times.
    let s = unsafe { actual.seconds() };
    let pts = if s.is_finite() { s.max(0.0) } else { at.max(0.0) };
    Ok((bgra(&image)?, pts))
}

/// Draws a decoded image into sRGB BGRA bytes (opaque, so premultiplied equals straight).
fn bgra(image: &CGImage) -> Result<ByteFrame, String> {
    let (w, h) = (CGImage::width(Some(image)), CGImage::height(Some(image)));
    if w == 0 || h == 0 {
        return Err("decoded an empty frame".into());
    }
    let mut bytes = vec![0u8; w * h * 4];
    // SAFETY: `kCGColorSpaceSRGB` is an immutable static CoreGraphics color space name constant.
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }));
    let info = CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;
    // SAFETY: `bytes` has length `w * h * 4` matching 8 bpc BGRA with stride `w * 4`, and outlives `context`.
    let context = unsafe { CGBitmapContextCreate(bytes.as_mut_ptr().cast(), w, h, 8, w * 4, space.as_deref(), info) }
        .ok_or("CGBitmapContextCreate failed")?;
    CGContext::draw_image(Some(&context), CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w as f64, h as f64)), Some(image));
    drop(context);
    Ok(ByteFrame {
        width: w as u32,
        height: h as u32,
        format: Format::Bgra,
        color: ColorInfo { matrix: crate::native_surface::Matrix::Bt709, transfer: crate::native_surface::Transfer::Srgb },
        planes: [bytes, Vec::new()],
    })
}

pub(in crate::video) fn probe(native: &mut Native, source: &MediaSource, headers: &[(String, String)], kind: Kind) -> Result<Meta, String> {
    let (asset, meta) = open(source, headers, kind, &native.decoded)?;
    if kind == Kind::Video {
        native.generator = Some(generator(&asset, None));
    }
    Ok(meta)
}

pub(in crate::video) fn decode(native: &mut Native, source: &MediaSource, headers: &[(String, String)], at: f64) -> Result<(ByteFrame, f64), String> {
    if native.generator.is_none() {
        let asset = asset(source, headers)?;
        native.generator = Some(generator(&asset, None));
    }
    image(native.generator.as_ref().unwrap(), at)
}

/// A clip's frame at `at` upright (as posters and headless runs show it), and as its buffer
/// stores it (as the player hands it out) with how that turns upright.
#[cfg(test)]
pub(crate) fn test_frames(source: &MediaSource, at: f64) -> Result<(ByteFrame, ByteFrame, Orient), String> {
    let asset = asset(source, &[])?;
    let raw = generator(&asset, None);
    // SAFETY: `raw` is a newly created, uniquely owned `AVAssetImageGenerator` not yet decoding.
    unsafe { raw.setAppliesPreferredTrackTransform(false) };
    Ok((image(&generator(&asset, None), at)?.0, image(&raw, at)?.0, orient(&asset)))
}
