//! Media Foundation: the source reader that opens and decodes a clip, and its library, loaded
//! with `LoadLibraryExW` the first time a clip opens so no MF DLL is in the import table.
//! Windows N editions without the Media Feature Pack have no `mfplat.dll`: every clip then
//! fails with a message saying so, and nothing else changes.

use super::super::{Kind, MediaSource};
use crate::native_surface::{ByteFrame, ColorInfo, Format, Matrix, Orient, Transfer};
use std::sync::OnceLock;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
use windows::Win32::System::Variant::VT_I8;
use windows::core::{GUID, HRESULT, HSTRING, Interface, PCSTR};

/// `MF_VERSION`: `MF_SDK_VERSION << 16 | MF_API_VERSION`.
const MF_VERSION: u32 = 0x0002_0070;
/// 100 ns units per second, Media Foundation's clock.
pub(super) const TICKS: f64 = 10_000_000.0;

type Startup = unsafe extern "system" fn(u32, u32) -> HRESULT;
type CreateAttributes = unsafe extern "system" fn(*mut *mut core::ffi::c_void, u32) -> HRESULT;
type CreateMediaType = unsafe extern "system" fn(*mut *mut core::ffi::c_void) -> HRESULT;
type CreateDeviceManager = unsafe extern "system" fn(*mut u32, *mut *mut core::ffi::c_void) -> HRESULT;
type CreateResolver = unsafe extern "system" fn(*mut *mut core::ffi::c_void) -> HRESULT;
type CreateReaderFromSource = unsafe extern "system" fn(*mut core::ffi::c_void, *mut core::ffi::c_void, *mut *mut core::ffi::c_void) -> HRESULT;

/// The few flat Media Foundation entry points; everything else is COM.
pub(super) struct Library {
    create_attributes: CreateAttributes,
    create_media_type: CreateMediaType,
    create_device_manager: CreateDeviceManager,
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

    /// Opens `source` as a media source: the file, or the first bytes of the URL, which say what
    /// it holds. It needs no GPU, so it runs while the decoders' device comes up.
    pub(super) fn resolve(&self, source: &MediaSource) -> Result<IMFMediaSource, String> {
        let url = url(source)?;
        let mut raw = std::ptr::null_mut();
        // SAFETY: as `attributes`; then a blocking resolve of the URL into an owned object.
        unsafe {
            (self.create_resolver)(&mut raw).ok().map_err(|e| e.message())?;
            let resolver = IMFSourceResolver::from_raw(raw);
            let flags = (MF_RESOLUTION_MEDIASOURCE | MF_RESOLUTION_CONTENT_DOES_NOT_HAVE_TO_MATCH_EXTENSION_OR_MIME_TYPE).0 as u32;
            let (mut kind, mut object) = (MF_OBJECT_INVALID, None);
            resolver.CreateObjectFromURL(&url, flags, None, &mut kind, &mut object).map_err(open_error)?;
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
    /// Seconds per frame, from the stream's nominal rate.
    pub frame: f64,
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
        let _ = reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false);
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
            let _ = reader.SetStreamSelection(index, false);
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
            frame: rate.map_or(1.0 / 30.0, |(n, d)| d / n),
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

fn url(source: &MediaSource) -> Result<HSTRING, String> {
    match source {
        MediaSource::File(path) => {
            if !path.exists() {
                return Err(format!("no such file: {}", path.display()));
            }
            let path = std::path::absolute(path).unwrap_or_else(|_| path.clone());
            Ok(HSTRING::from(path.as_os_str()))
        }
        MediaSource::Url(url) if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("file:") => Ok(HSTRING::from(url.as_str())),
        MediaSource::Url(url) => Err(format!("unsupported URL: {url}")),
    }
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
            Some(b) if locked => { let _ = b.Unlock2D(); }
            _ => { let _ = buffer.Unlock(); }
        }
        Ok(ByteFrame { width: w as u32, height: h as u32, format: picture.format, color: picture.color, planes: [luma, chroma] })
    }
}
