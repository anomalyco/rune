#![allow(unsafe_code)]

use super::alsa::{
    ALSA_LATENCY_US, AlsaApi, AlsaSink, SND_PCM_ACCESS_RW_INTERLEAVED, SND_PCM_FORMAT_FLOAT_LE,
    SND_PCM_FORMAT_S16_LE, alsa_format,
};
use super::pulse::{
    PA_SAMPLE_FLOAT32LE, PA_SAMPLE_S16LE, PA_STREAM_PLAYBACK, PA_USEC_INVALID, PaBufferAttr,
    PaSampleSpec, PulseApi, PulseSink, buffer_attr, pa_format, usec_to_frames,
};
use super::{dlopen_library, dlsym_fn, open_with, sanitize_c_string};
use crate::audio::{
    AudioBackend, AudioOutputError, AudioOutputSpec, SampleFormat, SinkDevice, SinkError,
    ValidatedSpec,
};
use std::ffi::{CStr, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::sync::Mutex;

fn test_spec(format: SampleFormat) -> ValidatedSpec {
    AudioOutputSpec::new(format, 48_000, 2)
        .app_name("Strata\0Evil")
        .stream_name("Preview\0Audio")
        .validate()
        .unwrap()
}

// --- PulseAudio fake symbol table ---

#[derive(Default)]
struct FakePulse {
    fail_new: Option<c_int>,
    fail_write: Option<c_int>,
    fail_latency: Option<c_int>,
    fail_flush: Option<c_int>,
    latency_usec: u64,
    last_app: String,
    last_stream: String,
    last_dir: c_int,
    last_ss: Option<PaSampleSpec>,
    last_attr: Option<PaBufferAttr>,
    written_bytes: usize,
    flushes: usize,
    frees: usize,
}

static FAKE_PULSE: Mutex<Option<FakePulse>> = Mutex::new(None);
static PULSE_LOCK: Mutex<()> = Mutex::new(());

fn with_fake_pulse<R>(init: FakePulse, f: impl FnOnce(PulseApi) -> R) -> (R, FakePulse) {
    let _serial = PULSE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    *FAKE_PULSE.lock().unwrap() = Some(init);
    let api = PulseApi {
        new: fake_pa_new,
        write: fake_pa_write,
        get_latency: fake_pa_get_latency,
        flush: fake_pa_flush,
        drain: fake_pa_drain,
        free: fake_pa_free,
        strerror: Some(fake_pa_strerror),
    };
    let out = f(api);
    let final_state = FAKE_PULSE.lock().unwrap().take().unwrap();
    (out, final_state)
}

unsafe extern "C" fn fake_pa_new(
    _server: *const c_char,
    name: *const c_char,
    dir: c_int,
    _dev: *const c_char,
    stream_name: *const c_char,
    ss: *const PaSampleSpec,
    _map: *const c_void,
    attr: *const PaBufferAttr,
    error: *mut c_int,
) -> *mut c_void {
    let mut g = FAKE_PULSE.lock().unwrap();
    let st = g.as_mut().unwrap();
    // SAFETY: test pointers passed by `PulseSink::open_with_api` are valid C strings and structs.
    unsafe {
        st.last_app = CStr::from_ptr(name).to_string_lossy().into_owned();
        st.last_stream = CStr::from_ptr(stream_name).to_string_lossy().into_owned();
        st.last_dir = dir;
        st.last_ss = Some(*ss);
        st.last_attr = Some(*attr);
    }
    if let Some(code) = st.fail_new {
        if !error.is_null() {
            // SAFETY: valid out-pointer.
            unsafe { *error = code };
        }
        return std::ptr::null_mut();
    }
    0x1000 as *mut c_void
}

unsafe extern "C" fn fake_pa_write(
    _s: *mut c_void,
    _data: *const c_void,
    bytes: usize,
    error: *mut c_int,
) -> c_int {
    let mut g = FAKE_PULSE.lock().unwrap();
    let st = g.as_mut().unwrap();
    if let Some(code) = st.fail_write.take() {
        if !error.is_null() {
            // SAFETY: valid out-pointer.
            unsafe { *error = code };
        }
        return -1;
    }
    st.written_bytes += bytes;
    0
}

unsafe extern "C" fn fake_pa_get_latency(_s: *mut c_void, error: *mut c_int) -> u64 {
    let mut g = FAKE_PULSE.lock().unwrap();
    let st = g.as_mut().unwrap();
    if let Some(code) = st.fail_latency.take() {
        if !error.is_null() {
            // SAFETY: valid out-pointer.
            unsafe { *error = code };
        }
        return PA_USEC_INVALID;
    }
    st.latency_usec
}

unsafe extern "C" fn fake_pa_flush(_s: *mut c_void, error: *mut c_int) -> c_int {
    let mut g = FAKE_PULSE.lock().unwrap();
    let st = g.as_mut().unwrap();
    if let Some(code) = st.fail_flush.take() {
        if !error.is_null() {
            // SAFETY: valid out-pointer.
            unsafe { *error = code };
        }
        return -1;
    }
    st.flushes += 1;
    0
}

unsafe extern "C" fn fake_pa_drain(_s: *mut c_void, _error: *mut c_int) -> c_int {
    panic!("pa_simple_drain must never be called by PulseSink");
}

unsafe extern "C" fn fake_pa_free(_s: *mut c_void) {
    if let Some(st) = FAKE_PULSE.lock().unwrap().as_mut() {
        st.frees += 1;
    }
}

unsafe extern "C" fn fake_pa_strerror(code: c_int) -> *const c_char {
    match code {
        11 => c"connection terminated".as_ptr(),
        _ => c"fake pulse error".as_ptr(),
    }
}

// --- ALSA fake symbol table ---

#[derive(Default)]
struct FakeAlsa {
    fail_open: Option<c_int>,
    fail_set_params: Option<c_int>,
    write_rc: Option<c_long>,
    delay_rc: c_int,
    delay_val: c_long,
    drop_rc: c_int,
    prepare_rc: c_int,
    recover_rc: c_int,
    last_params: Option<(c_int, c_int, c_uint, c_uint, c_int, c_uint)>,
    writes: usize,
    drops: usize,
    prepares: usize,
    recovers: usize,
    closes: usize,
}

static FAKE_ALSA: Mutex<Option<FakeAlsa>> = Mutex::new(None);
static ALSA_LOCK: Mutex<()> = Mutex::new(());

fn with_fake_alsa<R>(init: FakeAlsa, f: impl FnOnce(AlsaApi) -> R) -> (R, FakeAlsa) {
    let _serial = ALSA_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    *FAKE_ALSA.lock().unwrap() = Some(init);
    let api = AlsaApi {
        open: fake_snd_open,
        close: fake_snd_close,
        set_params: fake_snd_set_params,
        writei: fake_snd_writei,
        delay: fake_snd_delay,
        drop: fake_snd_drop,
        prepare: fake_snd_prepare,
        recover: fake_snd_recover,
        wait: fake_snd_wait,
        strerror: Some(fake_snd_strerror),
    };
    let out = f(api);
    let final_state = FAKE_ALSA.lock().unwrap().take().unwrap();
    (out, final_state)
}

unsafe extern "C" fn fake_snd_open(
    pcmp: *mut *mut c_void,
    _name: *const c_char,
    _stream: c_int,
    _mode: c_int,
) -> c_int {
    let g = FAKE_ALSA.lock().unwrap();
    let st = g.as_ref().unwrap();
    if let Some(rc) = st.fail_open {
        return rc;
    }
    // SAFETY: valid out-pointer from `AlsaSink::open_with_api`.
    unsafe { *pcmp = 0x2000 as *mut c_void };
    0
}

unsafe extern "C" fn fake_snd_close(_pcm: *mut c_void) -> c_int {
    if let Some(st) = FAKE_ALSA.lock().unwrap().as_mut() {
        st.closes += 1;
    }
    0
}

unsafe extern "C" fn fake_snd_set_params(
    _pcm: *mut c_void,
    format: c_int,
    access: c_int,
    channels: c_uint,
    rate: c_uint,
    soft_resample: c_int,
    latency: c_uint,
) -> c_int {
    let mut g = FAKE_ALSA.lock().unwrap();
    let st = g.as_mut().unwrap();
    st.last_params = Some((format, access, channels, rate, soft_resample, latency));
    st.fail_set_params.unwrap_or(0)
}

unsafe extern "C" fn fake_snd_writei(
    _pcm: *mut c_void,
    _buffer: *const c_void,
    size: c_ulong,
) -> c_long {
    let mut g = FAKE_ALSA.lock().unwrap();
    let st = g.as_mut().unwrap();
    st.writes += 1;
    st.write_rc.take().unwrap_or(size as c_long)
}

unsafe extern "C" fn fake_snd_delay(_pcm: *mut c_void, delayp: *mut c_long) -> c_int {
    let g = FAKE_ALSA.lock().unwrap();
    let st = g.as_ref().unwrap();
    if st.delay_rc == 0 {
        // SAFETY: valid out-pointer.
        unsafe { *delayp = st.delay_val };
    }
    st.delay_rc
}

unsafe extern "C" fn fake_snd_drop(_pcm: *mut c_void) -> c_int {
    if let Some(st) = FAKE_ALSA.lock().unwrap().as_mut() {
        st.drops += 1;
        return st.drop_rc;
    }
    0
}

unsafe extern "C" fn fake_snd_prepare(_pcm: *mut c_void) -> c_int {
    let mut g = FAKE_ALSA.lock().unwrap();
    let st = g.as_mut().unwrap();
    st.prepares += 1;
    st.prepare_rc
}

unsafe extern "C" fn fake_snd_recover(_pcm: *mut c_void, _err: c_int, _silent: c_int) -> c_int {
    let mut g = FAKE_ALSA.lock().unwrap();
    let st = g.as_mut().unwrap();
    st.recovers += 1;
    st.recover_rc
}

unsafe extern "C" fn fake_snd_wait(_pcm: *mut c_void, _timeout: c_int) -> c_int {
    1
}

unsafe extern "C" fn fake_snd_strerror(code: c_int) -> *const c_char {
    if code == -libc::ENODEV {
        c"No such device".as_ptr()
    } else if code == -libc::EPIPE {
        c"Broken pipe".as_ptr()
    } else {
        c"fake alsa error".as_ptr()
    }
}

// --- Unit tests ---

#[test]
fn sanitize_c_string_strips_nuls_and_falls_back_when_empty() {
    assert_eq!(sanitize_c_string("Strata\0Audio", "Rune").to_bytes(), b"StrataAudio");
    assert_eq!(sanitize_c_string("\0\0", "Rune").to_bytes(), b"Rune");
    assert_eq!(sanitize_c_string("", "Audio").to_bytes(), b"Audio");
}

#[test]
fn pulse_buffer_attr_and_usec_to_frames_math_are_aligned_and_exact() {
    assert_eq!(pa_format(SampleFormat::I16), PA_SAMPLE_S16LE);
    assert_eq!(pa_format(SampleFormat::F32), PA_SAMPLE_FLOAT32LE);

    // 48 kHz stereo I16 (4 B/frame): 60 ms = 2880 frames = 11_520 B; 10 ms = 480 frames = 1_920 B.
    let attr = buffer_attr(48_000, 4);
    assert_eq!(attr.maxlength, u32::MAX);
    assert_eq!(attr.prebuf, u32::MAX);
    assert_eq!(attr.fragsize, u32::MAX);
    assert_eq!(attr.tlength, 2_880 * 4);
    assert_eq!(attr.minreq, 480 * 4);
    assert_eq!(attr.tlength % 4, 0);
    assert_eq!(attr.minreq % 4, 0);

    // Latency usec -> frames with u128 math (no overflow even for huge timestamps).
    assert_eq!(usec_to_frames(0, 48_000), 0);
    assert_eq!(usec_to_frames(60_000, 48_000), 2_880);
    assert_eq!(usec_to_frames(1_000_000, 44_100), 44_100);
    assert_eq!(usec_to_frames(10_000, 192_000), 1_920);
    // Overflow-safe u128 intermediate math (would overflow u64 before dividing by 1_000_000).
    let expected = ((u128::from(u64::MAX / 2) * 192_000) / 1_000_000) as u64;
    assert_eq!(usec_to_frames(u64::MAX / 2, 192_000), expected);
    // Rates > 1_000_000 clamp cleanly at u64::MAX without panicking.
    assert_eq!(usec_to_frames(u64::MAX, 2_000_000), u64::MAX);
}

#[test]
fn dlopen_missing_library_and_dlsym_missing_symbol_return_clear_errors() {
    let err = dlopen_library(c"libnonexistent_rune_audio_test_9477.so.0").unwrap_err();
    assert!(err.contains("libnonexistent_rune_audio_test_9477.so.0"), "{err}");

    // `dlopen(NULL, ...)` opens the current process image; a missing symbol must fail cleanly.
    // SAFETY: `dlopen(NULL, RTLD_LAZY)` returns a valid handle for the current executable.
    let self_handle = unsafe { libc::dlopen(std::ptr::null(), libc::RTLD_LAZY) };
    assert!(!self_handle.is_null());
    // SAFETY: `self_handle` is valid; symbol lookup fails with NULL.
    let sym_err: Result<unsafe extern "C" fn(), String> =
        unsafe { dlsym_fn(self_handle, c"rune_definitely_missing_audio_symbol_9477") };
    let msg = sym_err.unwrap_err();
    assert!(msg.contains("rune_definitely_missing_audio_symbol_9477"), "{msg}");
}

#[test]
fn pulse_sink_handles_open_write_latency_flush_errors_and_drop() {
    let spec = test_spec(SampleFormat::I16);

    // Open failure reports PulseAudio error string.
    let (res, st) = with_fake_pulse(FakePulse { fail_new: Some(11), ..Default::default() }, |api| {
        PulseSink::open_with_api(api, &spec).err()
    });
    assert_eq!(res.as_deref(), Some("connection terminated"));
    assert_eq!(st.last_app, "StrataEvil");
    assert_eq!(st.last_stream, "PreviewAudio");
    assert_eq!(st.last_dir, PA_STREAM_PLAYBACK);
    assert_eq!(st.last_ss, Some(PaSampleSpec { format: PA_SAMPLE_S16LE, rate: 48_000, channels: 2 }));
    assert_eq!(st.frees, 0);

    // Happy path + write/latency/flush failure -> DeviceLost + Drop frees once.
    let (_, st) = with_fake_pulse(FakePulse { latency_usec: 25_000, ..Default::default() }, |api| {
        let mut sink = PulseSink::open_with_api(api, &spec).unwrap();
        assert_eq!(sink.backend(), AudioBackend::PulseAudio);
        assert_eq!(sink.write(&[0u8; 480 * 4]), Ok(480));
        assert_eq!(sink.latency_frames(), Ok(1_200));
        assert_eq!(sink.flush(), Ok(()));

        FAKE_PULSE.lock().unwrap().as_mut().unwrap().fail_write = Some(11);
        assert_eq!(sink.write(&[0u8; 4]), Err(SinkError::Lost("connection terminated".into())));

        FAKE_PULSE.lock().unwrap().as_mut().unwrap().fail_latency = Some(11);
        assert_eq!(sink.latency_frames(), Err(SinkError::Lost("connection terminated".into())));

        FAKE_PULSE.lock().unwrap().as_mut().unwrap().fail_flush = Some(11);
        assert_eq!(sink.flush(), Err(SinkError::Lost("connection terminated".into())));
    });
    assert_eq!(st.written_bytes, 480 * 4);
    assert_eq!(st.flushes, 1);
    assert_eq!(st.frees, 1);
}

#[test]
fn alsa_sink_handles_open_partial_write_epipe_recovery_enodev_and_drop() {
    assert_eq!(alsa_format(SampleFormat::I16), SND_PCM_FORMAT_S16_LE);
    assert_eq!(alsa_format(SampleFormat::F32), SND_PCM_FORMAT_FLOAT_LE);

    let spec = test_spec(SampleFormat::F32);

    // set_params failure closes the opened handle without leaking.
    let (err, st) = with_fake_alsa(
        FakeAlsa { fail_set_params: Some(-libc::EINVAL), ..Default::default() },
        |api| AlsaSink::open_with_api(api, &spec).err(),
    );
    assert!(err.is_some());
    assert_eq!(st.closes, 1);
    assert_eq!(
        st.last_params,
        Some((SND_PCM_FORMAT_FLOAT_LE, SND_PCM_ACCESS_RW_INTERLEAVED, 2, 48_000, 1, ALSA_LATENCY_US))
    );

    // Partial write, -EPIPE recoverable Underrun + prepare, -ENODEV unrecoverable Lost, negative delay clamped, flush + drop.
    let (_, st) = with_fake_alsa(FakeAlsa { delay_val: 960, ..Default::default() }, |api| {
        let mut sink = AlsaSink::open_with_api(api, &spec).unwrap();
        assert_eq!(sink.backend(), AudioBackend::Alsa);

        // Partial write: device accepts 200 of 480 frames.
        FAKE_ALSA.lock().unwrap().as_mut().unwrap().write_rc = Some(200);
        assert_eq!(sink.write(&[0u8; 480 * 8]), Ok(200));

        // -EPIPE on write -> snd_pcm_prepare + recoverable SinkError::Underrun.
        FAKE_ALSA.lock().unwrap().as_mut().unwrap().write_rc = Some(-(libc::EPIPE as c_long));
        assert_eq!(sink.write(&[0u8; 480 * 8]), Err(SinkError::Underrun));

        // -EINTR on write with recover == 0 -> Ok(0) retry without error.
        {
            let mut g = FAKE_ALSA.lock().unwrap();
            let s = g.as_mut().unwrap();
            s.write_rc = Some(-(libc::EINTR as c_long));
            s.recover_rc = 0;
        }
        assert_eq!(sink.write(&[0u8; 480 * 8]), Ok(0));

        // -ENODEV with recover < 0 -> SinkError::Lost.
        {
            let mut g = FAKE_ALSA.lock().unwrap();
            let s = g.as_mut().unwrap();
            s.write_rc = Some(-(libc::ENODEV as c_long));
            s.recover_rc = -libc::ENODEV;
        }
        assert_eq!(sink.write(&[0u8; 480 * 8]), Err(SinkError::Lost("No such device".into())));

        // Latency positive and negative clamped to 0.
        assert_eq!(sink.latency_frames(), Ok(960));
        FAKE_ALSA.lock().unwrap().as_mut().unwrap().delay_val = -12;
        assert_eq!(sink.latency_frames(), Ok(0));

        // Flush calls drop + prepare.
        assert_eq!(sink.flush(), Ok(()));
    });
    assert_eq!(st.prepares, 2); // 1 from -EPIPE + 1 from flush
    assert_eq!(st.drops, 2); // 1 from flush + 1 from Drop
    assert_eq!(st.closes, 1);
}

#[test]
fn probe_falls_back_from_pulse_to_alsa_and_reports_both_errors_when_both_fail() {
    let spec = test_spec(SampleFormat::I16);

    // PulseAudio fails -> falls back to ALSA.
    let (backend, st) = with_fake_alsa(FakeAlsa::default(), |alsa_api| {
        let dev = open_with(
            &spec,
            |_| Err("no pulse server".into()),
            |s| AlsaSink::open_with_api(alsa_api, s).map(|d| Box::new(d) as Box<dyn SinkDevice>),
        )
        .unwrap();
        dev.backend()
    });
    assert_eq!(backend, AudioBackend::Alsa);
    assert_eq!(st.drops, 1);
    assert_eq!(st.closes, 1);

    // Both fail -> OpenFailed names both reasons.
    let err = open_with(
        &spec,
        |_| Err("libpulse-simple.so.0 missing".into()),
        |_| Err("libasound.so.2 missing".into()),
    )
    .err()
    .unwrap();
    assert_eq!(
        err,
        AudioOutputError::OpenFailed(
            "PulseAudio: libpulse-simple.so.0 missing; ALSA: libasound.so.2 missing".into()
        )
    );
}

#[test]
fn missing_optional_strerror_and_alsa_delay_and_flush_failures_format_cleanly() {
    let spec = test_spec(SampleFormat::I16);

    // PulseAudio without optional pa_strerror falls back to numeric error code.
    let (pa_err, _) = with_fake_pulse(FakePulse { fail_new: Some(42), ..Default::default() }, |mut api| {
        api.strerror = None;
        PulseSink::open_with_api(api, &spec).err()
    });
    assert_eq!(pa_err.as_deref(), Some("PulseAudio error 42"));

    // ALSA without optional snd_strerror + delay -EPIPE / -ENODEV + flush drop/prepare failure.
    let (_, st) = with_fake_alsa(FakeAlsa::default(), |mut api| {
        api.strerror = None;
        let mut sink = AlsaSink::open_with_api(api, &spec).unwrap();

        FAKE_ALSA.lock().unwrap().as_mut().unwrap().delay_rc = -libc::EPIPE;
        assert_eq!(sink.latency_frames(), Err(SinkError::Underrun));

        {
            let mut g = FAKE_ALSA.lock().unwrap();
            let s = g.as_mut().unwrap();
            s.delay_rc = -libc::ENODEV;
            s.recover_rc = -libc::ENODEV;
        }
        assert_eq!(
            sink.latency_frames(),
            Err(SinkError::Lost(format!("ALSA error {}", -libc::ENODEV)))
        );

        FAKE_ALSA.lock().unwrap().as_mut().unwrap().drop_rc = -libc::EIO;
        assert_eq!(
            sink.flush(),
            Err(SinkError::Lost(format!("ALSA error {}", -libc::EIO)))
        );

        {
            let mut g = FAKE_ALSA.lock().unwrap();
            let s = g.as_mut().unwrap();
            s.drop_rc = 0;
            s.prepare_rc = -libc::EIO;
        }
        assert_eq!(
            sink.flush(),
            Err(SinkError::Lost(format!("ALSA error {}", -libc::EIO)))
        );
    });
    assert_eq!(st.closes, 1);
}
