//! Linux runtime-`dlopen` audio output backends behind [`SinkDevice`](super::SinkDevice):
//! probes PulseAudio's simple API (`libpulse-simple.so.0`, which also serves PipeWire via
//! `pipewire-pulse`), then falls back to ALSA (`libasound.so.2`). Neither library is linked at
//! build time; missing libraries or unreachable daemons return [`AudioOutputError::OpenFailed`].

#![allow(unsafe_code)]

mod alsa;
mod pulse;
#[cfg(test)]
mod tests;

use super::{AudioOutputError, SinkDevice, ValidatedSpec};
use std::ffi::{CStr, CString, c_void};

/// Opens the best available Linux audio output sink for `spec`: PulseAudio first, then ALSA.
pub(in crate::audio) fn open(spec: &ValidatedSpec) -> Result<Box<dyn SinkDevice>, AudioOutputError> {
    open_with(spec, pulse::open, alsa::open)
}

pub(super) fn open_with(
    spec: &ValidatedSpec,
    try_pulse: impl FnOnce(&ValidatedSpec) -> Result<Box<dyn SinkDevice>, String>,
    try_alsa: impl FnOnce(&ValidatedSpec) -> Result<Box<dyn SinkDevice>, String>,
) -> Result<Box<dyn SinkDevice>, AudioOutputError> {
    let pulse_err = match try_pulse(spec) {
        Ok(device) => return Ok(device),
        Err(err) => err,
    };
    match try_alsa(spec) {
        Ok(device) => Ok(device),
        Err(alsa_err) => Err(AudioOutputError::OpenFailed(format!(
            "PulseAudio: {pulse_err}; ALSA: {alsa_err}"
        ))),
    }
}

/// Lazily loads `soname` with `RTLD_LAZY | RTLD_LOCAL` for the lifetime of the process (never
/// `dlclose`d).
pub(super) fn dlopen_library(soname: &CStr) -> Result<*mut c_void, String> {
    // SAFETY: `soname` is a NUL-terminated C string; `RTLD_LAZY | RTLD_LOCAL` keeps loaded
    // symbols out of the global namespace.
    let handle = unsafe { libc::dlopen(soname.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
    if handle.is_null() {
        return Err(format!("could not load {}: {}", soname.to_string_lossy(), last_dlerror()));
    }
    Ok(handle)
}

/// Resolves a required function pointer `symbol` from a live `dlopen` handle.
///
/// # Safety
/// `handle` must be a non-null handle returned by [`dlopen_library`], and `F` must match the
/// exact C ABI signature of `symbol`.
pub(super) unsafe fn dlsym_fn<F: Copy>(handle: *mut c_void, symbol: &CStr) -> Result<F, String> {
    debug_assert_eq!(size_of::<F>(), size_of::<*mut c_void>());
    // SAFETY: `handle` is valid for the lifetime of the process and `symbol` is NUL-terminated.
    let ptr = unsafe { libc::dlsym(handle, symbol.as_ptr()) };
    if ptr.is_null() {
        return Err(format!("missing symbol {}: {}", symbol.to_string_lossy(), last_dlerror()));
    }
    // SAFETY: caller guarantees `F` matches `symbol`'s ABI and `ptr` is non-null.
    Ok(unsafe { std::mem::transmute_copy(&ptr) })
}

/// Resolves an optional function pointer `symbol` from a live `dlopen` handle.
///
/// # Safety
/// Same requirements as [`dlsym_fn`].
pub(super) unsafe fn dlsym_opt<F: Copy>(handle: *mut c_void, symbol: &CStr) -> Option<F> {
    debug_assert_eq!(size_of::<F>(), size_of::<*mut c_void>());
    // SAFETY: `handle` is valid for the lifetime of the process and `symbol` is NUL-terminated.
    let ptr = unsafe { libc::dlsym(handle, symbol.as_ptr()) };
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller guarantees `F` matches `symbol`'s ABI and `ptr` is non-null.
    Some(unsafe { std::mem::transmute_copy(&ptr) })
}

/// Strips interior NUL bytes from `input` so it can be passed to C APIs safely, falling back to
/// `fallback` when empty.
pub(super) fn sanitize_c_string(input: &str, fallback: &str) -> CString {
    let cleaned: Vec<u8> = input.bytes().filter(|&b| b != 0).collect();
    if !cleaned.is_empty()
        && let Ok(cs) = CString::new(cleaned)
    {
        return cs;
    }
    let fb: Vec<u8> = fallback.bytes().filter(|&b| b != 0).collect();
    CString::new(fb).unwrap_or_default()
}

fn last_dlerror() -> String {
    // SAFETY: `dlerror` returns either NULL or a thread-local NUL-terminated error string.
    let ptr = unsafe { libc::dlerror() };
    if ptr.is_null() {
        "unknown dynamic loader error".to_owned()
    } else {
        // SAFETY: `ptr` is non-null and NUL-terminated for the duration of this read.
        unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
    }
}
