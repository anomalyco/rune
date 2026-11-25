//! PulseAudio simple API (`libpulse-simple.so.0`) backend, loaded at runtime with `dlopen`.
//! Also serves PipeWire desktops via `pipewire-pulse`.

#![allow(unsafe_code)]

use super::{dlopen_library, dlsym_fn, dlsym_opt, sanitize_c_string};
use crate::audio::{AudioBackend, SampleFormat, SinkDevice, SinkError, ValidatedSpec};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::OnceLock;

/// `PA_STREAM_PLAYBACK` from `pulse/def.h`.
pub(super) const PA_STREAM_PLAYBACK: c_int = 1;
/// `PA_SAMPLE_S16LE` from `pulse/sample.h`.
pub(super) const PA_SAMPLE_S16LE: c_int = 3;
/// `PA_SAMPLE_FLOAT32LE` from `pulse/sample.h`.
pub(super) const PA_SAMPLE_FLOAT32LE: c_int = 5;
/// `PA_USEC_INVALID` returned by `pa_simple_get_latency` on error.
pub(super) const PA_USEC_INVALID: u64 = u64::MAX;

/// Target PulseAudio server buffer length (~60 ms).
const TARGET_LATENCY_MS: u64 = 60;
/// Target PulseAudio minimum request quantum (~10 ms).
const MINREQ_MS: u64 = 10;

/// C ABI layout of `pa_sample_spec` (`pulse/sample.h`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PaSampleSpec {
    pub(super) format: c_int,
    pub(super) rate: u32,
    pub(super) channels: u8,
}

/// C ABI layout of `pa_buffer_attr` (`pulse/def.h`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PaBufferAttr {
    pub(super) maxlength: u32,
    pub(super) tlength: u32,
    pub(super) prebuf: u32,
    pub(super) minreq: u32,
    pub(super) fragsize: u32,
}

pub(super) type PaSimpleNew = unsafe extern "C" fn(
    server: *const c_char,
    name: *const c_char,
    dir: c_int,
    dev: *const c_char,
    stream_name: *const c_char,
    ss: *const PaSampleSpec,
    map: *const c_void,
    attr: *const PaBufferAttr,
    error: *mut c_int,
) -> *mut c_void;
pub(super) type PaSimpleWrite =
    unsafe extern "C" fn(s: *mut c_void, data: *const c_void, bytes: usize, error: *mut c_int) -> c_int;
pub(super) type PaSimpleGetLatency = unsafe extern "C" fn(s: *mut c_void, error: *mut c_int) -> u64;
pub(super) type PaSimpleFlush = unsafe extern "C" fn(s: *mut c_void, error: *mut c_int) -> c_int;
pub(super) type PaSimpleDrain = unsafe extern "C" fn(s: *mut c_void, error: *mut c_int) -> c_int;
pub(super) type PaSimpleFree = unsafe extern "C" fn(s: *mut c_void);
pub(super) type PaStrerror = unsafe extern "C" fn(error: c_int) -> *const c_char;

/// Resolved `libpulse-simple.so.0` function table cached for process lifetime.
#[derive(Clone, Copy)]
pub(super) struct PulseApi {
    pub(super) new: PaSimpleNew,
    pub(super) write: PaSimpleWrite,
    pub(super) get_latency: PaSimpleGetLatency,
    pub(super) flush: PaSimpleFlush,
    #[allow(dead_code)] // Resolved to validate ABI completeness; never called (non-blocking drain polls latency).
    pub(super) drain: PaSimpleDrain,
    pub(super) free: PaSimpleFree,
    pub(super) strerror: Option<PaStrerror>,
}

static PULSE_API: OnceLock<Result<PulseApi, String>> = OnceLock::new();

fn api() -> Result<PulseApi, String> {
    PULSE_API.get_or_init(load_api).clone()
}

fn load_api() -> Result<PulseApi, String> {
    let handle = dlopen_library(c"libpulse-simple.so.0")?;
    // SAFETY: `handle` was loaded from `libpulse-simple.so.0` and each symbol matches its
    // documented PulseAudio C ABI signature in `pulse/simple.h` / `pulse/error.h`.
    unsafe {
        Ok(PulseApi {
            new: dlsym_fn(handle, c"pa_simple_new")?,
            write: dlsym_fn(handle, c"pa_simple_write")?,
            get_latency: dlsym_fn(handle, c"pa_simple_get_latency")?,
            flush: dlsym_fn(handle, c"pa_simple_flush")?,
            drain: dlsym_fn(handle, c"pa_simple_drain")?,
            free: dlsym_fn(handle, c"pa_simple_free")?,
            strerror: dlsym_opt(handle, c"pa_strerror"),
        })
    }
}

pub(super) fn pa_format(format: SampleFormat) -> c_int {
    match format {
        SampleFormat::I16 => PA_SAMPLE_S16LE,
        SampleFormat::F32 => PA_SAMPLE_FLOAT32LE,
    }
}

/// Computes frame-aligned `pa_buffer_attr` targeting ~60 ms `tlength` and ~10 ms `minreq`,
/// leaving `maxlength`, `prebuf`, and `fragsize` at `u32::MAX` (server defaults).
pub(super) fn buffer_attr(sample_rate: u32, frame_bytes: usize) -> PaBufferAttr {
    let fb = (frame_bytes.max(1)) as u64;
    let tlength_frames = (u64::from(sample_rate) * TARGET_LATENCY_MS / 1_000).max(1);
    let minreq_frames = (u64::from(sample_rate) * MINREQ_MS / 1_000).clamp(1, tlength_frames);
    let tlength = tlength_frames.saturating_mul(fb).min(u64::from(u32::MAX - 1)) as u32;
    let minreq = minreq_frames.saturating_mul(fb).min(u64::from(tlength)) as u32;
    PaBufferAttr {
        maxlength: u32::MAX,
        tlength,
        prebuf: u32::MAX,
        minreq,
        fragsize: u32::MAX,
    }
}

/// Converts a PulseAudio latency in microseconds into whole frames at `sample_rate` Hz using
/// overflow-safe `u128` arithmetic.
pub(super) fn usec_to_frames(usec: u64, sample_rate: u32) -> u64 {
    if usec == 0 || sample_rate == 0 {
        return 0;
    }
    ((u128::from(usec) * u128::from(sample_rate)) / 1_000_000).min(u128::from(u64::MAX)) as u64
}

fn format_pa_error(api: PulseApi, code: c_int) -> String {
    if let Some(strerror) = api.strerror {
        // SAFETY: `pa_strerror` returns a static NUL-terminated string (or NULL) for any error code.
        let ptr = unsafe { strerror(code) };
        if !ptr.is_null() {
            // SAFETY: `ptr` is non-null and NUL-terminated.
            return unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
        }
    }
    format!("PulseAudio error {code}")
}

/// Active `pa_simple` playback stream implementing [`SinkDevice`].
pub(super) struct PulseSink {
    api: PulseApi,
    handle: *mut c_void,
    sample_rate: u32,
    frame_bytes: usize,
}

// SAFETY: `PulseSink` owns its `pa_simple` connection exclusively and is only used from the
// audio output's single worker thread after construction.
unsafe impl Send for PulseSink {}

impl Drop for PulseSink {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // SAFETY: `self.handle` was returned by `pa_simple_new` and is freed exactly once.
            unsafe { (self.api.free)(self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}

pub(super) fn open(spec: &ValidatedSpec) -> Result<Box<dyn SinkDevice>, String> {
    let api = api()?;
    PulseSink::open_with_api(api, spec).map(|sink| Box::new(sink) as Box<dyn SinkDevice>)
}

impl PulseSink {
    pub(super) fn open_with_api(api: PulseApi, spec: &ValidatedSpec) -> Result<Self, String> {
        let app = sanitize_c_string(&spec.app_name, "Rune");
        let stream = sanitize_c_string(&spec.stream_name, "Audio");
        let ss = PaSampleSpec {
            format: pa_format(spec.format),
            rate: spec.sample_rate,
            channels: spec.channels as u8,
        };
        let attr = buffer_attr(spec.sample_rate, spec.frame_bytes);
        let mut err: c_int = 0;
        // SAFETY: all pointers are either NULL or valid for the duration of `pa_simple_new`.
        let handle = unsafe {
            (api.new)(
                std::ptr::null(),
                app.as_ptr(),
                PA_STREAM_PLAYBACK,
                std::ptr::null(),
                stream.as_ptr(),
                &ss,
                std::ptr::null(),
                &attr,
                &mut err,
            )
        };
        if handle.is_null() {
            return Err(format_pa_error(api, err));
        }
        Ok(Self {
            api,
            handle,
            sample_rate: spec.sample_rate,
            frame_bytes: spec.frame_bytes,
        })
    }
}

impl SinkDevice for PulseSink {
    fn backend(&self) -> AudioBackend {
        AudioBackend::PulseAudio
    }

    fn write(&mut self, pcm: &[u8]) -> Result<usize, SinkError> {
        if pcm.is_empty() {
            return Ok(0);
        }
        let mut err: c_int = 0;
        // SAFETY: `self.handle` is live and `pcm` is valid for `pcm.len()` bytes.
        let rc = unsafe { (self.api.write)(self.handle, pcm.as_ptr().cast(), pcm.len(), &mut err) };
        if rc < 0 {
            return Err(SinkError::Lost(format_pa_error(self.api, err)));
        }
        Ok(pcm.len() / self.frame_bytes)
    }

    fn latency_frames(&mut self) -> Result<u64, SinkError> {
        let mut err: c_int = 0;
        // SAFETY: `self.handle` is live and `&mut err` is a valid out-pointer.
        let usec = unsafe { (self.api.get_latency)(self.handle, &mut err) };
        if usec == PA_USEC_INVALID {
            return Err(SinkError::Lost(format_pa_error(self.api, err)));
        }
        Ok(usec_to_frames(usec, self.sample_rate))
    }

    fn flush(&mut self) -> Result<(), SinkError> {
        let mut err: c_int = 0;
        // SAFETY: `self.handle` is live and `&mut err` is a valid out-pointer.
        let rc = unsafe { (self.api.flush)(self.handle, &mut err) };
        if rc < 0 {
            return Err(SinkError::Lost(format_pa_error(self.api, err)));
        }
        Ok(())
    }
}
