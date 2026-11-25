//! ALSA PCM (`libasound.so.2`) fallback backend, loaded at runtime with `dlopen`.

#![allow(unsafe_code)]

use super::{dlopen_library, dlsym_fn, dlsym_opt};
use crate::audio::{AudioBackend, SampleFormat, SinkDevice, SinkError, ValidatedSpec};
use std::ffi::{CStr, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::sync::OnceLock;

/// `SND_PCM_STREAM_PLAYBACK` from `alsa/pcm.h`.
pub(super) const SND_PCM_STREAM_PLAYBACK: c_int = 0;
/// `SND_PCM_FORMAT_S16_LE` from `alsa/pcm.h`.
pub(super) const SND_PCM_FORMAT_S16_LE: c_int = 2;
/// `SND_PCM_FORMAT_FLOAT_LE` from `alsa/pcm.h`.
pub(super) const SND_PCM_FORMAT_FLOAT_LE: c_int = 14;
/// `SND_PCM_ACCESS_RW_INTERLEAVED` from `alsa/pcm.h`.
pub(super) const SND_PCM_ACCESS_RW_INTERLEAVED: c_int = 3;
/// Target ALSA buffer latency in microseconds (60 ms).
pub(super) const ALSA_LATENCY_US: c_uint = 60_000;

pub(super) type SndPcmOpen =
    unsafe extern "C" fn(pcmp: *mut *mut c_void, name: *const c_char, stream: c_int, mode: c_int) -> c_int;
pub(super) type SndPcmClose = unsafe extern "C" fn(pcm: *mut c_void) -> c_int;
pub(super) type SndPcmSetParams = unsafe extern "C" fn(
    pcm: *mut c_void,
    format: c_int,
    access: c_int,
    channels: c_uint,
    rate: c_uint,
    soft_resample: c_int,
    latency: c_uint,
) -> c_int;
pub(super) type SndPcmWritei =
    unsafe extern "C" fn(pcm: *mut c_void, buffer: *const c_void, size: c_ulong) -> c_long;
pub(super) type SndPcmDelay = unsafe extern "C" fn(pcm: *mut c_void, delayp: *mut c_long) -> c_int;
pub(super) type SndPcmDrop = unsafe extern "C" fn(pcm: *mut c_void) -> c_int;
pub(super) type SndPcmPrepare = unsafe extern "C" fn(pcm: *mut c_void) -> c_int;
pub(super) type SndPcmRecover =
    unsafe extern "C" fn(pcm: *mut c_void, err: c_int, silent: c_int) -> c_int;
pub(super) type SndPcmWait = unsafe extern "C" fn(pcm: *mut c_void, timeout: c_int) -> c_int;
pub(super) type SndStrerror = unsafe extern "C" fn(errnum: c_int) -> *const c_char;

/// Resolved `libasound.so.2` function table cached for process lifetime.
#[derive(Clone, Copy)]
pub(super) struct AlsaApi {
    pub(super) open: SndPcmOpen,
    pub(super) close: SndPcmClose,
    pub(super) set_params: SndPcmSetParams,
    pub(super) writei: SndPcmWritei,
    pub(super) delay: SndPcmDelay,
    pub(super) drop: SndPcmDrop,
    pub(super) prepare: SndPcmPrepare,
    pub(super) recover: SndPcmRecover,
    #[allow(dead_code)] // Resolved to validate ABI completeness; writes block on the worker thread.
    pub(super) wait: SndPcmWait,
    pub(super) strerror: Option<SndStrerror>,
}

static ALSA_API: OnceLock<Result<AlsaApi, String>> = OnceLock::new();

fn api() -> Result<AlsaApi, String> {
    ALSA_API.get_or_init(load_api).clone()
}

fn load_api() -> Result<AlsaApi, String> {
    let handle = dlopen_library(c"libasound.so.2")?;
    // SAFETY: `handle` was loaded from `libasound.so.2` and each symbol matches its documented
    // ALSA C ABI signature in `alsa/pcm.h` / `alsa/error.h`.
    unsafe {
        Ok(AlsaApi {
            open: dlsym_fn(handle, c"snd_pcm_open")?,
            close: dlsym_fn(handle, c"snd_pcm_close")?,
            set_params: dlsym_fn(handle, c"snd_pcm_set_params")?,
            writei: dlsym_fn(handle, c"snd_pcm_writei")?,
            delay: dlsym_fn(handle, c"snd_pcm_delay")?,
            drop: dlsym_fn(handle, c"snd_pcm_drop")?,
            prepare: dlsym_fn(handle, c"snd_pcm_prepare")?,
            recover: dlsym_fn(handle, c"snd_pcm_recover")?,
            wait: dlsym_fn(handle, c"snd_pcm_wait")?,
            strerror: dlsym_opt(handle, c"snd_strerror"),
        })
    }
}

pub(super) fn alsa_format(format: SampleFormat) -> c_int {
    match format {
        SampleFormat::I16 => SND_PCM_FORMAT_S16_LE,
        SampleFormat::F32 => SND_PCM_FORMAT_FLOAT_LE,
    }
}

fn format_alsa_error(api: AlsaApi, code: c_int) -> String {
    if let Some(strerror) = api.strerror {
        // SAFETY: `snd_strerror` returns a static NUL-terminated string (or NULL) for any code.
        let ptr = unsafe { strerror(code) };
        if !ptr.is_null() {
            // SAFETY: `ptr` is non-null and NUL-terminated.
            return unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
        }
    }
    format!("ALSA error {code}")
}

/// Active `snd_pcm_t` playback handle implementing [`SinkDevice`].
pub(super) struct AlsaSink {
    api: AlsaApi,
    handle: *mut c_void,
    frame_bytes: usize,
}

// SAFETY: `AlsaSink` owns its `snd_pcm_t` handle exclusively and is only used from the audio
// output's single worker thread after construction.
unsafe impl Send for AlsaSink {}

impl Drop for AlsaSink {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // SAFETY: `self.handle` was returned by `snd_pcm_open` and is closed exactly once.
            unsafe {
                let _ = (self.api.drop)(self.handle);
                let _ = (self.api.close)(self.handle);
            }
            self.handle = std::ptr::null_mut();
        }
    }
}

pub(super) fn open(spec: &ValidatedSpec) -> Result<Box<dyn SinkDevice>, String> {
    let api = api()?;
    AlsaSink::open_with_api(api, spec).map(|sink| Box::new(sink) as Box<dyn SinkDevice>)
}

impl AlsaSink {
    pub(super) fn open_with_api(api: AlsaApi, spec: &ValidatedSpec) -> Result<Self, String> {
        let mut handle: *mut c_void = std::ptr::null_mut();
        // SAFETY: `&mut handle` is a valid out-pointer and `"default"` is NUL-terminated.
        let rc = unsafe { (api.open)(&mut handle, c"default".as_ptr(), SND_PCM_STREAM_PLAYBACK, 0) };
        if rc < 0 || handle.is_null() {
            return Err(format_alsa_error(api, rc));
        }
        // SAFETY: `handle` was opened above and is closed immediately if `set_params` fails.
        let set_rc = unsafe {
            (api.set_params)(
                handle,
                alsa_format(spec.format),
                SND_PCM_ACCESS_RW_INTERLEAVED,
                c_uint::from(spec.channels),
                spec.sample_rate as c_uint,
                1,
                ALSA_LATENCY_US,
            )
        };
        if set_rc < 0 {
            // SAFETY: `handle` is live and owned by this function.
            unsafe {
                let _ = (api.close)(handle);
            }
            return Err(format_alsa_error(api, set_rc));
        }
        Ok(Self {
            api,
            handle,
            frame_bytes: spec.frame_bytes,
        })
    }
}

impl SinkDevice for AlsaSink {
    fn backend(&self) -> AudioBackend {
        AudioBackend::Alsa
    }

    fn write(&mut self, pcm: &[u8]) -> Result<usize, SinkError> {
        let frames = pcm.len() / self.frame_bytes;
        if frames == 0 {
            return Ok(0);
        }
        // SAFETY: `self.handle` is live and `pcm` holds `frames` interleaved frames.
        let rc = unsafe { (self.api.writei)(self.handle, pcm.as_ptr().cast(), frames as c_ulong) };
        if rc >= 0 {
            return Ok((rc as usize).min(frames));
        }
        let err = rc as c_int;
        if err == -libc::EPIPE {
            // Recoverable xrun: prepare the stream and signal `SinkError::Underrun` so the
            // worker emits `AudioEvent::Underrun` once and retries the uncommitted frames.
            // SAFETY: `self.handle` is live.
            let prep = unsafe { (self.api.prepare)(self.handle) };
            if prep < 0 {
                return Err(SinkError::Lost(format_alsa_error(self.api, prep)));
            }
            return Err(SinkError::Underrun);
        }
        // SAFETY: `self.handle` is live; `silent = 1` suppresses ALSA stderr output.
        let rec = unsafe { (self.api.recover)(self.handle, err, 1) };
        if rec == 0 {
            return Ok(0);
        }
        Err(SinkError::Lost(format_alsa_error(self.api, err)))
    }

    fn latency_frames(&mut self) -> Result<u64, SinkError> {
        let mut delay: c_long = 0;
        // SAFETY: `self.handle` is live and `&mut delay` is a valid out-pointer.
        let rc = unsafe { (self.api.delay)(self.handle, &mut delay) };
        if rc == 0 {
            return Ok(delay.max(0) as u64);
        }
        if rc == -libc::EPIPE {
            // SAFETY: `self.handle` is live.
            let prep = unsafe { (self.api.prepare)(self.handle) };
            if prep < 0 {
                return Err(SinkError::Lost(format_alsa_error(self.api, prep)));
            }
            return Err(SinkError::Underrun);
        }
        // SAFETY: `self.handle` is live; `silent = 1` suppresses stderr noise.
        let rec = unsafe { (self.api.recover)(self.handle, rc, 1) };
        if rec == 0 {
            return Ok(0);
        }
        Err(SinkError::Lost(format_alsa_error(self.api, rc)))
    }

    fn flush(&mut self) -> Result<(), SinkError> {
        // SAFETY: `self.handle` is live; `snd_pcm_drop` discards queued samples immediately and
        // `snd_pcm_prepare` leaves the stream ready for the next write.
        let drop_rc = unsafe { (self.api.drop)(self.handle) };
        if drop_rc < 0 {
            return Err(SinkError::Lost(format_alsa_error(self.api, drop_rc)));
        }
        // SAFETY: `self.handle` is live.
        let prep_rc = unsafe { (self.api.prepare)(self.handle) };
        if prep_rc < 0 {
            return Err(SinkError::Lost(format_alsa_error(self.api, prep_rc)));
        }
        Ok(())
    }
}
