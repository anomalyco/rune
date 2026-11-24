//! Media Foundation: the source reader that opens and decodes a clip, and its library, loaded
//! with `LoadLibraryExW` the first time a clip opens so no MF DLL is in the import table.
//! Windows N editions without the Media Feature Pack have no `mfplat.dll`: every clip then
//! fails with a message saying so, and nothing else changes.

#![allow(unsafe_code)]

use super::super::{Kind, MediaSource};
use crate::native_surface::{ByteFrame, ColorInfo, Format, Matrix, Orient, Transfer};
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::OnceLock;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
use windows::Win32::System::Variant::VT_I8;
use windows::core::{GUID, HRESULT, HSTRING, Interface, PCSTR, PCWSTR};

/// `MF_VERSION`: `MF_SDK_VERSION << 16 | MF_API_VERSION`.
const MF_VERSION: u32 = 0x0002_0070;
/// 100 ns units per second, Media Foundation's clock.
pub(super) const TICKS: f64 = 10_000_000.0;

type Startup = unsafe extern "system" fn(u32, u32) -> HRESULT;
type CreateAttributes = unsafe extern "system" fn(*mut *mut core::ffi::c_void, u32) -> HRESULT;
type CreateMediaType = unsafe extern "system" fn(*mut *mut core::ffi::c_void) -> HRESULT;
type CreateDeviceManager = unsafe extern "system" fn(*mut u32, *mut *mut core::ffi::c_void) -> HRESULT;
type CreateFile = unsafe extern "system" fn(MF_FILE_ACCESSMODE, MF_FILE_OPENMODE, MF_FILE_FLAGS, PCWSTR, *mut *mut core::ffi::c_void) -> HRESULT;
type CreateResolver = unsafe extern "system" fn(*mut *mut core::ffi::c_void) -> HRESULT;
type CreateReaderFromSource = unsafe extern "system" fn(*mut core::ffi::c_void, *mut core::ffi::c_void, *mut *mut core::ffi::c_void) -> HRESULT;

/// The few flat Media Foundation entry points; everything else is COM.
pub(super) struct Library {
    create_attributes: CreateAttributes,
    create_media_type: CreateMediaType,
    create_device_manager: CreateDeviceManager,
    create_file: CreateFile,
    create_resolver: CreateResolver,
    create_reader: CreateReaderFromSource,
}

static LIBRARY: OnceLock<Result<Library, String>> = OnceLock::new();

/// Loads Media Foundation and starts it, once per process.
pub(super) fn library() -> Result<&'static Library, String> {
    LIBRARY.get_or_init(load).as_ref().map_err(Clone::clone)
}

fn load() -> Result<Library, String> {
    let missing = || "Media Foundation isn't installed (a Windows N edition without the Media Feature Pack)".to_string();
    let plat = system_library("mfplat.dll").ok_or_else(missing)?;
    let rw = system_library("mfreadwrite.dll").ok_or_else(missing)?;
    // SAFETY: each symbol is looked up by its documented name and cast to its documented signature.
    unsafe {
        let startup: Startup = symbol(plat, c"MFStartup").ok_or_else(missing)?;
        let library = Library {
            create_attributes: symbol(plat, c"MFCreateAttributes").ok_or_else(missing)?,
            create_media_type: symbol(plat, c"MFCreateMediaType").ok_or_else(missing)?,
            create_device_manager: symbol(plat, c"MFCreateDXGIDeviceManager").ok_or_else(missing)?,
            create_file: symbol(plat, c"MFCreateFile").ok_or_else(missing)?,
            create_resolver: symbol(plat, c"MFCreateSourceResolver").ok_or_else(missing)?,
            create_reader: symbol(rw, c"MFCreateSourceReaderFromMediaSource").ok_or_else(missing)?,
        };
        // MFSTARTUP_FULL: the network source needs sockets.
        startup(MF_VERSION, 0).ok().map_err(|e| format!("Media Foundation didn't start: {}", e.message()))?;
        Ok(library)
    }
}

/// A DLL from System32 only, never the app's folder or the current directory.
pub(super) fn system_library(name: &str) -> Option<HMODULE> {
    // SAFETY: loads a system DLL by name; the handle is kept for the process's life.
    unsafe { LoadLibraryExW(&HSTRING::from(name), None, LOAD_LIBRARY_SEARCH_SYSTEM32) }.ok()
}

/// `name` from `module` as a function of type `F`.
///
/// # Safety
/// `F` must be the symbol's true signature.
pub(super) unsafe fn symbol<F: Copy>(module: HMODULE, name: &std::ffi::CStr) -> Option<F> {
    // SAFETY: a lookup by name on a loaded module.
    let f = unsafe { GetProcAddress(module, PCSTR(name.as_ptr().cast())) }?;
    debug_assert_eq!(size_of::<F>(), size_of_val(&f));
    // SAFETY: the caller vouches for `F`.
    Some(unsafe { std::mem::transmute_copy(&f) })
}

impl Library {
    pub(super) fn attributes(&self, count: u32) -> Result<IMFAttributes, String> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: MFCreateAttributes fills `raw` with an owned reference on success.
        unsafe { (self.create_attributes)(&mut raw, count).ok().map_err(|e| e.message())?; Ok(IMFAttributes::from_raw(raw)) }
    }

    fn media_type(&self) -> Result<IMFMediaType, String> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: as `attributes`.
        unsafe { (self.create_media_type)(&mut raw).ok().map_err(|e| e.message())?; Ok(IMFMediaType::from_raw(raw)) }
    }

    pub(super) fn device_manager(&self) -> Result<(IMFDXGIDeviceManager, u32), String> {
        let (mut token, mut raw) = (0, std::ptr::null_mut());
        // SAFETY: as `attributes`.
        unsafe { (self.create_device_manager)(&mut token, &mut raw).ok().map_err(|e| e.message())?; Ok((IMFDXGIDeviceManager::from_raw(raw), token)) }
    }

    /// Opens `target` as a media source: a strict local byte stream (bypassing scheme handlers
    /// and wildcard handler sniffing), or a remote URL whose first bytes say what it holds.
    /// It needs no GPU, so it runs while the decoders' device comes up.
    pub(super) fn resolve(&self, target: &Target) -> Result<IMFMediaSource, String> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: as `attributes`; then a blocking resolve into an owned `IMFMediaSource`.
        unsafe {
            (self.create_resolver)(&mut raw).ok().map_err(|e| e.message())?;
            let resolver = IMFSourceResolver::from_raw(raw);
            let (mut kind, mut object) = (MF_OBJECT_INVALID, None);
            match target {
                Target::Local { path, ext } => {
                    let mut stream_raw = std::ptr::null_mut();
                    (self.create_file)(MF_ACCESSMODE_READ, MF_OPENMODE_FAIL_IF_NOT_EXIST, MF_FILEFLAGS_NONE, PCWSTR(path.as_ptr()), &mut stream_raw)
                        .ok()
                        .map_err(open_error)?;
                    let stream = IMFByteStream::from_raw(stream_raw);
                    // Extension-matched byte-stream handler only: no wildcard handler enumeration
                    // and no process-local plugins; `CreateObjectFromByteStream` skips scheme handlers.
                    let flags = (MF_RESOLUTION_MEDIASOURCE | MF_RESOLUTION_READ | MF_RESOLUTION_DISABLE_LOCAL_PLUGINS).0 as u32;
                    let hint = HSTRING::from(format!("local.{ext}"));
                    resolver.CreateObjectFromByteStream(&stream, &hint, flags, None, &mut kind, &mut object).map_err(open_error)?;
                }
                Target::Remote(url) => {
                    let flags = (MF_RESOLUTION_MEDIASOURCE | MF_RESOLUTION_CONTENT_DOES_NOT_HAVE_TO_MATCH_EXTENSION_OR_MIME_TYPE).0 as u32;
                    resolver.CreateObjectFromURL(url, flags, None, &mut kind, &mut object).map_err(open_error)?;
                }
            }
            object.ok_or("no media source")?.cast::<IMFMediaSource>().map_err(|e| e.message())
        }
    }

    fn reader(&self, source: &IMFMediaSource, attributes: &IMFAttributes) -> Result<IMFSourceReader, String> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: as `attributes`; the reader takes its own references to the source and the
        // attributes, and shuts the source down when it goes.
        unsafe { (self.create_reader)(source.as_raw(), attributes.as_raw(), &mut raw).ok().map_err(open_error)?; Ok(IMFSourceReader::from_raw(raw)) }
    }
}

/// What a failed open means to someone looking at a clip that won't play.
fn open_error(e: windows::core::Error) -> String {
    match e.code().0 as u32 {
        0xC00D_36C4 => "Media Foundation can't read this format".into(),
        0x8007_0002 | 0x8007_0003 | 0xC00D_001A => "the clip wasn't found".into(),
        0x8007_0005 => "access to the clip was denied".into(),
        // NS_E_SERVER_NOT_FOUND is what the network source answers for a refused connection.
        0x8007_2EE7 | 0x8007_2EFD | 0x8007_2EFE | 0xC00D_2EE7 | 0xC00D_0035 => "the clip's server couldn't be reached".into(),
        // MF_E_INVALIDREQUEST from the byte stream: an mp4 cut short, its index never reached.
        0xC00D_36E5 => "the clip is incomplete or damaged".into(),
        _ => format!("Media Foundation couldn't open the clip: {} ({:#010x})", e.message(), e.code().0),
    }
}

/// The first video stream's picture, as it comes out of the reader.
#[derive(Clone, Copy, Debug)]
pub(super) struct Picture {
    /// The decoder's surface, which may be taller than the picture (1920 x 1088 for 1080p).
    pub coded: (u32, u32),
    /// The picture inside it: x, y, width, height, all even.
    pub crop: (u32, u32, u32, u32),
    /// Display size in pixels (pixel aspect and rotation applied).
    pub natural: (u32, u32),
    pub format: Format,
    pub color: ColorInfo,
    pub orient: Orient,
    /// Nominal seconds per frame from `MF_MT_FRAME_RATE`, when the stream reports a positive rate.
    pub frame_duration: Option<f64>,
}

impl Picture {
    /// Nominal seconds per frame, falling back to 30 fps for internal scheduling when unknown.
    pub(super) fn frame(&self) -> f64 {
        self.frame_duration.unwrap_or(super::super::DEFAULT_FRAME)
    }
}

/// The first audio stream's samples: 32-bit float, interleaved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Sound {
    pub channels: u16,
    pub rate: u32,
}

pub(super) struct Opened {
    pub reader: IMFSourceReader,
    /// The video stream's index, which reads from any stream report.
    pub video: Option<u32>,
    pub picture: Option<Picture>,
    /// The audio stream's index; the stream is selected only while sound plays.
    pub audio: Option<u32>,
    pub sound: Option<Sound>,
    /// The clip has an audio stream, decodable when it was set up (`audio`).
    pub has_sound: bool,
    pub duration: f64,
}

/// How the reader decodes.
pub(super) enum Decode<'a> {
    /// Into D3D11 textures on the manager's device, with hardware decoders where the GPU has one.
    Gpu(&'a IMFDXGIDeviceManager),
    /// Into system memory with software decoders.
    Cpu,
}

/// Reads `source` and sets up the streams `kind` plays: for video the first video stream as
/// NV12 and, when `audio`, the first audio stream as float; for audio only the audio stream.
pub(super) fn open(lib: &Library, source: &IMFMediaSource, kind: Kind, decode: Decode<'_>, audio: bool) -> Result<Opened, String> {
    let attributes = lib.attributes(4)?;
    // SAFETY: plain attribute writes on an object this thread owns.
    unsafe {
        if let Decode::Gpu(manager) = decode {
            attributes.SetUnknown(&MF_SOURCE_READER_D3D_MANAGER, manager).map_err(|e| e.message())?;
            attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1).map_err(|e| e.message())?;
        }
        // Converts a decoder's other output (P010, YUY2, interlaced) to progressive NV12.
        attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1).map_err(|e| e.message())?;
    }
    let reader = lib.reader(source, &attributes)?;
    // SAFETY: stream selection and type negotiation on the reader this thread made.
    unsafe {
        reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false).ok();
        let (video, audio_index) = streams(&reader);
        let (video, picture) = match video.filter(|_| kind == Kind::Video) {
            Some(index) => {
                reader.SetStreamSelection(index, true).map_err(|e| e.message())?;
                let wanted = lib.media_type()?;
                wanted.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| e.message())?;
                wanted.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).map_err(|e| e.message())?;
                reader.SetCurrentMediaType(index, None, &wanted).map_err(|e| no_decoder(e, "video"))?;
                (Some(index), Some(picture(&reader.GetCurrentMediaType(index).map_err(|e| e.message())?)?))
            }
            None if kind == Kind::Video => return Err("no playable video track".into()),
            None => (None, None),
        };
        let mut has_sound = audio_index.is_some();
        let (audio, sound) = match audio_index.filter(|_| audio || kind == Kind::Audio) {
            Some(index) => match sound(lib, &reader, index) {
                Ok(sound) => (Some(index), Some(sound)),
                Err(e) if kind == Kind::Audio => return Err(e),
                Err(_) => {
                    has_sound = false;
                    (None, None)
                }
            },
            None if kind == Kind::Audio => return Err("no playable audio track".into()),
            None => (None, None),
        };
        let duration = duration(&reader);
        Ok(Opened { reader, video, picture, audio, sound, has_sound, duration })
    }
}

/// The indexes of the first video and the first audio stream.
unsafe fn streams(reader: &IMFSourceReader) -> (Option<u32>, Option<u32>) {
    let (mut video, mut audio) = (None, None);
    // SAFETY: native type reads on a live reader; an index past the last stream fails.
    unsafe {
        for index in 0..64 {
            let Ok(t) = reader.GetNativeMediaType(index, 0) else { break };
            match t.GetMajorType() {
                Ok(major) if major == MFMediaType_Video => video = video.or(Some(index)),
                Ok(major) if major == MFMediaType_Audio => audio = audio.or(Some(index)),
                _ => {}
            }
        }
    }
    (video, audio)
}

/// Stream `index` set up as float audio. It stays selected; the decoder deselects it while muted.
unsafe fn sound(lib: &Library, reader: &IMFSourceReader, index: u32) -> Result<Sound, String> {
    // SAFETY: as in `open`.
    unsafe {
        reader.SetStreamSelection(index, true).map_err(|e| e.message())?;
        let wanted = lib.media_type()?;
        wanted.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).map_err(|e| e.message())?;
        wanted.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float).map_err(|e| e.message())?;
        if let Err(e) = reader.SetCurrentMediaType(index, None, &wanted) {
            reader.SetStreamSelection(index, false).ok();
            return Err(no_decoder(e, "audio"));
        }
        let current = reader.GetCurrentMediaType(index).map_err(|e| e.message())?;
        let channels = current.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(2).clamp(1, 8) as u16;
        let rate = current.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap_or(48_000);
        Ok(Sound { channels, rate })
    }
}

fn no_decoder(e: windows::core::Error, what: &str) -> String {
    match e.code().0 as u32 {
        // MF_E_TOPO_CODEC_NOT_FOUND, MF_E_INVALIDMEDIATYPE
        0xC00D_5212 | 0xC00D_36B4 => format!("no {what} decoder for this codec (Media Foundation has none installed)"),
        _ => format!("can't decode the {what}: {}", e.message()),
    }
}

/// Facts about the decoded picture from the reader's NV12 type.
unsafe fn picture(t: &IMFMediaType) -> Result<Picture, String> {
    // SAFETY: attribute reads on a live media type.
    unsafe {
        let size = t.GetUINT64(&MF_MT_FRAME_SIZE).map_err(|_| "video track has no size".to_string())?;
        let coded = ((size >> 32) as u32, size as u32);
        if coded.0 < 2 || coded.1 < 2 {
            return Err("video track has no size".into());
        }
        let mut area = MFVideoArea::default();
        let bytes = std::slice::from_raw_parts_mut((&raw mut area).cast::<u8>(), size_of::<MFVideoArea>());
        let crop = match t.GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, bytes, None) {
            Ok(()) if area.Area.cx > 0 && area.Area.cy > 0 => {
                let (x, y) = (area.OffsetX.value.max(0) as u32 & !1, area.OffsetY.value.max(0) as u32 & !1);
                let w = (area.Area.cx as u32).min(coded.0 - x) & !1;
                let h = (area.Area.cy as u32).min(coded.1 - y) & !1;
                (x, y, w.max(2), h.max(2))
            }
            _ => (0, 0, coded.0 & !1, coded.1 & !1),
        };
        let full = t.GetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE).is_ok_and(|r| r == MFNominalRange_0_255.0 as u32);
        // Unknown matrices follow the picture's size, as browsers do: SD is BT.601.
        let matrix = match t.GetUINT32(&MF_MT_YUV_MATRIX).map(|m| m as i32) {
            Ok(m) if m == MFVideoTransferMatrix_BT601.0 => Matrix::Bt601,
            Ok(m) if m == MFVideoTransferMatrix_BT709.0 => Matrix::Bt709,
            _ if crop.3 < 720 => Matrix::Bt601,
            _ => Matrix::Bt709,
        };
        let par = t.GetUINT64(&MF_MT_PIXEL_ASPECT_RATIO).ok().map(|p| ((p >> 32) as f64, (p as u32) as f64)).filter(|(n, d)| *n > 0.0 && *d > 0.0);
        let width = par.map_or(crop.2 as f64, |(n, d)| crop.2 as f64 * n / d).round() as u32;
        let orient = rotation(t.GetUINT32(&MF_MT_VIDEO_ROTATION).unwrap_or(0));
        let natural = orient.upright([width, crop.3]);
        let rate = t.GetUINT64(&MF_MT_FRAME_RATE).ok().map(|r| ((r >> 32) as f64, (r as u32) as f64)).filter(|(n, d)| *n > 0.0 && *d > 0.0);
        Ok(Picture {
            coded,
            crop,
            natural: (natural[0], natural[1]),
            format: if full { Format::Nv12Full } else { Format::Nv12Video },
            color: ColorInfo { matrix, transfer: Transfer::Bt709 },
            orient,
            frame_duration: rate.map(|(n, d)| d / n),
        })
    }
}

/// `MF_MT_VIDEO_ROTATION`: the picture must turn this many degrees counter-clockwise to stand
/// upright. As `Orient`, which maps upright coordinates (y down) to buffer coordinates.
fn rotation(degrees: u32) -> Orient {
    match degrees {
        90 => Orient([0, 1, -1, 0]),
        180 => Orient([-1, 0, 0, -1]),
        270 => Orient([0, -1, 1, 0]),
        _ => Orient::UP,
    }
}

/// The clip's length in seconds, 0 when it doesn't say (a live stream).
unsafe fn duration(reader: &IMFSourceReader) -> f64 {
    // SAFETY: a presentation attribute read; the PROPVARIANT holds a plain integer.
    unsafe {
        match reader.GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION) {
            Ok(v) => v.Anonymous.Anonymous.Anonymous.uhVal as f64 / TICKS,
            Err(_) => 0.0,
        }
    }
}

/// Moves the reader to `seconds`; it lands on the key frame at or before it.
pub(super) fn seek(reader: &IMFSourceReader, seconds: f64) -> Result<(), String> {
    let mut at = PROPVARIANT::default();
    // SAFETY: a VT_I8 PROPVARIANT owns nothing, so it needs no clearing.
    unsafe {
        (*at.Anonymous.Anonymous).vt = VT_I8;
        (*at.Anonymous.Anonymous).Anonymous.hVal = (seconds.max(0.0) * TICKS) as i64;
        reader.SetCurrentPosition(&GUID::zeroed(), &at).map_err(|e| format!("seek: {}", e.message()))
    }
}

/// Validated media target before Media Foundation loads or touches the network/filesystem.
pub(super) enum Target {
    Local { path: HSTRING, ext: String },
    Remote(HSTRING),
}

impl Target {
    pub(super) fn from_source(source: &MediaSource) -> Result<Self, String> {
        match source {
            MediaSource::File(path) => {
                let (path, ext) = validate_local_file(path)?;
                Ok(Self::Local { path, ext })
            }
            MediaSource::Url(url) if url.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("file:")) => {
                let path = parse_file_url(url)?;
                let (path, ext) = validate_local_file(&path)?;
                Ok(Self::Local { path, ext })
            }
            MediaSource::Url(url) if url.starts_with("http://") || url.starts_with("https://") => Ok(Self::Remote(HSTRING::from(url.as_str()))),
            MediaSource::Url(url) => Err(format!("unsupported URL: {url}")),
        }
    }
}

/// Validates that `path` is a plain local disk file with a direct media container extension
/// before touching disk or Media Foundation. Refuses UNC shares, Win32/NT device namespaces,
/// embedded URL schemes, NTFS alternate data streams, reserved DOS device names, trailing
/// dots/spaces, and shortcut/playlist redirectors (`.url`, `.lnk`, `.asx`, `.m3u8`, etc.).
fn validate_local_file(path: &Path) -> Result<(HSTRING, String), String> {
    let raw = path.to_str().ok_or_else(|| format!("unsupported local path: {}", path.display()))?;
    if raw.is_empty() || raw.chars().any(char::is_control) || raw.contains(['?', '*']) {
        return Err(format!("unsupported local path: {}", path.display()));
    }
    if has_unc_or_device_prefix(raw) || has_unc_or_device_prefix(raw.trim_start()) {
        return Err(format!("local media must be a plain local file path: {}", path.display()));
    }
    if !has_valid_local_colons(raw) {
        return Err(format!("local media must be a plain local file path: {}", path.display()));
    }
    for component in Path::new(raw).components() {
        match component {
            Component::Prefix(prefix) if matches!(prefix.kind(), Prefix::Disk(_)) => {}
            Component::Prefix(_) => return Err(format!("local media must be a plain local file path: {}", path.display())),
            Component::Normal(seg) => {
                let Some(seg) = seg.to_str() else {
                    return Err(format!("unsupported local path: {}", path.display()));
                };
                if seg.ends_with(['.', ' ']) || is_dos_device_name(seg) {
                    return Err(format!("unsupported local path: {}", path.display()));
                }
            }
            Component::RootDir | Component::CurDir | Component::ParentDir => {}
        }
    }
    let ext = local_media_extension(Path::new(raw))
        .ok_or_else(|| format!("unsupported local media file type: {}", path.display()))?;
    let abs = std::path::absolute(path).map_err(|e| format!("invalid local path: {e}"))?;
    let abs_str = abs.to_str().ok_or_else(|| format!("unsupported local path: {}", path.display()))?;
    let mut abs_components = abs.components();
    let is_plain_disk = matches!(abs_components.next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_)))
        && matches!(abs_components.next(), Some(Component::RootDir));
    if !is_plain_disk || has_unc_or_device_prefix(abs_str) || !has_valid_local_colons(abs_str) {
        return Err(format!("local media must be a plain local file path: {}", path.display()));
    }
    let meta = std::fs::metadata(&abs).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("no such file: {}", path.display()),
        _ => format!("can't open {}: {e}", path.display()),
    })?;
    if !meta.is_file() {
        return Err(format!("not a file: {}", path.display()));
    }
    Ok((HSTRING::from(abs.as_os_str()), ext))
}

fn has_unc_or_device_prefix(s: &str) -> bool {
    let bytes = s.as_bytes();
    let two_seps = bytes.len() >= 2 && matches!(bytes[0], b'\\' | b'/') && matches!(bytes[1], b'\\' | b'/');
    let nt_prefix = bytes.len() >= 3
        && matches!(bytes[0], b'\\' | b'/')
        && bytes[1] == b'?'
        && bytes[2] == b'?'
        && (bytes.len() == 3 || matches!(bytes[3], b'\\' | b'/'));
    two_seps || nt_prefix
}

/// Only a single leading drive letter (`X:\` or `X:/`) may contain `:`; any other `:` indicates
/// a URL scheme (`http:`, `file:`, `mms:`, `rtsp:`), a drive-relative path, or an NTFS ADS.
fn has_valid_local_colons(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut colons = bytes.iter().enumerate().filter_map(|(i, &b)| (b == b':').then_some(i));
    let Some(first) = colons.next() else { return true };
    if colons.next().is_some() {
        return false;
    }
    first == 1 && bytes[0].is_ascii_alphabetic() && matches!(bytes.get(2), Some(b'\\' | b'/'))
}

fn is_dos_device_name(segment: &str) -> bool {
    let stem = segment.split('.').next().unwrap_or(segment).trim_end_matches(' ');
    matches!(
        stem.to_ascii_uppercase().as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
            | "COM1" | "COM2" | "COM3" | "COM4" | "COM5" | "COM6" | "COM7" | "COM8" | "COM9"
            | "LPT1" | "LPT2" | "LPT3" | "LPT4" | "LPT5" | "LPT6" | "LPT7" | "LPT8" | "LPT9"
    ) || matches!(
        stem,
        "COM¹" | "COM²" | "COM³" | "LPT¹" | "LPT²" | "LPT³"
            | "com¹" | "com²" | "com³" | "lpt¹" | "lpt²" | "lpt³"
    )
}

/// Direct self-contained media container extensions accepted for local files on Windows.
/// Redirectors, shortcuts, and playlists (`.url`, `.lnk`, `.asx`, `.wax`, `.wvx`, `.wmx`, `.wpl`,
/// `.m3u`, `.m3u8`, `.pls`, `.qtl`, `.ram`, `.smil`, `.smi`, `.sami`, `.nsc`, `.xspf`, `.mpd`,
/// `.ism`, `.isml`, `.f4m`, `.sdp`) are refused before touching Media Foundation.
fn local_media_extension(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "mp4" | "m4v" | "mp4v" | "mov" | "webm" | "mkv" | "mk3d" | "avi" | "wmv"
            | "mpg" | "mpeg" | "mp2v" | "m2t" | "m2ts" | "mts" | "ts" | "tts" | "vob" | "mod" | "tod"
            | "3gp" | "3g2" | "3gp2" | "3gpp" | "uvu" | "ogv"
            | "mp3" | "mpa" | "m4a" | "aac" | "adt" | "adts" | "ac3" | "ec3" | "amr"
            | "wav" | "aif" | "aiff" | "flac" | "ogg" | "oga" | "opus" | "mka" | "weba" | "wma"
    )
    .then_some(ext)
}

/// Converts a local `file:///X:/...` or `file://localhost/X:/...` URL to a local Windows path,
/// refusing remote hosts, UNC escapes, queries, fragments, and malformed percent escapes.
fn parse_file_url(url: &str) -> Result<PathBuf, String> {
    let rest = url
        .get(..7)
        .filter(|p| p.eq_ignore_ascii_case("file://"))
        .map(|_| &url[7..])
        .ok_or_else(|| format!("unsupported file URL: {url}"))?;
    let after_host = if let Some(s) = rest.strip_prefix('/') {
        s
    } else if rest.get(..10).is_some_and(|p| p.eq_ignore_ascii_case("localhost/")) {
        &rest[10..]
    } else {
        return Err(format!("remote file URL isn't supported: {url}"));
    };
    if after_host.starts_with(['/', '\\']) || after_host.contains(['?', '#']) {
        return Err(format!("unsupported file URL: {url}"));
    }
    let decoded = percent_decode(after_host).ok_or_else(|| format!("invalid file URL encoding: {url}"))?;
    if decoded.starts_with(['/', '\\']) {
        return Err(format!("remote file URL isn't supported: {url}"));
    }
    Ok(PathBuf::from(decoded.replace('/', "\\")))
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = (*bytes.get(i + 1)? as char).to_digit(16)? as u8;
            let lo = (*bytes.get(i + 2)? as char).to_digit(16)? as u8;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// One read: the stream it came from, its flags, its time in seconds and the sample, if any.
pub(super) struct Read {
    pub stream: u32,
    pub flags: i32,
    pub time: f64,
    pub sample: Option<IMFSample>,
}

pub(super) fn read(reader: &IMFSourceReader, stream: u32) -> Result<Read, String> {
    let (mut index, mut flags, mut time, mut sample) = (0u32, 0u32, 0i64, None);
    // SAFETY: a blocking read into locals.
    unsafe { reader.ReadSample(stream, 0, Some(&mut index), Some(&mut flags), Some(&mut time), Some(&mut sample)) }
        .map_err(|e| format!("decode: {}", e.message()))?;
    Ok(Read { stream: index, flags: flags as i32, time: time as f64 / TICKS, sample })
}

/// A system-memory NV12 sample as packed planes of `picture.crop`'s size.
pub(super) fn nv12_bytes(sample: &IMFSample, picture: &Picture) -> Result<ByteFrame, String> {
    // SAFETY: the buffer is locked for the copy and unlocked after it.
    unsafe {
        let buffer = sample.ConvertToContiguousBuffer().map_err(|e| e.message())?;
        let (mut base, mut pitch, mut len) = (std::ptr::null_mut(), 0i32, 0u32);
        let two_d = buffer.cast::<IMF2DBuffer2>().ok();
        let locked = match &two_d {
            Some(b) => {
                let mut start = std::ptr::null_mut();
                b.Lock2DSize(MF2DBuffer_LockFlags_Read, &mut base, &mut pitch, &mut start, &mut len).is_ok()
            }
            None => false,
        };
        if !locked {
            buffer.Lock(&mut base, None, Some(&mut len)).map_err(|e| e.message())?;
            pitch = picture.coded.0 as i32;
        }
        let pitch = pitch.unsigned_abs() as usize;
        let (x, y, w, h) = picture.crop;
        let (w, h, x, y) = (w as usize, h as usize, x as usize, y as usize);
        // The luma plane's height from the buffer itself (NV12 is 1.5 planes of luma): the media
        // type's coded height can differ from what the decoder wrote (1088 against 1080 rows), and
        // reading chroma from there leaves the top rows without colour, a green band.
        let rows = match len as usize * 2 / (3 * pitch.max(1)) {
            0 => picture.coded.1 as usize,
            rows => rows.max(y + h),
        };
        let mut luma = Vec::with_capacity(w * h);
        let mut chroma = Vec::with_capacity(w * h / 2);
        for row in y..y + h {
            luma.extend_from_slice(std::slice::from_raw_parts(base.add(row * pitch + x), w));
        }
        let uv = base.add(rows * pitch);
        for row in y / 2..(y + h) / 2 {
            chroma.extend_from_slice(std::slice::from_raw_parts(uv.add(row * pitch + x), w));
        }
        match &two_d {
            Some(b) if locked => { b.Unlock2D().ok(); }
            _ => { buffer.Unlock().ok(); }
        }
        Ok(ByteFrame { width: w as u32, height: h as u32, format: picture.format, color: picture.color, planes: [luma, chroma] })
    }
}
